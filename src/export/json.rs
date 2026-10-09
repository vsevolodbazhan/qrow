//! JSON objects serialized one row at a time, preserving decimal digits.
use super::{
    Table, check_cancelled,
    value::{self, Kind, Value},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{
    Serialize,
    ser::{SerializeMap, SerializeSeq},
};
use std::{
    io::{self, Write},
    sync::atomic::AtomicBool,
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Options {
    pub pretty: bool,
    pub typed: bool,
    pub decimals_as_numbers: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            pretty: true,
            typed: true,
            decimals_as_numbers: false,
        }
    }
}

pub fn write(
    out: &mut impl Write,
    table: &Table<'_>,
    options: &Options,
    lines: bool,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    super::validate_table(table)?;
    check_cancelled(cancel)?;
    let names = value::column_names(
        table
            .column_indices
            .clone()
            .map(|i| table.columns[i].name.as_str()),
    );
    let kinds: Vec<_> = table
        .column_indices
        .clone()
        .map(|i| Kind::of(&table.columns[i].data_type))
        .collect();
    if lines {
        for row in table.row_indices.clone() {
            check_cancelled(cancel)?;
            serde_json::to_writer(
                &mut *out,
                &Object {
                    table,
                    row,
                    names: &names,
                    kinds: &kinds,
                    options,
                },
            )?;
            out.write_all(b"\n")?;
        }
    } else {
        let array = Array {
            table,
            names: &names,
            kinds: &kinds,
            options,
            cancel,
        };
        if options.pretty {
            serde_json::to_writer_pretty(&mut *out, &array)?;
        } else {
            serde_json::to_writer(&mut *out, &array)?;
        }
        out.write_all(b"\n")?;
    }
    out.flush()?;
    check_cancelled(cancel)?;
    Ok(table.row_count())
}
struct Array<'a> {
    table: &'a Table<'a>,
    names: &'a [String],
    kinds: &'a [Kind],
    options: &'a Options,
    cancel: &'a AtomicBool,
}
impl Serialize for Array<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.table.row_count()))?;
        for row in self.table.row_indices.clone() {
            check_cancelled(self.cancel).map_err(serde::ser::Error::custom)?;
            seq.serialize_element(&Object {
                table: self.table,
                row,
                names: self.names,
                kinds: self.kinds,
                options: self.options,
            })?;
        }
        seq.end()
    }
}
struct Object<'a> {
    table: &'a Table<'a>,
    row: usize,
    names: &'a [String],
    kinds: &'a [Kind],
    options: &'a Options,
}
impl Serialize for Object<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.names.len()))?;
        for (index, text) in self.table.values(self.row).enumerate() {
            let result = if self.options.typed {
                let value = value::normalize(self.kinds[index], text).map_err(|error| {
                    serde::ser::Error::custom(format!(
                        "Column {}, row {}, value {:?}: {error}",
                        self.names[index],
                        self.row + 1,
                        text.map(|text| text.chars().take(100).collect::<String>())
                    ))
                })?;
                match value {
                    Value::Null => map.serialize_entry(&self.names[index], &Option::<bool>::None),
                    Value::Boolean(value) => map.serialize_entry(&self.names[index], &value),
                    Value::Integer(value) => map.serialize_entry(&self.names[index], &value),
                    Value::Float(value) if value.is_finite() => {
                        map.serialize_entry(&self.names[index], &value)
                    }
                    Value::Float(_) => map.serialize_entry(&self.names[index], &text),
                    Value::Decimal(value)
                        if self.options.decimals_as_numbers && !special(value) =>
                    {
                        let number: serde_json::Number =
                            value.parse().map_err(serde::ser::Error::custom)?;
                        map.serialize_entry(&self.names[index], &number)
                    }
                    Value::Decimal(value) | Value::Text(value) => {
                        map.serialize_entry(&self.names[index], &value)
                    }
                    Value::Bytes(value) => {
                        map.serialize_entry(&self.names[index], &STANDARD.encode(value))
                    }
                    Value::Nested(value) => map.serialize_entry(&self.names[index], &value),
                }
            } else {
                map.serialize_entry(&self.names[index], &text)
            };
            result.map_err(|error| {
                serde::ser::Error::custom(format!(
                    "Column {}, row {}: {error}",
                    self.names[index],
                    self.row + 1
                ))
            })?;
        }
        map.end()
    }
}
fn special(value: &str) -> bool {
    matches!(
        value,
        "NaN" | "Infinity" | "+Infinity" | "-Infinity" | "inf" | "-inf"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{export::Rows, model::Column};
    fn table<'a>(columns: &'a [Column], rows: &'a Rows) -> Table<'a> {
        Table {
            columns,
            rows,
            row_indices: 0..rows.len(),
            column_indices: 0..=columns.len() - 1,
        }
    }
    #[test]
    fn typed_json_keeps_exact_decimal_digits_special_numbers_and_unique_keys() {
        let columns: Vec<_> = [
            ("id", "int8"),
            ("id", "numeric"),
            ("id_2", "bool"),
            ("special", "double"),
            ("nested", "jsonb"),
            ("absent", "text"),
        ]
        .into_iter()
        .map(|(name, data_type)| Column {
            name: name.into(),
            data_type: data_type.into(),
        })
        .collect();
        let rows: Rows = vec![vec![
            Some("9223372036854775807".into()),
            Some("12345678901234567890.123456789".into()),
            Some("t".into()),
            Some("NaN".into()),
            Some("[1,null,{\"k\":true}]".into()),
            None,
        ]]
        .into();
        for pretty in [false, true] {
            for lines in [false, true] {
                let mut out = Vec::new();
                let options = Options {
                    pretty,
                    ..Options::default()
                };
                write(
                    &mut out,
                    &table(&columns, &rows),
                    &options,
                    lines,
                    &AtomicBool::new(false),
                )
                .unwrap();
                let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
                let row = if lines { &parsed } else { &parsed[0] };
                assert_eq!(row["id"].as_i64(), Some(i64::MAX));
                assert_eq!(row["id_3"], "12345678901234567890.123456789");
                assert_eq!(row["id_2"], true);
                assert_eq!(row["special"], "NaN");
                assert!(row["nested"].is_array());
                assert!(row["absent"].is_null());
            }
        }
        let options = Options {
            pretty: false,
            decimals_as_numbers: true,
            ..Options::default()
        };
        let mut out = Vec::new();
        write(
            &mut out,
            &table(&columns, &rows),
            &options,
            false,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("\"id_3\":12345678901234567890.123456789")
        );
    }
    #[test]
    fn typed_binary_is_base64_and_text_mode_preserves_server_values() {
        let columns: Vec<_> = [
            ("hive", "BINARY"),
            ("postgres", "bytea"),
            ("trino", "varbinary"),
        ]
        .into_iter()
        .map(|(name, data_type)| Column {
            name: name.into(),
            data_type: data_type.into(),
        })
        .collect();
        let rows: Rows = vec![vec![
            Some("0x005cff".into()),
            Some("\\x005cff".into()),
            Some("AFz/".into()),
        ]]
        .into();
        for typed in [false, true] {
            let mut out = Vec::new();
            write(
                &mut out,
                &table(&columns, &rows),
                &Options {
                    typed,
                    ..Options::default()
                },
                false,
                &AtomicBool::new(false),
            )
            .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(json[0]["hive"], if typed { "AFz/" } else { "0x005cff" });
            assert_eq!(
                json[0]["postgres"],
                if typed { "AFz/" } else { "\\x005cff" }
            );
            assert_eq!(json[0]["trino"], "AFz/");
        }
    }
    #[test]
    fn empty_results_lines_newlines_and_cancelled_output_are_valid() {
        let columns = vec![Column {
            name: "text".into(),
            data_type: "text".into(),
        }];
        let rows = Rows::default();
        let mut out = Vec::new();
        write(
            &mut out,
            &table(&columns, &rows),
            &Options::default(),
            false,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out, b"[]\n");
        let rows: Rows = vec![vec![Some("a\nb".into())], vec![None]].into();
        out.clear();
        write(
            &mut out,
            &table(&columns, &rows),
            &Options::default(),
            true,
            &AtomicBool::new(false),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text.lines().next().unwrap()).unwrap()["text"],
            "a\nb"
        );
        assert!(
            write(
                &mut Vec::new(),
                &table(&columns, &rows),
                &Options::default(),
                false,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}

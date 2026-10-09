//! JSON objects serialized one row at a time, preserving decimal digits.
use super::{
    Table, check_cancelled,
    value::{self, Kind, Value},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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
    let mut writer = Stream::new(
        out,
        table
            .column_indices
            .clone()
            .map(|index| &table.columns[index]),
        options,
        lines,
    )?;
    writer.batch(table, 0, cancel)?;
    writer.finish(cancel)
}

pub(crate) struct Stream<'a, W> {
    out: &'a mut W,
    names: Vec<String>,
    kinds: Vec<Kind>,
    options: &'a Options,
    lines: bool,
    count: usize,
    _memory: std::sync::Arc<super::budget::Allowance>,
}

impl<'a, W: Write> Stream<'a, W> {
    pub(crate) fn new<'b>(
        out: &'a mut W,
        columns: impl Iterator<Item = &'b crate::model::Column>,
        options: &'a Options,
        lines: bool,
    ) -> io::Result<Self> {
        let memory = super::budget::GLOBAL.allowance(64 * super::budget::MIB)?;
        let columns: Vec<_> = columns.collect();
        let names = value::column_names(columns.iter().map(|column| column.name.as_str()));
        let kinds = columns
            .iter()
            .map(|column| Kind::of(&column.data_type))
            .collect();
        if !lines {
            out.write_all(b"[")?;
        }
        Ok(Self {
            out,
            names,
            kinds,
            options,
            lines,
            count: 0,
            _memory: memory,
        })
    }

    pub(crate) fn batch(
        &mut self,
        table: &Table<'_>,
        row_offset: usize,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        let out = &mut *self.out;
        let options = self.options;
        let lines = self.lines;
        let pretty = options.pretty && !lines;
        for row in table.row_indices.clone() {
            check_cancelled(cancel)?;
            if !lines {
                if self.count > 0 {
                    out.write_all(b",")?;
                }
                if pretty {
                    out.write_all(b"\n  ")?;
                }
            }
            out.write_all(b"{")?;
            for (column, text) in table.values(row).enumerate() {
                if column > 0 {
                    out.write_all(b",")?;
                }
                if pretty {
                    out.write_all(b"\n    ")?;
                }
                serde_json::to_writer(&mut *out, &self.names[column])?;
                out.write_all(if pretty { b": " } else { b":" })?;
                let result = write_value(out, self.kinds[column], text, options, pretty, cancel);
                result.map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "Column {}, row {}, value {:?}: {error}",
                            self.names[column],
                            row_offset + row + 1,
                            text.map(|text| text.chars().take(100).collect::<String>())
                        ),
                    )
                })?;
            }
            if pretty {
                out.write_all(b"\n  ")?;
            }
            out.write_all(b"}")?;
            if lines {
                out.write_all(b"\n")?;
            }
            self.count += 1;
        }
        Ok(())
    }

    pub(crate) fn finish(self, cancel: &AtomicBool) -> io::Result<usize> {
        if !self.lines {
            if self.options.pretty && self.count > 0 {
                self.out.write_all(b"\n")?;
            }
            self.out.write_all(b"]\n")?;
        }
        self.out.flush()?;
        check_cancelled(cancel)?;
        Ok(self.count)
    }
}

fn write_value(
    out: &mut impl Write,
    kind: Kind,
    text: Option<&str>,
    options: &Options,
    pretty: bool,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if !options.typed {
        serde_json::to_writer(out, &text)?;
        return Ok(());
    }
    match value::normalize(kind, text)? {
        Value::Null => serde_json::to_writer(out, &Option::<bool>::None)?,
        Value::Boolean(value) => serde_json::to_writer(out, &value)?,
        Value::Integer(value) => serde_json::to_writer(out, &value)?,
        Value::Float(value) if value.is_finite() => serde_json::to_writer(out, &value)?,
        Value::Float(_) => serde_json::to_writer(out, &text)?,
        Value::Decimal(value) if options.decimals_as_numbers && !special(value) => {
            let number: serde_json::Number = value
                .parse()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            serde_json::to_writer(out, &number)?;
        }
        Value::Decimal(value) | Value::Text(value) => serde_json::to_writer(out, &value)?,
        Value::Bytes(value) => serde_json::to_writer(out, &STANDARD.encode(value))?,
        Value::Nested(value) => write_nested(out, value.get(), pretty, cancel)?,
    }
    Ok(())
}

/// The fragment is already validated by RawValue. Copy spans directly, removing
/// only insignificant ASCII whitespace. This keeps JSON Lines on one line and
/// avoids an owned tree or another whole-cell buffer.
fn write_nested(
    out: &mut impl Write,
    text: &str,
    pretty: bool,
    cancel: &AtomicBool,
) -> io::Result<()> {
    if pretty {
        return out.write_all(text.as_bytes());
    }
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if index.is_multiple_of(4096) {
            check_cancelled(cancel)?;
            out.write_all(&text.as_bytes()[start..index])?;
            start = index;
        }
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            out.write_all(&text.as_bytes()[start..index])?;
            start = index + 1;
        }
    }
    out.write_all(&text.as_bytes()[start..])
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
    fn nested_json_whitespace_cannot_split_json_lines() {
        let columns = vec![Column {
            name: "nested".into(),
            data_type: "json".into(),
        }];
        let nested = "[\r\n 1,\n {\"value\":\"space \\n newline and \\\"quote\\\"\"}\r\n]";
        let rows: Rows = vec![
            vec![Some(nested.into())],
            vec![Some(" { \"a\" : [ true, null ] } ".into())],
        ]
        .into();
        for lines in [false, true] {
            let mut out = Vec::new();
            write(
                &mut out,
                &table(&columns, &rows),
                &Options {
                    pretty: false,
                    ..Options::default()
                },
                lines,
                &AtomicBool::new(false),
            )
            .unwrap();
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text.lines().count(), if lines { 2 } else { 1 });
            let first: serde_json::Value = if lines {
                serde_json::from_str(text.lines().next().unwrap()).unwrap()
            } else {
                serde_json::from_str::<serde_json::Value>(&text).unwrap()[0].clone()
            };
            assert_eq!(
                first["nested"],
                serde_json::from_str::<serde_json::Value>(nested).unwrap()
            );
            if lines {
                for line in text.lines() {
                    serde_json::from_str::<serde_json::Value>(line).unwrap();
                }
            }
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

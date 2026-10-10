//! Decode bounded schemas and one row from a raw Trino JSON page at a time.
use crate::{
    export::budget,
    model::{Column, Row},
};
use anyhow::{Context, Result};
use serde::{
    Deserialize,
    de::{self, DeserializeSeed, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::fmt;

const MAX_COLUMNS: usize = 4096;
const MAX_SCHEMA_BYTES: usize = budget::MIB;

pub(super) fn optional_text<'de, D: de::Deserializer<'de>, const LIMIT: usize>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    struct Optional<const LIMIT: usize>;
    impl<'de, const LIMIT: usize> Visitor<'de> for Optional<LIMIT> {
        type Value = Option<String>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("optional bounded Trino text")
        }
        fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: de::Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> std::result::Result<Self::Value, D::Error> {
            text::<D, LIMIT>(deserializer).map(Some)
        }
    }
    deserializer.deserialize_option(Optional::<LIMIT>)
}

pub(super) fn text<'de, D: de::Deserializer<'de>, const LIMIT: usize>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    struct Text<const LIMIT: usize>;
    impl<'de, const LIMIT: usize> Visitor<'de> for Text<LIMIT> {
        type Value = String;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("bounded Trino text")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.len() > LIMIT {
                return Err(E::custom("Trino text exceeds its size limit"));
            }
            Ok(value.to_owned())
        }
    }
    deserializer.deserialize_str(Text::<LIMIT>)
}

#[derive(Deserialize)]
struct Descriptor {
    #[serde(deserialize_with = "text::<_, 1024>")]
    name: String,
    #[serde(rename = "type", deserialize_with = "text::<_, 4096>")]
    data_type: String,
}

pub(super) fn columns(raw: &RawValue) -> Result<Vec<Column>> {
    struct Schema;
    impl<'de> Visitor<'de> for Schema {
        type Value = Vec<Column>;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("bounded Trino columns")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut columns = Vec::new();
            let mut bytes = 0usize;
            loop {
                if columns.len() == MAX_COLUMNS {
                    if sequence.next_element::<de::IgnoredAny>()?.is_some() {
                        return Err(de::Error::custom("Trino schema exceeds 4096 columns"));
                    }
                    break;
                }
                let Some(column) = sequence.next_element::<Descriptor>()? else {
                    break;
                };
                bytes += column.name.capacity() + column.data_type.capacity();
                if bytes > MAX_SCHEMA_BYTES {
                    return Err(de::Error::custom("Trino schema exceeds 1 MiB"));
                }
                columns.push(Column {
                    name: column.name,
                    data_type: column.data_type,
                });
            }
            Ok(columns)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let columns = de::Deserializer::deserialize_seq(&mut deserializer, Schema)?;
    deserializer.end()?;
    Ok(columns)
}

pub(super) struct Rows {
    raw: Box<RawValue>,
    offset: usize,
}

impl Rows {
    pub fn new(raw: Box<RawValue>) -> Result<Self> {
        anyhow::ensure!(
            raw.get().trim_start().starts_with('['),
            "Trino data is not an array"
        );
        let offset = raw.get().find('[').unwrap() + 1;
        Ok(Self { raw, offset })
    }

    pub fn next(&mut self, width: usize, direct: bool) -> Result<Option<Row>> {
        let source = self.raw.get();
        while source
            .as_bytes()
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
        if source.as_bytes().get(self.offset) == Some(&b',') {
            self.offset += 1;
        }
        while source
            .as_bytes()
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
        if source.as_bytes().get(self.offset) == Some(&b']') {
            return Ok(None);
        }
        let mut sequence =
            serde_json::Deserializer::from_str(&source[self.offset..]).into_iter::<&RawValue>();
        let raw = sequence.next().context("Incomplete Trino data")??;
        self.offset += sequence.byte_offset();
        let mut deserializer = serde_json::Deserializer::from_str(raw.get());
        let row = RowSeed { width, direct }.deserialize(&mut deserializer)?;
        deserializer.end()?;
        Ok(Some(row))
    }
}

struct RowSeed {
    width: usize,
    direct: bool,
}
impl<'de> DeserializeSeed<'de> for RowSeed {
    type Value = Row;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Row, D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'de> Visitor<'de> for RowSeed {
    type Value = Row;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("one bounded Trino row")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Row, A::Error> {
        let mut row = Vec::with_capacity(self.width);
        let mut bytes = self.width * std::mem::size_of::<Option<String>>();
        while let Some(raw) = sequence.next_element::<&RawValue>()? {
            if row.len() == self.width {
                return Err(de::Error::custom("Trino row has too many columns"));
            }
            let value = raw.get();
            let value = if value == "null" {
                None
            } else if value.starts_with('"') {
                let mut deserializer = serde_json::Deserializer::from_str(value);
                let value = de::Deserializer::deserialize_str(
                    &mut deserializer,
                    CellText {
                        direct: self.direct,
                        remaining: budget::MAX_ROW_BYTES.saturating_sub(bytes),
                    },
                )
                .map_err(de::Error::custom)?;
                Some(value)
            } else {
                if self.direct && value.len() > budget::MAX_CELL_BYTES {
                    return Err(de::Error::custom("Trino export cell exceeds 8 MiB"));
                }
                if self.direct && value.len() > budget::MAX_ROW_BYTES.saturating_sub(bytes) {
                    return Err(de::Error::custom("Trino export row exceeds 16 MiB"));
                }
                Some(value.to_owned())
            };
            bytes += value.as_ref().map_or(0, String::capacity);
            if self.direct && bytes > budget::MAX_ROW_BYTES {
                return Err(de::Error::custom("Trino export row exceeds 16 MiB"));
            }
            row.push(value);
        }
        if row.len() != self.width {
            return Err(de::Error::custom("Trino row has too few columns"));
        }
        Ok(row)
    }
}

struct CellText {
    direct: bool,
    remaining: usize,
}
impl<'de> Visitor<'de> for CellText {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded Trino cell")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<String, E> {
        if self.direct && value.len() > budget::MAX_CELL_BYTES {
            return Err(E::custom("Trino export cell exceeds 8 MiB"));
        }
        if self.direct && value.len() > self.remaining {
            return Err(E::custom("Trino export row exceeds 16 MiB"));
        }
        if value.len() > budget::MAX_ROW_BYTES {
            return Err(E::custom("Trino cell exceeds 16 MiB"));
        }
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_rows_preserve_decimal_lexemes_null_empty_and_nested_text() -> Result<()> {
        let mut rows = Rows::new(serde_json::from_str(
            r#"[[12345678901234567890.123450000,null,"",[1,2],"α🦀"],[1.2300e-5,null,"null",{},"next"]]"#,
        )?)?;
        assert_eq!(
            rows.next(5, true)?,
            Some(vec![
                Some("12345678901234567890.123450000".into()),
                None,
                Some(String::new()),
                Some("[1,2]".into()),
                Some("α🦀".into())
            ])
        );
        assert_eq!(
            rows.next(5, true)?,
            Some(vec![
                Some("1.2300e-5".into()),
                None,
                Some("null".into()),
                Some("{}".into()),
                Some("next".into())
            ])
        );
        assert!(rows.next(5, true)?.is_none());
        Ok(())
    }

    #[test]
    fn direct_decoder_rejects_cell_row_and_width_before_ownership() -> Result<()> {
        let oversized = "x".repeat(budget::MAX_CELL_BYTES + 1);
        let mut rows = Rows::new(serde_json::value::to_raw_value(&vec![vec![oversized]])?)?;
        assert!(
            rows.next(1, true)
                .unwrap_err()
                .to_string()
                .contains("8 MiB")
        );
        let cell = "x".repeat(budget::MAX_CELL_BYTES);
        let mut rows = Rows::new(serde_json::value::to_raw_value(&vec![vec![&cell, &cell]])?)?;
        assert!(
            rows.next(2, true)
                .unwrap_err()
                .to_string()
                .contains("16 MiB")
        );
        for (data, width) in [("[[1,2]]", 1), ("[[1]]", 2)] {
            let mut rows = Rows::new(serde_json::from_str(data)?)?;
            assert!(rows.next(width, true).is_err());
        }
        Ok(())
    }

    #[test]
    fn schema_limits_count_columns_and_owned_text() -> Result<()> {
        let descriptors = vec![serde_json::json!({"name":"x","type":"bigint"}); MAX_COLUMNS];
        assert_eq!(
            columns(&serde_json::value::to_raw_value(&descriptors)?)?.len(),
            MAX_COLUMNS
        );
        let mut extra = descriptors;
        extra.push(serde_json::json!({"name":"overflow","type":"bigint"}));
        assert!(columns(&serde_json::value::to_raw_value(&extra)?).is_err());
        let wide = vec![serde_json::json!({"name":"x".repeat(1024),"type":"varchar"}); 1024];
        assert!(columns(&serde_json::value::to_raw_value(&wide)?).is_err());
        for descriptor in [
            serde_json::json!({"name":"x".repeat(1025),"type":"bigint"}),
            serde_json::json!({"name":"x","type":"t".repeat(4097)}),
        ] {
            assert!(columns(&serde_json::value::to_raw_value(&vec![descriptor])?).is_err());
        }
        Ok(())
    }

    #[test]
    fn large_pages_decode_and_check_row_width_on_demand() -> Result<()> {
        let page = format!("[{}]", "[null],".repeat(100_000).trim_end_matches(','));
        let mut rows = Rows::new(serde_json::from_str(&page)?)?;
        assert_eq!(rows.next(1, true)?, Some(vec![None]));
        assert!(rows.next(2, true).is_err());
        Ok(())
    }
}

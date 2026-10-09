//! Typed Parquet files with exact decimals and explicit timestamp meanings.
use super::{
    Table, check_cancelled,
    decimal::{Decimal, Stats},
    temporal,
    value::{self, Kind, Value},
};
use parquet::{
    basic::{Compression as Codec, GzipLevel, LogicalType, Repetition, TimeUnit, Type as Physical},
    data_type::{
        BoolType, ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType,
        FloatType, Int32Type, Int64Type,
    },
    file::{
        properties::{EnabledStatistics, WriterProperties, WriterVersion},
        writer::{SerializedColumnWriter, SerializedFileWriter},
    },
    schema::types::{Type, TypePtr},
};
use std::{
    io::{self, Write},
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Compression {
    #[default]
    Snappy,
    Gzip,
    None,
}
impl Compression {
    pub const ALL: [Self; 3] = [Self::Snappy, Self::Gzip, Self::None];
    pub fn label(self) -> &'static str {
        match self {
            Self::Snappy => "Snappy",
            Self::Gzip => "Gzip",
            Self::None => "None",
        }
    }
    fn codec(self) -> Codec {
        match self {
            Self::Snappy => Codec::SNAPPY,
            Self::Gzip => Codec::GZIP(GzipLevel::default()),
            Self::None => Codec::UNCOMPRESSED,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ColumnTypes {
    #[default]
    Typed,
    Text,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Numeric {
    #[default]
    Decimal,
    Double,
    Text,
}
impl Numeric {
    pub const ALL: [Self; 3] = [Self::Decimal, Self::Double, Self::Text];
    pub fn label(self) -> &'static str {
        match self {
            Self::Decimal => "Decimal (exact)",
            Self::Double => "Double (approximate)",
            Self::Text => "Text",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Options {
    pub compression: Compression,
    pub column_types: ColumnTypes,
    pub numeric: Numeric,
}

#[derive(Clone, Copy, Debug)]
enum FieldKind {
    Boolean,
    Integer(u8),
    Float(bool),
    Decimal(u32, u32),
    Date,
    Timestamp(temporal::Timestamp),
    Binary(Kind),
    Text,
}
struct Field {
    name: String,
    column: usize,
    kind: FieldKind,
}

const ROW_GROUP_BYTES: usize = 64 * 1024 * 1024;

pub fn write(
    out: &mut (impl Write + Send),
    table: &Table<'_>,
    options: &Options,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    super::validate_table(table)?;
    check_cancelled(cancel)?;
    let fields = fields(table, options, cancel)?;
    let schema = Arc::new(
        Type::group_type_builder("result")
            .with_fields(fields.iter().map(field_type).collect::<io::Result<_>>()?)
            .build()
            .map_err(error)?,
    );
    let properties = Arc::new(
        WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_1_0)
            .set_created_by(format!("qrow {}", env!("CARGO_PKG_VERSION")))
            .set_compression(options.compression.codec())
            .set_dictionary_enabled(true)
            .set_dictionary_page_size_limit(1024 * 1024)
            .set_statistics_enabled(EnabledStatistics::Chunk)
            .set_max_row_group_bytes(Some(ROW_GROUP_BYTES))
            .build(),
    );
    let mut writer = SerializedFileWriter::new(out, schema, properties).map_err(error)?;
    let mut start = table.row_indices.start;
    while start < table.row_indices.end {
        check_cancelled(cancel)?;
        let mut end = start;
        let mut bytes = 0usize;
        while end < table.row_indices.end {
            let row_bytes: usize = table
                .values(end)
                .map(|value| value.map_or(24, |text| text.len().saturating_add(24)))
                .sum();
            if end > start && bytes.saturating_add(row_bytes) > ROW_GROUP_BYTES {
                break;
            }
            bytes = bytes.saturating_add(row_bytes);
            end += 1;
        }
        let mut group = writer.next_row_group().map_err(error)?;
        for field in &fields {
            check_cancelled(cancel)?;
            let mut column = group
                .next_column()
                .map_err(error)?
                .ok_or_else(|| invalid("Missing Parquet column"))?;
            write_column(&mut column, table, field, start..end, cancel)?;
            column.close().map_err(error)?;
        }
        group.close().map_err(error)?;
        start = end;
    }
    check_cancelled(cancel)?;
    writer.close().map_err(error)?;
    check_cancelled(cancel)?;
    Ok(table.row_count())
}

fn fields(table: &Table<'_>, options: &Options, cancel: &AtomicBool) -> io::Result<Vec<Field>> {
    let names = value::column_names(
        table
            .column_indices
            .clone()
            .map(|ix| table.columns[ix].name.as_str()),
    );
    table
        .column_indices
        .clone()
        .zip(names)
        .map(|(column, name)| {
            let kind = if options.column_types == ColumnTypes::Text {
                FieldKind::Text
            } else {
                kind(table, column, options, cancel)?
            };
            Ok(Field { name, column, kind })
        })
        .collect()
}
fn kind(
    table: &Table<'_>,
    column: usize,
    options: &Options,
    cancel: &AtomicBool,
) -> io::Result<FieldKind> {
    let name = table.columns[column].data_type.trim().to_ascii_lowercase();
    let base = name.split('(').next().unwrap_or_default().trim();
    if table.rows.context().iso_dates() {
        if base == "date" {
            return Ok(FieldKind::Date);
        }
        if let Some(timestamp) = temporal::Timestamp::from_type(&name)? {
            return Ok(FieldKind::Timestamp(timestamp));
        }
    }
    Ok(match Kind::of(&name) {
        Kind::Boolean => FieldKind::Boolean,
        Kind::Integer => FieldKind::Integer(match base {
            "tinyint" => 8,
            "smallint" | "int2" => 16,
            "int" | "integer" | "int4" => 32,
            _ => 64,
        }),
        Kind::Float => FieldKind::Float(matches!(base, "float" | "real" | "float4")),
        Kind::Decimal => match options.numeric {
            Numeric::Text => FieldKind::Text,
            Numeric::Double => FieldKind::Float(false),
            Numeric::Decimal => {
                let mut stats = Stats::default();
                for row in table.row_indices.clone() {
                    check_cancelled(cancel)?;
                    stats
                        .observe(cell(table, row, column))
                        .map_err(|error| cell_error(table, row, column, error))?;
                }
                if stats.special || !stats.present && base == "numeric" {
                    FieldKind::Text
                } else {
                    let explicit = decimal_type(&name);
                    let decimal = if let Some((precision, scale)) = explicit {
                        (precision > 0 && precision <= 38 && scale >= 0 && scale <= precision)
                            .then_some((precision as u32, scale as u32))
                    } else {
                        stats.inferred()
                    };
                    decimal.map_or(FieldKind::Text, |(precision, scale)| {
                        FieldKind::Decimal(precision, scale)
                    })
                }
            }
        },
        binary @ (Kind::Binary | Kind::HexBinary | Kind::Bytea) => FieldKind::Binary(binary),
        _ => FieldKind::Text,
    })
}

fn decimal_type(name: &str) -> Option<(i32, i32)> {
    let (_, rest) = name.split_once('(')?;
    let parameters = rest.strip_suffix(')')?;
    let (precision, scale) = parameters.split_once(',').unwrap_or((parameters, "0"));
    Some((precision.trim().parse().ok()?, scale.trim().parse().ok()?))
}
fn field_type(field: &Field) -> io::Result<TypePtr> {
    let (physical, logical) = match field.kind {
        FieldKind::Boolean => (Physical::BOOLEAN, None),
        FieldKind::Integer(bits) => (
            if bits <= 32 {
                Physical::INT32
            } else {
                Physical::INT64
            },
            Some(LogicalType::integer(bits as i8, true)),
        ),
        FieldKind::Float(single) => (
            if single {
                Physical::FLOAT
            } else {
                Physical::DOUBLE
            },
            None,
        ),
        FieldKind::Decimal(precision, scale) => (
            if precision <= 9 {
                Physical::INT32
            } else if precision <= 18 {
                Physical::INT64
            } else {
                Physical::FIXED_LEN_BYTE_ARRAY
            },
            Some(LogicalType::decimal(scale as i32, precision as i32)),
        ),
        FieldKind::Date => (Physical::INT32, Some(LogicalType::Date)),
        FieldKind::Timestamp(timestamp) => (
            Physical::INT64,
            Some(LogicalType::timestamp(
                timestamp.instant,
                if timestamp.nanos() {
                    TimeUnit::NANOS
                } else {
                    TimeUnit::MICROS
                },
            )),
        ),
        FieldKind::Binary(_) => (Physical::BYTE_ARRAY, None),
        FieldKind::Text => (Physical::BYTE_ARRAY, Some(LogicalType::String)),
    };
    let mut builder = Type::primitive_type_builder(&field.name, physical)
        .with_repetition(Repetition::OPTIONAL)
        .with_logical_type(logical);
    if let FieldKind::Decimal(precision, scale) = field.kind {
        builder = builder
            .with_precision(precision as i32)
            .with_scale(scale as i32);
        if precision > 18 {
            builder = builder.with_length(16);
        }
    }
    Ok(Arc::new(builder.build().map_err(error)?))
}

fn write_column(
    column: &mut SerializedColumnWriter<'_>,
    table: &Table<'_>,
    field: &Field,
    rows: Range<usize>,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let mut definitions = Vec::with_capacity(rows.len());
    macro_rules! values {
        ($convert:expr) => {{
            let mut values = Vec::new();
            for row in rows.clone() {
                check_cancelled(cancel)?;
                match cell(table, row, field.column) {
                    None => definitions.push(0),
                    Some(text) => {
                        definitions.push(1);
                        values.push(
                            ($convert)(text)
                                .map_err(|error| cell_error(table, row, field.column, error))?,
                        );
                    }
                }
            }
            values
        }};
    }
    macro_rules! batch {
        ($type:ty, $values:expr) => {{
            let values = $values;
            column
                .typed::<$type>()
                .write_batch(&values, Some(&definitions), None)
                .map_err(error)?;
        }};
    }
    match field.kind {
        FieldKind::Boolean => batch!(
            BoolType,
            values!(|text| match value::normalize(Kind::Boolean, Some(text))? {
                Value::Boolean(value) => Ok(value),
                _ => Err(invalid("Invalid boolean")),
            })
        ),
        FieldKind::Integer(bits) if bits <= 32 => batch!(
            Int32Type,
            values!(|text: &str| {
                let value: i64 = text.parse().map_err(|_| invalid("Invalid integer"))?;
                let min = -(1_i64 << (bits - 1));
                let max = (1_i64 << (bits - 1)) - 1;
                if !(min..=max).contains(&value) {
                    return Err(invalid("Integer exceeds the column range"));
                }
                i32::try_from(value).map_err(|_| invalid("Integer exceeds INT32"))
            })
        ),
        FieldKind::Integer(_) => batch!(
            Int64Type,
            values!(|text: &str| text
                .parse::<i64>()
                .map_err(|_| invalid("Integer exceeds INT64")))
        ),
        FieldKind::Float(true) => batch!(
            FloatType,
            values!(|text: &str| {
                let value: f64 = text.parse().map_err(|_| invalid("Invalid float"))?;
                let narrowed = value as f32;
                if value.is_finite() && !narrowed.is_finite() {
                    return Err(invalid("Float exceeds the column range"));
                }
                Ok(narrowed)
            })
        ),
        FieldKind::Float(false) => batch!(
            DoubleType,
            values!(|text: &str| text.parse::<f64>().map_err(|_| invalid("Invalid double")))
        ),
        FieldKind::Date => batch!(Int32Type, values!(temporal::date)),
        FieldKind::Timestamp(timestamp) => batch!(Int64Type, values!(|text| timestamp.parse(text))),
        FieldKind::Decimal(precision, scale) if precision <= 9 => batch!(
            Int32Type,
            values!(|text| {
                i32::try_from(Decimal::parse(text)?.coefficient(precision, scale)?)
                    .map_err(|_| invalid("Decimal exceeds INT32"))
            })
        ),
        FieldKind::Decimal(precision, scale) if precision <= 18 => batch!(
            Int64Type,
            values!(|text| {
                i64::try_from(Decimal::parse(text)?.coefficient(precision, scale)?)
                    .map_err(|_| invalid("Decimal exceeds INT64"))
            })
        ),
        FieldKind::Decimal(precision, scale) => batch!(
            FixedLenByteArrayType,
            values!(|text| {
                Ok(FixedLenByteArray::from(
                    Decimal::parse(text)?
                        .coefficient(precision, scale)?
                        .to_be_bytes()
                        .to_vec(),
                ))
            })
        ),
        FieldKind::Binary(kind) => batch!(
            ByteArrayType,
            values!(|text| match value::normalize(kind, Some(text))? {
                Value::Bytes(bytes) => Ok(ByteArray::from(bytes)),
                _ => Err(invalid("Invalid binary")),
            })
        ),
        FieldKind::Text => batch!(
            ByteArrayType,
            values!(|text: &str| Ok::<_, io::Error>(ByteArray::from(text)))
        ),
    }
    Ok(())
}

fn cell<'a>(table: &'a Table<'_>, row: usize, column: usize) -> Option<&'a str> {
    table.rows.get(row)?.get(column)?.as_deref()
}
fn cell_error(table: &Table<'_>, row: usize, column: usize, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "Column {}, row {}, value {:?}: {error}",
            table.columns[column].name,
            row + 1,
            cell(table, row, column).map(|text| text.chars().take(100).collect::<String>())
        ),
    )
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn error(error: parquet::errors::ParquetError) -> io::Error {
    io::Error::other(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        export::{Context, PostgresContext, Rows},
        model::Column,
    };
    use parquet::{
        file::reader::{FileReader, SerializedFileReader},
        record::RowAccessor,
    };
    use std::fs::File;

    fn table<'a>(columns: &'a [Column], rows: &'a Rows) -> Table<'a> {
        Table {
            columns,
            rows,
            row_indices: 0..rows.len(),
            column_indices: 0..=columns.len() - 1,
        }
    }
    fn schema_columns(names: &[(&str, &str)]) -> Vec<Column> {
        names
            .iter()
            .map(|(name, data_type)| Column {
                name: (*name).into(),
                data_type: (*data_type).into(),
            })
            .collect()
    }
    fn read(table: &Table<'_>, options: &Options) -> SerializedFileReader<File> {
        let temporary = tempfile::NamedTempFile::new().unwrap();
        let mut file = File::create(temporary.path()).unwrap();
        write(&mut file, table, options, &AtomicBool::new(false)).unwrap();
        SerializedFileReader::new(File::open(temporary.path()).unwrap()).unwrap()
    }
    #[test]
    fn every_codec_round_trips_exact_values_nulls_and_unique_names() {
        let columns = schema_columns(&[
            ("id", "tinyint"),
            ("id", "bigint"),
            ("id_2", "boolean"),
            ("money", "numeric"),
            ("huge", "decimal(38,0)"),
            ("bytes", "BINARY"),
            ("day", "date"),
            ("moment", "timestamp(6)"),
            ("items", "ARRAY"),
        ]);
        let huge = "-99999999999999999999999999999999999999";
        let rows: Rows = vec![
            vec![
                Some("-128".into()),
                Some(i64::MIN.to_string()),
                Some("true".into()),
                Some("9999".into()),
                Some(huge.into()),
                Some("0x005cff".into()),
                Some("1969-12-31".into()),
                Some("1969-12-31 23:59:59.123456".into()),
                Some("[1,null]".into()),
            ],
            vec![
                Some("127".into()),
                Some(i64::MAX.to_string()),
                None,
                Some("1.2345".into()),
                None,
                None,
                None,
                None,
                None,
            ],
        ]
        .into();
        for compression in Compression::ALL {
            let reader = read(
                &table(&columns, &rows),
                &Options {
                    compression,
                    ..Options::default()
                },
            );
            let metadata = reader.metadata().file_metadata();
            assert_eq!(metadata.num_rows(), 2);
            assert_eq!(
                metadata.created_by(),
                Some(concat!("qrow ", env!("CARGO_PKG_VERSION")))
            );
            assert!(metadata.key_value_metadata().is_none());
            let schema = metadata.schema_descr();
            assert_eq!(schema.column(1).name(), "id_3");
            assert_eq!(schema.column(2).name(), "id_2");
            assert_eq!(
                schema.column(3).logical_type_ref(),
                Some(&LogicalType::decimal(4, 8))
            );
            assert_eq!(
                schema.column(4).physical_type(),
                Physical::FIXED_LEN_BYTE_ARRAY
            );
            let actual = reader
                .get_row_iter(None)
                .unwrap()
                .collect::<parquet::errors::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(actual[0].get_byte(0).unwrap(), -128);
            assert_eq!(actual[1].get_long(1).unwrap(), i64::MAX);
            assert!(actual[0].get_bool(2).unwrap());
            assert_eq!(
                actual[0].get_decimal(3).unwrap().data(),
                &99990000_i32.to_be_bytes()
            );
            assert_eq!(
                actual[1].get_decimal(3).unwrap().data(),
                &12345_i32.to_be_bytes()
            );
            let huge: i128 = huge.parse().unwrap();
            assert_eq!(
                actual[0].get_decimal(4).unwrap().data(),
                &huge.to_be_bytes()
            );
            assert_eq!(actual[0].get_bytes(5).unwrap().data(), &[0, 92, 255]);
            assert_eq!(actual[0].get_timestamp_micros(7).unwrap(), -876544);
            assert_eq!(actual[0].get_string(8).unwrap(), "[1,null]");
            assert!(matches!(
                actual[1].get_column_iter().nth(2).unwrap().1,
                parquet::record::Field::Null
            ));
        }
    }
    #[test]
    fn numeric_schema_checks_all_rows_and_metadata_constraints() {
        for (name, values, expected) in [
            ("numeric", vec![Some("9999"), Some("1.2345")], Some((8, 4))),
            (
                "numeric",
                vec![Some("99999999999999999999999999999999999999"), Some("0.1")],
                None,
            ),
            ("numeric", vec![None, None], None),
            ("numeric(10,2)", vec![Some("1.23"), Some("NaN")], None),
            ("numeric(3,-2)", vec![Some("1000")], None),
            ("numeric(3,5)", vec![Some("0.00123")], None),
            ("numeric(39,0)", vec![Some("1")], None),
            ("numeric", vec![Some("Infinity")], None),
            ("numeric(18,2)", vec![Some("1.20")], Some((18, 2))),
            (
                "decimal(38,38)",
                vec![Some("0.99999999999999999999999999999999999999")],
                Some((38, 38)),
            ),
        ] {
            let columns = schema_columns(&[("amount", name)]);
            let rows: Rows = values
                .into_iter()
                .map(|value| vec![value.map(str::to_owned)])
                .collect();
            let reader = read(&table(&columns, &rows), &Options::default());
            let schema = reader.metadata().file_metadata().schema_descr();
            let logical = expected.map_or(LogicalType::String, |(precision, scale)| {
                LogicalType::decimal(scale, precision)
            });
            assert_eq!(
                schema.column(0).logical_type_ref(),
                Some(&logical),
                "{name}"
            );
        }
    }
    #[test]
    fn text_types_non_iso_dates_and_approximate_numeric_are_explicit() {
        let columns = schema_columns(&[
            ("day", "date"),
            ("moment", "timestamptz(6)"),
            ("amount", "numeric"),
        ]);
        let mut rows: Rows = vec![vec![
            Some("09/10/2026".into()),
            Some("Fri 09 Oct 01:02:03 2026 UTC".into()),
            Some("12345678901234567890.1".into()),
        ]]
        .into();
        rows.set_context(Context {
            postgres: Some(PostgresContext {
                date_style: "SQL, DMY".into(),
                interval_style: "sql_standard".into(),
                time_zone: "Europe/Paris".into(),
            }),
        });
        let reader = read(
            &table(&columns, &rows),
            &Options {
                numeric: Numeric::Double,
                ..Options::default()
            },
        );
        let actual = reader.get_row_iter(None).unwrap().next().unwrap().unwrap();
        assert_eq!(actual.get_string(0).unwrap(), "09/10/2026");
        assert_eq!(
            actual.get_string(1).unwrap(),
            "Fri 09 Oct 01:02:03 2026 UTC"
        );
        assert!(actual.get_double(2).unwrap().is_finite());
        let reader = read(
            &table(&columns, &rows),
            &Options {
                column_types: ColumnTypes::Text,
                ..Options::default()
            },
        );
        assert_eq!(
            reader
                .get_row_iter(None)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .get_string(2)
                .unwrap(),
            "12345678901234567890.1"
        );
    }
    #[test]
    fn timestamps_use_exact_units_and_utc_annotations() {
        let columns = schema_columns(&[
            ("wall", "timestamp(6)"),
            ("instant", "timestamp(9) with time zone"),
            ("hive_instant", "TIMESTAMP LOCAL TZ"),
        ]);
        let rows: Rows = vec![vec![
            Some("1970-01-01 00:00:00.000001".into()),
            Some("1970-01-01 01:00:00.000000001 Europe/Paris".into()),
            Some("1970-01-01 07:00:00+07".into()),
        ]]
        .into();
        let reader = read(&table(&columns, &rows), &Options::default());
        let schema = reader.metadata().file_metadata().schema_descr();
        assert_eq!(
            schema.column(0).logical_type_ref(),
            Some(&LogicalType::timestamp(false, TimeUnit::MICROS))
        );
        assert_eq!(
            schema.column(1).logical_type_ref(),
            Some(&LogicalType::timestamp(true, TimeUnit::NANOS))
        );
        assert_eq!(
            schema.column(2).logical_type_ref(),
            Some(&LogicalType::timestamp(true, TimeUnit::MICROS))
        );
        for (column, expected) in [(0, 1_i64), (1, 1_i64), (2, 0_i64)] {
            let mut values = Vec::new();
            let mut definitions = Vec::new();
            let parquet::column::reader::ColumnReader::Int64ColumnReader(mut column_reader) =
                reader
                    .get_row_group(0)
                    .unwrap()
                    .get_column_reader(column)
                    .unwrap()
            else {
                panic!("Expected INT64 timestamp")
            };
            assert_eq!(
                column_reader
                    .read_records(1, Some(&mut definitions), None, &mut values)
                    .unwrap(),
                (1, 1, 1)
            );
            assert_eq!(values, vec![expected]);
        }
        let columns = schema_columns(&[("too_precise", "timestamp(12)")]);
        assert!(
            write(
                &mut Vec::new(),
                &table(&columns, &Rows::default()),
                &Options::default(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .to_string()
            .contains("Text column types")
        );
        let reader = read(
            &table(&columns, &Rows::default()),
            &Options {
                column_types: ColumnTypes::Text,
                ..Options::default()
            },
        );
        assert_eq!(reader.metadata().file_metadata().num_rows(), 0);
    }
    #[test]
    fn bad_values_and_cancellation_preserve_the_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.parquet");
        std::fs::write(&path, "original").unwrap();
        let columns = schema_columns(&[("small", "tinyint")]);
        let rows: Rows = vec![vec![Some("128".into())]].into();
        let error = super::super::save(&path, &AtomicBool::new(false), |out| {
            write(
                out,
                &table(&columns, &rows),
                &Options::default(),
                &AtomicBool::new(false),
            )
        })
        .unwrap_err();
        assert!(error.to_string().contains("Column small, row 1"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(
            write(
                &mut Vec::new(),
                &table(&columns, &rows),
                &Options::default(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}

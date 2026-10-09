//! Export of result rows to text formats and files. The core library owns the
//! formats and the file handling, so they work without the UI.

pub(crate) mod budget;
mod context;
pub mod csv;
mod decimal;
mod disk;
mod jobs;
pub mod json;
pub mod markdown;
pub mod parquet;
mod rows;
pub mod spool;
pub mod stream;
mod temporal;
pub(crate) mod value;
pub use context::{Context, Postgres as PostgresContext};
pub use disk::DiskWriter;
pub use jobs::{Jobs, Writer};
pub use rows::Rows;

use crate::model::Column;
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Last-used export options for this workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Settings {
    pub format: Format,
    pub csv: csv::CsvOptions,
    pub markdown: markdown::Options,
    pub json: json::Options,
    pub parquet: parquet::Options,
    pub directory: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Format {
    #[default]
    Csv,
    Markdown,
    Json,
    JsonLines,
    Parquet,
}

impl Format {
    pub const ALL: [Self; 5] = [
        Self::Csv,
        Self::Markdown,
        Self::Json,
        Self::JsonLines,
        Self::Parquet,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV / TSV",
            Self::Markdown => "Markdown",
            Self::Json => "JSON array",
            Self::JsonLines => "JSON Lines",
            Self::Parquet => "Parquet",
        }
    }
}

impl Settings {
    pub fn extension(&self) -> &'static str {
        match self.format {
            Format::Csv => self.csv.extension(),
            Format::Markdown => "md",
            Format::Json => "json",
            Format::JsonLines => "jsonl",
            Format::Parquet => "parquet",
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        match self.format {
            Format::Csv => self.csv.validate(),
            Format::Markdown
                if self.markdown.style == markdown::Style::CodeBlock
                    && !(1..=1000).contains(&self.markdown.max_cell_width) =>
            {
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Cell width must be between 1 and 1000.",
                ))
            }
            _ => Ok(()),
        }
    }
}

/// Write a downloaded snapshot with the selected format.
pub fn write(
    out: &mut (impl Write + Send),
    table: &Table<'_>,
    settings: &Settings,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    settings.validate()?;
    validate_table(table)?;
    for row in table.row_indices.clone() {
        check_cancelled(cancel)?;
        budget::selected_row_bytes(table.values(row))?;
    }
    match settings.format {
        Format::Parquet => parquet::write(out, table, &settings.parquet, cancel),
        Format::Csv => write_csv(out, table, &settings.csv, cancel),
        Format::Markdown => markdown::write(out, table, &settings.markdown, cancel),
        Format::Json | Format::JsonLines => json::write(
            out,
            table,
            &settings.json,
            settings.format == Format::JsonLines,
            cancel,
        ),
    }
}

/// What a column holds, as far as a format needs to know. Each connector
/// names its types differently; [`ValueKind::of`] maps all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// Integers, floating-point numbers, and decimals.
    Number,
    Boolean,
    /// Text and every other type: dates, binary, nested values.
    Text,
}

impl ValueKind {
    /// The kind of a column type name from Kyuubi (`BIGINT`), Postgres
    /// (`int8`, `numeric`), or Trino (`decimal(18,2)`).
    pub fn of(type_name: &str) -> Self {
        let name = type_name.trim().to_ascii_lowercase();
        // `decimal(18,2)`, `timestamp(3) with time zone`: the base name decides.
        let base = name.split('(').next().unwrap_or_default().trim();
        match base {
            "tinyint" | "smallint" | "int" | "integer" | "bigint" | "int2" | "int4" | "int8"
            | "float" | "real" | "double" | "double precision" | "float4" | "float8"
            | "decimal" | "numeric" | "number" => Self::Number,
            "boolean" | "bool" => Self::Boolean,
            _ => Self::Text,
        }
    }
}

/// A result column as a format sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExportColumn {
    pub name: String,
    pub kind: ValueKind,
}

impl From<&Column> for ExportColumn {
    fn from(column: &Column) -> Self {
        Self {
            name: column.name.clone(),
            kind: ValueKind::of(&column.data_type),
        }
    }
}

/// Rows to export: some columns of some rows of a result. The rows stay with
/// the result; the selection names them by index.
#[non_exhaustive]
pub struct Table<'a> {
    pub columns: &'a [Column],
    pub rows: &'a Rows,
    /// Indices into `rows`, in order.
    pub row_indices: std::ops::Range<usize>,
    /// Indices into `columns`, in order.
    pub column_indices: std::ops::RangeInclusive<usize>,
}

impl Table<'_> {
    pub fn export_columns(&self) -> Vec<ExportColumn> {
        self.column_indices
            .clone()
            .filter_map(|i| self.columns.get(i).map(ExportColumn::from))
            .collect()
    }

    /// The values of one row, in the order of the selected columns.
    pub fn values(&self, row: usize) -> impl Iterator<Item = Option<&str>> {
        let values = self.rows.get(row);
        self.column_indices.clone().map(move |column| {
            values
                .and_then(|values| values.get(column))
                .and_then(|value| value.as_deref())
        })
    }

    pub fn row_count(&self) -> usize {
        self.row_indices.len()
    }
}

/// Stopped by a cancellation request.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The export was cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A source snapshot retains the preview batches without copying cell values.
pub struct Snapshot {
    columns: Vec<Column>,
    rows: Rows,
    _lease: rows::Lease,
    _metadata: budget::Allocation,
}

impl Snapshot {
    pub(crate) fn batches(&self) -> impl Iterator<Item = &[crate::model::Row]> {
        self.rows.batches()
    }

    pub fn new(columns: &[Column], rows: &Rows) -> io::Result<Self> {
        let metadata = columns
            .len()
            .saturating_mul(std::mem::size_of::<Column>())
            .saturating_add(
                columns
                    .iter()
                    .map(|column| column.name.len() + column.data_type.len())
                    .sum::<usize>(),
            )
            .saturating_add(rows.metadata_bytes());
        let metadata = budget::GLOBAL.acquire(metadata)?;
        let lease = rows.retain_for_export()?;
        Ok(Self {
            columns: columns.to_vec(),
            rows: rows.clone(),
            _lease: lease,
            _metadata: metadata,
        })
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn table(
        &self,
        selection: Option<(std::ops::Range<usize>, std::ops::RangeInclusive<usize>)>,
    ) -> Table<'_> {
        let (row_indices, column_indices) =
            selection.unwrap_or((0..self.rows.len(), 0..=self.columns.len().saturating_sub(1)));
        Table {
            columns: &self.columns,
            rows: &self.rows,
            row_indices,
            column_indices,
        }
    }
}

pub fn check_cancelled(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(io::Error::other(Cancelled))
    } else {
        Ok(())
    }
}

/// The clipboard output is bounded while it is written, before allocation.
pub const CLIPBOARD_BYTES: usize = 10 * 1024 * 1024;

pub struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    memory: Option<budget::Allocation>,
}

/// Completed text keeps its buffer charge while it waits for the UI.
pub struct Text {
    text: String,
    _memory: Option<budget::Allocation>,
}

impl std::ops::Deref for Text {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl Text {
    /// Transfer the final text to the system clipboard or another consumer.
    pub fn into_string(self) -> String {
        self.text
    }
}

impl LimitedWriter {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            memory: None,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_text(self) -> io::Result<Text> {
        let text = String::from_utf8(self.bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Text {
            text,
            _memory: self.memory,
        })
    }
    pub fn into_preview(self, truncated: bool) -> Text {
        let mut text = String::from_utf8_lossy(&self.bytes)
            .trim_start_matches('\u{feff}')
            .to_owned();
        if truncated {
            text.push_str("\n…");
        }
        Text {
            text,
            _memory: self.memory,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "The text exceeds the copy limit. Save it to a file instead.",
            ));
        }
        if self.memory.is_none() && !bytes.is_empty() {
            // Vec growth can retain old and new allocations briefly. Preview
            // decoding can also copy the buffer before the old one drops.
            let capacity = self
                .limit
                .checked_next_power_of_two()
                .and_then(|capacity| capacity.checked_mul(3))
                .and_then(|capacity| capacity.checked_add(64))
                .ok_or_else(|| io::Error::other("The copy memory limit is too large."))?;
            self.memory = Some(budget::GLOBAL.acquire(capacity)?);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes `table` as CSV to `out`. Returns the number of data rows.
pub fn write_csv(
    out: &mut impl Write,
    table: &Table<'_>,
    options: &csv::CsvOptions,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    validate_table(table)?;
    let _memory = budget::GLOBAL.allowance(64 * budget::MIB)?;
    check_cancelled(cancel)?;
    let columns = table.export_columns();
    let mut writer = csv::CsvWriter::new(out, options, &columns);
    writer.begin()?;
    for row in table.row_indices.clone() {
        check_cancelled(cancel)?;
        writer.row(table.values(row))?;
    }
    writer.finish()?;
    check_cancelled(cancel)?;
    Ok(table.row_count())
}

fn validate_table(table: &Table<'_>) -> io::Result<()> {
    if table.columns.is_empty()
        || table.row_indices.end > table.rows.len()
        || table.row_indices.start > table.row_indices.end
        || table.column_indices.is_empty()
        || *table.column_indices.end() >= table.columns.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid export range",
        ));
    }
    let headers = table
        .column_indices
        .clone()
        .map(|index| {
            let column = &table.columns[index];
            column
                .name
                .len()
                .saturating_add(column.data_type.len())
                .saturating_add(std::mem::size_of::<Column>())
        })
        .try_fold(0usize, |sum, bytes| sum.checked_add(bytes));
    if headers.is_none_or(|bytes| bytes > budget::MAX_ROW_BYTES) {
        return Err(io::Error::other("The export column headers exceed 16 MiB."));
    }
    Ok(())
}

/// Writes a file through a temporary file in the same directory, so the
/// target appears only complete. On an error or a cancellation, the target
/// does not change and the temporary file is deleted.
pub fn save<T>(
    path: &Path,
    cancel: &AtomicBool,
    write: impl FnOnce(&mut BufWriter<DiskWriter<File>>) -> io::Result<T>,
) -> io::Result<T> {
    check_cancelled(cancel)?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".qrow-export-")
        .suffix(".tmp")
        .tempfile_in(directory)?;
    let (file, temporary_path) = temporary.into_parts();
    let _buffer = budget::GLOBAL.acquire(8192)?;
    let mut out = BufWriter::new(DiskWriter::new(file, directory)?);
    let result = write(&mut out)?;
    let file = out.into_inner().map_err(|error| error.into_error())?;
    file.sync_all()?;
    check_cancelled(cancel)?;
    temporary_path.persist(path).map_err(|error| error.error)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_clipboard_and_preview_text_hold_the_buffer_reservation() {
        let budget = budget::Budget::new(64);
        for preview in [false, true] {
            let mut writer = LimitedWriter::new(10);
            writer.memory = Some(budget.acquire(64).unwrap());
            writer.write_all(b"hello").unwrap();
            let text = if preview {
                writer.into_preview(true)
            } else {
                writer.into_text().unwrap()
            };
            assert!(budget.acquire(1).is_err());
            assert_eq!(&*text, if preview { "hello\n…" } else { "hello" });
            drop(text);
            assert!(budget.acquire(64).is_ok());
        }
    }

    #[test]
    fn selecting_a_small_cell_does_not_inspect_an_oversized_unselected_cell() {
        let columns = vec![
            Column {
                name: "small".into(),
                data_type: "STRING".into(),
            },
            Column {
                name: "large".into(),
                data_type: "STRING".into(),
            },
        ];
        let rows = Rows::from(vec![vec![
            Some("selected".into()),
            Some("x".repeat(budget::MAX_CELL_BYTES + 1)),
        ]]);
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..1,
            column_indices: 0..=0,
        };
        let mut bytes = Vec::new();
        write(
            &mut bytes,
            &table,
            &Settings::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(bytes, b"small\r\nselected\r\n");
        let table = Table {
            column_indices: 0..=1,
            ..table
        };
        assert!(
            write(
                &mut Vec::new(),
                &table,
                &Settings::default(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .to_string()
            .contains("cell")
        );
    }

    fn columns() -> Vec<Column> {
        [
            ("route", "STRING"),
            ("departures", "BIGINT"),
            ("cancelled", "bool"),
        ]
        .into_iter()
        .map(|(name, data_type)| Column {
            name: name.into(),
            data_type: data_type.into(),
        })
        .collect()
    }

    #[test]
    fn type_names_of_every_connector_map_to_kinds() {
        for name in [
            "BIGINT",
            "int8",
            "integer",
            "DOUBLE",
            "double precision",
            "real",
            "numeric",
            "decimal(18,2)",
            "DECIMAL",
        ] {
            assert_eq!(ValueKind::of(name), ValueKind::Number, "{name}");
        }
        for name in ["BOOLEAN", "bool", "boolean"] {
            assert_eq!(ValueKind::of(name), ValueKind::Boolean, "{name}");
        }
        for name in [
            "STRING",
            "varchar(20)",
            "text",
            "timestamp(3) with time zone",
            "ARRAY",
            "bytea",
            "interval",
        ] {
            assert_eq!(ValueKind::of(name), ValueKind::Text, "{name}");
        }
    }

    #[test]
    fn a_table_selects_rows_and_columns_in_order() {
        let columns = columns();
        let rows: Rows = (0..4)
            .map(|i| vec![Some(format!("r{i}")), Some(i.to_string()), None])
            .collect();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 1..3,
            column_indices: 1..=2,
        };
        assert_eq!(
            table
                .export_columns()
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["departures", "cancelled"]
        );
        let mut out = Vec::new();
        let count = write_csv(
            &mut out,
            &table,
            &csv::CsvOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "departures,cancelled\r\n1,\r\n2,\r\n"
        );
    }

    #[test]
    fn a_cancelled_export_stops_and_leaves_the_target_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        std::fs::write(&path, "before").unwrap();
        let columns = columns();
        let rows: Rows = vec![vec![Some("a".into()), None, None]; 10].into();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..10,
            column_indices: 0..=2,
        };
        let error = save(&path, &AtomicBool::new(false), |out| {
            write_csv(
                out,
                &table,
                &csv::CsvOptions::default(),
                &AtomicBool::new(true),
            )
        })
        .unwrap_err();
        assert!(error.get_ref().is_some_and(|inner| inner.is::<Cancelled>()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
        // Only the target is in the directory: the temporary file is gone.
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_saved_file_replaces_the_target_only_when_complete() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        save(&path, &AtomicBool::new(false), |out| out.write_all(b"done")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "done");
        let error = save(&path, &AtomicBool::new(false), |out| {
            out.write_all(b"partial")?;
            Err::<(), _>(io::Error::other("disk full"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "disk full");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "done");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancellation_after_writing_prevents_publication_and_cleans_the_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        std::fs::write(&path, "before").unwrap();
        let cancel = AtomicBool::new(false);
        let error = save(&path, &cancel, |out| {
            out.write_all(b"complete")?;
            cancel.store(true, Ordering::Relaxed);
            Ok(())
        })
        .unwrap_err();
        assert!(error.get_ref().is_some_and(|inner| inner.is::<Cancelled>()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn clipboard_bound_rejects_a_large_cell_without_adding_partial_bytes() {
        let columns = columns();
        let rows: Rows = vec![vec![Some("x".repeat(1024)), None, None]].into();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..1,
            column_indices: 0..=2,
        };
        let mut out = LimitedWriter::new(64);
        let error = write_csv(
            &mut out,
            &table,
            &csv::CsvOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.to_string().contains("copy limit"));
        assert_eq!(
            String::from_utf8_lossy(out.as_bytes()),
            "route,departures,cancelled\r\n"
        );
    }

    #[test]
    fn cancellation_is_checked_even_for_header_only_results() {
        let columns = columns();
        let rows = Rows::default();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..0,
            column_indices: 0..=2,
        };
        let mut out = Vec::new();
        assert!(
            write_csv(
                &mut out,
                &table,
                &csv::CsvOptions::default(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert!(out.is_empty());
        assert_eq!(
            write_csv(
                &mut out,
                &table,
                &csv::CsvOptions::default(),
                &AtomicBool::new(false)
            )
            .unwrap(),
            0
        );
        assert_eq!(out, b"route,departures,cancelled\r\n");
    }
}

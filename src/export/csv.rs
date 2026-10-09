//! CSV and tab-separated text. The writer keeps null and the empty string
//! apart: a null is the null marker without quotes, and an empty string is
//! always `""`.

use super::{ExportColumn, ValueKind};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Separator {
    #[default]
    Comma,
    Semicolon,
    Tab,
    Pipe,
}

impl Separator {
    pub const ALL: [Self; 4] = [Self::Comma, Self::Semicolon, Self::Tab, Self::Pipe];
    pub fn char(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Semicolon => ';',
            Self::Tab => '\t',
            Self::Pipe => '|',
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Comma => "Comma",
            Self::Semicolon => "Semicolon",
            Self::Tab => "Tab",
            Self::Pipe => "Pipe",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    #[default]
    Crlf,
    Lf,
}

impl LineEnding {
    pub const ALL: [Self; 2] = [Self::Crlf, Self::Lf];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crlf => "\r\n",
            Self::Lf => "\n",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Crlf => "CRLF (Windows, Excel)",
            Self::Lf => "LF (macOS, Linux)",
        }
    }
}

/// How a null is written. The marker is never quoted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NullMarker {
    #[default]
    Empty,
    Null,
    Backslash,
}

impl NullMarker {
    pub const ALL: [Self; 3] = [Self::Empty, Self::Null, Self::Backslash];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "",
            Self::Null => "NULL",
            Self::Backslash => "\\N",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Null => "NULL",
            Self::Backslash => "\\N",
        }
    }
}

/// The options of a CSV export. The defaults are the Standard preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct CsvOptions {
    pub separator: Separator,
    pub line_ending: LineEnding,
    /// Write the column names as the first line.
    pub header: bool,
    /// Start the file with a UTF-8 byte-order mark, so Excel reads UTF-8.
    pub byte_order_mark: bool,
    /// Quote every non-null value, not only the values that need it.
    pub quote_all: bool,
    pub null: NullMarker,
    /// Prefix `'` to text that a spreadsheet would read as a formula.
    pub escape_formulas: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Preset::Standard.options()
    }
}

/// Named option sets. Changing an option of a preset makes the options
/// custom; [`CsvOptions::preset`] then returns `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Standard,
    Excel,
    ExcelSemicolon,
    Tsv,
}

impl Preset {
    pub const ALL: [Self; 4] = [Self::Standard, Self::Excel, Self::ExcelSemicolon, Self::Tsv];
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard (RFC 4180)",
            Self::Excel => "Excel",
            Self::ExcelSemicolon => "Excel (semicolon)",
            Self::Tsv => "Tab-separated",
        }
    }
    pub fn options(self) -> CsvOptions {
        let standard = CsvOptions {
            separator: Separator::Comma,
            line_ending: LineEnding::Crlf,
            header: true,
            byte_order_mark: false,
            quote_all: false,
            null: NullMarker::Empty,
            escape_formulas: false,
        };
        match self {
            Self::Standard => standard,
            Self::Excel => CsvOptions {
                byte_order_mark: true,
                escape_formulas: true,
                ..standard
            },
            Self::ExcelSemicolon => CsvOptions {
                separator: Separator::Semicolon,
                byte_order_mark: true,
                escape_formulas: true,
                ..standard
            },
            Self::Tsv => CsvOptions {
                separator: Separator::Tab,
                line_ending: LineEnding::Lf,
                ..standard
            },
        }
    }
}

impl CsvOptions {
    /// The preset with exactly these options.
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL
            .into_iter()
            .find(|preset| preset.options() == *self)
    }

    /// The file name extension.
    pub fn extension(&self) -> &'static str {
        if self.separator == Separator::Tab {
            "tsv"
        } else {
            "csv"
        }
    }
}

/// Writes records. Call [`begin`](Self::begin) once, then
/// [`row`](Self::row) for each row, then [`finish`](Self::finish).
pub struct CsvWriter<'a, W: Write> {
    out: &'a mut W,
    options: CsvOptions,
    columns: &'a [ExportColumn],
}

impl<'a, W: Write> CsvWriter<'a, W> {
    pub fn new(out: &'a mut W, options: &CsvOptions, columns: &'a [ExportColumn]) -> Self {
        Self {
            out,
            options: *options,
            columns,
        }
    }

    pub fn begin(&mut self) -> io::Result<()> {
        if self.options.byte_order_mark {
            self.out.write_all("\u{FEFF}".as_bytes())?;
        }
        if self.options.header {
            let columns = self.columns;
            self.record(columns.iter().map(|c| Some(c.name.as_str())), false)?;
        }
        Ok(())
    }

    pub fn row<'v>(&mut self, values: impl Iterator<Item = Option<&'v str>>) -> io::Result<()> {
        self.record(values, true)
    }

    pub fn finish(self) -> io::Result<()> {
        self.out.flush()
    }

    fn record<'v>(
        &mut self,
        values: impl Iterator<Item = Option<&'v str>>,
        data: bool,
    ) -> io::Result<()> {
        let separator = self.options.separator.char();
        let mut first = true;
        for (index, value) in values.enumerate() {
            if !first {
                let mut buffer = [0; 4];
                self.out
                    .write_all(separator.encode_utf8(&mut buffer).as_bytes())?;
            }
            first = false;
            let kind = if data {
                self.columns.get(index).map_or(ValueKind::Text, |c| c.kind)
            } else {
                ValueKind::Text
            };
            match value {
                None => self.out.write_all(self.options.null.as_str().as_bytes())?,
                Some(value) => {
                    encode(self.out, value, kind, &self.options)?;
                }
            }
        }
        self.out
            .write_all(self.options.line_ending.as_str().as_bytes())
    }
}

/// Write slices of a cell directly. A wide value does not need a second
/// allocation for escaping, and the clipboard bound applies during encoding.
fn encode(
    out: &mut impl Write,
    value: &str,
    kind: ValueKind,
    options: &CsvOptions,
) -> io::Result<()> {
    let formula = options.escape_formulas
        && kind == ValueKind::Text
        && value.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let separator = options.separator.char();
    let null = options.null.as_str();
    let quote = options.quote_all
        || value.is_empty()
        || (!null.is_empty() && value == null)
        || value.contains([separator, '"', '\r', '\n'])
        || value.starts_with(' ')
        || value.ends_with(' ');
    if quote {
        out.write_all(b"\"")?;
    }
    if formula {
        out.write_all(b"'")?;
    }
    for part in value.split_inclusive('"') {
        out.write_all(part.as_bytes())?;
        if part.ends_with('"') {
            out.write_all(b"\"")?;
        }
    }
    if quote {
        out.write_all(b"\"")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ExportColumn> {
        vec![
            ExportColumn {
                name: "name".into(),
                kind: ValueKind::Text,
            },
            ExportColumn {
                name: "amount".into(),
                kind: ValueKind::Number,
            },
        ]
    }

    fn write(options: &CsvOptions, rows: &[[Option<&str>; 2]]) -> String {
        let columns = columns();
        let mut out = Vec::new();
        let mut writer = CsvWriter::new(&mut out, options, &columns);
        writer.begin().unwrap();
        for row in rows {
            writer.row(row.iter().copied()).unwrap();
        }
        writer.finish().unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn null_and_the_empty_string_stay_apart() {
        let text = write(
            &CsvOptions::default(),
            &[[None, Some("")], [Some(""), None]],
        );
        assert_eq!(text, "name,amount\r\n,\"\"\r\n\"\",\r\n");
    }

    #[test]
    fn values_are_quoted_when_needed_and_quotes_are_doubled() {
        let text = write(
            &CsvOptions::default(),
            &[
                [Some("a,b"), Some("1")],
                [Some("say \"hi\""), Some("2")],
                [Some("two\nlines"), Some("3")],
                [Some(" padded "), Some("4")],
                [Some("plain"), Some("5")],
            ],
        );
        assert_eq!(
            text,
            "name,amount\r\n\"a,b\",1\r\n\"say \"\"hi\"\"\",2\r\n\"two\nlines\",3\r\n\" padded \",4\r\nplain,5\r\n"
        );
    }

    #[test]
    fn a_text_value_equal_to_the_null_marker_is_quoted() {
        let options = CsvOptions {
            null: NullMarker::Null,
            ..CsvOptions::default()
        };
        let text = write(&options, &[[Some("NULL"), None], [Some("\\N"), Some("1")]]);
        assert_eq!(text, "name,amount\r\n\"NULL\",NULL\r\n\\N,1\r\n");
    }

    #[test]
    fn quote_all_quotes_every_value_but_not_nulls() {
        let options = CsvOptions {
            quote_all: true,
            header: false,
            ..CsvOptions::default()
        };
        assert_eq!(write(&options, &[[Some("a"), None]]), "\"a\",\r\n");
    }

    #[test]
    fn excel_presets_escape_formulas_in_text_columns_only() {
        let text = write(
            &Preset::Excel.options(),
            &[[Some("=SUM(A1)"), Some("-3")], [Some("-x"), Some("@1")]],
        );
        assert_eq!(text, "\u{FEFF}name,amount\r\n'=SUM(A1),-3\r\n'-x,@1\r\n");
        let text = write(
            &Preset::ExcelSemicolon.options(),
            &[[Some("a;b"), Some("1,5")]],
        );
        assert_eq!(text, "\u{FEFF}name;amount\r\n\"a;b\";1,5\r\n");
    }

    #[test]
    fn tab_separated_text_uses_tabs_and_lf() {
        let text = write(&Preset::Tsv.options(), &[[Some("a\tb"), Some("1")]]);
        assert_eq!(text, "name\tamount\n\"a\tb\"\t1\n");
        assert_eq!(Preset::Tsv.options().extension(), "tsv");
    }

    #[test]
    fn options_name_their_preset_until_they_change() {
        for preset in Preset::ALL {
            assert_eq!(preset.options().preset(), Some(preset));
        }
        let custom = CsvOptions {
            separator: Separator::Pipe,
            ..CsvOptions::default()
        };
        assert_eq!(custom.preset(), None);
        assert_eq!(CsvOptions::default().preset(), Some(Preset::Standard));
    }

    proptest::proptest! {
        /// The output is valid CSV for an independent reader. The reader
        /// cannot tell a null from an empty string, so a null must read as
        /// its marker; the unit tests above check the null rules.
        #[test]
        fn an_independent_reader_reads_the_values_back(
            rows in proptest::collection::vec(
                proptest::collection::vec(
                    proptest::option::of(r#"[a-z,;|\t"\r\n =+@'\\NUL\u{e9}\u{4e2d}-]{0,8}"#),
                    2,
                ),
                0..6,
            ),
            separator in proptest::sample::select(Separator::ALL.to_vec()),
            line_ending in proptest::sample::select(LineEnding::ALL.to_vec()),
            null in proptest::sample::select(NullMarker::ALL.to_vec()),
            quote_all: bool,
        ) {
            let options = CsvOptions {
                separator,
                line_ending,
                null,
                quote_all,
                header: true,
                byte_order_mark: false,
                escape_formulas: false,
            };
            let columns = columns();
            let mut out = Vec::new();
            let mut writer = CsvWriter::new(&mut out, &options, &columns);
            writer.begin().unwrap();
            for row in &rows {
                writer.row(row.iter().map(|value| value.as_deref())).unwrap();
            }
            writer.finish().unwrap();

            let mut reader = ::csv::ReaderBuilder::new()
                .delimiter(separator.char() as u8)
                .has_headers(true)
                .from_reader(out.as_slice());
            let header = reader.headers().unwrap().clone();
            proptest::prop_assert_eq!(header.iter().collect::<Vec<_>>(), vec!["name", "amount"]);
            let records: Vec<_> = reader.records().map(Result::unwrap).collect();
            proptest::prop_assert_eq!(records.len(), rows.len());
            for (record, row) in records.iter().zip(&rows) {
                let expected: Vec<&str> = row
                    .iter()
                    .map(|value| value.as_deref().unwrap_or(null.as_str()))
                    .collect();
                proptest::prop_assert_eq!(record.iter().collect::<Vec<_>>(), expected);
            }
        }
    }

    #[test]
    fn options_read_old_and_partial_settings() {
        let options: CsvOptions = serde_json::from_str(r#"{"separator":"Tab"}"#).unwrap();
        assert_eq!(options.separator, Separator::Tab);
        assert!(options.header);
        let round: CsvOptions =
            serde_json::from_str(&serde_json::to_string(&Preset::Excel.options()).unwrap())
                .unwrap();
        assert_eq!(round, Preset::Excel.options());
    }
}

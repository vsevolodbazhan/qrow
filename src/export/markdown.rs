//! Markdown tables and fenced, padded text tables.
use super::{Table, ValueKind, check_cancelled};
use std::{
    io::{self, Write},
    sync::atomic::AtomicBool,
};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Style {
    #[default]
    Table,
    CodeBlock,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Options {
    pub style: Style,
    pub max_cell_width: usize,
    pub row_numbers: bool,
    pub pretty: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            style: Style::Table,
            max_cell_width: 40,
            row_numbers: false,
            pretty: false,
        }
    }
}

pub fn write(
    out: &mut impl Write,
    table: &Table<'_>,
    options: &Options,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    super::validate_table(table)?;
    let _memory = super::budget::GLOBAL.allowance(64 * super::budget::MIB)?;
    check_cancelled(cancel)?;
    if options.style == Style::CodeBlock && !(1..=1000).contains(&options.max_cell_width) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cell width must be between 1 and 1000.",
        ));
    }
    let columns = table.export_columns();
    let names: Vec<_> = columns.iter().map(|column| column.name.as_str()).collect();
    match options.style {
        Style::Table => {
            let mut widths = table_widths(
                names.iter().copied(),
                columns
                    .iter()
                    .map(|column| column.kind == ValueKind::Number),
            );
            if options.pretty {
                for row in table.row_indices.clone() {
                    check_cancelled(cancel)?;
                    measure_table(&mut widths, table.values(row));
                }
            }
            let number_width = table.row_indices.end.to_string().len().max(4);
            if options.pretty {
                pretty_line(
                    out,
                    options.row_numbers.then_some(("#", number_width)),
                    names.iter().copied().map(Some),
                    &widths,
                    cancel,
                )?;
                pretty_separator(
                    out,
                    options.row_numbers.then_some(number_width),
                    &widths,
                    columns
                        .iter()
                        .map(|column| column.kind == ValueKind::Number),
                    cancel,
                )?;
            } else {
                line(
                    out,
                    options.row_numbers.then_some("#"),
                    names.iter().copied().map(Some),
                )?;
                line(
                    out,
                    options.row_numbers.then_some("---:"),
                    columns.iter().map(|column| {
                        Some(if column.kind == ValueKind::Number {
                            "---:"
                        } else {
                            "---"
                        })
                    }),
                )?;
            }
            for row in table.row_indices.clone() {
                check_cancelled(cancel)?;
                let number = (row + 1).to_string();
                if options.pretty {
                    pretty_line(
                        out,
                        options
                            .row_numbers
                            .then_some((number.as_str(), number_width)),
                        table.values(row),
                        &widths,
                        cancel,
                    )?;
                } else {
                    line(
                        out,
                        options.row_numbers.then_some(number.as_str()),
                        table.values(row),
                    )?;
                }
            }
        }
        Style::CodeBlock => {
            let cap = options.max_cell_width;
            let mut widths: Vec<_> = names.iter().map(|name| width(name, cap)).collect();
            // Measure without retaining another copy of the rows.
            for row in table.row_indices.clone() {
                check_cancelled(cancel)?;
                for (index, value) in table.values(row).enumerate() {
                    widths[index] = widths[index].max(width(value.unwrap_or("NULL"), cap));
                }
            }
            let number_width = table.row_indices.end.to_string().len().max(1);
            // Every line starts with a pipe, so a cell cannot close the fence.
            out.write_all(b"```\n")?;
            code_line(
                out,
                options.row_numbers.then_some(("#", number_width)),
                names.iter().copied().map(Some),
                &widths,
                cap,
            )?;
            code_line(
                out,
                options.row_numbers.then_some(("-", number_width)),
                widths.iter().map(|_| Some("-")),
                &widths,
                cap,
            )?;
            for row in table.row_indices.clone() {
                check_cancelled(cancel)?;
                let number = (row + 1).to_string();
                code_line(
                    out,
                    options
                        .row_numbers
                        .then_some((number.as_str(), number_width)),
                    table.values(row),
                    &widths,
                    cap,
                )?;
            }
            out.write_all(b"```\n")?;
        }
    }
    out.flush()?;
    check_cancelled(cancel)?;
    Ok(table.row_count())
}
pub(crate) fn line<'a>(
    out: &mut impl Write,
    number: Option<&'a str>,
    values: impl Iterator<Item = Option<&'a str>>,
) -> io::Result<()> {
    out.write_all(b"|")?;
    for value in number.into_iter().map(Some).chain(values) {
        out.write_all(b" ")?;
        write_cell(out, value.unwrap_or("NULL"), false)?;
        out.write_all(b" |")?;
    }
    out.write_all(b"\n")
}
fn write_cell(out: &mut impl Write, value: &str, escape_tabs: bool) -> io::Result<()> {
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\t' if escape_tabs => out.write_all(b"&#9;")?,
            '\\' => out.write_all(b"\\\\")?,
            '|' => out.write_all(b"\\|")?,
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.write_all(b"<br>")?;
            }
            _ => {
                let mut bytes = [0; 4];
                out.write_all(ch.encode_utf8(&mut bytes).as_bytes())?;
            }
        }
    }
    Ok(())
}

/// Width after Markdown escaping, without allocating another cell buffer.
fn table_width(value: &str) -> usize {
    let mut width = 0;
    let mut start = 0;
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let escaped = match ch {
            '\\' | '|' => 2,
            '\r' | '\n' | '\t' => 4,
            _ => continue,
        };
        width += value[start..index].width() + escaped;
        start = index + ch.len_utf8();
        if ch == '\r' && chars.peek().is_some_and(|(_, ch)| *ch == '\n') {
            let (index, _) = chars.next().unwrap();
            start = index + 1;
        }
    }
    width + value[start..].width()
}

pub(crate) fn table_widths<'a>(
    names: impl Iterator<Item = &'a str>,
    numeric: impl Iterator<Item = bool>,
) -> Vec<usize> {
    names
        .zip(numeric)
        .map(|(name, numeric)| table_width(name).max(if numeric { 4 } else { 3 }))
        .collect()
}

pub(crate) fn measure_table<'a>(
    widths: &mut [usize],
    values: impl Iterator<Item = Option<&'a str>>,
) {
    for (width, value) in widths.iter_mut().zip(values) {
        *width = (*width).max(table_width(value.unwrap_or("NULL")));
    }
}

pub(crate) fn pretty_line<'a>(
    out: &mut impl Write,
    number: Option<(&'a str, usize)>,
    values: impl Iterator<Item = Option<&'a str>>,
    widths: &[usize],
    cancel: &AtomicBool,
) -> io::Result<()> {
    out.write_all(b"|")?;
    for (text, width) in number.into_iter().chain(
        values
            .zip(widths)
            .map(|(value, width)| (value.unwrap_or("NULL"), *width)),
    ) {
        out.write_all(b" ")?;
        write_cell(out, text, true)?;
        padding(out, width.saturating_sub(table_width(text)), b' ', cancel)?;
        out.write_all(b" |")?;
    }
    out.write_all(b"\n")
}

pub(crate) fn pretty_separator(
    out: &mut impl Write,
    number: Option<usize>,
    widths: &[usize],
    numeric: impl Iterator<Item = bool>,
    cancel: &AtomicBool,
) -> io::Result<()> {
    out.write_all(b"|")?;
    for (width, numeric) in number
        .into_iter()
        .map(|width| (width, true))
        .chain(widths.iter().copied().zip(numeric))
    {
        out.write_all(b" ")?;
        padding(out, width - usize::from(numeric), b'-', cancel)?;
        if numeric {
            out.write_all(b":")?;
        }
        out.write_all(b" |")?;
    }
    out.write_all(b"\n")
}

fn padding(
    out: &mut impl Write,
    mut count: usize,
    byte: u8,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let block = [byte; 256];
    while count > 0 {
        check_cancelled(cancel)?;
        let size = count.min(block.len());
        out.write_all(&block[..size])?;
        count -= size;
    }
    Ok(())
}

pub(crate) fn width(value: &str, cap: usize) -> usize {
    clipped(value, cap).1
}

fn grapheme_width(text: &str) -> usize {
    if text.chars().any(char::is_control) {
        text.chars()
            .map(|ch| {
                if ch.is_control() {
                    1
                } else {
                    ch.width().unwrap_or(0)
                }
            })
            .sum()
    } else {
        text.width()
    }
}

/// Keep complete graphemes within the display width. The ellipsis uses one
/// column. Stop scanning as soon as the width is full, including for wide cells.
fn clipped(value: &str, cap: usize) -> (&str, usize, bool) {
    let mut width = 0;
    let mut truncated = false;
    for grapheme in value.graphemes(true) {
        let next = width + grapheme_width(grapheme);
        if next > cap {
            truncated = true;
            break;
        }
        width = next;
    }
    if !truncated {
        return (value, width, false);
    }
    let mut end = 0;
    let mut width = 0;
    for (index, grapheme) in value.grapheme_indices(true) {
        let next = width + grapheme_width(grapheme);
        if next > cap.saturating_sub(1) {
            break;
        }
        end = index + grapheme.len();
        width = next;
    }
    (&value[..end], width + 1, true)
}

pub(crate) fn code_line<'a>(
    out: &mut impl Write,
    number: Option<(&'a str, usize)>,
    values: impl Iterator<Item = Option<&'a str>>,
    widths: &[usize],
    cap: usize,
) -> io::Result<()> {
    out.write_all(b"|")?;
    let has_number = number.is_some();
    for (index, (text, width)) in number
        .into_iter()
        .chain(
            values
                .zip(widths)
                .map(|(value, width)| (value.unwrap_or("NULL"), *width)),
        )
        .enumerate()
    {
        out.write_all(b" ")?;
        let limit = if has_number && index == 0 {
            cap.max(width)
        } else {
            cap
        };
        let (prefix, written, truncated) = clipped(text, limit);
        for ch in prefix.chars() {
            let ch = if ch.is_control() { ' ' } else { ch };
            let mut bytes = [0; 4];
            out.write_all(ch.encode_utf8(&mut bytes).as_bytes())?;
        }
        if truncated {
            out.write_all("…".as_bytes())?;
        }
        for _ in written..width {
            out.write_all(b" ")?;
        }
        out.write_all(b" |")?;
    }
    out.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{export::Rows, model::Column};
    #[test]
    fn tables_escape_pipes_backslashes_and_crlf_and_align_numbers() {
        let columns = vec![
            Column {
                name: "text|name".into(),
                data_type: "text".into(),
            },
            Column {
                name: "amount".into(),
                data_type: "numeric".into(),
            },
        ];
        let rows: Rows = vec![
            vec![
                Some("a\\|b\r\nc".into()),
                Some("12345678901234567890.001".into()),
            ],
            vec![None, None],
        ]
        .into();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..2,
            column_indices: 0..=1,
        };
        let mut out = Vec::new();
        write(
            &mut out,
            &table,
            &Options {
                row_numbers: true,
                ..Options::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "| # | text\\|name | amount |\n| ---: | --- | ---: |\n| 1 | a\\\\\\|b<br>c | 12345678901234567890.001 |\n| 2 | NULL | NULL |\n"
        );
    }
    #[test]
    fn pretty_tables_align_escaped_unicode_values_and_keep_valid_numeric_markers() {
        let columns = vec![
            Column {
                name: "text|name".into(),
                data_type: "text".into(),
            },
            Column {
                name: "n".into(),
                data_type: "int".into(),
            },
        ];
        let rows: Rows = vec![
            vec![Some("α|β\r\n👩‍💻".into()), Some("-12".into())],
            vec![Some("\\".into()), None],
            vec![None, Some("1000".into())],
            vec![Some("a\tb".into()), Some("2".into())],
        ]
        .into();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..4,
            column_indices: 0..=1,
        };
        let options = Options {
            pretty: true,
            row_numbers: true,
            ..Options::default()
        };
        let mut out = Vec::new();
        assert_eq!(
            write(&mut out, &table, &options, &AtomicBool::new(false)).unwrap(),
            4
        );
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("| #    | text\\|name | n    |\n| ---: | ---------- | ---: |\n"),
            "{text}"
        );
        assert!(text.contains("α\\|β<br>👩‍💻"));
        assert!(text.lines().all(|line| line.width() == 28), "{text}");
        assert_eq!(table_width("👩‍💻é中文"), 7);
        assert_eq!(table_width("a\r\nb"), 6);
        assert_eq!(table_width("a\tb"), 6);
        assert!(text.contains("a&#9;b"));
        assert!(!text.contains('\t'));
        assert_eq!(table_width("\\|"), 4);
        assert!(write(&mut Vec::new(), &table, &options, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn cancellation_stops_padding_before_a_large_pretty_header_completes() {
        struct CancelOnWrite<'a> {
            cancel: &'a AtomicBool,
            bytes: usize,
        }
        impl Write for CancelOnWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes += bytes.len();
                self.cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let columns = vec![Column {
            name: "text".into(),
            data_type: "text".into(),
        }];
        let rows: Rows = vec![vec![Some("x".repeat(100_000))]].into();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..1,
            column_indices: 0..=0,
        };
        let cancel = AtomicBool::new(false);
        let mut out = CancelOnWrite {
            cancel: &cancel,
            bytes: 0,
        };
        let error = write(
            &mut out,
            &table,
            &Options {
                pretty: true,
                ..Options::default()
            },
            &cancel,
        )
        .unwrap_err();
        assert!(
            error
                .get_ref()
                .is_some_and(|cause| cause.is::<super::super::Cancelled>())
        );
        assert!(
            out.bytes < 100,
            "wrote {} bytes after cancellation",
            out.bytes
        );
    }

    #[test]
    fn old_markdown_settings_keep_compact_tables() {
        let options: Options =
            serde_json::from_str(r#"{"style":"Table","row_numbers":true}"#).unwrap();
        assert!(!options.pretty);
        let changed = Options {
            pretty: true,
            ..options
        };
        let saved = serde_json::to_string(&changed).unwrap();
        assert!(serde_json::from_str::<Options>(&saved).unwrap().pretty);
    }

    #[test]
    fn clipping_keeps_graphemes_and_counts_display_columns() {
        assert_eq!(clipped("👩‍💻é中文", 4), ("👩‍💻é", 4, true));
        assert_eq!(clipped("a\r\nb", 4), ("a\r\nb", 4, false));
        assert_eq!(clipped("😀abcdef", 1), ("", 1, true));
    }

    #[test]
    fn code_blocks_are_padded_bounded_and_cannot_close_their_fence() {
        let columns = vec![Column {
            name: "text".into(),
            data_type: "text".into(),
        }];
        let rows: Rows = vec![
            vec![Some("😀最初abc".into())],
            vec![Some("\n```\n```".into())],
            vec![None],
        ]
        .into();
        let table = Table {
            columns: &columns,
            rows: &rows,
            row_indices: 0..3,
            column_indices: 0..=0,
        };
        let mut out = Vec::new();
        write(
            &mut out,
            &table,
            &Options {
                style: Style::CodeBlock,
                max_cell_width: 4,
                row_numbers: false,
                ..Options::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("```\n| text |\n| -    |\n| 😀…  |\n"));
        assert!(text.ends_with("| NULL |\n```\n"));
        assert_eq!(text.lines().filter(|line| *line == "```").count(), 2);
        assert!(
            write(
                &mut Vec::new(),
                &table,
                &Options {
                    style: Style::CodeBlock,
                    max_cell_width: 0,
                    ..Options::default()
                },
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}

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
}
impl Default for Options {
    fn default() -> Self {
        Self {
            style: Style::Table,
            max_cell_width: 40,
            row_numbers: false,
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
            for row in table.row_indices.clone() {
                check_cancelled(cancel)?;
                let number = (row + 1).to_string();
                line(
                    out,
                    options.row_numbers.then_some(number.as_str()),
                    table.values(row),
                )?;
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
fn line<'a>(
    out: &mut impl Write,
    number: Option<&'a str>,
    values: impl Iterator<Item = Option<&'a str>>,
) -> io::Result<()> {
    out.write_all(b"|")?;
    for value in number.into_iter().map(Some).chain(values) {
        out.write_all(b" ")?;
        let value = value.unwrap_or("NULL");
        let mut chars = value.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
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
        out.write_all(b" |")?;
    }
    out.write_all(b"\n")
}
fn width(value: &str, cap: usize) -> usize {
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

fn code_line<'a>(
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

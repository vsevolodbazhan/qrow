//! One format envelope over any number of committed spool batches.
use super::{
    Format, Settings, Table, ValueKind, budget, check_cancelled, csv, json, markdown, parquet,
    spool::Spool,
};
use std::{
    io::{self, Write},
    sync::{Arc, atomic::AtomicBool},
};

pub fn write(
    out: &mut (impl Write + Send),
    spool: &Arc<Spool>,
    settings: &Settings,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    settings.validate()?;
    check_cancelled(cancel)?;
    match settings.format {
        Format::Csv => csv(out, spool, &settings.csv, cancel),
        Format::Json | Format::JsonLines => json(
            out,
            spool,
            &settings.json,
            settings.format == Format::JsonLines,
            cancel,
        ),
        Format::Markdown => markdown(out, spool, &settings.markdown, cancel),
        Format::Parquet => parquet::write_spool(out, spool, &settings.parquet, cancel),
    }
}

fn table<'a>(spool: &'a Spool, rows: &'a super::Rows) -> Table<'a> {
    Table {
        columns: spool.columns(),
        rows,
        row_indices: 0..rows.len(),
        column_indices: 0..=spool.columns().len() - 1,
    }
}

fn csv(
    out: &mut impl Write,
    spool: &Arc<Spool>,
    options: &csv::CsvOptions,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    let _memory = budget::GLOBAL.allowance(64 * budget::MIB)?;
    let mut reader = spool.reader()?;
    let columns: Vec<_> = spool
        .columns()
        .iter()
        .map(super::ExportColumn::from)
        .collect();
    let mut writer = csv::CsvWriter::new(out, options, &columns);
    writer.begin()?;
    let mut count = 0;
    while let Some(batch) = reader.next(cancel)? {
        let table = table(spool, batch.rows());
        for row in table.row_indices.clone() {
            check_cancelled(cancel)?;
            writer.row(table.values(row))?;
            count += 1;
        }
    }
    writer.finish()?;
    check_cancelled(cancel)?;
    Ok(count)
}

fn json(
    out: &mut impl Write,
    spool: &Arc<Spool>,
    options: &json::Options,
    lines: bool,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    let mut reader = spool.reader()?;
    let mut writer = json::Stream::new(out, spool.columns().iter(), options, lines)?;
    let mut count = 0;
    while let Some(batch) = reader.next(cancel)? {
        let table = table(spool, batch.rows());
        writer.batch(&table, count, cancel)?;
        count += table.row_count();
    }
    writer.finish(cancel)
}

fn markdown(
    out: &mut impl Write,
    spool: &Arc<Spool>,
    options: &markdown::Options,
    cancel: &AtomicBool,
) -> io::Result<usize> {
    let _memory = budget::GLOBAL.allowance(64 * budget::MIB)?;
    let names: Vec<_> = spool
        .columns()
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    let mut widths: Vec<_> = names
        .iter()
        .map(|name| markdown::width(name, options.max_cell_width))
        .collect();
    let mut count = 0usize;
    if options.style == markdown::Style::CodeBlock {
        let mut reader = spool.reader()?;
        while let Some(batch) = reader.next(cancel)? {
            let table = table(spool, batch.rows());
            for row in table.row_indices.clone() {
                check_cancelled(cancel)?;
                for (column, text) in table.values(row).enumerate() {
                    widths[column] = widths[column].max(markdown::width(
                        text.unwrap_or("NULL"),
                        options.max_cell_width,
                    ));
                }
            }
            count += table.row_count();
        }
    }
    let number_width = count.to_string().len().max(1);
    match options.style {
        markdown::Style::Table => {
            markdown::line(
                out,
                options.row_numbers.then_some("#"),
                names.iter().copied().map(Some),
            )?;
            markdown::line(
                out,
                options.row_numbers.then_some("---:"),
                spool.columns().iter().map(|column| {
                    Some(if ValueKind::of(&column.data_type) == ValueKind::Number {
                        "---:"
                    } else {
                        "---"
                    })
                }),
            )?;
        }
        markdown::Style::CodeBlock => {
            out.write_all(b"```\n")?;
            markdown::code_line(
                out,
                options.row_numbers.then_some(("#", number_width)),
                names.iter().copied().map(Some),
                &widths,
                options.max_cell_width,
            )?;
            markdown::code_line(
                out,
                options.row_numbers.then_some(("-", number_width)),
                widths.iter().map(|_| Some("-")),
                &widths,
                options.max_cell_width,
            )?;
        }
    }
    let mut reader = spool.reader()?;
    count = 0;
    while let Some(batch) = reader.next(cancel)? {
        let table = table(spool, batch.rows());
        for row in table.row_indices.clone() {
            check_cancelled(cancel)?;
            count += 1;
            let number = count.to_string();
            match options.style {
                markdown::Style::Table => markdown::line(
                    out,
                    options.row_numbers.then_some(number.as_str()),
                    table.values(row),
                )?,
                markdown::Style::CodeBlock => markdown::code_line(
                    out,
                    options
                        .row_numbers
                        .then_some((number.as_str(), number_width)),
                    table.values(row),
                    &widths,
                    options.max_cell_width,
                )?,
            }
        }
    }
    if options.style == markdown::Style::CodeBlock {
        out.write_all(b"```\n")?;
    }
    out.flush()?;
    check_cancelled(cancel)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, Row};

    #[test]
    fn multiple_batches_match_preview_files_in_every_format() {
        let columns: Vec<_> = [
            "STRING",
            "numeric",
            "boolean",
            "varbinary",
            "json",
            "timestamp(9)",
        ]
        .into_iter()
        .map(|kind| Column {
            name: kind.into(),
            data_type: kind.into(),
        })
        .collect();
        let mut rows = Vec::new();
        for index in 0..1003 {
            let row: Row = vec![
                Some(format!("Café | {index}")),
                Some(if index % 2 == 0 { "123.45" } else { "0.1234" }.into()),
                Some("true".into()),
                Some("AFz/".into()),
                Some("[1,\n2]".into()),
                Some("2026-10-09 01:02:03.123456789".into()),
            ];
            rows.push(row);
        }
        let (spool, mut producer) =
            Spool::new(&columns, &super::super::Context::default()).unwrap();
        for chunk in rows.chunks(400) {
            producer.append(chunk, &AtomicBool::new(false)).unwrap();
        }
        producer.finish(&AtomicBool::new(false)).unwrap();
        let rows = super::super::Rows::from(rows);
        let table = Table {
            columns: &columns,
            row_indices: 0..rows.len(),
            column_indices: 0..=columns.len() - 1,
            rows: &rows,
        };
        for format in Format::ALL {
            let mut settings = Settings {
                format,
                ..Settings::default()
            };
            settings.csv.byte_order_mark = true;
            settings.markdown.row_numbers = true;
            let mut expected = Vec::new();
            super::super::write(&mut expected, &table, &settings, &AtomicBool::new(false)).unwrap();
            let mut actual = Vec::new();
            assert_eq!(
                write(&mut actual, &spool, &settings, &AtomicBool::new(false)).unwrap(),
                rows.len()
            );
            assert_eq!(actual, expected, "{}", format.label());
        }
        let mut settings = Settings {
            format: Format::Markdown,
            ..Settings::default()
        };
        settings.markdown.style = markdown::Style::CodeBlock;
        settings.markdown.row_numbers = true;
        let mut expected = Vec::new();
        super::super::write(&mut expected, &table, &settings, &AtomicBool::new(false)).unwrap();
        let mut actual = Vec::new();
        write(&mut actual, &spool, &settings, &AtomicBool::new(false)).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn binary_groups_leave_headroom_for_the_next_encoded_and_decoded_record() {
        use ::parquet::file::reader::{FileReader, SerializedFileReader};
        use ::parquet::record::RowAccessor;
        let columns = vec![Column {
            name: "bytes".into(),
            data_type: "varbinary".into(),
        }];
        let (spool, mut producer) =
            Spool::new(&columns, &super::super::Context::default()).unwrap();
        // Raw values alone fit in a 64 MiB group; the typed binary payload
        // makes its retained reader reservation substantially larger.
        let rows: Vec<Row> = (0..60)
            .map(|_| vec![Some("A".repeat(budget::MIB))])
            .collect();
        producer.append(&rows, &AtomicBool::new(false)).unwrap();
        drop(rows);
        let rows: Vec<Row> = (0..17)
            .map(|_| vec![Some("A".repeat(budget::MIB))])
            .collect();
        producer.append(&rows, &AtomicBool::new(false)).unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        drop(rows);
        let file = tempfile::tempfile().unwrap();
        assert_eq!(
            write(
                &mut &file,
                &spool,
                &Settings {
                    format: Format::Parquet,
                    ..Settings::default()
                },
                &AtomicBool::new(false)
            )
            .unwrap(),
            77
        );
        let reader = SerializedFileReader::new(file).unwrap();
        assert!(reader.metadata().num_row_groups() > 1);
        assert_eq!(reader.metadata().file_metadata().num_rows(), 77);
        for row in reader.get_row_iter(None).unwrap() {
            assert_eq!(
                row.unwrap().get_bytes(0).unwrap().len(),
                3 * budget::MIB / 4
            );
        }
    }

    #[test]
    fn a_producer_failure_removes_partial_output_and_keeps_the_destination() {
        use std::sync::mpsc;
        let columns = vec![Column {
            name: "value".into(),
            data_type: "STRING".into(),
        }];
        let (spool, mut producer) =
            Spool::new(&columns, &super::super::Context::default()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.json");
        std::fs::write(&path, "original").unwrap();
        let (started, ready) = mpsc::channel();
        let writer_spool = spool.clone();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            super::super::save(&writer_path, &AtomicBool::new(false), |out| {
                started.send(()).unwrap();
                write(
                    out,
                    &writer_spool,
                    &Settings {
                        format: Format::Json,
                        ..Settings::default()
                    },
                    &AtomicBool::new(false),
                )
            })
        });
        ready.recv().unwrap();
        producer
            .append(&[vec![Some("partial".into())]], &AtomicBool::new(false))
            .unwrap();
        spool.fail("injected download failure");
        assert!(
            writer
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("injected download failure")
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

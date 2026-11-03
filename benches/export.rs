//! One producer and one writer, measured in fresh processes with OS peak RSS.
use qrow::{
    export::{self, Format, spool::Spool},
    model::{Column, Row},
};
use std::{process::Command, sync::atomic::AtomicBool, time::Instant};

fn decode_sample(case: &str) {
    use qrow::connector::{
        hive::decode_columns,
        t_c_l_i_service::{TColumn, TI64Column, TStringColumn},
    };
    let (count, width): (usize, usize) = match case {
        "narrow" => (10_000, 8),
        "wide" => (1024, 8),
        "null_heavy" => (5000, 64),
        _ => unreachable!(),
    };
    let columns: Vec<_> = (0..width)
        .map(|column| {
            if case == "narrow" {
                TColumn::I64Val(TI64Column::new(
                    (0..count)
                        .map(|row| i64::MIN + row as i64 + column as i64)
                        .collect(),
                    vec![],
                ))
            } else {
                let mut nulls = vec![0; count.div_ceil(8)];
                let values = (0..count)
                    .map(|row| {
                        if case == "null_heavy" && (row + column) % 10 != 0 {
                            nulls[row / 8] |= 1 << (row % 8);
                            String::new()
                        } else if case == "wide" {
                            format!("{row:08}-{}", "日😀x".repeat(128))
                        } else {
                            format!("row-{row:08}-column-{column:04}")
                        }
                    })
                    .collect();
                TColumn::StringVal(TStringColumn::new(values, nulls))
            }
        })
        .collect();
    let mut elapsed = std::time::Duration::ZERO;
    let mut checksum = 0usize;
    for _ in 0..20 {
        let input = columns.clone();
        let started = Instant::now();
        let rows = decode_columns(std::hint::black_box(input), width).unwrap();
        elapsed += started.elapsed();
        assert_eq!(rows.len(), count);
        checksum += rows
            .iter()
            .flatten()
            .flatten()
            .map(String::len)
            .sum::<usize>();
        std::hint::black_box(&rows);
    }
    assert!(checksum > 0);
    println!(
        "{}",
        serde_json::json!({"elapsed_ns": elapsed.as_nanos() as f64, "cells": count*width*20})
    );
}

fn peak_rss(stderr: &[u8]) -> f64 {
    let stderr = String::from_utf8_lossy(stderr);
    let rss = stderr
        .lines()
        .find(|line| {
            line.to_ascii_lowercase()
                .contains("maximum resident set size")
        })
        .expect("OS did not report peak RSS");
    if cfg!(target_os = "macos") {
        rss.split_whitespace()
            .next()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            / 1_048_576.
    } else {
        rss.rsplit(':')
            .next()
            .unwrap()
            .trim()
            .parse::<f64>()
            .unwrap()
            / 1024.
    }
}

fn decoder_probes(executable: &std::path::Path) {
    for case in ["narrow", "wide", "null_heavy"] {
        let mut latency = Vec::new();
        let mut peak = Vec::new();
        for round in 0..4 {
            let output = Command::new("/usr/bin/time")
                .arg(if cfg!(target_os = "macos") {
                    "-l"
                } else {
                    "-v"
                })
                .arg(executable)
                .args(["--decode", case])
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let sample: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            if round > 0 {
                latency.push(
                    sample["elapsed_ns"].as_f64().unwrap() / sample["cells"].as_f64().unwrap(),
                );
                peak.push(peak_rss(&output.stderr));
            }
        }
        report(
            &format!("export.decode.{case}.ns_per_cell"),
            median(&mut latency),
            "ns/cell",
            if case == "narrow" { 400. } else { 50. },
        );
        report(
            &format!("export.decode.{case}.peak_rss"),
            peak.into_iter().fold(0., f64::max),
            "MiB",
            384.,
        );
    }
}

fn report(probe: &str, value: f64, unit: &str, budget: f64) {
    println!(
        "QROW_PERF {}",
        serde_json::json!({"probe":probe,"value":value,"unit":unit,"budget":budget})
    );
    assert!(
        value <= budget,
        "{probe} exceeded its budget: {value} {unit}"
    );
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn columns(case: &str) -> Vec<Column> {
    let count = match case {
        "narrow" => 4,
        "wide" => 8,
        "null_heavy" => 64,
        _ => panic!("Invalid case"),
    };
    (0..count)
        .map(|index| Column {
            name: format!("c{index}"),
            data_type: if case == "narrow" {
                "BIGINT"
            } else {
                "VARCHAR"
            }
            .into(),
        })
        .collect()
}
fn rows(case: &str, start: usize, end: usize, columns: usize) -> Vec<Row> {
    (start..end)
        .map(|row| {
            (0..columns)
                .map(|column| {
                    if case == "null_heavy" && (row + column) % 10 != 0 {
                        None
                    } else if case == "narrow" {
                        Some((row as i64 - column as i64).to_string())
                    } else if case == "wide" {
                        Some(format!("{row:08}-{column:04}-{}", "日😀x".repeat(128)))
                    } else {
                        Some(format!("row-{row:08}-column-{column:04}"))
                    }
                })
                .collect()
        })
        .collect()
}
fn sample(case: &str, format: Format) {
    let total = match case {
        "narrow" => 100_000,
        "wide" => 8192,
        "null_heavy" => 16_384,
        _ => panic!("Invalid case"),
    };
    let columns = columns(case);
    let width = columns.len();
    let (spool, mut producer) = Spool::new(&columns, &export::Context::default()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("output");
    let destination = output.clone();
    let source = spool.clone();
    let started = Instant::now();
    let writer = std::thread::spawn(move || {
        let mut settings = export::Settings::default();
        settings.format = format;
        export::save(&destination, &AtomicBool::new(false), |out| {
            export::stream::write(out, &source, &settings, &AtomicBool::new(false))
        })
        .unwrap()
    });
    let mut payload_bytes = 0usize;
    for start in (0..total).step_by(256) {
        let batch = rows(case, start, (start + 256).min(total), width);
        payload_bytes += batch
            .iter()
            .flatten()
            .filter_map(Option::as_ref)
            .map(String::len)
            .sum::<usize>();
        producer.append(&batch, &AtomicBool::new(false)).unwrap();
    }
    producer.finish(&AtomicBool::new(false)).unwrap();
    assert_eq!(writer.join().unwrap(), total);
    println!(
        "{}",
        serde_json::json!({"elapsed_ms":started.elapsed().as_secs_f64()*1000., "payload_bytes":payload_bytes, "spool_bytes":spool.bytes(),"output_bytes":std::fs::metadata(output).unwrap().len()})
    );
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--decode") {
        decode_sample(&args[2]);
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--sample") {
        sample(
            &args[2],
            if args[3] == "csv" {
                Format::Csv
            } else {
                Format::Parquet
            },
        );
        return;
    }
    let executable = std::env::current_exe().unwrap();
    decoder_probes(&executable);
    for case in ["narrow", "wide", "null_heavy"] {
        for format in ["csv", "parquet"] {
            let mut latency = Vec::new();
            let mut peak = Vec::new();
            let mut ratio = Vec::new();
            for round in 0..4 {
                let mut command = Command::new("/usr/bin/time");
                command.arg(if cfg!(target_os = "macos") {
                    "-l"
                } else {
                    "-v"
                });
                let output = command
                    .arg(&executable)
                    .args(["--sample", case, format])
                    .env("LC_ALL", "C")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let sample: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                let rss = peak_rss(&output.stderr);
                if round > 0 {
                    latency.push(
                        sample["elapsed_ms"].as_f64().unwrap()
                            / (sample["payload_bytes"].as_f64().unwrap() / 1_048_576.),
                    );
                    peak.push(rss);
                    ratio.push(
                        sample["spool_bytes"].as_f64().unwrap()
                            / sample["output_bytes"].as_f64().unwrap(),
                    );
                }
            }
            report(
                &format!("export.{case}.{format}.ms_per_mib"),
                median(&mut latency),
                "ms/MiB",
                match (case, format) {
                    ("narrow", _) => 400.,
                    ("wide", "csv") => 20.,
                    ("wide", _) => 10.,
                    _ => 800.,
                },
            );
            // Check the largest observed peak, not the median.
            report(
                &format!("export.{case}.{format}.peak_rss"),
                peak.into_iter().fold(0., f64::max),
                "MiB",
                384.,
            );
            if format == "csv" {
                report(
                    &format!("export.{case}.spool_csv_ratio"),
                    median(&mut ratio),
                    "ratio",
                    match case {
                        "narrow" => 3.,
                        "wide" => 1.25,
                        _ => 2.,
                    },
                );
            }
        }
    }
}

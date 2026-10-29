//! The cost of dbt manifests: the parse, the saved index, a lookup, and a
//! search. Reports medians and fails on gross regressions.
#[path = "../tests/support/dbt_manifest.rs"]
mod dbt_manifest;

use std::{hint::black_box, time::Instant};

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `--write DIR` writes the manifests, and `--parse FILE` reads and
    // parses one, so that a separate process can measure peak memory.
    if let Some(position) = args.iter().position(|arg| arg == "--write") {
        let directory = std::path::Path::new(&args[position + 1]);
        for (name, shape) in [
            ("70.json", dbt_manifest::Shape::sized(70)),
            ("70-parse.json", dbt_manifest::Shape::sized(70).parsed()),
            ("70-fusion.json", dbt_manifest::Shape::sized(70).fusion()),
            (
                "many.json",
                dbt_manifest::Shape {
                    models: 6_000,
                    sources: 2_000,
                    macros: 2_000,
                    columns: 8,
                    compiled: true,
                    fusion: false,
                },
            ),
        ] {
            std::fs::write(directory.join(name), dbt_manifest::generate(&shape)).unwrap();
        }
        return;
    }
    if let Some(position) = args.iter().position(|arg| arg == "--parse") {
        let started = Instant::now();
        let bytes = std::fs::read(&args[position + 1]).unwrap();
        let read = started.elapsed().as_secs_f64() * 1000.;
        let index = qrow::dbt::parse(&bytes).unwrap();
        let total = started.elapsed().as_secs_f64() * 1000.;
        let stamp = qrow::dbt::saved::Stamp {
            len: bytes.len() as u64,
            modified: None,
        };
        drop(bytes);
        println!(
            "read {read:.1} ms, read and parse {total:.1} ms, {} entries, {} tests",
            index.entries().len(),
            index.test_count()
        );
        let saved = qrow::dbt::saved::encode(&qrow::dbt::saved::Saved {
            manifest: args[position + 1].clone().into(),
            stamp,
            refreshed: std::time::SystemTime::now(),
            index,
        });
        let started = Instant::now();
        black_box(qrow::dbt::saved::decode(black_box(&saved)).unwrap());
        println!(
            "saved index {:.1} MB, load {:.1} ms",
            saved.len() as f64 / 1e6,
            started.elapsed().as_secs_f64() * 1000.
        );
        return;
    }
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|arg| arg.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![5, 15, 35, 70]
    } else {
        sizes
    };
    for megabytes in sizes {
        let started = Instant::now();
        let bytes = dbt_manifest::generate(&dbt_manifest::Shape::sized(megabytes));
        let generated = started.elapsed().as_secs_f64();
        let mut parse = Vec::new();
        let mut index = None;
        for _ in 0..7 {
            let started = Instant::now();
            let parsed = black_box(qrow::dbt::parse(black_box(&bytes))).unwrap();
            parse.push(started.elapsed().as_secs_f64() * 1000.);
            index = Some(parsed);
        }
        let index = index.unwrap();
        let saved = qrow::dbt::saved::Saved {
            manifest: "/synthetic/manifest.json".into(),
            refreshed: std::time::UNIX_EPOCH,
            stamp: qrow::dbt::saved::Stamp {
                len: bytes.len() as u64,
                modified: None,
            },
            index,
        };
        let mut write = Vec::new();
        let mut saved_bytes = Vec::new();
        for _ in 0..7 {
            let started = Instant::now();
            saved_bytes = qrow::dbt::saved::encode(black_box(&saved));
            write.push(started.elapsed().as_secs_f64() * 1000.);
        }
        let mut load = Vec::new();
        for _ in 0..7 {
            let started = Instant::now();
            let loaded = qrow::dbt::saved::decode(black_box(&saved_bytes)).unwrap();
            load.push(started.elapsed().as_secs_f64() * 1000.);
            black_box(loaded);
        }
        let index = saved.index;
        let mut lookup = Vec::new();
        let ids: Vec<String> = index
            .entries()
            .iter()
            .step_by(7)
            .map(|entry| entry.unique_id.to_string())
            .collect();
        for _ in 0..7 {
            let started = Instant::now();
            for id in &ids {
                let position = index.find(black_box(id)).unwrap();
                black_box(index.tests(position));
                black_box(index.children(position));
            }
            lookup.push(started.elapsed().as_secs_f64() * 1e6 / ids.len() as f64);
        }
        let mut search = Vec::new();
        for _ in 0..7 {
            let started = Instant::now();
            black_box(index.search(black_box("warehouse"), None, 50));
            black_box(index.search(black_box("entity_0099"), None, 50));
            black_box(index.search(black_box("no such text anywhere"), None, 50));
            search.push(started.elapsed().as_secs_f64() * 1000. / 3.);
        }
        let (parse, write, load, lookup, search) = (
            median(parse),
            median(write),
            median(load),
            median(lookup),
            median(search),
        );
        println!(
            "manifest {:.1} MB (generated in {generated:.1} s): {} entries, {} tests",
            bytes.len() as f64 / 1e6,
            index.entries().len(),
            index.test_count(),
        );
        println!(
            "  parse {parse:.1} ms; saved index {:.1} MB, write {write:.1} ms, load {load:.1} ms; lookup {lookup:.2} us; search {search:.2} ms",
            saved_bytes.len() as f64 / 1e6,
        );
        // Broad budgets catch order-of-magnitude regressions. A parse of
        // 70 MB takes about 70 ms, and a load about 8 ms, on an M3.
        let scale = megabytes as f64 / 70.;
        for (probe, value, budget) in [
            ("parse", parse, 50. + 700. * scale),
            ("load", load, 5. + 50. * scale),
            ("search", search, 1. + 10. * scale),
        ] {
            // qtest collects this line; see docs/testing.md.
            println!(
                "QROW_PERF {{\"probe\":\"dbt.{probe}.{megabytes}mb\",\"value\":{value},\"unit\":\"ms\",\"budget\":{budget}}}"
            );
            assert!(
                value < budget,
                "dbt {probe} of {megabytes} MB exceeded {budget} ms: {value:.1} ms"
            );
        }
    }
}

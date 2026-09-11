//! A repeatable release benchmark. Reports medians and fails on gross latency regressions.
use qrow::sql;
use std::{hint::black_box, time::Instant};

fn main() {
    let line = "SELECT route, COUNT(*) FROM flights WHERE note = '日本語; safe' GROUP BY route;\n";
    for bytes in [10_000, 100_000, 1_000_000] {
        let source = line.repeat(bytes / line.len() + 1);
        for _ in 0..3 {
            black_box(sql::validate_single(black_box(&source))).ok();
        }
        let mut samples = Vec::new();
        for _ in 0..21 {
            let started = Instant::now();
            black_box(sql::validate_single(black_box(&source))).ok();
            samples.push(started.elapsed().as_secs_f64() * 1000.);
        }
        samples.sort_by(f64::total_cmp);
        let median = samples[samples.len() / 2];
        println!(
            "validate_single bytes={} median_ms={median:.3}",
            source.len()
        );
        // Broad budgets catch order-of-magnitude regressions; compare reports for small changes.
        let budget_ms = match bytes {
            10_000 => 5.,
            100_000 => 25.,
            _ => 250.,
        };
        assert!(
            median < budget_ms,
            "SQL validation exceeded {budget_ms} ms: {median:.3} ms"
        );
    }
}

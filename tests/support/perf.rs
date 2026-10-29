//! Performance probes. Each probe prints one `QROW_PERF` JSON line, which
//! qtest collects into the run summary, and fails above its budget. Budgets
//! catch large regressions; `./qtest compare` measures small ones.
use std::time::{Duration, Instant};

/// The median of `samples`, in milliseconds.
pub fn median_ms(samples: &[Duration]) -> f64 {
    let mut values: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.).collect();
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Runs `work` `warmup` times, then `runs` times, and returns the timed runs.
pub fn sample(warmup: usize, runs: usize, mut work: impl FnMut()) -> Vec<Duration> {
    for _ in 0..warmup {
        work();
    }
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            work();
            started.elapsed()
        })
        .collect()
}

/// Prints a measurement and fails when it exceeds `budget`. Lower is better.
pub fn report(probe: &str, value: f64, unit: &str, budget: f64) {
    println!(
        "QROW_PERF {}",
        serde_json::json!({ "probe": probe, "value": value, "unit": unit, "budget": budget })
    );
    assert!(
        value <= budget,
        "{probe} is {value:.2} {unit}, over its budget of {budget} {unit}"
    );
}

//! Latency probes against the real servers. Run with `./qtest run perf-e2e`.
use crate::support::fixture::Kyuubi;
use crate::support::perf::{median_ms, report};
use crate::support::{TestApp, cell};
use gpui_kit::TestAppContext;
use std::time::{Duration, Instant};

/// Wall time from the action until `ready`, polled on each frame.
fn time_until(
    app: &TestApp,
    cx: &mut TestAppContext,
    act: impl FnOnce(&TestApp, &mut TestAppContext),
    ready: &str,
    row: usize,
) -> Duration {
    let started = Instant::now();
    act(app, cx);
    let expected = ready.to_owned();
    app.wait_until(cx, ready, Duration::from_secs(60), |window, _| {
        cell(window, row, 1).as_deref() == Some(expected.as_str())
    });
    started.elapsed()
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-e2e"]
fn query_to_first_row(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().connections(&["Alpha"], "");
    let app = TestApp::launch_with(cx, workspace, credentials);
    // The first query opens the session; later queries reuse it.
    app.run_complete(cx, "SELECT 'warm' AS value");
    let samples: Vec<_> = (0..5)
        .map(|index| {
            let value = format!("latency-{index}");
            app.type_sql(cx, &format!("SELECT '{value}' AS value"));
            time_until(&app, cx, |app, cx| app.click(cx, "run"), &value, 0)
        })
        .collect();
    report("e2e.query.first_row", median_ms(&samples), "ms", 5000.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-e2e"]
fn next_page_of_a_long_result(cx: &mut TestAppContext) {
    let (workspace, credentials) = Kyuubi::get().connections(&["Alpha"], "");
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_sql(cx, "SELECT concat('page-', lpad(CAST(id AS STRING), 5, '0')) AS value FROM range(5001) ORDER BY id");
    app.wait_status(cx, "Preview: More rows available");
    let samples: Vec<_> = (1..=4)
        .map(|page| {
            let first = format!("page-{:05}", page * 1000);
            time_until(&app, cx, |app, cx| app.click(cx, "next-page"), &first, 0)
        })
        .collect();
    report("e2e.results.next_page", median_ms(&samples), "ms", 5000.);
}

//! The shared wait contract must coordinate real threads and GPUI timers.
use crate::support::TestApp;
use gpui_kit::TestAppContext;
use qrow::model::Workspace;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[gpui_kit::test]
fn polling_does_not_accelerate_application_time(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    let virtual_start = cx.executor().now();
    let started = Instant::now();
    app.wait_until(cx, "wall time", Duration::from_secs(2), |_, _| {
        started.elapsed() >= Duration::from_millis(80)
    });
    assert!(
        cx.executor().now().duration_since(virtual_start)
            <= started.elapsed() + Duration::from_millis(25)
    );
}

#[gpui_kit::test]
fn application_timers_progress_while_waiting_for_real_threads(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    let timer_done = Arc::new(AtomicBool::new(false));
    let observed = timer_done.clone();
    let timer = cx.executor().timer(Duration::from_millis(150));
    cx.executor()
        .spawn(async move {
            timer.await;
            observed.store(true, Ordering::SeqCst);
        })
        .detach();
    let thread_done = Arc::new(AtomicBool::new(false));
    let observed = thread_done.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        observed.store(true, Ordering::SeqCst);
    });
    app.wait_until(
        cx,
        "the external response and application timer",
        Duration::from_secs(2),
        |_, _| thread_done.load(Ordering::SeqCst) && timer_done.load(Ordering::SeqCst),
    );
    thread.join().unwrap();
}

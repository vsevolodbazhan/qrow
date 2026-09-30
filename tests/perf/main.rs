//! Performance probes of the real window. Run with `./qtest run perf-ui`,
//! which builds them with the `perf` profile, like a release build.
#[path = "../support/mod.rs"]
mod support;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    InputEvent as _, Keystroke, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px,
};
use qrow::model::{SavedTab, Workspace};
use std::time::Instant;
use support::assistant::FakeCodex;
use support::perf::{median_ms, report, sample};
use support::{MemoryCredentials, TestApp, cell, elements, offline_profile};

fn demo(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::launch_demo(cx);
    app.wait_until(
        cx,
        "the demo rows",
        std::time::Duration::from_secs(10),
        |window, _| cell(window, 0, 1).is_some(),
    );
    app
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn results_frame(cx: &mut TestAppContext) {
    // The demo tab shows 1,000 rows of 141 columns on its first page.
    let app = demo(cx);
    let samples = app.update(cx, |window, cx| sample(5, 40, || window.render_frame(cx)));
    report("ui.results.frame", median_ms(&samples), "ms", 50.);
}

fn scroll_frames(app: &TestApp, cx: &mut TestAppContext, delta: ScrollDelta) -> f64 {
    app.update(cx, |window, cx| {
        let column: gpui_kit::ElementId = ("cell", 1usize).into();
        let position = elements(window)
            .into_iter()
            .find(|element| element.visible() && element.path().last() == Some(&column))
            .expect("A visible result cell")
            .bounds()
            .center();
        let samples = sample(5, 40, || {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        median_ms(&samples)
    })
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn results_scroll(cx: &mut TestAppContext) {
    let app = demo(cx);
    let down = scroll_frames(&app, cx, ScrollDelta::Pixels(point(px(0.), px(-120.))));
    report("ui.results.scroll_vertical", down, "ms", 50.);
    let right = scroll_frames(&app, cx, ScrollDelta::Pixels(point(px(-240.), px(0.))));
    report("ui.results.scroll_horizontal", right, "ms", 50.);
}

/// A workspace with one tab of 1 MB of SQL.
fn one_megabyte_of_sql() -> Workspace {
    let line = "SELECT route, COUNT(*) FROM flights WHERE note = '日本語; safe' GROUP BY route;\n";
    let sql = line.repeat(1_000_000 / line.len() + 1);
    let profile = offline_profile("Synthetic");
    let tab = SavedTab {
        sql,
        ..SavedTab::new(1, Some(profile.id))
    };
    Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    }
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn editor_with_one_megabyte_of_sql(cx: &mut TestAppContext) {
    let workspace = one_megabyte_of_sql();
    let started = Instant::now();
    let app = TestApp::launch_with(cx, workspace, MemoryCredentials::default());
    report(
        "ui.editor.open_1mb",
        started.elapsed().as_secs_f64() * 1000.,
        "ms",
        1000.,
    );

    app.click(cx, "sql-editor");
    app.press(cx, "cmd-down");
    let samples = app.update(cx, |window, cx| sample(3, 20, || window.input("x", cx)));
    report("ui.editor.keystroke_1mb", median_ms(&samples), "ms", 50.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn assistant_transcript(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(Workspace::default());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    // Each reply streams about 16 KB in 800 deltas.
    let samples: Vec<_> = (1..=3)
        .map(|reply| {
            let message = format!("Stream a long reply {reply}");
            app.type_message(cx, &message);
            let started = Instant::now();
            app.click(cx, "assistant-send");
            app.wait_reply(cx, &format!("End of {message}"));
            started.elapsed()
        })
        .collect();
    report(
        "ui.assistant.stream_reply",
        median_ms(&samples),
        "ms",
        5000.,
    );
    app.wait_idle(cx);
    let samples = app.update(cx, |window, cx| sample(5, 40, || window.render_frame(cx)));
    report("ui.assistant.frame", median_ms(&samples), "ms", 50.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn assistant_composer_keystroke(cx: &mut TestAppContext) {
    // A tab with 1 MB of SQL and two long replies are on screen. Each sample
    // types one character into the message field and draws the next frame
    // like the window does: only the views that changed render again.
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(one_megabyte_of_sql());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    for reply in 1..=2 {
        let message = format!("Stream a long reply {reply}");
        app.send(cx, &message);
        app.wait_reply(cx, &format!("End of {message}"));
    }
    app.wait_idle(cx);
    app.click(cx, "assistant-composer");
    let samples = app.update(cx, |window, cx| {
        window.draw(cx).clear(cx);
        sample(5, 40, || {
            let mut key = Keystroke::parse("x").expect("x is a keystroke");
            key.key_char = Some("x".into());
            window.dispatch_keystroke(key, cx);
            window.draw(cx).clear(cx);
        })
    });
    report("ui.assistant.keystroke", median_ms(&samples), "ms", 50.);
}

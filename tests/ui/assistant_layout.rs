//! Geometry of the transcript at a scale and width where replies were cut
//! off. Pixel checks, like text that reaches the edge of a reply, stay in the
//! desktop suite.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, jump_to_latest};
use crate::support::{MemoryCredentials, TestApp, bounds_of, labelled_starting};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Pixels, ScrollDelta, TestAppContext, point, px};
use qrow::model::Workspace;
use qrow::ui::ToggleSidebar;

fn launch(cx: &mut TestAppContext) -> TestApp {
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    workspace.settings.ui_scale = 1.1;
    workspace.settings.assistant.panel_width = 536.;
    TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default())
}

fn entry(app: &TestApp, cx: &mut TestAppContext, prefix: &str) -> Bounds<Pixels> {
    app.update(cx, |window, _| {
        labelled_starting(window, prefix)
            .first()
            .unwrap_or_else(|| panic!("No entry {prefix}"))
            .bounds()
    })
}

fn composer(app: &TestApp, cx: &mut TestAppContext) -> Bounds<Pixels> {
    app.update(cx, |window, _| bounds_of(window, "assistant-composer"))
}

fn jump_visible(app: &TestApp, cx: &mut TestAppContext) -> bool {
    app.settle(cx);
    app.update(cx, |window, _| window.try_find(jump_to_latest()).is_some())
}

#[gpui_kit::test]
fn a_reply_starts_at_the_left_edge_of_the_composer(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "How many tables are in `sandbox_vbazhan` schema?");
    app.wait_reply(cx, "I can help with this query");
    app.wait_idle(cx);
    let reply = entry(&app, cx, "Assistant: **I can help with this query");
    let composer = composer(&app, cx);
    let offset = f32::from(reply.origin.x - composer.origin.x).abs();
    assert!(
        offset <= 1.,
        "The reply starts {offset}px from the composer edge"
    );
}

fn check_wide_table(app: &TestApp, cx: &mut TestAppContext) {
    if jump_visible(app, cx) {
        app.click(cx, jump_to_latest());
    }
    assert!(
        !jump_visible(app, cx),
        "The transcript did not scroll to the latest message"
    );
    let reply = entry(app, cx, "Assistant: Ran a synthetic wide table query");
    let composer = composer(app, cx);
    // At the end of the transcript, the whole table is above the composer.
    assert!(
        reply.bottom() <= composer.top(),
        "The transcript cut off the bottom of the table"
    );
    assert!(
        f32::from(reply.size.width) >= f32::from(composer.size.width) * 0.9,
        "The table reply did not use the transcript width"
    );
    // The wheel over the table scrolls the transcript in both directions.
    let over_table = |app: &TestApp, cx: &mut TestAppContext, pixels: f32| {
        app.update(cx, |window, cx| {
            window.scroll(
                "assistant-transcript",
                ScrollDelta::Pixels(point(px(0.), px(pixels))),
                cx,
            )
        });
    };
    over_table(app, cx, 300.);
    app.wait_until(cx, "the scroll up", REPLY_TIMEOUT, |window, _| {
        window.try_find(jump_to_latest()).is_some()
    });
    for _ in 0..3 {
        over_table(app, cx, -1200.);
    }
    app.wait_until(cx, "the scroll to the end", REPLY_TIMEOUT, |window, _| {
        window.try_find(jump_to_latest()).is_none()
    });
}

#[gpui_kit::test]
fn a_wide_table_reply_fits_the_transcript_and_scrolls(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    // Earlier messages make the transcript long enough to scroll.
    app.send(cx, "Show many lines");
    app.wait_reply(cx, "Line 40");
    app.wait_idle(cx);
    app.send(cx, "Show a wide table");
    app.wait_reply(cx, "Ran a synthetic wide table query");
    app.wait_idle(cx);
    check_wide_table(&app, cx);
    app.dispatch(cx, ToggleSidebar);
    app.wait_gone(cx, "add-connection");
    check_wide_table(&app, cx);
}

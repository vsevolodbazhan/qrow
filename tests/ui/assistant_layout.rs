//! Geometry of the transcript at a scale and width where replies were cut
//! off. Pixel checks, like text that reaches the edge of a reply, stay in the
//! desktop suite.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, composer_text, jump_to_latest};
use crate::support::{MemoryCredentials, TestApp, bounds_of, labelled_starting};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Pixels, ScrollDelta, TestAppContext, point, px, size};
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

#[gpui_kit::test]
fn status_bar_buttons_at_both_ends_have_the_same_gap(cx: &mut TestAppContext) {
    let app = launch(cx);
    let (left, right) = app.update(cx, |window, _| {
        let gap = |first: &str, second: &str| {
            bounds_of(window, second).left() - bounds_of(window, first).right()
        };
        (
            gap("show-connections", "show-sign-ins"),
            gap("toggle-assistant", "toggle-activity"),
        )
    });
    assert_eq!(left, right, "Left gap {left:?}, right gap {right:?}");
}

#[gpui_kit::test]
fn the_message_field_grows_with_its_text_up_to_a_limit(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    let empty = composer(&app, cx).size.height;

    // A message of three lines keeps the height of an empty field.
    app.type_message(cx, "1\n2\n3");
    app.settle(cx);
    assert_eq!(composer(&app, cx).size.height, empty);

    // Each line after the third adds one row.
    app.type_message(cx, "1\n2\n3\n4\n5\n6");
    app.settle(cx);
    let six = composer(&app, cx).size.height;
    let row = (six - empty) / 3.;
    assert!(
        row > px(10.),
        "Six lines grew the field by only {:?}",
        six - empty
    );

    // A long message stops at 40% of the window and scrolls inside the field.
    let long = (1..=80)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    app.type_message(cx, &long);
    app.settle(cx);
    let viewport = app.update(cx, |window, _| window.viewport_size().height);
    let tall = composer(&app, cx).size.height;
    let text = tall - (empty - row * 3.);
    assert!(
        text <= viewport * 0.4 && text > viewport * 0.4 - row,
        "The text area of the field is {text:?} high in a window {viewport:?} high"
    );

    // A taller window raises the limit at once, without an edit. The
    // field grows to the new limit and does not shrink to three rows.
    let window = app.update(cx, |window, _| window.viewport_size());
    cx.simulate_window_resize(app.window, size(window.width, window.height + row * 10.));
    app.settle(cx);
    let taller = composer(&app, cx).size.height;
    assert!(
        taller > tall,
        "The field is {taller:?} high after the window grew, before {tall:?}"
    );

    // After Send, the field returns to its first height.
    app.click(cx, "assistant-send");
    app.wait_until(cx, "the empty field", REPLY_TIMEOUT, |window, _| {
        bounds_of(window, "assistant-composer").size.height == empty
    });
}

#[gpui_kit::test]
fn a_message_keeps_its_line_breaks(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "select 1");
    app.wait_reply(cx, "You: select 1");
    app.wait_idle(cx);
    app.send(cx, "select *\nfrom bookings\nlimit 10");
    app.wait_reply(cx, "You: select *");
    app.wait_idle(cx);
    let one = entry(&app, cx, "You: select 1").size.height;
    let three = entry(&app, cx, "You: select *").size.height;
    // The message has three lines, as typed, and not one line of joined
    // words.
    assert!(
        three >= one * 2.5,
        "A message of three lines is {three:?} high, one line is {one:?}"
    );
}

#[gpui_kit::test]
fn maximizing_the_assistant_replaces_the_workspace_and_restores_its_width(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.type_message(cx, "Черновик запроса");
    let original = composer(&app, cx);
    app.click(cx, "assistant-maximize");
    app.wait_label(cx, "Minimize");
    app.wait_gone(cx, "assistant-splitter");
    app.wait_gone(cx, "add-connection");
    let maximized = composer(&app, cx);
    assert!(maximized.size.width > original.size.width);
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.wait_gone(cx, "assistant-composer");
    app.click(cx, "activity-close");
    app.wait_label(cx, "Minimize");
    assert_eq!(composer(&app, cx), maximized);
    app.click(cx, "assistant-maximize");
    app.wait_label(cx, "Maximize");
    app.wait_for(cx, "assistant-splitter");
    app.wait_for(cx, "add-connection");
    assert_eq!(composer(&app, cx), original);
    app.update(cx, |window, _| {
        assert_eq!(composer_text(window).as_deref(), Some("Черновик запроса"));
    });
    app.click(cx, "assistant-maximize");
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.wait_label(cx, "Minimize");
    assert_eq!(composer(&app, cx), maximized);
    app.press(cx, "alt-secondary-r");
    app.wait_label(cx, "Maximize");
    assert_eq!(composer(&app, cx), original);
}

#[gpui_kit::test]
fn assistant_shortcuts_remember_size_and_mode_with_motion(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    workspace.settings.smooth_transitions = true;
    workspace.settings.assistant.panel_width = 573.;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    cx.update(|cx| cx.set_reduce_motion(false));
    app.dispatch(cx, ToggleSidebar);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.type_message(cx, "Черновик запроса");
    app.wait_until(cx, "settled assistant", REPLY_TIMEOUT, |window, _| {
        bounds_of(window, "assistant-panel").size.width == px(573.)
    });
    let original = composer(&app, cx);
    for _ in 0..2 {
        app.press(cx, "secondary-r");
        app.wait_gone(cx, "assistant-composer");
        app.press(cx, "secondary-r");
        app.wait_until(cx, "restored side pane", REPLY_TIMEOUT, |window, _| {
            bounds_of(window, "assistant-composer") == original
        });
    }
    app.press(cx, "secondary-shift-r");
    app.press(cx, "secondary-shift-r");
    app.wait_until(cx, "maximized assistant", REPLY_TIMEOUT, |window, _| {
        bounds_of(window, "assistant-panel").size.width == window.viewport_size().width
    });
    for _ in 0..2 {
        app.press(cx, "secondary-r");
        app.wait_gone(cx, "assistant-composer");
        app.press(cx, "secondary-r");
        app.wait_until(cx, "restored maximized pane", REPLY_TIMEOUT, |window, _| {
            bounds_of(window, "assistant-panel").size.width == window.viewport_size().width
        });
        app.wait_label(cx, "Minimize");
    }
    app.press(cx, "alt-secondary-r");
    app.press(cx, "alt-secondary-r");
    app.wait_until(
        cx,
        "original side-pane width",
        REPLY_TIMEOUT,
        |window, _| bounds_of(window, "assistant-composer") == original,
    );
    app.update(cx, |window, _| {
        assert_eq!(composer_text(window).as_deref(), Some("Черновик запроса"))
    });
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.press(cx, "secondary-shift-r");
    app.press(cx, "alt-secondary-r");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.press(cx, "secondary-r");
    app.wait_gone(cx, "assistant-composer");
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.press(cx, "secondary-shift-r");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.wait_label(cx, "Minimize");
    app.press(cx, "secondary-r");
    app.wait_gone(cx, "assistant-composer");
    app.update(cx, |window, cx| window.input("SELECT 73", cx));
    app.wait_until(cx, "editor focus after closing", REPLY_TIMEOUT, |_, _| {
        app.saved().tabs[0].sql == "SELECT 73"
    });
    app.press(cx, "alt-secondary-r");
    app.wait_gone(cx, "assistant-composer");
    app.press(cx, "secondary-shift-r");
    app.wait_label(cx, "Minimize");
}

#[gpui_kit::test]
fn maximized_assistant_follows_the_window_width(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.click(cx, "assistant-maximize");
    for width in [900., 1280.] {
        cx.simulate_window_resize(app.window, size(px(width), px(820.)));
        app.settle(cx);
        app.update(cx, |window, _| {
            let panel = bounds_of(window, "assistant-panel");
            assert_eq!(panel.left(), px(0.));
            assert_eq!(panel.size.width, window.viewport_size().width);
        });
    }
}

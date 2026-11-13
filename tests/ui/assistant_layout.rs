//! Geometry of the transcript at a scale and width where replies were cut
//! off. Pixel checks, like text that reaches the edge of a reply, stay in the
//! desktop suite.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT, composer_text, jump_to_latest};
use crate::support::{MemoryCredentials, TestApp, bounds_of, labelled_starting, offline_profile};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Pixels, ScrollDelta, TestAppContext, point, px, size};
use qrow::model::{AssistantConversation, SavedTab, Workspace};
use qrow::ui::{FormatSql, RunQuery, ToggleSidebar};

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
    app.wait_label(cx, "Minimize assistant");
    app.wait_gone(cx, "assistant-splitter");
    app.wait_gone(cx, "add-connection");
    let maximized = composer(&app, cx);
    assert!(maximized.size.width > original.size.width);
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.wait_gone(cx, "assistant-composer");
    app.click(cx, "activity-close");
    app.wait_label(cx, "Minimize assistant");
    assert_eq!(composer(&app, cx), maximized);
    app.click(cx, "assistant-maximize");
    app.wait_label(cx, "Maximize assistant");
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
    app.wait_label(cx, "Minimize assistant");
    assert_eq!(composer(&app, cx), maximized);
    app.press(cx, "secondary-shift-r");
    app.wait_label(cx, "Maximize assistant");
    assert_eq!(composer(&app, cx), original);
}

#[gpui_kit::test]
fn assistant_shortcuts_remember_size_and_mode_with_motion(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    workspace.settings.smooth_transitions = true;
    workspace.settings.assistant.panel_width = 573.;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    cx.update(|cx| cx.set_reduce_motion(true));
    app.dispatch(cx, ToggleSidebar);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.type_message(cx, "Черновик запроса");
    // Start an opening transition without depending on harness startup speed.
    app.press(cx, "secondary-r");
    app.wait_gone(cx, "assistant-composer");
    cx.update(|cx| cx.set_reduce_motion(false));
    app.press(cx, "secondary-r");
    app.update(cx, |window, _| {
        // Inner content has its full width even while the reveal is clipped.
        assert_eq!(bounds_of(window, "assistant-panel").size.width, px(573.));
        assert!(bounds_of(window, "assistant-motion").size.width < px(573.));
    });
    app.wait_until(cx, "settled assistant", REPLY_TIMEOUT, |window, _| {
        bounds_of(window, "assistant-motion").size.width == px(573.)
            && bounds_of(window, "assistant-panel").right() == window.viewport_size().width
    });
    let original_panel = app.update(cx, |window, _| bounds_of(window, "assistant-panel"));
    let original = composer(&app, cx);
    // Keep exact geometry checks and report distinct bounds on failure.
    let last_bounds = std::cell::Cell::new(None);
    let restored = |window: &mut gpui_kit::Window, _: &mut gpui_kit::App| {
        let composer = bounds_of(window, "assistant-composer");
        let panel = bounds_of(window, "assistant-panel");
        if last_bounds.replace(Some((panel, composer))) != Some((panel, composer)) {
            eprintln!(
                "Assistant restoration: expected panel {original_panel:?}, composer {original:?}; actual panel {panel:?}, composer {composer:?}"
            );
        }
        panel == original_panel && composer == original
    };
    for _ in 0..2 {
        app.press(cx, "secondary-r");
        app.wait_gone(cx, "assistant-composer");
        app.press(cx, "secondary-r");
        app.wait_until(cx, "restored side pane", REPLY_TIMEOUT, &restored);
    }
    app.press(cx, "secondary-shift-r");
    app.wait_until(cx, "maximized assistant", REPLY_TIMEOUT, |window, _| {
        bounds_of(window, "assistant-motion").size.width == window.viewport_size().width
    });
    for _ in 0..2 {
        app.press(cx, "secondary-r");
        app.wait_gone(cx, "assistant-composer");
        app.press(cx, "secondary-r");
        app.wait_until(cx, "restored maximized pane", REPLY_TIMEOUT, |window, _| {
            bounds_of(window, "assistant-motion").size.width == window.viewport_size().width
        });
        app.wait_label(cx, "Minimize assistant");
    }
    app.press(cx, "secondary-shift-r");
    app.wait_until(cx, "original side-pane width", REPLY_TIMEOUT, &restored);
    assert_eq!(app.saved().settings.assistant.panel_width, 573.);
    app.update(cx, |window, _| {
        assert_eq!(composer_text(window).as_deref(), Some("Черновик запроса"))
    });
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.press(cx, "secondary-shift-r");
    app.press(cx, "secondary-shift-r");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.press(cx, "secondary-r");
    app.wait_gone(cx, "assistant-composer");
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    app.press(cx, "secondary-shift-r");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    app.wait_label(cx, "Minimize assistant");
    app.press(cx, "secondary-r");
    app.wait_gone(cx, "assistant-composer");
    app.update(cx, |window, cx| window.input("SELECT 73", cx));
    app.wait_until(cx, "editor focus after closing", REPLY_TIMEOUT, |_, _| {
        app.saved().tabs[0].sql == "SELECT 73"
    });
    app.dispatch(cx, qrow::ui::MinimizeAssistant);
    app.wait_gone(cx, "assistant-composer");
    app.press(cx, "secondary-shift-r");
    app.wait_label(cx, "Minimize assistant");
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

/// A workspace with one Alpha tab and two conversations whose tabs closed.
fn launch_with_conversations(cx: &mut TestAppContext) -> TestApp {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Alpha");
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT 1".into();
    let mut workspace = codex.workspace(Workspace {
        profiles: vec![profile.clone()],
        tabs: vec![tab],
        ..Workspace::default()
    });
    workspace.settings.assistant.panel_width = 536.;
    for (thread, title, last_activity) in [
        ("synthetic-recent", "Recent conversation", 200),
        ("synthetic-older", "Older conversation", 100),
    ] {
        let mut conversation = AssistantConversation::new(thread, Default::default());
        conversation.title = title.into();
        conversation.last_activity = last_activity;
        conversation.detached_profile = Some(profile.id);
        workspace.assistant.conversations.push(conversation);
    }
    TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default())
}

fn maximize(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "assistant-maximize");
    app.wait_label(cx, "Minimize assistant");
    app.wait_gone(cx, "add-connection");
}

#[gpui_kit::test]
fn workspace_shortcuts_act_on_what_the_maximized_assistant_shows(cx: &mut TestAppContext) {
    let app = launch_with_conversations(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    maximize(&app, cx);
    app.wait_for(cx, "assistant-thread-list");
    let status = app.update(cx, |_, cx| app.status(cx));

    // The New Conversation tooltip names ⌘T, which now starts a conversation.
    app.hover(cx, "assistant-new");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(
            crate::support::label(window, "status-tooltip-title").as_deref(),
            Some("New conversation")
        );
        assert_eq!(
            crate::support::label(window, "status-tooltip-shortcut").as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⌘T"
            } else {
                "Ctrl+T"
            })
        );
        crate::support::assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
    });

    // The New Conversation tooltip names ⌘T, which now starts a conversation.
    app.hover(cx, "assistant-new");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, cx| {
        assert_eq!(
            crate::support::label(window, "status-tooltip-title").as_deref(),
            Some("New conversation")
        );
        assert_eq!(
            crate::support::label(window, "status-tooltip-shortcut").as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⌘T"
            } else {
                "Ctrl+T"
            })
        );
        crate::support::assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
    });

    // ⌘1 to ⌘9 select a conversation by its position in the list.
    app.press(cx, "secondary-2");
    app.wait_label(cx, "Conversation title: Older conversation");
    app.press(cx, "secondary-1");
    app.wait_label(cx, "Conversation title: Recent conversation");
    app.press(cx, "secondary-9");
    app.wait_label(cx, "Conversation title: Recent conversation");

    // The hidden editor does not run or format its SQL.
    app.dispatch(cx, RunQuery);
    app.dispatch(cx, FormatSql);
    app.settle(cx);
    app.update(cx, |_, cx| assert_eq!(app.status(cx), status));
    assert_eq!(app.saved().tabs[0].sql, "SELECT 1");

    // ⌘T starts a conversation in a new tab, and the pane stays maximized.
    app.press(cx, "secondary-t");
    app.wait_label(cx, "Conversation title: New conversation");
    app.wait_saved(cx, "the conversation tab", |saved| saved.tabs.len() == 2);
    app.wait_label(cx, "Minimize assistant");
    // The message field of the new conversation has the focus.
    app.update(cx, |window, cx| window.input("Новый вопрос", cx));
    app.wait_until(
        cx,
        "typing in the new conversation",
        REPLY_TIMEOUT,
        |window, _| composer_text(window).as_deref() == Some("Новый вопрос"),
    );

    // ⌘W returns to the side pane and does not close the hidden tab.
    app.press(cx, "secondary-w");
    app.wait_label(cx, "Maximize assistant");
    app.wait_for(cx, "add-connection");
    app.settle(cx);
    assert_eq!(app.saved().tabs.len(), 2);

    // ⌘J shows collapsed results instead of toggling them out of view.
    app.press(cx, "secondary-j");
    app.wait_saved(cx, "collapsed results", |saved| {
        saved.settings.results_minimized
    });
    maximize(&app, cx);
    app.press(cx, "secondary-j");
    app.wait_label(cx, "Maximize assistant");
    app.wait_saved(cx, "expanded results", |saved| {
        !saved.settings.results_minimized
    });

    // ⌘B shows the Connections sidebar, which the maximized pane hid.
    maximize(&app, cx);
    app.press(cx, "secondary-b");
    app.wait_label(cx, "Maximize assistant");
    app.wait_for(cx, "add-connection");
}

#[gpui_kit::test]
fn assistant_size_tooltips_show_the_shortcut_in_both_states(cx: &mut TestAppContext) {
    let app = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    for title in [
        "Maximize assistant",
        "Minimize assistant",
        "Maximize assistant",
    ] {
        app.hover(cx, "assistant-maximize");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, cx| {
            assert_eq!(
                crate::support::label(window, "status-tooltip-title").as_deref(),
                Some(title)
            );
            assert_eq!(
                crate::support::label(window, "status-tooltip-shortcut").as_deref(),
                Some(if cfg!(target_os = "macos") {
                    "⇧⌘R"
                } else {
                    "Ctrl+Shift+R"
                })
            );
            crate::support::assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
        });
        app.press(cx, "secondary-shift-r");
        app.wait_gone(cx, "status-tooltip-shortcut");
    }
}

#[gpui_kit::test]
fn a_conversation_selected_by_number_scrolls_into_view(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let profile = offline_profile("Alpha");
    let mut workspace = codex.workspace(Workspace {
        profiles: vec![profile.clone()],
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        ..Workspace::default()
    });
    for number in 1..=12u64 {
        let mut conversation =
            AssistantConversation::new(format!("synthetic-{number}"), Default::default());
        conversation.title = format!("Разговор {number}");
        conversation.last_activity = 1_000 - number;
        conversation.detached_profile = Some(profile.id);
        workspace.assistant.conversations.push(conversation);
    }
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    cx.simulate_window_resize(app.window, size(px(1280.), px(480.)));
    app.open_assistant(cx);
    app.show_conversation(cx);
    maximize(&app, cx);
    app.wait_for(cx, "assistant-thread-list");
    let visible = |app: &TestApp, cx: &mut TestAppContext, row: &str| {
        app.update(cx, |window, _| {
            let list = bounds_of(window, "assistant-thread-list");
            let row = bounds_of(window, row);
            row.top() >= list.top() && row.bottom() <= list.bottom()
        })
    };
    // Scroll to the end of the list, so the first conversation is out of view.
    app.update(cx, |window, cx| {
        window.scroll(
            "assistant-thread-list",
            ScrollDelta::Pixels(point(px(0.), px(-2_000.))),
            cx,
        )
    });
    app.settle(cx);
    assert!(!visible(&app, cx, "Разговор 1, Alpha, Tab Closed"));

    app.press(cx, "secondary-1");
    app.wait_label(cx, "Conversation title: Разговор 1");
    app.settle(cx);
    assert!(visible(&app, cx, "Разговор 1, Alpha, Tab Closed"));
}

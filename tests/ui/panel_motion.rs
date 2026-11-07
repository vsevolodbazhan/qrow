//! Motion preserves focus, input, and pane geometry through interrupted transitions.
use crate::support::{TestApp, bounds_of, labelled, offline_profile};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, point, px};
use qrow::model::{SavedTab, Workspace};
use qrow::ui::{ShowConnections, ShowSignIns, ToggleSidebar};
use std::time::Duration;

fn advance(app: &TestApp, cx: &mut TestAppContext, millis: u64) {
    cx.executor().advance_clock(Duration::from_millis(millis));
    app.update(cx, |window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
}

#[gpui_kit::test]
fn sidebar_motion_preserves_input_and_direct_resize(cx: &mut TestAppContext) {
    let profile = offline_profile("Motion");
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = "SELECT 1".into();
    let mut workspace = Workspace {
        tabs: vec![tab],
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.settings.smooth_transitions = true;
    let app = TestApp::launch(cx, workspace);
    let full = app.update(cx, |window, _| {
        bounds_of(window, "sidebar-motion").size.width
    });
    cx.update(|cx| cx.set_reduce_motion(false));
    app.dispatch(cx, ShowSignIns);
    app.wait_for(cx, "add-sign-in");
    app.dispatch(cx, ShowConnections);
    app.wait_for(cx, "add-connection");
    assert_eq!(
        app.update(cx, |window, _| bounds_of(window, "sidebar-motion")
            .size
            .width),
        full
    );
    let search_y = app.update(cx, |window, _| {
        bounds_of(window, "Search connections and relations")
            .center()
            .y
            - bounds_of(window, "sidebar-motion").top()
    });
    app.dispatch(cx, ToggleSidebar);
    advance(&app, cx, 20);
    let closing = app.update(cx, |window, _| {
        bounds_of(window, "sidebar-motion").size.width
    });
    assert!(closing > px(0.) && closing < full);
    // A click over the exiting search field must not take focus from the editor.
    app.update(cx, |window, cx| {
        assert!(labelled(window, "Search connections and relations").is_none());
        assert!(labelled(window, "New connection").is_none());
        for _ in 0..12 {
            window.focus_next(cx);
            window.render_frame(cx);
            assert!(labelled(window, "Search connections and relations").is_none());
        }
        for _ in 0..12 {
            window.focus_prev(cx);
            window.render_frame(cx);
            assert!(labelled(window, "Search connections and relations").is_none());
        }
        window.click_at("sidebar-motion", point(px(20.), search_y), cx);
        window.press("a", cx);
        assert!(
            labelled(window, "SQL Editor")
                .and_then(|element| element.value().map(str::to_owned))
                .unwrap()
                .contains('a')
        );
    });
    app.dispatch(cx, ToggleSidebar);
    let reversed = app.update(cx, |window, _| {
        bounds_of(window, "sidebar-motion").size.width
    });
    assert!((closing - reversed).abs() < px(0.5));
    advance(&app, cx, 250);
    assert_eq!(
        app.update(cx, |window, _| bounds_of(window, "sidebar-motion")
            .size
            .width),
        full
    );
    app.update(cx, |window, cx| {
        let start = bounds_of(window, "sidebar-splitter").center();
        window.drag(start, start + point(px(30.), px(0.)), cx);
        assert!((bounds_of(window, "sidebar-motion").size.width - full - px(30.)).abs() <= px(1.));
    });
    app.dispatch(cx, ToggleSidebar);
    advance(&app, cx, 250);
    app.wait_gone(cx, "sidebar-motion");
    // The wider tab strip gets one final frame to update its scroll geometry.
    advance(&app, cx, 0);
    app.update(cx, |window, cx| {
        assert_eq!(window.simulate_next_frame(cx), 0)
    });
}

#[gpui_kit::test]
fn editor_motion_keeps_the_new_document_and_typing_immediate(cx: &mut TestAppContext) {
    let profile = offline_profile("Editor motion");
    let mut first = SavedTab::new(1, Some(profile.id));
    first.sql = "SELECT 1".into();
    let mut second = SavedTab::new(2, Some(profile.id));
    second.sql = "SELECT 2".into();
    let mut workspace = Workspace {
        tabs: vec![first, second],
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.settings.smooth_transitions = true;
    let app = TestApp::launch(cx, workspace);
    cx.update(|cx| cx.set_reduce_motion(false));
    let editor_bounds = app.update(cx, |window, _| bounds_of(window, "SQL Editor"));
    app.click_labelled(cx, "Query 2");
    app.update(cx, |window, cx| {
        assert_eq!(bounds_of(window, "SQL Editor"), editor_bounds);
        assert_eq!(
            labelled(window, "SQL Editor")
                .and_then(|element| element.value().map(str::to_owned))
                .as_deref(),
            Some("SELECT 2")
        );
        window.input("x", cx);
        assert!(
            labelled(window, "SQL Editor")
                .and_then(|element| element.value().map(str::to_owned))
                .unwrap()
                .contains('x')
        );
    });
    app.click_labelled(cx, "Query 1");
    for millis in [0, 40, 60, 100] {
        advance(&app, cx, millis);
        app.update(cx, |window, _| {
            assert_eq!(bounds_of(window, "SQL Editor"), editor_bounds);
        });
    }
    app.update(cx, |window, _| {
        assert_eq!(
            labelled(window, "SQL Editor")
                .and_then(|element| element.value().map(str::to_owned))
                .as_deref(),
            Some("SELECT 1")
        )
    });
    app.click_labelled(cx, "Query 2");
    app.update(cx, |window, _| {
        assert!(
            labelled(window, "SQL Editor")
                .and_then(|element| element.value().map(str::to_owned))
                .unwrap()
                .contains('x')
        )
    });
}

#[gpui_kit::test]
fn assistant_sidebar_reverses_from_its_current_width(cx: &mut TestAppContext) {
    use crate::support::{MemoryCredentials, assistant::FakeCodex};
    use qrow::ui::ToggleAssistant;
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    workspace.settings.smooth_transitions = true;
    workspace.settings.assistant.panel_width = 360.;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    let full = app.update(cx, |window, _| {
        bounds_of(window, "assistant-motion").size.width
    });
    cx.update(|cx| cx.set_reduce_motion(false));
    app.dispatch(cx, ToggleAssistant);
    advance(&app, cx, 40);
    let closing = app.update(cx, |window, _| {
        bounds_of(window, "assistant-motion").size.width
    });
    assert!(closing > px(0.) && closing < full);
    app.update(cx, |window, _| {
        assert!(labelled(window, "Assistant Message").is_none());
    });
    app.dispatch(cx, ToggleAssistant);
    let reversed = app.update(cx, |window, _| {
        bounds_of(window, "assistant-motion").size.width
    });
    assert!((closing - reversed).abs() < px(0.5));
    advance(&app, cx, 250);
    assert!(
        (app.update(cx, |window, _| bounds_of(window, "assistant-motion")
            .size
            .width)
            - full)
            .abs()
            < px(0.5)
    );
    app.update(cx, |window, cx| {
        let start = bounds_of(window, "assistant-splitter").center();
        window.drag(start, start - point(px(30.), px(0.)), cx);
        assert!(
            (bounds_of(window, "assistant-motion").size.width - full - px(30.)).abs() <= px(1.)
        );
    });
    app.dispatch(cx, ToggleAssistant);
    advance(&app, cx, 250);
    app.wait_gone(cx, "assistant-motion");
}

#[gpui_kit::test]
fn activity_motion_reveals_reverses_and_removes_closed_controls(cx: &mut TestAppContext) {
    use qrow::ui::ToggleActivity;
    let mut workspace = Workspace::default();
    workspace.settings.smooth_transitions = true;
    let app = TestApp::launch(cx, workspace);
    cx.update(|cx| cx.set_reduce_motion(false));
    app.dispatch(cx, ToggleActivity);
    advance(&app, cx, 40);
    let (opening, full) = app.update(cx, |window, _| {
        (
            bounds_of(window, "activity-motion").size.width,
            bounds_of(window, "activity").size.width,
        )
    });
    assert!(opening > px(0.) && opening < full);
    app.update(cx, |window, _| {
        let panel = bounds_of(window, "activity-motion");
        let body = bounds_of(window, "workspace-body");
        assert_eq!(
            panel.right(),
            body.right(),
            "Activity enters from the right"
        );
        assert_eq!(panel.top(), body.top());
        assert_eq!(panel.size.height, body.size.height);
    });
    app.dispatch(cx, ToggleActivity);
    app.update(cx, |window, _| {
        assert!((bounds_of(window, "activity-motion").size.width - opening).abs() < px(0.5));
        assert!(labelled(window, "Close activity").is_none());
        assert!(labelled(window, "SQL Editor").is_some());
    });
    advance(&app, cx, 20);
    let closing = app.update(cx, |window, _| {
        bounds_of(window, "activity-motion").size.width
    });
    assert!(closing < opening);
    app.dispatch(cx, ToggleActivity);
    app.update(cx, |window, _| {
        assert!((bounds_of(window, "activity-motion").size.width - closing).abs() < px(0.5))
    });
    advance(&app, cx, 250);
    app.update(cx, |window, _| {
        assert_eq!(bounds_of(window, "activity-motion").size.width, full)
    });
    app.click_labelled(cx, "Close activity");
    advance(&app, cx, 250);
    app.wait_gone(cx, "activity-motion");
    advance(&app, cx, 0);
    app.update(cx, |window, cx| {
        assert_eq!(window.simulate_next_frame(cx), 0)
    });
    cx.update(|cx| cx.set_reduce_motion(true));
    app.dispatch(cx, ToggleActivity);
    app.update(cx, |window, _| {
        assert_eq!(bounds_of(window, "activity-motion").size.width, full)
    });
    app.dispatch(cx, ToggleActivity);
    app.wait_gone(cx, "activity-motion");
}

#[gpui_kit::test]
fn activity_reveal_fits_the_body_above_a_workspace_warning(cx: &mut TestAppContext) {
    use gpui_kit::size;
    use qrow::ui::ToggleActivity;
    for enabled in [false, true] {
        let mut workspace = Workspace::default();
        workspace.settings.smooth_transitions = enabled;
        workspace.settings.theme = "Synthetic unavailable theme".into();
        workspace.settings.editor_font_family = "Synthetic unavailable editor font".into();
        workspace.settings.logs_font_family = "Synthetic unavailable logs font".into();
        let app = TestApp::launch(cx, workspace);
        app.wait_for(cx, "workspace-message");
        app.dispatch(cx, ToggleActivity);
        for (width, height) in [(1280., 820.), (900., 600.)] {
            cx.simulate_window_resize(app.window, size(px(width), px(height)));
            app.settle(cx);
            advance(&app, cx, 0);
            app.update(cx, |window, _| {
                let body = bounds_of(window, "workspace-body");
                let activity = bounds_of(window, "activity");
                assert_eq!(activity, body);
                let close = bounds_of(window, "Close activity");
                assert!(close.top() >= body.top() && close.bottom() <= body.bottom());
            });
        }
        app.click_labelled(cx, "Close activity");
        app.wait_gone(cx, "activity-motion");
    }
}

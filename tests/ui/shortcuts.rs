//! Shortcuts from the saved settings, and the commands that have no
//! default shortcut.
use crate::support::{TestApp, offline_profile};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use qrow::model::{SavedTab, Workspace};
use std::time::Duration;
use uuid::Uuid;

fn workspace(titles: &[&str], shortcuts: &[(&str, Option<&str>)]) -> (Workspace, Uuid) {
    let profile = offline_profile("Shortcuts");
    let id = profile.id;
    let tabs = titles
        .iter()
        .enumerate()
        .map(|(index, title)| SavedTab {
            title: (*title).into(),
            ..SavedTab::new(index + 1, Some(id))
        })
        .collect();
    let mut workspace = Workspace {
        tabs,
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.settings.shortcuts = shortcuts
        .iter()
        .map(|(id, keys)| ((*id).to_owned(), keys.map(str::to_owned)))
        .collect();
    (workspace, id)
}

fn wait_minimized(app: &TestApp, cx: &mut TestAppContext, minimized: bool) {
    app.wait_until(
        cx,
        &format!("results minimized: {minimized}"),
        Duration::from_secs(5),
        |_, _| app.saved().settings.results_minimized == minimized,
    );
}

#[gpui_kit::test]
fn a_saved_shortcut_replaces_the_default_and_a_removed_one_does_nothing(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(
        &["Query 1"],
        &[
            ("toggle_results", Some("secondary-alt-j")),
            ("new_tab", None),
        ],
    );
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "secondary-j");
    app.settle(cx);
    assert!(
        !app.saved().settings.results_minimized,
        "⌘J has no command now"
    );
    app.press(cx, "secondary-alt-j");
    wait_minimized(&app, cx, true);
    app.press(cx, "secondary-t");
    app.settle(cx);
    assert_eq!(app.saved().tabs.len(), 1, "New tab has no shortcut");
}

#[gpui_kit::test]
fn next_and_previous_tab_wrap_at_the_ends_of_the_strip(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(
        &["Query 1", "Query 2", "Query 3"],
        &[
            ("next_tab", Some("secondary-alt-right")),
            ("previous_tab", Some("secondary-alt-left")),
        ],
    );
    let app = TestApp::launch(cx, workspace);
    let active = |app: &TestApp| app.saved().active_tab;
    let start = active(&app);
    let wait_active = |app: &TestApp, cx: &mut TestAppContext, expected: usize| {
        app.wait_until(
            cx,
            &format!("tab {expected}"),
            Duration::from_secs(5),
            |_, _| app.saved().active_tab == expected,
        );
    };
    app.press(cx, "secondary-alt-right");
    wait_active(&app, cx, (start + 1) % 3);
    app.press(cx, "secondary-alt-left");
    wait_active(&app, cx, start);
    app.press(cx, "secondary-alt-left");
    wait_active(&app, cx, (start + 2) % 3);
}

#[gpui_kit::test]
fn rename_tab_and_focus_editor_have_shortcuts_when_assigned(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(
        &["Query 1"],
        &[
            ("rename_tab", Some("f2")),
            ("focus_editor", Some("secondary-alt-e")),
        ],
    );
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "f2");
    app.wait_for(cx, "rename-tab-name");
    app.press(cx, "escape");
    app.wait_gone(cx, "rename-tab-name");

    // Move the focus away from the editor, then bring it back.
    app.click(cx, "show-connections");
    app.press(cx, "secondary-alt-e");
    app.update(cx, |window, cx| window.input("SELECT 42", cx));
    app.wait_until(cx, "typed SQL", Duration::from_secs(5), |_, _| {
        app.saved().tabs[0].sql.contains("SELECT 42")
    });
}

#[gpui_kit::test]
fn a_custom_scale_shortcut_replaces_the_default(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("increase_scale", Some("secondary-alt-="))]);
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "secondary-=");
    app.settle(cx);
    assert_eq!(app.saved().settings.ui_scale, 1., "⌘= has no command now");
    app.press(cx, "secondary-alt-=");
    app.wait_until(cx, "larger scale", Duration::from_secs(5), |_, _| {
        app.saved().settings.ui_scale > 1.
    });
}

/// Opens Settings at the Shortcuts page.
fn open_shortcuts(app: &TestApp, cx: &mut TestAppContext) {
    app.dispatch(cx, qrow::ui::OpenSettings);
    app.wait_for(cx, "settings-content");
    app.settings_page(cx, "Shortcuts");
    app.wait_for(cx, "shortcut-run_query");
}

fn saved_shortcut(app: &TestApp, id: &str) -> Option<Option<String>> {
    app.saved().settings.shortcuts.get(id).cloned()
}

fn message(app: &TestApp, cx: &mut TestAppContext, id: &str) -> Option<String> {
    let id = gpui_kit::SharedString::from(format!("shortcut-message-{id}"));
    app.update(cx, |window, _| crate::support::label(window, id.clone()))
}

#[gpui_kit::test]
fn a_recorded_shortcut_is_saved_and_runs_its_command(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "shortcut-toggle_results");
    app.press(cx, "secondary-alt-k");
    app.wait_until(cx, "the new shortcut", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "toggle_results") == Some(Some("secondary-alt-k".into()))
    });
    // ⌘W closes Settings again once the field stops recording.
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "settings-content");
    app.press(cx, "secondary-alt-k");
    wait_minimized(&app, cx, true);
    app.press(cx, "secondary-j");
    app.settle(cx);
    assert!(
        app.saved().settings.results_minimized,
        "⌘J no longer toggles results"
    );
}

#[gpui_kit::test]
fn a_conflict_keeps_recording_until_replace(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "shortcut-cancel_query");
    app.press(cx, "secondary-j");
    app.wait_for(cx, "replace-shortcut-cancel_query");
    assert_eq!(
        message(&app, cx, "cancel_query").as_deref(),
        Some("Used by Toggle Results.")
    );
    assert_eq!(
        saved_shortcut(&app, "cancel_query"),
        None,
        "nothing changes before Replace"
    );
    app.click(cx, "replace-shortcut-cancel_query");
    app.wait_until(cx, "the moved shortcut", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "cancel_query") == Some(Some("secondary-j".into()))
            && saved_shortcut(&app, "toggle_results") == Some(None)
    });
    assert_eq!(
        message(&app, cx, "cancel_query").as_deref(),
        Some("Toggle Results has no shortcut now.")
    );
}

#[gpui_kit::test]
fn escape_cancels_recording_and_delete_removes_the_shortcut(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "shortcut-toggle_results");
    app.wait_for(cx, "cancel-shortcut-toggle_results");
    app.press(cx, "escape");
    app.wait_gone(cx, "cancel-shortcut-toggle_results");
    app.wait_for(cx, "settings-content");
    assert_eq!(saved_shortcut(&app, "toggle_results"), None);

    // While a field records, ⌘W is a shortcut to record, not Close.
    app.click(cx, "shortcut-transpose_result");
    app.press(cx, "secondary-w");
    app.wait_for(cx, "replace-shortcut-transpose_result");
    assert_eq!(
        message(&app, cx, "transpose_result").as_deref(),
        Some("Used by Close Tab.")
    );
    app.press(cx, "escape");
    app.wait_gone(cx, "replace-shortcut-transpose_result");

    app.click(cx, "shortcut-toggle_results");
    app.press(cx, "backspace");
    app.wait_until(cx, "no shortcut", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "toggle_results") == Some(None)
    });
}

#[gpui_kit::test]
fn a_letter_without_a_modifier_is_not_a_shortcut(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "shortcut-new_conversation");
    app.press(cx, "k");
    app.wait_for(cx, "shortcut-message-new_conversation");
    let text = message(&app, cx, "new_conversation").unwrap_or_default();
    assert!(text.starts_with("Add "), "unexpected message {text:?}");
    assert_eq!(saved_shortcut(&app, "new_conversation"), None);
    // A function key needs no modifier.
    app.press(cx, "f6");
    app.wait_until(cx, "F6", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "new_conversation") == Some(Some("f6".into()))
    });
}

#[gpui_kit::test]
fn filters_and_find_by_keys_narrow_the_list(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("new_tab", Some("secondary-alt-n"))]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    let shown = |app: &TestApp, cx: &mut TestAppContext, id: &str| {
        let id = gpui_kit::SharedString::from(format!("shortcut-{id}"));
        app.update(cx, |window, _| window.try_find(id.clone()).is_some())
    };
    app.click(cx, "shortcut-filter-Customized");
    app.wait_gone(cx, "shortcut-run_query");
    assert!(shown(&app, cx, "new_tab"));
    app.click(cx, "shortcut-filter-No shortcut");
    app.wait_for(cx, "shortcut-cancel_query");
    assert!(!shown(&app, cx, "new_tab"));
    app.click(cx, "shortcut-filter-All");
    app.click(cx, "find-by-keys");
    app.press(cx, "secondary-enter");
    app.wait_for(cx, "clear-found-keys");
    for id in ["run_query", "send_message", "confirm_dialog"] {
        assert!(shown(&app, cx, id), "{id} uses ⌘↩");
    }
    assert!(!shown(&app, cx, "new_tab"));
    app.click(cx, "clear-found-keys");
    app.wait_for(cx, "shortcut-new_tab");
}

#[gpui_kit::test]
fn remove_reset_and_reset_all_return_shortcuts(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(
        &["Query 1"],
        &[("new_tab", Some("secondary-alt-n")), ("quit", None)],
    );
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.hover(cx, "shortcut-close_tab");
    app.click(cx, "remove-shortcut-close_tab");
    app.wait_until(cx, "Close Tab removed", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "close_tab") == Some(None)
    });
    app.click(cx, "reset-shortcut-close_tab");
    app.wait_until(cx, "Close Tab reset", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "close_tab").is_none()
    });
    app.reset_settings_page(cx, "Reset shortcuts");
    app.wait_until(cx, "default shortcuts", Duration::from_secs(5), |_, _| {
        app.saved().settings.shortcuts.is_empty()
    });
}

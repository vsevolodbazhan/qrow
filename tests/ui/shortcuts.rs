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
    // Past the autosave delay, so a change would be in the saved workspace.
    app.pass_time(cx, Duration::from_secs(1));
    assert!(
        !app.saved().settings.results_minimized,
        "⌘J has no command now"
    );
    app.press(cx, "secondary-alt-j");
    wait_minimized(&app, cx, true);
    app.press(cx, "secondary-t");
    // Past the autosave delay, so a change would be in the saved workspace.
    app.pass_time(cx, Duration::from_secs(1));
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
fn rename_tab_and_focus_commands_have_shortcuts_when_assigned(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(
        &["Query 1"],
        &[
            ("rename_tab", Some("f2")),
            ("focus_editor", Some("secondary-alt-e")),
            ("focus_catalog", Some("secondary-alt-c")),
        ],
    );
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "f2");
    app.wait_for(cx, "rename-tab-name");
    app.press(cx, "escape");
    app.wait_gone(cx, "rename-tab-name");

    // With the focus in the connection tree, typing does not reach the SQL.
    app.type_sql(cx, "SELECT 1");
    app.press(cx, "secondary-alt-c");
    app.update(cx, |window, cx| window.input("x", cx));
    // Past the autosave delay, so a change would be in the saved workspace.
    app.pass_time(cx, Duration::from_secs(1));
    assert_eq!(
        app.saved().tabs[0].sql,
        "SELECT 1",
        "the tree has the focus"
    );
    app.press(cx, "secondary-alt-e");
    app.update(cx, |window, cx| window.input(" -- 42", cx));
    app.wait_until(cx, "typed SQL", Duration::from_secs(5), |_, _| {
        app.saved().tabs[0].sql.contains("-- 42")
    });
}

#[gpui_kit::test]
fn an_editor_shortcut_wins_over_a_window_shortcut_with_the_same_keys(cx: &mut TestAppContext) {
    // New Tab takes the keys of Toggle Comment. In the editor, the keys
    // toggle the comment; elsewhere they open a tab.
    let (workspace, _) = workspace(&["Query 1"], &[("new_tab", Some("secondary-/"))]);
    let app = TestApp::launch(cx, workspace);
    app.type_sql(cx, "SELECT 1");
    app.press(cx, "secondary-/");
    app.wait_until(cx, "the comment", Duration::from_secs(5), |_, _| {
        app.saved().tabs[0].sql.starts_with("--")
    });
    assert_eq!(app.saved().tabs.len(), 1, "no new tab from the editor");
}

#[gpui_kit::test]
fn run_query_cannot_take_the_keys_of_an_editor_command(cx: &mut TestAppContext) {
    // Run Query also works in the SQL editor, so it conflicts there.
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-run_query");
    app.press(cx, "secondary-/");
    app.wait_for(cx, "replace-shortcut-run_query");
    assert_eq!(
        message(&app, cx, "run_query").as_deref(),
        Some("Used by Toggle Comment.")
    );
}

#[gpui_kit::test]
fn a_custom_scale_shortcut_replaces_the_default(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("increase_scale", Some("secondary-alt-="))]);
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "secondary-=");
    // Past the autosave delay, so a change would be in the saved workspace.
    app.pass_time(cx, Duration::from_secs(1));
    assert_eq!(app.saved().settings.ui_scale, 1., "⌘= has no command now");
    app.press(cx, "secondary-alt-=");
    app.wait_until(cx, "larger scale", Duration::from_secs(5), |_, _| {
        app.saved().settings.ui_scale > 1.
    });
}

#[gpui_kit::test]
fn scale_keys_work_in_a_dialog_and_an_exact_match_wins(cx: &mut TestAppContext) {
    // ⇧⌘= decreases although ⌘= (the default of Increase) ignores Shift.
    let (workspace, _) = workspace(
        &["Query 1"],
        &[
            ("decrease_scale", Some("secondary-shift-=")),
            ("rename_tab", Some("f2")),
        ],
    );
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "f2");
    app.wait_for(cx, "rename-tab-name");
    app.press(cx, "secondary-=");
    app.wait_until(cx, "larger scale", Duration::from_secs(5), |_, _| {
        app.saved().settings.ui_scale > 1.
    });
    app.press(cx, "secondary-shift-=");
    app.wait_until(cx, "the default scale", Duration::from_secs(5), |_, _| {
        app.saved().settings.ui_scale == 1.
    });
    app.wait_for(cx, "rename-tab-name");
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
    app.click_start(cx, "shortcut-toggle_results");
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
    // Past the autosave delay, so a change would be in the saved workspace.
    app.pass_time(cx, Duration::from_secs(1));
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
    app.click_start(cx, "shortcut-cancel_query");
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
    app.click_start(cx, "shortcut-toggle_results");
    app.wait_for(cx, "cancel-shortcut-toggle_results");
    app.press(cx, "escape");
    app.wait_gone(cx, "cancel-shortcut-toggle_results");
    app.wait_for(cx, "settings-content");
    assert_eq!(saved_shortcut(&app, "toggle_results"), None);

    // While a field records, ⌘W is a shortcut to record, not Close.
    app.click_start(cx, "shortcut-transpose_result");
    app.press(cx, "secondary-w");
    app.wait_for(cx, "replace-shortcut-transpose_result");
    assert_eq!(
        message(&app, cx, "transpose_result").as_deref(),
        Some("Used by Close Tab.")
    );
    app.press(cx, "escape");
    app.wait_gone(cx, "replace-shortcut-transpose_result");

    app.click_start(cx, "shortcut-toggle_results");
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
    app.click_start(cx, "shortcut-new_conversation");
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

#[gpui_kit::test]
fn recording_ends_when_the_focus_leaves_the_field(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-toggle_results");
    app.wait_for(cx, "cancel-shortcut-toggle_results");
    // The prompt ends with the focus, before any key.
    app.click_labelled(cx, "Search...");
    app.wait_gone(cx, "cancel-shortcut-toggle_results");
    // Typing in the Settings search is not a shortcut.
    app.press(cx, "backspace");
    app.pass_time(cx, Duration::from_secs(1));
    assert_eq!(saved_shortcut(&app, "toggle_results"), None);
    app.wait_gone(cx, "cancel-shortcut-toggle_results");
}

#[gpui_kit::test]
fn a_reset_to_keys_that_another_command_took_offers_replace(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(
        &["Query 1"],
        &[
            ("transpose_result", Some("secondary-j")),
            ("toggle_results", None),
        ],
    );
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "reset-shortcut-toggle_results");
    app.wait_for(cx, "replace-shortcut-toggle_results");
    assert_eq!(
        message(&app, cx, "toggle_results").as_deref(),
        Some("Used by Transpose Result.")
    );
    assert_eq!(
        saved_shortcut(&app, "toggle_results"),
        Some(None),
        "nothing changes yet"
    );
    app.click(cx, "replace-shortcut-toggle_results");
    app.wait_until(cx, "the default back", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "toggle_results").is_none()
            && saved_shortcut(&app, "transpose_result") == Some(None)
    });
}

#[gpui_kit::test]
fn fixed_keys_and_separate_places_get_the_right_message(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    // Up moves through the grid; First Row cannot take it.
    app.click_start(cx, "shortcut-first_row");
    app.press(cx, "up");
    app.wait_for(cx, "shortcut-message-first_row");
    assert_eq!(
        message(&app, cx, "first_row").as_deref(),
        Some("Used by Select Up.")
    );
    // The message is under the command name, left of the field.
    app.update(cx, |window, _| {
        let field = crate::support::bounds_of(window, "shortcut-first_row");
        let message = crate::support::bounds_of(window, "shortcut-message-first_row");
        assert!(message.right() <= field.left(), "{message:?} {field:?}");
    });
    app.press(cx, "escape");
    // Toggle Comment works in the SQL editor and Send Message in the
    // message field: they can share keys without a message.
    app.click_start(cx, "shortcut-send_message");
    app.press(cx, "secondary-/");
    app.wait_until(cx, "the shared keys", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "send_message") == Some(Some("secondary-/".into()))
    });
    assert_eq!(message(&app, cx, "send_message"), None);
}

#[gpui_kit::test]
fn enter_on_a_focused_field_records(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "f7");
    app.wait_until(cx, "F7", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "cancel_query") == Some(Some("f7".into()))
    });
    // The field keeps the focus, so Enter records again, on its release.
    press_and_release(&app, cx, "enter");
    app.wait_for(cx, "cancel-shortcut-cancel_query");
    let value = app.update(cx, |window, _| {
        window
            .try_find("shortcut-cancel_query")
            .and_then(|field| field.value().map(str::to_owned))
    });
    assert_eq!(value.as_deref(), Some("F7"));
}

#[gpui_kit::test]
fn replace_takes_the_keys_from_every_command_that_uses_them(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-quit");
    // Quit works everywhere, so ⌘↩ would hide all three of its other uses.
    app.press(cx, "secondary-enter");
    app.wait_for(cx, "replace-shortcut-quit");
    assert_eq!(
        message(&app, cx, "quit").as_deref(),
        Some("Used by Run Query, Send Message, and Save or Confirm.")
    );
    // Tab moves to the buttons of the field; it is not the new shortcut.
    app.press(cx, "tab");
    app.settle(cx);
    assert_eq!(
        message(&app, cx, "quit").as_deref(),
        Some("Used by Run Query, Send Message, and Save or Confirm.")
    );
    app.click(cx, "replace-shortcut-quit");
    app.wait_until(cx, "the moved keys", Duration::from_secs(5), |_, _| {
        let saved = app.saved().settings.shortcuts;
        saved.get("quit") == Some(&Some("secondary-enter".into()))
            && ["run_query", "send_message", "confirm_dialog"]
                .iter()
                .all(|id| saved.get(*id) == Some(&None))
    });
}

#[gpui_kit::test]
fn a_wider_command_cannot_take_the_keys_of_a_focused_place(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    // Text fields, the SQL editor among them, take ⌘[ first.
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "secondary-[");
    app.wait_for(cx, "replace-shortcut-cancel_query");
    assert_eq!(
        message(&app, cx, "cancel_query").as_deref(),
        Some("Used by Outdent.")
    );
    app.press(cx, "escape");
    app.wait_gone(cx, "cancel-shortcut-cancel_query");
    // The connection tree takes ⌥↑ first.
    app.click_start(cx, "shortcut-next_tab");
    app.press(cx, "alt-up");
    app.wait_for(cx, "replace-shortcut-next_tab");
    assert_eq!(
        message(&app, cx, "next_tab").as_deref(),
        Some("Used by Move Connection Up.")
    );
    app.press(cx, "escape");
    app.wait_gone(cx, "cancel-shortcut-next_tab");
    // The text fields of a dialog take ⇧← first.
    app.click_start(cx, "shortcut-close_dialog");
    app.press(cx, "shift-left");
    app.wait_for(cx, "shortcut-message-close_dialog");
    assert_eq!(
        message(&app, cx, "close_dialog").as_deref(),
        Some("Used by Select Left.")
    );
    app.press(cx, "escape");
    app.wait_gone(cx, "cancel-shortcut-close_dialog");
    for id in ["cancel_query", "next_tab", "close_dialog"] {
        assert_eq!(saved_shortcut(&app, id), None, "{id} changed");
    }
}

#[gpui_kit::test]
fn a_window_command_shares_keys_with_the_message_field(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("send_message", Some("f9"))]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-next_tab");
    app.press(cx, "f9");
    app.wait_until(cx, "the shared keys", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "next_tab") == Some(Some("f9".into()))
    });
    assert_eq!(
        message(&app, cx, "next_tab").as_deref(),
        Some("In the message field, F9 runs Send Message.")
    );
}

#[gpui_kit::test]
fn the_changed_row_stays_in_a_filter_that_no_longer_matches(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "shortcut-filter-No shortcut");
    app.wait_gone(cx, "shortcut-run_query");
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "f8");
    app.wait_until(cx, "F8", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "cancel_query") == Some(Some("f8".into()))
    });
    app.wait_for(cx, "shortcut-cancel_query");
}

#[gpui_kit::test]
fn settings_open_from_a_dialog(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("rename_tab", Some("f2"))]);
    let app = TestApp::launch(cx, workspace);
    app.press(cx, "f2");
    app.wait_for(cx, "rename-tab-name");
    app.press(cx, "secondary-,");
    app.wait_for(cx, "settings-content");
}

#[gpui_kit::test]
fn a_conflict_can_be_resolved_with_the_keyboard_only(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "secondary-j");
    app.wait_for(cx, "replace-shortcut-cancel_query");
    // Tab moves to Replace; Enter presses it.
    app.press(cx, "tab");
    press_and_release(&app, cx, "enter");
    app.wait_until(cx, "the moved keys", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "cancel_query") == Some(Some("secondary-j".into()))
    });
    app.wait_for(cx, "settings-content");
}

#[gpui_kit::test]
fn escape_on_replace_cancels_and_keeps_settings_open(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "secondary-j");
    app.wait_for(cx, "replace-shortcut-cancel_query");
    app.press(cx, "tab");
    app.press(cx, "escape");
    app.wait_gone(cx, "replace-shortcut-cancel_query");
    app.pass_time(cx, Duration::from_secs(1));
    app.wait_for(cx, "settings-content");
    assert_eq!(saved_shortcut(&app, "cancel_query"), None);
    // The field has the focus again, so Enter records.
    press_and_release(&app, cx, "enter");
    app.wait_for(cx, "cancel-shortcut-cancel_query");
}

#[gpui_kit::test]
fn a_recording_on_a_page_left_does_not_resume(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-refresh_metadata");
    app.wait_for(cx, "cancel-shortcut-refresh_metadata");
    app.settings_page(cx, "Appearance");
    app.wait_gone(cx, "shortcut-refresh_metadata");
    app.settings_page(cx, "Shortcuts");
    // The page keeps its scroll position, so the field shows again.
    app.wait_for(cx, "shortcut-refresh_metadata");
    app.press(cx, "f9");
    app.pass_time(cx, Duration::from_secs(1));
    assert_eq!(saved_shortcut(&app, "refresh_metadata"), None);
}

#[gpui_kit::test]
fn tab_out_of_the_field_ends_a_conflict(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "secondary-j");
    app.wait_for(cx, "replace-shortcut-cancel_query");
    // Replace, Cancel, then out of the field.
    for _ in 0..3 {
        app.press(cx, "tab");
    }
    app.wait_gone(cx, "replace-shortcut-cancel_query");
    assert_eq!(saved_shortcut(&app, "cancel_query"), None);
}

#[gpui_kit::test]
fn a_dialog_over_the_recorder_ends_the_recording(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("new_tab", Some("secondary-alt-n"))]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.wait_for(cx, "cancel-shortcut-cancel_query");
    app.click_labelled(cx, "Reset shortcuts");
    app.wait_for(cx, "confirm-reset-page");
    // The prompt does not stay behind the dialog.
    app.wait_gone(cx, "cancel-shortcut-cancel_query");
}

#[gpui_kit::test]
fn a_recorded_enter_does_not_start_another_recording(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-first_row");
    // Enter alone is a valid shortcut in the grid; its release must not
    // record again.
    press_and_release(&app, cx, "enter");
    app.wait_until(cx, "Enter", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "first_row") == Some(Some("enter".into()))
    });
    assert!(app.update(cx, |window, _| {
        window.try_find("cancel-shortcut-first_row").is_none()
    }));
}

#[gpui_kit::test]
fn closing_the_reset_dialog_returns_the_focus_to_the_field(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("new_tab", Some("secondary-alt-n"))]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "secondary-j");
    app.wait_for(cx, "replace-shortcut-cancel_query");
    // The focus is on Replace when the dialog opens.
    app.press(cx, "tab");
    app.click_labelled(cx, "Reset shortcuts");
    app.wait_for(cx, "cancel-reset-page");
    app.click(cx, "cancel-reset-page");
    app.wait_gone(cx, "cancel-reset-page");
    // The field has the focus again: Enter records, and Escape cancels
    // without closing Settings.
    press_and_release(&app, cx, "enter");
    app.wait_for(cx, "cancel-shortcut-cancel_query");
    app.press(cx, "escape");
    app.wait_gone(cx, "cancel-shortcut-cancel_query");
    app.wait_for(cx, "settings-content");
}

#[gpui_kit::test]
fn a_held_enter_records_once(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-first_row");
    key_event(&app, cx, "enter", Key::Down);
    key_event(&app, cx, "enter", Key::Repeat);
    key_event(&app, cx, "enter", Key::Up);
    app.wait_until(cx, "Enter", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "first_row") == Some(Some("enter".into()))
    });
    assert!(
        app.update(cx, |window, _| window
            .try_find("cancel-shortcut-first_row")
            .is_none()),
        "the repeat and the release do not record again"
    );
}

#[gpui_kit::test]
fn a_release_after_the_focus_moved_starts_nothing(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "f7");
    app.wait_until(cx, "F7", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "cancel_query") == Some(Some("f7".into()))
    });
    // Enter goes down on the field; the focus moves before its release.
    key_event(&app, cx, "enter", Key::Down);
    app.click_labelled(cx, "Search...");
    key_event(&app, cx, "enter", Key::Up);
    assert!(app.update(cx, |window, _| {
        window.try_find("cancel-shortcut-cancel_query").is_none()
    }));
}

#[gpui_kit::test]
fn confirming_a_reset_under_a_filter_keeps_the_keyboard_working(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[("new_tab", Some("secondary-alt-n"))]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click(cx, "shortcut-filter-Customized");
    app.wait_gone(cx, "shortcut-run_query");
    app.click_start(cx, "shortcut-new_tab");
    app.wait_for(cx, "cancel-shortcut-new_tab");
    app.reset_settings_page(cx, "Reset shortcuts");
    app.wait_until(cx, "the defaults", Duration::from_secs(5), |_, _| {
        app.saved().settings.shortcuts.is_empty()
    });
    // The Settings window has the focus, so Escape closes it.
    app.press(cx, "escape");
    app.wait_gone(cx, "settings-content");
}

#[gpui_kit::test]
fn a_press_that_left_its_field_starts_nothing(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-cancel_query");
    app.press(cx, "f7");
    app.wait_until(cx, "F7", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "cancel_query") == Some(Some("f7".into()))
    });
    // Enter goes down; the focus leaves and comes back before its release.
    key_event(&app, cx, "enter", Key::Down);
    app.press(cx, "tab");
    app.press(cx, "shift-tab");
    key_event(&app, cx, "enter", Key::Up);
    assert!(app.update(cx, |window, _| {
        window.try_find("cancel-shortcut-cancel_query").is_none()
    }));
    // A modifier added before the release is not a click either.
    key_event(&app, cx, "enter", Key::Down);
    key_event(&app, cx, "shift-enter", Key::Up);
    assert!(app.update(cx, |window, _| {
        window.try_find("cancel-shortcut-cancel_query").is_none()
    }));
}

#[gpui_kit::test]
fn another_key_during_a_held_enter_does_not_restart_recording(cx: &mut TestAppContext) {
    let (workspace, _) = workspace(&["Query 1"], &[]);
    let app = TestApp::launch(cx, workspace);
    open_shortcuts(&app, cx);
    app.click_start(cx, "shortcut-first_row");
    key_event(&app, cx, "enter", Key::Down);
    key_event(&app, cx, "x", Key::Down);
    key_event(&app, cx, "x", Key::Up);
    key_event(&app, cx, "enter", Key::Repeat);
    key_event(&app, cx, "enter", Key::Up);
    app.wait_until(cx, "Enter", Duration::from_secs(5), |_, _| {
        saved_shortcut(&app, "first_row") == Some(Some("enter".into()))
    });
    assert!(app.update(cx, |window, _| {
        window.try_find("cancel-shortcut-first_row").is_none()
    }));
}

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Down,
    Repeat,
    Up,
}

/// One key event, as the keyboard sends it.
fn key_event(app: &TestApp, cx: &mut TestAppContext, key: &str, kind: Key) {
    use gpui_kit::{InputEvent as _, KeyDownEvent, KeyUpEvent, Keystroke};
    let keystroke = Keystroke::parse(key).unwrap();
    app.update(cx, |window, cx| {
        let event = match kind {
            Key::Up => KeyUpEvent { keystroke }.to_platform_input(),
            _ => KeyDownEvent {
                keystroke,
                is_held: kind == Key::Repeat,
                prefer_character_input: false,
            }
            .to_platform_input(),
        };
        window.dispatch_event(event, cx);
        window.render_frame(cx);
    });
    app.settle(cx);
}

/// A key press with its release, as a keyboard sends it. A button clicks on
/// the release.
fn press_and_release(app: &TestApp, cx: &mut TestAppContext, key: &str) {
    use gpui_kit::{InputEvent as _, KeyDownEvent, KeyUpEvent, Keystroke};
    let keystroke = Keystroke::parse(key).unwrap();
    app.update(cx, |window, cx| {
        window.dispatch_event(
            KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        window.render_frame(cx);
    });
    app.settle(cx);
}

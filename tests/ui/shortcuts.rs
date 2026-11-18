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

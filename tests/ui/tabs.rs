use crate::support::{TestApp, offline_profile, value};
use gpui_kit::TestAppContext;
use qrow::model::{SavedTab, Workspace};
use std::time::Duration;
use uuid::Uuid;

fn tab(profile: Uuid, title: &str, sql: &str) -> SavedTab {
    SavedTab {
        title: title.into(),
        sql: sql.into(),
        ..SavedTab::new(1, Some(profile))
    }
}

/// Titles and SQL of the saved tabs of a connection, in tab order.
fn saved_tabs(app: &TestApp, profile: Uuid) -> Vec<(String, String)> {
    app.saved()
        .tabs
        .into_iter()
        .filter(|tab| tab.profile == Some(profile))
        .map(|tab| (tab.title, tab.sql))
        .collect()
}

fn wait_tabs(app: &TestApp, cx: &mut TestAppContext, profile: Uuid, expected: &[(&str, &str)]) {
    let expected: Vec<_> = expected
        .iter()
        .map(|(t, s)| (t.to_string(), s.to_string()))
        .collect();
    app.wait_until(
        cx,
        &format!("tabs {expected:?}"),
        Duration::from_secs(10),
        |_, _| saved_tabs(app, profile) == expected,
    );
}

fn rename(app: &TestApp, cx: &mut TestAppContext, title: &str) {
    app.context_menu_labelled(cx, title);
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "rename-tab-name");
}

#[gpui_kit::test]
fn duplicates_get_unique_names_and_keep_the_sql(cx: &mut TestAppContext) {
    let alpha = offline_profile("Alpha");
    let id = alpha.id;
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![tab(id, "Query 1", "SELECT 1")],
            profiles: vec![alpha],
            ..Workspace::default()
        },
    );
    for expected in ["Query 1 (Copy)", "Query 1 (Copy 2)"] {
        app.click_labelled(cx, "Query 1");
        app.context_menu_labelled(cx, "Query 1");
        app.choose(cx, "popup-menu", "Duplicate");
        app.wait_until(cx, expected, Duration::from_secs(10), |_, _| {
            saved_tabs(&app, id)
                .iter()
                .any(|(title, _)| title == expected)
        });
    }
    wait_tabs(
        &app,
        cx,
        id,
        &[
            ("Query 1", "SELECT 1"),
            ("Query 1 (Copy)", "SELECT 1"),
            ("Query 1 (Copy 2)", "SELECT 1"),
        ],
    );
}

#[gpui_kit::test]
fn a_rename_rejects_taken_and_long_names(cx: &mut TestAppContext) {
    let alpha = offline_profile("Alpha");
    let id = alpha.id;
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![
                tab(id, "Query 1", "SELECT 1"),
                tab(id, "Query 2", "SELECT 2"),
            ],
            profiles: vec![alpha],
            ..Workspace::default()
        },
    );
    rename(&app, cx, "Query 1");
    app.update(cx, |window, _| {
        assert_eq!(value(window, "rename-tab-name").as_deref(), Some("Query 1"))
    });
    // Each rejected name keeps the dialog open and the title unchanged.
    for rejected in ["Query 2".to_owned(), "x".repeat(61)] {
        app.fill(cx, "rename-tab-name", &rejected);
        app.click(cx, "rename-tab");
        app.settle(cx);
        app.wait_for(cx, "rename-tab-name");
        wait_tabs(
            &app,
            cx,
            id,
            &[("Query 1", "SELECT 1"), ("Query 2", "SELECT 2")],
        );
    }
    app.click(cx, "cancel-tab");
    app.wait_gone(cx, "rename-tab-name");

    rename(&app, cx, "Query 1");
    app.fill(cx, "rename-tab-name", "Renamed tab");
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    wait_tabs(
        &app,
        cx,
        id,
        &[("Renamed tab", "SELECT 1"), ("Query 2", "SELECT 2")],
    );
    // The same name again closes the dialog without a change.
    rename(&app, cx, "Renamed tab");
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    wait_tabs(
        &app,
        cx,
        id,
        &[("Renamed tab", "SELECT 1"), ("Query 2", "SELECT 2")],
    );
}

#[gpui_kit::test]
fn copy_and_move_to_another_connection(cx: &mut TestAppContext) {
    let (alpha, beta) = (offline_profile("Alpha"), offline_profile("Beta"));
    let (a, b) = (alpha.id, beta.id);
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![tab(a, "Query 1", "SELECT 'moved'"), tab(b, "Query 1", "")],
            profiles: vec![alpha, beta],
            ..Workspace::default()
        },
    );
    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Copy to Connection…", "Beta");
    wait_tabs(
        &app,
        cx,
        b,
        &[("Query 1", ""), ("Query 1 (Copy)", "SELECT 'moved'")],
    );
    wait_tabs(&app, cx, a, &[("Query 1", "SELECT 'moved'")]);

    // Copy selected Beta. Return to Alpha and move its only tab.
    app.click(cx, crate::support::connection_row(a));
    app.wait_until(cx, "Alpha's tab", Duration::from_secs(10), |window, _| {
        crate::support::labelled(window, "Query 1").is_some()
    });
    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Move to Connection…", "Beta");
    wait_tabs(
        &app,
        cx,
        b,
        &[
            ("Query 1", ""),
            ("Query 1 (Copy)", "SELECT 'moved'"),
            ("Query 1 (Copy 2)", "SELECT 'moved'"),
        ],
    );
    // Moving the last tab leaves the source connection with a new blank tab.
    wait_tabs(&app, cx, a, &[("Query 1", "")]);
}

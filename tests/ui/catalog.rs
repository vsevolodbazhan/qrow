//! The schema tree in the Connections sidebar, with cached catalogs and
//! connections that no test reaches.
use crate::support::{
    MemoryCredentials, TestApp, connection_row, labelled, offline_profile, press_at, shows,
};
use gpui_kit::TestAppContext;
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind},
    model::{CatalogSettings, Profile, SavedTab, Workspace},
    storage::{self, Credentials},
};
use std::{collections::BTreeMap, net::TcpListener, time::Duration};
use tempfile::TempDir;

fn table(name: &str, kind: RelationKind) -> RelationEntry {
    RelationEntry {
        name: name.into(),
        kind,
        comment: None,
    }
}

fn column(name: &str, data_type: &str) -> CatalogColumn {
    CatalogColumn {
        name: name.into(),
        data_type: data_type.into(),
        comment: None,
    }
}

/// Saves a schema cache for `profile` next to the workspace in `directory`.
fn cache(directory: &TempDir, profile: &Profile, schemas: &[(&str, Option<&[RelationEntry]>)]) {
    let mut catalog = Catalog::new(profile);
    let settings = CatalogSettings::default();
    catalog.apply_schemas(
        schemas.iter().map(|(name, _)| (*name).into()).collect(),
        &settings,
        1,
    );
    for (schema, relations) in schemas {
        if let Some(relations) = relations {
            catalog.apply_relations(schema, None, relations.to_vec(), 1);
        }
    }
    if catalog.relation("avia", "bookings").is_some() {
        catalog.apply_columns(
            "avia",
            Some("bookings"),
            BTreeMap::from([(
                "bookings".into(),
                vec![column("booking_id", "BIGINT"), column("gate", "STRING")],
            )]),
            1,
        );
    }
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
}

fn avia(directory: &TempDir, profile: &Profile) {
    cache(
        directory,
        profile,
        &[
            (
                "avia",
                Some(&[
                    table("bookings", RelationKind::Table),
                    table("daily", RelationKind::View),
                ]),
            ),
            ("finance", None),
        ],
    );
}

fn workspace(profiles: Vec<Profile>) -> Workspace {
    Workspace {
        tabs: vec![SavedTab::new(1, Some(profiles[0].id))],
        profiles,
        ..Workspace::default()
    }
}

fn expand_connection(app: &TestApp, cx: &mut TestAppContext, profile: &Profile) {
    app.toggle_connection(cx, profile.id);
}

fn wait_shows(app: &TestApp, cx: &mut TestAppContext, text: &str) {
    app.wait_until(cx, text, Duration::from_secs(10), |window, _| {
        labelled(window, text).is_some()
    });
}

#[gpui_kit::test]
fn the_tree_shows_a_cached_catalog_without_a_session(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &profile);
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    app.update(cx, |window, _| assert!(labelled(window, "avia").is_none()));

    expand_connection(&app, cx, &profile);
    wait_shows(&app, cx, "avia");
    app.click_labelled(cx, "avia");
    wait_shows(&app, cx, "daily");
    app.click_labelled(cx, "bookings");
    wait_shows(&app, cx, "booking_id BIGINT");
    wait_shows(&app, cx, "gate STRING");

    // Without a live session, an unloaded schema waits for a refresh.
    app.click_labelled(cx, "finance");
    wait_shows(&app, cx, "Not loaded");
    assert_eq!(app.credentials.reads(), 0, "The tree must not connect");

    app.click_labelled(cx, "avia");
    app.wait_until(
        cx,
        "avia to collapse",
        Duration::from_secs(10),
        |window, _| labelled(window, "bookings").is_none(),
    );
}

#[gpui_kit::test]
fn a_connection_click_selects_it_and_the_disclosure_expands_it(cx: &mut TestAppContext) {
    let first = offline_profile("First");
    let second = offline_profile("Second");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &second);
    let mut workspace = workspace(vec![first, second.clone()]);
    workspace.tabs.push(SavedTab::new(1, Some(second.id)));
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());

    app.click(cx, connection_row(second.id));
    app.wait_until(
        cx,
        "Second to be active",
        Duration::from_secs(10),
        |_, _| app.saved().active_tab == 1,
    );
    app.update(cx, |window, _| {
        assert!(
            labelled(window, "avia").is_none(),
            "A click must not expand"
        );
    });

    expand_connection(&app, cx, &second);
    wait_shows(&app, cx, "avia");
    app.press(cx, "left");
    app.wait_until(
        cx,
        "Second to collapse",
        Duration::from_secs(10),
        |window, _| labelled(window, "avia").is_none(),
    );
}

#[gpui_kit::test]
fn search_finds_tables_in_every_cache_and_limits_the_matches(cx: &mut TestAppContext) {
    let first = offline_profile("First");
    let second = offline_profile("Second");
    let directory = tempfile::tempdir().unwrap();
    let many: Vec<_> = (0..600)
        .map(|n| table(&format!("t{n:03}"), RelationKind::Table))
        .collect();
    cache(
        &directory,
        &first,
        &[
            ("sales", Some(&[table("orders", RelationKind::Table)])),
            ("wide", Some(&many)),
        ],
    );
    cache(
        &directory,
        &second,
        &[(
            "archive",
            Some(&[table("orders_2020", RelationKind::Table)]),
        )],
    );
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![first, second]),
        MemoryCredentials::default(),
    );

    app.fill_labelled(cx, "Search Tables", "orders");
    wait_shows(&app, cx, "orders");
    wait_shows(&app, cx, "orders_2020");
    app.update(cx, |window, _| {
        assert!(labelled(window, "wide").is_none());
        assert!(!shows(window, "Refine your search"));
    });

    app.fill_labelled(cx, "Search Tables", "t");
    app.wait_until(
        cx,
        "the match limit",
        Duration::from_secs(10),
        |window, _| shows(window, "Refine your search"),
    );

    app.fill_labelled(cx, "Search Tables", "");
    app.wait_until(
        cx,
        "the search to end",
        Duration::from_secs(10),
        |window, _| labelled(window, "orders").is_none() && labelled(window, "sales").is_none(),
    );
    assert_eq!(app.credentials.reads(), 0);
}

#[gpui_kit::test]
fn names_go_to_the_clipboard_and_into_the_editor(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &profile);
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    expand_connection(&app, cx, &profile);
    app.click_labelled(cx, "avia");
    app.click_labelled(cx, "bookings");
    wait_shows(&app, cx, "gate STRING");

    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Copy Qualified Name");
    app.settle(cx);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("avia.bookings")
    );

    app.context_menu_labelled(cx, "daily");
    app.choose(cx, "popup-menu", "Insert into Editor");
    app.wait_until(cx, "the inserted name", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == "avia.daily"
    });

    let gate = app.update(cx, |window, _| {
        labelled(window, "gate STRING").unwrap().bounds()
    });
    app.update(cx, |window, cx| press_at(window, gate.center(), 2, cx));
    app.wait_until(
        cx,
        "the double-clicked column",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == "avia.dailygate",
    );
}

#[gpui_kit::test]
fn a_failed_refresh_shows_its_error_on_the_connection(cx: &mut TestAppContext) {
    // A port that was free a moment ago refuses the connection.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let profile = Profile {
        host: "127.0.0.1".into(),
        port,
        ..offline_profile("Unreachable")
    };
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_with(cx, workspace(vec![profile.clone()]), credentials);

    expand_connection(&app, cx, &profile);
    wait_shows(&app, cx, "Not loaded");
    assert_eq!(app.credentials.reads(), 0);
    app.click_labelled(cx, "Refresh");
    app.wait_until(
        cx,
        "the refresh error",
        Duration::from_secs(20),
        |window, _| shows(window, "refused"),
    );
    assert_eq!(app.credentials.reads(), 1);
    // The header button tries the active connection again.
    app.click(cx, "refresh-schemas");
    app.wait_until(cx, "a second attempt", Duration::from_secs(20), |_, _| {
        app.credentials.reads() == 2
    });
}

#[gpui_kit::test]
fn deleting_a_connection_deletes_its_cache(cx: &mut TestAppContext) {
    let kept = offline_profile("Kept");
    let deleted = offline_profile("Deleted");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &kept);
    avia(&directory, &deleted);
    let workspace_file = directory.path().join("workspace.json");
    let (kept_cache, deleted_cache) = (
        storage::catalog_path(&workspace_file, kept.id),
        storage::catalog_path(&workspace_file, deleted.id),
    );
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![kept, deleted.clone()]),
        MemoryCredentials::default(),
    );
    app.context_menu(cx, connection_row(deleted.id));
    app.choose(cx, "popup-menu", "Delete");
    app.click(cx, "confirm-delete-connection");
    app.wait_until(cx, "the deleted cache", Duration::from_secs(10), |_, _| {
        !deleted_cache.exists()
    });
    assert!(kept_cache.exists());
}

#[gpui_kit::test]
fn hidden_schemas_leave_the_tree_when_the_connection_is_saved(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &profile);
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    expand_connection(&app, cx, &profile);
    wait_shows(&app, cx, "finance");

    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit Connection…");
    app.scroll_to(cx, "connection-hide-schemas");
    app.fill(cx, "connection-hide-schemas", "fin*, scratch");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(
        cx,
        "finance to leave",
        Duration::from_secs(10),
        |window, _| labelled(window, "finance").is_none() && labelled(window, "avia").is_some(),
    );
    app.wait_until(cx, "the saved patterns", Duration::from_secs(10), |_, _| {
        app.saved().profiles[0].catalog.exclude == ["fin*", "scratch"]
    });
    assert_eq!(app.credentials.reads(), 0);
}

//! The schema tree in the Connections sidebar, with cached catalogs and
//! connections that no test reaches.
use crate::support::{
    MemoryCredentials, TestApp, connection_row, label, labelled, offline_profile, press_at, shows,
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
    // Only Insert into Editor inserts. A double-click does not change SQL.
    app.update(cx, |window, cx| press_at(window, gate.center(), 2, cx));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(labelled(window, "gate STRING").is_some());
    });
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(app.saved().tabs[0].sql, "avia.daily");
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
    // The connection menu tries again.
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Refresh Schemas");
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

#[gpui_kit::test]
fn the_search_limit_does_not_apply_without_a_search(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..600).map(|n| format!("schema_{n:03}")).collect();
    let relations = [table("orders", RelationKind::Table)];
    let schemas: Vec<(&str, Option<&[RelationEntry]>)> = names
        .iter()
        .map(|name| (name.as_str(), Some(&relations[..])))
        .collect();
    cache(&directory, &profile, &schemas);
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    expand_connection(&app, cx, &profile);
    wait_shows(&app, cx, "schema_000");
    app.update(cx, |window, _| {
        assert!(!shows(window, "Refine your search"));
    });
    // The 600th schema is in the tree: a search for its table finds it.
    app.fill_labelled(cx, "Search Tables", "schema_599");
    wait_shows(&app, cx, "schema_599");
    app.fill_labelled(cx, "Search Tables", "");
    app.wait_until(cx, "the full tree", Duration::from_secs(10), |window, _| {
        labelled(window, "schema_000").is_some() && !shows(window, "Refine your search")
    });
}

#[gpui_kit::test]
fn the_keyboard_copies_and_inserts_the_selected_name(cx: &mut TestAppContext) {
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

    // A click selects the row and focuses the tree. Down moves to the next row.
    app.click_labelled(cx, "booking_id BIGINT");
    app.press(cx, "down");
    app.press(cx, "cmd-c");
    app.settle(cx);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("gate")
    );
    app.press(cx, "shift-enter");
    app.wait_until(
        cx,
        "the inserted column",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == "gate",
    );
}

#[gpui_kit::test]
fn schema_refreshes_show_in_logs_only_when_the_connection_enables_them(cx: &mut TestAppContext) {
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

    // Off by default: a refresh adds nothing to Logs.
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Refresh Schemas");
    app.wait_until(cx, "the first attempt", Duration::from_secs(20), |_, _| {
        app.credentials.reads() == 1
    });
    app.settle(cx);
    assert!(!app.logs(cx).contains("schema refresh"));

    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit Connection…");
    app.scroll_to(cx, "connection-log-refreshes");
    app.click(cx, "connection-log-refreshes");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the saved option", Duration::from_secs(10), |_, _| {
        app.saved().profiles[0].catalog.log_refreshes
    });

    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Refresh Schemas");
    app.wait_until(cx, "the second attempt", Duration::from_secs(20), |_, _| {
        app.credentials.reads() == 2
    });
    let mut logs = String::new();
    app.wait_until(
        cx,
        "the refresh in Logs",
        Duration::from_secs(10),
        |_, _| true,
    );
    for _ in 0..100 {
        logs = app.logs(cx);
        if logs.contains("Schema refresh failed") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        logs.contains("Started a schema refresh of the connection"),
        "{logs}"
    );
    assert!(logs.contains("Schema refresh failed"), "{logs}");
    // A refresh error does not mark the connection as having an unread error.
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, connection_row(profile.id)).as_deref(),
            Some("Unreachable")
        );
    });
}

#[gpui_kit::test]
fn a_tooltip_shows_only_a_cut_name_or_a_comment(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    let long = format!("asb_exp_{}_app_android_v1", "compactorderdetails".repeat(4));
    let mut catalog = Catalog::new(&profile);
    catalog.apply_schemas(vec!["avia".into()], &CatalogSettings::default(), 1);
    catalog.apply_relations(
        "avia",
        None,
        vec![
            table(&long, RelationKind::Table),
            table("daily", RelationKind::View),
        ],
        1,
    );
    catalog.apply_columns(
        "avia",
        Some("daily"),
        BTreeMap::from([(
            "daily".into(),
            vec![
                CatalogColumn {
                    name: "day".into(),
                    data_type: "DATE".into(),
                    comment: Some("Booking day".into()),
                },
                column("gate", "STRING"),
            ],
        )]),
        1,
    );
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    expand_connection(&app, cx, &profile);
    app.click_labelled(cx, "avia");
    app.click_labelled(cx, "daily");
    wait_shows(&app, cx, "gate STRING");

    let hover = |app: &TestApp, cx: &mut TestAppContext, row: &str| -> Option<String> {
        // One hover, then rest: the row decides when its tooltip opens.
        app.hover_labelled(cx, row);
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, _| label(window, "catalog-tooltip"))
    };
    assert_eq!(hover(&app, cx, &long), Some(long.clone()));
    assert_eq!(hover(&app, cx, "daily"), None);
    assert_eq!(hover(&app, cx, "gate STRING"), None);
    assert_eq!(
        hover(&app, cx, "day DATE").as_deref(),
        Some("day DATE\nBooking day")
    );

    // An open menu hides the tooltip of the row that it belongs to.
    app.context_menu_labelled(cx, "day DATE");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(label(window, "catalog-tooltip"), None);
    });
}

#[gpui_kit::test]
fn collapse_all_closes_the_rows_below_a_connection_or_a_schema(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &profile);
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    let gone = |app: &TestApp, cx: &mut TestAppContext, text: &str| {
        app.wait_until(
            cx,
            &format!("{text} to collapse"),
            Duration::from_secs(10),
            |window, _| labelled(window, text).is_none(),
        );
    };
    expand_connection(&app, cx, &profile);
    app.click_labelled(cx, "avia");
    app.click_labelled(cx, "bookings");
    wait_shows(&app, cx, "gate STRING");

    // A schema keeps its relations open, but their columns close.
    app.context_menu_labelled(cx, "avia");
    app.choose(cx, "popup-menu", "Collapse All");
    gone(&app, cx, "gate STRING");
    wait_shows(&app, cx, "bookings");

    // A connection keeps its schemas, but they close.
    app.click_labelled(cx, "bookings");
    wait_shows(&app, cx, "gate STRING");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Collapse All");
    gone(&app, cx, "bookings");
    wait_shows(&app, cx, "avia");

    // A search expands the schemas with matches. Collapse All closes them too.
    app.fill_labelled(cx, "Search Tables", "book");
    wait_shows(&app, cx, "bookings");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Collapse All");
    gone(&app, cx, "bookings");
    wait_shows(&app, cx, "avia");
}

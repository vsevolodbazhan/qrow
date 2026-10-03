//! The schema tree in the Connections sidebar, with cached catalogs and
//! connections that no test reaches.
use crate::support::{
    MemoryCredentials, TestApp, bounds_of, connection_row, elements, label, labelled, menu_item,
    offline_profile, press_at, shows, value,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, TestAppContext};
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind},
    model::{CatalogRefresh, CatalogSettings, Profile, SavedTab, SharedCatalog, Workspace},
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

/// Opens the Schema catalog list, searches for `option`, and chooses it.
fn choose_catalog(app: &TestApp, cx: &mut TestAppContext, option: &str) {
    app.click(cx, "connection-schema-catalog");
    app.settle(cx);
    app.update(cx, |window, cx| window.input(option, cx));
    app.settle(cx);
    app.press(cx, "enter");
    app.wait_until(cx, option, Duration::from_secs(10), |window, _| {
        value(window, "connection-schema-catalog").as_deref() == Some(option)
    });
    // Enter chooses the row and keeps the list open. Escape closes it.
    if app.update(cx, |window, _| {
        window.try_find("connection-new-shared-catalog").is_some()
    }) {
        app.press(cx, "escape");
    }
    app.wait_gone(cx, "connection-new-shared-catalog");
    app.wait_for(cx, "connection-name");
}

/// Scrolls the connection form to its end with the wheel over the refresh
/// period field.
fn scroll_form_to_end(app: &TestApp, cx: &mut TestAppContext) {
    app.scroll_to(cx, "connection-refresh-period");
    app.update(cx, |window, cx| {
        let position = window.find("connection-refresh-period").bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-400.),
                )),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    app.settle(cx);
}

fn expand_connection(app: &TestApp, cx: &mut TestAppContext, profile: &Profile) {
    app.toggle_connection(cx, profile.id);
}

fn wait_shows(app: &TestApp, cx: &mut TestAppContext, text: &str) {
    app.wait_until(cx, text, Duration::from_secs(10), |window, _| {
        labelled(window, text).is_some()
    });
}

/// A state notice describes its parent; its text uses the parent's label lane.
fn assert_notice_alignment(app: &TestApp, cx: &mut TestAppContext, parent: &str) {
    app.update(cx, |window, _| {
        let parent_label = bounds_of(window, &format!("{parent}\u{1f}label"));
        let notice_label = bounds_of(window, &format!("{parent}\u{1f}notice\u{1f}label"));
        assert!(
            (parent_label.left() - notice_label.left()).abs() <= gpui_kit::px(0.5),
            "Notice label {notice_label:?} must align with parent label {parent_label:?}"
        );
        assert!(notice_label.top() >= parent_label.bottom());
    });
}

fn assert_status_alignment(app: &TestApp, cx: &mut TestAppContext, id: &str) {
    app.update(cx, |window, _| {
        let lane = bounds_of(window, "add-connection").center().x;
        let status = bounds_of(window, id);
        assert!(
            (status.center().x - lane).abs() <= gpui_kit::px(0.5),
            "Status {status:?} must share the header's centerline {lane:?}"
        );
    });
}

#[gpui_kit::test]
fn unloaded_and_loading_notices_align_with_their_parent_at_each_depth(cx: &mut TestAppContext) {
    for scale in [0.75, 1.5] {
        // A listening socket holds each refresh in its loading state without
        // server responses or access to a user's connection.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let profiles: Vec<_> = ["Connection", "Schema", "Table"]
            .into_iter()
            .map(|name| Profile {
                host: "127.0.0.1".into(),
                port,
                ..offline_profile(name)
            })
            .collect();
        let directory = tempfile::tempdir().unwrap();
        cache(&directory, &profiles[1], &[("finance", None)]);
        cache(
            &directory,
            &profiles[2],
            &[("finance", Some(&[table("daily", RelationKind::Table)]))],
        );
        let credentials = MemoryCredentials::default();
        for profile in &profiles {
            credentials
                .set_password(profile.id, "synthetic-password")
                .unwrap();
        }
        let mut workspace = workspace(profiles.clone());
        workspace.settings.ui_scale = scale;
        let app = TestApp::launch_in(cx, directory, workspace, credentials);
        let parents = [
            format!("c\u{1f}{}", profiles[0].id),
            format!("s\u{1f}{}\u{1f}finance", profiles[1].id),
            format!("r\u{1f}{}\u{1f}finance\u{1f}daily", profiles[2].id),
        ];
        for (depth, profile) in profiles.iter().enumerate() {
            expand_connection(&app, cx, profile);
            if depth > 0 {
                // Expand this connection's schema, which has a stable node ID.
                let schema_label = format!("s\u{1f}{}\u{1f}finance\u{1f}label", profile.id);
                app.wait_for(cx, schema_label.clone());
                app.click(cx, schema_label);
            }
            if depth == 2 {
                wait_shows(&app, cx, "daily");
                app.click_labelled(cx, "daily");
            }
            app.wait_for(cx, format!("{}\u{1f}notice\u{1f}label", parents[depth]));
            assert_notice_alignment(&app, cx, &parents[depth]);
            app.update(cx, |window, _| {
                let lane = bounds_of(window, "add-connection").center().x;
                let refresh = bounds_of(
                    window,
                    &format!("{}\u{1f}notice\u{1f}refresh", parents[depth]),
                );
                assert!(
                    (refresh.right() - lane).abs() <= gpui_kit::px(0.5),
                    "Refresh text must end at the status centerline"
                );
            });
        }
        assert_status_alignment(
            &app,
            cx,
            &format!("s\u{1f}{}\u{1f}finance\u{1f}detail", profiles[2].id),
        );
        // Scope-specific refreshes retain the same alignment with a spinner.
        for (depth, parent) in parents.iter().enumerate() {
            let refresh = format!("{parent}\u{1f}notice\u{1f}refresh");
            app.click(cx, refresh.clone());
            app.wait_until(
                cx,
                "a loading notice",
                Duration::from_secs(10),
                |window, _| {
                    window.try_find(refresh.clone()).is_none()
                        && labelled(
                            window,
                            if depth == 0 {
                                "Loading schemas…"
                            } else {
                                "Loading…"
                            },
                        )
                        .is_some()
                },
            );
            assert_notice_alignment(&app, cx, parent);
            let status = if depth == 0 {
                format!("connection-busy-{}", profiles[0].id)
            } else {
                format!("{parent}\u{1f}busy")
            };
            assert_status_alignment(&app, cx, &status);
        }
        drop(listener);
        // Failed refreshes keep the same trailing lane for their warning icons.
        for (depth, parent) in parents.iter().enumerate() {
            let status = if depth == 0 {
                format!("connection-error-{}", profiles[0].id)
            } else {
                format!("{parent}\u{1f}error-icon")
            };
            app.wait_for(cx, status.clone());
            assert_status_alignment(&app, cx, &status);
        }
        app.update(cx, |window, _| window.remove_window());
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn large_schema_counts_remain_readable(cx: &mut TestAppContext) {
    let profile = offline_profile("Large catalog");
    let directory = tempfile::tempdir().unwrap();
    let relations: Vec<_> = (0..10_000)
        .map(|index| table(&format!("table_{index}"), RelationKind::Table))
        .collect();
    cache(&directory, &profile, &[("finance", Some(&relations))]);
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    expand_connection(&app, cx, &profile);
    wait_shows(&app, cx, "finance");
    app.update(cx, |window, cx| {
        let text = gpui_kit::SharedString::from("10000");
        let run = gpui_kit::TextRun {
            len: text.len(),
            font: gpui_kit::font(cx.theme().font_family.clone()),
            color: cx.theme().muted_foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(text, window.rem_size() * 0.75, &[run], None);
        let count = bounds_of(
            window,
            &format!("s\u{1f}{}\u{1f}finance\u{1f}detail", profile.id),
        );
        assert!(
            count.size.width + gpui_kit::px(0.5) >= line.width,
            "The full count must fit without clipping"
        );
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
        Some("`avia`.`bookings`")
    );

    app.context_menu_labelled(cx, "daily");
    app.choose(cx, "popup-menu", "Insert into Editor");
    app.wait_until(cx, "the inserted name", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == "`avia`.`daily`"
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
    assert_eq!(app.saved().tabs[0].sql, "`avia`.`daily`");
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
    app.choose(cx, "popup-menu", "Refresh");
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
    app.choose(cx, "popup-menu", "Edit");
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
        |_, _| app.saved().tabs[0].sql == "`gate`",
    );
}

#[gpui_kit::test]
fn schema_refreshes_go_to_activity_and_not_to_tab_logs(cx: &mut TestAppContext) {
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

    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Refresh");
    app.wait_until(cx, "the attempt", Duration::from_secs(20), |_, _| {
        app.credentials.reads() == 1
    });
    // The status bar counts the refresh error, which no tab shows.
    app.wait_until(
        cx,
        "the unseen error",
        Duration::from_secs(10),
        |window, _| label(window, "toggle-activity").as_deref() == Some("Activity, 1 unseen error"),
    );
    assert!(!app.logs(cx).contains("schema refresh"));

    // The status bar opens the connection with the newest unseen error.
    // Showing its Activity marks its errors as seen.
    app.click(cx, "toggle-activity");
    app.wait_for(cx, "activity");
    let activity = app.copy_activity(cx);
    assert!(
        activity.contains("Started a schema refresh of the connection"),
        "{activity}"
    );
    assert!(activity.contains("Schema refresh failed"), "{activity}");
    app.wait_until(
        cx,
        "the seen error",
        Duration::from_secs(10),
        |window, _| label(window, "toggle-activity").as_deref() == Some("Activity"),
    );
    // The rows follow the UI scale while Activity is open.
    let row_height = |app: &TestApp, cx: &mut TestAppContext| {
        app.update(cx, |window, _| {
            elements(window)
                .iter()
                .filter(|element| {
                    element
                        .path()
                        .last()
                        .is_some_and(|id| format!("{id:?}").contains("activity-entry"))
                })
                .map(|element| f32::from(element.bounds().size.height))
                // A row without buttons has only text.
                .fold(f32::MAX, f32::min)
        })
    };
    let before = row_height(&app, cx);
    app.press(cx, "cmd-=");
    app.settle(cx);
    let after = row_height(&app, cx);
    assert!(after > before, "{before} -> {after}");
    app.press(cx, "cmd--");
    app.click(cx, "activity-errors");
    let errors = app.copy_activity(cx);
    assert!(!errors.contains("Started a schema refresh"), "{errors}");
    assert!(errors.contains("Schema refresh failed"), "{errors}");
    app.press(cx, "escape");
    app.wait_gone(cx, "activity");
    // ⇧⌘U opens and closes Activity too.
    app.press(cx, "cmd-shift-u");
    app.wait_for(cx, "activity");
    app.press(cx, "cmd-shift-u");
    app.wait_gone(cx, "activity");

    // A refresh error does not mark the connection as having an unread
    // error. The collapsed connection row shows the refresh error.
    app.wait_until(
        cx,
        "the refresh error",
        Duration::from_secs(10),
        |window, _| {
            label(window, connection_row(profile.id)).as_deref()
                == Some("Unreachable, schema refresh error")
        },
    );
    // A click on the warning opens the Activity of the connection.
    app.click(cx, format!("connection-error-{}", profile.id));
    app.wait_for(cx, "activity");
    assert!(app.copy_activity(cx).contains("Schema refresh failed"));
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
    app.choose(cx, "popup-menu", "Collapse");
    gone(&app, cx, "gate STRING");
    wait_shows(&app, cx, "bookings");

    // A connection keeps its schemas, but they close.
    app.click_labelled(cx, "bookings");
    wait_shows(&app, cx, "gate STRING");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Collapse");
    gone(&app, cx, "bookings");
    wait_shows(&app, cx, "avia");

    // A search expands the schemas with matches. Collapse closes them too.
    app.fill_labelled(cx, "Search Tables", "book");
    wait_shows(&app, cx, "bookings");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Collapse");
    gone(&app, cx, "bookings");
    wait_shows(&app, cx, "avia");
}

#[gpui_kit::test]
fn the_schema_refresh_policy_is_validated_and_saved(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_with(cx, workspace(vec![profile.clone()]), credentials);
    let wait_error = |cx: &mut TestAppContext, expected: &str| {
        app.wait_until(cx, expected, Duration::from_secs(10), |window, _| {
            label(window, "connection-form-error-accessibility").as_deref() == Some(expected)
        });
    };
    let edit = |cx: &mut TestAppContext| {
        app.context_menu(cx, connection_row(profile.id));
        app.choose(cx, "popup-menu", "Edit");
        app.scroll_to(cx, "connection-schema-refresh");
    };
    let saved = |app: &TestApp| app.saved().profiles[0].catalog.clone();

    // Manual: Schema refresh comes first, with no refresh period.
    edit(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-schema-refresh").as_deref(),
            Some("Manual")
        );
        assert!(window.try_find("connection-refresh-period").is_none());
        let refresh = bounds_of(window, "connection-schema-refresh");
        let show = bounds_of(window, "connection-show-schemas");
        assert!(refresh.origin.y < show.origin.y);
    });
    app.scroll_to(cx, "connection-refresh-timeout");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-refresh-timeout").as_deref(),
            Some("30")
        );
    });

    // While connected shows the period, and both fields are validated.
    app.scroll_to(cx, "connection-schema-refresh");
    app.select(cx, "connection-schema-refresh", "While connected");
    app.wait_for(cx, "connection-refresh-period");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-refresh-period").as_deref(),
            Some("60")
        );
    });
    app.fill(cx, "connection-refresh-period", "4");
    app.click(cx, "save-profile");
    wait_error(cx, "Refresh period must be between 5 and 10080 minutes.");
    app.fill(cx, "connection-refresh-period", "15");
    // The error above the footer makes the form shorter, so the timeout
    // field needs a scroll to the end of the form.
    scroll_form_to_end(&app, cx);
    app.fill(cx, "connection-refresh-timeout", "0");
    app.click(cx, "save-profile");
    wait_error(cx, "Refresh timeout must be between 1 and 1440 minutes.");
    scroll_form_to_end(&app, cx);
    app.fill(cx, "connection-refresh-timeout", "45");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the saved policy", Duration::from_secs(10), |_, _| {
        let saved = saved(&app);
        saved.refresh == CatalogRefresh::WhileConnected
            && saved.refresh_minutes == 15
            && saved.timeout_minutes == 45
    });

    // Disabled hides the other Schemas fields and keeps their values.
    edit(cx);
    app.select(cx, "connection-schema-refresh", "Disabled");
    for hidden in [
        "connection-refresh-period",
        "connection-show-schemas",
        "connection-hide-schemas",
        "connection-refresh-timeout",
    ] {
        app.wait_gone(cx, hidden);
    }
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(
        cx,
        "the disabled policy",
        Duration::from_secs(10),
        |_, _| {
            let saved = saved(&app);
            saved.refresh == CatalogRefresh::Disabled
                && saved.refresh_minutes == 15
                && saved.timeout_minutes == 45
        },
    );

    // The hidden period stays for a change back to While connected.
    edit(cx);
    app.select(cx, "connection-schema-refresh", "While connected");
    app.wait_for(cx, "connection-refresh-period");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-refresh-period").as_deref(),
            Some("15")
        );
    });
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "connection-name");

    // A new connection does not browse schemas.
    app.click(cx, "add-connection");
    app.wait_for(cx, "connection-name");
    app.scroll_to(cx, "connection-schema-refresh");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-schema-refresh").as_deref(),
            Some("Disabled")
        );
        assert!(window.try_find("connection-show-schemas").is_none());
    });
}

#[gpui_kit::test]
fn turning_schema_browsing_off_and_on_shows_the_cache_again(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let profile = offline_profile("Warehouse");
    cache(&directory, &profile, &[("avia", None)]);
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_in(cx, directory, workspace(vec![profile.clone()]), credentials);
    let set_mode = |cx: &mut TestAppContext, mode: &str| {
        app.context_menu(cx, connection_row(profile.id));
        app.choose(cx, "popup-menu", "Edit");
        app.scroll_to(cx, "connection-schema-refresh");
        app.select(cx, "connection-schema-refresh", mode);
        app.click(cx, "save-profile");
        app.wait_gone(cx, "connection-name");
    };
    app.toggle_connection(cx, profile.id);
    app.wait_until(
        cx,
        "the cached schema",
        Duration::from_secs(10),
        |window, _| labelled(window, "avia").is_some(),
    );

    set_mode(cx, "Disabled");
    app.wait_until(cx, "no tree", Duration::from_secs(10), |window, _| {
        labelled(window, "avia").is_none()
    });

    // Browsing starts collapsed and reads the cache when expanded.
    set_mode(cx, "Manual");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(labelled(window, "avia").is_none());
        assert!(!shows(window, "Loading…"));
    });
    app.toggle_connection(cx, profile.id);
    app.wait_until(
        cx,
        "the cache again",
        Duration::from_secs(10),
        |window, _| labelled(window, "avia").is_some(),
    );
}

#[gpui_kit::test]
fn a_connection_without_schema_browsing_has_no_tree_and_no_refresh(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = offline_profile("Warehouse");
    // The cache stays from a time when the connection browsed schemas.
    cache(&directory, &profile, &[("avia", None)]);
    profile.catalog.refresh = CatalogRefresh::Disabled;
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![profile.clone()]),
        MemoryCredentials::default(),
    );
    app.toggle_connection(cx, profile.id);
    app.settle(cx);
    app.update(cx, |window, _| assert!(labelled(window, "avia").is_none()));
    app.context_menu(cx, connection_row(profile.id));
    app.update(cx, |window, _| {
        assert!(menu_item(window, "popup-menu", "Edit").is_some());
        // Without schema browsing, the menu has no Schemas section.
        assert!(window.try_find("menu-section-Schemas").is_none());
        for item in ["Refresh", "Collapse"] {
            assert!(menu_item(window, "popup-menu", item).is_none(), "{item}");
        }
    });
}

#[gpui_kit::test]
fn an_open_connection_tooltip_shows_a_refresh_error_when_it_arrives(cx: &mut TestAppContext) {
    // The server accepts the session, then closes it after a second, so the
    // refresh fails while the tooltip is open.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            std::thread::sleep(Duration::from_secs(1));
            drop(stream);
        }
    });
    let profile = Profile {
        host: "127.0.0.1".into(),
        port,
        ..offline_profile("Closing")
    };
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_with(cx, workspace(vec![profile.clone()]), credentials);
    let tooltip = |app: &TestApp, cx: &mut TestAppContext| {
        app.update(cx, |window, _| label(window, "catalog-tooltip"))
    };

    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Refresh");
    app.hover_labelled(cx, "Closing, refreshing schemas");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    assert_eq!(tooltip(&app, cx).as_deref(), Some("127.0.0.1 · synthetic"));

    // The pointer stays on the row. The open tooltip adds the error.
    app.wait_until(
        cx,
        "the error in the tooltip",
        Duration::from_secs(20),
        |window, _| {
            label(window, "catalog-tooltip")
                .is_some_and(|text| text.starts_with("127.0.0.1 · synthetic\n"))
        },
    );
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, connection_row(profile.id)).as_deref(),
            Some("Closing, schema refresh error")
        );
    });
}

#[gpui_kit::test]
fn connections_share_a_catalog_and_its_cache_follows_them(cx: &mut TestAppContext) {
    let first = offline_profile("Small");
    let second = offline_profile("Large");
    let directory = tempfile::tempdir().unwrap();
    avia(&directory, &first);
    let workspace_file = directory.path().join("workspace.json");
    let private = storage::catalog_path(&workspace_file, first.id);
    let credentials = MemoryCredentials::default();
    for profile in [&first, &second] {
        credentials
            .set_password(profile.id, "synthetic-password")
            .unwrap();
    }
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace(vec![first.clone(), second.clone()]),
        credentials,
    );
    let edit = |cx: &mut TestAppContext, profile: &Profile| {
        app.context_menu(cx, connection_row(profile.id));
        app.choose(cx, "popup-menu", "Edit");
        app.scroll_to(cx, "connection-schema-catalog");
    };

    // The first connection makes a shared catalog, which takes its cache.
    edit(cx, &first);
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-schema-catalog").as_deref(),
            Some("This connection")
        );
        assert!(window.try_find("connection-shared-catalog-name").is_none());
    });
    // The command below the catalog list makes a new shared catalog.
    app.click(cx, "connection-schema-catalog");
    app.wait_for(cx, "connection-new-shared-catalog");
    app.click(cx, "connection-new-shared-catalog");
    app.wait_for(cx, "connection-shared-catalog-name");
    app.wait_gone(cx, "connection-new-shared-catalog");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-schema-catalog").as_deref(),
            Some("New shared catalog")
        );
    });
    app.wait_until(
        cx,
        "the name focus",
        Duration::from_secs(10),
        |window, _| window.find("connection-shared-catalog-name").focused() == Some(true),
    );
    // One save makes at most one shared catalog, so the command goes away.
    app.click(cx, "connection-schema-catalog");
    app.wait_until(cx, "the open list", Duration::from_secs(10), |window, _| {
        labelled(window, "Search catalogs…").is_some()
    });
    app.update(cx, |window, _| {
        assert!(window.try_find("connection-new-shared-catalog").is_none());
    });
    app.press(cx, "escape");
    app.wait_until(
        cx,
        "the closed list",
        Duration::from_secs(10),
        |window, _| labelled(window, "Search catalogs…").is_none(),
    );
    // Another choice drops the new shared catalog, and the command comes back.
    choose_catalog(&app, cx, "This connection");
    app.wait_gone(cx, "connection-shared-catalog-name");
    app.click(cx, "connection-schema-catalog");
    app.wait_for(cx, "connection-new-shared-catalog");
    app.click(cx, "connection-new-shared-catalog");
    app.wait_for(cx, "connection-shared-catalog-name");
    app.wait_gone(cx, "connection-new-shared-catalog");
    app.wait_for(cx, "connection-name");
    app.click(cx, "save-profile");
    app.wait_until(
        cx,
        "the name error",
        Duration::from_secs(10),
        |window, _| {
            label(window, "connection-form-error-accessibility").as_deref()
                == Some("Give the shared catalog a name.")
        },
    );
    app.scroll_to(cx, "connection-shared-catalog-name");
    app.fill(cx, "connection-shared-catalog-name", "Lake");
    app.scroll_to(cx, "connection-hide-schemas");
    app.fill(cx, "connection-hide-schemas", "scratch");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the shared catalog", Duration::from_secs(10), |_, _| {
        let saved = app.saved();
        saved.shared_catalogs.len() == 1
            && saved.profiles[0].shared_catalog == Some(saved.shared_catalogs[0].id)
    });
    let lake = app.saved().shared_catalogs[0].clone();
    assert_eq!(lake.name, "Lake");
    assert_eq!(lake.settings.exclude, ["scratch"]);
    let shared = storage::catalog_path(&workspace_file, lake.id);
    app.wait_until(cx, "the moved cache", Duration::from_secs(10), |_, _| {
        shared.exists() && !private.exists()
    });

    // The second connection joins it and shows the same schemas.
    edit(cx, &second);
    choose_catalog(&app, cx, "Lake");
    app.wait_for(cx, "connection-shared-catalog-name");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-shared-catalog-name").as_deref(),
            Some("Lake")
        );
    });
    app.scroll_to(cx, "connection-hide-schemas");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-hide-schemas").as_deref(),
            Some("scratch")
        );
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the second member", Duration::from_secs(10), |_, _| {
        app.saved().profiles[1].shared_catalog == Some(lake.id)
    });
    expand_connection(&app, cx, &second);
    wait_shows(&app, cx, "avia");
    assert_eq!(app.credentials.reads(), 0, "The tree must not connect");

    // The catalog stays while one connection uses it.
    edit(cx, &first);
    choose_catalog(&app, cx, "This connection");
    app.wait_gone(cx, "connection-shared-catalog-name");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the first to leave", Duration::from_secs(10), |_, _| {
        let saved = app.saved();
        saved.profiles[0].shared_catalog.is_none() && saved.shared_catalogs.len() == 1
    });
    assert!(shared.exists());

    // The last connection that leaves deletes the catalog and its cache.
    edit(cx, &second);
    choose_catalog(&app, cx, "This connection");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "no shared catalog", Duration::from_secs(10), |_, _| {
        app.saved().shared_catalogs.is_empty()
    });
    app.wait_until(cx, "the deleted cache", Duration::from_secs(10), |_, _| {
        !shared.exists()
    });
}

#[gpui_kit::test]
fn saving_a_member_keeps_the_shared_preference_and_hidden_period(cx: &mut TestAppContext) {
    let lake = SharedCatalog {
        id: uuid::Uuid::new_v4(),
        name: "Lake".into(),
        settings: CatalogSettings {
            refresh: CatalogRefresh::Manual,
            refresh_minutes: 15,
            ..CatalogSettings::default()
        },
        preferred: None,
    };
    let member = |name: &str| Profile {
        shared_catalog: Some(lake.id),
        ..offline_profile(name)
    };
    let edited = member("Edited");
    // A name like the first choice must not stand for it.
    let named_like_any = member("Any connected connection");
    let mut off = member("Off");
    off.catalog.refresh = CatalogRefresh::Disabled;
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(edited.id, "synthetic-password")
        .unwrap();
    let mut saved = workspace(vec![named_like_any.clone(), off.clone(), edited.clone()]);
    saved.shared_catalogs = vec![SharedCatalog {
        preferred: Some(off.id),
        ..lake.clone()
    }];
    let app = TestApp::launch_with(cx, saved, credentials);
    let edit = |cx: &mut TestAppContext| {
        app.context_menu(cx, connection_row(edited.id));
        app.choose(cx, "popup-menu", "Edit");
        app.scroll_to(cx, "connection-preferred-catalog-connection");
    };
    let preferred = |app: &TestApp| app.saved().shared_catalogs[0].preferred;

    // A preferred member with browsing off stays preferred, and the hidden
    // period of a manual refresh stays with the shared catalog.
    edit(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "connection-preferred-catalog-connection").as_deref(),
            Some("Off")
        );
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.settle(cx);
    let catalog = app.saved().shared_catalogs[0].clone();
    assert_eq!(catalog.preferred, Some(off.id));
    assert_eq!(catalog.settings.refresh_minutes, 15);

    // The member whose name is the label of the first choice is chosen by
    // its row.
    edit(cx);
    app.select(
        cx,
        "connection-preferred-catalog-connection",
        "Any connected connection",
    );
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "no preference", Duration::from_secs(10), |_, _| {
        preferred(&app).is_none()
    });
    let rows = |cx: &mut TestAppContext| {
        edit(cx);
        app.update(cx, |window, cx| {
            window
                .within("connection-preferred-catalog-connection".to_owned())
                .click("input", cx)
        });
        app.settle(cx);
    };
    rows(cx);
    // The second row has the same label as the first.
    app.press(cx, "down");
    app.press(cx, "enter");
    app.settle(cx);
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the member", Duration::from_secs(10), |_, _| {
        preferred(&app) == Some(named_like_any.id)
    });
    edit(cx);
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.settle(cx);
    assert_eq!(preferred(&app), Some(named_like_any.id));
}

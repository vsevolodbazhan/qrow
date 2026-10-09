use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{
    TestApp, assert_catalog_error_dot, assert_catalog_icon, assert_connection_highlight, bounds_of,
    connection_row, labelled, selected_tree_rows,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, TestAppContext};
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind, Unfinished},
    model::{CatalogColumnReads, CatalogRefresh, CatalogSettings, SharedCatalog},
    storage,
};
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn schema_and_relation_error_dots_open_their_connection_activity(cx: &mut TestAppContext) {
    for relation in [false, true] {
        let (mut workspace, credentials) = Kyuubi::get().workspace("SELECT 1", "not-the-password");
        workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
        let profile = workspace.profiles[0].clone();
        let directory = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::new(&profile);
        catalog.apply_schemas(vec!["finance".into()], &CatalogSettings::default(), 1);
        catalog.apply_relations(
            "finance",
            None,
            vec![RelationEntry {
                name: "daily".into(),
                kind: RelationKind::Table,
                comment: None,
            }],
            1,
        );
        if relation {
            catalog.apply_columns(
                "finance",
                Some("daily"),
                std::collections::BTreeMap::from([(
                    "daily".into(),
                    vec![CatalogColumn {
                        name: "cached_id".into(),
                        data_type: "BIGINT".into(),
                        comment: None,
                    }],
                )]),
                1,
            );
        }
        storage::save_catalog(
            &storage::catalog_path(&directory.path().join("workspace.json"), profile.id),
            &catalog,
        )
        .unwrap();
        let app = TestApp::launch_in(cx, directory, workspace, credentials);
        let schema = format!("s\u{1f}{}\u{1f}finance", profile.id);
        let parent = if relation {
            format!("r\u{1f}{}\u{1f}finance\u{1f}daily", profile.id)
        } else {
            schema.clone()
        };
        app.toggle_connection(cx, profile.id);
        app.wait_for(cx, format!("{schema}\u{1f}label"));
        app.click(cx, format!("{schema}\u{1f}label"));
        if relation {
            app.wait_for(cx, format!("{parent}\u{1f}label"));
            app.click(cx, format!("{parent}\u{1f}label"));
        }
        app.context_menu(cx, format!("{parent}\u{1f}label"));
        app.choose(cx, "popup-menu", "Refresh");
        let status = format!("{parent}\u{1f}error-icon");
        app.wait_for(cx, status.clone());
        app.update(cx, |window, cx| {
            assert_catalog_error_dot(window, &status, cx.theme().danger);
        });
        app.click(cx, status.clone());
        app.wait_for(cx, "activity");
        let activity = app.copy_activity(cx);
        assert!(activity.contains("Schema refresh failed"), "{activity}");
        assert!(
            activity.contains("rejected SASL PLAIN authentication"),
            "{activity}"
        );
        assert!(activity.contains("finance"), "{activity}");
        if relation {
            assert!(activity.contains("daily"), "{activity}");
        }
        app.press(cx, "escape");
        app.wait_gone(cx, "activity");
        app.update(cx, |window, cx| {
            assert_catalog_error_dot(window, &status, cx.theme().danger);
            assert!(
                window
                    .try_find(format!("{parent}\u{1f}error\u{1f}refresh"))
                    .is_none()
            );
            assert!(
                window
                    .try_find(format!("{parent}\u{1f}notice\u{1f}label"))
                    .is_none()
            );
            assert!(
                window
                    .find(format!(
                        "r\u{1f}{}\u{1f}finance\u{1f}daily\u{1f}label",
                        profile.id
                    ))
                    .visible()
            );
            if relation {
                assert!(
                    window
                        .find(format!("{parent}\u{1f}0\u{1f}cached_id\u{1f}label"))
                        .visible()
                );
            }
        });
        // Failed scopes remain refreshable from the parent's menu.
        app.context_menu(cx, format!("{parent}\u{1f}label"));
        app.choose(cx, "popup-menu", "Refresh");
        app.wait_until(
            cx,
            "the second refresh attempt",
            QUERY_TIMEOUT,
            |window, _| {
                app.credentials.reads() == 2
                    && crate::support::label(window, "toggle-activity").as_deref()
                        == Some("Activity, 1 unseen error")
            },
        );
        let activity = app.activity(cx, profile.id);
        assert_eq!(
            activity.matches("Started a schema refresh").count(),
            2,
            "{activity}"
        );
        assert_eq!(
            activity.matches("Schema refresh failed").count(),
            2,
            "{activity}"
        );
        app.update(cx, |window, _| window.remove_window());
        cx.run_until_parked();
    }
}

/// Delay authentication of the refresh session while query sessions use the
/// real server normally. This makes the refresh cross the tab's idle deadline.
struct SlowRefresh {
    port: u16,
    stopped: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    acceptor: Option<thread::JoinHandle<()>>,
}

impl SlowRefresh {
    fn new(server_port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let stop = stopped.clone();
        let release = released.clone();
        let sessions = active.clone();
        let acceptor = thread::spawn(move || {
            let mut connections = 0;
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut client, _)) => {
                        client.set_nonblocking(false).unwrap();
                        connections += 1;
                        let delay = connections == 2;
                        let release = release.clone();
                        sessions.fetch_add(1, Ordering::SeqCst);
                        let sessions = sessions.clone();
                        let mut server = TcpStream::connect(("127.0.0.1", server_port)).unwrap();
                        let mut upstream_client = client.try_clone().unwrap();
                        let mut upstream_server = server.try_clone().unwrap();
                        thread::spawn(move || {
                            let _ = std::io::copy(&mut upstream_client, &mut upstream_server);
                            let _ = upstream_server.shutdown(Shutdown::Both);
                        });
                        thread::spawn(move || {
                            let mut first = [0; 4096];
                            if let Ok(count) = server.read(&mut first) {
                                if delay {
                                    while !release.load(Ordering::SeqCst) {
                                        thread::sleep(Duration::from_millis(10));
                                    }
                                }
                                if client.write_all(&first[..count]).is_ok() {
                                    let _ = std::io::copy(&mut server, &mut client);
                                }
                            }
                            let _ = client.shutdown(Shutdown::Both);
                            sessions.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("Refresh proxy failed: {error}"),
                }
            }
        });
        Self {
            port,
            stopped,
            released,
            active,
            acceptor: Some(acceptor),
        }
    }
}

impl Drop for SlowRefresh {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.released.store(true, Ordering::SeqCst);
        self.acceptor.take().unwrap().join().unwrap();
    }
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn an_automatic_refresh_finishes_before_an_overdue_idle_disconnect(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    let proxy = SlowRefresh::new(workspace.profiles[0].port);
    let profile = &mut workspace.profiles[0];
    profile.port = proxy.port;
    profile.lifecycle.idle_seconds = 1;
    profile.catalog.refresh = CatalogRefresh::WhileConnected;
    profile.catalog.include = vec!["default".into()];
    let id = profile.id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    assert!(
        !app.activity(cx, id)
            .contains("Started an automatic schema refresh")
    );
    assert_eq!(proxy.active.load(Ordering::SeqCst), 0);
    app.run_complete(cx, "SELECT 1");
    app.wait_cell(cx, 0, 1, "1");
    let started = Instant::now();
    app.wait_until(
        cx,
        "the deferred idle deadline",
        Duration::from_secs(3),
        |window, _| {
            assert!(
                !crate::support::label(window, "query-status")
                    .is_some_and(|status| status.starts_with("Disconnected"))
            );
            started.elapsed() >= Duration::from_millis(1200)
        },
    );
    proxy.released.store(true, Ordering::SeqCst);
    app.wait_status(cx, "Disconnected: Idle timeout");
    app.wait_cell(cx, 0, 1, "1");
    let activity = app.activity(cx, id);
    assert!(activity.contains("Schema refresh completed"), "{activity}");
    assert!(
        activity.contains("Disconnected after idle timeout"),
        "{activity}"
    );
    assert!(!activity.contains("Schema refresh stopped"), "{activity}");
    assert_eq!(
        activity
            .matches("Started an automatic schema refresh")
            .count(),
        1,
        "{activity}"
    );
    app.wait_until(
        cx,
        "both sessions to close",
        Duration::from_secs(5),
        |_, _| proxy.active.load(Ordering::SeqCst) == 0,
    );

    // Explicit Refresh can connect after the tab has disconnected. It leaves
    // the query tab disconnected and closes its own temporary session.
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Refresh");
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut activity = String::new();
    while Instant::now() < deadline {
        activity = app.activity(cx, id);
        if activity.matches("Schema refresh completed").count() == 2 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        activity.matches("Schema refresh completed").count(),
        2,
        "{activity}"
    );
    app.wait_status(cx, "Disconnected: Idle timeout");
    app.wait_until(
        cx,
        "the explicit refresh session to close",
        Duration::from_secs(5),
        |_, _| proxy.active.load(Ordering::SeqCst) == 0,
    );
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn qualified_search_finds_a_live_table_and_inserts_its_name(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_search_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    let profile = workspace.profiles[0].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id INT) USING parquet"),
    );
    app.run_complete(cx, &format!("CREATE VIEW {schema}.daily AS SELECT 1 AS id"));
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    let connection_left = app.update(cx, |window, _| {
        bounds_of(window, &format!("c\u{1f}{}\u{1f}label", profile.id)).left()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the catalog to load", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some() && labelled(window, "daily").is_some()
    });
    app.toggle_connection(cx, profile.id);
    app.wait_until(
        cx,
        "the connection to collapse",
        QUERY_TIMEOUT,
        |window, _| labelled(window, &schema).is_none(),
    );

    app.fill_labelled(cx, "Search tables", &format!("{schema}.bookings"));
    app.wait_until(cx, "the qualified match", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some() && labelled(window, "daily").is_none()
    });
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Copy qualified name");
    let name = cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(name, format!("`{schema}`.`bookings`"));
    app.fill_labelled(cx, "Search tables", &name);
    app.wait_until(
        cx,
        "the copied name to match",
        QUERY_TIMEOUT,
        |window, _| labelled(window, "bookings").is_some() && labelled(window, "daily").is_none(),
    );

    app.fill_labelled(cx, "Search tables", &format!("{schema}.missing"));
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the empty search notice", QUERY_TIMEOUT, |window, _| {
        labelled(window, "No matches").is_some() && labelled(window, "bookings").is_none()
    });
    app.update(cx, |window, _| {
        let notice = bounds_of(
            window,
            &format!("c\u{1f}{}\u{1f}notice\u{1f}label", profile.id),
        );
        assert_eq!(notice.left(), connection_left);
    });
    app.toggle_connection(cx, profile.id);
    app.wait_until(
        cx,
        "the connection to collapse",
        QUERY_TIMEOUT,
        |window, _| labelled(window, "No matches").is_none(),
    );
    app.fill_labelled(cx, "Search tables", &name);
    app.wait_until(cx, "the table to return", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some() && labelled(window, "No matches").is_none()
    });

    app.type_sql(cx, "SELECT COUNT(*) FROM ");
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Insert into editor");
    let sql = format!("SELECT COUNT(*) FROM `{schema}`.`bookings`");
    app.wait_until(cx, "the inserted name", QUERY_TIMEOUT, |_, _| {
        app.saved().tabs[0].sql == sql
    });
    app.run_complete(cx, &sql);
    app.wait_cell(cx, 0, 1, "0");
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn an_inserted_reserved_table_name_runs_with_ansi_keywords(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_keyword_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    let profile = workspace.profiles[0].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE VIEW {schema}.`select` AS SELECT 42 AS `from`"),
    );
    app.run_complete(cx, "SET spark.sql.ansi.enabled=true");
    app.run_complete(cx, "SET spark.sql.ansi.enforceReservedKeywords=true");
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the keyword schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the keyword table", QUERY_TIMEOUT, |window, _| {
        labelled(window, "select").is_some()
    });
    app.type_sql(cx, "SELECT `from` FROM ");
    app.context_menu_labelled(cx, "select");
    app.choose(cx, "popup-menu", "Insert into editor");
    let sql = format!("SELECT `from` FROM `{schema}`.`select`");
    app.wait_until(cx, "the quoted table name", QUERY_TIMEOUT, |_, _| {
        app.saved().tabs[0].sql == sql
    });
    app.run_complete(cx, &sql);
    app.wait_cell(cx, 0, 1, "42");
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_live_session_loads_the_tree_and_a_refresh_shows_a_new_column(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_tree_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) = kyuubi.connections(&["Writer", "Reader"], "SELECT 1");
    // Other tests make schemas too. The filter keeps the tree to this one.
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    // An automatic refresh would read the catalog before the schema exists.
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    let profile = workspace.profiles[0].clone();
    workspace.profiles[1].catalog.refresh = CatalogRefresh::Manual;
    let reader = workspace.profiles[1].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    // The tab has a live session, so expansion reads the catalog by itself.
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the table", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some()
    });
    app.click_labelled(cx, "bookings");
    app.wait_until(cx, "the columns", QUERY_TIMEOUT, |window, _| {
        labelled(window, "gate string").is_some()
    });
    app.update(cx, |window, _| {
        let relation = format!("r\u{1f}{}\u{1f}{schema}\u{1f}bookings", profile.id);
        for (index, name) in ["id", "gate"].iter().enumerate() {
            assert_catalog_icon(window, &format!("{relation}\u{1f}{index}\u{1f}{name}"));
        }
    });

    app.run_complete(
        cx,
        &format!("ALTER TABLE {schema}.bookings ADD COLUMNS (fare DOUBLE)"),
    );
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Refresh");
    app.wait_until(cx, "the new column", QUERY_TIMEOUT, |window, _| {
        labelled(window, "fare double").is_some()
    });
    app.update(cx, |window, _| {
        assert_catalog_icon(
            window,
            &format!(
                "r\u{1f}{}\u{1f}{schema}\u{1f}bookings\u{1f}2\u{1f}fare",
                profile.id
            ),
        );
    });

    // A different live query connection must remain the only highlighted root.
    app.select_connection(cx, &reader);
    app.run_complete(cx, "SELECT 1");
    app.click_labelled(cx, "id bigint");
    app.update(cx, |window, _| {
        assert_eq!(selected_tree_rows(window), ["id bigint"]);
    });
    for _ in 0..2 {
        app.toggle_connection(cx, profile.id);
        app.settle(cx);
        app.update(cx, |window, cx| {
            assert!(
                !selected_tree_rows(window)
                    .iter()
                    .any(|row| row == "id bigint")
            );
            assert_connection_highlight(window, cx, reader.id, true);
            assert_connection_highlight(window, cx, profile.id, false);
        });
    }
    app.click_labelled(cx, "id bigint");
    app.press(cx, "cmd-b");
    app.wait_gone(cx, "add-connection");
    app.press(cx, "cmd-b");
    app.wait_for(cx, "add-connection");
    app.click(cx, "connections-list");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(selected_tree_rows(window).is_empty())
    });

    app.select_connection(cx, &profile);
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn both_column_read_modes_refresh_tables_and_views(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_columns_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    let profile = workspace.profiles[0].clone();
    assert_eq!(profile.catalog_column_reads, CatalogColumnReads::Table);
    let directory = tempfile::tempdir().unwrap();
    let catalog_path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    let app = TestApp::launch_in(cx, directory, workspace, credentials);
    let scroll_tree = |cx: &mut TestAppContext, pixels: f32| {
        app.update(cx, |window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: window.find("connections-list").bounds().center(),
                    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(pixels),
                    )),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        app.settle(cx);
    };
    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    let fields = (0..32)
        .map(|ix| format!("c{ix} INT"))
        .collect::<Vec<_>>()
        .join(", ");
    for table in ["first", "second", "third", "fourth"] {
        app.run_complete(
            cx,
            &format!("CREATE TABLE {schema}.{table} ({fields}) USING parquet"),
        );
    }
    app.run_complete(
        cx,
        &format!("CREATE VIEW {schema}.recent AS SELECT c0 AS view_id FROM {schema}.first"),
    );

    for (mode, label, added) in [
        (
            CatalogColumnReads::Table,
            "One relation at a time",
            "table_read",
        ),
        (CatalogColumnReads::Schema, "Whole schema", "schema_read"),
    ] {
        app.context_menu(cx, connection_row(profile.id));
        app.choose(cx, "popup-menu", "Edit");
        app.wait_for(cx, "connection-name");
        app.connection_page(cx, "Catalog");
        app.scroll_to(cx, "connection-column-reads");
        app.select(cx, "connection-column-reads", label);
        app.click(cx, "save-profile");
        app.wait_gone(cx, "save-profile");
        app.wait_until(
            cx,
            "the column-read choice to save",
            QUERY_TIMEOUT,
            |_, _| app.saved().profiles[0].catalog_column_reads == mode,
        );
        app.run_complete(
            cx,
            &format!("ALTER TABLE {schema}.fourth ADD COLUMNS ({added} STRING)"),
        );
        app.context_menu(cx, connection_row(profile.id));
        app.choose(cx, "popup-menu", "Refresh");
        app.wait_until(
            cx,
            "all table and view columns to refresh",
            QUERY_TIMEOUT,
            |_, _| {
                storage::load_catalog(&catalog_path).is_some_and(|catalog| {
                    catalog.error.is_none()
                        && ["first", "second", "third", "fourth", "recent"]
                            .iter()
                            .all(|name| {
                                catalog.relation(&schema, name).is_some_and(|relation| {
                                    relation.error.is_none()
                                        && relation.columns.as_ref().is_some_and(|columns| {
                                            if *name == "recent" {
                                                columns.len() == 1 && columns[0].name == "view_id"
                                            } else if *name == "fourth" {
                                                columns.iter().any(|column| column.name == added)
                                            } else {
                                                columns.len() == 32
                                            }
                                        })
                                })
                            })
                })
            },
        );
        app.fill_labelled(cx, "Search tables", &format!("{schema}.fourth"));
        app.wait_until(cx, "the refreshed table", QUERY_TIMEOUT, |window, _| {
            labelled(window, "fourth").is_some()
        });
        app.click_labelled(cx, "fourth");
        app.wait_until(cx, "the table to expand", QUERY_TIMEOUT, |window, _| {
            labelled(window, "c0 int").is_some()
        });
        scroll_tree(cx, -20_000.);
        app.wait_until(
            cx,
            "the refreshed column in the tree",
            QUERY_TIMEOUT,
            |window, _| labelled(window, &format!("{added} string")).is_some(),
        );
        scroll_tree(cx, 20_000.);
        app.click_labelled(cx, "fourth");
        app.wait_until(cx, "the table to collapse", QUERY_TIMEOUT, |window, _| {
            labelled(window, "c0 int").is_none()
        });
        app.fill_labelled(cx, "Search tables", "");
    }
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn the_first_run_on_a_stale_connection_fills_its_tree(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_auto_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) = kyuubi.connections(&["Writer", "Reader"], "SELECT 1");
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    // Other tests make schemas too. The filter keeps the tree to this one.
    workspace.profiles[1].catalog.include = vec![schema.clone()];
    workspace.profiles[1].catalog.refresh = CatalogRefresh::WhileConnected;
    let reader = workspace.profiles[1].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    // The reader never read its catalog. Its first live session reads it,
    // without an expansion or a Refresh.
    app.select_connection(cx, &reader);
    app.run_complete(cx, "SELECT 1");
    let mut activity = String::new();
    let deadline = std::time::Instant::now() + QUERY_TIMEOUT;
    while std::time::Instant::now() < deadline {
        activity = app.activity(cx, reader.id);
        if activity.contains("Schema refresh completed") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(
        activity.contains("Started an automatic schema refresh of the connection"),
        "{activity}"
    );
    assert!(activity.contains("Schema refresh completed"), "{activity}");
    // Refreshes stay out of the tab Logs.
    assert!(!app.logs(cx).contains("schema refresh"));

    // The tree shows the cached schema, table, and columns.
    app.toggle_connection(cx, reader.id);
    app.wait_until(cx, "the schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.click_labelled(cx, "bookings");
    app.wait_until(cx, "the columns", QUERY_TIMEOUT, |window, _| {
        labelled(window, "gate string").is_some()
    });
    // Expansion did not read the catalog again.
    let activity = app.activity(cx, reader.id);
    assert_eq!(
        activity.matches("schema refresh of").count(),
        1,
        "{activity}"
    );

    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_live_shared_connection_resumes_an_unfinished_cache_before_its_period(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let done = format!("qrow_done_{suffix}");
    let pending = format!("qrow_pending_{suffix}");
    let (mut workspace, credentials) = kyuubi.connections(&["Writer", "Reader"], "SELECT 1");
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Disabled;
    let settings = CatalogSettings {
        refresh: CatalogRefresh::WhileConnected,
        refresh_minutes: 720,
        include: vec![done.clone(), pending.clone()],
        ..CatalogSettings::default()
    };
    let shared = SharedCatalog {
        id: uuid::Uuid::new_v4(),
        name: "Warehouse".into(),
        settings: settings.clone(),
        preferred: None,
    };
    workspace.profiles[1].catalog.refresh = CatalogRefresh::WhileConnected;
    workspace.profiles[1].shared_catalog = Some(shared.id);
    let reader = workspace.profiles[1].clone();
    workspace.shared_catalogs.push(shared.clone());

    let directory = tempfile::tempdir().unwrap();
    let path = storage::catalog_path(&directory.path().join("workspace.json"), shared.id);
    let at = qrow::catalog::now();
    let mut catalog = Catalog::empty(shared.id, None);
    catalog.apply_schemas(vec![done.clone(), pending.clone()], &settings, at);
    // The latest schema list is fresh, but the second schema was not read.
    catalog.apply_relations(
        &done,
        None,
        vec![RelationEntry {
            name: "already_cached".into(),
            kind: RelationKind::Table,
            comment: None,
        }],
        at,
    );
    catalog.unfinished = Some(Unfinished {
        started: at,
        done: [done.clone()].into(),
    });
    storage::save_catalog(&path, &catalog).unwrap();
    let app = TestApp::launch_in(cx, directory, workspace, credentials);
    app.run_complete(cx, &format!("CREATE DATABASE {done}"));
    app.run_complete(cx, &format!("CREATE DATABASE {pending}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {pending}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    app.select_connection(cx, &reader);
    app.run_complete(cx, "SELECT 1");
    app.wait_until(cx, "the completed shared cache", QUERY_TIMEOUT, |_, _| {
        storage::load_catalog(&path).is_some_and(|catalog| {
            catalog.unfinished.is_none()
                && catalog
                    .relation(&pending, "bookings")
                    .is_some_and(|relation| {
                        relation.columns.as_ref().is_some_and(|columns| {
                            columns.iter().any(|column| column.name == "gate")
                        })
                    })
        })
    });
    let activity = app.activity(cx, reader.id);
    assert!(
        activity.contains("Started an automatic schema refresh"),
        "{activity}"
    );
    assert!(
        activity.contains("Continued a stopped schema refresh"),
        "{activity}"
    );
    assert!(activity.contains("Schema refresh completed"), "{activity}");
    app.toggle_connection(cx, reader.id);
    app.click_labelled(cx, &pending);
    app.click_labelled(cx, "bookings");
    app.wait_until(cx, "the resumed columns", QUERY_TIMEOUT, |window, _| {
        labelled(window, "gate string").is_some()
    });
    // Schemas already completed in this period are preserved.
    assert!(
        storage::load_catalog(&path)
            .unwrap()
            .relation(&done, "already_cached")
            .is_some()
    );
    app.run_complete(cx, &format!("DROP DATABASE {done} CASCADE"));
    app.run_complete(cx, &format!("DROP DATABASE {pending} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_refresh_of_one_connection_fills_the_shared_tree_of_another(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_shared_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) = kyuubi.connections(&["Writer", "Reader"], "SELECT 1");
    // Other tests make schemas too. The filter keeps the tree to this one.
    let lake = SharedCatalog {
        id: uuid::Uuid::new_v4(),
        name: "Lake".into(),
        settings: CatalogSettings {
            refresh: CatalogRefresh::Manual,
            include: vec![schema.clone()],
            ..CatalogSettings::default()
        },
        preferred: None,
    };
    for profile in &mut workspace.profiles {
        profile.catalog.refresh = CatalogRefresh::Manual;
        profile.shared_catalog = Some(lake.id);
    }
    workspace.shared_catalogs = vec![lake];
    let (writer, reader) = (workspace.profiles[0].clone(), workspace.profiles[1].clone());
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    // The writer has a live session, so its expansion reads the catalog.
    app.toggle_connection(cx, writer.id);
    app.wait_until(cx, "the schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.toggle_connection(cx, writer.id);
    app.wait_until(cx, "the writer to collapse", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_none()
    });

    // The reader has no session, and its tree shows the same catalog.
    app.toggle_connection(cx, reader.id);
    app.wait_until(cx, "the shared schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.click_labelled(cx, "bookings");
    app.wait_until(cx, "the shared columns", QUERY_TIMEOUT, |window, _| {
        labelled(window, "gate string").is_some()
    });

    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_connected_member_transfers_its_fresh_cache_before_an_automatic_refresh(
    cx: &mut TestAppContext,
) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_seed_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    let profile = workspace.profiles[0].clone();
    let directory = tempfile::tempdir().unwrap();
    let workspace_file = directory.path().join("workspace.json");
    let private = storage::catalog_path(&workspace_file, profile.id);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // This recent cache contains data that the server no longer has. A
    // premature automatic refresh would remove it before the transfer.
    let mut catalog = Catalog::new(&profile);
    catalog.apply_schemas(vec![schema.clone()], &profile.catalog, at);
    catalog.apply_relations(
        &schema,
        None,
        vec![RelationEntry {
            name: "cached_orders".into(),
            kind: RelationKind::Table,
            comment: None,
        }],
        at,
    );
    catalog.apply_columns(
        &schema,
        Some("cached_orders"),
        std::collections::BTreeMap::from([(
            "cached_orders".into(),
            vec![CatalogColumn {
                name: "cached_id".into(),
                data_type: "BIGINT".into(),
                comment: None,
            }],
        )]),
        at,
    );
    storage::save_catalog(&private, &catalog).unwrap();
    let app = TestApp::launch_in(cx, directory, workspace, credentials);
    app.run_complete(cx, "SELECT 1");
    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.connection_page(cx, "Catalog");
    app.scroll_to(cx, "connection-schema-refresh");
    app.select(cx, "connection-schema-refresh", "While connected");
    app.scroll_to(cx, "connection-schema-catalog");
    app.click(cx, "connection-schema-catalog");
    app.wait_for(cx, "connection-new-shared-catalog");
    app.click(cx, "connection-new-shared-catalog");
    app.wait_for(cx, "connection-shared-catalog-name");
    app.scroll_to(cx, "connection-shared-catalog-name");
    app.fill(cx, "connection-shared-catalog-name", "Seeded");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "save-profile");
    app.wait_until(cx, "the transferred cache", QUERY_TIMEOUT, |_, _| {
        let saved = app.saved();
        let Some(shared) = saved.shared_catalogs.first() else {
            return false;
        };
        storage::load_catalog(&storage::catalog_path(&workspace_file, shared.id)).is_some_and(
            |cache| {
                cache
                    .relation(&schema, "cached_orders")
                    .is_some_and(|relation| {
                        relation.columns.as_ref().is_some_and(|columns| {
                            columns.iter().any(|column| column.name == "cached_id")
                        })
                    })
            },
        ) && !private.exists()
    });
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the seeded schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.click_labelled(cx, "cached_orders");
    app.wait_until(cx, "the seeded columns", QUERY_TIMEOUT, |window, _| {
        labelled(window, "cached_id bigint").is_some()
    });
}

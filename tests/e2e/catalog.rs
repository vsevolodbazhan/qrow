use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, bounds_of, connection_row, labelled};
use gpui_kit::TestAppContext;
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind, Unfinished},
    model::{CatalogRefresh, CatalogSettings, SharedCatalog},
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
    app.wait_status(cx, "Disconnected · Idle timeout");
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
    app.wait_status(cx, "Disconnected · Idle timeout");
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

    app.fill_labelled(cx, "Search Tables", &format!("{schema}.bookings"));
    app.wait_until(cx, "the qualified match", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some() && labelled(window, "daily").is_none()
    });
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Copy Qualified Name");
    let name = cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(name, format!("`{schema}`.`bookings`"));
    app.fill_labelled(cx, "Search Tables", &name);
    app.wait_until(
        cx,
        "the copied name to match",
        QUERY_TIMEOUT,
        |window, _| labelled(window, "bookings").is_some() && labelled(window, "daily").is_none(),
    );

    app.fill_labelled(cx, "Search Tables", &format!("{schema}.missing"));
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
    app.fill_labelled(cx, "Search Tables", &name);
    app.wait_until(cx, "the table to return", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some() && labelled(window, "No matches").is_none()
    });

    app.type_sql(cx, "SELECT COUNT(*) FROM ");
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Insert into Editor");
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
    app.choose(cx, "popup-menu", "Insert into Editor");
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
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    // Other tests make schemas too. The filter keeps the tree to this one.
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    // An automatic refresh would read the catalog before the schema exists.
    workspace.profiles[0].catalog.refresh = CatalogRefresh::Manual;
    let profile = workspace.profiles[0].clone();
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
        labelled(window, "gate STRING").is_some()
    });

    app.run_complete(
        cx,
        &format!("ALTER TABLE {schema}.bookings ADD COLUMNS (fare DOUBLE)"),
    );
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Refresh");
    app.wait_until(cx, "the new column", QUERY_TIMEOUT, |window, _| {
        labelled(window, "fare DOUBLE").is_some()
    });

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
        labelled(window, "gate STRING").is_some()
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
        labelled(window, "gate STRING").is_some()
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
        labelled(window, "gate STRING").is_some()
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
    app.wait_gone(cx, "connection-name");
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
        labelled(window, "cached_id BIGINT").is_some()
    });
}

//! Real LDAP/Kyuubi/Spark acceptance tests. Run only through `./qtest run backend`.
//! Ignored tests must fail, never silently skip, when explicitly requested without a fixture.
use anyhow::{Context, Result, ensure};
use qrow::{
    connector::{Connector, Secret, hive::HiveConnector},
    model::{Column, MAX_RESULT_BYTES, MAX_RESULT_ROWS, Profile, Row},
    worker::{Event, Worker},
};
use std::{
    process::Command,
    sync::{Arc, mpsc::RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

#[path = "../support/evidence.rs"]
mod evidence;
mod oidc;

const TIMEOUT: Duration = Duration::from_secs(150);
const REGISTER: &str = "CREATE TEMPORARY FUNCTION qrow_block AS 'io.qrow.fixture.Blocking'";

fn profile() -> Result<Profile> {
    ensure!(
        std::env::var("QROW_E2E_PROJECT")?.starts_with("qrow-e2e-"),
        "Not an isolated fixture"
    );
    Ok(Profile {
        name: "Qrow acceptance fixture".into(),
        host: "127.0.0.1".into(),
        port: std::env::var("QROW_E2E_PORT")?.parse()?,
        username: "qrow".into(),
        database: "default".into(),
        parameters: [("spark.sql.session.timeZone".into(), "UTC".into())].into(),
        ..Profile::default()
    })
}
struct Client {
    worker: Worker,
    profile: Profile,
}
impl Client {
    fn new() -> Result<Self> {
        Ok(Self::with(
            profile()?,
            HiveConnector::default(),
            Arc::new(|_| Ok(Secret::password("qrow-test-password"))),
        ))
    }
    fn with(
        profile: Profile,
        connector: HiveConnector,
        credentials: qrow::worker::CredentialProvider,
    ) -> Self {
        Self {
            profile,
            worker: Worker::with_connector(Arc::new(|| {}), Arc::new(connector), credentials),
        }
    }
    fn run(&self, sql: &str) {
        self.worker.run(self.profile.clone(), sql.into());
    }
    fn query(&self, sql: &str) -> Result<Page> {
        self.run(sql);
        self.page()
    }
    fn event(&self, deadline: Instant) -> Result<Event> {
        self.worker
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .context("Timed out waiting for Qrow worker")
    }
    fn page(&self) -> Result<Page> {
        let deadline = Instant::now() + TIMEOUT;
        let mut page = Page::default();
        loop {
            match self.event(deadline)? {
                Event::Columns(columns) => page.columns = columns,
                Event::Rows(rows) => page.rows.extend(rows),
                Event::Ready { more, limited } => {
                    page.more = more;
                    page.limited = limited;
                    return Ok(page);
                }
                Event::Error { message, .. } | Event::CancelError(message) => {
                    anyhow::bail!("{message}")
                }
                Event::Cancelled => anyhow::bail!("Unexpected cancellation"),
                _ => {}
            }
        }
    }
    fn failure(&self) -> Result<bool> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.event(deadline)? {
                Event::Error {
                    message,
                    disconnected,
                    ..
                } => {
                    println!("Expected failure: {message}");
                    return Ok(disconnected);
                }
                Event::Ready { .. } => anyhow::bail!("Query unexpectedly succeeded"),
                _ => {}
            }
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.worker.shutdown();
        self.worker.wait_for_shutdown(Duration::from_secs(5));
    }
}
#[derive(Default)]
struct Page {
    columns: Vec<Column>,
    rows: Vec<Row>,
    more: bool,
    limited: bool,
}
fn scalar(page: &Page, expected: &str) {
    assert_eq!(page.rows, vec![vec![Some(expected.into())]]);
}
/// Stops the engine, restarts Kyuubi, or waits for the servers.
fn observe(action: &str) -> Result<()> {
    let output = Command::new("python3")
        .args(["scripts/e2e/fixture.py", "observe", action])
        .output()?;
    ensure!(
        output.status.success(),
        "Observer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
fn count(token: &str, state: &str) -> Result<usize> {
    Ok(evidence::count(token, state)?)
}
fn wait_evidence(token: &str, state: &str, deadline: Instant) -> Result<()> {
    loop {
        match count(token, state)? {
            0 => {}
            1 => {
                ensure!(
                    Instant::now() <= deadline,
                    "Executor evidence arrived after deadline: {token}.{state}"
                );
                return Ok(());
            }
            n => anyhow::bail!("Executor recorded {token}.{state} {n} times"),
        }
        ensure!(
            Instant::now() < deadline,
            "Executor did not record {token}.{state}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
/// Fails when the worker sends an event, or the executor starts `token`
/// again, before `until`.
fn stays_quiet(client: &Client, token: Option<&str>, until: Instant) -> Result<()> {
    loop {
        if let Some(token) = token {
            ensure!(
                count(token, "started")? == 1,
                "SQL was automatically replayed"
            );
        }
        let now = Instant::now();
        if now >= until {
            return Ok(());
        }
        match client
            .worker
            .events
            .recv_timeout((until - now).min(Duration::from_millis(100)))
        {
            Ok(_) => anyhow::bail!("Unexpected background activity after failure"),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("The worker stopped"),
        }
    }
}
fn blocking(token: &str) -> String {
    format!("SELECT qrow_block(id, '{token}', CAST(60000 AS BIGINT)) FROM range(1)")
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn readiness() -> Result<()> {
    let client = Client::new()?;
    scalar(&client.query("SELECT substring(version(), 1, 5)")?, "3.5.3");
    scalar(&client.query("SELECT current_database()")?, "default");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn ldap_rejects_wrong_password_and_unknown_user() -> Result<()> {
    let mut p = profile()?;
    ensure!(
        HiveConnector::default()
            .connect(&p, Secret::password("incorrect"))
            .is_err()
    );
    p.username = "missing-user".into();
    ensure!(
        HiveConnector::default()
            .connect(&p, Secret::password("qrow-test-password"))
            .is_err()
    );
    scalar(&Client::new()?.query("SELECT 1")?, "1");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn exact_types_and_wide_results() -> Result<()> {
    let client = Client::new()?;
    let page = client.query("SELECT CAST(NULL AS STRING) AS missing, '' AS empty, '日本語😀' AS unicode, CAST('12345678901234567890.123456789' AS DECIMAL(29,9)) AS decimal_value, TIMESTAMP '2024-02-29 12:34:56.123456' AS timestamp_value, unhex('00ff41') AS binary_value, true AS flag, CAST(-128 AS TINYINT) AS tiny, CAST(32767 AS SMALLINT) AS small, CAST(2147483647 AS INT) AS integer_value, CAST(9223372036854775807 AS BIGINT) AS big, CAST(1.5 AS FLOAT) AS float_value, CAST(2.25 AS DOUBLE) AS double_value, DATE '2024-02-29' AS date_value")?;
    let expected = [
        None,
        Some(""),
        Some("日本語😀"),
        Some("12345678901234567890.123456789"),
        Some("2024-02-29 12:34:56.123456"),
        Some("0x00ff41"),
        Some("true"),
        Some("-128"),
        Some("32767"),
        Some("2147483647"),
        Some("9223372036854775807"),
        Some("1.5"),
        Some("2.25"),
        Some("2024-02-29"),
    ];
    assert_eq!(
        page.rows,
        vec![
            expected
                .into_iter()
                .map(|v| v.map(str::to_owned))
                .collect::<Row>()
        ]
    );
    assert_eq!(page.columns.len(), expected.len());
    assert_eq!(page.columns[0].name, "missing");
    let sql = format!(
        "SELECT {}",
        (0..150)
            .map(|i| format!("{i} AS c{i}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let wide = client.query(&sql)?;
    assert_eq!(wide.columns.len(), 150);
    assert_eq!(
        wide.rows[0],
        (0..150).map(|i| Some(i.to_string())).collect::<Row>()
    );
    scalar(
        &client.query("SELECT repeat('x', 20000)")?,
        &"x".repeat(20000),
    );
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn pagination_exhaustion_and_preview_limits() -> Result<()> {
    let client = Client::new()?;
    for count in [0, 250, 1000, 1001, 2250, 100_001] {
        let mut page = client.query(&format!("SELECT id FROM range({count}) ORDER BY id"))?;
        let mut rows = Vec::new();
        loop {
            ensure!(page.rows.len() <= 1000);
            rows.extend(page.rows);
            if !page.more {
                assert_eq!(page.limited, count > MAX_RESULT_ROWS);
                break;
            }
            client.worker.more();
            page = client.page()?;
        }
        assert_eq!(
            rows,
            (0..count.min(MAX_RESULT_ROWS))
                .map(|n| vec![Some(n.to_string())])
                .collect::<Vec<_>>()
        );
    }
    let mut page = client.query("SELECT repeat('x', 65536) FROM range(2000)")?;
    let mut bytes = 0;
    loop {
        bytes += page
            .rows
            .iter()
            .map(|row| {
                row.capacity() * size_of::<Option<String>>()
                    + row.iter().flatten().map(String::capacity).sum::<usize>()
            })
            .sum::<usize>();
        ensure!(bytes <= MAX_RESULT_BYTES);
        if !page.more {
            ensure!(page.limited);
            break;
        }
        client.worker.more();
        page = client.page()?;
    }
    ensure!(
        bytes > MAX_RESULT_BYTES / 2,
        "Memory limit test fetched too little data"
    );
    scalar(&client.query("SELECT 42")?, "42");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn sessions_are_isolated_and_sql_errors_are_recoverable() -> Result<()> {
    let first = Client::new()?;
    let second = Client::new()?;
    first.query("CREATE TEMPORARY VIEW qrow_private AS SELECT 73 AS n")?;
    scalar(&first.query("SELECT n FROM qrow_private")?, "73");
    second.run("SELECT n FROM qrow_private");
    ensure!(
        !second.failure()?,
        "Missing table must not discard a healthy session"
    );
    first.query("SET spark.sql.session.timeZone=Asia/Tokyo")?;
    scalar(&first.query("SELECT current_timezone()")?, "Asia/Tokyo");
    scalar(&second.query("SELECT current_timezone()")?, "UTC");
    first.run("SELECT FROM");
    ensure!(!first.failure()?);
    scalar(&first.query("SELECT n FROM qrow_private")?, "73");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn cancellation_interrupts_executor_within_ten_seconds_and_preserves_other_tab() -> Result<()> {
    let first = Client::new()?;
    let second = Client::new()?;
    first.query(REGISTER)?;
    let token = format!("cancel-{}", uuid::Uuid::new_v4());
    first.run(&blocking(&token));
    wait_evidence(&token, "started", Instant::now() + TIMEOUT)?;
    scalar(&second.query("SELECT sum(id) FROM range(10)")?, "45");
    let started = Instant::now();
    first.worker.cancel();
    let deadline = started + Duration::from_secs(10);
    wait_evidence(&token, "interrupted", deadline)?;
    wait_evidence(&token, "ended", deadline)?;
    loop {
        match first.event(deadline)? {
            Event::Cancelled => break,
            Event::Error { message, .. } | Event::CancelError(message) => {
                anyhow::bail!("{message}")
            }
            Event::Ready { .. } => anyhow::bail!("Cancelled operation reported success"),
            _ => {}
        }
    }
    let elapsed = started.elapsed();
    ensure!(elapsed <= Duration::from_secs(10));
    assert_eq!(count(&token, "completed")?, 0);
    scalar(&second.query("SELECT 43")?, "43");
    scalar(&first.query("SELECT 44")?, "44");
    println!(
        "Executor interruption and Qrow cancellation confirmed in {:?}",
        elapsed
    );
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn idle_disconnect_and_heartbeat_preserve_expected_session_state() -> Result<()> {
    let mut client = Client::new()?;
    client.profile.lifecycle.idle_seconds = 2;
    client.query("CREATE TEMPORARY VIEW qrow_idle AS SELECT 1")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if matches!(client.event(deadline)?, Event::IdleDisconnected) {
            break;
        }
    }
    client.run("SELECT * FROM qrow_idle");
    ensure!(!client.failure()?);
    scalar(&client.query("SELECT 7")?, "7");
    client.profile.lifecycle.keep_alive_seconds = 1;
    let page = client.query("SELECT id FROM range(1250) ORDER BY id")?;
    assert_eq!(page.rows.len(), 1000);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if matches!(client.event(deadline)?, Event::KeepAliveFinished) {
            break;
        }
    }
    client.worker.more();
    let page = client.page()?;
    assert_eq!(
        page.rows,
        (1000..1250)
            .map(|n| vec![Some(n.to_string())])
            .collect::<Vec<_>>()
    );
    ensure!(!page.more);
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn lifecycle_edits_preserve_live_session_state() -> Result<()> {
    let mut client = Client::new()?;
    client.profile.lifecycle.keep_alive_seconds = 60;
    client.profile.lifecycle.keep_alive_sql = "SELECT 1".into();
    client.query("CREATE TEMPORARY VIEW qrow_live AS SELECT 1")?;

    let page = client.query("SELECT id FROM range(1250) ORDER BY id")?;
    ensure!(page.more, "Expected an unfetched result page");
    assert_eq!(page.rows.len(), 1000);

    let mut updated = client.profile.clone();
    updated.name = "Qrow lifecycle update".into();
    updated.lifecycle.keep_alive_seconds = 1;
    updated.lifecycle.keep_alive_sql = "SELECT 1".into();
    client.worker.update_profile(updated.clone())?;
    client.profile = updated;
    loop {
        if matches!(
            client.event(Instant::now() + TIMEOUT)?,
            Event::KeepAliveFinished
        ) {
            break;
        }
    }

    client.worker.more();
    let page = client.page()?;
    assert_eq!(
        page.rows,
        (1000..1250)
            .map(|n| vec![Some(n.to_string())])
            .collect::<Vec<_>>()
    );
    scalar(&client.query("SELECT * FROM qrow_live")?, "1");

    let mut disconnected = client.profile.clone();
    disconnected.lifecycle.keep_alive_seconds = 0;
    disconnected.lifecycle.idle_seconds = 2;
    client.worker.update_profile(disconnected.clone())?;
    client.profile = disconnected;
    loop {
        if matches!(
            client.event(Instant::now() + TIMEOUT)?,
            Event::IdleDisconnected
        ) {
            break;
        }
    }

    client.run("SELECT * FROM qrow_live");
    ensure!(
        !client.failure()?,
        "A missing temporary view must not discard the new session"
    );
    scalar(&client.query("SELECT 1")?, "1");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn engine_and_transport_failure_never_replay_sql() -> Result<()> {
    for action in ["kill-engine", "restart-server"] {
        let client = Client::new()?;
        client.query(REGISTER)?;
        let token = format!("failure-{}", uuid::Uuid::new_v4());
        client.run(&blocking(&token));
        wait_evidence(&token, "started", Instant::now() + TIMEOUT)?;
        observe(action)?;
        ensure!(
            client.failure()?,
            "Dead transport/engine must discard session"
        );
        // The worker must stay quiet for 3 seconds after the failure. The
        // servers recover in that time.
        let quiet = Instant::now() + Duration::from_secs(3);
        observe("ready")?;
        stays_quiet(&client, Some(&token), quiet)?;
        scalar(&client.query("SELECT 99")?, "99");
    }
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn failed_heartbeat_disconnects_and_stops_background_queries() -> Result<()> {
    let mut client = Client::new()?;
    client.profile.lifecycle.keep_alive_seconds = 1;
    client.profile.lifecycle.keep_alive_sql = "SELECT * FROM qrow_missing_heartbeat_table".into();
    scalar(&client.query("SELECT 51")?, "51");
    ensure!(
        client.failure()?,
        "Failed heartbeat must discard its session"
    );
    // Three keep-alive intervals.
    stays_quiet(&client, None, Instant::now() + Duration::from_secs(3))
        .context("Heartbeat resumed without explicit execution")?;
    client.profile.lifecycle.keep_alive_seconds = 0;
    scalar(&client.query("SELECT 52")?, "52");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn cancellation_during_setup_prevents_user_sql_submission() -> Result<()> {
    // Delay only credential delivery. The subsequent authentication/session setup uses real Kyuubi.
    let (release, gate) = std::sync::mpsc::channel();
    let passwords = std::sync::Mutex::new(gate);
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(HiveConnector::default()),
        Arc::new(move |_| {
            passwords.lock().unwrap().recv_timeout(TIMEOUT)?;
            Ok(Secret::password("qrow-test-password"))
        }),
    );
    let client = Client {
        worker,
        profile: profile()?,
    };
    client.run("CREATE TEMPORARY VIEW qrow_cancelled_setup AS SELECT 1");
    let deadline = Instant::now() + TIMEOUT;
    ensure!(matches!(client.event(deadline)?, Event::Connecting));
    client.worker.cancel();
    release.send(())?;
    loop {
        match client.event(deadline)? {
            Event::Cancelled => break,
            Event::Connected => {}
            Event::Error { message, .. } => anyhow::bail!("{message}"),
            _ => anyhow::bail!("User SQL progressed after setup cancellation"),
        }
    }
    // Same real session must exist, but the cancelled CREATE must never have reached it.
    client.run("SELECT * FROM qrow_cancelled_setup");
    ensure!(!client.failure()?);
    scalar(&client.query("SELECT 53")?, "53");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn shutdown_stops_active_server_work() -> Result<()> {
    let client = Client::new()?;
    client.query(REGISTER)?;
    let token = format!("shutdown-{}", uuid::Uuid::new_v4());
    client.run(&blocking(&token));
    wait_evidence(&token, "started", Instant::now() + TIMEOUT)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    client.worker.shutdown();
    wait_evidence(&token, "interrupted", deadline)?;
    wait_evidence(&token, "ended", deadline)?;
    assert_eq!(count(&token, "completed")?, 0);
    Ok(())
}

#[test]
fn evidence_uses_executor_and_driver_markers() -> Result<()> {
    let root = tempfile::tempdir()?;
    let root = root.path();
    assert_eq!(evidence::count_in(root, "query", "started")?, 0);
    std::fs::write(root.join("query.interrupted"), "1\n")?;
    std::fs::write(root.join("query.task"), "app-123-4")?;
    std::fs::write(root.join("app-123-4.ended"), "1\n")?;
    assert_eq!(evidence::count_in(root, "query", "interrupted")?, 1);
    assert_eq!(evidence::count_in(root, "query", "ended")?, 1);
    for (token, state) in [
        ("../query", "started"),
        ("query", "task"),
        ("/query", "ended"),
        ("valid", "started;id"),
        ("x/../x", "started"),
        ("", "started"),
    ] {
        ensure!(
            evidence::count_in(root, token, state).is_err(),
            "Accepted {token}.{state}"
        );
    }
    std::fs::write(root.join("query.task"), "../outside")?;
    ensure!(evidence::count_in(root, "query", "ended").is_err());
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn catalog_reads_exact_schemas_relations_and_columns() -> Result<()> {
    use qrow::catalog::{self, CatalogWorker, RelationKind, Scope, Status};
    let client = Client::new()?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    // As a server pattern, `_` in the first name also matches the second one.
    let schema = format!("qrow_c_{id}");
    let sibling = format!("qrowxcx{id}");
    for sql in [
        format!("CREATE DATABASE {schema}"),
        format!("CREATE DATABASE {sibling}"),
        format!(
            "CREATE TABLE {schema}.orders (id INT COMMENT 'Order key', total DECIMAL(10,2), day STRING) USING parquet PARTITIONED BY (day) COMMENT 'Paid orders'"
        ),
        format!("CREATE VIEW {schema}.recent AS SELECT id FROM {schema}.orders"),
        format!("CREATE TABLE {sibling}.other (x INT) USING parquet"),
    ] {
        client.query(&sql)?;
    }
    let mut profile = client.profile.clone();
    profile.catalog.include = vec![schema.clone(), sibling.clone()];
    let member = profile.id;
    let worker = CatalogWorker::with_connector(
        catalog::CatalogConfig::private(profile),
        None,
        Arc::new(|| {}),
        Arc::new(HiveConnector::default()),
        Arc::new(|_| Ok(Secret::password("qrow-test-password"))),
        catalog::MINUTE,
    );
    let mut latest: Option<Arc<catalog::Catalog>> = None;
    let mut refresh = |scope: Scope| -> Result<Arc<catalog::Catalog>> {
        worker.refresh(member, scope.clone());
        let deadline = Instant::now() + TIMEOUT;
        let mut started = false;
        loop {
            match worker
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("Timed out waiting for the catalog worker")?
            {
                catalog::Event::Catalog(catalog) => latest = Some(catalog),
                catalog::Event::Status(status) => {
                    started |= status.includes(&scope);
                    if started && status == Status::default() {
                        return latest.clone().context("No catalog");
                    }
                }
            }
        }
    };
    let result = (|| -> Result<()> {
        let catalog = refresh(Scope::Connection)?;
        ensure!(catalog.error.is_none(), "{:?}", catalog.error);
        let mut names: Vec<_> = catalog.schemas.keys().cloned().collect();
        names.sort();
        let mut expected = vec![schema.clone(), sibling.clone()];
        expected.sort();
        assert_eq!(names, expected);
        let relations: Vec<_> = catalog
            .schema(&schema)
            .and_then(|node| node.relations.as_ref())
            .context("No relations")?
            .keys()
            .cloned()
            .collect();
        assert_eq!(relations, ["orders", "recent"]);
        let orders = catalog.relation(&schema, "orders").context("No orders")?;
        assert_eq!(orders.kind, RelationKind::Table);
        assert_eq!(orders.comment.as_deref(), Some("Paid orders"));
        assert_eq!(
            catalog.relation(&schema, "recent").context("No view")?.kind,
            RelationKind::View
        );

        let catalog = refresh(Scope::Schema(schema.clone()))?;
        let columns = catalog
            .relation(&schema, "orders")
            .and_then(|relation| relation.columns.clone())
            .context("No columns")?;
        let names: Vec<_> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "total", "day"]);
        assert_eq!(columns[0].data_type, "INT");
        assert_eq!(columns[0].comment.as_deref(), Some("Order key"));
        assert!(
            columns[1].data_type.starts_with("DECIMAL"),
            "{}",
            columns[1].data_type
        );
        let view: Vec<_> = catalog
            .relation(&schema, "recent")
            .and_then(|relation| relation.columns.clone())
            .context("No view columns")?
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(view, ["id"]);

        client.query(&format!(
            "ALTER TABLE {schema}.orders ADD COLUMNS (currency STRING)"
        ))?;
        let catalog = refresh(Scope::Relation(schema.clone(), "orders".into()))?;
        let names: Vec<_> = catalog
            .relation(&schema, "orders")
            .and_then(|relation| relation.columns.clone())
            .context("No columns")?
            .into_iter()
            .map(|c| c.name)
            .collect();
        ensure!(names.contains(&"currency".to_owned()), "{names:?}");

        client.query(&format!("DROP VIEW {schema}.recent"))?;
        let catalog = refresh(Scope::Relation(schema.clone(), "recent".into()))?;
        ensure!(catalog.relation(&schema, "recent").is_none());
        ensure!(catalog.relation(&schema, "orders").is_some());
        Ok(())
    })();
    worker.shutdown();
    worker.wait_for_shutdown(Duration::from_secs(5));
    for name in [&schema, &sibling] {
        let _ = client.query(&format!("DROP DATABASE IF EXISTS {name} CASCADE"));
    }
    result
}

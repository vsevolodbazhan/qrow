use anyhow::Result;
use qrow::{
    connector::{
        Completion, Connector, DatabaseConnector, MetadataRequest, QueryError, QueryState, Secret,
        Session, wait_for_completion,
    },
    model::{DatabaseType, Profile},
    tls::Trust,
};
use std::time::{Duration, Instant};

fn profile() -> Profile {
    let fixture = std::env::var("QROW_TRINO_FIXTURE").expect("Run with ./qtest run trino");
    assert!(fixture.starts_with("qrow-e2e-trino-"));
    Profile {
        database_type: DatabaseType::Trino,
        name: "Trino".into(),
        host: "localhost".into(),
        port: std::env::var("QROW_TRINO_PORT").unwrap().parse().unwrap(),
        username: "qrow".into(),
        database: "tpch".into(),
        trino_schema: "tiny".into(),
        tls: true,
        ..Profile::default()
    }
}
fn connector() -> DatabaseConnector {
    DatabaseConnector::new(
        Trust::from_pem(&std::fs::read(std::env::var("QROW_TRINO_CA").unwrap()).unwrap()).unwrap(),
    )
}
fn connect() -> Box<dyn Session> {
    connector()
        .connect(&profile(), Secret::password("qrow-test-password"))
        .unwrap()
}
fn complete(session: &mut dyn Session, sql: &str) -> Result<Completion> {
    session.execute(sql)?;
    wait_for_completion(session, Some(Instant::now() + Duration::from_secs(30)))
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn verified_tls_authentication_types_pages_and_error_recovery() -> Result<()> {
    assert!(
        connector()
            .connect(&profile(), Secret::password("wrong-test-password"))
            .is_err()
    );
    assert!(
        DatabaseConnector::default()
            .connect(&profile(), Secret::password("qrow-test-password"))
            .is_err()
    );
    let mut session = connect();
    complete(
        &mut *session,
        "SELECT i AS value, CAST(NULL AS varchar) AS missing, 'α🦀' AS unicode, ARRAY[1,2] AS items, DECIMAL '12345678901234567890.123456789' AS amount FROM UNNEST(sequence(1,1001)) AS t(i)",
    )?;
    assert_eq!(session.columns()?[0].data_type, "bigint");
    let rows = session.fetch(1000)?.rows;
    assert_eq!(rows.len(), 1000);
    assert_eq!(
        rows[0],
        vec![
            Some("1".into()),
            None,
            Some("α🦀".into()),
            Some("[1,2]".into()),
            Some("12345678901234567890.123456789".into())
        ]
    );
    assert_eq!(session.fetch(1000)?.rows[0][0].as_deref(), Some("1001"));
    assert!(session.fetch(1)?.rows.is_empty());
    let error = complete(&mut *session, "SELECT missing_column").unwrap_err();
    assert!(error.downcast_ref::<QueryError>().is_some());
    complete(&mut *session, "SELECT 42 AS value WHERE false")?;
    assert_eq!(session.columns()?[0].name, "value");
    assert!(session.fetch(1)?.rows.is_empty());
    complete(&mut *session, "SELECT 42 AS value")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    session.close()
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn session_changes_transactions_keep_alive_and_metadata() -> Result<()> {
    let mut session = connect();
    complete(&mut *session, "USE tpch.sf1")?;
    complete(&mut *session, "SELECT current_catalog, current_schema")?;
    assert_eq!(
        session.fetch(1)?.rows[0],
        vec![Some("tpch".into()), Some("sf1".into())]
    );
    complete(&mut *session, "SET SESSION query_max_run_time = '17m'")?;
    complete(&mut *session, "SHOW SESSION LIKE 'query_max_run_time'")?;
    assert_eq!(session.fetch(1)?.rows[0][1].as_deref(), Some("17m"));
    complete(&mut *session, "RESET SESSION query_max_run_time")?;
    complete(&mut *session, "START TRANSACTION")?;
    complete(&mut *session, "SELECT 8 AS value")?;
    complete(&mut *session, "ROLLBACK")?;
    complete(&mut *session, "SELECT i FROM UNNEST(sequence(1,3)) AS t(i)")?;
    session.fetch(1)?;
    session.execute_keep_alive("SELECT 1")?;
    wait_for_completion(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(10)),
    )?;
    session.close_keep_alive()?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("2"));
    for request in [
        MetadataRequest::Schemas,
        MetadataRequest::Relations {
            schema: "tiny".into(),
            relation: Some("nation".into()),
        },
        MetadataRequest::Columns {
            schema: "tiny".into(),
            relation: Some("nation".into()),
        },
    ] {
        session.execute_metadata(&request)?;
        wait_for_completion(
            &mut *session,
            Some(Instant::now() + Duration::from_secs(20)),
        )?;
        assert!(!session.fetch(100)?.rows.is_empty());
    }
    complete(&mut *session, "PREPARE answer FROM SELECT 7 AS value")?;
    complete(&mut *session, "EXECUTE answer")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("7"));
    complete(&mut *session, "DEALLOCATE PREPARE answer")?;
    session.close()
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn cancellation_and_row_limits_leave_the_session_usable() -> Result<()> {
    let mut session = connect();
    let cancel = session.execute("SELECT count(*) FROM tpch.sf1000.lineitem")?;
    cancel.cancel()?;
    assert_eq!(session.poll()?, QueryState::Cancelled);
    complete(&mut *session, "SELECT 42 AS value")?;
    // An old cancellation handle has no cursor after cleanup and must not
    // cancel a later query on this session.
    cancel.cancel()?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    complete(
        &mut *session,
        "SELECT x*1000+y AS i FROM UNNEST(sequence(1,101)) t(x) CROSS JOIN UNNEST(sequence(1,1000)) u(y) LIMIT 100001",
    )?;
    let mut rows = 0;
    loop {
        let batch = session.fetch(1000)?;
        if batch.rows.is_empty() {
            break;
        }
        rows += batch.rows.len();
    }
    assert_eq!(rows, qrow::model::MAX_RESULT_ROWS);
    assert!(session.result_limited());
    session.close()
}

mod protocol;

/// Only the disposable fixture's request methods/paths are inspected.
fn fixture_request_log() -> Result<String> {
    let fixture = std::env::var("QROW_TRINO_FIXTURE")?;
    anyhow::ensure!(fixture.starts_with("qrow-e2e-trino-"));
    let output = std::process::Command::new("docker")
        .args(["exec", &fixture, "cat", "/tmp/qrow-http-request.log"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "fixture request log is unavailable"
    );
    anyhow::ensure!(
        output.stdout.len() < 4 * 1024 * 1024,
        "fixture request log exceeds test limit"
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn fixture_query_id(marker: &str) -> Result<String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let certificate =
        reqwest::Certificate::from_pem(&std::fs::read(std::env::var("QROW_TRINO_CA")?)?)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .add_root_certificate(certificate)
        .timeout(Duration::from_secs(5))
        .build()?;
    let bytes = runtime.block_on(async {
        client
            .get(format!("https://localhost:{}/v1/query", profile().port))
            .basic_auth("qrow", Some("qrow-test-password"))
            .header("X-Trino-User", "qrow")
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    })?;
    let queries: serde_json::Value = serde_json::from_slice(&bytes)?;
    let matches = queries
        .as_array()
        .unwrap()
        .iter()
        .filter(|query| {
            query["query"]
                .as_str()
                .is_some_and(|sql| sql.contains(marker))
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        matches.len() == 1,
        "expected one uniquely marked pause-test query"
    );
    anyhow::ensure!(
        matches!(matches[0]["state"].as_str(), Some("RUNNING" | "FINISHING")),
        "pause-test query must remain active: {}",
        matches[0]["state"]
    );
    Ok(matches[0]["queryId"].as_str().unwrap().into())
}

fn result_requests(log: &str, method: &str, query: &str) -> usize {
    let route = format!("/v1/statement/executing/{query}/");
    log.lines()
        .filter(|line| line.contains(method) && line.contains(&route))
        .count()
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn direct_export_heartbeat_survives_a_spool_pause_beyond_the_client_timeout() -> Result<()> {
    use qrow::export::spool::Spool;
    use std::sync::atomic::AtomicBool;
    let mut session = connect();
    let cancellation = session.execute_export(
        "SELECT orderkey, rpad('α', 256, 'α') AS qrow_heartbeat_pause FROM tpch.sf1.lineitem",
    )?;
    assert_eq!(
        qrow::connector::wait_for_result(
            &mut *session,
            Some(Instant::now() + Duration::from_secs(30)),
        )?,
        QueryState::Streaming { has_results: true }
    );
    let (spool, mut producer) = Spool::new(&session.columns()?, &session.export_context())?;
    let stop = AtomicBool::new(false);
    let first = session.fetch(1000)?;
    assert!(!first.rows.is_empty());
    producer.append(&first.rows, &stop)?;
    let committed = spool.bytes();
    drop(first);
    let query = fixture_query_id("qrow_heartbeat_pause")?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let before = loop {
        let log = fixture_request_log()?;
        if result_requests(&log, "HEAD", &query) > 0 {
            break log;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "the first heartbeat was not logged"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let heads_before = result_requests(&before, "HEAD", &query);
    let gets_before = result_requests(&before, "GET", &query);
    assert!(gets_before > 0);
    // This fixture uses a five-second client timeout. Pause the producer,
    // including all result GETs, for more than twice that timeout.
    std::thread::sleep(Duration::from_secs(12));
    assert_eq!(spool.bytes(), committed);
    let after = fixture_request_log()?;
    let heads_after = result_requests(&after, "HEAD", &query);
    assert!(
        heads_after >= heads_before + 5,
        "the coordinator must observe repeated HEAD heartbeats"
    );
    assert_eq!(result_requests(&after, "GET", &query), gets_before);
    let mut resumed_get = false;
    // Drain the raw page retained before the pause. A fresh successful GET
    // must return rows and commit them before this test passes.
    for _ in 0..40 {
        let batch = session.fetch(1000)?;
        assert!(
            !batch.rows.is_empty(),
            "the paused query must remain readable"
        );
        producer.append(&batch.rows, &stop)?;
        if result_requests(&fixture_request_log()?, "GET", &query) > gets_before {
            resumed_get = true;
            break;
        }
    }
    assert!(resumed_get, "the paused query must serve a new result GET");
    assert!(spool.bytes() > committed);
    cancellation.cancel()?;
    session.close_operation()?;
    spool.cancel();
    drop(producer);
    complete(&mut *session, "SELECT 42")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    session.close()
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn direct_export_exceeds_both_preview_limits_in_order() -> Result<()> {
    use qrow::export::spool::{Spool, Status};
    use std::sync::atomic::AtomicBool;
    let mut session = connect();
    session.execute_export(
        "SELECT x*1000+y AS value, rpad('x', 700, 'x') AS payload FROM UNNEST(sequence(0,100)) t(x) CROSS JOIN UNNEST(sequence(1,1000)) u(y) WHERE x*1000+y <= 100001 ORDER BY value",
    )?;
    qrow::connector::wait_for_result(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(30)),
    )?;
    let (spool, mut producer) = Spool::new(&session.columns()?, &session.export_context())?;
    let stop = AtomicBool::new(false);
    let mut count = 0;
    loop {
        let batch = session.fetch(1000)?;
        if batch.rows.is_empty() {
            break;
        }
        for row in &batch.rows {
            count += 1;
            assert_eq!(row[0].as_deref(), Some(count.to_string().as_str()));
            assert_eq!(row[1].as_deref().unwrap().len(), 700);
        }
        producer.append(&batch.rows, &stop)?;
    }
    assert_eq!(count, 100001);
    assert!(!session.result_limited());
    assert_eq!(
        session.finish_execution()?,
        Completion::Finished { has_results: true }
    );
    session.close_operation()?;
    producer.finish(&stop)?;
    assert_eq!(spool.status(), Status::Complete { rows: 100001 });
    assert!(spool.bytes() > qrow::model::MAX_RESULT_BYTES as u64);
    let mut reader = spool.reader()?;
    let mut replayed = 0;
    while let Some(batch) = reader.next(&stop)? {
        for index in 0..batch.rows().len() {
            let row = &batch.rows()[index];
            replayed += 1;
            assert_eq!(row[0].as_deref(), Some(replayed.to_string().as_str()));
        }
    }
    assert_eq!(replayed, count);
    complete(&mut *session, "SELECT 42")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    session.close()
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn direct_exports_keep_session_settings_prepared_statements_and_transactions() -> Result<()> {
    let mut session = connect();
    for sql in [
        "USE tpch.sf1",
        "SET SESSION query_max_run_time = '17m'",
        "PREPARE synthetic FROM SELECT current_schema AS schema, DECIMAL '12345678901234567890.12345' AS exact, CAST(NULL AS varchar) AS missing, '' AS empty, 'α🦀' AS unicode, from_hex('005cff') AS bytes, TIMESTAMP '2026-10-09 01:02:03.123456789' AS moment",
        "START TRANSACTION",
    ] {
        session.execute_export(sql)?;
        assert_eq!(
            qrow::connector::wait_for_result(&mut *session, None)?,
            QueryState::Finished { has_results: false },
            "{sql}"
        );
        assert!(session.columns()?.is_empty());
        session.close_operation()?;
    }
    session.execute_export("EXECUTE synthetic")?;
    qrow::connector::wait_for_result(&mut *session, None)?;
    let columns = session.columns()?;
    assert_eq!(columns[1].data_type, "decimal(25, 5)");
    assert_eq!(columns[6].data_type, "timestamp(9)");
    assert_eq!(
        session.fetch(1000)?.rows,
        vec![vec![
            Some("sf1".into()),
            Some("12345678901234567890.12345".into()),
            None,
            Some("".into()),
            Some("α🦀".into()),
            Some("AFz/".into()),
            Some("2026-10-09 01:02:03.123456789".into()),
        ]]
    );
    assert!(session.fetch(1)?.rows.is_empty());
    session.close_operation()?;
    session.execute_export("ROLLBACK")?;
    assert_eq!(
        qrow::connector::wait_for_result(&mut *session, None)?,
        QueryState::Finished { has_results: false }
    );
    session.close_operation()?;
    session.execute_export("SELECT 42 AS value WHERE false")?;
    assert!(matches!(
        qrow::connector::wait_for_result(&mut *session, None)?,
        QueryState::Streaming { has_results: true } | QueryState::Finished { has_results: true }
    ));
    assert_eq!(session.columns()?[0].name, "value");
    assert!(session.fetch(1)?.rows.is_empty());
    session.close_operation()?;
    complete(&mut *session, "SHOW SESSION LIKE 'query_max_run_time'")?;
    assert_eq!(session.fetch(1)?.rows[0][1].as_deref(), Some("17m"));
    session.close()
}

#[path = "../support/trino_oidc.rs"]
mod trino_oidc;
#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn external_browser_confidential_oidc_login_cache_and_renewal() -> Result<()> {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (profile, trust) = trino_oidc::configuration();
    let opens = Arc::new(AtomicUsize::new(0));
    let service =
        qrow::external_auth::Service::new(Some(trino_oidc::browser(trust.clone(), opens.clone())));
    service.configure(std::slice::from_ref(&profile));
    let connector = DatabaseConnector::new(trust);
    let secret = service.secret(&profile)?;
    let mut session = connector.connect(&profile, secret.clone())?;
    complete(&mut *session, "SELECT current_user")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("alice"));
    let mut other = connector.connect(&profile, secret)?;
    complete(&mut *other, "SELECT 42")?;
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    let refresh_grants = trino_oidc::refresh_grants()?;
    std::thread::sleep(Duration::from_secs(5));
    complete(&mut *session, "SELECT 43")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("43"));
    assert!(trino_oidc::refresh_grants()? > refresh_grants);
    assert_eq!(
        opens.load(Ordering::SeqCst),
        1,
        "Trino renewal must not reopen the browser"
    );
    session.close()?;
    other.close()?;
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run trino"]
fn variable_precision_timestamps_keep_every_server_digit() -> Result<()> {
    let mut session = connect();
    complete(
        &mut *session,
        "SELECT TIMESTAMP '2026-10-09 01:02:03.123456' AS micros, TIMESTAMP '2026-10-09 01:02:03.123456789' AS nanos, TIMESTAMP '2026-10-09 01:02:03.123456789123' AS picos",
    )?;
    assert_eq!(
        session
            .columns()?
            .iter()
            .map(|column| column.data_type.as_str())
            .collect::<Vec<_>>(),
        vec!["timestamp(6)", "timestamp(9)", "timestamp(12)"]
    );
    assert_eq!(
        session.fetch(1)?.rows[0],
        vec![
            Some("2026-10-09 01:02:03.123456".into()),
            Some("2026-10-09 01:02:03.123456789".into()),
            Some("2026-10-09 01:02:03.123456789123".into())
        ]
    );
    Ok(())
}

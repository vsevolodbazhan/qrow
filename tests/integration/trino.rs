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

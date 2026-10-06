use anyhow::Result;
use qrow::{
    connector::{
        Completion, Connector, DatabaseConnector, MetadataRequest, QueryError, QueryState, Secret,
        Session, postgres::PostgresConnector, wait_for_completion,
    },
    model::{Authentication, DatabaseType, Profile},
    tls::Trust,
};
use std::{
    thread,
    time::{Duration, Instant},
};

fn profile() -> Profile {
    let fixture = std::env::var("QROW_POSTGRES_FIXTURE").expect("Run with ./qtest run postgres");
    assert!(fixture.starts_with("qrow-e2e-postgres-"));
    Profile {
        database_type: DatabaseType::Postgres,
        host: "127.0.0.1".into(),
        port: std::env::var("QROW_POSTGRES_PORT")
            .unwrap()
            .parse()
            .unwrap(),
        username: "qrow".into(),
        database: "qrow".into(),
        ..Profile::default()
    }
}

fn connect() -> Box<dyn Session> {
    DatabaseConnector::default()
        .connect(&profile(), Secret::password("qrow-test-password"))
        .unwrap()
}

fn complete(session: &mut dyn Session, sql: &str) -> Result<Completion> {
    session.execute(sql)?;
    wait_for_completion(session, Some(Instant::now() + Duration::from_secs(10)))
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn streams_server_formatted_types_nulls_and_pages() -> Result<()> {
    let mut session = connect();
    assert_eq!(
        complete(
            &mut *session,
            "SELECT i::bigint AS id, NULL::text AS missing, 'α🦀'::text AS unicode, ARRAY[1,2] AS items, 123.45::numeric AS amount, '{\"ok\":true}'::jsonb AS json FROM generate_series(1,1001) i"
        )?,
        Completion::Finished { has_results: true }
    );
    let columns = session.columns()?;
    assert_eq!(columns[0].name, "id");
    assert_eq!(columns[0].data_type, "int8");
    let rows = session.fetch(1000)?.rows;
    assert_eq!(rows.len(), 1000);
    assert_eq!(
        rows[0],
        vec![
            Some("1".into()),
            None,
            Some("α🦀".into()),
            Some("{1,2}".into()),
            Some("123.45".into()),
            Some("{\"ok\": true}".into())
        ]
    );
    assert_eq!(session.fetch(1000)?.rows[0][0].as_deref(), Some("1001"));
    assert!(session.fetch(1)?.rows.is_empty());
    complete(&mut *session, "SELECT 1::integer AS value WHERE false")?;
    assert_eq!(session.columns()?[0].name, "value");
    assert!(session.fetch(1)?.rows.is_empty());
    session.close()?;
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn errors_cancellation_and_keep_alive_preserve_the_session() -> Result<()> {
    let mut session = connect();
    let error = complete(&mut *session, "SELECT missing_column").unwrap_err();
    assert!(error.downcast_ref::<QueryError>().is_some());
    session.close_operation()?;
    complete(&mut *session, "SELECT i FROM generate_series(1,1001) i")?;
    assert_eq!(session.fetch(1000)?.rows.len(), 1000);
    session.execute_keep_alive("SELECT 1")?;
    wait_for_completion(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(10)),
    )?;
    session.close_keep_alive()?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("1001"));
    let cancellation = session.execute("SELECT pg_sleep(30)")?;
    // A separate session proves the query reached the server before cancellation.
    let mut observer = connect();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        complete(
            &mut *observer,
            "SELECT count(*) FROM pg_stat_activity WHERE query = 'SELECT pg_sleep(30)' AND state = 'active'",
        )?;
        if observer.fetch(1)?.rows[0][0].as_deref() == Some("1") {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    cancellation.cancel()?;
    assert_eq!(
        wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(5)))?,
        Completion::Cancelled
    );
    assert_eq!(session.poll()?, QueryState::Cancelled);
    complete(&mut *session, "SELECT 42 AS value")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn settings_metadata_and_ddl_use_postgres_names_and_comments() -> Result<()> {
    let mut p = profile();
    p.parameters.insert("TimeZone".into(), "UTC".into());
    let mut session =
        DatabaseConnector::default().connect(&p, Secret::password("qrow-test-password"))?;
    complete(&mut *session, "SHOW TimeZone")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("UTC"));
    let schema = format!("pgdata_{}", uuid::Uuid::new_v4().simple());
    complete(&mut *session, &format!("CREATE SCHEMA \"{schema}\""))?;
    complete(
        &mut *session,
        &format!("CREATE TABLE \"{schema}\".\"Odd Table\" (\"Odd Column\" numeric(10,2))"),
    )?;
    complete(
        &mut *session,
        &format!("COMMENT ON COLUMN \"{schema}\".\"Odd Table\".\"Odd Column\" IS 'amount'"),
    )?;
    session.execute_metadata(&MetadataRequest::Schemas)?;
    wait_for_completion(&mut *session, None)?;
    assert!(
        session
            .fetch(1000)?
            .rows
            .iter()
            .any(|row| row[0].as_deref() == Some(&schema))
    );
    session.execute_metadata(&MetadataRequest::Relations {
        schema: schema.clone(),
        relation: Some("Odd Table".into()),
    })?;
    wait_for_completion(&mut *session, None)?;
    assert_eq!(session.fetch(10)?.rows[0][1].as_deref(), Some("Odd Table"));
    session.execute_metadata(&MetadataRequest::Columns {
        schema: schema.clone(),
        relation: None,
    })?;
    wait_for_completion(&mut *session, None)?;
    let rows = session.fetch(10)?.rows;
    assert_eq!(rows[0][2].as_deref(), Some("Odd Column"));
    assert_eq!(rows[0][3].as_deref(), Some("numeric(10,2)"));
    assert_eq!(rows[0][4].as_deref(), Some("amount"));
    complete(&mut *session, &format!("DROP SCHEMA \"{schema}\" CASCADE"))?;
    assert!(session.columns()?.is_empty());
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn verifies_tls_and_rejects_wrong_passwords_databases_and_settings() -> Result<()> {
    let ca = std::fs::read(std::env::var("QROW_POSTGRES_CA")?)?;
    let mut p = profile();
    p.tls = true;
    let connector = PostgresConnector::new(Trust::from_pem(&ca)?);
    let mut session = connector.connect(&p, Secret::password("qrow-test-password"))?;
    complete(
        &mut *session,
        "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
    )?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("t"));
    assert!(
        PostgresConnector::default()
            .connect(&p, Secret::password("qrow-test-password"))
            .is_err()
    );
    assert!(connector.connect(&p, Secret::password("wrong")).is_err());
    p.database = "qrow_missing_database".into();
    assert!(
        connector
            .connect(&p, Secret::password("qrow-test-password"))
            .err()
            .unwrap()
            .to_string()
            .contains("qrow_missing_database")
    );
    p.database = "qrow".into();
    p.parameters.insert("invalid_parameter".into(), "x".into());
    assert!(
        connector
            .connect(&p, Secret::password("qrow-test-password"))
            .is_err()
    );
    p.authentication = Authentication::Oidc {
        sign_in: uuid::Uuid::new_v4(),
    };
    assert!(
        connector
            .connect(&p, Secret::password("qrow-test-password"))
            .is_err()
    );
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn immediate_cancellation_prevents_or_stops_submission() -> Result<()> {
    let mut session = connect();
    for _ in 0..10 {
        let cancel = session.execute("SELECT pg_sleep(30)")?;
        cancel.cancel()?;
        assert_eq!(
            wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(3)))?,
            Completion::Cancelled
        );
    }
    complete(&mut *session, "SELECT 42")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn oversized_rows_and_excess_rows_report_result_limits() -> Result<()> {
    let mut session = connect();
    complete(
        &mut *session,
        "SELECT repeat('x', 65 * 1024 * 1024) AS large",
    )?;
    assert!(session.fetch(1000)?.rows.is_empty());
    assert!(session.result_limited());
    complete(&mut *session, "SELECT i FROM generate_series(1,100001) i")?;
    for _ in 0..100 {
        assert_eq!(session.fetch(1000)?.rows.len(), 1000);
    }
    assert!(session.fetch(1000)?.rows.is_empty());
    assert!(session.result_limited());
    complete(&mut *session, "SELECT 42")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn a_limited_partial_page_keeps_its_valid_rows() -> Result<()> {
    let mut session = connect();
    complete(
        &mut *session,
        "SELECT repeat('x',100000) FROM generate_series(1,1000)",
    )?;
    let rows = session.fetch(1000)?.rows;
    assert!(!rows.is_empty());
    assert!(rows.len() < 1000);
    assert!(!session.result_limited());
    assert!(session.fetch(1000)?.rows.is_empty());
    assert!(session.result_limited());
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn retained_cancellation_handles_are_safe_after_session_close_or_drop() -> Result<()> {
    for explicit_close in [false, true] {
        let mut session = connect();
        let cancellation = session.execute("SELECT pg_sleep(30)")?;
        if explicit_close {
            session.close()?;
        }
        drop(session);
        cancellation.cancel()?;
    }
    Ok(())
}

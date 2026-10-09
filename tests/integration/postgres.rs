use anyhow::Result;
use qrow::{
    connector::{
        Completion, Connector, DatabaseConnector, MetadataRequest, QueryError, QueryState, Secret,
        Session, postgres::PostgresConnector, wait_for_completion, wait_for_result,
    },
    model::{Authentication, DatabaseType, PostgresSslMode, Profile},
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
fn explicit_tls_modes_encrypt_without_a_trust_store_and_keep_verification_available() -> Result<()>
{
    let mut p = profile();
    for (mode, expected) in [
        (PostgresSslMode::Require, "t"),
        (PostgresSslMode::Disable, "f"),
    ] {
        p.postgres_ssl_mode = Some(mode);
        let mut session =
            PostgresConnector::default().connect(&p, Secret::password("qrow-test-password"))?;
        complete(
            &mut *session,
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
        )?;
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some(expected));
    }
    p.postgres_ssl_mode = Some(PostgresSslMode::VerifyFull);
    assert!(
        PostgresConnector::default()
            .connect(&p, Secret::password("qrow-test-password"))
            .is_err()
    );
    let ca = std::fs::read(std::env::var("QROW_POSTGRES_CA")?)?;
    let mut session = PostgresConnector::new(Trust::from_pem(&ca)?)
        .connect(&p, Secret::password("qrow-test-password"))?;
    complete(
        &mut *session,
        "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
    )?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("t"));
    Ok(())
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
    for iteration in 0..30 {
        let cancel = session.execute("SELECT pg_sleep(30)")?;
        cancel.cancel()?;
        assert_eq!(
            wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(3)))?,
            Completion::Cancelled
        );
        assert_eq!(session.poll()?, QueryState::Cancelled);
        assert_eq!(
            complete(&mut *session, "SELECT 42")?,
            Completion::Finished { has_results: true },
            "successor of cancelled query {iteration}"
        );
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
        // A handle retained by another thread must not affect successor work.
        session.execute("SELECT 43")?;
        cancel.cancel()?;
        assert_eq!(
            wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(3)))?,
            Completion::Finished { has_results: true }
        );
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("43"));
    }
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn cancellation_preserves_session_state_and_each_tls_mode() -> Result<()> {
    let ca = std::fs::read(std::env::var("QROW_POSTGRES_CA")?)?;
    let connector = PostgresConnector::new(Trust::from_pem(&ca)?);
    let mut observer = connect();
    for mode in [
        PostgresSslMode::Disable,
        PostgresSslMode::Require,
        PostgresSslMode::VerifyFull,
    ] {
        let mut p = profile();
        p.host = "localhost".into();
        p.postgres_ssl_mode = Some(mode);
        let mut session = connector.connect(&p, Secret::password("qrow-test-password"))?;
        complete(
            &mut *session,
            "CREATE TEMPORARY TABLE cancellation_state AS SELECT pg_backend_pid() AS pid",
        )?;
        complete(&mut *session, "SELECT pid FROM cancellation_state")?;
        let pid = session.fetch(1)?.rows[0][0].clone().unwrap();
        let cancel = session.execute("SELECT pg_sleep(30) AS cancelled_work")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            complete(
                &mut *observer,
                &format!(
                    "SELECT count(*) FROM pg_stat_activity WHERE pid = {pid} AND wait_event = 'PgSleep'"
                ),
            )?;
            if observer.fetch(1)?.rows[0][0].as_deref() == Some("1") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "query did not reach pg_sleep with {mode:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        cancel.cancel()?;
        assert_eq!(
            wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(3)))?,
            Completion::Cancelled
        );
        assert_eq!(
            complete(
                &mut *session,
                "SELECT pid = pg_backend_pid() FROM cancellation_state"
            )?,
            Completion::Finished { has_results: true }
        );
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("t"));
        // Retained handles cannot cancel a new query, even on the same backend.
        session.execute("SELECT 43 AS value, pg_sleep(0.25)")?;
        cancel.cancel()?;
        assert_eq!(
            wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(3)))?,
            Completion::Finished { has_results: true }
        );
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("43"));
    }
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
    session.execute_export("SELECT pg_backend_pid(), 42")?;
    wait_for_result(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(10)),
    )?;
    assert_eq!(session.fetch(1)?.rows[0][1].as_deref(), Some("42"));
    assert!(session.fetch(1)?.rows.is_empty());
    session.finish_execution()?;
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn export_resolves_custom_type_names_without_collecting_their_members() -> Result<()> {
    let mut session = connect();
    complete(&mut *session, "CREATE TEMP TABLE custom_type_owner (i int)")?;
    complete(&mut *session, "BEGIN")?;
    let labels = (0..2000)
        .map(|i| format!("'label_{i}'"))
        .collect::<Vec<_>>()
        .join(",");
    complete(
        &mut *session,
        &format!("CREATE TYPE pg_temp.export_enum AS ENUM ({labels})"),
    )?;
    complete(
        &mut *session,
        "CREATE TYPE pg_temp.export_composite AS (value pg_temp.export_enum)",
    )?;
    session.execute_export("SELECT 'label_1999'::pg_temp.export_enum AS enum_value, ROW('label_1'::pg_temp.export_enum)::pg_temp.export_composite AS composite_value")?;
    wait_for_result(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(10)),
    )?;
    let columns = session.columns()?;
    assert_eq!(columns[0].data_type, "export_enum");
    assert_eq!(columns[1].data_type, "export_composite");
    assert_eq!(
        session.fetch(1)?.rows[0],
        vec![Some("label_1999".into()), Some("(label_1)".into())]
    );
    assert!(session.fetch(1)?.rows.is_empty());
    session.finish_execution()?;
    complete(&mut *session, "ROLLBACK")?;
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

fn advisory_key() -> u64 {
    (uuid::Uuid::new_v4().as_u128() as u64) & i64::MAX as u64
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn preview_and_export_publish_rows_before_eof_and_defer_session_reuse() -> Result<()> {
    for export in [false, true] {
        let mut observer = connect();
        let key = advisory_key();
        complete(&mut *observer, &format!("SELECT pg_advisory_lock({key})"))?;
        let mut session = connect();
        complete(
            &mut *session,
            "CREATE FUNCTION pg_temp.export_tail(i int, key bigint) RETURNS bool LANGUAGE plpgsql AS $$ BEGIN IF i = 1001 THEN RAISE NOTICE 'flush first page'; PERFORM pg_advisory_lock(key); END IF; RETURN true; END $$",
        )?;
        let sql =
            format!("SELECT i, pg_temp.export_tail(i, {key}) FROM generate_series(1, 1001) i");
        if export {
            session.execute_export(&sql)?;
        } else {
            session.execute(&sql)?;
        }
        assert_eq!(
            wait_for_result(
                &mut *session,
                Some(Instant::now() + Duration::from_secs(10))
            )?,
            QueryState::Streaming { has_results: true }
        );
        let page = session.fetch(1000)?.rows;
        assert_eq!(page.len(), 1000);
        assert_eq!(page[0][0].as_deref(), Some("1"));
        assert_eq!(page[999][0].as_deref(), Some("1000"));
        assert_eq!(session.poll()?, QueryState::Streaming { has_results: true });
        assert!(session.execute_keep_alive("SELECT 1").is_err());
        complete(&mut *observer, &format!("SELECT pg_advisory_unlock({key})"))?;
        assert_eq!(session.fetch(1000)?.rows[0][0].as_deref(), Some("1001"));
        assert!(session.fetch(1000)?.rows.is_empty());
        assert_eq!(
            session.finish_execution()?,
            Completion::Finished { has_results: true }
        );
        complete(&mut *session, "SELECT 42")?;
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("42"));
    }
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn direct_export_exceeds_both_preview_limits_in_order() -> Result<()> {
    let mut session = connect();
    for (total, width) in [(100123, 0), (2000, 40000)] {
        session.execute_export(&format!(
            "SELECT i, repeat('x', {width}) FROM generate_series(1, {total}) i"
        ))?;
        wait_for_result(
            &mut *session,
            Some(Instant::now() + Duration::from_secs(10)),
        )?;
        let mut count = 0;
        loop {
            let batch = session.fetch(1000)?.rows;
            if batch.is_empty() {
                break;
            }
            for row in batch {
                count += 1;
                assert_eq!(row[0].as_deref(), Some(count.to_string().as_str()));
                let text = row[1].as_ref().unwrap();
                assert_eq!(text.len(), width);
                assert!(text.bytes().all(|byte| byte == b'x'));
            }
        }
        assert_eq!(count, total);
        assert!(!session.result_limited());
        assert_eq!(
            session.finish_execution()?,
            Completion::Finished { has_results: true }
        );
        session.close_operation()?;
    }
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn export_keeps_temp_tables_search_path_settings_and_uncommitted_data() -> Result<()> {
    let mut session = connect();
    complete(
        &mut *session,
        "CREATE TEMP TABLE export_state (pid int, amount numeric(20,5))",
    )?;
    complete(&mut *session, "SET search_path = pg_temp")?;
    complete(&mut *session, "SET DateStyle = 'SQL, DMY'")?;
    complete(&mut *session, "BEGIN")?;
    complete(
        &mut *session,
        "INSERT INTO export_state VALUES (pg_backend_pid(), 123456789012345.12345)",
    )?;
    session.execute_export("SELECT pid = pg_backend_pid() AS same_backend, amount, DATE '2026-10-09' AS day FROM export_state")?;
    wait_for_result(&mut *session, None)?;
    assert_eq!(session.columns()?[1].data_type, "numeric(20,5)");
    assert_eq!(
        session.export_context().postgres.unwrap().date_style,
        "SQL, DMY"
    );
    assert_eq!(
        session.fetch(1000)?.rows,
        [vec![
            Some("t".into()),
            Some("123456789012345.12345".into()),
            Some("09/10/2026".into())
        ]]
    );
    assert!(session.fetch(1000)?.rows.is_empty());
    session.finish_execution()?;
    complete(&mut *session, "ROLLBACK")?;
    complete(
        &mut *session,
        "SELECT count(*), current_setting('DateStyle') FROM export_state",
    )?;
    assert_eq!(
        session.fetch(1)?.rows[0],
        [Some("0".into()), Some("SQL, DMY".into())]
    );
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn cancelling_a_blocked_export_cleans_up_and_preserves_failed_transaction_state() -> Result<()> {
    for transaction in [false, true] {
        let mut session = connect();
        if transaction {
            complete(&mut *session, "BEGIN")?;
        }
        let cancel = session
            .execute_export("SELECT i, repeat('x', 1024) FROM generate_series(1, 1000000) i")?;
        wait_for_result(
            &mut *session,
            Some(Instant::now() + Duration::from_secs(10)),
        )?;
        // The bounded channel has no consumer. Cancellation must still progress.
        cancel.cancel_with_deadline(Instant::now() + Duration::from_secs(2))?;
        assert_eq!(session.finish_execution()?, Completion::Cancelled);
        session.close_operation()?;
        if transaction {
            assert!(
                complete(&mut *session, "SELECT 42")
                    .unwrap_err()
                    .to_string()
                    .starts_with("25P02:")
            );
            complete(&mut *session, "ROLLBACK")?;
        }
        session.execute("SELECT 43")?;
        cancel.cancel()?;
        wait_for_completion(&mut *session, Some(Instant::now() + Duration::from_secs(5)))?;
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("43"));
    }
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn export_cell_limit_stops_before_ownership_and_normal_mode_restores_its_ceiling() -> Result<()> {
    let mut session = connect();
    complete(
        &mut *session,
        "CREATE TEMP TABLE buffer_owner AS SELECT pg_backend_pid() AS pid",
    )?;
    session.execute_export("SELECT repeat('x', 9 * 1024 * 1024)")?;
    let readiness = wait_for_result(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(10)),
    );
    let error = match readiness {
        Ok(_) => session.fetch(1).unwrap_err(),
        Err(error) => error,
    };
    assert!(error.to_string().contains("8 MiB export limit"));
    session.close_operation()?;
    complete(&mut *session, "SELECT repeat('x', 65 * 1024 * 1024)")?;
    assert!(session.fetch(1)?.rows.is_empty());
    assert!(session.result_limited());
    session.execute_export("SELECT 42, pid = pg_backend_pid() FROM buffer_owner")?;
    wait_for_result(
        &mut *session,
        Some(Instant::now() + Duration::from_secs(10)),
    )?;
    let row = session.fetch(1)?.rows.remove(0);
    assert_eq!(row[0].as_deref(), Some("42"));
    assert_eq!(row[1].as_deref(), Some("t"));
    assert!(session.fetch(1)?.rows.is_empty());
    session.finish_execution()?;
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn rejected_parameter_descriptions_do_not_leak_prepared_statements() -> Result<()> {
    let mut session = connect();
    let parameters = (1..=4097)
        .map(|index| format!("${index}::pg_catalog.int4"))
        .collect::<Vec<_>>()
        .join(",");
    for _ in 0..3 {
        session.execute_export(&format!("SELECT ARRAY[{parameters}]"))?;
        let error = wait_for_result(
            &mut *session,
            Some(Instant::now() + Duration::from_secs(10)),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("parameter count exceeds 4,096"));
        session.close_operation()?;
        complete(
            &mut *session,
            "SELECT count(*) FROM pg_catalog.pg_prepared_statements",
        )?;
        assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("0"));
    }
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn late_export_failure_wakes_the_writer_before_stalled_cancel_cleanup() -> Result<()> {
    use qrow::{
        export,
        logs::ExecutionId,
        worker::{Event, Worker},
    };
    use std::{
        io::Read,
        net::{Shutdown, TcpListener, TcpStream},
        sync::{Arc, atomic::AtomicBool, mpsc},
    };
    let mut profile = profile();
    profile.postgres_ssl_mode = Some(PostgresSslMode::Disable);
    let upstream = (profile.host.clone(), profile.port);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    profile.port = listener.local_addr()?.port();
    let (cancellation, cancelled_peer) = mpsc::channel();
    let proxy = thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let mut server = TcpStream::connect(upstream).unwrap();
        let mut client_read = client.try_clone().unwrap();
        let mut server_write = server.try_clone().unwrap();
        let outbound = thread::spawn(move || {
            let _ = std::io::copy(&mut client_read, &mut server_write);
            let _ = server_write.shutdown(Shutdown::Both);
        });
        let inbound = thread::spawn(move || {
            let _ = std::io::copy(&mut server, &mut client);
            let _ = client.shutdown(Shutdown::Both);
        });
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut packet = [0; 16];
        peer.read_exact(&mut packet).unwrap();
        assert_eq!(
            u32::from_be_bytes(packet[4..8].try_into().unwrap()),
            80877102
        );
        cancellation.send(Instant::now()).unwrap();
        // Accept the captured CancelRequest, but withhold its required EOF.
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
        outbound.join().unwrap();
        inbound.join().unwrap();
    });
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(PostgresConnector::default()),
        Arc::new(|_| Ok(Secret::password("qrow-test-password"))),
    );
    let jobs = export::Jobs::default();
    let flag = Arc::new(AtomicBool::new(false));
    let download = worker.run_and_export(profile, "SELECT i, CASE WHEN i = 1001 THEN repeat('x', 9 * 1024 * 1024) ELSE 'small' END FROM generate_series(1, 1001) i".into(), ExecutionId(124), None, &jobs, flag.clone())?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("preserved.csv");
    std::fs::write(&path, "previous output")?;
    let output = path.clone();
    let source = download;
    let cancel = flag.clone();
    let (written, result) = mpsc::channel();
    let guard = jobs.register(flag);
    let writer = thread::spawn(move || {
        let _guard = guard;
        let result = source.wait_spool().and_then(|spool| {
            export::save(&output, &cancel, |out| {
                export::stream::write(out, &spool, &export::Settings::default(), &cancel)
            })
        });
        written.send(result).unwrap();
    });
    let stopped = cancelled_peer.recv_timeout(Duration::from_secs(15))?;
    assert!(result.recv_timeout(Duration::from_millis(1200))?.is_err());
    assert!(stopped.elapsed() < Duration::from_millis(1500));
    assert_eq!(std::fs::read_to_string(&path)?, "previous output");
    loop {
        if let Event::DownloadFailed {
            message,
            disconnected,
            ..
        } = worker.events.recv_timeout(Duration::from_secs(5))?
        {
            assert!(message.contains("8 MiB export limit"), "{message}");
            assert!(disconnected);
            break;
        }
    }
    assert!(stopped.elapsed() < Duration::from_secs(5));
    writer.join().unwrap();
    worker.shutdown();
    worker.wait_for_shutdown(Duration::from_secs(5));
    proxy.join().unwrap();
    assert_eq!(jobs.active_count(), 0);
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn worker_exports_its_same_session_table_and_replays_after_new_sql() -> Result<()> {
    use qrow::{
        export,
        logs::ExecutionId,
        worker::{Event, Worker},
    };
    use std::sync::{Arc, atomic::AtomicBool};
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(PostgresConnector::default()),
        Arc::new(|_| Ok(Secret::password("qrow-test-password"))),
    );
    let profile = profile();
    worker.run(profile.clone(), "CREATE TEMP TABLE export_worker AS SELECT i, 12345678901234567890.12345::numeric AS amount FROM generate_series(1, 100123) i".into());
    loop {
        match worker.events.recv_timeout(Duration::from_secs(10))? {
            Event::Ready { .. } => break,
            Event::Error { message, .. } => anyhow::bail!(message),
            _ => {}
        }
    }
    let jobs = export::Jobs::default();
    let flag = Arc::new(AtomicBool::new(false));
    let download = worker.run_and_export(
        profile.clone(),
        "SELECT i, amount FROM export_worker ORDER BY i".into(),
        ExecutionId(123),
        worker.session_generation(&profile),
        &jobs,
        flag.clone(),
    )?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("all.csv");
    let output = path.clone();
    let source = download.clone();
    let cancel = flag.clone();
    let guard = jobs.register(flag);
    let writer = thread::spawn(move || {
        let _guard = guard;
        let spool = source.wait_spool()?;
        export::save(&output, &cancel, |out| {
            export::stream::write(out, &spool, &export::Settings::default(), &cancel)
        })
    });
    let mut preview = 0;
    let mut last = None;
    loop {
        match worker.events.recv_timeout(Duration::from_secs(20))? {
            Event::PreviewRows { rows, .. } => {
                preview += rows.len();
                last = rows.get(rows.len() - 1).unwrap()[0].clone();
            }
            Event::PreviewComplete { complete, .. } => assert!(!complete),
            Event::Downloaded { spool, .. } => {
                assert_eq!(
                    spool.status(),
                    export::spool::Status::Complete { rows: 100123 }
                );
                break;
            }
            Event::DownloadFailed { message, .. } | Event::Error { message, .. } => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
    assert_eq!(preview, 1000);
    assert_eq!(last.as_deref(), Some("1000"));
    assert_eq!(
        worker
            .logs
            .try_iter()
            .filter(|event| event.execution_id == Some(ExecutionId(123))
                && event.kind == qrow::logs::LogKind::ExecutionStarted)
            .count(),
        1
    );
    assert_eq!(writer.join().unwrap()?, 100123);
    let mut count = 0;
    for (index, row) in csv::Reader::from_path(&path)?.records().enumerate() {
        let row = row?;
        assert_eq!(&row[0], (index + 1).to_string());
        assert_eq!(&row[1], "12345678901234567890.12345");
        count += 1;
    }
    assert_eq!(count, 100123);
    worker.run(profile, "SELECT 43".into());
    download.cancel(); // A completed download must not cancel its successor.
    loop {
        match worker.events.recv_timeout(Duration::from_secs(10))? {
            Event::Rows(rows) => assert_eq!(rows[0][0].as_deref(), Some("43")),
            Event::Ready { .. } => break,
            Event::Error { message, .. } => anyhow::bail!(message),
            _ => {}
        }
    }
    let replay = download.spool().unwrap();
    let cancel = AtomicBool::new(false);
    let retry = directory.path().join("retry.csv");
    assert_eq!(
        export::save(&retry, &cancel, |out| export::stream::write(
            out,
            &replay,
            &export::Settings::default(),
            &cancel
        ))?,
        100123
    );
    assert_eq!(std::fs::read(path)?, std::fs::read(retry)?);
    worker.shutdown();
    worker.wait_for_shutdown(Duration::from_secs(5));
    assert_eq!(jobs.active_count(), 0);
    Ok(())
}

#[test]
#[ignore = "needs the server fixture: ./qtest run postgres"]
fn export_captures_type_modifiers_and_settings_without_changing_the_session() -> Result<()> {
    let mut session = connect();
    complete(&mut *session, "SET DateStyle = 'SQL, DMY'")?;
    complete(&mut *session, "SET IntervalStyle = 'sql_standard'")?;
    complete(&mut *session, "SET TimeZone = 'Europe/Paris'")?;
    complete(
        &mut *session,
        "SELECT 1.20::numeric(18,2), 1000::numeric(3,-2), 0.00123::numeric(3,5), TIMESTAMP(3) '2026-10-09 01:02:03.123', TIMESTAMPTZ(6) '2026-10-09 01:02:03+02', DATE '2026-10-09'",
    )?;
    let columns = session.columns()?;
    assert_eq!(
        columns
            .iter()
            .map(|column| column.data_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "numeric(18,2)",
            "numeric(3,-2)",
            "numeric(3,5)",
            "timestamp(3)",
            "timestamptz(6)",
            "date"
        ]
    );
    let context = session.export_context();
    assert!(!context.iso_dates());
    let settings = context.postgres.unwrap();
    assert_eq!(settings.date_style, "SQL, DMY");
    assert_eq!(settings.interval_style, "sql_standard");
    assert_eq!(settings.time_zone, "Europe/Paris");
    assert_eq!(session.fetch(1)?.rows[0][5].as_deref(), Some("09/10/2026"));
    complete(&mut *session, "BEGIN")?;
    assert!(complete(&mut *session, "SELECT missing_column").is_err());
    complete(&mut *session, "ROLLBACK")?;
    complete(
        &mut *session,
        "SELECT current_setting('DateStyle'), current_setting('IntervalStyle'), current_setting('TimeZone')",
    )?;
    assert_eq!(
        session.fetch(1)?.rows[0],
        vec![
            Some("SQL, DMY".into()),
            Some("sql_standard".into()),
            Some("Europe/Paris".into())
        ]
    );
    Ok(())
}

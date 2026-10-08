use super::*;
use serde_json::json;
use std::sync::Arc;
#[path = "../../support/trino_protocol.rs"]
mod fixture;
use fixture::{Reply, Server, done};
#[test]
fn pages_use_the_latest_cursor_and_preserve_values_and_session_headers() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({"nextUri":"{origin}/next/1"}))
                .header("X-Trino-Set-Schema", "new_schema")
                .header("X-Trino-Set-Session", "name=a%2Cb%3Dc"),
            Reply::page(
                json!({"nextUri":"{origin}/next/2","columns":[{"name":"value","type":"varchar"}],"data":[[null],[""],["α🦀"]]}),
            ),
            Reply::page(json!({"data":[[12345678901234567890u64],[true],[[1,2]]]})),
            done(),
        ],
    );
    let mut session = server.connect();
    complete(&mut *session, "SELECT value")?;
    assert_eq!(
        session.fetch(2)?.rows,
        vec![vec![None], vec![Some("".into())]]
    );
    assert_eq!(
        session.fetch(10)?.rows,
        vec![
            vec![Some("α🦀".into())],
            vec![Some("12345678901234567890".into())],
            vec![Some("true".into())],
            vec![Some("[1,2]".into())]
        ]
    );
    complete(&mut *session, "SELECT 2")?;
    let requests = server.requests();
    assert!(requests[2].starts_with("GET /next/1 "));
    assert!(requests[3].starts_with("GET /next/2 "));
    let last = requests.last().unwrap().to_lowercase();
    assert!(last.contains("x-trino-schema: new_schema\r\n"));
    assert!(last.contains("x-trino-session: name=a%2cb%3dc\r\n"));
    assert!(
        !requests
            .iter()
            .any(|request| request.to_lowercase().contains("authorization:"))
    );
    session.close()
}

#[test]
fn verified_https_sends_password_or_refreshable_bearer_tokens() -> Result<()> {
    use qrow::connector::TokenSource;
    use zeroize::Zeroizing;
    struct Tokens;
    impl TokenSource for Tokens {
        fn access_token(&self) -> Result<Zeroizing<String>> {
            Ok(Zeroizing::new("synthetic-token".into()))
        }
    }
    let server = Server::new(true, vec![done(), done(), done()]);
    let mut session = server.connect();
    complete(&mut *session, "SELECT 2")?;
    session.close()?;
    let mut token_session = DatabaseConnector::new(crate::oidc_provider::trust())
        .connect(&server.profile, Secret::Token(Arc::new(Tokens)))?;
    token_session.close()?;
    let requests = server.requests();
    assert!(requests[0].to_lowercase().contains("authorization: basic "));
    assert!(requests[2].contains("synthetic-token"));
    let mut plain = server.profile.clone();
    plain.tls = false;
    assert!(
        DatabaseConnector::default()
            .connect(&plain, Secret::password("must-not-leave-client"))
            .is_err()
    );
    assert!(
        DatabaseConnector::default()
            .connect(&plain, Secret::Token(Arc::new(Tokens)))
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    Ok(())
}

#[test]
fn cancellation_deletes_the_current_query_and_old_handles_do_not_target_new_work() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({"nextUri":"{origin}/query/one"})),
            Reply::page(json!({"nextUri":"{origin}/query/two"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
        ],
    );
    let mut session = server.connect();
    let cancel = session.execute("SELECT slow")?;
    assert_eq!(session.poll()?, QueryState::Running);
    cancel.cancel()?;
    assert_eq!(session.poll()?, QueryState::Cancelled);
    complete(&mut *session, "SELECT 42")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("1"));
    let requests = server.requests();
    assert!(requests[3].starts_with("DELETE /query/two "));
    assert!(requests[4].starts_with("DELETE /query/two "));
    assert!(requests[5].starts_with("POST /v1/statement "));
    session.close()
}

#[test]
fn rejects_off_origin_cursors_redirects_bad_json_and_oversized_responses() {
    for reply in [
        Reply::page(json!({"nextUri":"http://example.test/steal"})),
        Reply {
            status: 302,
            body: String::new(),
            headers: "Location: http://example.test/steal\r\n".into(),
        },
        Reply {
            status: 200,
            body: "invalid-json".into(),
            headers: String::new(),
        },
        Reply {
            status: 200,
            body: "x".repeat(16 * 1024 * 1024 + 1),
            headers: String::new(),
        },
    ] {
        let server = Server::new(false, vec![reply]);
        assert!(
            DatabaseConnector::default()
                .connect(&server.profile, Secret::password(""))
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn sql_errors_keep_the_session_and_multiple_statements_never_reach_the_server() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(
                json!({"error":{"errorName":"SYNTAX_ERROR","message":"Synthetic SQL error"}}),
            ),
            done(),
        ],
    );
    let mut session = server.connect();
    assert!(session.execute("SELECT 1; SELECT 2").is_err());
    assert_eq!(server.requests().len(), 1);
    let error = session.execute("SELECT broken").err().unwrap();
    assert!(error.downcast_ref::<QueryError>().is_some());
    complete(&mut *session, "SELECT 3; -- trailing comment")?;
    assert!(server.requests().last().unwrap().ends_with("SELECT 3"));
    session.close()
}

#[test]
fn response_headers_preserve_and_clear_transactions_roles_and_prepared_statements() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({}))
                .header("X-Trino-Started-Transaction-Id", "test-transaction")
                .header("X-Trino-Set-Role", "tpch=ROLE%7Breader%7D")
                .header("X-Trino-Added-Prepare", "answer=SELECT+7"),
            done(),
            Reply::page(json!({}))
                .header("X-Trino-Clear-Transaction-Id", "true")
                .header("X-Trino-Deallocated-Prepare", "answer")
                .header("X-Trino-Clear-Session", "setting"),
            done(),
        ],
    );
    let mut profile = server.profile.clone();
    profile.parameters.insert("setting".into(), "a,b=c".into());
    let mut session = DatabaseConnector::default().connect(&profile, Secret::password(""))?;
    complete(&mut *session, "START TRANSACTION")?;
    complete(&mut *session, "EXECUTE answer")?;
    complete(&mut *session, "ROLLBACK")?;
    complete(&mut *session, "SELECT 3")?;
    let requests = server.requests();
    let inside = requests[2].to_lowercase();
    let after = requests[4].to_lowercase();
    assert!(inside.contains("x-trino-transaction-id: test-transaction"));
    assert!(inside.contains("x-trino-role: tpch=role%7breader%7d"));
    assert!(inside.contains("x-trino-prepared-statement: answer=select+7"));
    assert!(inside.contains("x-trino-session: setting=a%2cb%3dc"));
    assert!(after.contains("x-trino-transaction-id: none"));
    assert!(!after.contains("x-trino-prepared-statement:"));
    assert!(!after.contains("x-trino-session:"));
    session.close()
}

#[test]
fn disconnect_rolls_back_an_open_transaction() -> Result<()> {
    let server = Server::new(
        false,
        vec![
            done(),
            Reply::page(json!({})).header("X-Trino-Started-Transaction-Id", "test-transaction"),
            Reply::page(json!({})).header("X-Trino-Clear-Transaction-Id", "true"),
        ],
    );
    let mut session = server.connect();
    complete(&mut *session, "START TRANSACTION")?;
    session.close()?;
    assert!(server.requests()[2].ends_with("ROLLBACK"));
    session.close()?;
    assert_eq!(server.requests().len(), 3);
    Ok(())
}

fn challenge(redirect: bool) -> Reply {
    Reply { status: 401, body: String::new(), headers: String::new() }.header("WWW-Authenticate", &format!(
        "Basic realm=\"test, realm\", Bearer realm=\"Trino\", {}x_token_server=\"{{origin}}/token?secret=test\"",
        if redirect { "x_redirect_server=\"{origin}/browser?secret=test\", " } else { "" }))
}
fn external(
    server: &mut Server,
    timeout: Duration,
    browser: qrow::external_auth::Browser,
) -> (qrow::external_auth::Service, Secret) {
    use qrow::model::Authentication;
    server.profile.authentication = Authentication::TrinoExternal;
    let service = qrow::external_auth::Service::with_timeout(Some(browser), timeout);
    service.configure(std::slice::from_ref(&server.profile));
    let secret = service.secret(&server.profile).unwrap();
    (service, secret)
}
#[test]
fn external_initial_pending_token_ack_retry_and_cache_preserve_requests() -> Result<()> {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut server = Server::new(
        true,
        vec![
            challenge(true),
            Reply::page(json!({"nextUri":"{origin}/token/next"})),
            Reply {
                status: 503,
                body: String::new(),
                headers: String::new(),
            },
            Reply::page(json!({"token":"opaque-trino-encrypted-token"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
            done(),
            done(),
        ],
    );
    server.profile.parameters = [("query_max_run_time".into(), "17m".into())].into();
    let opens = Arc::new(AtomicUsize::new(0));
    let counter = opens.clone();
    let (_service, secret) = external(
        &mut server,
        Duration::from_secs(5),
        Arc::new(move |url| {
            assert!(url.contains("/browser?"));
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    let mut session = server.connect_secret(secret.clone())?;
    complete(&mut *session, "SELECT 42")?;
    let _other = server.connect_secret(secret)?;
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    let requests = server.requests();
    assert!(requests[0].starts_with("POST /v1/statement "));
    assert!(!requests[0].contains("authorization:"));
    assert!(requests[1].starts_with("GET /token?"));
    assert!(requests[2].starts_with("GET /token/next "));
    assert!(requests[4].starts_with("DELETE /token/next "));
    for request in &requests[1..=4] {
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("x-trino-user:"));
    }
    let retried = requests[5].replace("authorization: Bearer opaque-trino-encrypted-token\r\n", "");
    assert_eq!(requests[0], retried);
    assert!(requests[6].contains("authorization: Bearer opaque-trino-encrypted-token"));
    Ok(())
}
#[test]
fn external_renewal_has_no_browser_and_rejected_tokens_are_bounded() {
    use std::sync::Arc;
    let mut server = Server::new(
        true,
        vec![
            challenge(false),
            Reply::page(json!({"token":"replacement1"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            challenge(false),
            Reply::page(json!({"token":"replacement2"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            challenge(false),
        ],
    );
    let (_service, secret) = external(
        &mut server,
        Duration::from_secs(3),
        Arc::new(|_| panic!("renewal must not open a browser")),
    );
    let error = server.connect_secret(secret).err().unwrap();
    assert!(error.to_string().contains("rejected the replacement"));
    assert_eq!(server.requests().len(), 7);
}
#[test]
fn external_rejection_and_unsafe_polling_urls_do_not_leak_details() {
    use std::sync::Arc;
    for response in [
        json!({"error":"secret-server-error-token"}),
        json!({"nextUri":"https://attacker.example/token?secret=identifier"}),
        json!({"nextUri":"{origin}/token#secret"}),
        json!({}),
        json!({"token":"bad\r\ntoken"}),
    ] {
        let mut server = Server::new(true, vec![challenge(false), Reply::page(response)]);
        let (_service, secret) =
            external(&mut server, Duration::from_secs(2), Arc::new(|_| Ok(())));
        let error = server.connect_secret(secret).err().unwrap().to_string();
        assert!(!error.contains("secret-server-error-token"));
        assert!(!error.contains("identifier"));
        assert!(!error.contains("attacker.example"));
        assert_eq!(server.requests().len(), 2);
    }
}
#[test]
fn external_overall_timeout_and_cancellation_before_a_cursor() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let mut server = Server::new(
        true,
        vec![
            challenge(false),
            Reply::page(json!({"nextUri":"{origin}/token"})),
        ],
    );
    let (_service, secret) = external(&mut server, Duration::from_millis(60), Arc::new(|_| Ok(())));
    let started = Instant::now();
    let error = server.connect_secret(secret).err().unwrap();
    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut server = Server::new(true, vec![challenge(true)]);
    let cancelled = Arc::new(AtomicBool::new(false));
    let browser_cancel = cancelled.clone();
    let (_service, secret) = external(
        &mut server,
        Duration::from_secs(3),
        Arc::new(move |_| {
            browser_cancel.store(true, Ordering::SeqCst);
            Ok(())
        }),
    );
    let secret = secret.with_control(qrow::external_auth::Control::new(
        Arc::new(move || cancelled.load(Ordering::SeqCst)),
        Arc::new(|_| {}),
    ));
    assert!(
        server
            .connect_secret(secret)
            .err()
            .unwrap()
            .is::<qrow::external_auth::Cancelled>()
    );
    assert!(server.requests().len() <= 2);
    assert!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count()
            == 1
    );
}
#[test]
fn external_cleanup_does_not_start_authentication() -> Result<()> {
    use std::sync::Arc;
    let mut server = Server::new(
        true,
        vec![
            done(),
            Reply::page(json!({"nextUri":"{origin}/query/next"})),
            challenge(true),
        ],
    );
    let (_service, secret) = external(
        &mut server,
        Duration::from_secs(2),
        Arc::new(|_| panic!("cleanup must not open a browser")),
    );
    let mut session = server.connect_secret(secret)?;
    session.execute("SELECT 1")?;
    assert!(session.close_operation().is_err());
    assert_eq!(server.requests().len(), 3);
    Ok(())
}

#[test]
fn external_sign_out_keeps_a_cleanup_credential_without_new_login() -> Result<()> {
    let mut server = Server::new(
        true,
        vec![
            challenge(false),
            Reply::page(json!({"token":"opaque"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
            Reply::page(json!({"nextUri":"{origin}/query/active"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
        ],
    );
    let (service, secret) = external(
        &mut server,
        Duration::from_secs(2),
        Arc::new(|_| panic!("no browser")),
    );
    let mut session = server.connect_secret(secret)?;
    session.execute("SELECT 1")?;
    service.clear(server.profile.id);
    session.close_operation()?;
    let requests = server.requests();
    assert!(requests[5].starts_with("DELETE /query/active "));
    assert!(requests[5].contains("authorization: Bearer opaque"));
    Ok(())
}

#[test]
fn external_settings_invalidation_still_rolls_back_without_authentication() -> Result<()> {
    let mut server = Server::new(
        true,
        vec![
            challenge(false),
            Reply::page(json!({"token":"opaque"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
            done().header("X-Trino-Started-Transaction-Id", "transaction"),
            done().header("X-Trino-Clear-Transaction-Id", "true"),
        ],
    );
    let (service, secret) = external(
        &mut server,
        Duration::from_secs(2),
        Arc::new(|_| panic!("cleanup must not open a browser")),
    );
    let mut session = server.connect_secret(secret)?;
    complete(&mut *session, "START TRANSACTION")?;
    let mut changed = server.profile.clone();
    changed.username = "other-user".into();
    service.configure(&[changed]);
    session.close()?;
    let requests = server.requests();
    assert!(requests[5].ends_with("ROLLBACK"));
    assert!(requests[5].contains("authorization: Bearer opaque"));
    assert_eq!(requests.len(), 6);
    Ok(())
}

#[test]
fn external_rejected_sql_preserves_body_and_updated_session_headers() -> Result<()> {
    let mut server = Server::new(
        true,
        vec![
            done().header("X-Trino-Set-Schema", "updated"),
            challenge(true),
            Reply::page(json!({"token":"opaque"})),
            Reply {
                status: 204,
                body: String::new(),
                headers: String::new(),
            },
            done(),
        ],
    );
    let (_service, secret) = external(&mut server, Duration::from_secs(2), Arc::new(|_| Ok(())));
    let mut session = server.connect_secret(secret)?;
    complete(&mut *session, "SELECT 42")?;
    let requests = server.requests();
    assert!(requests[1].ends_with("SELECT 42"));
    assert!(requests[1].contains("x-trino-schema: updated"));
    assert_eq!(
        requests[1],
        requests[4].replace("authorization: Bearer opaque\r\n", "")
    );
    Ok(())
}
#[test]
fn external_submission_does_not_retry_server_or_network_failure() -> Result<()> {
    for status in [503, 0] {
        // Status zero closes the transport after receiving the complete SQL,
        // which leaves execution uncertain from the client's perspective.
        let mut server = Server::new(
            true,
            vec![
                done(),
                Reply {
                    status,
                    body: String::new(),
                    headers: String::new(),
                },
                done(),
            ],
        );
        let (_service, secret) = external(
            &mut server,
            Duration::from_secs(2),
            Arc::new(|_| panic!("not an authentication rejection")),
        );
        let mut session = server.connect_secret(secret)?;
        assert!(session.execute("SELECT 42").is_err());
        assert_eq!(server.requests().len(), 2);
    }
    Ok(())
}

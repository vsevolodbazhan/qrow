//! Export-only negotiation, ordered commit boundaries, and opaque HTTPS transport.
use super::protocol_fixture as coordinator;
use super::*;
use base64::Engine;
use qrow::model::transfer::{Transfer, TransferPreset, TrinoSpooling};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[path = "../../support/trino_storage.rs"]
mod storage;
use coordinator::{Reply, Server, done};
fn configure(session: &mut dyn Session, mode: TrinoSpooling) -> Result<()> {
    let mut transfer = Transfer {
        preset: TransferPreset::Custom,
        ..Default::default()
    };
    transfer.custom.trino_spooling = mode;
    session.configure_export(&transfer)
}
fn inline(rows: Value, offset: u64, compressed: bool) -> Value {
    let bytes = serde_json::to_vec(&rows).unwrap();
    let wire = if compressed {
        lz4_flex::block::compress(&bytes)
    } else {
        bytes.clone()
    };
    let mut descriptor = json!({"type":"inline","data":base64::engine::general_purpose::STANDARD.encode(&wire),"metadata":{"rowOffset":offset,"rowsCount":rows.as_array().unwrap().len(),"segmentSize":wire.len()}});
    if compressed {
        descriptor["metadata"]["uncompressedSize"] = json!(bytes.len());
    }
    descriptor
}
fn remote(origin: &str, path: &str, bytes: &[u8], offset: u64, rows: u64) -> Value {
    json!({"type":"spooled","uri":format!("{origin}/{path}"),"ackUri":format!("{origin}/ack-{path}"),"headers":{"x-amz-server-side-encryption-customer-key":["synthetic-storage-key"]},"metadata":{"rowOffset":offset.to_string(),"rowsCount":rows.to_string(),"segmentSize":bytes.len().to_string(),"expiresAt":"2099-01-01T00:00:00Z"}})
}
fn page(encoding: &str, segments: Vec<Value>, next: Option<&str>) -> Reply {
    let mut page = json!({"columns":[{"name":"value","type":"bigint"}],"data":{"encoding":encoding,"segments":segments}});
    if let Some(next) = next {
        page["nextUri"] = json!(next);
    }
    Reply::page(page)
}
#[test]
fn inline_json_and_lz4_preserve_empty_boundaries_and_require_commit() -> Result<()> {
    for compressed in [false, true] {
        let encoding = if compressed { "json+lz4" } else { "json" };
        let server = Server::new(
            true,
            vec![
                done(),
                page(
                    encoding,
                    vec![
                        inline(json!([]), 0, compressed),
                        inline(json!([[11], [12]]), 0, compressed),
                        inline(json!([]), 2, compressed),
                    ],
                    None,
                ),
            ],
        );
        let mut session = server.connect();
        configure(&mut *session, TrinoSpooling::Parallel)?;
        session.execute_export("SELECT inline")?;
        assert_eq!(
            qrow::connector::wait_for_result(&mut *session, None)?,
            QueryState::Streaming { has_results: true }
        );
        assert!(session.fetch(1)?.rows.is_empty());
        assert!(session.fetch(1).is_err());
        assert!(session.commit_export_rows(0)?);
        assert_eq!(session.fetch(1)?.rows, vec![vec![Some("11".into())]]);
        assert!(session.commit_export_rows(2).is_err());
        session.commit_export_rows(1)?;
        assert_eq!(session.fetch(10)?.rows, vec![vec![Some("12".into())]]);
        session.commit_export_rows(1)?;
        assert!(session.fetch(1)?.rows.is_empty());
        assert!(session.commit_export_rows(0)?);
        assert!(session.fetch(1)?.rows.is_empty());
        assert!(!session.commit_export_rows(0)?);
        assert_eq!(
            session.finish_execution()?,
            Completion::Finished { has_results: true }
        );
        session.close()?;
        let requests = server.requests();
        assert!(
            !requests[0]
                .to_ascii_lowercase()
                .contains("x-trino-query-data-encoding")
        );
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("x-trino-query-data-encoding: json+lz4,json")
        );
    }
    Ok(())
}
#[test]
fn enabled_export_falls_back_without_resubmission_and_preview_never_negotiates() -> Result<()> {
    let server = Server::new(true, vec![done(), done(), done()]);
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Serial)?;
    session.execute_export("SELECT fallback")?;
    assert_eq!(session.fetch(10)?.rows.len(), 1);
    session.commit_export_rows(1)?;
    assert!(session.fetch(1)?.rows.is_empty());
    session.close_operation()?;
    session.execute("SELECT preview")?;
    qrow::connector::wait_for_completion(&mut *session, None)?;
    session.close()?;
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("x-trino-query-data-encoding")
    );
    assert!(
        !requests[2]
            .to_ascii_lowercase()
            .contains("x-trino-query-data-encoding")
    );
    Ok(())
}
#[test]
fn parallel_prefetch_spans_coordinator_pages_but_never_acks_before_commit() -> Result<()> {
    let storage = storage::Server::new(coordinator::server_config(), |request| {
        if request.starts_with("GET /one ") {
            storage::Reply::bytes(b"[[1],[2]]".to_vec())
        } else if request.starts_with("GET /two ") {
            storage::Reply::bytes(b"[[3]]".to_vec())
        } else if request.starts_with("GET /three ") {
            storage::Reply::bytes(b"[[4]]".to_vec())
        } else {
            storage::Reply::status(204)
        }
    });
    let server = Server::new(
        true,
        vec![
            done(),
            page(
                "json",
                vec![remote(&storage.origin, "one", b"[[1],[2]]", 0, 2)],
                Some("{origin}/two"),
            ),
            page(
                "json",
                vec![remote(&storage.origin, "two", b"[[3]]", 2, 1)],
                Some("{origin}/three"),
            ),
            page(
                "json",
                vec![remote(&storage.origin, "three", b"[[4]]", 3, 1)],
                None,
            ),
        ],
    );
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Parallel)?;
    session.execute_export("SELECT ordered")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("1"));
    storage.wait_for("/two", 1);
    assert!(
        !storage
            .requests()
            .iter()
            .any(|h| h.starts_with("GET /ack-") || h.starts_with("GET /three "))
    );
    session.commit_export_rows(1)?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("2"));
    assert!(
        !storage
            .requests()
            .iter()
            .any(|h| h.starts_with("GET /ack-one "))
    );
    session.commit_export_rows(1)?;
    storage.wait_for("/ack-one", 1);
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("3"));
    session.commit_export_rows(1)?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("4"));
    session.commit_export_rows(1)?;
    assert!(session.fetch(1)?.rows.is_empty());
    session.commit_export_rows(0)?;
    session.close()?;
    let requests = storage.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|h| h.starts_with("GET /ack-"))
            .count(),
        3
    );
    for request in requests {
        let head = request.to_ascii_lowercase();
        assert!(head.contains("x-amz-server-side-encryption-customer-key: synthetic-storage-key"));
        assert!(head.contains("accept-encoding: identity"));
        assert!(
            !head.contains("authorization:")
                && !head.contains("x-trino-")
                && !head.contains("synthetic-password")
        );
    }
    Ok(())
}
#[test]
fn serial_download_waits_for_commit_and_close_does_not_ack_uncommitted_rows() -> Result<()> {
    let storage = storage::Server::new(coordinator::server_config(), |_| {
        storage::Reply::bytes(b"[[9]]".to_vec())
    });
    let server = Server::new(
        true,
        vec![
            done(),
            page(
                "json",
                vec![
                    remote(&storage.origin, "one", b"[[9]]", 0, 1),
                    remote(&storage.origin, "two", b"[[9]]", 1, 1),
                ],
                None,
            ),
        ],
    );
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Serial)?;
    session.execute_export("SELECT no_commit")?;
    assert_eq!(session.fetch(10)?.rows.len(), 1);
    session.close_operation()?;
    assert_eq!(storage.requests().len(), 1);
    assert!(storage.requests()[0].starts_with("GET /one "));
    session.close()
}
#[test]
fn remote_lz4_retries_only_transport_and_ack_failures_are_redacted_warnings() -> Result<()> {
    let bytes = lz4_flex::block::compress(b"[[7],[8]]");
    let wire = bytes.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let storage = storage::Server::new(coordinator::server_config(), move |head| {
        if head.starts_with("GET /data?") {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                storage::Reply::status(503)
            } else {
                storage::Reply::bytes(wire.clone())
            }
        } else {
            storage::Reply::status(403)
        }
    });
    let mut segment = remote(
        &storage.origin,
        "data?signature=opaque-secret",
        &bytes,
        0,
        2,
    );
    segment["metadata"]["uncompressedSize"] = json!(9);
    let server = Server::new(true, vec![done(), page("json+lz4", vec![segment], None)]);
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Serial)?;
    session.execute_export("SELECT retry")?;
    assert_eq!(session.fetch(10)?.rows.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    session.commit_export_rows(2)?;
    let warning = session.export_cleanup_warning().unwrap();
    assert!(warning.contains("1 committed segments"));
    assert!(!warning.contains("opaque-secret") && !warning.contains(&storage.origin));
    session.close()
}
#[test]
fn redirects_drop_storage_headers_across_origins_and_reject_http() -> Result<()> {
    let destination = storage::Server::new(coordinator::server_config(), |_| {
        storage::Reply::bytes(b"[[6]]".to_vec())
    });
    let url = format!("{}/destination", destination.origin);
    let source = storage::Server::new(coordinator::server_config(), move |_| {
        storage::Reply::redirect(url.clone())
    });
    let server = Server::new(
        true,
        vec![
            done(),
            page(
                "json",
                vec![remote(&source.origin, "data", b"[[6]]", 0, 1)],
                None,
            ),
        ],
    );
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Serial)?;
    session.execute_export("SELECT redirect")?;
    assert_eq!(session.fetch(1)?.rows[0][0].as_deref(), Some("6"));
    assert!(source.requests()[0].contains("synthetic-storage-key"));
    assert!(!destination.requests()[0].contains("synthetic-storage-key"));
    session.close()?;
    let source = storage::Server::new(coordinator::server_config(), |_| {
        storage::Reply::redirect("http://localhost:1/leak?signature=secret".into())
    });
    let server = Server::new(
        true,
        vec![
            done(),
            page(
                "json",
                vec![remote(&source.origin, "data", b"[[6]]", 0, 1)],
                None,
            ),
        ],
    );
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Serial)?;
    session.execute_export("SELECT downgrade")?;
    let error = session.fetch(1).unwrap_err().to_string();
    assert!(error.contains("HTTPS"));
    assert!(!error.contains("signature") && !error.contains("secret"));
    session.close()
}
#[test]
fn invalid_sizes_counts_offsets_schemas_and_expiry_fail_without_ack() -> Result<()> {
    let cases = [
        {
            let mut s = inline(json!([[1]]), 0, false);
            s["metadata"]["segmentSize"] = json!(33 * 1024 * 1024);
            s
        },
        inline(json!([[1]]), 1, false),
        {
            let mut s = inline(json!([[1]]), 0, false);
            s["metadata"]["rowsCount"] = json!(2);
            s
        },
        {
            let mut s = inline(json!([[1], [2]]), 0, false);
            s["metadata"]["rowsCount"] = json!(1);
            s
        },
        inline(json!([[1, 2]]), 0, false),
        {
            let mut s = inline(json!([[1]]), 0, false);
            s["metadata"]["expiresAt"] = json!("2000-01-01T00:00:00Z");
            s
        },
        {
            let mut s = inline(json!([[1]]), 0, false);
            s["metadata"]["segmentSize"] = json!(4);
            s
        },
    ];
    for segment in cases {
        let server = Server::new(true, vec![done(), page("json", vec![segment], None)]);
        let mut session = server.connect();
        configure(&mut *session, TrinoSpooling::Serial)?;
        if session.execute_export("SELECT invalid").is_ok() {
            assert!(session.fetch(10).is_err());
        }
        assert!(!server.requests().iter().any(|h| h.starts_with("GET /ack")));
        let _ = session.close();
    }
    for size in [1, 33 * 1024 * 1024] {
        let mut segment = inline(json!([[1]]), 0, true);
        segment["metadata"]["uncompressedSize"] = json!(size);
        let server = Server::new(true, vec![done(), page("json+lz4", vec![segment], None)]);
        let mut session = server.connect();
        configure(&mut *session, TrinoSpooling::Serial)?;
        if session.execute_export("SELECT invalid_lz4").is_ok() {
            assert!(session.fetch(10).is_err());
        }
        let _ = session.close();
    }
    Ok(())
}

#[test]
fn local_expiry_uses_authoritative_http_expiration_without_retry_or_ack() -> Result<()> {
    for (expiry, status, requests) in [
        ("2000-01-01T00:00:00", 403, 1),
        ("2000-01-01T00:00:00", 410, 1),
        ("2000-01-01T00:00:00Z", 200, 0),
    ] {
        let storage = storage::Server::new(coordinator::server_config(), move |_| {
            storage::Reply::status(status)
        });
        let mut segment = remote(&storage.origin, "expired", b"[[1]]", 0, 1);
        segment["metadata"]["expiresAt"] = json!(expiry);
        let server = Server::new(true, vec![done(), page("json", vec![segment], None)]);
        let mut session = server.connect();
        configure(&mut *session, TrinoSpooling::Serial)?;
        session.execute_export("SELECT expired_local")?;
        let error = session.fetch(1).unwrap_err().to_string();
        assert!(error.contains(if requests == 0 {
            "expired"
        } else if status == 403 {
            "403"
        } else {
            "410"
        }));
        assert_eq!(storage.requests().len(), requests);
        assert!(!storage.requests().iter().any(|head| head.contains("ack")));
        session.close()?;
    }
    Ok(())
}
#[test]
fn empty_coordinator_pages_and_all_zero_segments_reach_real_eof() -> Result<()> {
    let server = Server::new(
        true,
        vec![
            done(),
            page("json", vec![], Some("{origin}/empty")),
            page(
                "json",
                vec![inline(json!([]), 0, false)],
                Some("{origin}/last"),
            ),
            page("json", vec![], None),
        ],
    );
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Parallel)?;
    session.execute_export("SELECT empty")?;
    assert!(session.fetch(1)?.rows.is_empty());
    assert!(session.commit_export_rows(0)?);
    assert!(session.fetch(1)?.rows.is_empty());
    assert!(!session.commit_export_rows(0)?);
    assert_eq!(
        session.finish_execution()?,
        Completion::Finished { has_results: true }
    );
    session.close()
}

#[test]
fn oversized_escaped_cell_fails_after_bounded_download_without_ack() -> Result<()> {
    let mut wire = Vec::with_capacity(10 * 1024 * 1024);
    wire.extend_from_slice(b"[[\"\\u0078");
    wire.extend(std::iter::repeat_n(b'x', 9 * 1024 * 1024));
    wire.extend_from_slice(b"\"]]");
    let size = wire.len();
    let storage = storage::Server::new(coordinator::server_config(), move |_| {
        storage::Reply::bytes(wire.clone())
    });
    let mut segment = remote(&storage.origin, "escaped-cell", b"", 0, 1);
    segment["metadata"]["segmentSize"] = json!(size);
    let server = Server::new(true, vec![done(), page("json", vec![segment], None)]);
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Parallel)?;
    session.execute_export("SELECT escaped_cell")?;
    let error = session.fetch(1).unwrap_err().to_string();
    assert!(error.contains("8 MiB"));
    assert_eq!(storage.requests().len(), 1);
    assert!(!storage.requests()[0].contains("ack"));
    session.close()
}
#[test]
fn cancelling_stalled_segment_body_joins_work_after_successful_delete() -> Result<()> {
    let storage = storage::Server::new(coordinator::server_config(), |_| storage::Reply::Stall);
    let server = Server::new(
        true,
        vec![
            done(),
            page(
                "json",
                vec![remote(&storage.origin, "stall", &[0; 100], 0, 1)],
                Some("{origin}/cancel"),
            ),
        ],
    );
    let mut session = server.connect();
    configure(&mut *session, TrinoSpooling::Serial)?;
    let cancel = session.execute_export("SELECT stall")?;
    let reader = std::thread::spawn(move || {
        let result = session.fetch(1);
        (result, session)
    });
    storage.wait_for("/stall", 1);
    let started = Instant::now();
    cancel.cancel()?;
    let (result, mut session) = reader.join().unwrap();
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        server
            .requests()
            .iter()
            .any(|h| h.starts_with("DELETE /cancel "))
    );
    assert!(!storage.requests().iter().any(|h| h.contains("/ack-stall")));
    session.close()
}

#[test]
fn cancellation_during_coordinator_admission_exits_the_fetch_loop_and_allows_reuse() -> Result<()> {
    let origin = Arc::new(std::sync::Mutex::new(String::new()));
    let route = origin.clone();
    let count = Arc::new(AtomicUsize::new(0));
    let server = storage::Server::new(coordinator::server_config(), move |head| {
        if head.starts_with("POST /v1/statement ") {
            let page = if count.fetch_add(1, Ordering::SeqCst) == 1 {
                json!({"columns":[{"name":"value","type":"bigint"}],"data":{"encoding":"json","segments":[]},"nextUri":format!("{}/admission",route.lock().unwrap())})
            } else {
                json!({"columns":[{"name":"value","type":"bigint"}],"data":[[1]]})
            };
            storage::Reply::bytes(serde_json::to_vec(&page).unwrap())
        } else if head.starts_with("GET /admission ") {
            storage::Reply::Stall
        } else {
            storage::Reply::status(204)
        }
    });
    *origin.lock().unwrap() = server.origin.clone();
    let mut profile = Profile {
        database_type: DatabaseType::Trino,
        host: "localhost".into(),
        port: url::Url::parse(&server.origin)?.port().unwrap(),
        username: "qrow".into(),
        tls: true,
        ..Default::default()
    };
    profile.lifecycle.response_timeout_seconds = 10;
    let mut session = DatabaseConnector::new(coordinator::trust())
        .connect(&profile, Secret::password("synthetic-password"))?;
    configure(&mut *session, TrinoSpooling::Parallel)?;
    let cancel = session.execute_export("SELECT blocked_admission")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let result = session.fetch(1);
        tx.send((result, session)).unwrap();
    });
    server.wait_for("/admission", 1);
    cancel.cancel()?;
    let (result, mut session) = rx.recv_timeout(Duration::from_secs(2))?;
    assert!(result.is_err());
    reader.join().unwrap();
    session.close_operation()?;
    session.execute("SELECT reused")?;
    assert_eq!(
        qrow::connector::wait_for_completion(&mut *session, None)?,
        Completion::Finished { has_results: true }
    );
    assert!(
        server
            .requests()
            .iter()
            .any(|head| head.starts_with("DELETE /admission "))
    );
    session.close()
}

//! Local wire-level fixture. No Java, Kyuubi installation, or real credentials required.
use qrow::{
    connector::{
        Connector, MetadataRequest, QueryState, Secret,
        hive::HiveConnector,
        sasl::{FrameReader, FrameWriter, MAX_FRAME},
        t_c_l_i_service::*,
    },
    model::Profile,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use thrift::protocol::*;

struct Peer {
    input: TBinaryInputProtocol<FrameReader<TcpStream>>,
    output: TBinaryOutputProtocol<FrameWriter<TcpStream>>,
    socket: TcpStream,
    sequence: i32,
    method: String,
}

impl Peer {
    fn accept(listener: &TcpListener) -> Self {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        for (status, expected) in [
            (1, b"PLAIN".as_slice()),
            (2, b"\0test-user\0test-password".as_slice()),
        ] {
            let mut header = [0; 5];
            socket.read_exact(&mut header).unwrap();
            assert_eq!(header[0], status);
            let size = u32::from_be_bytes(header[1..].try_into().unwrap());
            let mut payload = vec![0; size as usize];
            socket.read_exact(&mut payload).unwrap();
            assert_eq!(payload, expected);
        }
        socket.write_all(&[5, 0, 0, 0, 0]).unwrap();
        Self {
            input: TBinaryInputProtocol::new(FrameReader::new(socket.try_clone().unwrap()), true),
            socket: socket.try_clone().unwrap(),
            output: TBinaryOutputProtocol::new(FrameWriter::new(socket), true),
            sequence: 0,
            method: String::new(),
        }
    }
    fn read<T: TSerializable>(&mut self, method: &str) -> T {
        let message = self.input.read_message_begin().unwrap();
        assert_eq!(message.name, method);
        assert_eq!(message.message_type, TMessageType::Call);
        self.sequence = message.sequence_number;
        self.method = method.into();
        self.input.read_struct_begin().unwrap();
        assert_eq!(self.input.read_field_begin().unwrap().id, Some(1));
        let request = T::read_from_in_protocol(&mut self.input).unwrap();
        self.input.read_field_end().unwrap();
        assert_eq!(
            self.input.read_field_begin().unwrap().field_type,
            TType::Stop
        );
        self.input.read_struct_end().unwrap();
        self.input.read_message_end().unwrap();
        request
    }
    fn reply(&mut self, value: impl TSerializable) {
        self.output
            .write_message_begin(&TMessageIdentifier::new(
                &self.method,
                TMessageType::Reply,
                self.sequence,
            ))
            .unwrap();
        self.output
            .write_struct_begin(&TStructIdentifier::new("result"))
            .unwrap();
        self.output
            .write_field_begin(&TFieldIdentifier::new("success", TType::Struct, 0))
            .unwrap();
        value.write_to_out_protocol(&mut self.output).unwrap();
        self.output.write_field_end().unwrap();
        self.output.write_field_stop().unwrap();
        self.output.write_struct_end().unwrap();
        self.output.write_message_end().unwrap();
        self.output.flush().unwrap();
    }
}

fn success() -> TStatus {
    TStatus::new(TStatusCode::SUCCESS_STATUS, None, None, None, None)
}
fn operation(results: bool) -> TOperationHandle {
    TOperationHandle::new(
        THandleIdentifier::new(vec![3; 16], vec![4; 16]),
        TOperationType::EXECUTE_STATEMENT,
        results,
        None,
    )
}
fn status(state: TOperationState, results: bool) -> TGetOperationStatusResp {
    TGetOperationStatusResp::new(
        success(),
        Some(state),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(results),
        None,
    )
}
fn profile(port: u16) -> Profile {
    Profile {
        name: "Test".into(),
        host: "127.0.0.1".into(),
        port,
        username: "test-user".into(),
        database: "avia".into(),
        parameters: BTreeMap::from([(
            "kyuubi.engine.share.level.subdomain".into(),
            "fixture".into(),
        )]),
        ..Default::default()
    }
}

fn initialize(peer: &mut Peer) {
    let req: TOpenSessionReq = peer.read("OpenSession");
    assert_eq!(
        req.client_protocol,
        TProtocolVersion::HIVE_CLI_SERVICE_PROTOCOL_V6
    );
    assert_eq!(req.username.as_deref(), Some("test-user"));
    assert!(req.password.is_none());
    assert_eq!(
        req.configuration.unwrap()["kyuubi.engine.share.level.subdomain"],
        "fixture"
    );
    peer.reply(TOpenSessionResp::new(
        success(),
        TProtocolVersion::HIVE_CLI_SERVICE_PROTOCOL_V6,
        Some(TSessionHandle::new(THandleIdentifier::new(
            vec![1; 16],
            vec![2; 16],
        ))),
        None,
    ));
    let req: TExecuteStatementReq = peer.read("ExecuteStatement");
    assert_eq!(req.statement, "USE `avia`");
    assert_eq!(req.run_async, Some(true));
    peer.reply(TExecuteStatementResp::new(
        success(),
        Some(operation(false)),
    ));
    let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
    peer.reply(status(TOperationState::FINISHED_STATE, false));
    let _: TCloseOperationReq = peer.read("CloseOperation");
    peer.reply(TCloseOperationResp::new(success()));
}

fn configuration(peer: &mut Peer, sql: &str, read_value: Option<&str>) {
    let request: TExecuteStatementReq = peer.read("ExecuteStatement");
    assert_eq!(request.statement, sql);
    assert_eq!(request.run_async, Some(false));
    assert!(request.conf_overlay.is_none());
    peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
    let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
    peer.reply(status(TOperationState::FINISHED_STATE, true));
    if let Some(value) = read_value {
        for values in [
            vec![
                vec![qrow::model::transfer::INCREMENTAL_KEY.to_owned()],
                vec![value.to_owned()],
            ],
            vec![vec![], vec![]],
        ] {
            let _: TFetchResultsReq = peer.read("FetchResults");
            peer.reply(TFetchResultsResp::new(
                success(),
                Some(false),
                Some(TRowSet::new(
                    0,
                    vec![],
                    Some(
                        values
                            .into_iter()
                            .map(|values| TColumn::StringVal(TStringColumn::new(values, vec![0])))
                            .collect(),
                    ),
                    None,
                    None,
                )),
            ));
        }
    }
    let _: TCloseOperationReq = peer.read("CloseOperation");
    peer.reply(TCloseOperationResp::new(success()));
}

#[test]
fn export_collect_setting_is_restored_and_adaptive_fetch_preserves_normal_limits() {
    use qrow::model::transfer::{INCREMENTAL_KEY, Transfer, TransferPreset};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        for (sql, overrides, requested, value) in [
            ("SELECT export", true, 3276, "x".repeat(4096)),
            ("SELECT inherit", false, 13107, "small".into()),
            ("SELECT normal", false, 1000, "n".into()),
        ] {
            if overrides {
                configuration(&mut peer, "SET", Some("false"));
                configuration(&mut peer, &format!("SET {INCREMENTAL_KEY}=true"), None);
            }
            let request: TExecuteStatementReq = peer.read("ExecuteStatement");
            assert_eq!(request.statement, sql);
            assert!(request.conf_overlay.is_none());
            peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
            if overrides {
                configuration(&mut peer, &format!("SET {INCREMENTAL_KEY}=false"), None);
                configuration(&mut peer, "SET", Some("false"));
            }
            let _: TGetResultSetMetadataReq = peer.read("GetResultSetMetadata");
            peer.reply(TGetResultSetMetadataResp::new(
                success(),
                Some(TTableSchema::new(vec![TColumnDesc::new(
                    "value".into(),
                    TTypeDesc::new(vec![TTypeEntry::PrimitiveEntry(TPrimitiveTypeEntry::new(
                        TTypeId::STRING_TYPE,
                        None,
                    ))]),
                    0,
                    None,
                )])),
            ));
            let fetch: TFetchResultsReq = peer.read("FetchResults");
            assert_eq!(fetch.max_rows, requested);
            peer.reply(TFetchResultsResp::new(
                success(),
                Some(true),
                Some(TRowSet::new(
                    0,
                    vec![],
                    Some(vec![TColumn::StringVal(TStringColumn::new(
                        vec![value],
                        vec![0],
                    ))]),
                    None,
                    None,
                )),
            ));
            let _: TCloseOperationReq = peer.read("CloseOperation");
            peer.reply(TCloseOperationResp::new(success()));
        }
        let _: TCloseSessionReq = peer.read("CloseSession");
        peer.reply(TCloseSessionResp::new(success()));
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    session
        .configure_export(&Transfer {
            preset: TransferPreset::Conservative,
            ..Default::default()
        })
        .unwrap();
    session.execute_export("SELECT export").unwrap();
    session.columns().unwrap();
    let rows = session.fetch(session.export_fetch_rows()).unwrap().rows;
    assert_eq!(rows[0][0].as_ref().unwrap().len(), 4096);
    assert!(session.export_fetch_rows() < 900);
    session.close_operation().unwrap();
    session.configure_export(&Transfer::default()).unwrap();
    session.execute_export("SELECT inherit").unwrap();
    session.columns().unwrap();
    assert_eq!(
        session.fetch(session.export_fetch_rows()).unwrap().rows[0][0].as_deref(),
        Some("small")
    );
    session.close_operation().unwrap();
    session.execute("SELECT normal").unwrap();
    session.columns().unwrap();
    assert_eq!(
        session.fetch(50_000).unwrap().rows[0][0].as_deref(),
        Some("n")
    );
    session.close_operation().unwrap();
    session.close().unwrap();
    server.join().unwrap();
}

#[test]
fn export_cancellation_deadline_bounds_a_stalled_separate_authentication() {
    use std::{sync::mpsc, time::Instant};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let (stalled, observed) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let _: TExecuteStatementReq = peer.read("ExecuteStatement");
        peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        // Consume PLAIN and credentials, then withhold authentication completion.
        for _ in 0..2 {
            let mut header = [0; 5];
            socket.read_exact(&mut header).unwrap();
            let mut payload = vec![0; u32::from_be_bytes(header[1..].try_into().unwrap()) as usize];
            socket.read_exact(&mut payload).unwrap();
        }
        stalled.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    let cancel = session.execute("SELECT n").unwrap();
    let target = cancel.clone();
    let started = Instant::now();
    let request =
        thread::spawn(move || target.cancel_with_deadline(Instant::now() + Duration::from_secs(2)));
    observed.recv_timeout(Duration::from_secs(1)).unwrap();
    cancel.abort_transport();
    assert!(request.join().unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(cancel);
    drop(session);
    server.join().unwrap();
}

#[test]
fn ldap_session_parameters_async_query_exact_values_and_fetch_exhaustion() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let req: TExecuteStatementReq = peer.read("ExecuteStatement");
        assert_eq!(req.statement, "SELECT amount FROM ledger");
        assert_eq!(req.run_async, Some(true));
        peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
        let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
        peer.reply(status(TOperationState::RUNNING_STATE, true));
        let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
        peer.reply(status(TOperationState::FINISHED_STATE, true));
        let _: TGetResultSetMetadataReq = peer.read("GetResultSetMetadata");
        peer.reply(TGetResultSetMetadataResp::new(
            success(),
            Some(TTableSchema::new(vec![TColumnDesc::new(
                "amount".into(),
                TTypeDesc::new(vec![TTypeEntry::PrimitiveEntry(TPrimitiveTypeEntry::new(
                    TTypeId::DECIMAL_TYPE,
                    None,
                ))]),
                0,
                None,
            )])),
        ));
        let fetch: TFetchResultsReq = peer.read("FetchResults");
        assert_eq!(fetch.max_rows, 250);
        assert_eq!(fetch.orientation, TFetchOrientation::FETCH_NEXT);
        // Deliberately false despite returned rows, matching the Hive compatibility quirk.
        peer.reply(TFetchResultsResp::new(
            success(),
            Some(false),
            Some(TRowSet::new(
                0,
                vec![],
                Some(vec![TColumn::StringVal(TStringColumn::new(
                    vec!["999999999999.123456789".into(), "".into()],
                    vec![2],
                ))]),
                None,
                None,
            )),
        ));
        let _: TFetchResultsReq = peer.read("FetchResults");
        peer.reply(TFetchResultsResp::new(
            success(),
            Some(false),
            Some(TRowSet::new(2, vec![], Some(vec![]), None, None)),
        ));
        let _: TCloseOperationReq = peer.read("CloseOperation");
        peer.reply(TCloseOperationResp::new(success()));
        let _: TCloseSessionReq = peer.read("CloseSession");
        peer.reply(TCloseSessionResp::new(success()));
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    session.execute("SELECT amount FROM ledger").unwrap();
    assert_eq!(session.poll().unwrap(), QueryState::Running);
    assert_eq!(
        session.poll().unwrap(),
        QueryState::Finished { has_results: true }
    );
    assert_eq!(session.columns().unwrap()[0].data_type, "DECIMAL");
    let batch = session.fetch(250).unwrap();
    assert_eq!(
        batch.rows,
        vec![vec![Some("999999999999.123456789".into())], vec![None]]
    );
    assert!(session.fetch(250).unwrap().rows.is_empty());
    session.close().unwrap();
    server.join().unwrap();
}

#[test]
fn cancellation_uses_an_independent_authenticated_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let _: TExecuteStatementReq = peer.read("ExecuteStatement");
        peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
        let mut control = Peer::accept(&listener);
        let cancel: TCancelOperationReq = control.read("CancelOperation");
        assert_eq!(cancel.operation_handle, operation(true));
        control.reply(TCancelOperationResp::new(success()));
        let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
        peer.reply(status(TOperationState::CANCELED_STATE, true));
        let _: TCloseOperationReq = peer.read("CloseOperation");
        peer.reply(TCloseOperationResp::new(success()));
        let _: TCloseSessionReq = peer.read("CloseSession");
        peer.reply(TCloseSessionResp::new(success()));
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    let cancel = session.execute("SELECT long_running_query()").unwrap();
    cancel.cancel().unwrap();
    assert_eq!(session.poll().unwrap(), QueryState::Cancelled);
    session.close().unwrap();
    server.join().unwrap();
}

#[test]
fn dropped_transport_returns_error_without_resubmitting_statement() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let req: TExecuteStatementReq = peer.read("ExecuteStatement");
        assert_eq!(req.statement, "SELECT 1");
        // Close without acknowledging; execution outcome is intentionally unknown.
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    assert!(session.execute("SELECT 1").is_err());
    server.join().unwrap();
}

#[test]
fn wrapped_engine_failure_reconnects_only_on_explicit_run() {
    use qrow::worker::{Event, Worker};
    use std::sync::Arc;
    for during_poll in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = profile(listener.local_addr().unwrap().port());
        let server = thread::spawn(move || {
            let mut peer = Peer::accept(&listener);
            initialize(&mut peer);
            let req: TExecuteStatementReq = peer.read("ExecuteStatement");
            assert_eq!(req.statement, "SELECT original");
            let message = "Error operating ExecuteStatement: org.apache.kyuubi.shaded.thrift.transport.TTransportException: Socket is closed by peer.";
            if during_poll {
                peer.reply(TExecuteStatementResp::new(
                    success(),
                    Some(operation(false)),
                ));
                let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
                let mut response = status(TOperationState::ERROR_STATE, false);
                response.error_message = Some(message.into());
                peer.reply(response);
                let _: TCloseOperationReq = peer.read("CloseOperation");
                peer.reply(TCloseOperationResp::new(success()));
            } else {
                peer.reply(TExecuteStatementResp::new(
                    TStatus::new(
                        TStatusCode::ERROR_STATUS,
                        None,
                        None,
                        None,
                        Some(message.to_owned()),
                    ),
                    None,
                ));
            }
            let _: TCloseSessionReq = peer.read("CloseSession");
            peer.reply(TCloseSessionResp::new(success()));
            let mut peer = Peer::accept(&listener);
            initialize(&mut peer);
            let req: TExecuteStatementReq = peer.read("ExecuteStatement");
            assert_eq!(req.statement, "SELECT explicit_retry");
            peer.reply(TExecuteStatementResp::new(
                success(),
                Some(operation(false)),
            ));
            let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
            peer.reply(status(TOperationState::FINISHED_STATE, false));
            let _: TCloseOperationReq = peer.read("CloseOperation");
            peer.reply(TCloseOperationResp::new(success()));
            let _: TCloseSessionReq = peer.read("CloseSession");
            peer.reply(TCloseSessionResp::new(success()));
        });
        let worker = Worker::with_connector(
            Arc::new(|| {}),
            Arc::new(HiveConnector::default()),
            Arc::new(|_| Ok(Secret::password("test-password"))),
        );
        worker.run(p.clone(), "SELECT original".into());
        loop {
            if let Event::Error {
                disconnected,
                message,
                ..
            } = worker.events.recv_timeout(Duration::from_secs(3)).unwrap()
            {
                assert!(disconnected);
                assert!(message.contains("Socket is closed by peer"));
                break;
            }
        }
        assert!(
            worker
                .events
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        worker.run(p, "SELECT explicit_retry".into());
        loop {
            match worker.events.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::Ready { .. } => break,
                Event::Error { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
        worker.shutdown();
        worker.wait_for_shutdown(Duration::from_secs(3));
        server.join().unwrap();
    }
}

#[test]
fn heartbeat_closes_its_own_operation_and_preserves_the_preview_cursor() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let _: TExecuteStatementReq = peer.read("ExecuteStatement");
        peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
        let req: TExecuteStatementReq = peer.read("ExecuteStatement");
        assert_eq!(req.statement, "SELECT 42");
        let heartbeat = operation(false);
        peer.reply(TExecuteStatementResp::new(
            success(),
            Some(heartbeat.clone()),
        ));
        let req: TGetOperationStatusReq = peer.read("GetOperationStatus");
        assert_eq!(req.operation_handle, heartbeat);
        peer.reply(status(TOperationState::FINISHED_STATE, false));
        let req: TCloseOperationReq = peer.read("CloseOperation");
        assert_eq!(req.operation_handle, heartbeat);
        peer.reply(TCloseOperationResp::new(success()));
        let req: TFetchResultsReq = peer.read("FetchResults");
        assert_eq!(req.operation_handle, operation(true));
        peer.reply(TFetchResultsResp::new(
            success(),
            Some(false),
            Some(TRowSet::new(0, vec![], Some(vec![]), None, None)),
        ));
        let req: TCloseOperationReq = peer.read("CloseOperation");
        assert_eq!(req.operation_handle, operation(true));
        peer.reply(TCloseOperationResp::new(success()));
        let _: TCloseSessionReq = peer.read("CloseSession");
        peer.reply(TCloseSessionResp::new(success()));
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    session.execute("SELECT data").unwrap();
    session.execute_keep_alive("SELECT 42").unwrap();
    assert_eq!(
        session.poll().unwrap(),
        QueryState::Finished { has_results: false }
    );
    session.close_keep_alive().unwrap();
    assert!(session.fetch(250).unwrap().rows.is_empty());
    session.close().unwrap();
    server.join().unwrap();
}

#[test]
fn connector_rejects_aggregate_response_before_reading_oversized_string() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        let _: TOpenSessionReq = peer.read("OpenSession");
        let mut bytes = Vec::new();
        let mut output = TBinaryOutputProtocol::new(&mut bytes, true);
        output
            .write_message_begin(&TMessageIdentifier::new(
                "OpenSession",
                TMessageType::Reply,
                peer.sequence,
            ))
            .unwrap();
        output
            .write_field_begin(&TFieldIdentifier::new("success", TType::Struct, 0))
            .unwrap();
        output
            .write_field_begin(&TFieldIdentifier::new("status", TType::Struct, 1))
            .unwrap();
        output
            .write_field_begin(&TFieldIdentifier::new("statusCode", TType::I32, 1))
            .unwrap();
        output.write_i32(0).unwrap();
        output
            .write_field_begin(&TFieldIdentifier::new("infoMessages", TType::List, 2))
            .unwrap();
        output
            .write_list_begin(&TListIdentifier::new(TType::String, 2))
            .unwrap();
        output.write_string(&"x".repeat(MAX_FRAME / 2)).unwrap();
        // The second string alone fits, but the response including its first
        // string does not. Omit its payload: the error must precede any read.
        output.write_i32((MAX_FRAME / 2) as i32).unwrap();
        for frame in bytes.chunks(64 * 1024) {
            peer.socket
                .write_all(&(frame.len() as u32).to_be_bytes())
                .unwrap();
            peer.socket.write_all(frame).unwrap();
        }
    });
    let error = match HiveConnector::default().connect(&p, Secret::password("test-password")) {
        Ok(_) => panic!("Oversized server response was accepted"),
        Err(error) => error,
    };
    assert!(qrow::connector::error_message(&error).contains("remaining response byte limit"));
    server.join().unwrap();
}

#[test]
fn catalog_requests_send_exact_names_and_replace_the_current_operation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = profile(listener.local_addr().unwrap().port());
    let metadata = |id: u8, kind: TOperationType| {
        TOperationHandle::new(
            THandleIdentifier::new(vec![id; 16], vec![id; 16]),
            kind,
            true,
            None,
        )
    };
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let req: TGetSchemasReq = peer.read("GetSchemas");
        assert_eq!((req.catalog_name, req.schema_name), (None, None));
        peer.reply(TGetSchemasResp::new(
            success(),
            Some(metadata(5, TOperationType::GET_SCHEMAS)),
        ));
        let req: TGetOperationStatusReq = peer.read("GetOperationStatus");
        assert_eq!(
            req.operation_handle,
            metadata(5, TOperationType::GET_SCHEMAS)
        );
        peer.reply(status(TOperationState::FINISHED_STATE, true));
        // The next request closes the previous operation first.
        let req: TCloseOperationReq = peer.read("CloseOperation");
        assert_eq!(
            req.operation_handle,
            metadata(5, TOperationType::GET_SCHEMAS)
        );
        peer.reply(TCloseOperationResp::new(success()));
        let req: TGetTablesReq = peer.read("GetTables");
        assert_eq!(req.catalog_name, None);
        assert_eq!(req.schema_name.as_deref(), Some("sales_eu"));
        assert_eq!(req.table_name.as_deref(), Some("%"));
        assert_eq!(req.table_types, None);
        peer.reply(TGetTablesResp::new(
            success(),
            Some(metadata(6, TOperationType::GET_TABLES)),
        ));
        let _: TCloseOperationReq = peer.read("CloseOperation");
        peer.reply(TCloseOperationResp::new(success()));
        let req: TGetColumnsReq = peer.read("GetColumns");
        assert_eq!(req.schema_name.as_deref(), Some("sales_eu"));
        assert_eq!(req.table_name.as_deref(), Some("daily_orders"));
        assert_eq!(req.column_name.as_deref(), Some("%"));
        peer.reply(TGetColumnsResp::new(
            success(),
            Some(metadata(7, TOperationType::GET_COLUMNS)),
        ));
        let req: TFetchResultsReq = peer.read("FetchResults");
        assert_eq!(
            req.operation_handle,
            metadata(7, TOperationType::GET_COLUMNS)
        );
        peer.reply(TFetchResultsResp::new(
            success(),
            Some(false),
            Some(TRowSet::new(0, vec![], Some(vec![]), None, None)),
        ));
        let req: TCloseOperationReq = peer.read("CloseOperation");
        assert_eq!(
            req.operation_handle,
            metadata(7, TOperationType::GET_COLUMNS)
        );
        peer.reply(TCloseOperationResp::new(success()));
        let req: TGetColumnsReq = peer.read("GetColumns");
        assert_eq!(req.table_name.as_deref(), Some("%"));
        peer.reply(TGetColumnsResp::new(
            TStatus::new(
                TStatusCode::ERROR_STATUS,
                None,
                None,
                None,
                Some("Schema sales_eu not found".into()),
            ),
            None,
        ));
        let _: TCloseSessionReq = peer.read("CloseSession");
        peer.reply(TCloseSessionResp::new(success()));
    });
    let mut session = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .unwrap();
    session.execute_metadata(&MetadataRequest::Schemas).unwrap();
    assert_eq!(
        session.poll().unwrap(),
        QueryState::Finished { has_results: true }
    );
    session
        .execute_metadata(&MetadataRequest::Relations {
            schema: "sales_eu".into(),
            relation: None,
        })
        .unwrap();
    session
        .execute_metadata(&MetadataRequest::Columns {
            schema: "sales_eu".into(),
            relation: Some("daily_orders".into()),
        })
        .unwrap();
    assert!(session.fetch(1000).unwrap().rows.is_empty());
    let error = session
        .execute_metadata(&MetadataRequest::Columns {
            schema: "sales_eu".into(),
            relation: None,
        })
        .err()
        .unwrap();
    // An ordinary server error keeps the session for the next request.
    assert!(
        error
            .downcast_ref::<qrow::connector::QueryError>()
            .is_some()
    );
    session.close().unwrap();
    server.join().unwrap();
}

/// Answers OpenSession and returns the request to select the database.
fn open_session_then_use(peer: &mut Peer) -> TExecuteStatementReq {
    let _: TOpenSessionReq = peer.read("OpenSession");
    peer.reply(TOpenSessionResp::new(
        success(),
        TProtocolVersion::HIVE_CLI_SERVICE_PROTOCOL_V6,
        Some(TSessionHandle::new(THandleIdentifier::new(
            vec![1; 16],
            vec![2; 16],
        ))),
        None,
    ));
    peer.read("ExecuteStatement")
}

#[test]
fn a_missing_initial_database_is_named_with_the_server_message() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut p = profile(listener.local_addr().unwrap().port());
    p.database = "missing".into();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        let request = open_session_then_use(&mut peer);
        assert_eq!(request.statement, "USE `missing`");
        peer.reply(TExecuteStatementResp::new(
            success(),
            Some(operation(false)),
        ));
        let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
        peer.reply(TGetOperationStatusResp::new(
            success(),
            Some(TOperationState::ERROR_STATE),
            Some("42000".to_owned()),
            None,
            Some(
                "Error operating ExecuteStatement: [SCHEMA_NOT_FOUND] The schema `missing` cannot be found."
                    .to_owned(),
            ),
            None,
            None,
            None,
            None,
            None,
        ));
    });
    let error = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .err()
        .expect("the database does not exist");
    let message = qrow::connector::error_message(&error);
    assert!(
        message.starts_with(
            "Could not select the initial database \"missing\". Check that it exists on this server, or change Initial database in the connection: "
        ),
        "{message}"
    );
    assert!(message.contains("[SCHEMA_NOT_FOUND]"), "{message}");
    server.join().unwrap();
}

#[test]
fn a_silent_server_fails_after_the_response_timeout() {
    // The server accepts the connection and never answers.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let _ = finished.recv();
        drop(socket);
    });
    let started = std::time::Instant::now();
    let endpoint = qrow::connector::sasl::Endpoint {
        host: "127.0.0.1",
        port,
        tls: None,
        read_timeout: Duration::from_secs(1),
    };
    let error =
        match qrow::connector::sasl::connect(&endpoint, "synthetic-user", "synthetic-password") {
            Ok(_) => panic!("A silent server must not authenticate"),
            Err(error) => format!("{error:#}"),
        };
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        error.contains("Kyuubi did not answer within 1 seconds"),
        "{error}"
    );
    done.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn a_setup_step_without_an_answer_explains_the_engine_start() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut p = profile(listener.local_addr().unwrap().port());
    // The shortest response timeout that a connection accepts.
    p.lifecycle.response_timeout_seconds = 10;
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        open_session_then_use(&mut peer);
        // Like Kyuubi while it starts an engine: no answer for a while.
        let _ = wait.recv_timeout(Duration::from_secs(30));
    });
    let started = std::time::Instant::now();
    let error = HiveConnector::default()
        .connect(&p, Secret::password("test-password"))
        .err()
        .expect("the server did not answer");
    let message = qrow::connector::error_message(&error);
    assert!(
        message.starts_with(
            "Kyuubi did not answer within 10 seconds when Qrow tried to select the initial database \"avia\"."
        ),
        "{message}"
    );
    assert!(!message.contains("os error"), "{message}");
    assert!(started.elapsed() < Duration::from_secs(20));
    done.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn run_export_cancel_aborts_sasl_before_open_session_or_schema() {
    use qrow::{export, logs::ExecutionId, worker::Worker};
    use std::{
        sync::{Arc, atomic::AtomicBool, mpsc},
        time::Instant,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = profile(listener.local_addr().unwrap().port());
    let (stalled, observed) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        for _ in 0..2 {
            let mut header = [0; 5];
            socket.read_exact(&mut header).unwrap();
            let mut payload = vec![0; u32::from_be_bytes(header[1..].try_into().unwrap()) as usize];
            socket.read_exact(&mut payload).unwrap();
        }
        stalled.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(HiveConnector::default()),
        Arc::new(|_| Ok(Secret::password("synthetic"))),
    );
    let jobs = export::Jobs::default();
    let download = worker
        .run_and_export(
            profile,
            "SELECT never_submitted".into(),
            ExecutionId(201),
            None,
            &jobs,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    worker.cancel();
    let error = download.wait_spool().err().unwrap();
    assert!(error.get_ref().unwrap().is::<export::Cancelled>());
    assert!(jobs.cancel_and_wait(Duration::from_secs(2)));
    assert!(start.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}

#[test]
fn run_export_cancel_stops_execute_and_schema_replies_before_releasing_the_worker() {
    use qrow::{export, logs::ExecutionId, worker::Worker};
    use std::{
        sync::{Arc, atomic::AtomicBool, mpsc},
        time::Instant,
    };
    for schema in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let profile = profile(listener.local_addr().unwrap().port());
        let (stalled, observed) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut peer = Peer::accept(&listener);
            initialize(&mut peer);
            let request: TExecuteStatementReq = peer.read("ExecuteStatement");
            assert_eq!(request.statement, "SELECT blocked_reply");
            if schema {
                peer.reply(TExecuteStatementResp::new(success(), Some(operation(true))));
                let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
                peer.reply(status(TOperationState::FINISHED_STATE, true));
                let _: TGetResultSetMetadataReq = peer.read("GetResultSetMetadata");
            }
            stalled.send(()).unwrap();
            if schema {
                let mut cancellation = Peer::accept(&listener);
                let _: TCancelOperationReq = cancellation.read("CancelOperation");
                cancellation.reply(TCancelOperationResp::new(success()));
            }
            let mut byte = [0];
            assert_eq!(peer.socket.read(&mut byte).unwrap(), 0);
        });
        let worker = Worker::with_connector(
            Arc::new(|| {}),
            Arc::new(HiveConnector::default()),
            Arc::new(|_| Ok(Secret::password("test-password"))),
        );
        let jobs = export::Jobs::default();
        let download = worker
            .run_and_export(
                profile,
                "SELECT blocked_reply".into(),
                ExecutionId(202),
                None,
                &jobs,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        observed.recv_timeout(Duration::from_secs(3)).unwrap();
        let started = Instant::now();
        worker.cancel();
        assert!(
            download
                .wait_spool()
                .err()
                .unwrap()
                .get_ref()
                .unwrap()
                .is::<export::Cancelled>()
        );
        assert!(jobs.cancel_and_wait(Duration::from_secs(3)));
        assert!(started.elapsed() < Duration::from_secs(3));
        server.join().unwrap();
    }
}

#[test]
fn finished_exports_close_without_another_status_request() {
    use qrow::{
        export,
        logs::ExecutionId,
        worker::{Event, Worker},
    };
    use std::sync::{Arc, atomic::AtomicBool};
    for has_results in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = profile(listener.local_addr().unwrap().port());
        let server = thread::spawn(move || {
            let mut peer = Peer::accept(&listener);
            initialize(&mut peer);
            let req: TExecuteStatementReq = peer.read("ExecuteStatement");
            assert_eq!(req.statement, "SELECT synthetic_empty");
            peer.reply(TExecuteStatementResp::new(
                success(),
                Some(operation(has_results)),
            ));
            let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
            peer.reply(status(TOperationState::FINISHED_STATE, has_results));
            if has_results {
                let _: TGetResultSetMetadataReq = peer.read("GetResultSetMetadata");
                peer.reply(TGetResultSetMetadataResp::new(
                    success(),
                    Some(TTableSchema::new(vec![TColumnDesc::new(
                        "value".into(),
                        TTypeDesc::new(vec![TTypeEntry::PrimitiveEntry(TPrimitiveTypeEntry::new(
                            TTypeId::INT_TYPE,
                            None,
                        ))]),
                        0,
                        None,
                    )])),
                ));
                let _: TFetchResultsReq = peer.read("FetchResults");
                peer.reply(TFetchResultsResp::new(
                    success(),
                    Some(false),
                    Some(TRowSet::new(0, vec![], Some(vec![]), None, None)),
                ));
            }
            let _: TCloseOperationReq = peer.read("CloseOperation");
            peer.reply(TCloseOperationResp::new(success()));
            let _: TCloseSessionReq = peer.read("CloseSession");
            peer.reply(TCloseSessionResp::new(success()));
        });
        let worker = Worker::with_connector(
            Arc::new(|| {}),
            Arc::new(HiveConnector::default()),
            Arc::new(|_| Ok(Secret::password("test-password"))),
        );
        let jobs = export::Jobs::default();
        let download = worker
            .run_and_export(
                p,
                "SELECT synthetic_empty".into(),
                ExecutionId(204),
                None,
                &jobs,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        loop {
            match worker.events.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::Downloaded { spool, .. } => {
                    assert!(has_results);
                    assert_eq!(spool.status(), export::spool::Status::Complete { rows: 0 });
                    break;
                }
                Event::Ready { .. } if !has_results => {
                    assert!(
                        download
                            .wait_spool()
                            .err()
                            .unwrap()
                            .to_string()
                            .contains("without a result set")
                    );
                    break;
                }
                Event::Error { message, .. } | Event::DownloadFailed { message, .. } => {
                    panic!("{message}")
                }
                _ => {}
            }
        }
        worker.shutdown();
        worker.wait_for_shutdown(Duration::from_secs(3));
        server.join().unwrap();
    }
}

#[test]
fn run_export_cancel_interrupts_the_previous_profiles_session_close() {
    use qrow::{
        export,
        logs::ExecutionId,
        worker::{Event, Worker},
    };
    use std::{
        sync::{Arc, atomic::AtomicBool, mpsc},
        time::Instant,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let original = profile(listener.local_addr().unwrap().port());
    let mut changed = original.clone();
    changed.username = "different-account".into();
    let (stalled, observed) = mpsc::channel();
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let _: TExecuteStatementReq = peer.read("ExecuteStatement");
        peer.reply(TExecuteStatementResp::new(
            success(),
            Some(operation(false)),
        ));
        let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
        peer.reply(status(TOperationState::FINISHED_STATE, false));
        let _: TCloseOperationReq = peer.read("CloseOperation");
        peer.reply(TCloseOperationResp::new(success()));
        let _: TCloseSessionReq = peer.read("CloseSession");
        stalled.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(peer.socket.read(&mut byte).unwrap(), 0);
    });
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(HiveConnector::default()),
        Arc::new(|_| Ok(Secret::password("test-password"))),
    );
    worker.run(original, "SET previous_session".into());
    while !matches!(
        worker.events.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::Ready { .. }
    ) {}
    let jobs = export::Jobs::default();
    let download = worker
        .run_and_export(
            changed,
            "SELECT must_not_execute".into(),
            ExecutionId(203),
            None,
            &jobs,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    observed.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    worker.cancel();
    assert!(download.wait_spool().is_err());
    assert!(jobs.cancel_and_wait(Duration::from_secs(2)));
    assert!(started.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}

#[test]
fn failed_collect_restoration_discards_the_session_without_submitting_user_sql() {
    use qrow::{
        export::Jobs,
        logs::ExecutionId,
        model::transfer::TransferPreset,
        worker::{Event, Worker},
    };
    use std::sync::{Arc, atomic::AtomicBool};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut profile = profile(listener.local_addr().unwrap().port());
    profile.transfer.preset = TransferPreset::Conservative;
    let server = thread::spawn(move || {
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        configuration(&mut peer, "SET", Some("false"));
        for value in ["true", "false"] {
            let request: TExecuteStatementReq = peer.read("ExecuteStatement");
            assert_eq!(
                request.statement,
                format!("SET {}={value}", qrow::model::transfer::INCREMENTAL_KEY)
            );
            peer.reply(TExecuteStatementResp::new(
                TStatus {
                    status_code: TStatusCode::ERROR_STATUS,
                    error_message: Some("Setting denied".into()),
                    ..success()
                },
                None,
            ));
        }
        drop(peer);
        let mut peer = Peer::accept(&listener);
        initialize(&mut peer);
        let request: TExecuteStatementReq = peer.read("ExecuteStatement");
        assert_eq!(request.statement, "SELECT reconnect");
        peer.reply(TExecuteStatementResp::new(
            success(),
            Some(operation(false)),
        ));
        let _: TGetOperationStatusReq = peer.read("GetOperationStatus");
        peer.reply(status(TOperationState::FINISHED_STATE, false));
        let _: TCloseOperationReq = peer.read("CloseOperation");
        peer.reply(TCloseOperationResp::new(success()));
        let _: TCloseSessionReq = peer.read("CloseSession");
        peer.reply(TCloseSessionResp::new(success()));
    });
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(HiveConnector::default()),
        Arc::new(|_| Ok(Secret::password("test-password"))),
    );
    worker
        .run_and_export(
            profile.clone(),
            "SELECT must_not_run".into(),
            ExecutionId(501),
            None,
            &Jobs::default(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    loop {
        if let Event::DownloadFailed {
            message,
            disconnected,
            ..
        } = worker.events.recv_timeout(Duration::from_secs(10)).unwrap()
        {
            assert!(disconnected);
            assert!(message.contains("could not be restored"));
            break;
        }
    }
    assert!(worker.session_generation(&profile).is_none());
    worker.run(profile.clone(), "SELECT reconnect".into());
    loop {
        match worker.events.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Ready { .. } => break,
            Event::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert!(worker.session_generation(&profile).is_some());
    worker.shutdown();
    server.join().unwrap();
}

//! Local wire-level fixture. No Java, Kyuubi installation, or real credentials required.
use qrow::{
    connector::{
        Connector, QueryState,
        hive::HiveConnector,
        sasl::{FrameReader, FrameWriter},
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
use zeroize::Zeroizing;

struct Peer {
    input: TBinaryInputProtocol<FrameReader<TcpStream>>,
    output: TBinaryOutputProtocol<FrameWriter<TcpStream>>,
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
    let mut session = HiveConnector
        .connect(&p, Zeroizing::new("test-password".into()))
        .unwrap();
    session.execute("SELECT amount FROM ledger").unwrap();
    assert_eq!(session.poll().unwrap(), QueryState::Running);
    assert_eq!(
        session.poll().unwrap(),
        QueryState::Finished { has_results: true }
    );
    assert_eq!(session.columns().unwrap()[0].data_type, "DECIMAL");
    let batch = session.fetch(250).unwrap();
    assert!(batch.more);
    assert_eq!(
        batch.rows,
        vec![vec![Some("999999999999.123456789".into())], vec![None]]
    );
    assert!(!session.fetch(250).unwrap().more);
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
    let mut session = HiveConnector
        .connect(&p, Zeroizing::new("test-password".into()))
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
    let mut session = HiveConnector
        .connect(&p, Zeroizing::new("test-password".into()))
        .unwrap();
    assert!(session.execute("SELECT 1").is_err());
    server.join().unwrap();
}

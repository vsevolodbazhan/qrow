use super::{Cancellation, Connector, QueryError, QueryState, Session, sasl, t_c_l_i_service::*};
use crate::model::{Batch, Column, Profile, Row};
use anyhow::{Context, Result, ensure};
use std::{sync::Arc, thread, time::Duration};
use zeroize::Zeroizing;

pub struct HiveConnector;

struct ConnectionFailure {
    message: String,
    details: Option<ErrorDetails>,
}

impl std::fmt::Display for ConnectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::fmt::Debug for ConnectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionFailure")
            .field("message", &self.message)
            .field("details", &self.details)
            .finish()
    }
}

impl std::error::Error for ConnectionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.details
            .as_ref()
            .map(|details| details as &(dyn std::error::Error + 'static))
    }
}

impl std::fmt::Display for ErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Debug for ErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ErrorDetails {}

struct ErrorDetails(String);
struct Credentials {
    profile: Profile,
    password: Zeroizing<String>,
}
struct Cancel {
    credentials: Arc<Credentials>,
    handle: TOperationHandle,
}

impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        let p = &self.credentials.profile;
        // A separate authenticated transport keeps CancelOperation independent of blocked fetching/polling.
        let mut client = sasl::connect(&p.host, p.port, &p.username, &self.credentials.password)?;
        check(
            client
                .cancel_operation(TCancelOperationReq::new(self.handle.clone()))?
                .status,
        )
    }
}

pub struct HiveSession {
    client: sasl::Client,
    credentials: Arc<Credentials>,
    session: Option<TSessionHandle>,
    operation: Option<TOperationHandle>,
    preview_operation: Option<TOperationHandle>,
    column_count: usize,
}

impl Connector for HiveConnector {
    fn connect(&self, profile: &Profile, password: Zeroizing<String>) -> Result<Box<dyn Session>> {
        profile.validate()?;
        let mut client = sasl::connect(&profile.host, profile.port, &profile.username, &password)?;
        let opened = client.open_session(TOpenSessionReq::new(
            TProtocolVersion::HIVE_CLI_SERVICE_PROTOCOL_V6,
            Some(profile.username.clone()),
            None,
            Some(profile.parameters.clone()),
        ))?;
        check(opened.status)?;
        let session = opened
            .session_handle
            .context("Kyuubi returned no session handle")?;
        let mut connection = HiveSession {
            client,
            credentials: Arc::new(Credentials {
                profile: profile.clone(),
                password,
            }),
            session: Some(session),
            operation: None,
            preview_operation: None,
            column_count: 0,
        };
        let setup = (|| -> Result<()> {
            ensure!(
                opened.server_protocol_version.0 >= 5,
                "Kyuubi does not support columnar results (HiveServer2 protocol V6)"
            );
            connection.execute(&format!("USE `{}`", profile.database.replace('`', "``")))?;
            loop {
                match connection.poll()? {
                    QueryState::Running => thread::sleep(Duration::from_millis(100)),
                    QueryState::Finished { .. } => break,
                    QueryState::Cancelled => {
                        anyhow::bail!("Initial database selection was cancelled")
                    }
                }
            }
            connection.close_operation()?;
            Ok(())
        })();
        if let Err(error) = setup {
            let _ = connection.close();
            return Err(error.context("Could not initialize the session"));
        }
        Ok(Box::new(connection))
    }
}

pub fn check(status: TStatus) -> Result<()> {
    if status.status_code == TStatusCode::SUCCESS_STATUS
        || status.status_code == TStatusCode::SUCCESS_WITH_INFO_STATUS
    {
        return Ok(());
    }
    let message = status
        .error_message
        .clone()
        .unwrap_or_else(|| format!("Kyuubi returned status {}", status.status_code.0));
    if status.status_code == TStatusCode::ERROR_STATUS {
        let connection_failure = status
            .sql_state
            .as_deref()
            .is_some_and(|state| state.starts_with("08"))
            || status
                .info_messages
                .iter()
                .flatten()
                .any(|info| is_session_failure(info));
        return Err(query_failure(
            message,
            status.info_messages.clone().unwrap_or_default(),
            status.sql_state.clone(),
            status.error_code,
            connection_failure,
        ));
    }
    anyhow::bail!(format_error(
        message,
        status.info_messages.unwrap_or_default(),
        status.sql_state,
        status.error_code,
    ))
}

fn is_session_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "ttransportexception",
        "socket is closed by peer",
        "invalid sessionhandle",
        "invalid session handle",
        "session is closed",
    ]
    .iter()
    .any(|signature| message.contains(signature))
}

fn format_error(
    message: String,
    info_messages: Vec<String>,
    sql_state: Option<String>,
    error_code: Option<i32>,
) -> String {
    let mut formatted = message;
    if !info_messages.is_empty() {
        formatted.push_str("\nDiagnostics:\n");
        for info in info_messages {
            formatted.push_str(&info);
            formatted.push('\n');
        }
        formatted.pop();
    }
    if let Some(sql_state) = sql_state {
        formatted.push_str("\nSQL state: ");
        formatted.push_str(&sql_state);
    }
    if let Some(error_code) = error_code {
        formatted.push_str("\nError code: ");
        formatted.push_str(&error_code.to_string());
    }
    formatted
}

fn query_failure(
    message: String,
    info_messages: Vec<String>,
    sql_state: Option<String>,
    error_code: Option<i32>,
    connection_failure: bool,
) -> anyhow::Error {
    // Kyuubi can wrap a dead engine transport in a successful Thrift response.
    // Only query errors allow the worker to reuse the existing session.
    let primary = message.clone();
    let message = format_error(message, info_messages, sql_state, error_code);
    if connection_failure || is_session_failure(&message) {
        let details = (message != primary).then(|| ErrorDetails(message.clone()));
        anyhow::Error::new(ConnectionFailure {
            message: primary,
            details,
        })
    } else {
        QueryError(message).into()
    }
}

impl Session for HiveSession {
    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.preview_operation = self.operation.take();
        self.execute(sql)
    }

    fn close_keep_alive(&mut self) -> Result<()> {
        let result = self.close_operation();
        self.operation = self.preview_operation.take();
        result
    }

    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.close_operation()?;
        let response = self.client.execute_statement(TExecuteStatementReq::new(
            self.session.clone().context("Session is closed")?,
            sql.to_owned(),
            None,
            Some(true),
            None,
        ))?;
        check(response.status)?;
        let operation = response
            .operation_handle
            .context("Kyuubi returned no operation handle")?;
        self.operation = Some(operation.clone());
        Ok(Arc::new(Cancel {
            credentials: self.credentials.clone(),
            handle: operation,
        }))
    }

    fn poll(&mut self) -> Result<QueryState> {
        let operation = self.operation.clone().context("No active operation")?;
        let response = self
            .client
            .get_operation_status(TGetOperationStatusReq::new(operation.clone(), Some(false)))?;
        let status = response.status.clone();
        check(status.clone())?;
        let state = response
            .operation_state
            .context("Kyuubi returned no operation state")?;
        if state == TOperationState::FINISHED_STATE {
            Ok(QueryState::Finished {
                has_results: response.has_result_set.unwrap_or(operation.has_result_set),
            })
        } else if state == TOperationState::CANCELED_STATE {
            Ok(QueryState::Cancelled)
        } else if state == TOperationState::ERROR_STATE || state == TOperationState::TIMEDOUT_STATE
        {
            let connection_failure = response
                .sql_state
                .as_deref()
                .or(status.sql_state.as_deref())
                .is_some_and(|state| state.starts_with("08"));
            Err(query_failure(
                response
                    .error_message
                    .unwrap_or_else(|| "Query failed or timed out".into()),
                status.info_messages.unwrap_or_default(),
                response.sql_state.or(status.sql_state),
                response.error_code.or(status.error_code),
                connection_failure,
            ))
        } else if state == TOperationState::CLOSED_STATE || state == TOperationState::UKNOWN_STATE {
            anyhow::bail!("The operation is no longer available on Kyuubi")
        } else {
            Ok(QueryState::Running)
        }
    }

    fn columns(&mut self) -> Result<Vec<Column>> {
        let response = self
            .client
            .get_result_set_metadata(TGetResultSetMetadataReq::new(
                self.operation.clone().context("No active operation")?,
            ))?;
        check(response.status)?;
        let schema = response
            .schema
            .context("Kyuubi returned no result schema")?;
        let columns: Vec<_> = schema
            .columns
            .into_iter()
            .map(|column| Column {
                name: column.column_name,
                data_type: type_label(&column.type_desc),
            })
            .collect();
        self.column_count = columns.len();
        Ok(columns)
    }

    fn fetch(&mut self, count: usize) -> Result<Batch> {
        let response = self.client.fetch_results(TFetchResultsReq::new(
            self.operation.clone().context("No active operation")?,
            TFetchOrientation::FETCH_NEXT,
            count.min(1000) as i64,
            Some(0),
        ))?;
        check(response.status)?;
        let results = response.results.context("Kyuubi returned no row set")?;
        ensure!(
            results.binary_columns.is_none(),
            "Packed binary column results are unsupported by this protocol version"
        );
        ensure!(
            results.rows.is_empty(),
            "Expected columnar results from HiveServer2 protocol V6"
        );
        let rows = decode_columns(results.columns.unwrap_or_default(), self.column_count)?;
        ensure!(
            rows.len() <= count.min(1000),
            "Kyuubi returned more rows than requested"
        );
        // Older Hive-compatible servers report hasMoreRows=false even when more rows exist.
        // An empty fetch is the portable end-of-results signal used by PyHive.
        Ok(Batch {
            more: !rows.is_empty(),
            rows,
        })
    }

    fn close_operation(&mut self) -> Result<()> {
        if let Some(operation) = self.operation.take() {
            check(
                self.client
                    .close_operation(TCloseOperationReq::new(operation))?
                    .status,
            )?;
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        let operation_result = if self.preview_operation.is_some() {
            let result = self.close_keep_alive();
            result.and(self.close_operation())
        } else {
            self.close_operation()
        };
        if let Some(session) = self.session.take() {
            check(
                self.client
                    .close_session(TCloseSessionReq::new(session))?
                    .status,
            )?;
        }
        operation_result
    }
}

fn type_label(desc: &TTypeDesc) -> String {
    match desc.types.first() {
        Some(TTypeEntry::PrimitiveEntry(p)) => {
            let names = [
                "BOOLEAN",
                "TINYINT",
                "SMALLINT",
                "INT",
                "BIGINT",
                "FLOAT",
                "DOUBLE",
                "STRING",
                "TIMESTAMP",
                "BINARY",
                "ARRAY",
                "MAP",
                "STRUCT",
                "UNION",
                "USER",
                "DECIMAL",
                "NULL",
                "DATE",
                "VARCHAR",
                "CHAR",
                "INTERVAL YEAR MONTH",
                "INTERVAL DAY TIME",
                "TIMESTAMP LOCAL TZ",
            ];
            names
                .get(p.type_.0 as usize)
                .unwrap_or(&"UNKNOWN")
                .to_string()
        }
        Some(TTypeEntry::ArrayEntry(_)) => "ARRAY".into(),
        Some(TTypeEntry::MapEntry(_)) => "MAP".into(),
        Some(TTypeEntry::StructEntry(_)) => "STRUCT".into(),
        _ => "UNKNOWN".into(),
    }
}

pub fn decode_columns(columns: Vec<TColumn>, expected: usize) -> Result<Vec<Row>> {
    if columns.is_empty() {
        return Ok(vec![]);
    }
    ensure!(
        columns.len() == expected,
        "Result column count does not match schema"
    );
    let mut decoded = Vec::with_capacity(columns.len());
    for column in columns {
        macro_rules! values {
            ($c:expr, $format:expr) => {{
                let c = $c;
                c.values
                    .into_iter()
                    .enumerate()
                    .map(|(i, value)| {
                        let null = c
                            .nulls
                            .get(i / 8)
                            .is_some_and(|byte| byte & (1 << (i % 8)) != 0);
                        if null { None } else { Some(($format)(value)) }
                    })
                    .collect::<Vec<Option<String>>>()
            }};
        }
        let values = match column {
            TColumn::BoolVal(c) => values!(c, |v: bool| v.to_string()),
            TColumn::ByteVal(c) => values!(c, |v: i8| v.to_string()),
            TColumn::I16Val(c) => values!(c, |v: i16| v.to_string()),
            TColumn::I32Val(c) => values!(c, |v: i32| v.to_string()),
            TColumn::I64Val(c) => values!(c, |v: i64| v.to_string()),
            TColumn::DoubleVal(c) => values!(c, |v: thrift::OrderedFloat<f64>| v.to_string()),
            TColumn::StringVal(c) => values!(c, |v| v),
            TColumn::BinaryVal(c) => values!(c, |v: Vec<u8>| {
                use std::fmt::Write;
                let mut out = String::from("0x");
                for b in v {
                    let _ = write!(out, "{b:02x}");
                }
                out
            }),
        };
        decoded.push(values);
    }
    let length = decoded[0].len();
    ensure!(
        decoded.iter().all(|c| c.len() == length),
        "Result columns have inconsistent lengths"
    );
    let mut rows = vec![Vec::with_capacity(expected); length];
    for column in decoded {
        for (row, cell) in rows.iter_mut().zip(column) {
            row.push(cell);
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_session_failures_do_not_preserve_the_connection() {
        for (message, state, info) in [
            ("Socket is closed by peer.", None, None),
            ("Invalid SessionHandle [id]", None, None),
            ("Invalid session handle", None, None),
            ("Session is closed", None, None),
            ("Connection lost", Some("08S01"), None),
            (
                "Error operating ExecuteStatement",
                None,
                Some(vec![
                    "org.apache.kyuubi.shaded.thrift.transport.TTransportException".into(),
                ]),
            ),
        ] {
            let error = check(TStatus::new(
                TStatusCode::ERROR_STATUS,
                info,
                state.map(str::to_owned),
                None,
                Some(message.to_owned()),
            ))
            .unwrap_err();
            assert!(error.downcast_ref::<QueryError>().is_none(), "{message}");
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn ordinary_sql_errors_preserve_the_connection() {
        for message in [
            "Syntax error",
            "Table not found",
            "Query failed or timed out",
        ] {
            let error = check(TStatus::new(
                TStatusCode::ERROR_STATUS,
                None,
                Some("42000".to_owned()),
                None,
                Some(message.to_owned()),
            ))
            .unwrap_err();
            assert!(error.downcast_ref::<QueryError>().is_some());
        }
    }

    #[test]
    fn server_diagnostics_are_kept_in_the_alternate_error_chain() {
        let error = check(TStatus::new(
            TStatusCode::ERROR_STATUS,
            Some(vec!["detail one".into(), "detail two".into()]),
            Some("42000".into()),
            Some(17),
            Some("Syntax error".into()),
        ))
        .unwrap_err();
        assert!(error.to_string().starts_with("Syntax error"));
        let complete = format!("{error:#}");
        assert!(complete.contains("detail one"));
        assert!(complete.contains("detail two"));
        assert!(complete.contains("SQL state: 42000"));
        assert!(complete.contains("Error code: 17"));
    }

    #[test]
    fn connection_failure_keeps_diagnostics_without_changing_its_classification() {
        let error = check(TStatus::new(
            TStatusCode::ERROR_STATUS,
            Some(vec!["transport detail".into()]),
            Some("08S01".into()),
            Some(17),
            Some("Connection lost".into()),
        ))
        .unwrap_err();
        assert!(error.downcast_ref::<QueryError>().is_none());
        assert_eq!(error.to_string(), "Connection lost");
        let complete = format!("{error:#}");
        assert!(complete.contains("transport detail"));
        assert!(complete.contains("SQL state: 08S01"));
        assert!(complete.contains("Error code: 17"));
    }

    #[test]
    fn null_bitmap_and_binary_and_decimal_remain_exact() {
        let rows = decode_columns(
            vec![
                TColumn::StringVal(TStringColumn::new(
                    vec!["123456789.123456789".into(), "".into()],
                    vec![2],
                )),
                TColumn::BinaryVal(TBinaryColumn::new(vec![vec![0, 255, 128], vec![]], vec![0])),
            ],
            2,
        )
        .unwrap();
        assert_eq!(
            rows[0],
            vec![Some("123456789.123456789".into()), Some("0x00ff80".into())]
        );
        assert_eq!(rows[1], vec![None, Some("0x".into())]);
    }
    #[test]
    fn rejects_ragged_results() {
        assert!(
            decode_columns(
                vec![
                    TColumn::StringVal(TStringColumn::new(vec!["a".into()], vec![])),
                    TColumn::StringVal(TStringColumn::new(vec![], vec![])),
                ],
                2
            )
            .is_err()
        );
    }
}

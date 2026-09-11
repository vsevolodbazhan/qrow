# Rust SQL desktop console

Discussion notes, 11 September 2026.

## Purpose

Build a native Rust SQL desktop application for personal use and exploration. DataGrip is the reference application, but the first version covers only the workflow used most often: connect, edit highlighted SQL, execute a query, and inspect results in a table.

The motivation is DataGrip's slow startup, freezes, and sluggish interaction. Fast startup and a responsive interface are the primary product goals. Broad DataGrip feature parity is not a goal for the prototype.

## Constraints and decisions

| Area | Decision |
| --- | --- |
| Platform | macOS only initially |
| UI | Native Rust, with no embedded web frontend |
| Dependencies | Open source |
| Local Java | No Java runtime or Java components in the application |
| Packaging | Self-contained application; no separate driver installation for the prototype |
| Initial query source | Spark SQL through Kyuubi |
| Connection protocol | HiveServer2 Thrift directly from Rust, following the existing Python client's approach |
| Authentication | LDAP with username and password |
| Future query source | Trino |
| Architecture | Replaceable connectors; no full plugin system needed for the prototype |

Spark and Kyuubi still use their server-side runtimes. The no-Java requirement applies to the desktop application.

JDBC was the initial proposal. ODBC was considered next, including bundling a driver. The final direction is direct Thrift because open-source dependencies and a self-contained application take priority. No ODBC driver has been selected. ADBC is deferred; compatibility with this deployment has not been established.

## Existing connection behavior

The working Python client uses `hive.Connection` with:

- Host and port.
- Username and password.
- `auth='LDAP'`.
- A `configuration` dictionary containing server-side parameters.

The existing endpoint uses port `10009`, with `avia` as the initial database. One relevant session parameter is `kyuubi.engine.share.level.subdomain`, used to select the engine-sharing subdomain. The Rust connector must preserve this behavior when opening sessions.

The JDBC URL is evidence of the current configuration, not a format that the new connector must use. Driver-specific URL syntax must be translated into the appropriate session properties.

Passwords must not be stored in this document, source code, or plain-text application configuration.

## Connection profiles

Support multiple named, saved profiles. Each profile contains:

- Display name.
- Host and port.
- Username.
- A reference to a password stored in macOS Keychain.
- Initial database.
- Server-side session parameters, editable as key/value pairs.

Profiles can share an endpoint while using different accounts and settings. Existing examples include `rivendell-s`, `rivendell-xl`, and `rivendell-xxl`. These accounts already provide different Spark resource sizes; the application does not need to allocate or model those resources itself.

## Editor and tabs

- Multiple query tabs from the first version.
- SQL syntax highlighting.
- One selected connection profile per tab.
- One persistent Kyuubi session per connected tab.
- Run selected text when there is a selection; otherwise run the editor contents.
- Support one SQL statement per execution. Multi-statement scripts are out of scope.
- Allow concurrent queries across tabs, with at most one active query in each tab.

Each session receives its profile's settings. Separate sessions allow tab-specific `USE` and `SET` state; actual isolation under the deployed Kyuubi configuration must be verified. Separate sessions do not imply separate Spark engines.

### Switching a tab's profile

Allow an existing tab to switch connection profiles while preserving its SQL. For example, a query written for `rivendell-s` can be run on `rivendell-xl` without copying it to another tab.

Switching opens a fresh session and clears the previous results. Disable switching while a query is running in that tab. Session state from the previous connection does not transfer.

## Results

- Display results as a table.
- Fetch incrementally in batches and show rows as they arrive.
- Start with a 1,000-row preview and provide an explicit “Load more” action.
- Make it clear that the displayed data is a partial result when more rows are available.
- Keep fetching cancellable and the interface responsive.
- Copying cells or selected rows is optional and must not delay the first working version.
- No data export in the prototype.

The preview limit is a client fetching/display policy. It must not silently rewrite the user's SQL. It also does not guarantee that Spark performs less server-side work.

## Cancellation and connection failures

A Cancel button is required. It should request cancellation on Kyuubi, respond immediately in the UI, and show a cancelling state until the server reports the outcome. Server-side cancellation behavior must be tested.

If a connection drops:

- Preserve the SQL text and already fetched results.
- Mark the tab disconnected and report the failure.
- Reconnect on the next execution.
- Never automatically rerun the interrupted statement, since it may already have changed data.

A new session does not automatically restore settings established by earlier SQL statements.

## Workspace persistence

Automatically save and restore:

- Open tabs.
- SQL text in each tab.
- The connection profile selected in each tab.

Previous result sets are not restored after restarting the application. Sessions reconnect when a query is executed, rather than blocking application startup. Connection profiles and Keychain passwords persist independently of open tabs.

## Performance goals

Proposed initial target: an editable window within one second of launch on the user's Mac. This is a target to measure, not a validated guarantee; reference hardware and measurement conditions remain to be defined.

The application must remain usable while connecting, waiting for Kyuubi to start an engine, executing concurrent queries, and fetching results. Backend engine startup time is distinct from application startup time.

Implementation direction:

- Keep blocking network and query work off the UI thread.
- Render only the visible portion of the results table.
- Use bounded fetching and buffering.
- Measure editing and scrolling responsiveness while results arrive.

## Extensibility

Keep the editor, tabs, and results UI independent of HiveServer2 details. Introduce a small connector boundary covering connection/session lifecycle, execution, status, cancellation, and batched results.

Trino is the next intended source. ODBC or ADBC may become additional connector implementations later. The prototype does not need dynamic plugin loading, a driver marketplace, or a universal database abstraction.

## Out of scope

- Autocomplete and schema-aware completion.
- Schema or database exploration.
- Multi-statement script execution.
- Data export.
- Broad DataGrip feature parity.
- Windows and Linux support.
- Public-release or team-distribution readiness.

## Open technical questions

- Which native Rust UI framework and editor component meet the performance and usability goals?
- Which open-source Rust Thrift and SASL components can reproduce the working LDAP connection?
- How does the deployed Kyuubi version handle session configuration, engine sharing, and session isolation?
- Can cancellation proceed promptly while execution or fetching is in progress?
- How should an unfinished server-side operation be released when the user stops at a preview or replaces the result?
- How should Spark values, including nulls, decimals, timestamps, binary values, and nested types, be displayed accurately?
- What memory limit and retention policy should apply after repeated “Load more” requests?
- Should the initial macOS build target Apple Silicon only or also Intel?

## Recommended next steps

These are proposed validation steps. Implementation has not been requested yet.

1. Build a small Rust connectivity proof against Kyuubi. Verify LDAP authentication, session properties, engine selection, execution, batched fetching, cancellation, concurrent sessions, and recovery from a dropped connection.
2. Test a native Rust editor and results table with several restored tabs and incoming result batches. Measure startup and interaction latency before choosing the UI framework.
3. Build the agreed application workflow around the validated connector and UI components.

The project is plausible at this scope. The main unresolved risk is the correctness and maintenance cost of the Rust HiveServer2 client, particularly authentication, cancellation, and session behavior. A successful basic query alone is insufficient validation.

## Reference material

- [Kyuubi PyHive client](https://kyuubi.readthedocs.io/en/v1.9.0/client/python/pyhive.html): existing direct HiveServer2 client approach and authentication examples.
- [HiveServer2 interface](https://hive.apache.org/docs/latest/admin/setting-up-hiveserver2/): protocol overview and Thrift interface definition.
- [Kyuubi engine sharing](https://github.com/apache/kyuubi/blob/master/docs/deployment/engine_share_level.md): engine-sharing configuration and subdomains.
- [Apache Hive ODBC documentation](https://hive.apache.org/docs/latest/admin/hiveodbc/): Apache Hive's legacy ODBC driver targets HiveServer1, not HiveServer2.

These references provide background. Compatibility must be checked against the actual deployed versions.

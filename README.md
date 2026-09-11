# Qrow

A native Rust SQL workbench for macOS. Connect to Spark through Kyuubi, edit SQL,
run queries in parallel tabs, and inspect results without a local JVM or web UI.

This is a personal prototype. The HiveServer2 connector has local wire-level
tests; compatibility with the real Kyuubi deployment still needs validation.

## Run

Install a current stable Rust toolchain and the Xcode command-line tools, then:

```sh
cargo run --bin qrow
```

For a populated UI preview that does not connect to a server, access Keychain, or
change the saved workspace:

```sh
cargo run --bin qrow -- --demo
```

## Build the macOS application

```sh
sh scripts/package-macos.sh
open dist/Qrow.app
```

The script builds for the current Mac's architecture and produces `dist/Qrow.app`
and `dist/Qrow-macos.zip`. On an Apple Silicon Mac these are ARM64 builds. The app
is locally ad-hoc signed, not notarized for public distribution. No separately
installed database driver is required.

## Connect and query

1. Click **Add connection** or the **+** beside Connections.
2. Enter a name, Kyuubi host, port, LDAP username, password, and initial database.
3. Add any session parameters, such as `kyuubi.engine.share.level.subdomain`.
4. Save the connection. macOS may ask for Keychain access.
5. Write SQL and click **Run query** or press **⌘Enter**.

Passwords are stored in macOS Keychain under service `io.qrow.connection`, keyed
by profile UUID. They are retrieved on a background thread when a tab connects.
The prototype supports SASL PLAIN over TCP for the existing LDAP deployment,
matching the provided PyHive configuration. SASL PLAIN does not encrypt the
transport. Use it on the same trusted network/VPN as that setup. TLS, Kerberos,
HTTP transport, and SSH tunneling are not implemented.

Select text to execute just that statement. Without a selection, the whole
editor is submitted. Multiple statements are rejected locally. No SQL is
automatically retried after a connection failure.

Use **⌘T** or **+** in the tab strip to open a tab. Each tab owns an independent
session and can run one query at a time. Different tabs can run concurrently.
The Cancel button sends a request over a separate authenticated transport so it
does not queue behind a blocked fetch. Kyuubi must accept the operation handle
on that transport; this requires live validation. Cancellation cannot roll back
a statement that has already completed. If a connection or initial session setup
is still in progress, cancellation prevents submission of the user's query once
setup returns. Individual network reads time out after 120 seconds. On exit,
the app requests cancellation and session cleanup, waiting up to one second
after saving the workspace. If the server is unreachable, cleanup is best effort
and server-side idle/session timeouts remain responsible for abandoned resources.

Switch profiles using the toolbar or sidebar. SQL stays in the tab, while the
old session and results are released. Switching and closing are disabled while
the tab is busy. Right-click a connection to edit or duplicate it. Profile edits
take effect on the next query if they require a new session.

Results arrive in batches of up to 250 rows, stopping at a 1,000-row preview.
**Load 1,000 more** advances the cursor. The client does not add a SQL `LIMIT`.
An empty fetch confirms exhaustion because some servers misreport `hasMoreRows`.
Preview storage is capped at 100,000 rows or approximately 64 MiB per tab; an
incoming batch that would exceed the cap is discarded and the cursor is closed.
Frame size is also capped at 64 MiB. The table renders visible rows only.

Decimals and textual timestamps retain their server representation. Binary
values display as hexadecimal. Nulls display as `NULL`; empty strings remain
empty. Nested values use the textual representation returned by HiveServer2.
Cells display a shortened preview, while right-click **Copy cell** or **Copy row**
copies the full stored value. File export is not included.

## Workspace

Tabs, SQL, selected profiles, and connection settings are saved automatically to:

```text
~/Library/Application Support/Qrow/workspace.json
```

Passwords and results are not written to this file. The workspace is written
atomically in the background after a short editing debounce and flushed on exit.
Tabs restore without opening connections. A corrupt or unsupported workspace is
left untouched and automatic saving is disabled for that run.

For isolated testing, set `QROW_DATA_DIR` to another directory. The demo uses an
in-memory workspace and never saves it.

## Validation

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Local protocol tests exercise SASL authentication, session parameters, async
execution, result metadata, exact decimal/null handling, batched fetching,
cancellation, and dropped connections. They do not prove server configuration,
engine isolation, or cancellation behavior in a deployed Kyuubi instance.

The first UI frame's elapsed startup time is printed to stderr. This is a useful
local diagnostic, not a measurement of cold launch to first visible display.

After saving a real connection in the app, verify session initialization and a
read-only query with:

```sh
cargo run --bin qrow-probe -- "your profile name"
```

The probe reads the profile and Keychain password, executes `SELECT 1`, verifies
the result, and closes its session. It does not verify engine sharing or cancellation.

## Structure

- `src/connector/`: connector traits, HiveServer2 implementation, SASL transport,
  and generated Apache Thrift bindings.
- `src/worker.rs`: per-tab query execution and cancellation coordination.
- `src/ui.rs`: native egui/eframe interface and virtualized result table.
- `src/storage.rs`: workspace persistence and macOS Keychain access.
- `src/sql.rs`: SQL highlighting lexer and single-statement validation.
- `tests/hive_protocol.rs`: local protocol fixtures.
- `PROJECT_PLAN.md`: agreed product scope and decisions.

Generated bindings are checked in, so building the application does not need the
Thrift compiler. To regenerate them, install Thrift **0.24.0** and run:

```sh
sh scripts/generate-thrift.sh
```

The script applies four compiler-output corrections for union collections.
Apache Thrift 0.24 marks its Rust generator deprecated, so maintaining or replacing
these bindings is a known dependency risk. Future Trino, ODBC, or ADBC connectors
should implement the application connector boundary without changing the editor.

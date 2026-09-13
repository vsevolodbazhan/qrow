# Qrow

A native Rust SQL workbench for macOS. Connect to Spark through Kyuubi, edit SQL,
run queries in parallel tabs, and inspect results without a local JVM or web UI.
The interface uses GPUI Kit 0.6.1 with native Metal rendering. SQL highlighting
uses Tree-sitter. The default theme uses One Dark-style blue-gray colors, with query tabs, a
connection sidebar, a resizable editor and results table, and connection settings
in a popup dialog.

This is a personal prototype. The HiveServer2 connector has local wire-level
tests, and the project owner has confirmed that it works against their real
Kyuubi connection. That confirmation does not establish compatibility with all
Kyuubi deployments or validate every cancellation and failure scenario.

## Prerequisites

- macOS. The prototype has been built and tested on Apple Silicon.
- A current stable Rust toolchain, including `cargo` and `rustc`.
- Xcode command-line tools. Install them with `xcode-select --install` if needed.
- Python 3 for the packaging script's dependency-license collection.
- [uv](https://docs.astral.sh/uv/) for the locked icon packaging dependency.

Check your tools:

```sh
rustc --version
cargo --version
xcode-select -p
python3 --version
uv --version
```

Run the commands below from the repository root, the directory containing
`Cargo.toml`. The first build downloads dependencies and takes longer than
subsequent builds. Java, Python database clients, ODBC drivers, and the Thrift
compiler are not required to build or run Qrow. GPUI is built with runtime Metal
shader compilation, so the separate Xcode Metal compiler is not required either.

## Build and launch the macOS app

For normal use, build the optimized application bundle and launch it:

```sh
sh scripts/package-macos.sh
open dist/Qrow.app
```

The script builds for the current Mac's architecture and produces `dist/Qrow.app`
and `dist/Qrow-macos.zip`. On an Apple Silicon Mac these are ARM64 builds. The app
is locally ad-hoc signed, not notarized for public distribution. No separately
installed database driver is required.

The icon compiler is portable and can run on Linux before the final macOS build:

```sh
uv run --locked python scripts/build-icon.py assets/app-icons/macos/qrow.png Qrow.icns
```

You can also double-click `dist/Qrow.app` in Finder or copy it to Applications.
After changing the source, quit Qrow, rerun the packaging script, and reopen the
app. The packaged app does not update automatically when you run `cargo build`.

## Build and launch from the terminal

For development, compile and launch the debug build in one command:

```sh
cargo run --locked --bin qrow
```

To build an optimized executable without creating an app bundle:

```sh
cargo build --locked --release --bin qrow
./target/release/qrow
```

Use the release build when checking startup time and responsiveness. Running
the executable from a terminal also shows startup timing and diagnostic output.

## Preview without a database

Launch the demo to inspect syntax highlighting, tabs, and a populated results
table. It does not connect to a server, access Keychain, or change the saved
workspace:

```sh
cargo run --locked --bin qrow -- --demo
```

Or, after building the release executable:

```sh
./target/release/qrow --demo
```

For live queries, connect to the network or VPN that can reach your Kyuubi
endpoint, then add a connection profile as described below.

## Connect and query

1. Click **+** beside Connections to open connection settings.
2. Enter a name, Kyuubi host, port, LDAP username, password, and initial database.
3. Enter session parameters as a JSON object, for example:
   `{"kyuubi.engine.share.level.subdomain": "your-subdomain"}`.
4. Save the connection. macOS may ask for Keychain access.
5. Write SQL and click **Run** or press **⌘Enter**.

In connection settings, **⌘Enter** saves and **Escape** dismisses the popup.

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

Switch profiles using the Connections sidebar. SQL stays in the tab, while the
old session and results are released. Switching and closing are disabled while
the tab is busy. Use the settings icon beside a connection to edit it; the editor
also has a **Duplicate** button. Editing is disabled while that profile has a
running query. Saving an edit disconnects idle sessions that use that profile;
the next query opens a new session.

Results stream in batches of up to 250 rows into a 1,000-row page. **Next**
fetches another page only when needed. **Previous** and **Next** reuse downloaded
pages without executing SQL again. Row numbers refer to the full result, and
copy actions use the full stored values on the displayed page. You can browse
downloaded pages while a fetch is running. New batches preserve the current
scroll position; changing pages clears selection and scrolls to the top.
The client does not add a SQL `LIMIT`. An empty fetch confirms exhaustion
because some servers misreport `hasMoreRows`. If Next finds no further rows,
the last populated page stays visible. Cancellation or a fetch failure retains
previously downloaded rows.
Preview storage is capped at 100,000 rows or approximately 64 MiB per tab; an
incoming batch that would exceed the cap is discarded and the cursor is closed.
Frame size is also capped at 64 MiB. The table virtualizes rows and columns.
Drag column boundaries to resize them. Drag the divider above Results to resize
the editor, and the sidebar divider to change its width. **⌘B** toggles the sidebar.

Decimals and textual timestamps retain their server representation. Binary
values display as hexadecimal. Nulls display as `NULL`; empty strings remain
empty. Nested values use the textual representation returned by HiveServer2.
Cells display a shortened preview, while right-click **Copy cell** or **Copy row**
copies the full stored value. File export is not included.

## Idle connections and reconnecting

Each connection has a **When idle** choice in its settings:

- **Disconnect after** releases each idle tab's session after the configured
  number of seconds. The default is 900 seconds, including for existing profiles.
  The timer starts after query or preview fetching finishes. Reading results or
  editing SQL does not reset it, and a running query is never interrupted by it.
- **Keep connected** replaces idle disconnection with periodic heartbeat SQL.
  The editor suggests a 300-second interval and `SELECT 1`. Both are configurable.
  Choose a lightweight, read-only statement. Heartbeats run only while the tab
  is idle, use the same session, and preserve the user's result cursor. They do
  not create new sessions or reconnect after failures. This mode is off by default.

**Disconnect** in the query toolbar releases the active tab's session. It is
unavailable while a query or heartbeat is running; Cancel remains available.
Manual and idle disconnection preserve SQL and downloaded results. They release
unfetched rows, temporary views, and session settings. The next explicit Run opens
and initializes a new session with the profile's configured database and parameters.

A dead connection, including a Kyuubi error wrapping an engine transport failure,
is discarded. Qrow reports the error and reconnects on the next explicit Run;
it never automatically resubmits the failed SQL. A failed heartbeat also disconnects
and stops background queries until the user runs a query again.

Qrow's idle timeout is separate from Kyuubi's engine idle timeout. Kyuubi's
[engine shutdown check](https://github.com/apache/kyuubi/blob/master/kyuubi-common/src/main/scala/org/apache/kyuubi/session/SessionManager.scala)
requires no active user sessions. Releasing Qrow's sessions allows that timeout
to take effect, but other clients or tabs can still hold the engine open. There
are no heartbeat requests when Keep connected is off.

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

Install and enable the development checks and Git hooks:

```sh
brew install shellcheck actionlint
sh scripts/install-check-tools.sh
sh scripts/install-hooks.sh
sh scripts/check.sh
```

See [Quality checks](docs/QUALITY.md) for staged-snapshot hooks, CI, coverage,
performance budgets, and dependency policy. The hooks do not replace the running
application or access real connections.

The basic Rust checks remain available individually:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

For the disposable real-server suite and native UI release checks, see
[End-to-end testing](docs/E2E.md). Run the backend suite with
`sh scripts/check.sh backend-e2e`; run the native suite with
`sh scripts/check.sh ui-e2e` from a macOS session configured for automation.

Local protocol tests exercise SASL authentication, session parameters, async
execution, result metadata, exact decimal/null handling, batched fetching,
cancellation, and dropped connections. They do not prove server configuration,
engine isolation, or cancellation behavior in a deployed Kyuubi instance.

UI initialization time is printed to stderr. This is a local diagnostic, not
a measurement of cold launch to first visible display.

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
- `src/ui.rs`: workspace state, worker events, and session commands.
- `src/ui/workspace_view.rs`: GPUI Kit workspace layout and window overlays.
- `src/ui/profile_view.rs`: connection settings popup.
- `src/ui/results.rs`: virtualized results, column metadata, and clipboard actions.
- `src/storage.rs`: workspace persistence and macOS Keychain access.
- `src/sql.rs`: SQL lexer and single-statement validation.
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

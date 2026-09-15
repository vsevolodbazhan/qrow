# Qrow

A Rust-based SQL workbench for macOS.

I'm a data engineer. I run SQL every day, and for years my tool of choice was
DataGrip. It worked well until it didn't: slow startup times, a heavy memory
footprint, and regular freezing that came with the territory of running on the
JVM. I went looking for an alternative. DBeaver was out there, sure, but it felt
like an app from the past.

Building a whole new SQL client from scratch wasn't realistic on top of a
full-time job — until AI models got good enough to make it one. I'm not a
professional developer, and I can't write Rust myself. What I bring is years of
hands-on SQL and data-engineering experience, and a strong sense of what a
workbench like this should feel like to use. An AI wrote the code; I directed
every decision, reviewed the result, and I use Qrow as my own daily driver
against a real Kyuubi connection.

Connect to Spark through Kyuubi, edit SQL, run queries in parallel tabs, and
inspect results without a local JVM or web UI. The interface uses GPUI Kit 0.6.1
with native Metal rendering. SQL highlighting uses Tree-sitter. The default
theme uses One Dark-style blue-gray colors, with query tabs, a connection
sidebar, a resizable editor and results table, and connection settings in a
popup dialog.

![Qrow screenshot placeholder](docs/screenshot.png)

## Status and limitations

Qrow is a personal tool, scoped to what I actually use day to day, not a
general-purpose client:

- **Kyuubi/Spark only.** The only connector implemented is HiveServer2 over
  SASL PLAIN, matching the deployment I connect to daily. Trino, ODBC, and
  other backends aren't implemented.
- **Apple Silicon macOS only.** That's the hardware I run every day; there's
  no Intel or other-OS build.
- **Ad-hoc signed, not notarized.** I haven't paid for an Apple Developer
  Program membership yet, so there's no notarized release — build from source
  (see below).
- **Tested by one person, against one deployment.** The HiveServer2 connector
  has local wire-level protocol tests, and I've confirmed it works against my
  own real Kyuubi connection. That doesn't establish compatibility with other
  Kyuubi deployments, and not every cancellation or failure scenario has been
  exercised outside my own setup.

## Prerequisites

- macOS. The prototype has been built and tested on Apple Silicon.
- A current stable Rust toolchain, including `cargo` and `rustc`.
- Xcode command-line tools. Install them with `xcode-select --install` if needed.
- [uv](https://docs.astral.sh/uv/), which provisions the pinned Python 3.11+
  toolchain used by the packaging, dependency-license, and code-generation
  scripts. A separately installed Python is not required.

Check your tools:

```sh
rustc --version
cargo --version
xcode-select -p
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
sh scripts/package/macos.sh
open dist/Qrow.app
```

The script builds for the current Mac's architecture and produces `dist/Qrow.app`
and `dist/Qrow-macos.zip`. On an Apple Silicon Mac these are ARM64 builds. The app
is locally ad-hoc signed, not notarized for public distribution. No separately
installed database driver is required.

The icon compiler is portable and can run on Linux before the final macOS build:

```sh
uv run --locked python scripts/package/icon.py assets/app-icons/macos/qrow.png Qrow.icns
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

See [Usage](docs/USAGE.md) for connection internals, tab and cancellation
behavior, result streaming and copy semantics, idle/reconnect handling,
appearance settings, and workspace storage.

## Development

See [Quality checks](docs/QUALITY.md) for local validation, Git hooks, and CI.
See [End-to-end testing](docs/E2E.md) for the real-server and native UI suites.
See [Architecture](docs/ARCHITECTURE.md) for the code map and Thrift bindings.

## Issues & contributions

Issues and discussion are welcome — if something breaks, or you have ideas,
open an issue. The project isn't set up for external pull requests right now.
It's MIT-licensed, so if you want to take it in your own direction, fork it.

## License

MIT — see [LICENSE](LICENSE).

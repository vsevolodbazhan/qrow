<p align="center">
    <img src="assets/app-icons/macos/qrow.png" alt="Qrow macOS Icon" width="128" height="128">
</p>

# Qrow

Desktop SQL workbench, written in Rust. Built with [GPUI](https://gpui.rs/) and [GPUI Kit](https://github.com/longbridge/gpui-kit).

![Qrow demo with the schema catalog and assistant](assets/screenshots/demo.png)

## Why

I'm a data engineer. I run SQL every day, and for years my tool of choice was
[DataGrip](https://www.jetbrains.com/datagrip/). It worked well until it didn't: slow startup times, a heavy memory
footprint, regular freezing, and all the usual quirks that come from running on the JVM.
I went looking for an alternative and found none suited to me.

Building a whole new SQL client from scratch on top of a
full-time job wasn't realistic for me — until AI models got good enough, that is.

I'm not a Rust developer, so practically every line of code here is written by one
model or the other. I bring in my direction, general coding expertise, and product sense.

## Inspiration

Qrow is heavily inspired by DataGrip and [Zed](https://zed.dev/). It's built using [GPUI](https://gpui.rs/), which is a UI framework by Zed's creators.

## Features

The shortcuts below use macOS symbols. On Linux and Windows, use Ctrl in place of ⌘.

### Connections

Qrow connects directly to Spark (HiveServer2), Postgres, and Trino. It does not
need a JVM, an ODBC driver, or a separately installed database driver.

- **Spark (HiveServer2)** uses SASL PLAIN, with a password or an OpenID
  Connect access token.
- **Postgres** uses password authentication with TLS encryption and optional
  certificate verification.
- **Trino** uses HTTP or HTTPS with a username, a password, or the browser
  sign-in of the coordinator.

![Qrow demo with connection creation menu open](assets/screenshots/demo-connections.png)

Qrow keeps passwords in macOS Keychain, the Linux desktop secret store, or
Windows Credential Manager. One OpenID Connect sign-in can authenticate
several connections, and you can copy its settings to share them with a
colleague. Drag connections to change their order. See
[connections](docs/connections.md).

### Queries and Results

Each connection has its own query tabs, and each tab has its own session.
Tabs run queries at the same time, also when you select another connection.
Press **⌘Enter** to run the statement at the cursor or the selected SQL. A light tint shows the statement that runs. A
status dot on each tab and connection shows work, unread results, and errors.

![Qrow demo with query results shown](assets/screenshots/demo-results.png)

Results show in pages of 1,000 rows as they arrive. Copy a cell or a row with
its full value. The Logs panel of each tab shows the SQL, the session events,
and the errors. **Activity** (**⇧⌘U**) shows the background work of each
connection: schema refreshes, keep-alives, sessions, and queries. See
[queries](docs/queries.md), [results](docs/results.md), and
[activity](docs/activity.md).

### Data Export

Export a result to CSV, TSV, Markdown, JSON, or Parquet. Save it to a file, or
copy it to the clipboard. The export includes all rows, also when the preview
shows only some of them.

- Qrow reads the remaining rows from the open cursor, or uses a download that
  it kept. It runs the query again only when neither is available.
- CSV presets include Excel, with a byte-order mark and formula escaping.
- Parquet keeps booleans, numbers, exact decimals, dates, timestamps, and
  binary values in their types.
- Large exports write the file in the background and show a progress
  notification.

![Qrow demo with data export window](assets/screenshots/demo-export.png)

See [export results](docs/results.md#export-results).

### Schema Catalog

The Connections sidebar is a tree of schemas, tables, views, and columns.
Qrow keeps a copy of the catalog on your computer, so the tree shows it also
when the connection has no session.

- **Refresh** a connection, a schema, or a table manually, or automatically
  after a period while the connection is connected. A refresh uses its own
  session. The next refresh continues a refresh that stopped.
- **Share** one catalog between connections that read the same database catalog.
  A refresh of one connection fills the tree of all of them.
- **Show or hide** schemas with glob patterns, for example `sales_*`.
- **Search** tables and views in all connections, and copy or insert their
  quoted names into the editor.

![Qrow demo with schema catalog](assets/screenshots/demo-catalog.png)

See [browse schemas](docs/connections.md#browse-schemas).

### Integrations with dbt

Attach a dbt project to a connection. Qrow reads the `manifest.json` that dbt
writes, and can read it again each time dbt changes it. Qrow does not run dbt.

- Tables that dbt builds show their dbt kind in the schema tree, and their
  description in a tooltip.
- **Show dbt details** opens the description, columns, tests, lineage, and
  the compiled and raw SQL of a model. Click a parent or a child to follow the
  lineage.
- A schema mapping, for example `dbt_dev_* = *`, matches development schemas
  with catalog schemas. The form shows how many models match tables.

![Qrow demo with dbt model details](assets/screenshots/demo-dbt.png)

See [attach a dbt project](docs/connections.md#attach-a-dbt-project).

### Assistant

Qrow can open an AI assistant beside your query tab. It runs on a separately
installed Codex or Claude Code executable with your own sign-in, so you can
use your OpenAI or Claude subscription. The assistant is off by default.
Enable it in **Qrow → Settings… → Assistant**, then press **⌘R**.

- Each conversation writes SQL in its own query tab, so you can work on it
  together. Several conversations can work at the same time.
- The assistant looks up tables and columns in the schema catalog, and reads
  descriptions, tests, lineage, and model SQL from the dbt project.
- Write **Assistant Notes** for a connection to give the assistant facts
  about its data.
- Qrow asks before an assistant query runs, or runs assistant queries
  automatically. You select the mode for each conversation.
- Qrow keeps a copy of each conversation on your computer. You can read
  earlier conversations before the harness starts.

![Qrow demo with assistant conversation](assets/screenshots/demo-assistant.png)

See [assistant](docs/assistant.md) for setup, data sharing, and query safety.

## Limitations

Qrow is a personal tool, tailored to my setup and my needs, so:

- **Spark (HiveServer2), Postgres, and Trino only.** DuckDB is planned.
  Kerberos and SSH tunnels are not supported. See
  [connections](docs/connections.md) for connector limits.
- **Bring your own assistant.** The assistant needs a separately installed Codex
  or Claude Code executable and your own subscription or API key.
- **Unsigned packages.** Signing and notarization need paid developer
  certificates. Qrow is early in development, so I don't pay for them yet.
  macOS shows a Gatekeeper warning and Windows shows a SmartScreen warning
  the first time you open Qrow. [Installation](#installation) shows how to
  continue.
- **Windows support is new.** It has had less real-desktop use than macOS and
  Linux. Please report problems in
  [issues](https://github.com/vsevolodbazhan/qrow/issues).

## Installation

Download packages from [GitHub Releases](https://github.com/vsevolodbazhan/qrow/releases).
Replace `VERSION` in the file names below with the release version.

| System | Requirements | Package |
| --- | --- | --- |
| macOS | macOS 12 or later, a Metal-capable GPU | `Qrow-VERSION-arm64.dmg` (Apple Silicon) or `Qrow-VERSION-x86_64.dmg` (Intel) |
| Windows | Windows 10 or later | `Qrow-VERSION-windows-x86_64-setup.exe` (Intel/AMD) or `Qrow-VERSION-windows-aarch64-setup.exe` (ARM64) |
| Linux | Wayland or X11 desktop, Vulkan, a Secret Service provider | `Qrow-VERSION-linux-x86_64.tar.gz` or `Qrow-VERSION-linux-aarch64.tar.gz` |

### macOS

Install Qrow from the [Homebrew tap](https://github.com/vsevolodbazhan/homebrew-qrow):

```sh
brew tap vsevolodbazhan/qrow
brew install --cask vsevolodbazhan/qrow/qrow
```

To install the latest nightly build, use the separate cask:

```sh
brew install --cask vsevolodbazhan/qrow/qrow@nightly
```

The Homebrew casks install the Apple Silicon build only. On an Intel Mac,
download the `x86_64` DMG from GitHub Releases.

Alternatively, download the DMG for your Mac, open it, and drag Qrow to
Applications.

#### Gatekeeper warning

<img src="assets/screenshots/gatekeeper/warning.png" alt="Gatekeeper Warning" width="384" height="384">

macOS shows this warning because Qrow is not notarized. To open Qrow:

1. Open Qrow. You'll see the warning. Close it.
2. Open **System Settings → Privacy & Security** and click **Open Anyway**.

![Gatekeeper Settings](assets/screenshots/gatekeeper/settings.png)

### Windows

Download the installer for your processor and run it. The installer installs Qrow for your user account and adds it to the Start
menu. It does not need administrator rights. To install for all users, select
that option in the first dialog.

- **Upgrade:** run the installer of the new version. It closes a running Qrow
  first.
- **Remove:** open **Settings → Apps → Installed apps**, find Qrow, and select
  **Uninstall**. Your workspace and settings stay on the computer.

#### SmartScreen warning

The installer is not signed, so your browser can warn about the download, and
Windows can show **Windows protected your PC** when you run it. To continue:

1. In the browser download list, select **Keep** if the browser asks.
2. Run the installer. In the SmartScreen dialog, select **More info**.
3. Make sure that the file name is a Qrow installer, then select **Run anyway**.

### Linux

Download the archive for your architecture. Extract it into `~/.local` and make
sure that `~/.local/bin` is in your `PATH`:

```sh
mkdir -p ~/.local
tar -xzf Qrow-VERSION-linux-ARCH.tar.gz -C ~/.local
```

The archive includes a desktop launcher. Saved passwords need a desktop Secret
Service, such as GNOME Keyring. The archive uses the system libraries and is
built on Ubuntu 24.04. Use it on that or a newer, compatible distribution.

Use the application menu at the top left for Settings and other commands.

## Build

### Prerequisites

- Rust from [rustup](https://rustup.rs/). `rust-toolchain.toml` selects the
  pinned toolchain.
- [uv](https://docs.astral.sh/uv/) for `./qtest` and the build scripts. It
  installs the Python version that the scripts need.
- On macOS, the Xcode command-line tools: `xcode-select --install`.
- On Windows, Visual Studio Build Tools with **Desktop development with C++**
  and the Windows SDK.
- On Linux, a C and C++ toolchain, pkg-config, CMake, and the Wayland, X11,
  xkbcommon, fontconfig, FreeType, and Vulkan development libraries. On
  Ubuntu, `sh scripts/ci/linux-build-deps.sh` installs them.

Java, ODBC drivers, and the Thrift compiler are not required.

### Run

Compile and launch the debug build:

```sh
cargo run --locked --bin qrow
```

Build and launch an optimized executable:

```sh
cargo build --locked --release --bin qrow
./target/release/qrow
```

Use the release build to check startup time and responsiveness.

To build a DMG, a Linux archive, or a Windows installer, see
[package the app](docs/development.md#package-the-app).

## Preview

Launch the demo to see the SQL editor, results, schema catalog, and dbt
details without a database server:

```sh
cargo run --locked --bin qrow -- --demo
```

Or, after you build the release executable:

```sh
./target/release/qrow --demo
```

The first connection opens with the columns of `avia.bookings`. Expand other
tables or use **Search…** to look at the catalog. In either Rivendell
connection, right-click a table and select **Show dbt details**.

The demo uses synthetic data. It does not connect to a server, read saved
passwords, or change your workspace.

## Documentation

See the [documentation index](docs/README.md) for application behavior,
architecture, development, and testing.

## Contributions

Issues and discussion are welcome, but the project isn't set up for external
pull requests right now. You are free to fork it, though.

## License

Qrow uses the [MIT License](LICENSE).
Third-party code retains its original licenses. See [NOTICE](NOTICE).

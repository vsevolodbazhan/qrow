<p align="center">
    <img src="assets/app-icons/macos/qrow.png" alt="Qrow macOS Icon" width="128" height="128">
</p>

# Qrow

Desktop SQL workbench, written in Rust. Built with [GPUI](https://gpui.rs/) and [GPUI Kit](https://github.com/longbridge/gpui-kit).

![Qrow demo with the schema catalog and assistant](assets/screenshots/demo.png)

## Overview

Qrow is a SQL client for data engineers and analysts. It is written in Rust
and draws its interface on the GPU. With no Electron and no JVM, it starts
quickly and stays responsive. It connects to databases without separate
drivers. Qrow is inspired by [DataGrip](https://www.jetbrains.com/datagrip/)
and [Zed](https://zed.dev/).

## Features

- **[Connections](docs/connections.md):** Spark (HiveServer2), Postgres, and
  Trino. Passwords stay in the system secret store.
- **[Queries](docs/queries.md):** query tabs that run at the same time, each
  with its own session.
- **[Results](docs/results.md):** paged results and export to CSV, TSV,
  Markdown, JSON, or Parquet.
- **[Schema catalog](docs/connections.md#browse-schemas):** a searchable tree
  of schemas, tables, and columns that is available offline.
- **[dbt](docs/connections.md#attach-a-dbt-project):** model descriptions,
  tests, lineage, and SQL from your dbt project.
- **[Assistant](docs/assistant.md):** an optional AI assistant that runs on
  your own Codex or Claude Code.

## Limitations

- Spark (HiveServer2), Postgres, and Trino only. DuckDB is planned. No
  Kerberos or SSH tunnels.
- The packages are not code-signed, so macOS and Windows show a warning on
  first open.
- macOS first. Windows and Linux are supported but can have rough edges.

## Installation

Download the package for your system from the
[latest release](https://github.com/vsevolodbazhan/qrow/releases/latest).
Qrow then [updates itself](docs/installation.md#updates).

| System | Package |
| --- | --- |
| macOS | `Qrow-VERSION-arm64.dmg` (Apple Silicon) or `Qrow-VERSION-x86_64.dmg` (Intel). Drag Qrow to Applications. |
| Windows | `Qrow-VERSION-windows-x86_64-setup.exe` (Intel/AMD) or `Qrow-VERSION-windows-aarch64-setup.exe` (ARM64). |
| Linux | `Qrow-VERSION-linux-x86_64.tar.gz` or `Qrow-VERSION-linux-aarch64.tar.gz`. Extract it into `~/.local`. |

See [installation](docs/installation.md) for requirements and the first-open
warnings.

Qrow has two channels:

- **Stable** for daily work. Choose it if you want fewer surprises.
- **Nightly** for the latest features and fixes. Choose it if you want to try
  changes early and report problems.

To change the channel, open **Settings → Updates → Release Channel**.

### Package managers

You can also install Qrow with [Homebrew](https://brew.sh/) on macOS or
[Scoop](https://scoop.sh/) on Windows. Then the package manager updates Qrow.

#### macOS

```sh
brew install --cask vsevolodbazhan/qrow/qrow
```

```sh
brew install --cask vsevolodbazhan/qrow/qrow@nightly
```

#### Windows

```powershell
scoop bucket add qrow https://github.com/vsevolodbazhan/scoop-qrow
scoop install qrow
```

```powershell
scoop bucket add qrow https://github.com/vsevolodbazhan/scoop-qrow
scoop install qrow-nightly
```

## Build

Install the [prerequisites](docs/development.md#build-prerequisites), then run a debug build with:

```sh
cargo run --locked --bin qrow
```

Add the `--release` flag to run an optimized release-like build:
```sh
cargo run --locked --release --bin qrow
```

See [package the app](docs/development.md#package-the-app) for release packages.

## Preview

Try Qrow with synthetic data, without a database server or access to your
workspace:

```sh
cargo run --locked --bin qrow -- --demo
```

## Documentation

See the [documentation index](docs/README.md).

## Contributions

Issues are welcome. The project does not accept external pull requests right
now, but you are free to fork it.

## License

[MIT](LICENSE). Third-party code keeps its original licenses. See [NOTICE](NOTICE).

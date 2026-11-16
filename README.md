<p align="center">
    <img src="assets/app-icons/macos/qrow.png" alt="Qrow macOS Icon" width="128" height="128">
</p>

# Qrow

Desktop SQL workbench, written in Rust.

![Qrow demo with the schema tree and assistant](assets/screenshots/demo.png)

## Overview

Qrow is a SQL client for data engineers and analysts. It is written in Rust using [GPUI](https://gpui.rs/) and [GPUI Kit](https://github.com/longbridge/gpui-kit). With no Electron and no JVM, it starts quickly and stays responsive. Qrow is inspired by [DataGrip](https://www.jetbrains.com/datagrip/)
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

- Spark (HiveServer2), Postgres, and Trino only.
- The packages are not code-signed, so macOS and Windows show a warning on
  first open.
- macOS first. Windows and Linux are supported but can have rough edges.

## Installation

Go to [qrow.dev](https://qrow.dev/#install) to get Qrow for your system, or
follow the steps below. After you install it, Qrow updates itself. See
[installation](docs/installation.md) for requirements and the first-open
warnings.

### macOS

Download `Qrow-VERSION-arm64.dmg` (Apple Silicon) or
`Qrow-VERSION-x86_64.dmg` (Intel) from the
[latest release](https://github.com/vsevolodbazhan/qrow/releases/latest).
Open it and drag Qrow to Applications.

Or install with [Homebrew](https://brew.sh/) (Apple Silicon only):

```sh
brew install --cask vsevolodbazhan/qrow/qrow
```

```sh
brew install --cask vsevolodbazhan/qrow/qrow@nightly
```

### Windows

Download `Qrow-VERSION-windows-x86_64-setup.exe` (Intel/AMD) or
`Qrow-VERSION-windows-aarch64-setup.exe` (ARM64) from the
[latest release](https://github.com/vsevolodbazhan/qrow/releases/latest) and
run it.

Or install with [Scoop](https://scoop.sh/):

```powershell
scoop bucket add qrow https://github.com/vsevolodbazhan/scoop-qrow
scoop install qrow
```

```powershell
scoop bucket add qrow https://github.com/vsevolodbazhan/scoop-qrow
scoop install qrow-nightly
```

### Linux

```sh
curl -fsSL https://qrow.dev/install.sh | sh
```

Or extract `Qrow-VERSION-linux-x86_64.tar.gz` or
`Qrow-VERSION-linux-aarch64.tar.gz` from the
[latest release](https://github.com/vsevolodbazhan/qrow/releases/latest) into
`~/.local`.

### Channels

- **Stable** for daily work. Choose it if you want fewer surprises.
- **Nightly** for the latest features and fixes. Choose it if you want to try
  changes early and report problems.

To change the channel, open **Settings → Updates → Release Channel**. A copy
from Homebrew or Scoop keeps the channel of its package.

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

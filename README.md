<p align="center">
    <img src="assets/app-icons/macos/qrow.png" alt="Qrow macOS Icon" width="128" height="128">
</p>

# Qrow

Desktop SQL workbench, written in Rust. Built with [GPUI](https://gpui.rs/) and [GPUI Kit](https://github.com/longbridge/gpui-kit).

![Qrow demo with the schema catalog and assistant](assets/screenshots/demo.png)

## Description

Qrow connects directly to Kyuubi (Spark), Postgres, and Trino. It does not
need a JVM, an ODBC driver, or a separately installed database driver. It has
query tabs with their own sessions, data export, a schema catalog that stays
available offline, dbt project details, and an optional AI assistant.

See [qrow.dev](https://qrow.dev) for the features and the reasons for the project.

## Install

See the [install instructions](https://qrow.dev/#install) on the website for
Homebrew, the macOS DMG, the Linux archive, and the Windows archive. All builds
are on the [releases page](https://github.com/vsevolodbazhan/qrow/releases).

## Requirements and limitations

Qrow is a personal tool, tailored to my setup and my needs, so:

- **Kyuubi/Spark, Postgres, and Trino only.** DuckDB is planned. Kerberos and
  SSH tunnels are not supported. See [connections](docs/connections.md) for
  connector limits.
- **macOS 12 or later.** Apple Silicon and Intel builds need a Metal-capable GPU.
  The Intel build uses `x86_64-apple-darwin` and needs SSE4.1. AVX is not required.
  The Homebrew cask installs only the Apple Silicon build. On an Intel Mac, use the `x86_64` DMG.
- **Linux.** Qrow needs a Wayland or X11 desktop, Vulkan, and a Secret Service provider,
  such as GNOME Keyring. Release archives target x86-64 and ARM64 Linux and are built on
  Ubuntu 24.04. They use system libraries and need a compatible distribution.
  Use the application menu at the top left for Settings and other commands.
  The window controls are on the right when the desktop allows Qrow to draw them.
- **Windows.** Release archives target x86-64 and ARM64. There is no installer, and the
  unsigned binary can cause a SmartScreen warning. UI, browser sign-in, assistant shutdown,
  and title bar behavior need real Windows testing.
- **Linux and Windows builds are nightly only.** The latest stable release has only macOS builds.
- **Bring your own assistant.** The assistant needs a separately installed Codex
  or Claude Code executable and your own subscription or API key.
- **Not notarized.** Certification and notarization of macOS apps requires Apple Developer ID,
  which is a paid membership. As Qrow is mostly a personal tool that is early in development,
  I'm not planning to pay for that membership yet. Releases use ad hoc code signing, so
  Gatekeeper blocks the first launch. The [install instructions](https://qrow.dev/#gatekeeper)
  show how to open the app.

## Build

### Prerequisites

- Rust from [rustup](https://rustup.rs/). `rust-toolchain.toml` selects the pinned toolchain.
- On Windows, Visual Studio Build Tools with Desktop development with C++ and the Windows SDK.
- On macOS, Xcode command-line tools. Install them with `xcode-select --install` if needed.
- On Linux, a C and C++ toolchain, pkg-config, CMake, and the Wayland, X11,
  xkbcommon, xkbcommon-x11, fontconfig, FreeType, and Vulkan development libraries.
  On Ubuntu, `sh scripts/ci/linux-build-deps.sh` installs the libraries.
- [uv](https://docs.astral.sh/uv/), which provisions the pinned Python 3.11+
  toolchain used by `./qtest` and by the packaging, dependency-license, and
  code-generation scripts. A separately installed Python is not required.

Check your tools:

```sh
rustc --version
cargo --version
uv --version
```

Java, ODBC drivers, and the Thrift compiler are not required to build or run Qrow. On macOS, GPUI uses runtime Metal
shader compilation, so the separate Xcode Metal compiler is not required either.

### Debug

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

### Package

On macOS, build the optimized application bundle and launch it:

```sh
sh scripts/package/macos.sh
open dist/Qrow.app
```

The script builds for the current Mac's architecture and produces `dist/Qrow.app`,
`dist/Qrow-macos.zip`, and the installer image `dist/Qrow-VERSION-ARCH.dmg`.
After changing the source, quit Qrow, rerun the packaging script, and reopen the
app. The packaged app does not update automatically when you run `cargo build`.
To build for the other Mac architecture or another minimum macOS version, see
[macOS builds](docs/development.md#macos-builds).

On Linux, build a native archive in `target/package`:

```sh
sh scripts/package/linux.sh
```

The archive uses the libraries of the build system. Build it on a system that
is compatible with the target computers.

On Windows, build the native unsigned archive:

```sh
uv run --locked --no-dev python scripts/package/windows.py
```

See [Windows builds](docs/development.md#windows-builds) for cross checks from other systems.

## Demo

Launch the demo to inspect the SQL editor, results, schema catalog, and dbt
details. The first connection opens with the columns of `avia.bookings`.
Expand other tables or use **Search…** to inspect the catalog.
In either Rivendell connection, right-click a table and select **Show dbt
details** to read its description, columns, tests, lineage, and SQL.

The demo uses synthetic data. It does not connect to a server, access Keychain,
or change the saved workspace. It writes a temporary dbt manifest and removes
the file when the window closes:

```sh
cargo run --locked --bin qrow -- --demo
```

Or, after building the release executable:

```sh
./target/release/qrow --demo
```

## Documentation

See the [documentation index](docs/README.md) for application behavior,
architecture, development, and end-to-end testing.

## Contributions

Issues and discussion are welcome, but the project isn't set up for external pull requests right now.
You are free to fork it, though.

## License

Qrow uses the [MIT License](LICENSE).
Third-party code retains its original licenses. See [NOTICE](NOTICE).

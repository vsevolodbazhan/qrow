# Development

Use local checks to verify code, scripts, and dependency policy before a commit.
These checks do not start Kyuubi or exercise the native UI. Real-server and
native UI tests have a separate [end-to-end testing guide](end-to-end-testing.md).

For application prerequisites, builds, packaging, and demo launch commands, see
the [project README](../README.md). Run the commands below from the repository
root. The full local suite requires macOS.

## Set up check tools

Install the script linters:

```sh
brew install shellcheck actionlint
```

Install the pinned Cargo check tools:

```sh
sh scripts/core/install.sh
```

Install the repository hooks:

```sh
sh scripts/hooks/install.sh
```

[rust-toolchain.toml](../rust-toolchain.toml) selects Rust, rustfmt, and Clippy.
The [tool installer](../scripts/core/install.sh) selects the dependency and
coverage tools. Cargo checks use the lockfile. Python scripts use `uv` and the
repository's pinned environment.

## Run checks

Use [scripts/check.sh](../scripts/check.sh) as the check entry point:

| Command | Purpose |
| --- | --- |
| `sh scripts/check.sh` | Full local suite, excluding packaging and end-to-end tests. |
| `sh scripts/check.sh core/backend` | Formatting, core lint, core tests, and Rust API documentation. |
| `sh scripts/check.sh core/macos` | Full application lint and tests on macOS. |
| `sh scripts/check.sh core/scripts` | Shell and workflow lint, automation tests, and policy checks. |
| `sh scripts/check.sh core/dependencies` | Dependency audit, license policy, and unused dependencies. |
| `sh scripts/check.sh core/coverage` | Core line coverage with an enforced floor. |
| `sh scripts/check.sh core/performance` | SQL validation benchmarks with enforced budgets. |
| `sh scripts/check.sh hook` | Fast checks used by the pre-commit hook. |

Use `core/backend` for backend changes, `core/macos` for UI changes, and
`core/scripts` for automation changes. Run the full suite for dependency changes
or changes that affect several components. Native lint and core coverage do not
verify pointer, keyboard, or window behavior.

These checks do not run real Kyuubi queries or retrieve user credentials.
The Keychain integration test remains opt-in. End-to-end tests are ignored in
ordinary Cargo runs.

## Check the native UI

Use the release [demo](../README.md#preview) for visual checks without database,
Keychain, or workspace access. Test the interactions affected by a UI change.
Compilation alone does not establish correct interaction behavior.

For persistence checks, use a temporary workspace and synthetic profiles:

```sh
qrow_test_data_dir=$(mktemp -d)
QROW_DATA_DIR="$qrow_test_data_dir" ./target/release/qrow
```

`QROW_DATA_DIR` isolates workspace files only. Use new profile UUIDs and synthetic
credentials. Never use the user's workspace or passwords as test fixtures.
Check both application quit and last-window closure when changing persistence.

When changing table or splitter behavior, check dragging, release, the next click,
both scroll axes, and modal overlays. Verify SQL selection with emoji and
non-Latin text. Report an automation failure as a verification gap.

Use release builds for performance measurements. `cargo build` does not update
`dist/Qrow.app`. Follow the [packaging instructions](../README.md#release) when
you need a new distributable. Do not replace the app while someone is testing it.

## Hooks and continuous integration

Pre-commit checks the staged Git snapshot and selects checks by changed path.
Pre-push checks each distinct revision being pushed with the full local suite.
Hooks export snapshots into temporary directories and share a build cache under
`target/hook-checks`. They do not stash, restage, or modify working files.
They do not package the application.

Stage required code and configuration together. A passing working-copy check
does not prove that the staged snapshot passes. Markdown changes outside the
script paths do not trigger pre-commit checks. Changes under `scripts/` trigger
script checks, including changes to its README.

The [core workflow](../.github/workflows/core.yml) selects jobs from changed
paths. Draft pull requests skip the change filter and selected checks. Mark a
pull request ready for review to run them. Linux runs headless core and
automation checks. macOS builds the application, measures SQL validation, and
packages the release. A manual dispatch runs every core job. Coverage and
package reports are retained for 14 days. See [End-to-end testing](end-to-end-testing.md#continuous-integration) for the real-server workflow and merge gate.

## Test boundaries and budgets

Local protocol fixtures test the connector without a real Spark deployment.
Worker tests exercise session coordination and bounded fetching. Property tests
exercise SQL validation with arbitrary Unicode and quoting. Preserve generated
Proptest regression seeds when fixing a discovered failure.

The core library must build with `--no-default-features`. Core coverage excludes
the GPUI frontend and generated bindings. The line-coverage floor is 80%.
Generated bindings are also excluded from normal style lint. Qrow forbids unsafe
code in its own sources, including generated bindings. Third-party dependencies
can contain unsafe code.

The [SQL benchmark](../benches/sql.rs) measures validation after warmup and reports
the median of 21 samples. Budgets are 5 ms at 10 KB, 25 ms at 100 KB, and 250 ms
at 1 MB. These are regression thresholds, not rendering or Spark latency targets.

After packaging, check the release size:

```sh
uv run --locked python scripts/core/size.py
```

The [size checker](../scripts/core/size.py) allows 24 MiB for the executable and
10 MiB for the zipped bundle. Investigate growth before changing a budget.

The startup log measures UI initialization. It does not measure cold launch to
the first visible frame. Report the build, hardware, measurement method, and
verification gaps when publishing performance results.

## Dependency maintenance

Keep lint rules, coverage floors, performance budgets, and dependency policy
intact when fixing a failure. Investigate the cause instead of weakening checks.

[Cargo.toml](../Cargo.toml) defines application lint rules.
[deny.toml](../deny.toml) defines dependency sources, licenses, bans, and advisory
exceptions. [dependency-reviews.toml](../dependency-reviews.toml) records the scoped
license review. These files are authoritative for exact versions and review dates.
The [policy checker](../scripts/core/policy.py) rejects expired or mismatched
exceptions and unpinned GitHub Actions. The dependency audit targets macOS ARM64.

GPUI brings some incompatible transitive versions. Duplicate versions produce
warnings. Advisory and license exceptions must remain specific and justified.
Packaging includes third-party license notices. Upgrade GPUI Kit and its
framework components as a compatible set. Preserve runtime Metal shaders unless
the build requirements deliberately change.

## Generated bindings

Building Qrow does not require the Thrift compiler. To regenerate the checked-in
bindings, install Thrift 0.24.0 and run:

```sh
sh scripts/generate/thrift.sh
```

The [generation script](../scripts/generate/thrift.sh) uses the
[vendored interface](../vendor/TCLIService.thrift) and applies corrections to the
Rust generator output. Change the interface and script when needed. Do not make
binding edits that cannot be reproduced by generation.

## Probe an existing connection

The probe uses a saved profile and its real Keychain password. Run it only when
you intend to access that deployment:

```sh
cargo run --locked --bin qrow-probe -- "your profile name"
```

The [probe](../src/bin/qrow-probe.rs) initializes a session, runs `SELECT 1`,
checks the result, and closes the session. It does not verify engine sharing
or cancellation.

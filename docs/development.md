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

Ruff is installed by `uv` from the locked development dependency group.

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
coverage tools. Cargo checks use the lockfile. Python scripts use `uv`, the
repository's pinned environment, and Ruff.

## Run checks

Use [scripts/check.sh](../scripts/check.sh) as the check entry point:

| Command | Purpose |
| --- | --- |
| `sh scripts/check.sh` | Full local suite, excluding packaging and end-to-end tests. |
| `sh scripts/check.sh core/backend` | Formatting, core lint, core tests, and Rust API documentation. |
| `sh scripts/check.sh core/macos` | Full application lint and tests on macOS. |
| `sh scripts/check.sh core/scripts` | ShellCheck, Actionlint, Ruff, and script unit tests. |
| `sh scripts/check.sh core/dependencies` | Dependency audit, license policy, unused dependencies, and `policy.py`. |
| `sh scripts/check.sh core/coverage` | Core line coverage with an enforced floor. |
| `sh scripts/check.sh core/performance` | SQL validation benchmarks with enforced budgets. |
| `sh scripts/check.sh hook` | Fast checks used by the pre-commit hook. |

The `dependencies.sh` script owns the `policy.py` check. The `scripts.sh`
script does not run `policy.py`.

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

The macOS end-to-end run uses a synthetic Codex app server to check assistant
opt-in, the docked pane, a chat turn, appended SQL, Undo, query approval,
statement targeting, tab changes during a turn, automatic query execution, and
generated conversation titles. It does not use a Codex account.

The release profile favors size so the optional assistant stays within the
macOS package budget. Run `core/performance` after a release-profile change.

Stage required code and configuration together. A passing working-copy check
does not prove that the staged snapshot passes. Markdown changes outside the
script paths do not trigger pre-commit checks. Changes under `scripts/` trigger
script checks, including changes to its README.

The [test workflow](../.github/workflows/test.yml) checks pushes to `main`,
manual dispatches, and pull requests with the
`opened`, `reopened`, `synchronize`, `ready_for_review`, and
`converted_to_draft` actions. It runs all jobs for a non-draft pull request. A
draft pull request, including a `converted_to_draft` event, starts no jobs.

The workflow runs one job at a time in this order:

```text
dependencies -> scripts -> backend -> macos
```

A failed job skips all jobs that follow it. Every job checks out the same pull
request merge result. A newer run cancels an older run for the same pull request
or branch, including manual runs.

E2E tests run locally only. They do not run in GitHub Actions. Use the
[end-to-end testing guide](end-to-end-testing.md) to run them.

The workflow retains `core-coverage` and `macos-package-and-performance` for one
day. Configure these individual checks as required branch-protection checks:

```text
test / dependencies
test / scripts
test / backend
test / macos
```

There is no aggregate CI gate. The [release workflow](../.github/workflows/release.yml)
is manual. Use it to publish a nightly or stable release after the core checks
pass.

The release workflow accepts an optional commit SHA or ref. Leave the field
blank to use the latest commit on the branch selected for the workflow run. It
reads the application version from `Cargo.toml`. The stable tag is
`v<version>`. The nightly tag is
`v<version>-nightly.<UTC date>.<workflow run number>`.

The workflow runs `sh scripts/check.sh core/backend` and
`sh scripts/check.sh core/macos`. It does not run E2E tests. The macOS job
builds the application bundle, creates a DMG, and publishes it as the GitHub
Release asset. The workflow uses the latest non-draft release on the selected
channel as the changelog start tag. If the channel has no previous release, it
writes the target commit history as the changelog.

The jobs run in this order:

```text
resolve-target -> test-core-backend -> test-core-macos -> package -> publish
```

The publish job creates the release tag before it creates the GitHub Release.
The first release on a channel can run without a previous release or tag.

The packaged application shows the channel-specific release version in the
About dialog. The macOS bundle metadata keeps the numeric version from
`Cargo.toml`.

The custom [Homebrew tap](https://github.com/vsevolodbazhan/homebrew-qrow)
publishes the latest stable release as `qrow` and the latest prerelease as
`qrow@nightly`. A scheduled job in the tap checks the public GitHub releases
every 15 minutes, downloads each new DMG, calculates its SHA-256 checksum, and
updates the cask. The tap does not need a secret in this repository.

Install the stable cask with:

```sh
brew tap vsevolodbazhan/qrow
brew install --cask vsevolodbazhan/qrow/qrow
```

Install the nightly cask with:

```sh
brew install --cask vsevolodbazhan/qrow/qrow@nightly
```

The current release package uses ad hoc code signing. Homebrew can install the
custom cask, but macOS Gatekeeper may warn until the app is signed and notarized
with Apple Developer ID.

See
[End-to-end testing](end-to-end-testing.md#continuous-integration) for the
real-server and native UI details. Local checks cannot verify GitHub event
filters, run cancellation, artifact transfer, or branch protection settings.

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

## Build identity

[Cargo.toml](../Cargo.toml) holds the version. The About dialog reads it, and
the [packaging script](../scripts/package/macos.sh) puts it in
`CFBundleShortVersionString` through
[the version reader](../scripts/package/version.py). Change the version in one
place only. macOS accepts up to three numbers in a version; packaging stops if
the version has a different form.

The [build script](../build.rs) records the short Git commit of the working tree
in the executable. The About dialog shows it with the version. Cargo runs
the script again when `HEAD` or a reference changes. A build without Git history
reports the version alone. Set `QROW_COMMIT` to record the commit for such a
build:

```sh
QROW_COMMIT=dcc75d4fd874 cargo build --locked --release --bin qrow
```

## Generated icon asset

The executable embeds [the small application icon](../assets/app-icons/qrow-256.png)
for the About dialog. After a change of the
[source icon](../assets/app-icons/macos/qrow.png), make the asset again:

```sh
uv run --locked python scripts/generate/app_icon.py
```

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

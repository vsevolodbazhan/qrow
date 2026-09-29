# Development

Use local checks to verify code, scripts, and dependency policy before a commit.
[Testing](testing.md) describes `./qtest`, the command for all checks and
tests, including the suites with real servers and the desktop suite.

For application prerequisites, builds, packaging, and demo launch commands, see
the [project README](../README.md). Run the commands below from the repository
root. The full local suite requires macOS.

## Set up check tools

Install the script linters and the pinned Cargo check tools, then the
repository hooks:

```sh
brew install shellcheck actionlint
./qtest install
sh scripts/hooks/install.sh
./qtest doctor
```

[rust-toolchain.toml](../rust-toolchain.toml) selects Rust, rustfmt, and Clippy.
The [qtest catalog](../scripts/qtest/catalog.py) pins cargo-nextest, the
dependency tools, and the coverage tool. Cargo checks use the lockfile. Python
scripts use `uv`, the repository's pinned environment, and Ruff.

## Run checks

Run `./qtest` for the fast local checks, and `./qtest run all` for the full
local suite. [Testing](testing.md#suites) lists each suite. Use `./qtest run
--changed` to run the suites that your changes affect. Core coverage does not
include the UI.

These checks do not run real Kyuubi queries or retrieve user credentials.
The suites with servers run only when you select them.

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

Pre-commit checks the staged Git snapshot and selects suites by changed path.
Pre-push checks each distinct revision being pushed with the full local suite.
[Testing](testing.md#hooks-and-continuous-integration) gives the commands.
Hooks export snapshots into temporary directories and share a build cache under
`target/hook-checks`. They do not stash, restage, or modify working files.
They do not package the application.

The release profile favors size so the optional assistant stays within the
macOS package budget.

Stage required code and configuration together. A passing working-copy check
does not prove that the staged snapshot passes. Markdown changes other than
`docs/testing.md` do not trigger pre-commit checks. Changes under `scripts/`
trigger script checks, including changes to its README.

The [test workflow](../.github/workflows/test.yml) checks pushes to `main`,
manual dispatches, and pull requests with the
`opened`, `reopened`, `synchronize`, `ready_for_review`, and
`converted_to_draft` actions. It runs all jobs for a non-draft pull request. A
draft pull request, including a `converted_to_draft` event, starts no jobs.

Each job waits for the less expensive checks that can predict its failure.
`e2e-backend` does not use the result of `core-macos`, so the two run at the
same time:

```text
core-dependencies -> core-scripts -> core-backend -> core-macos  -> e2e-macos
                                                  -> e2e-backend -^
```

`e2e-macos` waits for `core-macos` and `e2e-backend`. A failed job skips all
jobs that wait for it, directly or through another job. Every job checks out the same pull
request merge result. Core jobs run for fork pull requests. E2E jobs run for
pushes, manual dispatches, and pull requests from this repository. They skip
fork pull requests because they execute repository code in Docker and through
macOS accessibility APIs. A newer run cancels an older run for the same pull
request or branch, including manual runs.

The `core-macos` job also builds the release application package and checks its
size. The `e2e-macos` job reuses this package by default, and then does not set
up Rust. A manual dispatch has the `reuse_macos_package` input. Set it to
`false` to build the package in the E2E job. Native fixture archives download
from a mirror in each run. For the download sources, see
[Run the servers](testing.md#run-the-servers).

The workflow retains `core-coverage`, `macos-package-and-performance`,
`backend-evidence`, and `macos-evidence` for one day. Configure these individual
checks as required branch-protection checks:

```text
test / core-dependencies
test / core-scripts
test / core-backend
test / core-macos
test / e2e-backend
test / e2e-macos
```

There is no aggregate CI gate. The [release workflow](../.github/workflows/release.yml)
is manual. Use it to publish a nightly or stable release after the core and E2E
checks pass.

The release workflow accepts an optional commit SHA or ref. Leave the field
blank to use the latest commit on the branch selected for the workflow run. It
reads the application version from `Cargo.toml`. The stable tag is
`v<version>`. The nightly tag is
`v<version>-nightly.<UTC date>.<workflow run number>`.

The workflow runs `./qtest run fmt clippy rustdoc unit` on Linux,
`./qtest run clippy unit ui` on macOS, and the `backend` and `e2e` suites. The
E2E jobs run before packaging. The macOS
package job builds the application bundle, creates a DMG, and publishes it as
the GitHub Release asset. The workflow uses the latest non-draft release on the
selected channel as the changelog start tag. If the channel has no previous
release, it writes the target commit history as the changelog.

The jobs run in this order. As in the test workflow, `test-e2e-backend` runs at
the same time as `test-core-macos`, and `test-e2e-macos` waits for both:

```text
resolve-target -> test-core-backend -> test-core-macos  -> test-e2e-macos -> package -> publish
                                    -> test-e2e-backend -^
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
[Testing](testing.md#hooks-and-continuous-integration) for the suites of
each job. Local checks cannot verify GitHub event
filters, run cancellation, artifact transfer, or branch protection settings.

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

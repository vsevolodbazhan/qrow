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

The pre-commit hook checks the staged Git snapshot. The pre-push hook checks
the last commit of each pushed branch. Both select suites by changed path.
[Testing](testing.md#hooks-and-continuous-integration) gives the suites of
each hook and each CI job. Hooks export snapshots into temporary directories
and share a build cache under `target/hook-checks`. They do not stash,
restage, or modify working files. They do not package the application.

The release profile favors size so the optional assistant stays within the
macOS package budget.

Stage required code and configuration together. A passing working-copy check
does not prove that the staged snapshot passes. Changes under `scripts/`
trigger script checks, including changes to its README. Changes to the
Python and shell files in `tests/desktop/` also trigger script checks.
Changes under `vendor/` trigger the Rust suites, including changes to its
Markdown files, for example `QROW-PATCH.md`. Other Markdown changes, except
`docs/testing.md`, do not trigger hook checks.

The [test workflow](../.github/workflows/test.yml) checks pushes to `main`,
manual dispatches, and pull requests into any branch with the `opened`,
`reopened`, `synchronize`, `ready_for_review`, and `converted_to_draft`
actions. Pull requests in a stack get checks before their base merges. The
test workflow calls the [checks workflow](../.github/workflows/checks.yml),
which has the check jobs. On `main`, the test workflow then deletes old Rust
build caches. See [Testing](testing.md#hooks-and-continuous-integration). For a non-draft pull request, the `plan` job selects the
jobs from the changed paths and always selects `static`. Pushes and manual
runs start all jobs. [Testing](testing.md#hooks-and-continuous-integration)
gives the paths that select each job. A draft pull request, including a
`converted_to_draft` event, starts no jobs. To run the checks of a draft pull
request, mark it as ready for review.

Every job checks out the same commit: the pull request merge result, or the
pushed commit. A newer run cancels an older run for the same pull request or
branch, including manual runs.

Configure these individual checks as required checks of the `main` ruleset:

```text
checks / plan
checks / static
checks / core
checks / ui
checks / package
checks / backend
checks / e2e
checks / perf
```

Do not require the Intel checks. Pull requests skip them, and the run on
`main` after a merge reports their failures.

There is no aggregate CI gate. A skipped job counts as passed: a job that the
plan does not select, and a server job of a fork pull request. The `plan`
check must be required, because the other jobs skip when it fails. Do not
require the `checks` check. Only a skipped draft run reports it.

The [release workflow](../.github/workflows/release.yml) is manual. Use it to
publish a nightly or stable release. It accepts an optional commit SHA or ref.
Leave the field blank to use the latest commit on the branch selected for the
workflow run. It reads the application version from `Cargo.toml`. The stable
tag is `v<version>`. The nightly tag is
`v<version>-nightly.<UTC date>.<workflow run number>`.

The jobs run in this order:

```text
resolve-target -> checks -> dmg -> publish
```

`resolve-target` finds the commit and the release version. `checks` is the
checks workflow with all jobs. Its `package` job builds the package with the
release version, and its `e2e` job tests that package. `package-intel` and
`e2e-intel` do the same for Intel. `dmg` puts each tested package in a DMG
with `arm64` or `x86_64` in its name. It runs
[`scripts/package/dmg.py`](../scripts/package/dmg.py), which opens a Finder
window with the Qrow background, the app icon, and an Applications shortcut.
The publish job creates the release
tag, and then the GitHub Release with both DMGs as its assets. It also
keeps the unsuffixed DMG as an ARM64 alias for the existing tap updater. The
workflow uses the latest non-draft release on the selected channel as the
changelog start tag. If the channel has no previous release, it writes the
target commit history as the changelog. The first release on a channel can
run without a previous release or tag.

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

Local checks cannot verify GitHub event filters, run cancellation, artifact
transfer, or ruleset settings.

## Dependency maintenance

Keep lint rules, coverage floors, performance budgets, and dependency policy
intact when fixing a failure. Investigate the cause instead of weakening checks.

[Cargo.toml](../Cargo.toml) defines application lint rules.
[deny.toml](../deny.toml) defines dependency sources, licenses, bans, and advisory
exceptions. [dependency-reviews.toml](../dependency-reviews.toml) records the scoped
license review. These files are authoritative for exact versions and review dates.
The [policy checker](../scripts/core/policy.py) rejects expired or mismatched
exceptions and unpinned GitHub Actions. The dependency audit targets macOS ARM64 and x86_64.

GPUI brings some incompatible transitive versions. Duplicate versions produce
warnings. Advisory and license exceptions must remain specific and justified.
Packaging includes third-party license notices. Upgrade GPUI Kit and its
framework components as a compatible set. Preserve runtime Metal shaders unless
the build requirements deliberately change.

`vendor/` contains patched copies of some GPUI crates, and
[Cargo.toml](../Cargo.toml) uses them in place of the crates.io releases. The
`QROW-PATCH.md` file of each crate lists the changes. When you upgrade GPUI Kit,
apply the changes to the new release again, or remove a patch when the release
includes its fix.

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
Rust generator output. It also removes the server processor, because Qrow is
only a client. Thus Qrow uses the `thrift` crate without its default `server`
feature. Change the interface and script when needed. Do not make binding edits
that cannot be reproduced by generation.

## Probe an existing connection

The probe uses a saved profile and its real Keychain password. Run it only when
you intend to access that deployment:

```sh
cargo run --locked --bin qrow-probe -- "your profile name"
```

The [probe](../src/bin/qrow-probe.rs) initializes a session, runs `SELECT 1`,
checks the result, and closes the session. It does not verify engine sharing
or cancellation.

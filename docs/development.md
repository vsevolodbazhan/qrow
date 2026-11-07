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

The Linux dependency graph uses three unmaintained D-Bus crates:
`zbus_names`, `zvariant`, and `zvariant_derive`. GPUI and its Linux
dependencies still use the older D-Bus stack. The dependency policy has
dated exceptions for their maintenance advisories. Remove the exceptions
when that stack moves to zbus 6.

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

For each visual change, check these points in the demo:

- Check the dark theme and the light theme.
- Check a window that is 1280 points wide and a window that is 900 points
  wide, with the Assistant open and closed.
- For tab strips and tables, check five or more window widths. At each width,
  scroll to the start, the middle, and the end.
- Check hover, keyboard focus, selection, and validation errors.
- Use long non-Latin text, for example a Cyrillic tab title.

UI tests cannot read painted pixels. Check colors, fades, and borders on
screen, and report these checks. The
[UI conventions](../.agents/skills/qrow-ui-conventions/SKILL.md) list the
interface decisions of the project and the known UI bug classes.

Use release builds for performance measurements. `cargo build` does not update
`dist/Qrow.app`. Follow the [packaging instructions](../README.md#release) when
you need a new distributable. Do not replace the app while someone is testing it.

### Interface text

Use these rules for UI text:

- Headings, panel names, and field labels are nouns in title case:
  **Connection Settings**, **Initial Database**, **Sign-In**.
- Buttons, menu items, and action tooltips are verbs in sentence case:
  **Move to connection…**, **Copy qualified name**, **Show activity**. This rule
  also applies to the macOS menu bar.
- States, choices, placeholders, empty states, and messages use sentence
  case: **Not connected**, **While connected**, **No connections**.
- Dialog titles that are questions or conditions use sentence case:
  **Delete assistant conversation?**
- Product names and acronyms keep their case: **Qrow**, **Codex**, **SQL**.

### Fields

Text fields, text areas, selects, and comboboxes show their focus with a
tinted border only. Call `.focus_ring(false)` on each new field. The ring of
GPUI Kit is painted outside the border, so a list or a panel that clips its
content cuts the ring off.

## Hooks and continuous integration

The pre-commit hook checks the staged Git snapshot. The pre-push hook checks
the last commit of each pushed branch. Both select suites by changed path.
[Testing](testing.md#hooks-and-continuous-integration) gives the suites of
each hook and each CI job. Hooks export snapshots into temporary directories
and share a build cache under `target/hook-checks`. They do not stash,
restage, or modify working files. They do not package the application.

The release profile uses one code generation unit to keep the macOS package
within its size budget. The `perf` profile uses 16 units and no link-time
optimization to reduce build time. Debug and test builds optimize the
dependencies, like GPUI, but not Qrow. The UI tests run about 5 times faster
and the E2E tests 3 times faster. The first build after a dependency change
takes longer. Later builds compile only Qrow, as before.

Optimized dependencies also make the app react faster in tests. A test must
not depend on slow work: wait for the condition that it checks. For example,
wait for the saved workspace with `wait_saved`, and wait for an animated
element to stop before you click it.

Stage required code and configuration together. A passing working-copy check
does not prove that the staged snapshot passes. Changes under `scripts/`
trigger script checks, including changes to its README. Changes to the
Python and shell files in `tests/desktop/` also trigger script checks.
Changes under `vendor/` trigger the Rust suites, including changes to its
Markdown files, for example `QROW-PATCH.md`. Other Markdown changes, except
`docs/testing.md`, do not trigger hook checks.

The [test workflow](../.github/workflows/test.yml) runs for ready pull requests
into any branch, pushes to `main`, manual runs, and scheduled runs. Draft
pull requests start no validation. A newer pull request run cancels an older
run. Runs on `main` finish so they can save build caches.

The test and release workflows share one validation graph. Common tests run
once before native tests. Native compilation overlaps common validation.
Each native test consumes the build of the same target. All four targets,
including Intel macOS, have the same functional scope on pull requests and
`main`. Full mode adds timing stress and report-only measurements.
[Continuous integration](ci.md) describes every job and its evidence.

Configure only `ci-required` as the required check in the `main` ruleset.
After the new graph passes on its pull request, an administrator must remove
the old `checks / ...` contexts from the same ruleset before merge. Those jobs
no longer exist. After merge, inspect the first `main` run and its full-mode
stress evidence. [CI merge steps](ci.md#merge-and-administration) give the
complete procedure and current verification gaps.

Fork pull requests use disposable runners and synthetic credentials. They do
not get secrets. Linux package smoke uses X11 and software Vulkan. Windows,
Wayland, and hardware GPU qualification remain gaps; see [CI limitations](ci.md#known-gaps).

The [release workflow](../.github/workflows/release.yml) is manual. Use it to
publish a nightly or stable release. It accepts an optional commit SHA or ref. The resolved commit must match the
workflow run commit. Select the candidate branch before starting the workflow.
Leave the field blank to use that commit. It reads the application version from `Cargo.toml`. The stable
tag is `v<version>`. The nightly tag is
`v<version>-nightly.<UTC date>.<workflow run number>`.

The jobs run in this order:

```text
resolve-target -> validation (shared test workflow) -> dmg and linux -> publish
```

`resolve-target` finds the commit and release version. The validation
workflow builds each native package and tests its extracted application.
The macOS desktop suite uses that candidate archive. `dmg` puts each tested
package in a DMG
with `arm64` or `x86_64` in its name. It runs
[`scripts/package/dmg.py`](../scripts/package/dmg.py). When the DMG mounts,
Finder shows the app icon, an Applications shortcut, and an arrow between
them. The background is light because Finder draws the icon labels in black on
a background picture, also in the dark appearance. The background has a 1x and
a 2x version, so it stays sharp on Retina displays.
During packaging, the writable image uses a temporary mount that Finder does
not show. The script ejects the image before it creates the compressed DMG.
If macOS reports that the image is busy, the script makes at most 10 eject
attempts. After three attempts, it uses forced ejection. A persistent error
fails packaging. The `linux` jobs download the qualified Ubuntu 24.04 archives for x86-64 and
ARM64 and check their recorded SHA-256 digests. They do not rebuild the app.
The DMG jobs check the archive digest, wrap the qualified app, mount the final
image, compare its app files, and launch the mounted app.
The publish job creates the release tag and an unpublished release with both
DMGs and both Linux archives. It downloads the assets and checks SHA256SUMS
before publishing. A failed upload or digest check leaves an unpublished
release. A rerun with an existing release stops; inspect that release before
retrying. It also
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
exceptions and unpinned GitHub Actions. The dependency audit targets macOS and
Linux, on ARM64 and x86_64.

GPUI brings some incompatible transitive versions. Duplicate versions produce
warnings. Advisory and license exceptions must remain specific and justified.
Packaging includes third-party license notices. Upgrade GPUI Kit and its
framework components as a compatible set. Preserve runtime Metal shaders unless
the build requirements deliberately change.

`vendor/` contains patched copies of some GPUI crates, tokio-postgres and reqwest.

[Cargo.toml](../Cargo.toml) uses them in place of the crates.io releases. The
`QROW-PATCH.md` file of each crate lists the changes. The
[Postgres patch](../vendor/tokio-postgres/QROW-PATCH.md) adds bounded schema
metadata and an ordered buffer reset. The
[HTTP patch](../vendor/reqwest/QROW-PATCH.md) adds TCP ownership before TLS and
an HTTP/1 read-buffer limit. When you upgrade a patched dependency,
apply its changes to the new release again, or remove the patch when the release
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

## Bundled fonts

Qrow includes the Regular, Medium, SemiBold, Bold, and Italic files of IBM Plex
Sans, and the Regular, Bold, and Italic files of JetBrains Mono, in
[assets/fonts](../assets/fonts). Code does not use other weights of JetBrains
Mono, so they are not included. The files come from the upstream releases:
`@ibm/plex-sans` 1.1.0 and JetBrains Mono 2.304.
[Assets](../src/assets.rs) registers them at startup, so they show in the font
lists of Settings. Both fonts use the SIL Open Font License. Each font folder
has the license text, and [NOTICE](../NOTICE) has the copyright and license
texts. When you update a font, replace all its files from one release, and
update the version and the license text in NOTICE.

## Generated icon asset

The executable embeds [the small application icon](../assets/app-icons/qrow-256.png)
for the About dialog. The Linux archive installs it as the launcher icon. After a change of the
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

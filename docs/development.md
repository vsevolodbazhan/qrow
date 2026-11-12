# Development

Use local checks to verify code, scripts, and dependency policy before a commit.
[Testing](testing.md) describes `./qtest`, the command for all checks and
tests, including the suites with real servers and the desktop suite.

For build and demo launch commands, see the
[project README](../README.md#build). [Package the app](#package-the-app)
describes the release packages. Run the commands below from the repository
root. The full local suite requires macOS.

## Build prerequisites

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

Java, ODBC drivers, and the Thrift compiler are not necessary. Use the release
build to check startup time and responsiveness.

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
Keychain, or workspace access. The demo uses synthetic data. In either Rivendell
connection, right-click a table and select **Show dbt details**. Test the interactions affected by a UI change.
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
`dist/Qrow.app`. Follow [package the app](#package-the-app) when
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
into any branch, pushes to `main`, manual runs, and a weekly scheduled run. Draft pull
requests start no validation. A newer pull request run cancels an older run.
Runs on `main` finish so they can save build caches.

The test and release workflows share one validation graph. Common tests run
once, and native compilation overlaps them. Each native test consumes the
build of the same target. A pull request tests Linux x86-64 natively. A pull
request that changes only documentation or other static paths runs only the
static checks. `main`
and release candidates test all four targets, including Intel macOS, in full
mode with timing stress and report-only measurements. Check the `main` run
after each merge, and fix or revert a failed target.
[Continuous integration](ci.md) describes every level, job, and its evidence.

Configure only `ci-required` as the required check in the `main` ruleset.
[CI merge steps](ci.md#merge-and-administration) give the complete procedure
and current verification gaps.

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
resolve-target -> validation (shared test workflow) -> dmg, linux, and windows -> publish
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
The `windows` jobs do the same for the qualified Windows installers and
portable archives.
The publish job creates the release tag and an unpublished release with both
DMGs, both Linux archives, both Windows installers, and both Windows portable
archives. It signs `SHA256SUMS` with minisign and uploads
`SHA256SUMS.minisig`. The signed trusted comment is `Qrow release TAG`. The job
then downloads the assets, checks the signature with
[assets/release/minisign.pub](../assets/release/minisign.pub), and checks
SHA256SUMS before publishing. A failed upload, signature, or digest check
leaves an unpublished release. A rerun with an existing release stops; inspect
that release before retrying. It also keeps the unsuffixed DMG as an ARM64
alias for the existing tap updater. The workflow uses the latest non-draft
release on the selected channel as the changelog start tag. If the channel has
no previous release, it writes the target commit history as the changelog. The
first release on a channel can run without a previous release or tag.

The publish job reads the minisign secret key from the `RELEASE_SIGNING_KEY`
secret of the `release` environment. The key has no password. If the secret is
missing or does not match the committed public key, the job stops before it
creates the release. Keep a backup of the secret key outside GitHub. To change
the key, replace the public key file and the key in
[Installation](installation.md#verify-a-download) in the same change. To set up
the secret, create the `release` environment in the repository settings, then
run:

```sh
gh secret set RELEASE_SIGNING_KEY --env release --repo vsevolodbazhan/qrow < qrow-release.key
```

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

The custom [Scoop bucket](https://github.com/vsevolodbazhan/scoop-qrow)
publishes the latest stable release as `qrow` and the latest prerelease as
`qrow-nightly`. Both manifests install the Windows portable archives. A
scheduled job in the bucket checks the public GitHub releases every hour,
downloads each new archive, calculates its SHA-256 checksum, and updates the
manifest. The bucket does not need a secret in this repository. Scoop installs
each package in its own folder and does not use the installer, so stable and
nightly builds can be installed at the same time.

Cargo does not publish Qrow to crates.io. Cargo ignores the patches in `vendor/`
when it builds a published crate, so the crate does not compile.

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

`QROW_RELEASE_VERSION` sets the release version, for example
`0.5.0-nightly.20261112.42`. A build with this variable also gets an update
channel: `nightly` for a nightly version, otherwise `stable`. Only such builds
can check for updates. A build without the variable has no channel. The
[update module](../src/update.rs) reads releases, verifies the signed
checksums with [assets/release/minisign.pub](../assets/release/minisign.pub),
and installs the Linux archive. The application does not use it yet.

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
Also make the WOFF2 subsets of the [website](../website/README.md) again.

## Generated icon assets

The executable embeds [the small application icon](../assets/app-icons/qrow-256.png)
for the About dialog. The Linux archive installs it as the launcher icon. The
Windows executable also embeds [the Windows icon](../assets/app-icons/windows/qrow.ico)
as a resource, with sizes from 16 to 256 pixels. Explorer, the title bar, the
taskbar, and Alt+Tab show it. After a change of the
[source icon](../assets/app-icons/macos/qrow.png), make both assets again:

```sh
uv run --locked python scripts/generate/app_icon.py
```

Also make the social preview of the [website](../website/README.md#update-the-social-preview) again.

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

## Website

The landing page is in [website](../website). It is static HTML with no build
step. A push to `main` that changes it deploys it to <https://qrow.dev>. See
the [website README](../website/README.md) to preview the page, to update its
screenshot, and for the deployment.

## Package the app

The packaging scripts build release packages for the current computer. CI
uses the same scripts for release packages. See [continuous integration](ci.md)
for package qualification and release publication.

### macOS

Build the application bundle and the installer image, then launch the app:

```sh
sh scripts/package/macos.sh
open dist/Qrow.app
```

The script builds for the architecture of the current Mac. It writes
`dist/Qrow.app`, `dist/Qrow-macos.zip`, and `dist/Qrow-VERSION-ARCH.dmg`.
`cargo build` does not update the packaged app. After a source change, quit
Qrow, run the script again, and open the app again.

The minimum macOS version is 12.0. [`.cargo/config.toml`](../.cargo/config.toml)
sets the build baseline, and packaging uses it for the app bundle. A higher
`MACOSX_DEPLOYMENT_TARGET` raises both values. A lower value stops packaging.
Do not use `target-cpu=native` for a package that you share with older Macs.
The Intel build needs SSE4.1. It does not need AVX.

To build for the other Mac architecture, install its Rust target and set
`CARGO_BUILD_TARGET` to `x86_64-apple-darwin` or `aarch64-apple-darwin`.
Packaging reads the executable from the target subdirectory and checks its
architecture. The package includes the license notices of the selected
target. CI tests both builds on macOS 15. It does not test every macOS 12
driver.

GitHub releases contain `Qrow-VERSION-arm64.dmg` and
`Qrow-VERSION-x86_64.dmg`. The file `Qrow-VERSION.dmg` without an architecture
suffix is a copy of the ARM64 image. The Homebrew casks use it, so they
install only the ARM64 app. Keep this file until the tap updater supports
both architectures.

GPUI compiles Metal shaders at run time, so the Xcode Metal compiler is not
necessary.

### Linux

Build a native archive in `target/package`:

```sh
sh scripts/package/linux.sh
```

The archive uses the libraries of the build system. Build it on a system that
is compatible with the target computers. Release archives are built on
Ubuntu 24.04.

### Windows

Build the unsigned installer and the portable archive on Windows:

```sh
uv run --locked --no-dev python scripts/package/windows.py
```

GPUI compiles release shaders with the Windows SDK, so Linux cross checks
cannot produce a working package. The installer is defined in
[scripts/package/qrow.iss](../scripts/package/qrow.iss). The portable zip
archive holds the same files for Scoop and manual use. It has no top folder. The packaging
script downloads the pinned Inno Setup compiler, checks its SHA-256 digest,
and installs it in portable mode in `target/tools`. Set `QROW_ISCC` to the
path of `ISCC.exe` to use another compiler. Do not change the `AppId` of the
installer: upgrades and the uninstaller find an earlier install by it.

Package qualification installs Qrow for the current user, so it stops when
Qrow is already installed on the computer. Run it under a Windows account
without Qrow.

Package notices use UTF-8 for Cargo metadata and license files on all platforms.
They do not depend on the Windows system code page.

## Windows cross checks

For local type checks on Linux, install `cargo-xwin` and the
`x86_64-pc-windows-msvc` Rust target. Run `cargo xwin check --target
x86_64-pc-windows-msvc --all-targets`. Run Clippy with the
same arguments. Use two Cargo jobs on hosts with limited memory.

For ARM64 cross checks, select `--target aarch64-pc-windows-msvc` and set
`XWIN_CROSS_COMPILER=clang`. The clang backend supplies SDK include flags
that the ARM64 `ring` build accepts.

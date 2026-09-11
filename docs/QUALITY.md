# Quality checks

Run from the repository root on macOS. Rust 1.97.1, rustfmt, and Clippy are pinned
in `rust-toolchain.toml`. Cargo commands use the lockfile.

## Setup

```sh
brew install shellcheck actionlint
sh scripts/install-check-tools.sh
sh scripts/install-hooks.sh
```

The tool installer pins cargo-deny 0.20.2, cargo-machete 0.9.2, and
cargo-llvm-cov 0.9.1. It also installs LLVM's coverage tools for the pinned Rust
version. Python 3.11 or later is required for policy checks and packaging.

## Local commands

```sh
sh scripts/check.sh             # Full suite, including coverage and dependency audit
sh scripts/check.sh hook        # Fast checks used before each commit
sh scripts/check.sh core        # No GPUI dependencies; lint, tests, and rustdoc
sh scripts/check.sh native      # Full application lint and tests
sh scripts/check.sh scripts     # ShellCheck, actionlint, hook and policy tests
sh scripts/check.sh dependencies
sh scripts/check.sh coverage
sh scripts/check.sh performance
```

Checks never run real Kyuubi queries, retrieve credentials, or open the UI. The
existing Keychain integration test stays opt-in. Generated Thrift bindings are
excluded from Clippy's normal style checks and from the coverage report. Unsafe
code is forbidden throughout Qrow, including generated bindings. This does not
forbid unsafe code inside third-party dependencies.

The `ui` Cargo feature enables the application and its UI dependencies. Core
checks use `--no-default-features`, which also verifies that library modules do
not depend on GPUI. Default builds still include the application.

## Git hooks

The installer sets repository-local `core.hooksPath` to `.githooks`. It refuses
to replace a different configured hook directory. Each clone needs installation.

- Pre-commit checks the staged tree: formatting, core Clippy, core tests, and
  dependency waiver dates and CI action pins.
- Pre-push checks each distinct revision being pushed with the full local suite.
  Deleting a remote branch does not run checks.

Hooks export Git snapshots into temporary directories. They neither stash nor
modify working files, stage formatting changes, or replace `dist/Qrow.app`.
Dependencies share `target/hook-checks` between snapshots. The first run takes
longer while this cache builds. Hook tests cover partially staged files, failure
propagation, preservation of the index and worktree, and pushed revisions.

Stage the new hook scripts and check configuration with this initial setup.
Subsequent commits use the configuration in their own staged snapshot. Hooks are
local safeguards; CI remains necessary because Git allows hooks to be bypassed.

## What fails a check

- Formatting differences, compiler/Clippy warnings, broken rustdoc links, test
  failures, unused direct dependencies, or script/workflow lint failures.
- Unsafe code in Qrow, discarded must-use values, debug macros, unfinished
  `todo!`/`unimplemented!` calls, and the additional Clippy rules in Cargo.toml.
- RustSec findings other than individually documented maintenance exceptions,
  unapproved licenses, unknown registries/Git dependencies, wildcard version
  requirements, or reintroduction of egui, a webview, or JNI.
- An expired advisory review date or a GitHub Action reference without a full
  commit SHA. Duplicate transitive versions are reported but do not fail. GPUI
  currently requires several incompatible versions in the same graph.
- Core line coverage below 80%. The measured local baseline is 84.2%. This
  measures the core and excludes generated bindings, integration test files,
  binaries, and the GPUI frontend. It is not a UI coverage claim.

The property tests run 512 cases each, checking arbitrary Unicode token ranges,
quoted semicolons, and nested comments. Commit any generated
Proptest regression seed files after fixing a discovered counterexample.
The worker tests verify the 100,000-row and 64 MiB preview limits after repeated
fetches, along with concurrent tabs, cancellation, and session reuse.

## Performance

`cargo bench --no-default-features --bench sql` warms the validator, takes 21
samples per input size, and reports the median. Budgets are 5 ms at 10 KB, 25 ms
at 100 KB, and 250 ms at 1 MB. Local measurements during setup were approximately
0.25 ms, 2.5 ms, and 27 ms. These generous limits catch large regressions on CI;
compare reports when assessing smaller changes. They measure SQL validation,
not rendering or query execution on Spark.

After an optimized package build:

```sh
sh scripts/package-macos.sh
python3 scripts/check-size.py
```

Budgets are 24 MiB for the executable and 10 MiB for the zipped bundle, against
an initial baseline of approximately 16 MiB and 5.7 MiB. Investigate growth before
raising a budget. Package building is a separate CI step and is not part of the
Git hooks, so committing cannot replace the app being tested manually.

Startup, frame latency, scrolling, editor selection, macOS focus behavior,
Keychain prompts, and workspace restoration still need native UI verification.
A passing benchmark cannot establish those properties. Use the release demo and
an isolated `QROW_DATA_DIR` for UI checks.

## Dependency maintenance exceptions

The initial audit found no vulnerability advisories for the macOS dependency
graph, but these four transitive maintenance advisories have no compatible
fixed release in the selected GPUI stack. Each exception has a reason in
`deny.toml` and expires on 2026-12-11. New advisory IDs still fail. Reassess the
upstream migration and supported alternatives before extending a review date.

| Dependency | Advisory | Dependency path |
| --- | --- | --- |
| instant | [RUSTSEC-2024-0384](https://rustsec.org/advisories/RUSTSEC-2024-0384.html) | GPUI Component / notify |
| paste | [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) | GPUI Component / Metal |
| rustybuzz | [RUSTSEC-2026-0206](https://rustsec.org/advisories/RUSTSEC-2026-0206.html) | GPUI / SVG rendering |
| ttf-parser | [RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192.html) | GPUI / SVG fonts |

[cargo-deny's policy documentation](https://embarkstudios.github.io/cargo-deny/checks/)
explains the dependency checks. The audit targets the macOS ARM64 graph, which
is the supported distributable.

## CI

`.github/workflows/quality.yml` runs on pushes, pull requests, manual dispatch,
and weekly schedules. Linux runs headless core checks, coverage, and script
checks. A macOS ARM64 runner builds the full app, runs tests and benchmarks,
packages it, verifies signing, and enforces size limits. Dependency checks also
run weekly so new advisories are detected without a code change.

CI uploads the core LCOV report and the macOS package/performance report for 14
days. Actions are pinned to immutable commits, permissions are read-only, and
checkout credentials are not persisted. Dependabot proposes Cargo and Actions
updates weekly, grouping the GPUI packages together.

No remote is configured yet, so the hosted workflow has not run. After pushing
to GitHub, make the three Quality jobs required checks in branch protection.
Local checks cannot enforce remote branch protection.

GPUI Kit 0.6.1 brings `libbz2-rs-sys` 0.2.5 through its HTTP compression stack.
Its `bzip2-1.0.6` license has an exception for that exact version. The package
includes the supplied license text. The reason and 2026-12-11 review deadline
are in `docs/dependency-reviews.toml`; the policy check rejects missing, expired,
or mismatched license reviews. This does not allow the license for other crates.

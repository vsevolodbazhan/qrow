# Testing

Use `./qtest` to run all checks and tests of Qrow: formatting, lint, unit
tests, UI tests, coverage, performance, dependency policy, scripts, and the
end-to-end suites. Humans, hooks, CI, and coding agents use the same command.

Run from the repository root:

```sh
./qtest                  # fast local checks (the default group)
./qtest run ui           # one suite
./qtest run ui/queries   # the tests of a suite whose names contain "queries"
./qtest run --changed    # the suites that your changed files affect
./qtest list             # suites, groups, and what each one needs
./qtest doctor           # missing tools, with the fix for each one
./qtest help TOPIC       # one section of this page
```

`./qtest help TOPIC` prints a section of this page. The topic is the section
heading in lowercase, with hyphens, for example `write-a-ui-test`.

The [catalog](../scripts/qtest/catalog.py) defines the suites, groups, pinned
tools, and change rules. Rust tests run through
[cargo-nextest](https://nexte.st/) with the [nextest profiles](../.config/nextest.toml).

## Suites

A suite is the smallest unit that you can select. Suites marked with `*`
accept a test filter.

| Suite | Checks | Needs |
| --- | --- | --- |
| `fmt` | Rust formatting. | Rust |
| `clippy` | Clippy for the core library and, on macOS, the application. | Rust |
| `rustdoc` | Rust API documentation without warnings. | Rust |
| `unit` * | Rust unit and integration tests that need no UI and no servers. | Rust, cargo-nextest |
| `ui` * | Headless tests of the real Qrow window, without servers. | macOS, cargo-nextest |
| `coverage` | Core line coverage with an 80% floor. | cargo-llvm-cov |
| `perf` | SQL validation benchmark with enforced budgets. | Rust |
| `scripts` | ShellCheck, actionlint, Ruff, and automation unit tests. | uv, ShellCheck, actionlint |
| `policy` | Dependency waiver dates and pinned CI actions. | uv |
| `deps` | Dependency policy, unused dependencies, advisories, licenses, and sources. | cargo-machete, cargo-deny |
| `backend` | Connector and worker against disposable LDAP, Kyuubi, and Spark in Docker. | Docker |
| `e2e` | The packaged app, driven through macOS accessibility, against real servers. | macOS desktop, Java 17 or Docker |

On Linux, `unit` and `clippy` use only the core library, and `ui` and `e2e`
are not available.

The `backend` and `e2e` suites start disposable servers, and `e2e` takes over
the desktop. They run only when you select them by name. See
[End-to-end testing](end-to-end-testing.md) for their servers, evidence, and
limits. Use `./qtest run e2e --runtime docker` to run the servers in Docker
instead of local Java processes.

## Groups

A group selects several suites.

| Group | Suites | Use |
| --- | --- | --- |
| `default` | `fmt`, `clippy`, `unit`, `ui` | Runs when you give no selector. |
| `all` | `scripts`, `deps`, `fmt`, `clippy`, `rustdoc`, `unit`, `ui`, `perf`, `coverage` | Every local suite without servers. The pre-push hook runs it. |

A group skips a suite that the platform does not support, for example `ui`
on Linux. A suite that you select by name does not skip. It fails with a
missing prerequisite.

## Select tests

`./qtest run` accepts one or more selectors:

- A suite name, for example `unit`.
- A group name, for example `all`.
- `SUITE/FILTER`, for the tests of a suite whose names contain `FILTER`. Use
  letters, digits, `_`, and `::`, for example `ui/connections::new_connection`.

Options:

| Option | Effect |
| --- | --- |
| `--changed` | Adds the suites that your changed files affect. |
| `--repeat N` | Runs each selected suite `N` times and stops at the first failure. Use it to find unstable tests. |
| `--fail-fast` | Skips the remaining suites after a failure. |
| `--runtime native\|docker` | Selects the server runtime of the `e2e` suite. |
| `--json` | Prints only a JSON summary on stdout. |
| `--quiet` | Does not show command output. The logs keep it. |
| `-- ARGS` | Gives `ARGS` to cargo-nextest, for example `-- --no-capture`. |

`--changed` reads `QROW_CHANGED_FILES` when it is set. Otherwise it uses your
uncommitted and untracked files and the commits since `origin/main`. Changes
to Rust sources, tests, and Cargo files select `fmt`, `clippy`, `unit`, and
`ui`. Changes to dependency policy files or workflows select `policy`. Changes
to scripts, hooks, workflows, `qtest`, or this page select `scripts`.

To list the tests of a suite, run `./qtest list ui --tests`.

## Read results

Each run writes its artifacts to `target/qtest/runs/<run>/`.
`target/qtest/runs/latest` points to the most recent run. `./qtest artifacts`
prints its directory.

| Path | Content |
| --- | --- |
| `summary.json` | The result of each suite, with the JSON schema of `--json`. |
| `logs/<suite>.log` | The commands and the full output of the suite. |
| `junit/<suite>.xml` | The nextest JUnit report of a failed suite. |

The summary gives the status of each suite: `passed`, `failed`, `skipped`, or
`missing`. A failed suite names its failed step. A failed nextest suite also
lists each failed test and its panic message. A missing suite lists each
missing prerequisite and its fix.

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | All selected suites passed or were skipped. |
| 1 | A suite failed. |
| 2 | Usage error, for example an unknown suite. |
| 3 | A prerequisite is missing, and no suite failed. |

qtest does not run a failed test again. A test must pass on its first run.
Use `--repeat` to find unstable tests. Do not add retries.

## Check prerequisites

`./qtest doctor` checks what each suite needs and prints the fix for each
gap. Give suite names to check only those suites. It exits with code 3 when
a prerequisite is missing.

`./qtest install` installs the pinned Cargo tools: cargo-nextest,
cargo-deny, cargo-machete, and cargo-llvm-cov with its LLVM component. Give
tool names to install only those tools. The [catalog](../scripts/qtest/catalog.py)
holds the pinned versions.

Install the script linters separately:

```sh
brew install shellcheck actionlint
```

When Docker is installed but its daemon is not running, `backend` and
`e2e --runtime docker` stop with code 3 and ask you to start Docker.

## Write a UI test

UI tests open the real Qrow window in a headless GPUI Kit window. They send
clicks and keys through GPUI and check the visible result and the saved
workspace. They do not need a server, Keychain, or a desktop session.

Add a test from the template:

```sh
./qtest new ui connections duplicate_name_keeps_the_form_open
./qtest run ui/duplicate_name_keeps_the_form_open
```

The template fails until you write the test. The
[test support](../tests/ui/support.rs) gives each test these parts:

- `TestApp::launch` opens Qrow with a new temporary workspace and in-memory
  passwords. `TestApp::launch_with` also takes synthetic passwords.
- `app.update` gives access to the window, for example
  `window.click("run", cx)` or `window.find("profile-…").label()`.
- `app.fill` types into an input and checks that the input took the text.
- `app.wait_until` waits for a condition on wall time. On timeout it shows
  the element labels and the saved workspace.
- `app.saved()` reads the saved workspace, and `app.credentials` holds the
  synthetic passwords.

Follow these rules:

- Find controls by element ID. Do not find them by the text that they show.
- Do not sleep. Wait for a condition with `wait_until`.
- After each action, make sure that it had an effect before the next action.
- Check the result that a user can see, and the saved state when it applies.
- Include a negative case when it applies, for example a rejected input.

Real worker threads do real network I/O in these tests. The support code
allows wake-ups from other threads, so waits use wall time. Reduced motion
opens dialogs without animation.

## Element IDs

GPUI Kit components register their element ID for tests, for example
`Button::new("save-profile")`. Add an ID to a control when a test needs it:

- Use lowercase words with hyphens, for example `connection-host`.
- Make repeated IDs from a domain value, for example `profile-<uuid>`. Do not
  make an ID from a list position.
- Keep an ID stable when its label changes.

## Hooks and continuous integration

Install the repository hooks with `sh scripts/hooks/install.sh`. The hooks
check a snapshot of the commit. They do not change your files or the index.

| Hook | Runs |
| --- | --- |
| pre-commit | `./qtest run --changed --fail-fast` on the staged files. |
| pre-push | `./qtest run all --fail-fast` on each pushed commit. |

Hook snapshots share a build directory, `target/hook-checks`, in the checkout
that runs the hook.

The [test workflow](../.github/workflows/test.yml) runs the same suites:

| Job | Runs |
| --- | --- |
| `core-dependencies` | `deps` |
| `core-scripts` | `scripts` |
| `core-backend` | `fmt`, `clippy`, `rustdoc`, `unit`, `coverage` on Linux |
| `core-macos` | `clippy`, `unit`, `ui`, `perf` on macOS, then packaging |
| `e2e-backend` | `backend` |
| `e2e-macos` | `e2e` |

In CI, qtest uses the `ci` nextest profile. See
[Development](development.md#hooks-and-continuous-integration) for workflow
events and release checks.

## Limitations

- UI tests send input through GPUI. They do not verify macOS input, the menu
  bar, the real Keychain, input methods, or rendered pixels. The `e2e` suite
  covers these parts of the packaged app.
- `--repeat` runs complete suites again. It does not run one test in a loop.
- `./qtest new` supports only UI tests.

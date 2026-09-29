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
./qtest fixture up       # keep the test servers running between runs
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
| `backend` * | Connector and worker against the real servers, without the UI. | Docker |
| `e2e` * | The real Qrow window, headless, against the real servers. | macOS, Docker or Java 17 |
| `desktop` | Smoke checks of the packaged app on the desktop: the menu bar, Keychain, quit, and pixels. | macOS desktop, Docker or Java 17 |

On Linux, `unit` and `clippy` use only the core library, and `ui`, `e2e`, and
`desktop` are not available.

The `backend`, `e2e`, and `desktop` suites use disposable LDAP, Kyuubi, and
Spark servers. `desktop` also takes over the desktop. These suites run only
when you select them by name. See [Run the servers](#run-the-servers), and see
[End-to-end testing](end-to-end-testing.md) for the servers, evidence, and
limits.

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
| `--runtime auto\|docker\|native` | Selects the server runtime. `auto`, the default, uses Docker when its daemon answers, and local Java processes otherwise. |
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
`--runtime docker` stop with code 3 and ask you to start Docker.

## Run the servers

The `backend`, `e2e`, and `desktop` suites share one set of disposable
servers in a run. qtest builds the test programs and the app package first,
then starts the servers. The servers are ready when the synthetic user `qrow`
gets an answer to `SELECT 1`. This answer also starts the Spark engine of the
user, so the first test does not wait for an engine start.

To keep the servers running between runs, start them yourself:

```sh
./qtest fixture up       # start the servers and keep them running
./qtest run e2e          # uses the running servers
./qtest fixture status   # show the runtime, port, and health
./qtest fixture down     # stop and remove the servers
```

A run uses the servers of `fixture up` when they answer SQL. Otherwise it
starts new servers and removes them at the end, also after a failure. Each run
copies the server logs and evidence to `fixture/` in its artifact directory.

Two runtimes are available:

- `docker` starts the [Compose project](../tests/fixture/compose.yml) with a
  random loopback port.
- `native` starts local Java processes from [verified
  downloads](../tests/fixture/native-downloads.json). It needs a Java 17 JDK
  in `JAVA_HOME`. Hosted macOS CI runners use it because they have no Docker.

The `backend` suite stops Spark engines and restarts Kyuubi, so it needs the
Docker runtime. Its tests run one at a time.

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
[test support](../tests/support/mod.rs) gives each test these parts:

- `TestApp::launch` opens Qrow with a new temporary workspace and in-memory
  passwords. `TestApp::launch_with` also takes synthetic passwords.
- `app.update` gives access to the window, for example
  `window.click("run", cx)` or `window.find("profile-…").label()`.
- `app.fill` types into an input and checks that the input took the text.
- `app.wait_until` waits for a condition on wall time. On timeout it shows
  the element labels and the saved workspace.
- `app.saved()` reads the saved workspace, and `app.credentials` holds the
  synthetic passwords.

More helpers operate controls that GPUI Kit owns:

- `app.context_menu(id)` and `app.context_menu_labelled(label)` open a
  context menu. `app.choose("popup-menu", item)` and
  `app.choose_in_submenu(parent, item)` click a menu item by its label.
- `app.select(id, option)` chooses an option of a select.
  `app.stepper(id)` and `app.step(id, "increment", expected)` read and change
  a number field of Settings.
- `app.click_labelled(label)` clicks an element that has no ID, like the
  search field of Settings. Use it only for elements that GPUI Kit names.
- `app.dispatch(action)` sends an action, like `OpenSettings`, as its menu
  item does. The menu bar itself is part of the `desktop` suite.

Assistant tests use the synthetic Codex server in
[`tests/desktop/fake-codex.py`](../tests/desktop/fake-codex.py).
`FakeCodex::new()` gives a workspace directory and a Codex executable for
it. `codex.workspace(...)` turns on the assistant with this executable.
Some messages make the server wait for a marker file, which
`codex.mark(name)` creates. The
[assistant support](../tests/support/assistant.rs) sends messages and reads
the transcript, the editor, and the approval card.

Follow these rules:

- Find controls by element ID. Do not find them by the text that they show.
- Do not sleep. Wait for a condition with `wait_until`.
- After each action, make sure that it had an effect before the next action.
- Check the result that a user can see, and the saved state when it applies.
- Include a negative case when it applies, for example a rejected input.

Real worker threads do real network I/O in these tests. The support code
allows wake-ups from other threads, so waits use wall time. Reduced motion
opens dialogs without animation.

## Write an E2E test

E2E tests use the same [test support](../tests/support/mod.rs) as UI tests and
connect to the real servers. Put them in [`tests/e2e/`](../tests/e2e/), and
mark each test so that `cargo test` without servers does not run it:

```rust
#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn query_rows_reach_the_results_table(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace("SELECT 1 AS value", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(cx, "the result", QUERY_TIMEOUT, |window, _| {
        cell(window, 0, 1).as_deref() == Some("1")
    });
}
```

- `Kyuubi::get()` reads the servers from the environment that qtest sets. It
  fails when the servers are not there. It does not skip the test.
- `cell(window, row, column)` and `header(window, column)` read the result
  table. Column 0 holds the row number.
- `app.type_sql` replaces the SQL of the active tab through the editor.
  `app.run_sql` also runs it. `app.wait_status("Complete")` waits for the
  status bar, and `app.wait_cell(row, column, text)` waits for a result.
- `app.select_connection(profile)` selects a connection, and `app.logs()`
  reads Logs through **Copy All Logs**.
- `blocking(token, milliseconds)` makes a query that holds an executor
  task. The fixture records the task, and `evidence(token, "started")` or
  `app.wait_evidence(...)` reads that record. Register the function first
  with `REGISTER_BLOCKING`.
- `app.scroll_to(id)` scrolls a form to a field below its fold.
- Tests run at the same time and share the Spark engine of `qrow`. Do not
  change shared state, like global tables. The fixture has two executor
  cores, so put a test that holds executors in the `blocking` module, where
  tests run one at a time. A test that stops or restarts a server belongs in
  the `backend` suite.

Add a test from the template with `./qtest new e2e SUITE NAME`. Run one test
with `./qtest run e2e/query_rows`.

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
| `e2e-backend` | `backend` with Docker |
| `e2e-macos` | `e2e` and `desktop` with local Java servers |

In CI, qtest uses the `ci` nextest profile. See
[Development](development.md#hooks-and-continuous-integration) for workflow
events and release checks.

## Limitations

- UI and E2E tests send input through GPUI. They do not verify macOS input,
  the menu bar, the real Keychain, input methods, or rendered pixels. The
  `desktop` suite covers these parts of the packaged app.
- GPUI's leak detector fails a UI test that ends with a leaked entity. GPUI
  Kit's `context_menu` keeps each dismissed menu alive, so Qrow opens its
  context menus itself.
- An assistant test waits about 5 seconds while the synthetic Codex server
  starts.
- The servers have room for one Spark engine. Tests connect only as `qrow`.
- `--repeat` runs complete suites again. It does not run one test in a loop.

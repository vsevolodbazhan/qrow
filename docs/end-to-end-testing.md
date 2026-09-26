# End-to-end testing

The end-to-end suite tests Qrow against real Kyuubi and Spark servers. It checks
whether connection, query, result, and session behavior work together. The native
suite also tests the application through real keyboard and pointer events.

These tests complement [local checks](development.md). Protocol fixtures can
verify client messages, but cannot establish how a real server responds.
Native compilation cannot establish that the application controls work.

The reference stack uses Kyuubi 1.12.0, Spark 3.5.3 in standalone mode, LDAP with
synthetic users, and ZooKeeper. A successful run establishes behavior for that
configuration. It does not establish compatibility with every deployment.

## Run backend tests

Use the [application prerequisites](../README.md#prerequisites). Install Docker
with Compose v2 and start its daemon. Allow approximately 8 GB for the server
stack (although it's been confirmed to run on M1 8 GB Mac). This is test infrastructure memory, not Qrow's memory requirement.

Run from the repository root:

```sh
sh scripts/check.sh e2e/backend
```

The command starts a disposable server stack, waits for an authenticated SQL
response, runs the backend tests, collects artifacts, and removes the stack.
Initial image downloads and builds are substantially larger than the application.

Do not point the suite at an existing deployment. Failure tests stop fixture
engines and restart servers. The suite must control disposable resources.

## Run native UI tests

Use an unlocked, logged-in macOS desktop session. The native driver takes focus and sends real input events. Save work in other applications before the run.

Check automation access:

```sh
sh scripts/e2e/driver.sh --preflight
```

The driver at `target/e2e-tools/native-driver`, or its invoking terminal as
required by macOS, needs Accessibility and Screen Recording permissions.
Grant access through System Settings if preflight reports missing permissions.
Preflight does not change privacy settings or substitute a headless test.

The default native suite uses Java 17 for the server fixture. Set `JAVA_HOME`
to a Java 17 JDK, then run:

```sh
sh scripts/check.sh e2e/macos
```

To use Docker servers for the same native UI suite:

```sh
sh scripts/check.sh e2e/macos --runtime docker
```

The Docker mode requires a local daemon and does not require a host JDK.
Java is a server-fixture dependency. Qrow itself remains a native Rust application.

To check the editor's active line without a server, build an isolated package and
run the native driver in demo mode:

```sh
qrow_editor_test_dir=$(mktemp -d)
mkdir -p "$qrow_editor_test_dir/workspace"
QROW_DIST_DIR="$qrow_editor_test_dir/package" QROW_BUILD_PROFILE=debug sh scripts/package/macos.sh
sh scripts/e2e/driver.sh --preflight
QROW_E2E_ARTIFACTS="$qrow_editor_test_dir" \
QROW_DATA_DIR="$qrow_editor_test_dir/workspace" \
QROW_E2E_BUNDLE="$qrow_editor_test_dir/package/Qrow.app" \
target/e2e-tools/native-driver --editor-highlight-only
```

The driver checks the active line color at the editor's right edge. It saves
`editor-highlight.png` in the temporary directory.

The suite creates a release package inside the run directory. It does not
replace `dist/Qrow.app`. It uses a temporary workspace and fresh synthetic
Keychain credentials. The driver restores the previous clipboard contents
after text entry.

To check the Assistant font picker without a server fixture, use an isolated
debug package and the synthetic Codex server:

```sh
qrow_font_test_dir=$(mktemp -d)
mkdir -p "$qrow_font_test_dir/workspace"
QROW_DIST_DIR="$qrow_font_test_dir/package" QROW_BUILD_PROFILE=debug sh scripts/package/macos.sh
sh scripts/e2e/driver.sh --preflight
QROW_E2E_ARTIFACTS="$qrow_font_test_dir" \
QROW_DATA_DIR="$qrow_font_test_dir/workspace" \
QROW_E2E_BUNDLE="$qrow_font_test_dir/package/Qrow.app" \
target/e2e-tools/native-driver --assistant-font-only
```

This check captures the conversation search row, selects System Font and Menlo,
reads the saved settings, captures the transcript in each font, and deletes a
conversation whose Codex history is missing. It then starts Qrow again with the
same workspace and checks that the unsent conversation does not cause an error.
Like Codex, the synthetic Codex server saves a conversation only after its first
message.

To check the layout of assistant messages, use the same package steps with a
new, empty workspace directory and run:

```sh
target/e2e-tools/native-driver --assistant-layout-only
```

The driver writes a synthetic workspace with a UI scale of 1.1 and an assistant
pane width of 536. At this width, messages can be measured at one width and
drawn at another. The check makes sure that:

- A message with inline code that fits on one line shows its last word.
- The assistant reply starts at the left edge of the composer, and the user
  message bubble ends at its right edge.
- A reply with a wide table uses the full transcript width.
- The transcript scrolls to the end of the table.
- The mouse wheel scrolls the transcript over the table.

The table checks run with the Connections sidebar shown and hidden.

To check that the assistant keeps earlier SQL, use the package steps in the
assistant font check above. Use a new, empty workspace directory. Replace the
last driver command with:

```sh
target/e2e-tools/native-driver --assistant-append-only
```

The check asks for two queries. It verifies that the second query follows the
first query in the same tab. It also checks that the conversation gets a
generated title after the first reply, that Qrow saves the title, and that the
second reply does not change it.

To check how the assistant runs one statement from a tab with several queries,
use the package steps in the assistant font check above. Use a new, empty
workspace directory. Replace the last driver command with:

```sh
target/e2e-tools/native-driver --assistant-statement-only
```

The driver uses a synthetic connection and cancels each query before it
connects. It checks the exact SQL in each approval card. One card shows the
latest appended query. The other shows an earlier query that the assistant
selected by its byte range.

To check the conversation actions, use the package steps in the assistant font
check above. Use a new, empty workspace directory. Replace the last command
with:

```sh
target/e2e-tools/native-driver --assistant-titles-only
```

The driver submits an empty name in the rename dialog and checks that the
dialog stays open. It renames a conversation and makes a new title from the
pane header. It starts a second conversation with the same generated title and
checks that the thread list does not show thread IDs. Then it uses the thread
list context menu to cancel a rename, rename a conversation, make a new title,
and delete a conversation. The driver reads the saved workspace to check each
title and title source.

To check a tab rename and selection change during an assistant turn, use the
same package steps and a new, empty workspace directory. Run:

```sh
target/e2e-tools/native-driver --assistant-retarget-only
```

The driver renames another tab and selects it during a turn. The assistant
reads SQL and requests a run with a tab ID that is not in the open tabs. The
check confirms that the query approval card names the selected tab and shows
its SQL. The driver cancels the request before it connects.

## Inspect failures

Each run prints an artifact path under:

```text
target/e2e/qrow-e2e-<run-id>/
```

Artifacts include test output, server logs, and execution evidence. Native UI
runs also save application screenshots and accessibility snapshots. Driver
preflight output is in `target/e2e-tools/preflight.log`.

Start with the failed command or assertion. Use server logs to inspect connection
and execution failures. Use screenshots and accessibility snapshots to inspect
UI failures. Failed runs keep their artifacts after fixture cleanup.

The E2E orchestrator prints each fixture phase. Native archive downloads report
received bytes, total bytes, percentage, transfer rate, and estimated time left
every 15 seconds. Long package, backend, and native-driver commands print a
keep-alive every 30 seconds and stream their output to the run log. The saved
command logs contain the same command output.

A missing fixture, failed assertion, deadline, or cleanup failure fails the run.
The suite does not automatically rerun failed tests. Readiness checks can retry
`SELECT 1` while waiting for servers to become available.

An operating system crash or SIGKILL can interrupt Keychain cleanup. If that
happens, set `QROW_E2E_ARTIFACTS` to the affected run directory and run:

```sh
uv run --locked python scripts/e2e/keychain.py
```

The cleanup script validates the run directory and synthetic profiles before
removing credentials. It needs the run's saved workspace to identify those
profiles. `QROW_DATA_DIR` alone does not isolate Keychain.

## How the suite works

The [orchestrator](../scripts/e2e/run.py) creates independent Docker projects,
networks, ports, and evidence volumes. Ports bind to loopback. Backend tests
exercise the real [worker](../tests/live_kyuubi.rs) against this stack.

For native runs, the [server manager](../scripts/e2e/servers.py) starts Java
processes on temporary loopback ports. It packages the app before starting the
servers to reduce peak resource use. It terminates server process groups,
including Spark engines and executors, after the run.

[Docker fixture sources](../tests/e2e/fixture/) pin base images by digest.
[Native downloads](../tests/e2e/native-downloads.json) pin archive versions,
sizes, and SHA-512 checksums. Verified native downloads are cached under
`target/e2e-downloads` between local runs. The first run after a dependency
change still downloads from the public archive service, which can be slow.

The [native driver](../tests/e2e/native/Driver.swift) locates controls through
the accessibility tree. It uses pointer and keyboard events to operate them
and checks displayed values and enabled states. Server-side execution markers
provide evidence for cancellation beyond a UI status change.

The driver records Qrow process memory and CPU samples. Its launch measurement
ends when the New Connection control becomes accessible. This does not measure
cold launch to a visible frame or rendering latency. The original M1 Mac with
8 GB memory target remains unverified; it does not block the early release.
Passing on a larger runner does not establish performance on that target.

The native scenario starts with the application menu. It selects **About Qrow**,
checks the version line and the copyright, closes the dialog with Escape, and
opens the dialog again. This check does not use a server.

The native scenario checks connection-form error alerts. It rejects a missing
username, a new connection with a duplicate name, and a rename to a duplicate
name. Each error keeps the form open for correction.

The native scenario checks result retention across connection changes. It runs a
query on one profile, downloads more than one page, switches to a second
profile, and checks that the first tab keeps its rows and session. It checks
that each connection restores its own tab set and that keep-alive continues in
hidden tabs. Returning to the first profile restores its last active tab.
The scenario also checks profile edits while another profile is selected.
Lifecycle edits preserve the session and cursor. Password changes and deletion
close the matching sessions. It checks that **Disconnect** acts on the active
tab only. Returning to a connection makes its tabs visible again. It also
checks that connection selection does not add an activity entry. It checks Copy
to Connection and Move to Connection from nested tab menus. The new tab has the
source SQL, a unique tab name, and no result rows. After a copy, the source tab
keeps its rows. After the scenario moves the last tab, the source connection
has a new blank tab. See [Connections](connections.md) for tab copy and move
behavior.

The driver also blocks writes to its temporary workspace before **⌘Q** and
window close. It checks that failed saves keep the editor open and preserve
SQL text. It selects **Keep Editing**, restores write access, and retries the
save. After Qrow exits, the driver checks the saved SQL. The second case starts
a new Qrow process with the same workspace. Core storage tests separately check
competing processes and lock release after a process is killed.

## Continuous integration

End-to-end tests are currently not run in CI/CD.

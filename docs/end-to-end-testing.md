# End-to-end testing

The end-to-end suites test Qrow against real Kyuubi and Spark servers. They
check whether connection, query, result, and session behavior work together:

- `backend` tests the connector and the worker without the UI.
- `e2e` tests the real Qrow window, headless, through GPUI input. See
  [Testing](testing.md#write-an-e2e-test) to write these tests.
- `desktop` tests the packaged application through real keyboard and pointer
  events. It checks only the menu bar, Keychain, quit, and rendered pixels.

[Testing](testing.md#run-the-servers) describes how qtest starts, shares, and
reuses the servers.

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
./qtest run backend
```

The command builds the tests, starts a disposable server stack, waits for an
authenticated SQL response, runs the backend tests one at a time, collects
artifacts, and removes the stack. It uses the servers of `./qtest fixture up`
when they answer. Initial image downloads and builds are substantially larger
than the application.

Run the headless window tests against the same servers with:

```sh
./qtest run e2e
```


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

Then run:

```sh
./qtest run desktop
```

The servers use Docker when its daemon answers, and local Java processes
otherwise. For local Java processes, set `JAVA_HOME` to a Java 17 JDK, or
select them with `--runtime native`. The Docker runtime does not require a
host JDK.
Java is a server-fixture dependency. Qrow itself remains a native Rust application.

The suite creates a release package inside the run directory. It does not
replace `dist/Qrow.app`. It uses a temporary workspace and fresh synthetic
Keychain credentials. The driver restores the previous clipboard contents
after text entry.

The `desktop` suite checks only what needs the real operating system. The
[UI tests](testing.md#write-a-ui-test) and the
[E2E tests](testing.md#write-an-e2e-test) check the other behavior of the
window in-process. The main scenario of the driver:

1. Opens **About Qrow** and **Settings…** from the application menu.
2. Adds a connection through the connection form. The password goes to the
   real Keychain, and a real query reads it.
3. Blocks writes to the workspace before **⌘Q**. The failed save keeps the
   editor open with its SQL, **Keep Editing** returns to it, and **Retry Save
   and Quit** saves and quits.

Then the driver runs these scenarios, each with a new workspace:

| Scenario | Checks |
| --- | --- |
| `window-close-only` | The failed save before a window close, as for **⌘Q**. |
| `assistant-layout-only` | Pixels of assistant messages at a scale of 1.1 and a pane width of 536: a message with inline code keeps one line, bold text wraps inside the reply, the user bubble ends at the right edge of the composer, and the composer has equal space above the field and below **Send**. |
| `assistant-selection-only` | In One Dark, a selection in a user message changes the color of its bubble and keeps the color of its text. |
| `editor-highlight-only` | In demo mode, the active line color reaches the right edge of the editor. |

The assistant scenarios use the synthetic Codex server. To run one scenario
without servers, build an isolated package and run the driver:

```sh
qrow_scenario_dir=$(mktemp -d)
mkdir -p "$qrow_scenario_dir/workspace"
QROW_DIST_DIR="$qrow_scenario_dir/package" QROW_BUILD_PROFILE=debug sh scripts/package/macos.sh
sh scripts/e2e/driver.sh --preflight
QROW_E2E_ARTIFACTS="$qrow_scenario_dir" \
QROW_DATA_DIR="$qrow_scenario_dir/workspace" \
QROW_E2E_BUNDLE="$qrow_scenario_dir/package/Qrow.app" \
target/e2e-tools/native-driver --assistant-layout-only
```

The driver saves its screenshots and pixel captures in the temporary
directory.

## Inspect failures

Each run prints its artifact directory, `target/qtest/runs/<run>/`. See
[Testing](testing.md#read-results) for its summary and logs. The end-to-end
suites add these directories:

| Path | Content |
| --- | --- |
| `fixture/` | Server logs, execution evidence, and the reference stack versions. |
| `desktop/` | Application screenshots, accessibility snapshots, and driver logs. |

Driver preflight output is in `target/e2e-tools/preflight.log`.

Start with the failed command or assertion. Use server logs to inspect connection
and execution failures. Use screenshots and accessibility snapshots to inspect
UI failures. Failed runs keep their artifacts after fixture cleanup.

The fixture prints each phase. Native archive downloads report received
bytes, total bytes, percentage, transfer rate, and estimated time left every
15 seconds.

A missing fixture, failed assertion, deadline, or cleanup failure fails the run.
The suite does not automatically rerun failed tests. Readiness checks can retry
`SELECT 1` while waiting for servers to become available.

An operating system crash or SIGKILL can interrupt Keychain cleanup. If that
happens, set `QROW_E2E_ARTIFACTS` to the `desktop/` directory of the affected
run and run:

```sh
uv run --locked python scripts/e2e/keychain.py
```

The cleanup script validates the run directory and synthetic profiles before
removing credentials. It needs the run's saved workspace to identify those
profiles. `QROW_DATA_DIR` alone does not isolate Keychain.

## How the suite works

The [fixture](../scripts/e2e/fixture.py) creates independent Docker projects,
networks, ports, and evidence volumes. Ports bind to loopback. Backend tests
exercise the real [worker](../tests/backend.rs) against these servers.

The native runtime starts Java processes on temporary loopback ports, with
[verified downloads](../scripts/e2e/servers.py). qtest packages the app before
it starts the servers to reduce peak resource use. The fixture terminates
server process groups, including Spark engines and executors, when it stops.

The fixture keeps a Spark engine for 10 minutes after its last session, so
tests that follow each other do not wait for an engine start. The Spark worker
has room for one engine.

[Docker fixture sources](../tests/fixture/server/) pin base images by digest.
[Native downloads](../tests/fixture/native-downloads.json) pin archive versions,
sizes, SHA-512 checksums, and an ordered list of sources. The Apache archives
come first from the
[Qrow E2E fixtures mirror](https://github.com/vsevolodbazhan/qrow-e2e-fixtures),
because the public Apache archive service is slow. If a source fails, or stays
below 256 KiB/s for 60 seconds, the download continues from the next source.
The last source has no speed limit. To change an archive version, add the new
archive to the mirror first. The mirror README gives the procedure.

Verified native downloads are cached under `target/e2e-downloads` between local
runs. CI downloads the archives in each run. It does not use an Actions cache,
because the mirror is faster than a cache restore. Each native archive transfer
has a 2-hour limit. The macOS CI jobs allow 150 minutes,
so a slow download from the last source still leaves time to start the fixture
and run the native checks.

The [native driver](../tests/desktop/Driver.swift) locates controls through
the accessibility tree. It uses pointer and keyboard events to operate them
and checks displayed values and enabled states. Server-side execution markers
provide evidence for cancellation beyond a UI status change.
The driver sets the Qrow window to 992 by 652 points, the window size on a
hosted macOS runner, and centers it near the top of the main display. Local and
CI runs then show the same layout, including the side on which a submenu opens.
The main display must fit this window with a 16-point margin on each side.

The driver records Qrow process memory and CPU samples. Its launch measurement
ends when the New Connection control becomes accessible. This does not measure
cold launch to a visible frame or rendering latency. The original M1 Mac with
8 GB memory target remains unverified; it does not block the early release.
Passing on a larger runner does not establish performance on that target.

Core storage tests check competing processes and lock release after a
process is killed. The driver checks the save of a real quit and window close.

## Continuous integration

The `test` workflow runs `e2e-backend` after `core-backend`, and `e2e-macos`
after `core-macos` and `e2e-backend`. It runs them for pushes to `main`, manual dispatches, and non-draft pull
requests from this repository. It skips fork pull requests. The backend job
runs `backend` with Docker servers. The macOS job runs `e2e` and `desktop` on
the hosted `macos-15` runner with local Java 17 servers. Both suites share the
servers of the job.

The macOS E2E job reuses the package artifact from `core-macos` by default. A
manual `test` dispatch can set `reuse_macos_package` to `false` to build a fresh
package. The release workflow runs the same E2E jobs before it packages a
release. CI keeps E2E evidence artifacts for one day.

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

Use an unlocked, logged-in macOS desktop session. The native driver takes focus
and sends real input events. Save work in other applications before the run.

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

The suite creates a release package inside the run directory. It does not
replace `dist/Qrow.app`. It uses a temporary workspace and fresh synthetic
Keychain credentials. The driver restores the previous clipboard contents
after text entry.

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
heartbeat every 30 seconds and stream their output to the run log. The saved
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

The native scenario also checks result retention across connection changes. It
runs a query on one profile, downloads more than one page, selects a second
profile, and checks that the downloaded rows remain visible. It then runs a new
query and checks that the new result replaces the old result. The old session is
closed when the profile changes, so the test also checks that Qrow does not fetch
unfetched rows from that session.

## Continuous integration

End-to-end tests are currently not run in CI/CD.

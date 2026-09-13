# Real-server acceptance tests

The reference deployment is Kyuubi 1.12.0, Spark 3.5.3 in standalone mode,
LDAP with synthetic users, and ZooKeeper. Docker Compose creates a new project,
network, and evidence volume for each run. Image sources are pinned by digest in
`tests/e2e/fixture/Dockerfile`, `ldap.Dockerfile`, and `tests/e2e/compose.yml`.
The [Kyuubi 1.12 configuration source](https://github.com/apache/kyuubi/blob/v1.12.0/kyuubi-common/src/main/scala/org/apache/kyuubi/config/KyuubiConf.scala)
defines the authentication and session settings used here.
The Spark processes run in Linux containers. Qrow still has no JVM dependency.

## Local use

Install Docker with Compose v2 and start its daemon. Allow approximately 8 GB
for the server stack. This is test infrastructure, not Qrow's memory requirement.
The initial image download and build are substantially larger than the app.
Use the repository's pinned Rust toolchain and Python 3.11 or later.

```sh
sh scripts/check.sh e2e/backend
```

This command builds and starts the fixture, waits for an LDAP-authenticated SQL
round trip, runs the real Rust worker tests serially, collects evidence, and
removes the containers and volumes. A missing server, failed assertion, test
process deadline, or cleanup failure fails the command. Tests never silently
pass because a fixture is unavailable. Ordinary checks and Git hooks do not
start these servers or retrieve credentials. The live tests are explicitly
ignored in ordinary Cargo runs.

Artifacts are under `target/e2e/qrow-e2e-<run-id>/`. They include the test output,
container logs and status, Kyuubi engine logs, and executor/task evidence.
Failed runs retain artifacts after removing the server stack. Concurrent runs
use separate projects and ports. Ports bind only to the Docker host's loopback.
Do not point these commands at an existing deployment: failure tests kill the
fixture's Spark driver and restart its Kyuubi container.

## Backend coverage

| Scenario | Evidence required |
| --- | --- |
| Reference version and initial database | Spark reports 3.5.3; the initialized database is correct |
| LDAP | Valid login succeeds; wrong password and unknown user fail |
| Results | Exact null, empty string, Unicode, decimal, timestamp, binary, numeric and date values; 150 columns and long cells |
| Fetching | Ordered rows across batch/page boundaries, zero rows, exact page exhaustion, 100,000-row and 64 MiB caps |
| Independent sessions | Temporary views and changed session settings do not leak to another tab; SQL errors leave healthy sessions usable |
| Cancellation | A real executor starts the blocking function; Qrow's separate authenticated cancellation interrupts it; the Spark driver records task termination within 10 seconds; another tab works during and after cancellation |
| Lifecycle | Idle disconnection loses temporary session state; heartbeat SQL preserves the user's partially fetched cursor; a failed heartbeat disconnects and stops background work |
| Setup and shutdown | Cancelling while credentials are pending prevents SQL submission after real session setup; shutdown interrupts active executor work and produces a terminal task event |
| Engine/transport loss | A started query fails, its worker disconnects, no second execution is recorded, and the next explicit query reconnects |

The blocking Hive UDF and Spark listener exist only in the fixture image.
The UDF records invocation and interruption on the executor. The driver listener
records terminal task events. Tests correlate both using the Spark application
and task IDs. They reject a normally completed query as cancellation evidence.
No automatic test reruns hide failures. Readiness retries only `SELECT 1`, before
assertions or when explicitly waiting for a restarted fixture.

The suite establishes behavior for this reference configuration. It does not
establish compatibility with every Kyuubi deployment or prove transaction
rollback. The earlier user-confirmed live `SELECT 1` remains separate evidence.

## Native UI tests

Run this only in an unlocked, logged-in macOS desktop session. The driver sends
real pointer and keyboard events; it takes focus while running. It uses the
app's accessibility tree to locate controls and assert displayed values.

```sh
sh scripts/e2e/driver.sh --preflight
sh scripts/check.sh e2e/macos
```

The compiled driver at `target/e2e-tools/native-driver`, or its invoking terminal
as required by macOS, needs Accessibility and Screen Recording permissions.
Grant these through System Settings. Preflight fails if they are unavailable;
it does not change privacy settings or substitute a headless test. A provider's
ability to build macOS software does not establish these permissions or the
presence of a usable graphical session.

The suite packages a release app under the run's artifact directory. It does
not replace `dist/Qrow.app`. It uses a temporary `QROW_DATA_DIR`, creates a
connection through the form, saves synthetic credentials to Keychain, and tests
real results, pagination, Unicode SQL selection, concurrent tabs, cancellation,
and explicit reconnect. It saves Qrow-window screenshots and accessibility snapshots at
checkpoints and on failure. Credentials for the run's fresh profile UUIDs are
removed during cleanup. Neither user profiles nor user passwords are fixtures. Text entry uses native
paste shortcuts and restores the previous clipboard contents. Clicks allow
dialog geometry and input focus to settle; assertions wait for the resulting
control values and enabled states.
An OS crash or SIGKILL during a profile save can interrupt credential cleanup;
rerun `scripts/e2e/keychain.py` with that run's `QROW_E2E_ARTIFACTS` once its
workspace exists. It validates the fixture directory and profiles before removal.

The driver records Qrow process RSS in KiB and CPU percentage through `ps`.
Its launch measurement ends when the New connection control is accessible.
It is not a cold-launch-to-visible-frame or rendering-latency measurement.
The hardware target is an M1 Mac with 8 GB RAM. This target remains unverified
and does not block the early release. Server resource use is excluded from
Qrow's process samples. No claim about M1 performance follows from a larger CI
runner passing these tests.

## Native reference servers on ARM

The native suite runs on the standard GitHub-hosted ARM `macos-15` runner.
Java 17 starts real Kyuubi 1.12.0, Spark 3.5.3 master and worker, ZooKeeper
3.9.3, and a loopback LDAP server using UnboundID 7.0.3. The backend Linux
suite retains Docker Compose and OpenLDAP. Both use the same synthetic user
LDIF, Kyuubi configuration, and executor cancellation evidence code.

No Docker VM, external host, SSH keys, or repository secrets are needed for
native E2E. Qrow itself remains native Rust; Java is only a test fixture.
Set `JAVA_HOME` to a Java 17 JDK before running `sh scripts/check.sh e2e/macos`
locally. To use Docker for the same native UI scenarios instead, run:

```sh
sh scripts/check.sh e2e/macos --runtime docker
```

This needs a local Docker daemon and does not need a host JDK.
`tests/e2e/native-downloads.json` pins downloads and SHA-512 digests.
Verified archives are cached under `target/e2e-downloads`.

`scripts/e2e/servers.py` binds servers to loopback on temporary ports,
puts configuration and logs under the run's artifact directory, and terminates
its server process groups, including engines and executors, after each run.
The app is packaged before starting servers to avoid overlapping compiler and
server memory use. Workspace and Keychain fixtures remain isolated.

## CI and merge policy

`core.yml` retains the existing lint, test, documentation, coverage,
dependency, performance, and package checks. `e2e.yml` runs the real-service
and native UI suites after a successful `core` completion event. No runner is
allocated while core is running. Both suites check out the triggering run's
`head_sha`; fork code is excluded from jobs with test infrastructure access.
`e2e / backend` runs first. Only its success
allows `e2e / macos` to run, against a fresh stack. `e2e / gate` fails if
either job fails, is cancelled, or is skipped.

Fork PRs run ordinary checks without the acceptance jobs.
Their acceptance gate intentionally stays red: a maintainer must review the
contribution and move it to an internal branch before merging. The workflow
does not use `pull_request_target`.

The native E2E and core macOS jobs both use `macos-15`. The driver preflight
checks the graphical session and automation permissions before starting servers.

Require `core / backend`, `core / macos`, `core / dependencies`, and `e2e / gate`
in branch protection. Workflow files cannot enforce this repository setting.
The gate publishes a commit status on the tested SHA because `workflow_run`
checks belong to the default branch. GitHub only enables this trigger once
`e2e.yml` exists on `main`; it cannot run from this PR alone.
Artifacts remain available for 14 days. The fixture processes are stopped after
each run, including failure. GitHub also discards the hosted runner itself.
Native UI success requires the complete suite to pass; compilation and driver
preflight alone are insufficient.

## Local verification

On 2026-09-13, all 11 real backend tests passed against the pinned reference
stack. The native release suite also passed connection creation, real result
display, pagination, Unicode selection, concurrent tabs, server-confirmed
cancellation, and reconnect. The UI tests exposed missing result/status
accessibility information; Qrow now gives those elements roles and labels.
The ordinary full quality suite passed, and native checks passed after this UI
change. The isolated release package remained within the existing size budgets.

On 2026-09-13, the native JVM fixture passed the same UI scenarios locally on
ARM, including loopback engine discovery and executor/driver cancellation
markers. The explicit `--runtime docker` UI run also passed after this change.
The [hosted verification run](https://github.com/vsevolodbazhan/qrow/actions/runs/34740343377)
also passed both backend E2E and native UI E2E on the standard ARM `macos-15`
runner. The temporary verification workflow was removed afterward.

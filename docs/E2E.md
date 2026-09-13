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
sh scripts/check.sh backend-e2e
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
sh scripts/native-e2e.sh --preflight
sh scripts/check.sh ui-e2e
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
rerun `scripts/native-e2e-cleanup.py` with that run's `QROW_E2E_ARTIFACTS` once its
workspace exists. It validates the fixture directory and profiles before removal.

The driver records Qrow process RSS in KiB and CPU percentage through `ps`.
Its launch measurement ends when the New connection control is accessible.
It is not a cold-launch-to-visible-frame or rendering-latency measurement.
The hardware target is an M1 Mac with 8 GB RAM. This target remains unverified
and does not block the early release. Server resource use is excluded from
Qrow's process samples. No claim about M1 performance follows from a larger CI
runner passing these tests.

## Separate server host for the Mac

CI runs the native app on a Mac and the reference stack on a dedicated Linux
Docker host. The Mac needs Docker CLI, the Compose v2 and Buildx plugins, SSH, Swift command
line tools, Rust, Python, and uv. The Linux account needs Docker access and
Python 3. This host must contain only disposable test infrastructure.

Local runs can use the same arrangement with an existing SSH alias:

```sh
export QROW_E2E_SSH=qrow-test-host
export DOCKER_HOST=ssh://qrow-test-host
sh scripts/check.sh ui-e2e
```

Images build on the Docker host using copied build contexts and named volumes;
there are no local bind-mount assumptions. A job-local SSH tunnel forwards the
fixture's loopback port to the Mac. The Linux host never exposes LDAP or
unencrypted HiveServer2 publicly. Host keys must be verified in advance.

## CI and merge policy

`core.yml` retains the existing lint, test, documentation, coverage,
dependency, performance, and package checks. `e2e.yml` runs the real-service
and native UI suites in a separate workflow. Its backend job waits for the core
run with the same event and commit to pass before starting tests. `e2e / backend` runs first. Only its success
allows `e2e / macos` to run, against a fresh stack. `e2e / gate` fails if
either job fails, is cancelled, or is skipped.

Fork PRs run ordinary checks without the acceptance jobs or SSH credentials.
Their acceptance gate intentionally stays red: a maintainer must review the
contribution and move it to an internal branch before merging. The workflow
does not use `pull_request_target`.

The native job uses GitHub's standard `macos-15` hosted runner, matching the
core macOS job. The driver preflight checks its graphical session and automation
permissions before running the suite. See [GitHub-hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

Configure these repository settings before enabling the native job:

- `QROW_E2E_SSH_HOST` and `QROW_E2E_SSH_USER` variables: the dedicated Linux
  Docker host's DNS name or IPv4 address and SSH user.
- `QROW_E2E_SSH_KEY` secret: that test host's SSH key.
- `QROW_E2E_KNOWN_HOSTS` secret: its independently verified known-hosts entry.

The SSH configuration and key live in the job's temporary directory and are
removed in an always-run cleanup step. The runner user's SSH files are untouched.
Do not reuse a production Docker host or production access key.

Require `core / backend`, `core / macos`, `core / dependencies`, and `e2e / gate`
in branch protection. Workflow files cannot enforce this repository setting.
Artifacts remain available for 14 days. Configure periodic cleanup of orphaned
`qrow-e2e-*` projects on the dedicated Docker host in case its Mac runner is
terminated before the workflow's cleanup executes.

Hosted runner GUI permissions, remote Docker connectivity, and the first hosted
workflow run must be verified in the configured environment. Compiling the
Swift driver or passing Rust tests does not establish native UI success.

## Local verification

On 2026-09-13, all 11 real backend tests passed against the pinned reference
stack. The native release suite also passed connection creation, real result
display, pagination, Unicode selection, concurrent tabs, server-confirmed
cancellation, and reconnect. The UI tests exposed missing result/status
accessibility information; Qrow now gives those elements roles and labels.
The ordinary full quality suite passed, and native checks passed after this UI
change. The isolated release package remained within the existing size budgets.

These local runs do not verify GitHub's hosted graphical session or the remote
Docker/SSH configuration. Those remain deployment checks for the CI setup above.

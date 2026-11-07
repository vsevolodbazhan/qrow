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

## Plan and execute checks

Use a saved plan to separate selection from execution:

```sh
./qtest plan unit ui --target linux-x86_64 --output target/check-plan.json
./qtest execute target/check-plan.json --install --output target/check-result.json
./qtest verify target/check-plan.json target/check-result.json
```

Store plans under `target/`, or outside the source directory. A plan records
the source files, target, suite definitions, runtime, repeat count, and build
settings. Execution stops if these inputs change. A plan for another target
can be made on any host. Execute it on that target's native architecture.
Windows is a planned target and cannot execute application suites yet.

Use `--runtime docker` for disposable servers. Use `--repeat N` on `plan` to
repeat suites. Each attempt keeps a separate nextest report. A failed attempt
fails the run. Verification rejects missing suites, failed suites, and empty
nextest selections. Checks without test cases, such as formatting, need a
successful command result.

Execution uses machine-wide locks for a shared Cargo directory, fixture, or
desktop. A run owns its output directory. It writes progress to `events.jsonl`
and final results to `summary.json`. Cancellation stops the child process
group and keeps partial results. Fixture cleanup failures fail the run.
Child builds receive the run's Git commit through `QROW_COMMIT`, unless it is
already supplied. Background changes to other Git refs do not rebuild each
stress-test iteration. Exported snapshots without Git keep the supplied build
identity, if any.

The portable entry point is `uv run --locked python scripts/qtest`. On Windows,
`qtest.cmd` calls this entry point. Application and native fixture support on
Windows still require platform adapters. Local `run`, hook, fixture, and
comparison commands remain available. The legacy `ci` scheduler was removed;
see [Continuous integration](ci.md) for job ownership.

To build once and execute the same binaries, use a prepared build:

```sh
./qtest plan ui e2e --runtime docker --output target/ui-plan.json
./qtest prepare target/ui-plan.json --output target/ui-builds --install
./qtest execute target/ui-plan.json --reuse-builds target/ui-builds/manifest.json --output target/ui-result.json
./qtest verify target/ui-plan.json target/ui-result.json
```

Preparation groups test binaries by features and profile. It builds each group
once before servers start. Execution verifies the source, target, compiler
settings, toolchain, and archive digests. It extracts each archive once and
uses nextest metadata without another Cargo build. Missing or stale builds
fail; execution does not fall back to compilation. A plan for another
architecture of the same OS, like `--target macos-x86_64` on Apple silicon,
prepares a cross-compiled build. Install the Rust target first with
`rustup target add`. Execute that build on the plan target. `run --reuse-builds` also
supports filtered repeats with these binaries. Doctests and non-nextest checks
remain separate commands.

## Suites

A suite is the smallest unit that you can select. Suites marked with `*`
accept a test filter.

| Suite | Checks | Needs |
| --- | --- | --- |
| `app-unit` * | Application unit tests and doctests, without repeating portable core tests. | Desktop libraries, cargo-nextest |
| `clippy-native` | Production application Clippy for the current target. | Desktop build libraries |
| `native-storage` | OS credential tests with synthetic data. | cargo-nextest; on Linux, D-Bus and GNOME Keyring |
| `native-font` | Native macOS font and tooltip layout on the main thread. | macOS, Rust |
| `native-ui` * | macOS branches of headless shortcut assertions. | macOS, cargo-nextest |
| `native-assistant` * | macOS branch of the assistant server interaction. | macOS, cargo-nextest, fixture runtime |
| `postgres-core` | Postgres connector scenarios without GPUI. | Docker, cargo-nextest |
| `trino-core` | Trino connector scenarios without GPUI. | Docker, cargo-nextest |
| `postgres-ui` | Postgres headless window scenarios. | Docker, desktop libraries, cargo-nextest |
| `trino-ui` | Trino headless window scenarios. | Docker, desktop libraries, cargo-nextest |
| `fmt` | Rust formatting. | Rust |
| `clippy` | Clippy for the core library. | Rust |
| `clippy-app` | Clippy for the application with all features. | Rust; on Linux, the desktop libraries |
| `rustdoc` | Rust API documentation without warnings. | Rust |
| `unit` * | Rust unit and integration tests that need no UI and no servers. | cargo-nextest; on Linux, the desktop libraries |
| `ui` * | Headless tests of the real Qrow window, without servers. | cargo-nextest; on Linux, the desktop libraries |
| `coverage` | Core line coverage with an 80% floor. | cargo-llvm-cov |
| `perf` | SQL, dbt, and export benchmarks with enforced budgets. | Rust |
| `perf-ui` * | Frame, scroll, editor, assistant, and Activity timings of the real window in a release-like build. | macOS, cargo-nextest |
| `perf-e2e` * | Window query latency against the real servers. | macOS, Docker or Java 17 |
| `perf-kyuubi` * | Kyuubi export transfer measurements against the real servers. | Docker or Java 17 |
| `perf-postgres` | Postgres export transfer and COPY measurements. | Docker, cargo-nextest |
| `perf-trino` | Trino export transfer measurements. | Docker, cargo-nextest |
| `perf-app` | Launch time, idle memory, and idle CPU of the release app on the desktop. | macOS desktop |
| `perf-package` | Launch time, idle memory, and idle CPU of the packaged app on the desktop. | macOS desktop |
| `scripts` | ShellCheck, actionlint, Ruff, and automation unit tests. | uv, ShellCheck, actionlint |
| `policy` | Dependency waiver dates and pinned CI actions. | uv |
| `deps` | Dependency policy, unused dependencies, advisories, licenses, and sources. | cargo-machete, cargo-deny |
| `postgres` | Postgres connector and real-window tests with a disposable server. | Docker, cargo-nextest; on Linux, the desktop libraries |
| `trino` | Trino connector and real-window tests with a disposable server. | Docker, cargo-nextest; on Linux, the desktop libraries |
| `backend` * | Connector and worker against the real servers, without the UI. | Docker |
| `e2e` * | The real Qrow window, headless, against the real servers. | Docker and, on Linux, the desktop libraries; on macOS, Docker or Java 17 |
| `package` | The release app package in `target/package/`, its installer image, and its size budget. | macOS, Xcode tools |
| `linux-package` | Build the native archive in `target/package`. | Linux desktop build libraries |
| `package-smoke` | Inspect and launch the extracted package. | A built package; on Linux, Xvfb and xdotool |
| `desktop` | Smoke checks of the packaged app on the desktop: the menu bar, Keychain, quit, and pixels. | macOS desktop, Docker or Java 17 |

The suites do the same checks on macOS and Linux. `clippy` lints the core
library without the application. `clippy-app`, `unit`, and `ui` build the
application. On Linux, they need the desktop libraries that the README lists.
On Ubuntu, `sh scripts/ci/linux-build-deps.sh` installs them. These suites
need macOS and are not available on Linux: `perf-ui`, `perf-e2e`, `perf-app`,
`perf-package`, `package`, `desktop`, `native-font`, `native-ui`, and
`native-assistant`.

The `backend`, `e2e`, and `desktop` suites use disposable LDAP, Kyuubi,
Spark, and OpenID Connect servers. `desktop` also takes over the desktop. These suites, the
`perf-*` probes, and `package` run only when you select them by name.
`package` does not replace `dist/Qrow.app`. See [Run the servers](#run-the-servers) and
[Run the desktop suite](#run-the-desktop-suite).

The suites without servers check these parts. The integration tests of the
core library are modules of one test binary,
[`tests/integration/`](../tests/integration/), so a change to the library
links one binary. The `backend` suite runs the `backend::` module of this
binary.

On macOS, `native-font` checks tooltip alignment with the native font
backend at 1× and 2× display scales. Titles, statuses, and shortcuts share
the vertical center of the first line. Checks cover system and Menlo fonts
at 75%, 100%, 110%, 125%, and 150% UI scale. It runs this check on the process main
thread because AppKit requires that thread. The other headless UI tests
use GPUI's test text backend. Run `./qtest run native-font` on macOS for the
native check. `ui` and its test filters run only the headless GPUI tests.

UI tests also compare the Connections and Sign-Ins headers with query tabs.
They check text height, control centers, and separator positions at different
UI scales and window widths. The native check repeats these comparisons with
system and Menlo fonts at 1× and 2× display scales. The query E2E test checks
header alignment before and after a query completes. Divider checks verify that
the editor and toolbar touch the sidebar border and that the active gutter
paints the adjacent pixel. They cover both display scales and pane resizing.

- Local protocol fixtures test the connector without a real Spark deployment.
  They verify client messages, but they cannot show how a real server
  responds. The `backend` suite does that.
- Worker tests check session coordination and bounded fetching.
  Transfer tests check captured fetch overrides, normal preview request sizes,
  and connection limits that stay active through file publication.
  Window tests check preset persistence, engine-specific controls, validation,
  rejection before sign-in or SQL, and Retry after the result tab closes.
- Postgres tests check first-page display before server completion, direct
  reads above both preview limits, cancellation of a blocked row consumer,
  and temporary tables, settings, and uncommitted data in the same session.
  They check bounded custom-type descriptions and writer failure before
  stalled cancellation cleanup. Synthetic driver tests check an ordered
  buffer reset behind a partial frame and block later requests until it ends.
- Postgres transport tests use synthetic peers. They check cancellation during
  SSL negotiation, the TLS handshake, authentication, and socket registration.
  They check fragmented messages, rejection of a large declared message before
  body allocation, cancellation deadlines for both sockets, and the server's
  ReadyForQuery confirmation before session reuse after a SQL error.
- The Trino cancellation test waits for **SQL accepted by the server** in
  Logs before it selects **Cancel**. It then checks session reuse. The
  **Executing** status can show before the server returns a query URL. A
  cancellation at that time can close the session; see
  [Trino limits](connections.md#trino-limits).
- Property tests check SQL validation with arbitrary Unicode and quoting.
  Keep the Proptest regression seeds that a fixed failure adds.
- Export tests use the `csv` crate to check text syntax. The Kyuubi,
  Postgres, and Trino suites also read exported text with the pinned DuckDB
  Python package. CSV checks use `allow_quoted_nulls=false` to distinguish
  nulls from empty strings and literal null markers. Parquet checks cover all
  three compression choices and typed decimals, binary values, dates, and
  timestamps. Run them through
  `./qtest` to use the Python dependencies in `uv.lock`.
- Export core tests check concurrent spool readers, cancellation wakeups,
  typed record replay, malformed records, and end row counts. Disk tests
  check reservations shared by output files and spools on one volume.
- Worker tests check cursor overflow, stale drain requests, and cancellation
  bound to the captured query. UI tests check background export ownership,
  Retry, and replay settings. Kyuubi Docker tests export the live cursor and
  retry a failed save after another query without submitting the old SQL.
  The cancellation test pauses server responses before the Cancel click, then
  releases them for protocol cleanup. The copy-limit test uses cells below
  8 MiB whose combined CSV output exceeds 10 MiB.
  Direct export tests check first-page retention, original-session temporary
  views, original result SQL after editor changes, and reruns after
  disconnect without a session warning. Wire tests cancel before SASL, the
  operation handle, and the schema arrive, and while a previous session closes. UI tests check that
  cancelling the save panel submits no SQL and reads no credentials.
  Export tests wait for the preview update after the file appears. File
  completion and window updates can arrive at different times.
  Wire tests check that completed queries close without another status request.
- Storage tests check competing processes, and the release of the workspace
  lock after a process is killed.
- The core library must build with `--no-default-features`. `coverage`
  excludes the GPUI frontend and the generated bindings.
  Coverage runs one top-level test at a time in each test binary. Export tests
  share the production memory budget in that process. This setting prevents
  unrelated tests from exhausting the shared budget. Tests that start competing
  threads still check concurrent work. Other suites keep their test parallelism.
- Qrow forbids unsafe code in its own sources, including the generated
  bindings. Third-party dependencies can contain unsafe code.

The macOS unit tests use explicit temporary keychains with synthetic tokens.
They check token removal when another executable created the item. They do not
change the default keychain or its search list. A separate password test uses
the real macOS Keychain with a randomly named synthetic item. `native-storage`
runs both checks on disposable macOS runners and removes the synthetic item.

On Linux, `native-storage` starts a private D-Bus session and a
temporary GNOME Keyring. It checks password and token storage with synthetic
values. It needs `dbus-run-session` and `gnome-keyring-daemon`. The test does
not use the desktop secret store.

Kyuubi transfer tests check adaptive request counts and normal preview
limits on the wire. Worker tests check session reuse after a preset change,
first-page retention, rejection before SQL, slot ownership through file
publication, and terminal failure without fetch retry. Registry tests check
cancellation before the download callback attaches and during shutdown.
Worker tests pause the completion notification before the worker can continue.
They check immediate export admission after producer cleanup. The file writer
must keep its slot until publication. These checks cover successful downloads,
commands without results, failures, and successful or rejected cursor drains.
A Docker backend test checks actual Spark collection behavior with executor
evidence before the first fetch. It also checks restoration for explicit
and absent settings, and the collection mode of the next normal query.

## Trino tests

Local wire tests check raw socket closure during TLS, request headers and
response bodies. They cover POST, GET, HEAD and DELETE, pooled connection
reuse, late connection rejection, and the cleanup deadline. A heartbeat's
transport can close while the primary transport stays open.
The tests also check a stalled primary body after a successful cancel response,
cancellation before GET admission, and old cancellation after session handoff.
Closed query pools must release their memory reservation before the next query.
These tests keep the old cancellation handle and session alive.
Repeated cancellation sends one `DELETE` and uses the first cleanup result.
Raw JSON tests check exact decimal text, row and schema limits, and decoding
on demand. Session tests reject cumulative header limits without partial
updates. Preview tests check positional reads while the prefix writer stays
open.
Wire tests also check the heartbeat timer while a primary response body
stops. A stalled HEAD request can stop without closing the primary socket.
Wide-row tests check the fetch byte limit and the order of a pending row.
SQL error tests check transaction-header updates before the next statement.

Run `./qtest run trino --runtime docker` to test the connector and the real
window against a disposable Trino 483 coordinator. The suite uses
synthetic credentials, a temporary certificate authority, and a loopback HTTPS
port. A pinned Adobe S3Mock 5.2.3 container supplies HTTPS storage. It shares
a network namespace with Trino, so presigned loopback URLs work from both
the coordinator and client. The fixture removes both containers after the run.
The coordinator container can use 3 GiB of memory and S3Mock 1 GiB. Parallel
tests hold several large results at the same time.
S3Mock does not check presigned signatures or expiry and does not encrypt stored
objects. Fake HTTPS peers check expiry and storage encryption headers.
The suite checks password authentication, certificate checks, types, pagination,
session changes, transactions, prepared statements, metadata, cancellation,
and result limits.
The fixture sets the client timeout to five seconds. A direct-export test
commits a spool batch, stops result reads for 12 seconds, then continues the
download. It checks repeated HEAD requests in the fixture request log and
session reuse after cancellation. Serial and parallel export tests validate and
replay 50,000 rows with Bangkok and New York session time zones. They compare
storage request logs before and after each execution. Each remote GET must
have a matching DELETE after the client commits its segment. Connector tests
run one at a time to keep this trace specific to the execution.
Fake peers check offset-bearing expiry before a GET. Offsetless expiry uses
terminal storage responses, with no retry or ACK after HTTP 403 or 410.
Other direct tests exceed both preview limits and check settings, prepared statements, transactions, and empty results.
Window tests save large direct exports, keep prepared statements and session
settings, and reuse completed spools. They check original SQL after editor
changes, warnings after disconnect, retry after tab closure, cancellation,
empty results, and preservation of an existing file for a statement with no
result set. Offline window tests cancel the save panel without SQL or credentials.
Worker tests publish query progress before the schema arrives. A window test
checks the full status in Details, including text wrapping for a long filename.
The suite then runs the same disposable coordinator with OAuth2
and a synthetic confidential client. An injectable browser follows the
coordinator and provider redirects over verified HTTPS. Tests check that connection selection does not start
sign-in. A query starts sign-in when necessary. Tests also check browser
progress and cancellation in the connection toolbar, token reuse without a
saved sign-in, and renewal without another browser login.
The fixture provider uses only synthetic accounts and an HTTPS callback at
Trino. The storage namespace stays alive during the OAuth2 restart.

The suite needs Docker and OpenSSL. It builds the tests before the server
starts. The Linux `backend` CI job runs its connector and real-window tests.
The Kyuubi `e2e` suite excludes
Postgres and Trino tests because each connector has its own fixture.

## Groups

A group selects several suites.

| Group | Suites | Use |
| --- | --- | --- |
| `default` | `fmt`, `clippy`, `clippy-app`, `unit`, `ui` | Runs when you give no selector. |
| `all` | `scripts`, `deps`, `fmt`, `clippy`, `clippy-app`, `rustdoc`, `unit`, `ui`, `perf`, `coverage` | Every local suite without servers. |

A group skips a suite that the platform does not support, for example
`package` on Linux. A suite that you select by name does not skip. It fails
with a missing prerequisite.

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
to Rust sources, tests, Cargo files, `vendor/`, themes, and embedded icons
select `fmt`, `clippy`, `rustdoc`, `unit`, and `ui`. Changes to Cargo files or
dependency policy files also select `deps`. Changes to Cargo files, dependency
policy files, or workflows select `policy`. Changes to scripts, hooks,
workflows, `qtest`, the Python dependencies, this page, or the Python and
shell files in `tests/desktop/` select `scripts`. The `scripts` suite also
lints these files.

To list the tests of a suite, run `./qtest list ui --tests`.

## Read results

Each run writes its artifacts to `target/qtest/runs/<run>/`.
`target/qtest/runs/latest` points to the most recent run. `./qtest artifacts`
prints its directory. When you set `CARGO_TARGET_DIR`, qtest and the scripts
use that directory in place of `target` for all paths on this page. Hook
snapshots set it, as
[Hooks and continuous integration](#hooks-and-continuous-integration) tells.

| Path | Content |
| --- | --- |
| `summary.json` | The result of each suite, with the JSON schema of `--json`. |
| `logs/<suite>.log` | The commands and the full output of the suite. |
| `junit/<suite>.xml` | The nextest JUnit report of a failed suite. |
| `fixture/` | Server logs, execution evidence, and the versions of the servers. |
| `desktop/` | Screenshots, accessibility snapshots, and logs of the desktop driver. |

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

Start with the failed step or assertion. Use the server logs for connection
and query failures. Use the screenshots and accessibility snapshots for
desktop failures. A failed run keeps its artifacts after the servers stop.

qtest does not run a failed test again. A test must pass on its first run.
Use `--repeat` to find unstable tests. Do not add retries. Readiness checks
can send `SELECT 1` again while the servers start.

## Check prerequisites

`./qtest doctor` checks what each suite needs and prints the fix for each
gap. Give suite names to check only those suites. It exits with code 3 when
a prerequisite is missing.

`./qtest install` installs the pinned Cargo tools: cargo-nextest,
cargo-deny, cargo-machete, and cargo-llvm-cov with its LLVM component. Give
tool names to install only those tools. The [catalog](../scripts/qtest/catalog.py)
holds the pinned versions and the prebuilt release archives of each tool.
On Apple silicon Macs and on x86_64 Linux, each tool comes from its archive,
which takes seconds. qtest checks the archive against the SHA-256 digest in
the catalog. When the download or the check fails, qtest builds the tool from
source. On other computers, the tools that have no archive for the computer
build from source.

Install the script linters separately:

```sh
brew install shellcheck actionlint
```

When Docker is installed but its daemon is not running, `backend` and
`--runtime docker` stop with code 3 and ask you to start Docker.

## Run the servers

The `backend`, `e2e`, and `desktop` suites share one set of disposable
servers in a run. qtest builds the test programs and the app package first,
then starts the servers. When two suites use the same test program, for
example `e2e` and `perf-e2e`, qtest builds it once. The servers are ready
when the synthetic user `qrow` gets an answer to `SELECT 1`. This answer also
starts the Spark engine of the user, so the first test does not wait for an
engine start.

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
  random loopback port. LDAP waits for a successful local EXTERNAL bind before
  it imports schemas. If the bind fails 100 times, LDAP startup fails and
  the fixture saves the container logs.
- `native` starts local Java processes from [verified
  downloads](../tests/fixture/native-downloads.json). It needs a Java 17 JDK
  in `JAVA_HOME`. Hosted macOS CI runners use it because they have no Docker.

The `backend` suite stops Spark engines and restarts Kyuubi, so it needs the
Docker runtime. Its tests run one at a time. Do not point a suite at an
existing deployment. The suites must control disposable servers.

The servers are Kyuubi 1.12.0, Spark 3.5.3 in standalone mode, LDAP with
the synthetic user `qrow`, and ZooKeeper. Let Docker use approximately 8 GB of memory
for them. They also run on an M1 Mac with 8 GB of memory. The first image download and build are much larger than Qrow. Java
belongs only to the servers. Qrow itself does not use a JVM.

The fixture also tests [sign-ins](connections.md#sign-in-with-openid-connect)
and TLS:

- A mock OpenID Connect provider serves HTTPS on a loopback port. It accepts
  the public client `qrow-desktop`, loopback redirect URIs with any port, and
  PKCE with `S256`. It signs tokens with RS256 and rotates refresh tokens.
- A TLS proxy in front of the binary port of Kyuubi. The plain port stays
  available for the other tests.
- A custom Kyuubi authenticator. It accepts a valid access token for the
  database usernames of its identity, and gives other passwords to LDAP.
- A new certificate authority and server certificate for each start.

Run `./qtest run e2e/sign_ins backend/oidc --runtime docker` to check browser
sign-in, token refresh, and connection authorization. The E2E tests also change
the host of a saved OIDC connection and run a query immediately after saving.

| User | Database usernames |
| --- | --- |
| `alice` | `qrow` |
| `bob` | `qrow` |
| `mallory` | None |

The tests get `QROW_E2E_TLS_PORT`, `QROW_E2E_OIDC_ISSUER`, and
`QROW_E2E_TLS_CA` (the path of the CA certificate). Tests select the user and
the token lifetime through the fixture parameters of the authorization URL.
Tests that need no servers use a mock provider in the test process, with the
synthetic certificates in
[`tests/integration/testdata/tls/`](../tests/integration/testdata/tls/).
The native runtime starts the provider and the proxy as Java processes too.

The servers keep the Spark engine of a user for 10 minutes after its last
session, so tests that follow each other do not wait for an engine start.
The Spark worker has room for one engine and two executor cores.

Catalog tests, dbt tests, and assistant tests that read live columns run one
at a time in the same test group. Each test uses a unique schema name.
Spark lists all schemas before Qrow applies a connection's catalog filter.
If another test drops a schema during that list, the catalog request can fail.
Other E2E tests can run at the same time.

The Docker runtime binds ports to loopback, and each run gets its own
Compose project and network. The servers write the execution evidence to a
directory in the artifacts of the fixture, and tests read the files there.
The [server sources](../tests/fixture/server/) pin base images by digest. The native
runtime starts Java processes on temporary loopback ports and stops their
process groups, with Spark engines and executors, at the end.

The native runtime checks the size and SHA-512 checksum of each download,
and keeps verified downloads in `target/e2e-downloads`. [The download
list](../tests/fixture/native-downloads.json) gives an ordered list of
sources for each archive. The first source is the
[Qrow E2E fixtures mirror](https://github.com/vsevolodbazhan/qrow-e2e-fixtures),
because the Apache archive service is slow. When a source fails, or stays
below 256 KiB/s for 60 seconds, the download continues from the next source.
The last source has no speed limit. To change an archive version, add the new
archive to the mirror first, as its README tells.

## Test Postgres

Run `./qtest run postgres`. This suite starts a separate disposable Postgres 17
container with a synthetic password and a temporary TLS certificate. It tests
connector results, SQL errors, cancellation, session settings, schema metadata,
TLS encryption without a trusted certificate, and certificate verification.
It also tests encrypted queries, result pages, and cancellation through the
real Qrow window. The unit suite includes a protocol test that
checks that required TLS cannot fall back to plaintext.

Cancellation tests check the result of a new query after each cancelled query.
They also check that an old cancellation handle cannot cancel new work.
Protocol tests hold the cancellation socket open to check that cleanup waits
for server closure. A cancellation timeout must prevent session reuse.
Use `./qtest run postgres --runtime docker --repeat 5` to check these timing paths.

The suite requires Docker. It does not use the Spark fixture or a native server
runtime. It removes the container, its volumes, and certificates after the run.
The backend CI job includes the connector tests. The Postgres UI tests require
a local macOS run with Docker.

The `e2e` suite selects tests for the Kyuubi fixture. The `postgres` suite
selects the Postgres tests from the same test binary and supplies their server.
Postgres tests fail if their fixture settings are missing.

## Run the desktop suite

The `desktop` suite operates the packaged app with real keyboard and pointer
events. It checks only what needs the real operating system:

1. **About Qrow** and **Settings…** open from the menu bar, and **⌘W** closes
   the Settings window.
2. A connection that you add through the form keeps its password in the real
   Keychain, and a real query reads it. This check follows the close of
   Settings, so it also checks that background results still reach the main
   window. The driver waits for the saved connection in the sidebar.
3. Pixels of the selected connection row: when the pointer is on the status
   dot, the focus outline stays above the hover fill of the dot.
4. When the workspace cannot be saved, **⌘Q** keeps the editor open with its
   SQL. **Keep editing** returns to it, and **Retry save and quit** saves and
   quits.

Then it runs these scenarios, each with a new workspace:

| Scenario | Checks |
| --- | --- |
| `window-close-only` | The failed save before a window close, as for **⌘Q**. |
| `assistant-layout-only` | Pixels of assistant messages at a scale of 1.1 and a pane width of 536: a message with inline code keeps one line, a wrapped message with inline code shows all its lines, bold text wraps inside the reply, the user bubble ends at the right edge of the composer, and the composer has equal space above the field and below **Send**. |
| `assistant-selection-only` | In One Dark, a selection in a user message changes the color of its bubble and keeps the color of its text. |
| `assistant-delete-menu-only` | **About Qrow**, **Settings…**, and **Quit Qrow** stay enabled after **Delete**, **Cancel**, or **Escape** closes the conversation delete dialog. About opens after each close. Settings opens after deletion. These actions do not need another click in the workspace. |
| `editor-highlight-only` | In demo mode, the active line color reaches the right edge of the editor. |
| `results-text-only` | In demo mode, a result cell shows the letters below the baseline, for example `g` and `p` in `Singapore Airlines`. The route cell of the same row gives the baseline. |

Use an unlocked, logged-in macOS session. The driver takes the focus, so save
your work in other applications first. Check the automation permissions:

```sh
sh scripts/e2e/driver.sh --preflight
./qtest run desktop
```

The driver uses a monotonic clock that is available on macOS 12.
The driver at `target/e2e-tools/native-driver`, or the terminal that starts
it, needs the Accessibility and Screen Recording permissions. Give them in
System Settings when preflight reports that they are missing. Preflight
writes `target/e2e-tools/preflight.log`. It does not change the settings.
`./qtest run desktop` runs the preflight before it builds the package. The
driver also checks the permissions each time it starts. CI stops before it
starts servers when a required prerequisite is missing.

The suite builds a release package in the run directory. It does not replace
`dist/Qrow.app`. It uses a temporary workspace and new synthetic Keychain
passwords. The driver restores the clipboard after it types text. It sets the
Qrow window to 992 by 652 points, the window size on a hosted macOS runner,
so local and CI runs show the same layout. The main display must fit this
window with 16 points of space on each side. The pixel checks read the
pixels of the Qrow window alone, so other windows, their shadows, and
notifications do not change the result. A check captures its region again
until two captures in a row match, so it does not measure a frame of an
animation. The driver also records the
launch time and the memory and CPU use of Qrow. Its launch time ends when the
**New connection** control is accessible.

The assistant scenarios need no servers. To run one scenario, build an
isolated package and start the driver:

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

The driver saves its screenshots and pixel captures in that directory.

A crash of the operating system or a SIGKILL can stop the removal of the
synthetic Keychain passwords. To remove them, set `QROW_E2E_ARTIFACTS` to the
`desktop/` directory of that run, and run:

```sh
uv run --locked python scripts/e2e/keychain.py
```

The script removes only the passwords of the synthetic connections in the
saved workspace of the run. `QROW_DATA_DIR` does not isolate Keychain.

## Check performance

The performance suites measure probes. A probe prints one `QROW_PERF` line
with its name, value, unit, and budget, and fails above its budget. The
budgets catch large regressions. The run summary lists the measurements of
each suite in `metrics`, and the run directory has them in `perf.json`.

| Suite | Probes |
| --- | --- |
| `perf` | SQL validation of 10 KB, 100 KB, and 1 MB. The parse of synthetic dbt manifests of 5, 15, 35, and 70 MB, the load of their saved index, and a search of the index. Kyuubi column decoding and CSV/Parquet export throughput, peak memory, and spool size on narrow, wide, and null-heavy data. |
| `perf-ui` | A frame and a scroll step of the demo result table with 141 columns, opening and typing into a tab with 1 MB of SQL, an assistant reply of 800 streamed parts and a frame of the transcript after three such replies, a keystroke in the message field, and [Activity](activity.md) with a full log of 50,000 entries: opening it, a frame, a scroll step, a new entry, the removal of old entries, a change of filter, and 100 new entries while Activity is closed. The keystroke probe draws its frames like the window does: only the views that changed render again. The other probes render the full window in each frame. The `perf` Cargo profile uses the release optimization level, but uses more code generation units and no link-time optimization. |
| `perf-e2e` | The time from **Run** to the first result row and to the next page. |
| `perf-kyuubi` | Kyuubi export throughput and peak memory for each preset. |
| `perf-postgres` | Postgres export throughput and peak memory for each preset. Normal protocol and server CSV `COPY` comparison. |
| `perf-trino` | Trino export throughput and peak memory for each preset. |
| `perf-app` | The time until the release app reports a ready UI, its memory after it idles, and its CPU use while it idles. The memory probes are the resident size and the physical footprint. Activity Monitor shows the physical footprint. The workspace has one synthetic connection and an indented query. The app idles for 18 seconds before the measurement, so the caret no longer blinks and Qrow has returned its free memory to macOS. The first launch after a build warms up, and the median of the next three counts. |
| `perf-package` | The `perf-app` probes on the app in a package zip, without a build. The zip is `QROW_MACOS_PACKAGE`, or `target/macos-package/Qrow-macos.zip`. |

CI runs `perf` with its budgets on Linux. It runs the other probes as
report-only suites, because the hosted runners are slower and less steady
than a developer computer. Pull requests skip the report-only suites. Runs
on `main` keep the measurements for 90 days. To get the measurements of a
run, download its `performance-common-performance`, `performance-common-backend`,
or `performance-macos-arm64` artifact. For example:

```sh
gh run download RUN_ID -n performance-common-performance
```

To find smaller changes, compare the probes with another revision on the
same machine:

```sh
./qtest compare main              # perf, perf-ui, and perf-app
./qtest compare main perf-ui --rounds 5 --threshold 10
```

The command builds the revision in a worktree under `target/qtest/compare/`
and runs the suites there and in your working tree. The two sides take turns
in each round, so that slow changes of the machine affect both. It prints
the median of each probe and its change, and exits with code 1 when a probe
is slower than the threshold, 25% by default. The revision must have the
performance suites.

Add a probe as a test in [`tests/perf/`](../tests/perf/) or in the `perf`
module of `tests/e2e/`, and mark it `#[ignore]`. Measure with
`support::perf::sample`, and report with `support::perf::report`. Set the
budget several times above the measured value, so that the probe fails only
for a large regression.

The [SQL benchmark](../benches/sql.rs) reports the median of 21 samples
after a warm-up. Its budgets are 5 ms at 10 KB, 25 ms at 100 KB, and 250 ms at
1 MB. The release profile favors a small size, so run `perf` after you change
it.

The [export benchmark](../benches/export.rs) runs one spool producer and one
CSV or Parquet writer in each fresh process. It checks the full row count
and publishes the file with the normal atomic Save procedure. It reports
the median of three measured samples after one warm-up. Latency is per MiB
of cell payload. The memory probe uses the largest operating-system peak
resident set size of the three samples. The CSV cases also report the
spool-to-CSV size ratio. The decoder cases use the same narrow, wide, and
null-heavy schemas and exclude input cloning from their timed interval.

Export budgets allow large regressions before failure. Integer decoding
must stay below 400 ns per cell. Wide and null-heavy decoding must stay below
50 ns per cell. Writer latency limits are 400 ms/MiB for narrow data and
800 ms/MiB for null-heavy data. Wide CSV and Parquet limits are 20 and
10 ms/MiB. Each process must stay below 384 MiB peak resident set size.
The spool-to-CSV ratio limits are 3 for narrow data, 1.25 for wide data,
and 2 for null-heavy data.

The [transfer probes](../tests/integration/transfer_perf.rs) measure the
normal Worker, spool, and CSV Save path on 50,000 rows with a different
1 KiB text value in each row. Run all engines with:

```sh
./qtest run perf-kyuubi perf-postgres perf-trino --runtime docker
```

Each case uses a fresh client process and a warmed connection. One warm-up
precedes three measured samples. The report gives median milliseconds per MiB
of published file and the largest client peak resident set size. Raw samples include file bytes
and elapsed seconds for throughput calculations. A complete probe has a
20-minute deadline. It checks every exported row and each unique ID.
Trino selects an ordered, bounded key range from TPCH sf1 orders. The range
keeps the server sort within the fixture memory limit. Conservative uses
classic paging. Balanced uses Serial spooling. Fast uses Parallel spooling.
The Custom case uses Serial spooling. The historical baseline uses classic
paging. The spooling cases use the fixture's TLS storage. The corpus contains
repeated text. These
results do not predict performance with incompressible data or remote storage.
Each sample runs one export. Separate tests check the parallel segment limit.
With Docker, Kyuubi uses a separate CONNECTION engine and an in-memory catalog
for each sample. The corpus does not need a persistent metastore. Before each
sample, the probe stops engines within its disposable fixture. This releases
the two worker cores held by the readiness engine.
With the native runtime, each sample opens a new session on the readiness USER
engine. This engine holds both worker cores. The native fixture cannot stop
engines safely. Native samples share JVM warm-up and report client memory only.
Do not compare native measurements with the Docker measurements.
Raw samples record the runtime and engine share level.
The profile starts with incremental collect off. Conservative and Custom
change it to on. With Docker, the probe also reads the driver's peak resident
set size before the session closes. This peak includes JVM startup and warm-up.

The default comparison case is `legacy_policy`. It uses the current code
with fixed 1,000-row Kyuubi requests and no collection override. It is not a
measurement of an older source revision. To compare an older executable,
set `QROW_TRANSFER_BASELINE_BIN` to an integration-test executable with
`transfer_perf::sample`. Use the same corpus, validation, and test harness.
For historical Kyuubi comparisons, use Docker. The native probe uses
`legacy_policy` as its comparison case. An older executable can request a
separate engine and wait for worker cores that the native readiness engine holds.
The sample must print `QROW_TRANSFER_SAMPLE` JSON. Set
`QROW_TRANSFER_BASELINE_REVISION` to its full commit ID. The case then has
the name `baseline`. Raw samples record that commit ID.

The Postgres protocol and `COPY` cases use the same ordered query and atomic
file publication. Both use unencrypted transport to the disposable local
server. They check row order, NULLs, commas, quotes, line breaks, and Unicode.
These cases compare local CSV formatting with server CSV formatting. They do
not measure the complete Worker and spool path. Production export continues
to use the normal protocol.

CI runs the Kyuubi, Postgres, and Trino probes in the Linux `perf-backend`
job with Docker. These probes report measurements without enforced throughput
budgets.

The [dbt benchmark](../benches/dbt.rs) makes synthetic manifests with the
[generator](../tests/support/dbt_manifest.rs) and reports the median of 7
samples. Its budgets are about 10 times the values on an M3: for 70 MB, the
parse must take less than 750 ms, the load of the saved index less than
55 ms, and a search less than 11 ms. To measure the peak memory of a parse,
write the manifests and parse one in a separate process:

```sh
cargo bench --locked --no-default-features --bench dbt -- --write /tmp/dbt
/usr/bin/time -l target/release/deps/dbt-HASH --parse /tmp/dbt/70.json
```

The generated manifests use about 450 MB of disk space. Remove them after
the measurement.

After you package the app, check its size:

```sh
uv run --locked python scripts/core/size.py
```

The [size check](../scripts/core/size.py) allows 40 MiB for the executable
and 15 MiB for the zipped bundle. Find the cause of an increase before you
change a budget.

When you publish performance results, give the build, the hardware, the
method, and what you did not measure.

## Write a UI test

Shared waits use a wall-time deadline and advance GPUI polling timers by the
measured elapsed time. Polling more often does not advance application time
faster. Use explicit clock advancement only for deterministic timer tests.
Use `click_ready` for an enabled button reached after asynchronous work. It
dispatches input once. Use `click` when disabled behavior is the test subject.
Wait failures include recent actions and the current observed labels.

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
- `app.hover(id)` and `app.hover_labelled(label)` move the pointer over an
  element, for controls that show only on hover, like **Show tab** in
  Activity or the close button of a tab.
- Settings opens in its own window. While it is open, the helpers operate
  the Settings window.

Assistant tests use the synthetic Codex server in
[`tests/desktop/fake-codex.py`](../tests/desktop/fake-codex.py).
`FakeCodex::new()` gives a workspace directory and a Codex executable for
it. `codex.workspace(...)` turns on the assistant with this executable.
Some messages make the server wait for a marker file, which
`codex.mark(name)` creates. Without the marker files, the server answers at
once. For example, it answers `initialize` only after `initialize-release`
when the test creates `hold-initialize` before the launch. The
[assistant support](../tests/support/assistant.rs) sends messages and reads
the transcript, the editor, and the approval card. `codex.processes()` lists
the server processes, and `FakeCodex::running(pid)` shows if a process still
exists. `app.pass_time(duration)` moves the test clock of Qrow's timers
forward, for example for the idle stop of Codex. Codex itself uses wall time.
Idle-stop tests wait for Qrow to process the history response after a reply.
Codex events restart the idle period, so process them before you advance the
test clock.

The synthetic Claude Code in
[`tests/desktop/fake-claude.py`](../tests/desktop/fake-claude.py) speaks the
stream-json protocol of `claude -p`. `FakeClaude::beside(&directory)` puts its
executable in the workspace directory of a `FakeCodex`, so a test can use
both harnesses. `claude.select(settings)` selects Claude Code. Its marker
files work as the Codex markers do, for example `signed-out` and `release`.
`claude.turns()` lists the messages of the turns that it ran. The unit tests
of the [Claude Code harness](../src/assistant/claude/tests.rs) use the same
script.

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
  status of the active tab, which its dot tooltip shows, and `app.wait_cell(row, column, text)` waits for a result.
- `app.select_connection(profile)` selects a connection, and `app.logs()`
  reads Logs through **Copy all logs**.
- `blocking(token, milliseconds)` makes a query that holds an executor
  task. The fixture records the task in a file on the host, and
  `evidence(token, "started")` or `app.wait_evidence(...)` reads that file.
  Register the function first with `REGISTER_BLOCKING`.
- `app.scroll_to(id)` scrolls a form to a field above or below its fold.
  It takes one more step after the field starts to show, so you can click
  the field.
- Tests run at the same time and share the Spark engine of `qrow`. Do not
  change shared state, like global tables. The fixture has two executor
  cores, so put a test that holds executors in the `blocking` module, where
  tests run one at a time. A test that stops or restarts a server belongs in
  the `backend` suite.
- Use a unique schema name for catalog tests. Put tests that create, drop,
  or read fixture schemas in the catalog test group of the
  [nextest profiles](../.config/nextest.toml). Update the group's filter when
  you add a test outside the `catalog` module.
- Check the result that a user can see. The `backend` suite checks the
  contract with the servers, for example that Spark stops a cancelled task
  in 10 seconds. Do not check it again in an E2E test.

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
Each hook runs `./qtest hook NAME` in the snapshot. The command runs the
suites that the changed files select, as `--changed` does (see
[Select tests](#select-tests)), and that the hook includes:

| Hook | Changed files | Suites |
| --- | --- | --- |
| pre-commit | The staged files. | `scripts`, `policy`, `fmt`, `clippy`, `clippy-app` |
| pre-push | The changes that the remote does not have. | `scripts`, `policy`, `deps`, `fmt`, `clippy`, `clippy-app`, `rustdoc`, `unit`, `ui` |

With a warm build cache and a change to Rust sources, pre-commit takes about
20 seconds, and pre-push takes about 2 minutes. The pre-push hook checks the
last commit of each pushed branch. When the push adds commits to a remote
branch, the hook compares with that branch. For a new or rewritten branch, it
compares with the merge base of the remote default branch.

The hooks do not run `perf` and `coverage`. CI runs them. To run them before
you push, run `./qtest run all`.

Hook snapshots share a build directory, `target/hook-checks`, in the main
checkout. All worktrees of the repository use it. To use a different
directory, set `QROW_HOOK_TARGET_DIR`. You can remove the directory to get
disk space back. The next hook then builds the dependencies again.

The test and release workflows use one validation graph. Common static,
core, backend, headless UI, and performance jobs run before native tests.
Each native test consumes its own build artifact. Portable tests run once;
OS assertions and package checks run on all applicable native targets.

[Continuous integration](ci.md) lists each job, its dependencies, prepared
build variants, evidence, cache policy, full-run measurements, and merge steps.
The only branch protection context is `ci-required`. The old `checks / ...`
jobs and the `qtest ci` scheduler have been removed.

## Limitations

- A passing E2E run shows the behavior with the reference servers. It does
  not show compatibility with every Kyuubi or Spark deployment.
- UI and E2E tests send input through GPUI. They do not verify macOS input,
  the menu bar, the real Keychain, input methods, or rendered pixels. The
  `desktop` suite covers these parts of the packaged app.
- GPUI's leak detector fails a UI test that ends with a leaked entity. GPUI
  Kit's `context_menu` keeps each dismissed menu alive, so Qrow opens its
  context menus itself.
- The servers have room for one Spark engine. Tests connect only as `qrow`.
- `--repeat` runs complete suites again. It does not run one test in a loop.
- The UI probes run on GPUI's test platform, which lays out and paints
  frames without the GPU. They do not measure Metal rendering. `perf-app`
  measures the time until Qrow reports a ready UI, not until the first frame
  is on the screen.
- Synthetic export and Docker transfer probes have local measurements on an
  M1 Mac with 8 GB of memory. These probes do not establish Metal frame times
  or performance for every query, server, or concurrent export.
- The pixel checks of `desktop` depend on the main display and its scale.
- In CI, only `perf` has enforced budgets. The other probes are report-only,
  so a slower probe does not fail CI. Compare their history on `main`.

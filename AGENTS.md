# Working on Qrow

Qrow is a personal macOS SQL workbench. Preserve fast startup and responsive
editing while connecting to Spark through Kyuubi. Read [README.md](README.md) for
the current workflow and [docs/QUALITY.md](docs/QUALITY.md) for check commands,
budgets, and dependency exceptions.

## Pull requests

- Write pull request titles and descriptions in English.

## Product and architecture

- Keep the application native Rust with GPUI. Do not introduce a webview, local
  JVM, or proprietary database driver. The working connector uses HiveServer2
  Thrift directly with LDAP/SASL PLAIN, matching the user's PyHive setup.
- Keep `src/lib.rs` and its modules buildable without the `ui` feature. UI code
  belongs in the application binary. Future connectors, such as Trino, should
  implement the connector boundary instead of changing editor behavior.
- Send connection and query work through `Worker`. Network calls, Keychain
  access, and workspace writes must not block the GPUI thread. Use event-driven
  wakeups; do not add continuous polling or repaint loops while idle.
- Keep table rendering and clipboard behavior in `src/ui/results.rs`. Extract
  new state-management and form logic into focused modules rather than growing
  the root render method. Avoid unrelated refactors when fixing a small issue.
- Autocomplete, schema exploration, export, and multi-statement execution are
  outside the agreed prototype scope unless the user requests them.

## Session and data invariants

- Each tab owns an independent session and supports one active query. Separate
  tabs may run concurrently. Switching profiles preserves SQL but releases the
  old session and result preview. Busy tabs cannot switch profiles or close.
- Cancellation uses a separate authenticated transport so it cannot queue
  behind a blocked fetch. Preserve cancellation during connection setup. A
  cancellation request is not proof of server cancellation or transaction rollback.
- Never automatically resubmit SQL after a transport failure. Retain the user's
  SQL and explain the failure; the next explicit execution may reconnect.
- Fetch bounded previews. Preserve both row and memory caps, exact textual
  decimals/timestamps, null versus empty-string distinctions, and binary values.
  Do not add a SQL `LIMIT` to implement client-side previewing. Server
  `hasMoreRows` metadata can be unreliable; preserve the tested exhaustion logic.
- Virtualize both rows and columns. Real results can have over 140 columns.
  Truncate displayed cell previews without truncating stored or copied values.

## GPUI lessons

- Upgrade GPUI, GPUI Component, and its assets as a compatible set. Their APIs
  can change together; a newer component release may target a different GPUI
  package. Check Cargo.toml and Cargo.lock before choosing versions.
- Keep the `runtime_shaders` feature unless the build requirements deliberately
  change. It allows builds with Xcode command-line tools without the separate
  Metal compiler. Initialize GPUI Component, assets, the Root view, and the SQL
  language registry before constructing editor controls.
- Native input selection ranges are UTF-16. Use the input handler's range/text
  APIs when executing selected SQL; do not use UTF-16 offsets as Rust byte
  offsets. Verify selection with emoji and non-Latin text.
- Check actual pointer behavior after UI changes. During the migration, the
  component splitter could remain active after mouse release, and row scrolling
  consumed horizontal wheel events. Qrow has explicit handlers for these cases.
  Test dragging, release, the next click, both scroll axes, and modal overlays
  before removing or replacing those handlers.
- Keep table overlays and scrollbars bounded to the table's layout container.
  Reset table selection and scroll position when replacing results, while
  preserving the position when appending another preview batch.
- Flush workspace state on both application quit and last-window closure.
  Restoring tabs must not eagerly connect to databases or restore result sets.

## User data and verification

- Use `--demo` for visual checks without database or persistence access. For
  persistence tests, use a temporary `QROW_DATA_DIR` and synthetic profiles.
  That variable isolates workspace files, **not Keychain**; use fresh profile
  UUIDs and never reuse the user's credentials in fixtures.
- Preserve workspace format compatibility, profile UUIDs, and Keychain service
  identifiers. A corrupt workspace must remain untouched, with saving disabled
  for that run. Never overwrite the real workspace to set up a test.
- The user confirmed a real Kyuubi query worked. Do not erase that evidence,
  claim compatibility with every deployment, or imply live cancellation was
  verified by local protocol tests. The probe only runs `SELECT 1`.
- Run the checks appropriate to the change through `scripts/check.sh`. Use
  `core` for backend changes, `native` for UI changes, `scripts` for automation,
  and the full suite for dependency or cross-cutting changes. Native Clippy and
  core coverage do not verify GUI interaction behavior.
- Do not loosen lint rules, coverage floors, performance budgets, or dependency
  policy merely to make checks pass. Investigate failures. Keep unavoidable
  dependency exceptions specific, justified, and subject to enforced review dates.
- Preserve generated Proptest regression seeds when fixing discovered failures.
  Change Thrift generation through `scripts/generate-thrift.sh` and the vendored
  interface; do not hand-edit generated bindings without a reproducible change.
- Hooks check Git snapshots, not the working copy. Include required code and
  configuration in the staged change instead of bypassing a failing hook. Hook
  scripts must not stash, restage, or modify the user's files.
- Measure release builds. The startup log reports UI initialization, not cold
  launch to first visible frame. SQL benchmarks do not measure rendering or
  remote Spark latency. Report exactly what was measured and what was not tested.
- `cargo build` does not update `dist/Qrow.app`. Use the packaging script when a
  refreshed distributable is needed. Checks and hooks must not replace the app
  while the user is testing it. If UI automation fails, report the verification
  gap instead of treating an attempted interaction as a pass.

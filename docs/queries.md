# Queries

Each query tab contains SQL, a selected connection, a result preview, and an
Logs panel. A tab can run one query at a time. Other tabs can run
concurrently.

## Run SQL

1. Select a [connection profile](connections.md).
2. Enter SQL in the editor.
3. Select the text to run, if you want to run only part of the editor.
4. Click **Run** or press **⌘Enter**.

Without a selection, Qrow submits the full editor contents. It does not select
the statement under the cursor. Each execution must contain one SQL statement.
Qrow rejects multiple statements before sending them to the server.

A new execution clears the previous preview. Results appear as Qrow fetches
them. The Logs panel remains across executions in the tab. See
[Results](results.md) for paging and storage limits.

## Logs

Select **Logs** beside **Results** to inspect Qrow activity. The history shows
connection changes, the exact SQL submitted to the selected connection,
execution completion, preview page fetches, cancellation, and errors. Each
entry starts with a wall-clock timestamp in brackets, followed by its message.
Errors use the error color. Durations are client measurements.

Use **Font Family**, **Font Size**, and **Line Height** in the Logs section of
Settings to change the Logs text. The default line-height multiplier is 1.2.

Qrow does not retrieve Kyuubi or Spark logs. It does not show individual row
batches or successful keep-alive checks. It shows keep-alive failures.

Qrow keeps Logs history in memory. Closing the query tab or quitting Qrow
deletes that history. **Clear** deletes the current history. **Copy All** and
**Copy Error** copy the stored text. Text selection and copying preserve
multiline and Unicode error details.

Qrow selects Logs when the active query fails. A failed background query does
not change the active query tab. It shows an unread error indicator on that
tab. A successful retry selects Results after its first preview fetch. A
statement without a result set keeps Logs selected. Selecting a panel after
a failure cancels the automatic panel change.

Qrow limits history to 100 activity groups and 8 MiB per query tab. Qrow
removes complete old groups when a limit is reached. The latest execution and
its complete error are kept. A single latest execution can exceed 8 MiB. Qrow
shows a retention notice when it removes old groups.

## Manage tabs

Press **⌘T** or click **+** in the tab strip to create a tab. The new tab uses
the active tab's selected profile, but has its own session.

Right-click a tab and choose **Edit Tab…** to rename it. The current name appears
as the placeholder. Leave the field empty to keep that name. Renaming preserves
the SQL, connection, and downloaded results.

A busy tab cannot close or switch profiles. Switching tabs does not stop work
in another tab. See [Connections](connections.md) for session behavior.

## Cancel work

Click **Cancel** to request cancellation. A request does not prove that the
server has stopped the query. Cancellation cannot roll back completed SQL.

During connection setup, Qrow records the request. It prevents submission of
the user's SQL after setup returns. It does not immediately interrupt every
setup step. During result fetching, cancellation releases the cursor and keeps
rows that Qrow has already downloaded.

Individual network reads have a 120-second timeout. This is not a limit on the
total query duration. A failed cancellation request is reported as an error.

On application exit, Qrow saves the workspace and requests cancellation and
session cleanup. It waits up to one second for worker shutdown. If the server
cannot be reached, server idle and session timeouts remain responsible for
abandoned resources.

## Errors and limitations

SQL, Logs history, and downloaded rows remain available after a query or
fetch failure. Reconnection requires an explicit Run and does not restore
session settings from earlier SQL statements.

## Syntax

The editor provides syntax generic SQL syntax highlighting. Autocomplete, schema exploration, and LSPs are not implemented.

## Design

The [UI](../src/ui.rs) reads selected text through the native input handler.
Native selection ranges use UTF-16 offsets. Rust string offsets use bytes.
Using the input handler preserves selections that contain emoji or non-Latin
text. The [SQL validator](../src/sql.rs) checks statement boundaries locally.

The [worker](../src/worker.rs) runs connection, execution, and fetch work outside
the UI thread. The [connector](../src/connector/hive.rs) sends cancellation over
a separate authenticated transport. This prevents cancellation from waiting
behind a blocked fetch on the query transport.

Cancellation depends on the server accepting the operation handle on that
transport. The [end-to-end suite](end-to-end-testing.md) checks server-side
cancellation in its reference configuration. This evidence does not prove
transaction rollback or cancellation compatibility with every Kyuubi deployment.

# Queries

Each query tab contains SQL, a selected connection, a result preview, and a
Logs panel. A tab can run one query at a time. Other tabs can run
concurrently.

## Run SQL

1. Select a [connection profile](connections.md). Qrow shows that connection's tabs.
2. Select or create a tab under that connection.
3. Enter SQL in the editor.
4. Select the text to run, if you want to run only part of the editor.
5. Click **Run** or press **⌘Enter**.

Without a selection, Qrow submits the full editor contents. It does not select
the statement under the cursor. Each execution must contain one SQL statement.
Qrow rejects multiple statements before sending them to the server.

The dot of a tab shows the state of its work. Point to the dot to read the
status of the last query, for example **Error: Connection failed**. The bar
above Results shows neutral tags for the visible rows, loaded rows, columns,
and duration of the last query. At small widths, **Result Details** shows
the values in a popover. See [Results](results.md#browse-and-copy) for the
controls and keyboard use. Action tooltips show
the command name and its keyboard shortcut on one line. The name and shortcut
share a vertical center.

A new accepted execution clears the previous preview and the unread outcome
of that tab. The blue dot shows that the execution continues. Results appear
as Qrow fetches them. Switching connections does not clear the preview or
close the tab's session.
The Logs panel remains across executions in the tab. A successful execution
also clears the unread error, even if you selected Logs after the failure. See
[Results](results.md) for paging and storage limits.

## Logs

Select **Logs** beside **Results** to inspect the work of the tab. The history shows
connection and session events, the exact SQL submitted to the tab's owning
connection, execution completion, preview page fetches, cancellation, and
errors. Selecting a connection or query tab does not add a log entry. Each
entry starts with a wall-clock timestamp in brackets, followed by its message.
Errors use the error color. Durations are client measurements.

Use **Font Family**, **Font Size**, and **Line Height** in the Logs section of
Settings to change the Logs text. The default line-height multiplier is 1.2.

Logs show only the work that changes something for the tab. A failed
keep-alive shows with its SQL and the error, because it closed the session.
Select **Show Activity** on that line to see the other work of the connection.
Successful keep-alives and schema refreshes show only in
[Activity](activity.md). Qrow does not retrieve Kyuubi or Spark logs. It does not show individual row
batches.

Qrow keeps Logs history in memory. Closing the query tab or quitting Qrow
deletes that history. **Clear** deletes the current history. **Copy All** copies
all stored entries with the timestamps that Logs shows. **Copy Error** copies
the latest error with its timestamp. Text selection and both copy commands
preserve multiline text and Unicode error details.

Qrow selects Logs when a query fails. It selects Results when a query succeeds,
after the first preview fetch or completion without a result set. This applies
even if you selected Logs before completion. You can select either panel again
after completion. Background queries update their own panel without changing
the active query tab. A failed background query shows a red dot on its tab.
A successful background query shows a green dot until its Results show.

## Query tab state

Each query tab has one status dot before its close button:

| Dot | State |
| --- | --- |
| Dim blue | The session is connected and idle. The dot uses 40% opacity. |
| Faint blue | The session is connecting. The dot uses 20% opacity. |
| Blue | Query or session work continues. |
| Green | A query result is ready and unread. |
| Red | A query or keep-alive error is unread. |
| No dot | No live session, work, or unread result. |

Point to the dot to see the tab name and a short status beside it. The status
uses secondary text. The name and status share a vertical center. If the name
wraps, the status stays beside its first line. The accessible name also gives
the states in words.
The dot shows SQL execution and session state. The priority is error, running
work, connecting, unread success, then connected and idle. Assistant turns and
approval requests use the
[conversation indicators](assistant.md#conversation-state). SQL that the
assistant runs uses the same query dot.

Results and errors that finish behind [Activity](activity.md) stay unread.
Show the Results to read a successful query. Show Logs to read an error.
A new execution clears the unread outcome of that tab. Reading one tab does
not read the other tabs of its connection. When you read the outcome, the
dot returns to dim blue if the session stays open. Otherwise, it disappears.

## Logs history

Qrow puts Logs entries into history groups. One group holds all entries of
one query execution. An entry without an execution has its own group. Examples
are a failed keep-alive, a disconnect, and rejected SQL.

Qrow limits the history of each query tab to 100 execution groups, 50 other
groups, and 8 MiB of text. When one type of group is more than its limit, Qrow
removes the oldest complete groups of that type. Thus disconnects and rejected
SQL do not remove query history. When the text is more than 8 MiB, Qrow removes the oldest
complete groups of any type. Qrow keeps the latest execution and the group
of the latest error. A single latest execution can be more than 8 MiB. Qrow
adds an `Older log entries were removed` line at the start of Logs history when it
removes old groups. The line marks the boundary before the retained entries.
It has no timestamp because it does not describe a timed event. The line
appears in the entry count and **Copy All** output.

## Manage tabs

Press **⌘T** or click **+** in the tab strip to create a tab. The new tab belongs
to the active connection and has its own session.

Each connection always has at least one tab. Closing or moving the last tab
creates a blank replacement. See [Connections](connections.md#manage-tabs-under-a-connection)
for copy and move actions.

Right-click a tab and choose **Rename…**. Enter a unique name, or leave the
field empty to keep the current name. Renaming preserves the SQL, connection,
and downloaded results.

A busy tab cannot close. A tab also cannot close while its [assistant
conversation](assistant.md#conversations-and-query-tabs) works or waits for
approval. Switching connections or tabs does not stop work in another tab. See [Connections](connections.md) for session behavior.

## Cancel work

Click **Cancel** to request cancellation. A request does not prove that the
server has stopped the query. Cancellation cannot roll back completed SQL.

During connection setup, Qrow records the request. It prevents submission of
the user's SQL after setup returns. It does not immediately interrupt every
setup step. During result fetching, cancellation releases the cursor and keeps
rows that Qrow has already downloaded.

Each network read waits up to the
[response timeout](connections.md#authentication-and-connection-failures) of
the connection, 300 seconds by default. This is not a limit on the total query
duration. A read that times out fails with `Kyuubi did not answer within 300
seconds`, with the timeout of the connection. A failed cancellation request is reported as an error.

On application exit, Qrow uses the [workspace close
confirmation](workspace.md#quit-and-save) if a query is active, then requests
cancellation and session cleanup. It waits up to one second for worker
shutdown. If the server cannot be reached, server idle and session timeouts
remain responsible for abandoned resources.

## Errors and limitations

SQL, Logs history, and downloaded rows remain available after a query or
fetch failure. Reconnection requires an explicit Run and does not restore
session settings from earlier SQL statements.

## Syntax

The editor highlights SQL syntax and the full width of the active line.
Autocomplete, schema exploration, and language server features are not available.

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
transport. The [E2E tests](testing.md#suites) check server-side
cancellation in its reference configuration. This evidence does not prove
transaction rollback or cancellation compatibility with every Kyuubi deployment.

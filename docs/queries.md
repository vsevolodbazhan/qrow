# Queries

Each query tab contains SQL, a selected connection, one or more
[result tabs](results.md#result-tabs), and a Logs panel. A tab can run one query at a time. Other tabs can run
concurrently.

## SQL files

Select a connection, then use **File → Import SQL file…** (**⌘O**) to open
a UTF-8 SQL file. The file opens in a new query tab under that
connection. The tab uses the file name without its extension. Duplicate
names get a copy suffix. Import does not run SQL. An unreadable file shows
an error and does not add a tab.

Right-click a tab and select **Save as SQL file…** to save its full SQL text.
Press **⇧⌘S** to save the active tab.
Choose a path in the save panel. The suggested name has a `.sql` extension.
On Linux, use Ctrl in place of ⌘.

Tabs and files are separate copies. Workspace autosave does not change an
imported or saved file. Use **Save as SQL file…** again to write later edits.
Files contain SQL only; they do not contain results or connection settings.

## Run SQL

1. Select a [connection profile](connections.md). Qrow shows that connection's tabs.
2. Select or create a tab under that connection.
3. Enter SQL in the editor.
4. Put the cursor in the statement to run, or select SQL text.
5. Click **Run** or press **⌘Enter**.

Without a selection, Qrow runs the full SQL statement at the cursor.
The statement can extend across several lines. Semicolons in quoted text
or comments do not end a statement. Spaces before or after a statement on
the same line belong to that statement.

If the cursor is on a blank line or a comment between statements, Qrow does
not run SQL. Move the cursor into a statement, or select SQL text.
Blank lines and comments inside a statement belong to that statement.

If you select text, Qrow runs that text. Each execution must contain one SQL
statement. Qrow rejects a selection with multiple statements before it sends
SQL to the server.

The editor shows the statement that **Run** uses with a light tint. On a line
that contains only that statement, the tint goes across the full width. When
the cursor is on a blank line or a comment between statements, there is no
tint. When you select text, the selection shows the SQL to run, and there is
no tint.

After a query finishes, use **Export results…** in Results to save all rows.
If the result cursor is unavailable, review the original SQL before an export
rerun. See [Rerun for export](results.md#rerun-for-export) for session checks
and cancellation limits.

The dot of a tab shows the state of its work. Point to the dot to read the
tab name and a short status. The tooltip has no query detail line. The bar
above Results shows neutral tags for the visible rows, loaded rows, columns,
and duration of the last query. At small widths, **Result Details** shows
the values in a popover. See [Results](results.md#browse-and-copy) for the
controls and keyboard use. Action tooltips show
the command name and its keyboard shortcut on one line. The name and shortcut
share a vertical center.

A new accepted execution clears the unpinned result tab and the unread outcome
of that tab. Pinned result tabs keep their rows. See
[Result tabs](results.md#result-tabs). The blue dot shows that the execution continues. Results appear
as Qrow fetches them. Switching connections does not clear the preview or
close the tab's session.
The Logs panel remains across executions in the tab. A successful execution
also clears the unread error, even if you selected Logs after the failure. See
[Results](results.md) for paging and storage limits.

## Logs

Select the **Logs** tab before the result tabs to inspect the work of the tab. The history shows
connection and session events, the exact SQL submitted to the tab's owning
connection, execution completion, preview page fetches, cancellation, and
errors. Selecting a connection or query tab does not add a log entry.

Each entry is a row. The wall-clock time is at the left, then an error icon
for an error, the message, and the actions of the entry. The copied text has
the date and the time in brackets. Errors use the error color. SQL shows in a
code block below its message. Messages use the interface font. Times and SQL
use the Logs font. Durations are client measurements, for example
`Query finished on the server (842 ms)` or `Fetched rows 1–2,250 (all rows)`.
Numbers group their digits.

Use **Font Family**, **Font Size**, and **Line Height** in the Logs section of
Settings to change the times and the SQL. The default font is JetBrains Mono,
and the default line-height multiplier is 1.2.

A code block of more than 5 lines shows its first 5 lines, which fade out at
the bottom. Click **Expand query** at the right of the entry to show all lines,
and **Collapse query** to show 5 lines again. **Copy query** on a submitted
query copies all of its SQL, also the lines that a collapsed block does not
show.

Logs show only the rows on the screen and near it, so a long history scrolls
and selects text without delay. While you are at the end of Logs, the list
follows new entries. When you scroll up, it keeps its place. Drag in the text
to select it, or double-click a word. A selection can continue across
entries.

Logs show only the work that changes something for the tab. A failed
keep-alive shows with its SQL and the error, because it closed the session.
Select **Show activity** on that line to see the other work of the connection.
Successful keep-alives and schema refreshes show only in
[Activity](activity.md). Qrow does not retrieve Kyuubi or Spark logs. It does not show individual row
batches.

Qrow keeps Logs history in memory. Closing the query tab or quitting Qrow
deletes that history. The Logs toolbar shows the number of entries in a chip,
and its commands as icon buttons with tooltips. **Copy all logs** copies all
stored entries with their dates and times. **Copy latest error** copies the
latest error with its date and time. It shows only when the history has an
error. **Clear logs** deletes the current history. Text selection and the copy
commands preserve multiline text and Unicode error details.

Qrow selects Logs when a query fails. It selects the result tab of the query
when the query succeeds, after the first preview fetch or completion without a
result set. This applies even if you selected Logs before completion. You can
select Logs or any result tab again after completion. Background queries update their own panel without changing
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
appears in the entry count and **Copy all** output.

## Manage tabs

Press **⌘T** or click **+** in the tab strip to create a tab. The new tab belongs
to the active connection and has its own session.

Press **⌘1** through **⌘9** (**Ctrl+1** through **Ctrl+9** on Linux) to select
a query tab by its position from the left under the active connection. If
that position has no tab, the selection does not change. These shortcuts do
not switch tabs while a dialog is open.

A tab that is not selected is at most 240 points wide. A long name ends in an
ellipsis. The selected tab grows to show its full name. Only a name that is
wider than the visible strip ends in an ellipsis. When you select another tab,
the previous tab shrinks back. When the strip is short of room, the tabs that
are not selected shrink, down to a width that shows a short name like
`Query 12` in full. When only a little room is missing, they shrink a little
more, so a few tabs fit without a scroll. Below these widths, the strip
scrolls:

- Scroll it with a trackpad swipe or the mouse wheel. When the scroll stops,
  a tab starts exactly at the left edge. At the end of the strip, the last tab
  ends at the right edge.
- An edge with more tabs past it shows a short shade. A tab that an edge cuts
  fades into the strip, and it hides its status dot and close button. A tab
  cut by the left edge shows the start of its name at the edge.
- Click a cut tab to select it. The selected tab always shows in full. The
  strip scrolls to it after you select it, add or close a tab, rename a tab,
  or change the width of the window, the sidebar, or the assistant pane.
- Point to a tab to see its full name and its status in a tooltip, for example
  `Query 5` and `Not connected`. Each tab shows this tooltip, also the selected
  tab and a tab with a status dot. Tab tooltips close when the pointer moves
  away, when the strip scrolls, and when the window changes size.

Each connection always has at least one tab. Closing or moving the last tab
creates a blank replacement. See [Connections](connections.md#manage-tabs-under-a-connection)
for copy and move actions.

Right-click a tab and choose **Rename…**. The **Rename Tab** dialog opens with
the current name selected, so you can type a new name at once. Enter a unique
name, or leave the field empty to keep the current name. Renaming preserves
the SQL, connection, and downloaded results.

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

For Kyuubi, each network read waits up to the
[response timeout](connections.md#authentication-and-connection-failures) of
the connection, 300 seconds by default. This is not a limit on the total query
duration. A read that times out fails with `Kyuubi did not answer within 300
seconds`, with the timeout of the connection. A failed cancellation request is reported as an error.

For Postgres, see [Postgres limits](connections.md#postgres-limits) for timeout
and transaction behavior. For Trino, see [Trino limits](connections.md#trino-limits).

On application exit, Qrow uses the [workspace close
confirmation](workspace.md#quit-and-save) if a query is active, then requests
cancellation and session cleanup. It waits up to one second for worker
shutdown. If the server cannot be reached, server idle and session timeouts
remain responsible for abandoned resources.

## Errors and limitations

SQL, Logs history, and downloaded rows remain available after a query or
fetch failure. Reconnection requires an explicit Run and does not restore
session settings from earlier SQL statements.

## Format SQL

Right-click in the SQL editor and choose **Format**, or press **⌥⌘L**.
If you select text, the command formats each statement in that selection.
Otherwise, it formats the statement at the cursor. Text between statements
stays the same. Press **⌘Z** once to undo the command.

The command uses **Tab Size** and **SQL Keyword Case** from
[Editor settings](workspace.md#appearance-and-layout). It does not run SQL
or open a connection. Formatting runs only when you request it.

The formatter checks that SQL tokens stay the same. If the check fails,
the command keeps that statement and shows a warning. Unsupported statements
also stay the same. The command uses the quoting rules of the connection
type: Spark SQL, Postgres, or Trino. Quoted text keeps its exact content.

## Syntax

The editor highlights SQL syntax and the full width of the active line. SQL
keywords such as `DESC` share one keyword color. In light themes with a
saturated comment color, comments use the muted text color. An empty editor
shows the hint `Write SQL, then press ⌘↩ to run`. In a narrow editor, the hint
wraps.
Autocomplete, schema exploration, and language server features are not available.

## Design

The [UI](../src/ui.rs) reads selected text through the native input handler.
Native selection ranges use UTF-16 offsets. Rust string offsets use bytes.
Using the input handler preserves selections that contain emoji or non-Latin
text. The [SQL validator](../src/sql.rs) checks statement boundaries locally.
The tint and **Run** use the same statement search, so the tint shows the SQL
that **Run** sends.

The [worker](../src/worker.rs) runs connection, execution, and fetch work outside
the UI thread. The [connector](../src/connector/hive.rs) sends cancellation over
a separate authenticated transport. This prevents cancellation from waiting
behind a blocked fetch on the query transport.

Cancellation depends on the server accepting the operation handle on that
transport. The [E2E tests](testing.md#suites) check server-side
cancellation in its reference configuration. This evidence does not prove
transaction rollback or cancellation compatibility with every Kyuubi deployment.

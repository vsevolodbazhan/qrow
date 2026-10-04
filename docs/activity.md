# Activity

Activity shows the background work of a connection in one log: schema
refreshes, keep-alives, sessions, and the queries of its tabs. Use it to find
why a refresh failed or when a session closed.

## Open Activity

Do one of these steps:

- Click **Activity** at the right end of the status bar.
- Press **⇧⌘U**. Press it again to close Activity.
- Select **View → Activity**.
- Right-click a connection, then select **Show Activity**.
- Click the status dot on a connection row.
- Click **Show Activity** on a failed keep-alive line in the Logs of a tab.

Activity covers the main area of the window. Press **Esc** or click **Close**
to go back.

The status bar opens the connection with the newest unseen refresh error.
Otherwise, it opens a connection with an unread tab error, if one exists.
If no connection has an unseen error, it opens the connection of the active tab.
Use the connection list in the Activity header to show another connection.

## Read Activity

Each connection has one log. The entries are in the order that they occur,
with the newest at the end. Each entry starts with a timestamp in brackets,
like an entry in the [Logs of a tab](queries.md#logs). Errors use the error
color. The log follows new entries while you are at the end of it. **Jump to
Latest** goes back to the end.

| Work | Entries |
| --- | --- |
| Schema refresh | The start, the session, each request with its duration and count, and the result. Errors are in full. |
| Keep-alive | The tab, the outcome, and the duration. A failed keep-alive tells that it closed the session. |
| Session | Opened, closed, and idle disconnect, for each tab of the connection. |
| Query | The tab submitted a query, then the query completed, failed, or was cancelled. |

Entries of a tab start with the tab name, for example `Query 3: Submitted a
query`. Activity does not show SQL text or query errors. Click **Show Tab** to
see the tab of the query and its Logs, which have the details. An entry of a
closed tab stays, without **Show Tab**.

A shared catalog refresh goes to the Activity of the connection that ran it.

Select **Errors only** to show only errors. **All activity** shows all
entries. **Copy All** copies the entries that show. **Copy** on an error
copies that error. Both actions include the recorded timestamps and the full
text of each entry. **Clear** deletes the log of the connection.

## Unseen errors

The connection menu shows each connection name and its unread error count as
separate labels.

The Activity icon keeps its shape. Its dot is at the top right corner of the
button. A red dot means that a connection has an unseen refresh error or a
tab has an unread error. The tooltip and the
accessible name give the count. A refresh with several failed requests counts
once. Each tab with an unread query or keep-alive error counts once.

When the Activity of a connection shows, its refresh errors become seen.
An unread tab error stays until the Logs of that tab show. Use **Show Tab**
to open the tab. The Activity entry keeps its error color after the error
becomes seen.

A blue dot means that query or session work continues, or a schema refresh
runs or waits. An unread error takes priority over work. The tooltip shows
**Activity** and its keyboard shortcut on the first line.
The name and shortcut share a text baseline.
Short status lines below it give the error count and the work that continues.
These lines use secondary text, also when the dot is red. Activity has no
green completion dot.

## Limits

Qrow keeps Activity in memory. Quitting Qrow deletes it. Each connection keeps
up to 50,000 entries and 8 MiB of text. When a log is larger, Qrow removes the
oldest entries and adds an `Older entries were removed` line at the start.
Deleting a connection deletes its Activity.

The [assistant](assistant.md) cannot read Activity.

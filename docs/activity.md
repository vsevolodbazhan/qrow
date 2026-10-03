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
- Click the warning icon or the spinner on a connection row.
- Click **Show Activity** on a failed keep-alive line in the Logs of a tab.

Activity covers the main area of the window. Press **Esc** or click **Close**
to go back.

The status bar opens the connection with the newest unseen error. If no
connection has an unseen error, it opens the connection of the active tab.
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
copies that error. **Clear** deletes the log of the connection.

## Unseen errors

A red dot on the **Activity** button means that a connection has unseen
errors. Point to the button to read their number. These are
failed schema refreshes, which no tab shows. A refresh with several
failed requests counts once. When the Activity of a connection shows, its
errors become seen.

A failed query or a failed keep-alive does not count. It marks its tab with an
unread error indicator, and its Activity entry uses the error color.

The **Activity** button shows a spinner while a schema refresh of a connection
runs or waits.

## Limits

Qrow keeps Activity in memory. Quitting Qrow deletes it. Each connection keeps
up to 50,000 entries and 8 MiB of text. When a log is larger, Qrow removes the
oldest entries and adds an `Older entries were removed` line at the start.
Deleting a connection deletes its Activity.

The [assistant](assistant.md) cannot read Activity.

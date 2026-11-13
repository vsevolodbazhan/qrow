# Activity

Activity shows background work in logs. Each connection has a log of its
schema refreshes, keep-alives, sessions, and the queries of its tabs. The
**Qrow** log has the events of the app itself, such as
[update checks](installation.md#updates). Use Activity to find why a refresh
failed, when a session closed, or why an update did not arrive.

## Open Activity

Do one of these steps:

- Click **Activity** at the right end of the status bar.
- Press **⇧⌘U**. Press it again to close Activity.
- Select **View → Activity**.
- Right-click a connection, then select **Show activity**.
- Click the status dot on a connection row.
- Click **Show activity** on a failed keep-alive line in the Logs of a tab.

Activity covers the main area of the window. Press **Esc** or click **Close activity**
to go back.

The status bar opens the connection with the newest unseen refresh error.
Otherwise, it opens a connection with an unread tab error, if one exists.
If no connection has an unseen error, it opens the connection of the active tab.
Without connections, it opens the **Qrow** log. Use the source list in
the Activity header to show another log. **Qrow** is always first in the list.

## Read Activity

Each connection has one log. The entries are in the order that they occur,
with the newest at the end. The log follows new entries while you are at the
end of it. **Jump to latest** goes back to the end.

Each entry is a row with lanes, like an entry in the
[Logs of a tab](queries.md#logs):

1. The time of the entry. The copied text also has the date.
2. An error icon, if the entry is an error. Errors also use the error color.
3. The source of the entry: the tab name, or **dbt**, **Schemas**, or
   **Sign-in** for work without a tab, or **Updates** in the **Qrow** log.
   A long tab name ends in an ellipsis. Point to it to see the full name in a
   tooltip. The tooltip shows only when the name is cut.
4. The message. Messages and sources use the interface font. Times use the
   Logs font.
5. The actions of the entry, at the right end of the row.

| Work | Entries |
| --- | --- |
| Schema refresh | The start, the session, each request with its duration and count, and the result. Errors are in full. |
| Keep-alive | The tab, the outcome, and the duration. A failed keep-alive tells that it closed the session. |
| Session | Opened, closed, and idle disconnect, for each tab of the connection. |
| Query | The tab submitted a query, then the query finished, failed, or was cancelled. |
| Updates | In the **Qrow** log: the result of each check, the download, and the installation. A failed automatic check is not an error, so a computer that is offline does not show an unread error every hour. A failed manual check, download, or installation is an error. |

Activity does not show SQL text or query errors. Point to the entry of a tab,
or move the keyboard focus to it, to show the **Show tab** arrow. Click it to
see the tab of the query and its Logs, which have the details. An entry of a
closed tab stays, without **Show tab**. The copied text of an entry of a tab
starts with the tab name, for example `Query 3: Submitted a query`.

A shared catalog refresh goes to the Activity of the connection that ran it.

Select **Errors only** to show only errors. **All activity** shows all
entries. The selected filter is filled. The toolbar shows the number of
entries in a chip, and **Copy all activity** and **Clear activity** as icon
buttons with tooltips. **Copy all activity** copies the entries that show.
**Copy error** on an error row copies that error. Both actions include the
recorded date and time and the full text of each entry. **Clear activity**
deletes the log of the connection.

## Unseen errors

The connection menu shows each connection name and its unread error count as
separate labels.

The Activity icon keeps its shape. Its dot is at the top right corner of the
button. A red dot means that a connection has an unseen refresh error or a
tab has an unread error. The tooltip and the
accessible name give the count. A refresh with several failed requests counts
once. Each tab with an unread query or keep-alive error counts once.

When the Activity of a connection shows, its refresh errors become seen.
An unread tab error stays until the Logs of that tab show. Use **Show tab**
to open the tab. The Activity entry keeps its error color after the error
becomes seen.

A blue dot means that at least one connection shows **In use**. This includes
query or session work, schema refreshes, and assistant turns. An unread error
takes priority over work. The tooltip shows
**Activity** and its keyboard shortcut on the first line.
The name and shortcut share a vertical center.
The status below it shows **In use** while a connection shows that status.
Otherwise, the status shows **Idle**. Unread errors add their count and replace
**Idle**. These lines use secondary text, also when the dot is red. Activity has no
green completion dot.

## Limits

Qrow keeps Activity in memory. Quitting Qrow deletes it. Each connection keeps
up to 50,000 entries and 8 MiB of text. When a log is larger, Qrow removes the
oldest entries and adds an `Older entries were removed` line at the start.
Deleting a connection deletes its Activity.

The [assistant](assistant.md) cannot read Activity.

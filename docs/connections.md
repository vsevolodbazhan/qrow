# Connections

A connection profile stores the settings for a Kyuubi endpoint. Each connection
owns one or more query tabs. Select a profile in the Connections sidebar to show
its tabs. Qrow restores the last tab selected for that connection.

Each tab has its own session. Switching connections keeps sessions, SQL, results,
and Logs history in hidden tabs. A hidden tab can continue to run a query.

## Create a profile

1. Click **+** beside Connections.
2. Enter a name for the profile.
3. Enter the Kyuubi host and port.
4. Enter your LDAP username and password.
5. Enter the initial database.
6. Enter session parameters as a JSON object with string values.
7. Optional: To [browse the schemas](#browse-schemas) of the connection, set
   **Schema refresh** to **Manual** or **While connected**. Then you can enter
   [schema patterns](#show-or-hide-schemas).
8. Click **Save**.

Connection names must be unique. If the form contains an error, Qrow keeps the
form open and shows the error above the form actions.

For example, a session parameter can select an engine-sharing subdomain:

```json
{"kyuubi.engine.share.level.subdomain": "your-subdomain"}
```

macOS can request Keychain access when you save the password.

## Edit, duplicate, or delete a profile

Right-click a profile and use the **Connection** section of the menu: **Edit**,
**Duplicate**, or **Delete**.
An empty password field during an edit keeps the stored password. A duplicate
has a new profile identifier, a unique name based on the source name, and
requires a password.

Saving an edit keeps live sessions that use the profile when you change only the
name, the Connection Lifecycle fields, or the Schemas fields. The worker applies the new lifecycle
policy after active query, fetch, or keep-alive work finishes. The idle timer
and the keep-alive interval of these sessions then start again from the policy
update.

Changing the host, port, username, database, session parameters, or password
releases the matching sessions. Their SQL and downloaded results remain
available. The next Run opens a session with the updated connection settings.
Editing and deletion are disabled while a session using the profile is busy.
Deletion requires confirmation. Qrow closes sessions that use the deleted
profile, even when another profile is selected. Qrow removes the profile and requests deletion
of its stored password. Keychain deletion failures are not reported in the UI,
so a failed deletion can leave the password in Keychain.

## Browse schemas

The Connections sidebar is a tree. Each connection is a root row. Under a
connection that browses schemas, the tree shows its schemas, then the tables
and views of each schema, then the columns of each table or view. A column
row shows the column name and its type. A schema row shows the number of its
tables and views.

Schema browsing is off for a new connection. To turn it on, open the
connection settings and set **Schema refresh**, the first field of the
Schemas section:

- **Disabled**: Qrow does not read or show the schemas of the connection. The
  connection row has no arrow, and its menu has no **Schemas** section. The
  other Schemas fields do not show, and they keep their values. Qrow keeps
  the copy of the schemas on your computer, so the tree shows it again when
  you turn browsing on.
- **Manual**: Qrow reads the schemas when you select **Refresh**, or when you
  expand an unread row while a tab of the connection is connected.
- **While connected**: as **Manual**, and Qrow also
  [refreshes the schemas automatically](#refresh-schemas-automatically).

To use the tree:

- Click a connection to select it. This does not expand the connection.
- Click the arrow before a connection to expand or collapse it.
- Click a schema, a table, or a view to expand or collapse it.
- To collapse all rows below a connection or a schema, right-click it and
  select **Collapse**. The connection or the schema stays expanded. During
  a search, **Collapse** on a connection also collapses the schemas that
  the search expanded.
- After you click an arrow or a row below a connection, use the arrow keys to
  move through the tree, expand rows, and collapse rows. A click on a
  connection puts the cursor in the SQL editor. The tree shows the selected
  row only while it has the focus.
- Point to a row to see its full name when the sidebar cuts it. A tooltip
  also shows the comment of a table, a view, or a column, and the first line
  of the error of a failed refresh. A row whose name fits and that has no comment or error
  shows no tooltip. The tree shows no tooltips while a context menu is open.

Qrow keeps a copy of the schemas of each connection on your computer. The tree
shows this copy, also when the connection has no session. Qrow reads the copy
when you first expand the connection, search the tree, or connect a tab of
the connection. Qrow does not read the copy or open a session at startup.

### Refresh schemas

A refresh reads the schemas from the server again. Qrow opens a separate
session for the refresh and closes it when the refresh ends. This session does
not change the idle timer, the keep-alive, or the results of a tab.

- To refresh all schemas, with the tables and the columns of each schema,
  right-click the connection and select **Refresh** in the **Schemas**
  section.
- To refresh one schema and the columns of all its tables, right-click the
  schema and select **Refresh**.
- To refresh one table or view and its columns, right-click it and select
  **Refresh**.
- To stop a connection refresh, right-click the connection and select **Stop
  Refresh**. The tree keeps the schemas that it had before the refresh.

A connection refresh reads one schema at a time: first its tables, then their
columns. It starts with the schemas that Qrow read longest ago. The progress
row shows the number of schemas that are done. A connection with many schemas
or tables can need many minutes. To make it faster, hide the schemas that you
do not use.

Each refresh stops when it takes longer than **Refresh timeout** in the
connection settings. The default is 30 minutes. The tree keeps what the
refresh read before it stopped. The error **Refresh stopped after 30
minutes** shows like other [refresh errors](#refresh-errors).

When a connection refresh stops before its end, for example at its timeout,
when you stop it, or when the session fails, the next connection refresh in
the **Refresh period** continues it. It does not read again the schemas that
the stopped refresh read, and its progress starts at their number. After the
refresh period, the next connection refresh reads all schemas again. With
**Manual** refresh, the period is the last saved **Refresh period**, 60
minutes by default.

Qrow does not start a session to read schemas when no tab of the connection
has a live session. When a tab of the connection has a live session, Qrow
reads the missing data when you expand a row:

- A connection that Qrow never read.
- A schema without its list of tables.
- A table or a view without its columns.

When the connection has no live session, an unread row shows **Not loaded**.
Click **Refresh** in that row to read it. This opens a session.

#### Refresh errors

If a refresh fails or stops at its timeout, the tree keeps the data that it
had before the refresh. The error shows on the row of the refreshed part:

- A connection: the connection row shows a warning icon, also when the
  connection is collapsed. Point to the row to read the first line of the
  error below the host and the user. The first row under the expanded
  connection shows the error with **Refresh**. The icon goes away when the
  next connection refresh starts.
- A schema, a table, or a view: the row shows a warning icon. Point to the row
  to read the first line of the error. The first row under the expanded row
  shows the error with **Refresh**.

The Logs of each tab of the connection show the full error. The connection
row uses the same warning icon for an unread query error. Only a refresh error
adds text to the tooltip of the row. If Qrow cannot connect, it stops the
refreshes that wait for that connection.

### Refresh schemas automatically

When **Schema refresh** is **While connected**, Qrow refreshes the schemas of
the connection after each refresh period, but only while a tab of the
connection has a live session. The refresh then uses
the engine that the tab already started. Qrow never opens a session for an
automatic refresh on a connection without one.

- When a tab of a connection gets a live session and the last connection
  refresh is older than the period, Qrow refreshes the connection at once. A
  connection that Qrow never read is always older than the period. Thus, the
  first query of a connection fills its tree.
- While a tab of the connection stays connected, Qrow refreshes the connection
  each time the period passes. The period starts at the start of the last
  connection refresh, also a manual refresh, or a refresh that failed or
  stopped.
- When no tab of the connection has a live session any more, Qrow stops an
  automatic refresh in progress. The tree keeps what the refresh read.

Set these fields in the Schemas section of the connection settings:

- **Refresh period**: the minutes between automatic refreshes, from 5 to
  10,080 (7 days). The default is 60. This field shows only for **While
  connected**.
- **Refresh timeout**: the longest time of one refresh, manual or automatic,
  from 1 to 1,440 minutes (1 day). The default is 30.

### Search the tree

Type in **Search tables…** to find schemas and tables in all connections. The
search does not find columns. Qrow expands the connections and schemas that
contain matches. A schema that matches by name shows all its tables when you
expand it. The search shows the first 500 matches. Clear the search field to
show the full tree again.

### Use names in SQL

Right-click a schema, a table, a view, or a column, then select:

- **Copy Name** or **Copy Qualified Name** to copy the name. A table name
  includes its schema, for example `` `sales`.`orders` ``.
- **Insert into Editor** to put the name at the cursor of the SQL editor. The
  name replaces the selected text.

A double-click does not insert a name.

To use the keyboard, select a row in the tree, then:

- Press **⌘C** to copy its name.
- Press **Shift-Enter** to insert its name into the SQL editor.

Qrow puts backticks around each name that it inserts into SQL. This also
lets you use names that are SQL keywords. A backtick in a name becomes two
backticks.

### Show refreshes in Logs

To see the requests of each refresh, open the connection settings and set
**Schema refresh logs** to **Enabled**. The option applies to the connection. Qrow then
records each refresh in the Logs of each tab of the connection:

- The start of the refresh. An automatic refresh starts with **Started an
  automatic schema refresh**.
- The session that the refresh opens.
- Each request, with its duration and the number of schemas, relations, or
  columns that it returned. For example, `List columns of all relations in
  sales: 769 columns` is one request for the columns of all tables and views
  in the schema `sales`.
- The result of the refresh: completed, cancelled, stopped, or failed. If some
  schemas or tables could not be read, the result tells how many.

When the option is **Disabled**, Logs show only the errors of a refresh: each
failed request and the result of a failed refresh, with the full error.

The entries of one refresh share one place in the Logs history. A large
refresh does not remove the history of queries. A refresh error does not mark
the connection with an unread error, because the tree shows the error. The
default is **Disabled**.

### Show or hide schemas

Use **Show schemas** and **Hide schemas** in the connection settings to select
which schemas the tree shows. Each field takes glob patterns separated by
commas. `*` matches any text, and `?` matches one character. Letter case does
not matter.

- If **Show schemas** is empty, the tree shows all schemas.
- **Hide schemas** hides a schema also when **Show schemas** matches it.

For example, `sales_*, ops` in **Show schemas** and `*_tmp` in **Hide
schemas** show `sales_eu` and `ops`, but not `sales_tmp`.

When you save new patterns, the tree hides schemas at once. A connection
refresh skips hidden schemas that it has not started to read. A schema that you
add to **Show schemas** shows after the next connection refresh. A connection
with many schemas refreshes faster when you hide the schemas that you do not
use.

### Schema limitations

- With the Kyuubi share level `CONNECTION`, each refresh session starts its own
  Spark engine, also for an automatic refresh. The default share level,
  `USER`, uses the engine of your other sessions.
- The tree does not show temporary views, because they belong to the session
  of a tab.
- The tree does not show which columns are partition columns. HiveServer2 does
  not report this.
- Qrow does not save which rows are expanded.
- Qrow cannot stop a refresh while it opens its session or sends a request to
  the server. Each of these steps can take up to 2 minutes when the server
  does not answer. Thus, a refresh can take longer than its timeout.
- One catalog request can return at most 200,000 rows or 64 MB. If a schema
  has more columns, its refresh fails. Hide schemas or refresh single tables.

## Sessions and idle behavior

Each tab has one session for its owning profile. Selecting a different profile
changes the visible tab set. It does not stop work in hidden tabs. Each tab keeps
its result cursor and connection status while hidden.

Tabs can execute SQL concurrently, including when they use the same profile.
Separate sessions do not necessarily use separate Spark engines. Engine sharing
depends on the Kyuubi configuration.

Use the **When idle** picker to control each session:

| Choice | Behavior |
| --- | --- |
| **Disconnect after** | Releases the session after the specified idle time. The default is 900 seconds. Running work is not interrupted. |
| **Keep connected** | Sends a keep-alive query after each interval without other session work. The form suggests 300 seconds and `SELECT 1`. Both values can be changed. This mode is off by default. |

The idle timer and the keep-alive interval start again when a query, a fetch
of more rows from the server, a policy update, or a keep-alive finishes.
Paging through downloaded rows and editing SQL do not reset them.

Use a lightweight, read-only statement for keep-alive query. Qrow checks that the
text contains one statement, but does not enforce read-only behavior. Keep-alives
use the existing session and preserve its result cursor. A failed keep-alive
disconnects the session and stops background queries until the next explicit Run.

To release the active tab's session, select its connection and click **Disconnect**.
This action is disabled while a query or keep-alive runs. It is also disabled
when the active tab has no live session for its owning connection.

Manual and idle disconnection preserve SQL and downloaded results. Unfetched
rows and session state, such as temporary views or settings applied with SQL,
are lost.

Qrow's session idle timeout is separate from the server's engine idle timeout.
Disconnecting a Qrow tab does not guarantee that its Spark engine stops. Other
tabs, clients, and server policies can keep the engine active.

## Manage tabs under a connection

Each connection always has at least one tab. Creating a connection creates a
blank tab. Closing or moving the last tab creates a blank replacement for the
source connection.

Right-click a tab to choose **Rename…**, **Duplicate**, **Copy to
Connection…**, or **Move to Connection…**. When the assistant is on, the menu
also has [**Start Conversation**](assistant.md#conversations-and-query-tabs). Duplicate copies the tab within its
connection. Copy and move show a submenu of destination connections. Choosing
a destination selects the resulting tab. Move is disabled while the tab is
busy or while its assistant conversation works or waits for approval. A move
keeps the [assistant conversation](assistant.md#conversations-and-query-tabs)
of the tab. Tab names must be unique within a connection. A duplicate or copied tab
uses the source name with **(Copy)**; later copies add a number when needed. A
move keeps the source name when it is available and adds a copy suffix if the
destination already uses that name.

These actions copy only the current SQL text and tab name. Duplicate and copy
update the name as described above. They do not copy
results, Logs history, session state, or execution status. They do not run SQL.

Deleting a connection requires confirmation. The confirmation states that all
owned tabs and SQL will be deleted. Deletion is disabled while any owned tab is
busy.

## Authentication and connection failures

Qrow supports HiveServer2 over TCP with SASL PLAIN authentication for LDAP. SASL PLAIN does not encrypt the
transport. Use a trusted network or VPN. TLS, Kerberos, HTTP transport, and SSH
tunneling are not implemented.

Passwords are stored in macOS Keychain. They are not part of the
[workspace file](workspace.md#saved-state). If Qrow cannot read a password,
edit the connection to save a password again.

Each attempt to connect to one address of the host has a 10-second timeout.
If the host has more than one address, Qrow tries the next address after a
failure. Each network write has a 15-second timeout. For the read timeout, see
[Cancel work](queries.md#cancel-work).

Qrow discards a failed connection and reports the error. This includes recognized
Kyuubi errors that wrap an engine transport failure. The next explicit Run can
reconnect. Qrow never automatically resubmits failed SQL. An ordinary SQL error
can leave a healthy session available for the next query.

## Design

The [worker](../src/worker.rs) owns session work for each tab. Credential access
and network calls run on background threads. The
[HiveServer2 connector](../src/connector/hive.rs) sends profile parameters when
opening a session, then selects the initial database.

[Idle maintenance](../src/worker/lifecycle.rs) waits for a command or the next
session deadline. A lifecycle update wakes the worker so it can recalculate the
next deadline. It does not require continuous UI polling. The keep-alive uses a
separate operation in the same session so it can preserve the result cursor.

The [catalog worker](../src/catalog/worker.rs) of each connection owns its
schema copy. It loads the copy from disk, runs the queued refreshes in one
session, and saves the copy after each refresh. Qrow sends names to the server
as given. HiveServer2 reads `_` in a name as a pattern, so the worker removes
the rows of other names. Refresh requests that wait are merged: a schema
refresh includes the refreshes of its tables.

[Credential storage](../src/storage.rs) uses Keychain service
`io.qrow.connection`, keyed by profile UUID. Keeping that identifier stable
preserves access to existing passwords.

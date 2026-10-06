# Connections

A connection profile stores the settings for a Kyuubi endpoint. Each connection
owns one or more query tabs. Select a profile in the Connections sidebar to show
its tabs. Qrow restores the last tab selected for that connection. To show the
Connections sidebar, click the plug button at the left end of the status
bar, press **⌘B**, or select **View → Connections**.

Each tab has its own session. Switching connections keeps sessions, SQL, results,
and Logs history in hidden tabs. A hidden tab can continue to run a query.

## Connection state

Each connection row has one status dot. Dim blue means that at least one query
tab has a live idle session. Faint blue at 20% opacity means that a query tab
is connecting. Blue means that query, assistant, or schema work
continues. Green means that a result or reply is unread. Red means that an
error is unread. Yellow means that an assistant query needs approval.

The priority is approval, error, running work, connecting, unread success,
then connected and idle.
The tooltip shows the connection name and a short status beside it. The status
uses secondary text. The name and status share a vertical center. The status
stays beside the first line when the name wraps.
Host and User show on compact labeled rows below the name. **Unread Error**
identifies an unread error. No dot means that no session, work, or
unread outcome needs an indicator. A schema refresh uses its own temporary
session. That session does not give the connection a dim blue dot.

Click the dot to open [Activity](activity.md). Viewing Activity reads refresh
errors. It does not read query results or tab errors. Show the Results or Logs
of the affected tab to read those outcomes. Selecting a connection shows its
last selected tab. The other tabs keep their unread outcomes.

## Create a profile

1. Click **+** beside Connections.
2. Enter a name for the profile.
3. Enter the Kyuubi host and port.
4. Turn on **TLS** if the server accepts TLS on this port.
5. Select the authentication:
   - **Password**: enter your LDAP username and password.
   - **Sign-in (OpenID Connect)**: select a
     [sign-in](#sign-in-with-openid-connect) and enter the database username.
6. Enter the initial database.
7. Enter session parameters as a JSON object with string values.
8. Optional: To [browse the schemas](#browse-schemas) of the connection, set
   **Schema refresh** to **Manual** or **While connected**. Then you can enter
   [schema patterns](#show-or-hide-schemas).
9. Optional: When the [assistant](assistant.md) is on, enter **Assistant
   notes**. See [Give the assistant facts about a
   connection](#give-the-assistant-facts-about-a-connection).
10. Click **Save**.

Connection names must be unique. If the form contains an error, Qrow keeps the
form open and shows the error above the form actions.

For example, a session parameter can select an engine-sharing subdomain:

```json
{"kyuubi.engine.share.level.subdomain": "your-subdomain"}
```

macOS can request Keychain access when you save the password.

## Sign in with OpenID Connect

A sign-in lets connections authenticate with an access token from an OpenID
Connect provider, for example a Keycloak realm. You sign in once in the
browser. Several connections can use the same sign-in. Each connection keeps
its own host, database username, and session settings. Kyuubi checks that the
signed-in identity can use the database username of the connection.

The **Sign-ins** sidebar shows each sign-in with its account. To open it,
click the key button at the left end of the status bar, or select
**View → Sign-ins**. Click the button again to hide the sidebar. The button
shows a yellow dot when sign-ins need attention: a connection uses a sign-in
that is not signed in, a sign-in has expired, Qrow cannot reach the provider,
or the last sign-in action failed. Point to the button to read how many
sign-ins need attention.

### Add a sign-in

1. Open the **Sign-ins** sidebar and click **+**. In Connection Settings, you
   can also click **New Sign-in…** at the bottom of the **Sign-in** list. The
   connection then uses the new sign-in.
2. Enter a name, the issuer URL, and the client ID.
3. Enter other scopes if the server requires them. Qrow always requests
   `openid`, `profile`, and `email`.
4. Enter the database hosts that can receive the access tokens.
5. Click **Save**, or press **Enter** or **⌘Enter**.

The provider must register Qrow as a public client without a client secret.
The client must accept the redirect URI `http://127.0.0.1:PORT/callback`. By
default, Qrow uses an available port. If the provider accepts only some ports,
enter them in **Callback Ports**. Qrow tries them in order and uses the first
port that no other program uses. The provider must support the
authorization-code flow with PKCE (`S256`). Add the `offline_access` scope if
the provider issues refresh tokens only with it. Use **Resource** only if the
provider requires an RFC 8707 resource indicator.

### Share a sign-in

To give a sign-in to a colleague, for example in a Slack message:

1. Right-click the sign-in and select **Copy Settings**.
2. Paste the text into the message.

The text contains the name, the issuer, the client ID, the scopes, the
resource, the database hosts, and the callback ports. It does not contain
your account or your tokens.

To add a sign-in that a colleague sent to you:

1. Copy the text of the message.
2. Open the **Sign-ins** sidebar and click the paste button beside **+**.
   When the sidebar is empty, click **Paste Sign-in…**.
3. Sign-in Settings opens with the values. Make sure that you trust the
   issuer and the database hosts. Qrow sends your access tokens to these
   hosts.
4. Click **Save**.

Qrow ignores other text of the message around the settings. If a sign-in
with the same name exists, Qrow adds `copy` to the name. If the clipboard
does not contain sign-in settings, Qrow tells you and adds nothing. Each
user signs in with their own account. Each connection keeps its own
database username, so a colleague must add their own connections.

### Sign in and use a sign-in

Click **Sign In…** on the row of the sign-in. Qrow opens the default browser.
Finish the sign-in there. Qrow waits up to five minutes. Click **Cancel** to
stop. The row then shows the email or name of the account.

Click a row to open **Sign-in Settings**. The **Account** field shows the
status and the actions for the account on the same row. The actions are on
the right. Right-click a row for **Sign In…**,
**Cancel Sign-in**, **Sign Out**, or **Retry**, and for **Edit**,
**Copy Settings**, and **Delete**.

The **Connections** field shows the number of connections that use the
sign-in. Neutral tags show the names below the heading. The tags wrap when
they do not fit on one line.

To use a sign-in, edit a connection, select **Sign-in (OpenID Connect)** in
**Authentication**, and select the sign-in in the **Sign-in** list. The host of the connection must be
one of the database hosts of the sign-in. Qrow does not send a token to
another host. Turn on **TLS** when the server accepts it. Without TLS, anyone
on the network path can read the access token and use it until it expires, so
use a trusted network or VPN. Qrow never sends the refresh token to Kyuubi.

| State | Recovery |
| --- | --- |
| **Not signed in** | Click **Sign In…**. |
| **Waiting for the browser…** | Finish the sign-in in the browser, or click **Cancel**. |
| The account email or name | No action. |
| **Expired · Sign in again** | Click **Sign In…**. |
| **Cannot reach the provider** | Select **Retry**. Qrow keeps the account. |
| **The last action failed** | Open Sign-in Settings to read the error, then try again. |

When you run a query and its connection needs a new sign-in, Qrow opens the
browser sign-in. This occurs when the sign-in is not signed in, when it has
expired, or when the provider does not accept the refresh token. The status
of the tab is **Waiting for sign-in**, and Logs names the sign-in. After the sign-in,
the query runs. The query did not reach the server before, so this is not a
second run. If the sign-in fails, the query ends with
**Error: Sign-in required**. If you cancel it in the browser, the query ends
with **Cancelled: Sign-in not finished**. Click **Cancel** to stop the query
while it waits. The browser sign-in then continues in the Sign-ins sidebar.
Qrow opens the browser only once for each query. An open session continues
without a new token, so a query in it does not open the browser. While a
browser sign-in or a sign-out of the sign-in runs, each new query of its
connections waits, also in an open session, because the account can change.
After a change of account, Qrow closes the sessions of the old account before
their next query.

If Kyuubi does not accept the token, the error names the database username. SASL PLAIN does not tell why the server
rejected a token, so Qrow cannot show if the token expired or if the account
is not available to the identity.

To edit a sign-in, click its row. While you are signed in, you cannot change
the issuer, client ID, scopes, or resource. Sign out first.

### Tokens and sessions

Qrow keeps the access token and the refresh token in macOS Keychain. Tokens
are not in the workspace file, Logs, error messages, or assistant context.
Before Qrow opens a transport, it gets an access token that stays valid for
at least 60 seconds. If necessary, it uses the refresh token. When the provider
replaces the refresh token, Qrow saves the new token in Keychain before it uses
the new access token. All tabs of a Qrow process share one refresh. If the
provider gives no refresh token, a new browser sign-in is necessary when the
access token expires.

An open session continues after its access token expires, if the server
permits it. Cancellation opens a second transport, so it gets a new access
token for the account that opened the session.

**Sign out** deletes the tokens from Keychain. It does not end the session at
the provider, and it does not revoke the tokens. Sign out is disabled while a
query that uses the sign-in runs. Sign out releases the idle sessions of the
sign-in. Their SQL and downloaded results remain available. A sign-in as
another account also releases these sessions. If Qrow cannot delete the
tokens, it shows the error and the sign-in stays signed in.

**Delete** is available only when no connection uses the sign-in. The
**Connections** field of Sign-in Settings names the connections that use it.
When **Delete** is disabled, hover over the command to see the reason in a tooltip.
Change authentication in these connections before you delete the sign-in.
Qrow asks before it deletes the sign-in. Delete also deletes the tokens of the
sign-in.

### Server requirements

Kyuubi must accept the access token in the SASL PLAIN password field of the
binary transport. Kyuubi does not do this by default. A custom
`PasswdAuthenticationProvider` must validate the token and check that the
identity can use the database username. The HTTP bearer authenticator of
Kyuubi does not apply to the binary transport. To protect the token, the port
must accept TLS, for example through the TLS settings of the Kyuubi binary
frontend or a TLS proxy.

Limitations:

- Each sign-in has one set of scopes and one resource indicator. Qrow does not
  examine the access token. The server must check its audience and scopes.
- Only one Qrow process coordinates the refresh of a sign-in. `qrow-probe`
  supports only connections with password authentication.

## Give the assistant facts about a connection

Use **Assistant Notes** to tell the assistant facts about the data of a
connection, for example "Dates in `avia` are in UTC" or "`bookings.status = 3`
means cancelled". The field is at the end of
Connection Settings. It shows only while the assistant is on.

The notes can have up to 16 KB. Near the limit, the help text below the field
shows the number of bytes. Qrow saves the notes as plain text in the
workspace and sends them to Codex. Do not put passwords, tokens, or other
secrets in them. [Assistant](assistant.md#connection-notes) tells when Qrow
sends the notes.

A duplicate keeps the notes of the source.

## Edit, duplicate, or delete a profile

Right-click a profile and use the **Connection** section of the menu: **Edit**,
**Duplicate**, or **Delete**.
An empty password field during an edit keeps the stored password. When you
change a connection from a sign-in to a password, enter the password. A
duplicate has a new profile identifier and a unique name based on the source
name. A duplicate with password authentication requires a password. A
duplicate keeps the sign-in and the assistant notes of the source.

Saving an edit keeps live sessions that use the profile when you change only the
name, the Connection Lifecycle fields, the Schemas fields, or the
assistant notes. The worker applies the new lifecycle
policy after active query, fetch, or keep-alive work finishes. The idle timer
and the keep-alive interval of these sessions then start again from the policy
update.

Changing the host, port, username, database, session parameters, TLS,
authentication, sign-in, or password releases the matching sessions. Their SQL and downloaded results remain
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
row shows a minus icon, the column name, and its type. A schema row shows
the number of its tables and views.

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
  The current query's connection stays highlighted.
- Click the arrow before a connection to expand or collapse it.
  This does not select the connection for the query. An outline shows
  the keyboard position without a second connection highlight.
- Click a schema, a table, or a view to expand or collapse it.
- To collapse all rows below a connection or a schema, right-click it and
  select **Collapse**. The connection or the schema stays expanded. During
  a search, **Collapse** on a connection also collapses the schemas that
  the search expanded.
- After you click an arrow or a row below a connection, use the arrow keys to
  move through the tree, expand rows, and collapse rows. A click on a
  connection puts the cursor in the SQL editor. The tree highlights a selected
  schema, table, view, or column only while the tree has the focus.
  If you collapse a connection, the previous row highlight disappears.
  If you hide the Connections panel, the tree clears its row selection.
- Point to a row to see its full name when the sidebar cuts it. A tooltip
  also shows the comment of a table, a view, or a column, and the first line
  of the error of a failed refresh. A row whose name fits and that has no comment or error
  shows no tooltip. The tree shows no tooltips while a context menu is open.

Qrow keeps a copy of the schemas of each connection on your computer. The tree
shows this copy, also when the connection has no session. Connections that
read the same metastore can [share one copy](#share-schemas). The [assistant](assistant.md#look-up-tables-and-columns)
can read this copy to find table and column names. Qrow reads the copy
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
columns. By default, **Column reads** is **One relation at a time**. Qrow reads
the columns of each table or view in a separate request. Each request ends
before the next starts. This reduces the column result that the Spark driver
holds in memory.

For fewer requests, set **Column reads** to **Whole schema** in the
connection's **Schemas** section. This reads all columns of a schema in one
request. Large schemas can exhaust the memory of a small driver. If a request
fails and the session stays available, Qrow reads each table separately.
Per-relation reads can take longer because they send more requests. The same
**Refresh timeout** applies to both choices. Connections saved before this
setting was added also use **One relation at a time**.

A connection refresh starts with the schemas that Qrow read longest ago.
The progress row shows the number of schemas that are done. A connection
with many schemas or tables can need many minutes. To make it faster, hide
the schemas that you do not use.

Each refresh stops when it takes longer than **Refresh timeout** in the
connection settings. The default is 30 minutes. The tree keeps what the
refresh read before it stopped. The error **Refresh stopped after 30
minutes** shows like other [refresh errors](#refresh-errors).

When a connection refresh stops before its end, for example at its timeout,
when you stop it, or when the session fails, the next connection refresh in
the **Refresh period** continues it. It does not read again the schemas that
the stopped refresh read without an error, and its progress starts at their
number. A schema with a failed read is read again. A schema that you hide
and show again is read again because its cached data was removed. After the
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

- A connection: the connection row shows a red dot, also when the
  connection is collapsed. Point to the row to see **Unread Error** beside
  the connection name. The first row under the expanded
  connection shows the error with **Refresh**. The dot goes away when you view
  the Activity of the connection. The error text stays until the next refresh
  starts.
- A schema, a table, or a view: the row shows a warning icon. Point to the row
  to read the first line of the error. The first row under the expanded row
  shows the error with **Refresh**.

[Activity](activity.md) shows the full error. Click the status dot or select
**Show Activity** from the connection menu to open it. The connection row uses
the same red dot for an unread query error. If Qrow cannot connect, it stops the
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
- If the last connection refresh stopped before its end, Qrow starts a
  refresh when the connection gets a live session again. This also applies
  after an application restart. Within the refresh period, the refresh
  continues the unfinished schemas. After the period, it reads all schemas.
- While a tab of the connection stays connected, Qrow refreshes the connection
  each time the period passes. The period starts at the start of the last
  connection refresh, also a manual refresh, or a refresh that failed or
  stopped.
- When no tab of the connection has a live session any more, Qrow stops an
  automatic refresh in progress. The tree keeps what the refresh read.
  An idle disconnection waits for an active refresh to finish. An explicit
  disconnection does not wait.

For a [shared catalog](#share-schemas), the period and the timeout belong to
the shared catalog. An unfinished refresh starts again when the first
member connects after all members disconnected. Another member that
connects while the catalog stays connected does not start a new attempt.

Set these fields in the Schemas section of the connection settings:

- **Refresh period**: the minutes between automatic refreshes, from 5 to
  10,080 (7 days). The default is 60. This field shows only for **While
  connected**.
- **Refresh timeout**: the longest time of one refresh, manual or automatic,
  from 1 to 1,440 minutes (1 day). The default is 30.

### Share schemas

Connections that read the same metastore, for example through different
users or Spark clusters, can share one schema catalog. Qrow then keeps one
copy of the schemas for all of them, and a refresh of one connection fills the
tree of each connection.

To share a catalog:

1. Open the settings of a connection that browses schemas.
2. Open the **Schema catalog** list, then select **New shared catalog…** below
   the list. The connection brings its copy of the schemas to the new
   catalog before an automatic refresh can start.
3. Enter a **Shared catalog name**, then select **Save**.
4. Open the settings of each other connection, select the shared catalog in
   **Schema catalog**, then select **Save**.

The connections of a shared catalog must read the same metastore with the
same permissions and the same Spark catalog. Qrow cannot check this.

For a connection that uses a shared catalog:

- **Schema refresh**, **Refresh period**, **Show schemas**, **Hide schemas**,
  and **Refresh timeout** belong to the shared catalog. A change in the
  settings of one connection applies to all connections of the catalog. When
  you select a catalog in **Schema catalog**, the fields show its settings.
- **Schema refresh** set to **Disabled** turns schema browsing off only for
  this connection. The connection stays in the shared catalog.
- **Preferred connection** selects the connection that automatic refreshes
  use while one of its tabs has a live session. Otherwise, Qrow uses the
  first connection in the sidebar that has a live session. The default is
  **Any connected connection**.
- **Column reads** belongs to each connection. A refresh uses the choice of
  the connection that runs it. Different clusters can use different choices.

Each refresh uses the session, the user, and the cluster of the connection
that asked for it. Qrow runs one refresh of a shared catalog at a time. A
refresh that a running or waiting refresh includes does not wait again.
Only the connection that runs a refresh shows its progress and its errors.
This includes the errors of schemas, tables, and views. A connection whose
refresh waits shows **Waiting…**. **Stop Refresh** stops only the refreshes of its
connection.

To stop sharing, select **This connection** in **Schema catalog**. The
connection then starts with an empty copy of the schemas. When the last
connection leaves a shared catalog, or you delete it, Qrow deletes the shared
catalog and its copy of the schemas.

### Search the tree

Type in **Search tables…** to find schemas and tables in all connections. The
search also finds views. Use a name or part of a name. For a table or view,
you can include the schema, for example `integrations.bookings`. The search
also accepts names from **Copy Qualified Name**, for example
`` `integrations`.`bookings` ``. The search ignores letter case and spaces
at the start and end of the search text.

The search does not find columns. Qrow expands the connections and schemas
that contain matches. If you expand a connection with no matches, the tree
shows **No matches**. You can collapse and expand the connection again.
A schema that matches by name shows all its tables when you expand it. The
search shows the first 500 matches. If the limit hides all matches in a
connection, the tree shows **Search limit reached**. Clear the search field
to show the full tree again.

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

### See refresh details

[Activity](activity.md) records each refresh of the connection: its start,
the session that it opens, each request with its duration and count, and its
result. For example, `List columns of all relations in sales: 769 columns` is
one request for the columns of all tables and views in the schema `sales`. If
some schemas or tables could not be read, the result tells how many. For a
shared catalog, the entries go to the Activity of the connection that ran the
refresh.

Refreshes do not go to the Logs of the tabs. A failed refresh gives its
connection a red dot and counts as an unseen error in the status bar.

### Show or hide schemas

Use **Show schemas** and **Hide schemas** in the connection settings to select
which schemas the tree shows. For a [shared catalog](#share-schemas), the
patterns apply to all its connections. Each field takes glob patterns separated by
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
  the server. A wait for a server answer uses the connection's **Response
  timeout**. Thus, a refresh can take longer than its **Refresh timeout**.
- One catalog request can return at most 200,000 rows or 64 MB. If a schema
  exceeds this limit with **Whole schema**, use **One relation at a time**.
  The limit still applies to each table request. These limits apply after
  the server creates its result. They do not limit Spark driver memory.
- Per-relation reads reduce the column result size. The table list still reads a
  whole schema. Server patterns can also match extra names, for example when
  a name contains `_`. Thus, a refresh can still exhaust driver memory.
- The keyboard cannot reach **New shared catalog…** below the **Schema
  catalog** list. Use the pointer. In the list, **Enter** selects a catalog
  and **Escape** closes the list.

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

A schema refresh uses a separate session. While a refresh runs, the tabs of
the connection that runs it do not disconnect because of idle time. The
refresh does not reset their idle timers. If the idle time has passed, the
tabs disconnect after the refresh finishes, fails, or stops. The **Refresh
timeout** still applies. A later refresh does not extend an idle time that
has already passed. Other connections of a shared catalog keep their
own idle behavior.

Use a lightweight, read-only statement for keep-alive query. Qrow checks that the
text contains one statement, but does not enforce read-only behavior. Keep-alives
use the existing session and preserve its result cursor. A failed keep-alive
disconnects the session and stops background queries until the next explicit Run.
[Activity](activity.md) shows each keep-alive. The Logs of a tab show only a
failed keep-alive, with its SQL.

To release the active tab's session, select its connection and click **Disconnect**.
This action is disabled while a query or keep-alive runs. It is also disabled
when the active tab has no live session for its owning connection.

To release all tab sessions of a connection, right-click the connection and
select **Disconnect**. This command acts on that connection, also when another
connection is selected. It is disabled while one of its tab sessions is busy
or when none of its tabs has a live session. The next **Run** opens a new session.

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

Qrow supports HiveServer2 over TCP with SASL PLAIN authentication. The SASL
PLAIN password is an LDAP password or the access token of a
[sign-in](#sign-in-with-openid-connect). SASL PLAIN does not encrypt the
transport. Turn on **TLS** to encrypt it, or use a trusted network or VPN.
Qrow verifies the server
certificate and the host name against the macOS trust store. To trust a
company certificate authority, add it to Keychain and trust it. Kerberos, HTTP
transport, and SSH tunneling are not implemented.

Passwords are stored in macOS Keychain. They are not part of the
[workspace file](workspace.md#saved-state). If Qrow cannot read a password,
edit the connection to save a password again.

Each attempt to connect to one address of the host has a 10-second timeout.
If the host has more than one address, Qrow tries the next address after a
failure. Each network write has a 15-second timeout.

**Response timeout** sets how long Qrow waits for one answer from Kyuubi. The
default is 300 seconds, and the range is 10 to 3600 seconds. The first query
of a session can wait while Kyuubi starts an engine. Kyuubi limits that wait
with `kyuubi.session.engine.initialize.timeout`. Set **Response timeout**
higher than that limit, or a slow engine start fails with `Kyuubi did not
answer within 300 seconds`. A change applies to the sessions that open after
you save. The timeout does not limit the duration of a query, because Qrow
asks for the status of a running query again and again. See
[Cancel work](queries.md#cancel-work).

A new session opens and then selects the initial database. Kyuubi can hold
these steps while it starts a Spark engine. If Kyuubi does not answer within
the response timeout, the error names the step and tells that the engine can
still start. If the initial database does
not exist, the error names the database and includes the message of Spark.
The status of the tab is **Error: Connection failed** when a session could not
open, and **Error: Connection lost** when an open session failed.

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
preserves access to existing passwords. Sign-in tokens use the separate
service `io.qrow.sign-in`, keyed by sign-in UUID. Each record holds the
issuer, client, subject, and token requirements of its tokens.

The [sign-in service](../src/oidc/service.rs) is shared by all tabs. It runs
one refresh at a time for each sign-in, and discards a result that arrives
after a sign-out or removal. The [browser flow](../src/oidc/flow.rs) uses
discovery, PKCE with `S256`, a new `state` and `nonce` for each attempt, and a
callback listener bound to `127.0.0.1` that closes after the attempt. The
[ID token check](../src/oidc/jwt.rs) verifies the signature (RS256, PS256, or
ES256), issuer, audience, expiry, and nonce. The worker asks for credentials
before each new session.

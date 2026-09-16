# Usage

## Connect and query

1. Click **+** beside Connections to open connection settings.
2. Enter a name, Kyuubi host, port, LDAP username, password, and initial database.
3. Enter session parameters as a JSON object, for example:
   `{"kyuubi.engine.share.level.subdomain": "your-subdomain"}`.
4. Save the connection. macOS may ask for Keychain access.
5. Write SQL and click **Run** or press **⌘Enter**.

In connection settings, **⌘Enter** saves and **Escape** dismisses the popup.

Passwords are stored in macOS Keychain under service `io.qrow.connection`, keyed
by profile UUID. They are retrieved on a background thread when a tab connects,
and deleted on that thread when the connection is deleted.
The prototype supports SASL PLAIN over TCP for the existing LDAP deployment,
matching the provided PyHive configuration. SASL PLAIN does not encrypt the
transport. Use it on the same trusted network/VPN as that setup. TLS, Kerberos,
HTTP transport, and SSH tunneling are not implemented.

Select text to execute just that statement. Without a selection, the whole
editor is submitted. Multiple statements are rejected locally. No SQL is
automatically retried after a connection failure.

Use **⌘T** or **+** in the tab strip to open a tab. Each tab owns an independent
session and can run one query at a time. Different tabs can run concurrently.
The Cancel button sends a request over a separate authenticated transport so it
does not queue behind a blocked fetch. Kyuubi must accept the operation handle
on that transport; this requires live validation. Cancellation cannot roll back
a statement that has already completed. If a connection or initial session setup
is still in progress, cancellation prevents submission of the user's query once
setup returns. Individual network reads time out after 120 seconds. On exit,
the app requests cancellation and session cleanup, waiting up to one second
after saving the workspace. If the server is unreachable, cleanup is best effort
and server-side idle/session timeouts remain responsible for abandoned resources.

Switch profiles using the Connections sidebar. SQL stays in the tab, while the
old session and results are released. Switching and closing are disabled while
the tab is busy. Right-click a connection for **Edit connection…**, **Duplicate**,
and **Delete**; deleting asks for confirmation and also removes the stored
password. Editing and deleting are disabled while that profile has a running
query. Saving an edit disconnects idle sessions that use that profile; the next
query opens a new session.

Right-click a query tab for **Edit tab…**, which renames it. The current name is
the field's placeholder, so leaving the field blank keeps it. Renaming preserves
the tab's SQL, connection, and downloaded results.

Results stream in batches of up to 250 rows into a 1,000-row page. **Next**
fetches another page only when needed. **Previous** and **Next** reuse downloaded
pages without executing SQL again. Row numbers refer to the full result, and
copy actions use the full stored values on the displayed page. You can browse
downloaded pages while a fetch is running. New batches preserve the current
scroll position; changing pages clears selection and scrolls to the top.
The client does not add a SQL `LIMIT`. An empty fetch confirms exhaustion
because some servers misreport `hasMoreRows`. If Next finds no further rows,
the last populated page stays visible. Cancellation or a fetch failure retains
previously downloaded rows.
Preview storage is capped at 100,000 rows or approximately 64 MiB per tab; an
incoming batch that would exceed the cap is discarded and the cursor is closed.
Frame size is also capped at 64 MiB. The table virtualizes rows and columns.
Drag column boundaries to resize them. Drag the divider above Results to resize
the editor, and the sidebar divider to change its width. **⌘B** toggles the sidebar.

Decimals and textual timestamps retain their server representation. Binary
values display as hexadecimal. Nulls display as `NULL`; empty strings remain
empty. Nested values use the textual representation returned by HiveServer2.
Cells display a shortened preview, while right-click **Copy cell** or **Copy row**
copies the full stored value. File export is not included.

## Idle connections and reconnecting

Each connection has a **When idle** choice in its settings:

- **Disconnect after** releases each idle tab's session after the configured
  number of seconds. The default is 900 seconds, including for existing profiles.
  The timer starts after query or preview fetching finishes. Reading results or
  editing SQL does not reset it, and a running query is never interrupted by it.
- **Keep connected** replaces idle disconnection with periodic heartbeat SQL.
  The editor suggests a 300-second interval and `SELECT 1`. Both are configurable.
  Choose a lightweight, read-only statement. Heartbeats run only while the tab
  is idle, use the same session, and preserve the user's result cursor. They do
  not create new sessions or reconnect after failures. This mode is off by default.

**Disconnect** in the query toolbar releases the active tab's session. It is
unavailable while a query or heartbeat is running; Cancel remains available.
Manual and idle disconnection preserve SQL and downloaded results. They release
unfetched rows, temporary views, and session settings. The next explicit Run opens
and initializes a new session with the profile's configured database and parameters.

A dead connection, including a Kyuubi error wrapping an engine transport failure,
is discarded. Qrow reports the error and reconnects on the next explicit Run;
it never automatically resubmits the failed SQL. A failed heartbeat also disconnects
and stops background queries until the user runs a query again.

Qrow's idle timeout is separate from Kyuubi's engine idle timeout. Kyuubi's
[engine shutdown check](https://github.com/apache/kyuubi/blob/master/kyuubi-common/src/main/scala/org/apache/kyuubi/session/SessionManager.scala)
requires no active user sessions. Releasing Qrow's sessions allows that timeout
to take effect, but other clients or tabs can still hold the engine open. There
are no heartbeat requests when Keep connected is off.

## Appearance

Open **Qrow → Settings…** to choose the interface font, editor font and size,
or interface scale. The interface font applies to controls and query results;
the editor font applies to SQL. Editor font size is in pixels before scaling.
Use interface scale to resize text and controls throughout the app.
Changes appear immediately and are saved with the workspace. **Restore defaults**
resets all appearance settings, including the system interface font.

## Workspace

Tabs, SQL, selected profiles, and connection settings are saved automatically to:

```text
~/Library/Application Support/Qrow/workspace.json
```

Passwords and results are not written to this file. The workspace is written
atomically in the background after a short editing debounce and flushed on exit.
Tabs restore without opening connections. A corrupt or unsupported workspace is
left untouched and automatic saving is disabled for that run.

For isolated testing, set `QROW_DATA_DIR` to another directory. The demo uses an
in-memory workspace and never saves it.

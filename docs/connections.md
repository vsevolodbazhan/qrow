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
7. Click **Save**.

For example, a session parameter can select an engine-sharing subdomain:

```json
{"kyuubi.engine.share.level.subdomain": "your-subdomain"}
```

macOS can request Keychain access when you save the password.

## Edit, duplicate, or delete a profile

Right-click a profile to use **Edit Connection…**, **Duplicate**, or **Delete**.
An empty password field during an edit keeps the stored password. A duplicate
has a new profile identifier, a unique name based on the source name, and
requires a password.

Saving an edit keeps live sessions that use the profile when you change only the
name or the Connection Lifecycle fields. The worker applies the new lifecycle
policy after active query, fetch, or keep-alive work finishes. A shorter
keep-alive interval starts from the policy update. Switching to Disconnect
starts a new idle timeout from the policy update.

Changing the host, port, username, database, session parameters, or password
releases the matching sessions. Their SQL and downloaded results remain
available. The next Run opens a session with the updated connection settings.
Editing and deletion are disabled while a session using the profile is busy.
Deletion requires confirmation. Qrow closes sessions that use the deleted
profile, even when another profile is selected. Qrow removes the profile and requests deletion
of its stored password. Keychain deletion failures are not reported in the UI,
so a failed deletion can leave the password in Keychain.

## Sessions and idle behavior

Each tab has one session for its owning profile. Selecting a different profile
changes the visible tab set. It does not stop work in hidden tabs. Each tab keeps
its result cursor and connection status while hidden.

Tabs can execute SQL concurrently, including when they use the same profile.
Separate sessions do not necessarily use separate Spark engines. Engine sharing
depends on the Kyuubi configuration.

The **When Idle** setting controls each session:

| Choice | Behavior |
| --- | --- |
| **Disconnect after** | Releases the session after the specified idle time. The default is 900 seconds. Reading results and editing SQL do not reset the timer. Running work is not interrupted. |
| **Keep Connected** | Sends periodic keep-alive query while the session is idle. The form suggests 300 seconds and `SELECT 1`. Both values can be changed. This mode is off by default. |

Use a lightweight, read-only statement for keep-alive query. Qrow checks that the
text contains one statement, but does not enforce read-only behavior. keep-alives
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

Right-click a tab to choose **Edit Tab…**, **Duplicate**, **Copy to
Connection…**, or **Move to Connection…**. Duplicate copies the tab within its
connection. Copy and move show a submenu of destination connections. Choosing
a destination selects the resulting tab. Move is disabled while the tab is
busy. Tab names must be unique within a connection. A duplicate or copied tab
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

[Credential storage](../src/storage.rs) uses Keychain service
`io.qrow.connection`, keyed by profile UUID. Keeping that identifier stable
preserves access to existing passwords.

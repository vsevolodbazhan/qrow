# Connections

A connection profile stores the settings for a Kyuubi endpoint. Select a profile
in the Connections sidebar to use it for the next query in the active query tab.
The current result preview stays visible until the next accepted query replaces
it.

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
has a new profile identifier and requires a password.

Saving an edit disconnects idle sessions that use the profile. Their SQL and
downloaded results remain available. The next Run uses the updated settings.
Editing and deletion are disabled while a tab using the profile is busy.
Deletion requires confirmation. Qrow removes the profile and requests deletion
of its stored password. Keychain deletion failures are not reported in the UI,
so a failed deletion can leave the password in Keychain.

## Sessions and idle behavior

Each tab has its own session. Tabs can execute SQL concurrently, including when
they use the same profile. Separate sessions do not necessarily use separate
Spark engines. Engine sharing depends on the Kyuubi configuration.

The **When Idle** setting controls each session:

| Choice | Behavior |
| --- | --- |
| **Disconnect after** | Releases the session after the specified idle time. The default is 900 seconds. Reading results and editing SQL do not reset the timer. Running work is not interrupted. |
| **Keep Connected** | Sends periodic heartbeat SQL while the session is idle. The form suggests 300 seconds and `SELECT 1`. Both values can be changed. This mode is off by default. |

Use a lightweight, read-only statement for heartbeat SQL. Qrow checks that the
text contains one statement, but does not enforce read-only behavior. Heartbeats
use the existing session and preserve its result cursor. A failed heartbeat
disconnects the session and stops background queries until the next explicit Run.

Click **Disconnect** to release the active tab's session. This action is disabled
while a query or heartbeat runs. Manual and idle disconnection preserve SQL and
downloaded results. Unfetched rows and session state, such as temporary views or
settings applied with SQL, are lost. Selecting another profile also releases the
current session and keeps downloaded results visible. The next Run uses the new
profile.

Qrow's session idle timeout is separate from the server's engine idle timeout.
Disconnecting a Qrow tab does not guarantee that its Spark engine stops. Other
tabs, clients, and server policies can keep the engine active.

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
session deadline. It does not require continuous UI polling. The heartbeat uses
a separate operation in the same session so it can preserve the result cursor.

[Credential storage](../src/storage.rs) uses Keychain service
`io.qrow.connection`, keyed by profile UUID. Keeping that identifier stable
preserves access to existing passwords.

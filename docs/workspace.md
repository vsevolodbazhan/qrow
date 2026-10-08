# Workspace

Qrow saves your tabs and settings automatically. Restored tabs do not connect
to a database until you run SQL.

## Appearance and layout

Open **Qrow → Settings…** to change the appearance. The Appearance page shows
the **Interface**, **Assistant**, **Editor**, and **Logs** sections. Interface
has **Theme**, **Scale**, and **Font Family**. Editor, Logs, and Assistant have
**Font Family**, **Font Size**, and **Line Height**. The interface font applies
to controls and results. Editor also has **Tab Size**, the number of spaces for
each indent level. The default is 2. The Tab key and SQL that Qrow formats for
the assistant use this size. The editor indents with spaces. **SQL Keyword
Case** is the last Editor setting. At the moment, it applies only to SQL that
the [assistant](assistant.md) writes.
Editor settings apply to SQL. Logs settings apply to log entries.
Assistant settings apply to conversation messages. Code in messages keeps
its code font. Line height is relative to the font size. Scale multiplies the
Editor, Logs, and Assistant font sizes.
Each font picker has a **System Font** option. This option uses the macOS
interface font.

The theme defaults to **System**. System follows the macOS appearance and uses
GPUI Kit's default light or dark theme. The selector also includes One Dark,
CM Twilight, and the themes bundled from GPUI Kit. CM Twilight is darker than
One Dark. It uses the navy and orange colors of twilight in Chiang Mai. A
selected theme takes effect immediately.

The Assistant page has the **General** and **Codex** sections. See
[AI assistant](assistant.md#set-up-the-assistant) for these settings.

Select a page or section in the list at the left. Use the search box above the
list to show only settings that match a word in their name or description.
Each setting shows a description below its name. A long description wraps, and
the controls stay aligned at the right.

Changes appear immediately and are saved with the workspace. **Restore defaults**
restores the defaults of all appearance settings, including the System theme,
the system interface font, and SQL Keyword Case. It does not change the
Assistant page. If a saved font or theme is unavailable, Qrow uses a default and
reports the substitution.

The sidebar at the left of the window shows
[Connections](connections.md) or [Sign-Ins](connections.md#sign-in-with-openid-connect).
The two buttons at the left end of the status bar choose the sidebar. Click the
button of the visible sidebar to hide it. Press **⌘B** to show or hide the
Connections sidebar. **View → Toggle sidebar** hides or shows the sidebar that
was visible last. The **View** menu has the commands of both buttons. Qrow opens with the Connections sidebar. When the focused part of
the workspace closes, for example the result table after a failed query, the
SQL editor gets the focus. Thus shortcuts like **⌘B** continue to operate. Drag the sidebar divider
to change its width. Drag the divider above Results or Logs to change the
editor height.
These two divider positions are not saved in the workspace. Qrow saves the
optional [assistant pane](assistant.md) width.

## Saved state

The workspace file contains connection profiles, connection-owned tabs, tab
names, SQL, the selected connection, the last active tab for each connection,
and settings. Optional assistant state includes the enabled setting, panel and
model preferences, Codex thread identifiers, conversation titles, query
execution modes, and the query tab of each conversation. The workspace does not contain assistant messages, tool
arguments, tool results, result rows, Logs history, or Codex credentials. Its
default path is:

```text
~/Library/Application Support/Qrow/workspace.json
```

Qrow keeps the [schema copy](connections.md#browse-schemas) of each connection
in a separate file, in the `catalog` folder next to the workspace file. The
file name is the profile identifier. A [shared catalog](connections.md#share-schemas)
has one file, with the identifier of the shared catalog as its name. A schema
copy contains schema, table, view, and column names, their types, and their
comments. It does not contain refresh errors. Qrow deletes the file when you
delete the connection, when the connection joins a shared catalog, or when no
connection uses the shared catalog any more. Qrow ignores the file of a
connection that belongs to another host, port, username, or set of session
parameters, and makes a new copy. A change to one of these settings does not
clear a shared catalog. If Qrow cannot open the workspace, it does not write
schema copies.

Qrow keeps a compact copy of each [dbt manifest](connections.md#attach-a-dbt-project)
in the `dbt` folder next to the workspace file. The copy has the names,
descriptions, tests, and lineage of the manifest, and the positions of its SQL
texts, but not the SQL. Qrow deletes a copy when no connection uses its
manifest. If Qrow cannot open the workspace, it keeps the copies only in
memory.

Passwords and sign-in tokens remain in [macOS Keychain](connections.md#authentication-and-connection-failures).
Passwords, tokens, and result sets are not written to the workspace file. The
workspace keeps each [sign-in](connections.md#sign-in-with-openid-connect)
configuration and the issuer, subject, name, and email of its account. SQL text is
stored as plain text. Do not put passwords into saved SQL or session parameters.

Workspace version 3 adds the optional assistant state. Qrow gives version 1
and version 2 workspaces safe assistant defaults during load. The assistant is
off by default. The migration preserves connections, tabs, SQL, active-tab
state, and appearance settings.
Workspace version 4 links each assistant conversation to a query tab. When
Qrow loads an earlier workspace, the conversation that was open gets the active
tab. The other conversations load without a query tab, under the connection
that was active. An earlier version of Qrow cannot open a version 4
workspace.
Workspace version 5 adds shared schema catalogs. Earlier workspaces load
without shared catalogs. An earlier version of Qrow cannot open a version 5
workspace.
Qrow ignores the removed **Schema refresh logs** option of a connection, because
[Activity](activity.md) always records refreshes. The next save removes the
option from the file.

Workspace version 7 adds the database type of each connection. Profiles from
older workspaces use Kyuubi. Earlier versions of Qrow cannot open a version 7
workspace.

Workspace version 6 adds sign-ins, and the TLS and authentication choices of
each connection. Earlier connections load with password authentication and
without TLS. They keep their identifiers and stored passwords. An earlier
version of Qrow cannot open a version 6 workspace.

Postgres profiles store an optional TLS mode. A profile without this field uses
its earlier TLS checkbox. An enabled checkbox still verifies the certificate
and hostname. See [Postgres connections](connections.md#use-postgres).

## Quit and save

Qrow saves after a short editing delay. **Qrow → Quit Qrow**, **⌘Q**, and the
window close button wait for confirmation that the latest workspace is saved.
You can continue to edit while a save is in progress. Qrow saves those new edits
before it exits.

If an Assistant turn or query is active when you quit or close the window, Qrow
shows **Work is still running**. Select **Keep working** to leave Qrow open.
Select **Quit anyway** to stop the active work. Qrow still saves the workspace
before it exits.

## Load and save failures

If the workspace is corrupt, unreadable, or uses an unsupported version, Qrow
reports the failure. It leaves the file untouched and disables saving for that
run. New edits from that run will not be saved. Preserve the original file before
attempting recovery. Qrow does not provide an automatic repair tool.

Only one Qrow process can write to a workspace directory. If another process
has the workspace open, the new process reports the conflict and disables
saving. Use the original process. The operating system releases the lock when
that process exits or crashes. Do not delete `workspace.lock` to remove a lock.

A save failure is reported in the application. Do not assume changes reached
disk after a save error. If a save fails during Quit or window close, Qrow keeps
the window open. Select **Keep editing** to retain access to your SQL. Correct
the file access problem, then select **Retry save and quit**. **Quit Without
Saving** exits without confirmation that the latest edits reached disk.

The current framework cannot cancel termination requested through the macOS
Dock or system shutdown. Qrow attempts a final save for those requests, but a
failure cannot keep the window open. Use **Qrow → Quit Qrow** or **⌘Q** for
save confirmation. A force quit or system crash can lose edits that are not yet
saved.

For isolated development, see [Development](development.md#check-the-native-ui).
`QROW_DATA_DIR` changes the workspace directory, but does not isolate Keychain.
Passwords and sign-in tokens are keyed by profile and sign-in identifiers, so
a copy of a workspace shares them with the original.
The [demo](../README.md#preview) uses an in-memory workspace and does not access
databases or Keychain. The demo writes its synthetic dbt manifest to a temporary
file. It removes this file when the window closes.

## Design

The [workspace model](../src/model.rs) holds saved data separately from live
sessions, results, and Logs history. [Storage](../src/storage.rs) writes
through a background saver. Storage claims an operating-system lock before it
loads a writable workspace. It holds that lock until the saver stops. Each save
uses a unique temporary file. Storage flushes the file, replaces the workspace,
and flushes the parent directory before it confirms success. This avoids a
partially written workspace after a normal write failure.

The [UI](../src/ui.rs) schedules saves after edits and flushes the latest state
before confirmed quit. Existing workspace versions, profile UUIDs, and Keychain service
identifiers must remain compatible.

Native focus, window dragging, double-click behavior, appearance changes, and
workspace restoration need direct UI verification when changed. The existing
end-to-end suite does not establish every layout and window interaction.

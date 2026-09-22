# Workspace

Qrow saves the connections, tabs, and settings of each workspace separately.
Restored tabs do not connect to a database until you run SQL.

## Create, select, or rename a workspace

On the first launch, Qrow shows a welcome screen. Select **Create workspace…**,
enter a name, then select **Create workspace**. If you cancel, the welcome
screen stays open. Qrow does not create a workspace until you submit a name.

The macOS **Workspaces** menu has separate commands:

- **New Workspace…** creates and opens a workspace.
- **Select Workspace** opens a native submenu of workspace names.
  A checkmark identifies the current workspace. Select a name to open it.
- **Rename Workspace…** changes the current workspace name. It keeps the same
  files, connections, tabs, sessions, and results.

The workspace name in the window header opens a native selection menu.
You can also select **Select Workspace** in Settings.
Names must be unique and contain no more than 80 characters.
Select, Rename, and Delete are unavailable until a workspace exists.

Qrow opens the last opened workspace at startup. There is no default-workspace
setting. An existing workspace from an older version keeps the name **Default**
until you rename it. Its files and connection passwords stay in their original
locations. A new workspace starts with no connections and default settings.

Wait for running queries to finish before you create, select, rename, or delete a
workspace. Switching saves the current SQL, then closes that workspace's
sessions and results. Returning restores saved tabs and settings, but not
result rows or Logs history. If the save or destination load fails, the current
workspace stays open. Escape closes the form, except while a workspace
operation is in progress. Workspace commands are unavailable in demo mode.

## Delete a workspace

Select **Workspaces → Delete Workspace…** to delete the current workspace.
The confirmation names the workspace. **Cancel** or **Escape** keeps it open.
Select **Delete workspace** to permanently remove its saved queries,
connections, settings, and saved passwords. There is no Trash, Undo, or Restore.
Passwords that another workspace still uses are kept.

Qrow saves the current state and checks the next workspace before deletion.
If either step fails, the current workspace stays open and Qrow reports the
error. After deletion, Qrow closes the deleted workspace's sessions and results
and opens the most recently used remaining workspace. When none remain, Qrow
shows the welcome screen. It does not create a replacement workspace.

Qrow records the order in which you open workspaces. For older workspace lists,
only the last selection is known. Qrow uses reverse creation order for the
other workspaces until you open them again.

If file or password removal fails after deletion starts, Qrow reports incomplete
cleanup. It retries cleanup at the next startup. This retry does not restore the
workspace.

## Appearance and layout

Open **Qrow → Settings…** to change the appearance. The dialog shows the
**Interface**, **Editor**, and **Logs** sections on one page. The Interface
section has **Scale** and **Font Family**. The Editor and Logs sections have
**Font Family**, **Font Size**, and **Line Height**. The interface font applies
to controls and results. Editor settings apply only to SQL. Logs settings apply
only to Logs. Line height is relative to the font size. Scale multiplies the
Editor and Logs font sizes.

Select a section in the list at the left to move to that section. Use the search
box above the list to show only the settings that match a word in their name or
description. The section list needs the pointer. All controls stay on the page,
so the keyboard reaches each of them without the list.

Changes appear immediately and are saved with the workspace. **Restore defaults**
restores the defaults of all appearance settings, including the system interface
font. If a saved font is unavailable, Qrow uses a default font and reports the
substitution.

The application uses its bundled dark theme, independent of the macOS appearance.
There is no light-theme setting.

Press **⌘B** to hide or show the Connections sidebar. Drag the sidebar divider
to change its width. Drag the divider above Results or Logs to change the
editor height.
These layout positions are not saved in the workspace.

## Saved state

The workspace file contains connection profiles, connection-owned tabs, tab
names, SQL, the selected connection, the last active tab for each connection,
and appearance settings. It does not contain
result rows or Logs history. Its default path is:

```text
~/Library/Application Support/Qrow/workspace.json
```

The workspace list, recent selections, and pending deletion cleanup are in
`workspaces.json` in the same
directory. New workspace files are in `workspaces/<UUID>/workspace.json`.
Workspace names are labels, not directory paths. Qrow updates the workspace-list
format when it saves. Earlier app builds cannot read the updated list.

Passwords remain in [macOS Keychain](connections.md#authentication-and-connection-failures).
Passwords and result sets are not written to the workspace file. SQL text is
stored as plain text. Do not put passwords into saved SQL or session parameters.

Qrow saves after a short editing delay. **Qrow → Quit Qrow**, **⌘Q**, and the
window close button wait for confirmation that the latest workspace is saved.
You can continue to edit while a save is in progress. Qrow saves those new edits
before it exits.

## Load and save failures

If the workspace is corrupt, unreadable, or uses an unsupported version, Qrow
reports the failure. It leaves the file untouched and disables saving for that
run. The editor stays hidden until the workspace can load. Preserve the original file before
attempting recovery. Qrow does not provide an automatic repair tool.

Only one Qrow process can write to each workspace file. If another process
has the workspace open, the new process reports the conflict and disables
saving. Use the original process. The operating system releases the lock when
that process exits or crashes. Do not delete `workspace.lock` to remove a lock.

A save failure is reported in the application. Do not assume changes reached
disk after a save error. If a save fails during Quit or window close, Qrow keeps
the window open. Select **Keep Editing** to retain access to your SQL. Correct
the file access problem, then select **Retry Save and Quit**. **Quit Without
Saving** exits without confirmation that the latest edits reached disk.

The current framework cannot cancel termination requested through the macOS
Dock or system shutdown. Qrow attempts a final save for those requests, but a
failure cannot keep the window open. Use **Qrow → Quit Qrow** or **⌘Q** for
save confirmation. A force quit or system crash can lose edits that are not yet
saved.

For isolated development, see [Development](development.md#check-the-native-ui).
`QROW_DATA_DIR` changes the directory for the workspace list and all workspace
files, but does not isolate Keychain.
The [demo](../README.md#preview) uses an in-memory workspace and does not access
databases or Keychain.

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

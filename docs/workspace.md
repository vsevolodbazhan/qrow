# Workspace

Qrow saves your tabs and settings automatically. Restored tabs do not connect
to a database until you run SQL.

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
run. New edits from that run will not be saved. Preserve the original file before
attempting recovery. Qrow does not provide an automatic repair tool.

Only one Qrow process can write to a workspace directory. If another process
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
`QROW_DATA_DIR` changes the workspace directory, but does not isolate Keychain.
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

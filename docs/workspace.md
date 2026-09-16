# Workspace

Qrow saves your tabs and settings automatically. Restored tabs do not connect
to a database until you run SQL.

## Appearance and layout

Open **Qrow → Settings…** to change the interface font, editor font, editor font
size, or interface scale. The interface font applies to controls and results.
The editor font applies to SQL. Editor font size uses pixels before scaling.

Changes appear immediately and are saved with the workspace. **Restore defaults**
resets all appearance settings, including the system interface font. If a saved
font is unavailable, Qrow uses a default font and reports the substitution.

The application uses its bundled dark theme, independent of the macOS appearance.
There is no light-theme setting.

Press **⌘B** to hide or show the Connections sidebar. Drag the sidebar divider
to change its width. Drag the divider above Results to change the editor height.
These layout positions are not saved in the workspace.

## Saved state

The workspace file contains connection profiles, open tabs, tab names, SQL,
selected profiles, the active tab, and appearance settings. Its default path is:

```text
~/Library/Application Support/Qrow/workspace.json
```

Passwords remain in [macOS Keychain](connections.md#authentication-and-connection-failures).
Passwords and result sets are not written to the workspace file. SQL text is
stored as plain text. Do not put passwords into saved SQL or session parameters.

Qrow saves after a short editing delay. It also flushes saved state on application
quit and when the last window closes.

## Load and save failures

If the workspace is corrupt, unreadable, or uses an unsupported version, Qrow
reports the failure. It leaves the file untouched and disables saving for that
run. New edits from that run will not be saved. Preserve the original file before
attempting recovery. Qrow does not provide an automatic repair tool.

A save failure is reported in the application. Do not assume changes reached
disk after a save error.

For isolated development, see [Development](development.md#check-the-native-ui).
`QROW_DATA_DIR` changes the workspace directory, but does not isolate Keychain.
The [demo](../README.md#preview) uses an in-memory workspace and does not access
databases or Keychain.

## Design

The [workspace model](../src/model.rs) holds saved data separately from live
sessions and results. [Storage](../src/storage.rs) writes through a background
saver. It writes a temporary file, flushes it, and renames it to replace the
workspace. This avoids leaving a partially written workspace after a normal
write failure.

The [UI](../src/ui.rs) schedules saves after edits and flushes the latest state
on shutdown. Existing workspace versions, profile UUIDs, and Keychain service
identifiers must remain compatible.

Native focus, window dragging, double-click behavior, appearance changes, and
workspace restoration need direct UI verification when changed. The existing
end-to-end suite does not establish every layout and window interaction.

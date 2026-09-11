# GPUI Kit rebuild

The UI uses `gpui-kit` 0.6.1 as its framework dependency. Kit supplies the matching
GPUI, components, assets, and platform crates, including runtime Metal shaders.
The core library still builds with `--no-default-features`.

The workspace now composes a connection sidebar, query tab bar, execution
toolbar, SQL editor, results viewport, and status bar. Connection settings use
a centered popup dialog with grouped fields and a fixed footer. Presentation
uses theme colors and relative spacing. System appearance changes update the theme.

Workspace commands and worker events remain in `src/ui.rs`. Layout is in
`src/ui/workspace_view.rs`, settings presentation in `src/ui/profile_view.rs`,
and table rendering, scroll handling, and clipboard behavior in
`src/ui/results.rs`. The window owns overlay rendering separately from workspace
state. Sessions, query cancellation, bounded fetching, credentials, and workspace
formats retain their existing behavior.

The table reserves a horizontal scrollbar lane inside its viewport. This avoids
clipping an overlaid track. The existing horizontal-wheel capture and splitter
release safeguards remain. Cell rendering borrows stored values and allocates
only their display prefixes; clipboard commands retain full values.

## Validation on 2026-09-11

- `sh scripts/check.sh` passed, including core and native tests, script checks,
  dependency audit, SQL performance budgets, and core coverage.
- `sh scripts/check.sh native` passed again after the final UI corrections.
- The signed release bundle passed packaging and size checks. The executable
  is 21.09 MiB and the archive is 7.60 MiB, below the 24 MiB and 10 MiB budgets.
- The final release demo reported 218 ms for UI initialization. This is one
  startup log sample, not cold launch to first visible frame.
- Core line coverage was 88.62%, with 981 of 1,107 measured lines covered.
- SQL validation medians were 0.302 ms at 10 KB, 3.202 ms at 100 KB, and
  31.460 ms at 1 MB. These do not measure rendering or Spark latency.
- Demo window checks covered tab switching, creating a tab, empty results,
  sidebar and editor splitters, release followed by another click, vertical
  scrolling, and horizontal scrollbar dragging across a 141-column result.
- Connection checks covered inline validation, synthetic profile saving,
  Command-Enter, Escape dismissal, and a fixed Save/Cancel footer at the
  850 by 560 minimum content size.
- Keyboard selection copied `SELECT 'Привет 😀' AS greeting;` exactly from an
  editor containing a preceding comment with emoji and Japanese text.

The native automation tool produced zero-distance horizontal wheel events.
Horizontal mouse/trackpad scrolling therefore still needs a human check;
scrollbar dragging was verified. Automated column-divider drags did not visibly
change column widths, so column resizing also needs a manual check. Direct
accessibility text selection was also
unsupported, so Unicode selection was checked with keyboard input instead.
Light appearance, interface zoom, live Kyuubi execution and cancellation, and
workspace restoration were not revalidated through the GUI in this change.

The dependency migration removed three obsolete advisory waivers. A scoped
license exception for `libbz2-rs-sys` 0.2.5 is documented in
`dependency-reviews.toml`, with a review deadline enforced by the policy checker.
The packaged third-party notices include its supplied license text. Rust also
reports a future-compatibility warning from the transitive `block` 0.1.6 crate;
current checks pass without weakening lint rules.

## Layout follow-up

The connector label is `Spark (HiveServer2)`. Sidebar profile icons and labels
align to the leading edge. Query tabs use Kit's large size, increasing the tab
height from 24 to 36 pixels. Settings open in a centered modal popup, sized from
the current window and theme font. Fields scroll while the footer stays visible.
Enter confirmation uses the same save command and waits for its result before
closing. Duplicating a profile reuses the popup and focuses the new name.

The window uses Kit's `TitleBar` with native macOS window buttons. The sidebar
footer shows only the connector label. Native checks passed after these changes;
the release demo confirmed popup save and dismissal, the compact popup layout,
and tab selection after a title-bar drag. Window movement and double-click
behavior still need manual verification.

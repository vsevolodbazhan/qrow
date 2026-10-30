# GPUI Component patch

The completion overlay host passes nonselectable notes to the menu. Notes
can appear without candidates and offer an explicit action below the list.
Keyboard navigation visits candidates only. Catalog updates preserve the
selected candidate when its replacement edit is unchanged. Narrow windows
keep the full description in a scrollable stacked panel. Completion labels
use UTF-8 character boundaries for highlights, and menu, note, row, and
description elements expose accessibility and test metadata. Handled menu keys
stop propagation to the editor. Deferred insertion and dismissal must match
the menu generation. The list delegate holds its menu weakly, so a closed menu
can be released. `tests/ui/sql_completion.rs` covers the production integration,
and `tests/ui/sql_completion_races.rs` checks delayed responses.

This directory contains the `gpui-component` 0.6.6 crate from crates.io. The
source has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes these files:

- `src/input/input.rs` and `src/input/editor.rs`: `context_menu_with_capabilities`
  passes the current selection and edit state to a custom context menu.
  The input entity is borrowed during the callback, so the callback cannot
  read that entity. The editor uses this snapshot for Format SQL and the
  clipboard commands. The Qrow UI test checks the callback after a right-click.

- `src/input/input.rs`: disabled inputs do not register a focusable frame.
  Before, clicking a disabled input moved keyboard focus to its frame.
  The sign-in UI and E2E tests check that the locked account field cannot
  receive keyboard focus.
- `src/menu/popup_menu.rs` and `src/menu/menu_item.rs`: standard menu items
  accept a tooltip, including when disabled. Dismissing a menu hides the
  tooltip. This gives a disabled Delete action a reason without an extra row.
- `src/searchable_list/item.rs`: a row of a Select or Combobox list has 1 pixel
  of space above and below its highlight. Before, the highlights of two
  adjacent rows touched, for example a hovered row below the chosen row. Popup
  menu items have a 2-pixel gap between them. GPUI Component 0.7.0 does not
  have the fix. The row with the highlight keeps the element ID: GPUI stores
  the hover state of an element under its ID, so a row without one draws no
  hover highlight.
- `src/root.rs`: Root hides the managed tooltip when a dialog or a sheet opens
  or closes. Before, a shortcut that closed a dialog under the pointer, for
  example ⌘Enter on a hovered Save button, left the tooltip of the button on
  the screen. The trigger was gone, so it did not get a hover-out.
- `src/root.rs`: an asynchronous owner can identify and close its dialog
  without closing a newer modal. Removing a dialog below another modal keeps
  the focus on that modal and repairs its focus restoration. Export completion
  uses this API to preserve an unanswered quit confirmation.
- `src/tab/tab_bar.rs`: the first tab of a tab bar without a prefix has no
  left border. Before, the tab bar always assumed a prefix, so the border of
  the first tab and the divider of a sidebar beside the bar made a double line.
- `src/input/input.rs` and `src/input/textarea.rs`: `focus_ring(false)` turns
  off only the ring outside the border of an input or a textarea. The focused
  field keeps its tinted border, as a Select and an InputGroup do. Before, the
  input lost its focus border too, and a textarea had no way to turn off the
  ring. Qrow turns off the ring of its fields, because lists and panels that
  clip their content cut the ring off. The test of each file checks the
  setting. Run them in a copy of the crate in the GPUI Kit repository, as the
  tests read files outside the crate.
- `src/menu/context_menu.rs`: the dismiss handler of a context menu holds its
  shared state weakly and releases the dismissed menu. Before, the state owned
  the subscription and the subscription owned the state, so the last menu of
  each context menu area stayed alive until the application exited. The
  leak detector of the UI tests found this with the menu of the results table.
- `src/table/state.rs`: the header clamps the horizontal scroll offset to the
  content width before it selects the columns to render. Before, a fast
  trackpad swipe moved the shared offset past the right edge. The header then
  skipped columns, and the rows showed them at the clamped offset without a
  name. The Qrow UI test `results::fast_horizontal_scroll_keeps_the_column_headers`
  checks the fix.
- `src/tooltip.rs`: with the `test-support` feature, a tooltip is the observed
  element `tooltip`, so application tests can find an open tooltip.
- `Cargo.toml`: `cargo machete` ignores the `log` dependency, which the crate
  declares but does not use.

Remove the patch when a GPUI Kit release includes the fixes. Update the crate
with the other GPUI crates as one set.

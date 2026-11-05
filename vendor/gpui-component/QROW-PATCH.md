# GPUI Component patch

This directory contains the `gpui-component` 0.6.6 crate from crates.io. The
source has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes these files:

- `src/input/input.rs` and `src/input/editor.rs`: `context_menu_with_capabilities`
  passes the current selection and edit state to a custom context menu.
  The input entity is borrowed during the callback, so the callback cannot
  read that entity. The editor uses this snapshot for Format SQL and the
  clipboard commands. The Qrow UI test checks the callback after a right-click.
- `src/form/form.rs`: forms apply caller styles after their default styles.
  The export form uses a 16-point gap between columns.

- `src/input/input.rs`: disabled inputs do not register a focusable frame.
  Before, clicking a disabled input moved keyboard focus to its frame.
  The sign-in UI and E2E tests check that the locked account field cannot
  receive keyboard focus.
- `src/input/mod.rs`: exports `EMPHASIS_OPACITY` of the GPUI Base patch, so
  Qrow tests can find the tint of the SQL editor.
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
- `src/setting/page.rs`: selecting a group in the Settings sidebar scrolls the
  group heading as close to the top of the page as the content allows.
  Before, the page scrolled only until the group was visible, so its heading
  stopped in the middle of the page.
- `src/sheet.rs`: `Sheet::margin_bottom` keeps a band at the bottom of the
  window free. The dbt details sheet ends above the status bar.
- `src/table/state.rs`: when the rows overflow the body, each row and the
  header end with room for the vertical scrollbar after their cells, so it
  covers no text, while row backgrounds, stripes, the selection tint, and
  the row and header lines reach the right edge. When the columns overflow
  and the last row would sit under the horizontal scrollbar, the rows end
  with room for it. Before, the scrollbars covered the text of the bottom
  row and of the rightmost column, and later a padded body left an empty
  band beside and below the rows.
  When columns are hidden past an edge of the scrolled columns, the cut
  column fades into that edge, in the header and in each row, with a 28 px
  gradient in the color that the row shows there: the table, a stripe, a
  hover, the selected row, or the background of the cut cell from
  `TableDelegate::cell_background` (new, `None` by default). The fades stop
  above the header border, sit inside the scrollbar room, and do not take
  pointer input. The Qrow UI test
  `results::cut_columns_fade_only_where_more_columns_are_hidden` checks
  which sides fade. A new table size
  renders the table again: the header picks its visible columns from the
  size of the last frame, so a wider pane left the header cells of the
  revealed columns blank.
- `src/tab/tab.rs`: a tab with a maximum width also shrinks when its bar is
  short of room, down to a least width of 10.5 rems, which fits a short label
  like "Query 12" in the large size. The selected tab does not shrink, so it
  shows its whole label up to the maximum width. `TabBar::fit_width` gives
  the tabs other than the selected one a smaller least width, so that a bar
  that would scroll only a little fits all its tabs. A label that does not fit
  ends in an ellipsis, and the suffix, like a close button, keeps its size.
  Before, the bar scrolled and clipped the close button of the last tab under
  the bar suffix.
- `src/tab/tab_bar.rs`: a vertical wheel also scrolls a tab bar, which has
  nothing to scroll vertically. Before, only a horizontal swipe scrolled it.
  A bar of the `Tab` variant that tracks its scroll shows a short gradient
  in the border color at each edge with hidden tabs past it. It snaps its
  scroll so that a tab starts exactly at the start edge: when a wheel or a
  swipe over it stops, and when the bar width, the selection, the tab count,
  or a tab width changes. After such a change, the selected tab also scrolls
  fully into view. At the end of the scroll range, the last tab aligns to
  the end edge instead. A tab that the end edge cuts fades into the bar. A
  tab that the start edge cuts has its title past the edge, so the bar
  paints the title from the edge on, cut with an ellipsis, or nothing when
  not even two characters and the ellipsis fit after the tab's left padding.
  Fades and cues stop above the bottom border of the bar. With `test-support`, `pinned_tab_titles`
  gives the titles that the last frame pinned. The
  Qrow UI tests in `tabs` check the wheel, the snap, the end of the range, a
  resize, an Assistant toggle, a rename, and new tabs.
- `src/tooltip.rs`: `ManagedTooltipExt` is public, and `managed_tooltip_when`
  shows its tooltip only while a condition holds when the pointer enters.
  A result column header shows its full name only when its column cuts it.
  The Qrow UI test `results::a_column_name_that_fits_has_no_tooltip` checks
  the condition.
- `src/menu/popup_menu.rs`: `PopupMenu::set_trigger_focus` is public, so a
  context menu that the application opens shows the shortcuts of the view
  under the pointer.

Remove the patch when a GPUI Kit release includes the fixes. Update the crate
with the other GPUI crates as one set.

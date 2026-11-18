# GPUI Base patch

This directory contains the `gpui-base` 0.6.6 crate from crates.io. The source
has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes these files:

- `src/test_support.rs`: snapshots expose the native click action through
  `is_clickable`. Enabled buttons do not expose an explicit disabled flag in
  this GPUI version. The test harness checks the click action before input.
  `tests/ui/timing.rs` covers enabled and disabled production Run buttons.

- `src/input/base/element.rs`: the active line paint now covers the editor's
  right padding.
- `src/text/inline_flow.rs`: a Markdown line with inline code keeps one line at
  its own max-content width. Before, the line wrapper added glyph advances,
  which are wider than kerned text, and broke the line. A chat bubble sized for
  one line then hid the last word. `src/text/inline.rs` adds a kerned test font
  for the regression test.
- `src/text/inline_flow.rs`: a wrapped Markdown flow reports at least the
  width that the line wrapper measured for each line. Before, the flow
  reported the shaped width of its longest line, which is narrower. A chat
  bubble sized from that width wrapped the text again into more lines and hid
  the last line. `src/text/inline.rs` makes the kerned test font kern only
  text of two or more characters, as the wrapper measures one character at a
  time.
- `src/text/inline_flow.rs`: the line wrapper measures bold and italic text in
  its own face. Before, the wrapper measured it in the regular body face, which
  is narrower. A line with bold text then became wider than the column, and
  the column clipped its end.
- `src/text/inline_flow.rs`: Markdown text breaks lines with its own line
  wrapper instead of GPUI's. The new wrapper adds the same widths, but finds
  the breaks with the punctuation rules of the Unicode line breaking
  algorithm (UAX #14), and does not break inside a grapheme. Before, a line
  could start with `?`, also after a space, as in `сегодня ?`. Bold or
  styled text is a row of elements for the wrapper, and GPUI's wrapper
  allowed a break before each element after a space. A bold `«Aviasales»`
  then broke after `«`. The tests of the file check the rules and both
  cases.
- `src/text/inline.rs`: Markdown text paints its selection under the glyphs,
  as the input does. Before, the selection was painted over the glyphs and
  dimmed the selected text. `src/text/text_view.rs` adds the regression test.
- `src/input/editor/indent.rs` and `src/input/base/element.rs`: the editor
  paints its indent guides as 1-pixel quads. Before, the guides were one
  stroked path. Each frame with a path makes GPUI draw through an
  intermediate texture of the window size, and the texture then stays in GPU
  memory: approximately 16 MB for a 1280 by 821 point window on a Retina
  display. `src/input/editor/indent.rs` has the unit test.
- `src/input/base/blink_cursor.rs`: the caret stops blinking and stays visible
  10 seconds after the last input, focus, or window activation. Before, the
  caret of a focused input blinked until the input lost focus, and each blink
  repainted the whole Qrow window. The file also contains two fixes from GPUI
  Kit 0.7.0 (longbridge/gpui-kit#3139 and #3140): a stop clears the blink
  state, and a pause does not start a blink loop in an unfocused input.
- `src/selectable_text.rs`: `SelectableText::highlights` styles ranges of the
  text. The dbt details sheet uses it to show SQL with syntax highlighting
  that the user can select. Before, selectable text was plain. The file has
  the regression test.
- `src/selectable_text.rs`: the pointer is a text cursor over selectable
  text, as over Markdown text. Before, it stayed an arrow, so the SQL of the
  dbt details sheet did not look selectable. GPUI tests cannot read the
  cursor, so this change has no test.
- `src/text/inline.rs` and `src/text/mod.rs`: the table that keeps the shaped
  text of a Markdown paragraph for the next frame removes the entries of
  dropped paragraphs when it grows to twice the size after its last cleanup,
  and at 64 entries or more. Before, it removed them only at 4,096 entries.
  Each streamed reply step and each closed tooltip drops paragraphs, so after
  some work the table held thousands of dropped paragraphs: about 25 MB in
  a measured session. `retained_layout_count`, for tests only, gives the
  number of entries. The Qrow UI test
  `replaced_paragraphs_release_their_shaped_text` is the regression test.
- `src/input/base/state.rs`, `src/input/base/element.rs`, and
  `src/input/mod.rs`: `set_emphasized_range` tints one byte range of the text
  with the selection color at `EMPHASIS_OPACITY`. Qrow uses it to show the
  statement that Run uses. A row that the range fills to its line end gets
  the tint to the right edge. The element paints the tint as one quad for
  each row, not as a path, because a path keeps a window-size texture in GPU
  memory, and the tint shows most of the time. The Qrow UI test
  `the_editor_tints_the_statement_that_run_uses` is the regression test.

- `src/tree.rs`: `TreeState` implements `Focusable`, so a context menu of a
  tree row can show the shortcuts that the tree binds.
- `src/tooltip.rs`: a scroll wheel event hides the managed tooltip, as it
  already hid on a key press, a mouse press, and a pointer move off the
  trigger. Before, a tooltip stayed on the screen while its trigger scrolled
  away.
- `src/input/base/element.rs`: the placeholder of an empty multi-line input
  wraps at its spaces to the text width. Before, a narrow SQL editor cut its
  placeholder off at the right edge. GPUI tests do not shape real text, so
  this change has no test.
- `src/input/base/state.rs`: `set_auto_grow` fits the row count to the
  current text at once. Before, the new limit set the row count to the
  minimum until the next edit, so a field that held a long message shrank.
  Qrow changes the limit of the message field when the window height changes.
  The Qrow UI test `the_message_field_grows_with_its_text_up_to_a_limit` is
  the regression test.
- `src/text/markdown_ext.rs`, `src/text/format/markdown.rs`, and
  `src/text/text_view.rs`: `MarkdownExtensions::line_breaks` and
  `TextView::markdown_line_breaks` show each line ending in a paragraph as a
  line break. CommonMark joins the lines with spaces. Qrow uses the option for
  the messages that you send, so a message with SQL keeps its lines. The Qrow
  UI test `a_message_keeps_its_line_breaks` is the regression test.
- `src/text_selection.rs`, `src/text/text_view.rs`, and
  `src/text/selection_adapter.rs`: a drag selection works in a virtual list,
  such as the Qrow transcript.
  - A scroll during a drag, by the wheel or by drag auto-scroll, moves the
    cursor to the text under the pointer. Before, the cursor stayed on the
    text where the pointer was before the scroll. The selection grew only on
    the next pointer move, and then by the whole scrolled distance.
  - `TextSelectionRegistration::with_keep_offscreen` and
    `TextView::virtual_row` mark a participant that a selection keeps while
    frames do not paint it. Its owner keeps it alive, for a text view with
    `TextView::state`. `TextView::document_order` gives it a stable order.
    Hit tests skip an offscreen participant. An end in it goes far before or
    far after the painted text, in document order, so the painted rows
    between the ends show the selection. Both ends in one offscreen
    participant keep their places. An offscreen participant keeps its
    projected copy while only the window points of the selection move.
    Before, the frame removed the participant, and the selection disappeared
    when its start scrolled out of the list. Other participants still leave
    the selection when frames stop painting them, as for a closed view.
  - Drag auto-scroll measures the pointer against all that the anchor's
    participant showed during the drag, and finds a stalled container from
    the cursor's participant when the anchor is offscreen. Before, an anchor
    clipped to a slice at the bottom edge reversed the scroll. A mask that
    collapses over an anchor inside that area still stops auto-scroll.
  The tests in `src/text_selection.rs` check these rules. The Qrow UI tests
  in `assistant_selection` check a wheel scroll and drag auto-scroll across
  four replies. To run the GPUI Base tests, use
  `CARGO_TARGET_DIR=target/gpui-base-tests cargo test --manifest-path
  vendor/gpui-base/Cargo.toml --lib text_selection::`.
- `src/scrollbar.rs` and `src/scrollable_mask.rs`: the vertical
  `ScrollableMask` clamps a wheel step to `ScrollbarHandle::max_wheel_offset`
  (new). By default, it is the content size less the viewport. A `ListState`
  also reaches its current offset: `max_offset_for_scrollbar` leaves out the
  padding of the list, but at its end the offset includes the padding.
  Before, the mask clamped the offset to the rows first, so the first wheel
  step from the end of the Qrow transcript moved it 10.5 points more than the
  step. The content size, and with it the scrollbar thumb, does not change.
  The test in `src/scrollable_mask.rs` and the Qrow UI test
  `a_wheel_step_from_the_end_moves_the_transcript_by_its_delta` check it. A
  step of at most 1 point from the end still returns to the end: GPUI's list
  takes that place for its end and follows new rows from there.

Remove a patch when a GPUI Kit release includes its fix. Update the crate set
as one unit.

- Input paint can interpolate short horizontal caret movements with the existing motion engine. The styled layer supplies the duration. Logical input and popup geometry stay immediate. Mouse placement, selection, IME, scrolling, layout changes, and large jumps disable interpolation. Reduced motion remains immediate.

- Discrete input wheel events can use a caller-supplied transition duration. Targets accumulate and stay within the viewport limits. Pixel gestures, typing, mouse placement, and direct viewport changes cancel pending travel. Frame requests stop when the target is reached.

- `InputEditorStyle::auto_grow_transition` opts into content-driven height
  transitions for auto-grow inputs. The default is zero. Direct window, width,
  and line-height changes snap to their new size. Reduced motion and zero
  duration settle immediately. Keyed transitions reverse from their current
  height and stop requesting frames when settled.

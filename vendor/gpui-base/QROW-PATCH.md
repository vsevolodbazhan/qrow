# GPUI Base patch

This directory contains the `gpui-base` 0.6.6 crate from crates.io. The source
has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes these files:

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
- `src/text/inline.rs`: Markdown text paints its selection under the glyphs,
  as the input does. Before, the selection was painted over the glyphs and
  dimmed the selected text. `src/text/text_view.rs` adds the regression test.
- `src/input/base/blink_cursor.rs`: the caret stops blinking and stays visible
  10 seconds after the last input, focus, or window activation. Before, the
  caret of a focused input blinked until the input lost focus, and each blink
  repainted the whole Qrow window. The file also contains two fixes from GPUI
  Kit 0.7.0 (longbridge/gpui-kit#3139 and #3140): a stop clears the blink
  state, and a pause does not start a blink loop in an unfocused input.

Remove a patch when a GPUI Kit release includes its fix. Update the crate set
as one unit.

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

Remove a patch when a GPUI Kit release includes its fix. Update the crate set
as one unit.

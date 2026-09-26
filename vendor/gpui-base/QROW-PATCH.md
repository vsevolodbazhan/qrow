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

Remove a patch when a GPUI Kit release includes its fix. Update the crate set
as one unit.

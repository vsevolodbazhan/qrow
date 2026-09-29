# GPUI macOS platform patch

This directory contains the `gpui-pre-macos` 0.3.6 crate from crates.io, the
macOS platform of GPUI. The source has an Apache 2.0 license. See
`LICENSE-APACHE`.

Qrow changes these files:

- `src/window.rs` and `src/display_link.rs`: a window stops its display link
  1 second after GPUI last requested a frame, and starts it again at the next
  request. Before, the display link of a visible window fired at the refresh
  rate of the display, and each tick woke the main thread, also when nothing
  changed. The window now supplies GPUI's frame waker, which GPUI calls when
  it wants a frame. `src/display_link.rs` has the unit tests.

Remove the patch when a GPUI release includes the fix. Update the crate with
the other GPUI crates as one set.

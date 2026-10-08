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
- `src/memory_relief.rs`, `src/display_link.rs`, and `src/gpui_macos.rs`:
  5 seconds after the display links of all windows stop, a background thread
  calls `malloc_zone_pressure_relief` one time. The macOS allocator then
  returns the free pages of freed blocks. Before, these pages stayed in the
  process footprint: after a 110 MB dbt manifest parse,
  the idle footprint was 93 MB, and now it is 75 MB. `src/memory_relief.rs`
  has the unit tests of the timing.

Remove the patch when a GPUI release includes the fix. Update the crate with
the other GPUI crates as one set.

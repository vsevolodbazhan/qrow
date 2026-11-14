# GPUI macOS platform patch

This directory contains the `gpui-pre-macos` 0.3.6 crate from crates.io, the
macOS platform of GPUI. The source has an Apache 2.0 license. See
`LICENSE-APACHE`.

Qrow changes these files:

- `src/renderer.rs`, `src/software_renderer.rs`, `src/opengl_renderer/`,
  `src/gpui_macos.rs`, `src/window.rs`, and `Cargo.toml`: the platform can
  draw GPUI scenes on the CPU when software rendering is selected or OpenGL
  cannot start. It presents the pixels through a Core Animation layer and keeps
  native text, windows, input, and accessibility. Metal remains
  the default when available. Headless tests use the same renderer selection.
  `QROW_RENDERER` selects `auto`, `metal`, `opengl`, or `software`. An explicit
  value takes precedence over `QROW_SOFTWARE_RENDERER=1`. Forced GPU modes
  report initialization failures. Automatic selection uses Metal when
  available, then OpenGL, then CPU if OpenGL cannot start.
  OpenGL requires hardware Core Profile (OpenGL 3.3+, GLSL 3.30, VAO and
  instanced drawing). It uses an atlas and a native OpenGL layer.
  Renderer fallback is controlled by `QROW_RENDERER` and applies only during
  initialization. Each rectangle or path triangle is one affine instance.
  A rectangle uploads 224 bytes.
  It composes quads, sprites, borders, paths, gradients, patterns, shadows,
  and underlines without full frame CPU readback in window drawing.
  The renderer unit tests run in the full `./qtest run ui` suite.
  `./qtest run software-ui` checks native editor input and result pixels with
  demo data. `./qtest run opengl-renderer opengl-ui` checks actual GPU pixels
  and native window interaction. See the [rendering limits](../../README.md#limitations).

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

Remove each patch when a GPUI release includes that change. Update the crate with
the other GPUI crates as one set.

# GPUI Component patch

This directory contains the `gpui-component` 0.6.6 crate from crates.io. The
source has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes these files:

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
- `src/tooltip.rs`: with the `test-support` feature, a tooltip is the observed
  element `tooltip`, so application tests can find an open tooltip.
- `Cargo.toml`: `cargo machete` ignores the `log` dependency, which the crate
  declares but does not use.

Remove the patch when a GPUI Kit release includes the fixes. Update the crate
with the other GPUI crates as one set.

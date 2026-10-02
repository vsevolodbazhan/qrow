# GPUI Component patch

This directory contains the `gpui-component` 0.6.6 crate from crates.io. The
source has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes these files:

- `src/searchable_list/item.rs`: a row of a Select or Combobox list has 1 pixel
  of space above and below its highlight. Before, the highlights of two
  adjacent rows touched, for example a hovered row below the chosen row. Popup
  menu items have a 2-pixel gap between them. GPUI Component 0.7.0 does not
  have the fix.
- `Cargo.toml`: `cargo machete` ignores the `log` dependency, which the crate
  declares but does not use.

Remove the patch when a GPUI Kit release includes the fix. Update the crate
with the other GPUI crates as one set.

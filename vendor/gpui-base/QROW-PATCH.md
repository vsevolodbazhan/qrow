# GPUI Base patch

This directory contains the `gpui-base` 0.6.6 crate from crates.io. The source
has an Apache 2.0 license. See `LICENSE-APACHE`.

Qrow changes the active line paint width in `src/input/base/element.rs`. The
paint now covers the editor's right padding. Remove this patch when a GPUI Kit
release includes the fix. Update the crate set as one unit.

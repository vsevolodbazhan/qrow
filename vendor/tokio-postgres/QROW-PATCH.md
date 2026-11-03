# Postgres driver patch

Use this patched driver for Postgres streaming exports with bounded metadata.
It is based on tokio-postgres 0.7.18 from crates.io. Preserve the upstream
license files. Cargo selects this copy through `[patch.crates-io]`.

`prepare_bounded` describes a statement without recursive custom-type lookup.
Qrow reads the requested type names through a bounded catalog stream in the
same session. Descriptions retain type OIDs and modifiers. Custom parameter
types in these descriptions are opaque; use the method for metadata only.
A parsed statement owns Close/Sync before description validation. A rejected
description closes its server statement before the session can be reused.

`reset_buffers_when_idle` is an ordered local barrier. It waits for previous
writes, responses, and partial frames, then replaces the empty driver buffers.
Qrow closes previous operations and drops their response owners before this
barrier. The transport reports complete plaintext frame boundaries. The
barrier sends no SQL and cannot pass a later request.

Server schema fields and names have limits before string allocation. Startup
and later session parameters share limits on key count, string size, and total
retained bytes. Type-cache clearing also drops the helper statement owners.
Startup notices have count and field-size limits, including empty notices.

When you update the dependency, apply these changes to the new source and run
the Postgres protocol, Docker connector, and UI suites. Remove the patch when
the upstream driver supplies equivalent bounded metadata and buffer control.

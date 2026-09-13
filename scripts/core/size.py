#!/usr/bin/env python3
"""Check optimized distributable sizes. Never measures debug binaries."""
from pathlib import Path
import os
import sys

root = Path(__file__).resolve().parents[2]
dist = os.environ.get("QROW_DIST_DIR", "dist")
limits = {
    f"{dist}/Qrow.app/Contents/MacOS/qrow": 24 * 1024 * 1024,
    f"{dist}/Qrow-macos.zip": 10 * 1024 * 1024,
}
errors = []
for name, limit in limits.items():
    path = root / name
    if not path.is_file():
        errors.append(f"Missing {name}; run sh scripts/package/macos.sh first.")
        continue
    size = path.stat().st_size
    print(f"{name}: {size / 1024 / 1024:.2f} MiB / {limit / 1024 / 1024:.0f} MiB")
    if size > limit:
        errors.append(f"{name} exceeded its release size budget. Investigate dependencies before changing the limit.")
if errors:
    sys.exit("\n".join(errors))

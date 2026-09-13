#!/usr/bin/env python3
"""Build a padded macOS ICNS file from Qrow's flattened PNG icon."""

from __future__ import annotations

import argparse
import io
from pathlib import Path
import struct

from PIL import Image


CANVAS_SIZE = 1024
INSET = 64
ICNS_VARIANTS = (
    ("icp4", 16),
    ("ic11", 32),
    ("icp5", 32),
    ("ic12", 64),
    ("ic07", 128),
    ("ic13", 256),
    ("ic08", 256),
    ("ic14", 512),
    ("ic09", 512),
    ("ic10", 1024),
)


def build_icon(source: Path, destination: Path) -> None:
    with Image.open(source) as opened:
        image = opened.convert("RGBA")
    if image.size != (CANVAS_SIZE, CANVAS_SIZE):
        raise ValueError(f"Expected a {CANVAS_SIZE}x{CANVAS_SIZE} PNG, got {image.size[0]}x{image.size[1]}")

    icon = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE))
    artwork_size = CANVAS_SIZE - 2 * INSET
    artwork = image.resize((artwork_size, artwork_size), Image.Resampling.LANCZOS)
    icon.alpha_composite(artwork, (INSET, INSET))

    chunks = []
    for chunk_type, size in ICNS_VARIANTS:
        variant = icon.resize((size, size), Image.Resampling.LANCZOS)
        buffer = io.BytesIO()
        variant.save(buffer, format="PNG")
        contents = buffer.getvalue()
        chunks.append(chunk_type.encode("ascii") + struct.pack(">I", len(contents) + 8) + contents)

    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(b"icns" + struct.pack(">I", sum(map(len, chunks)) + 8) + b"".join(chunks))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    build_icon(args.source, args.destination)


if __name__ == "__main__":
    main()

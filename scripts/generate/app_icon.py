#!/usr/bin/env python3
"""Build the small in-application icon from Qrow's flattened PNG icon."""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "assets/app-icons/macos/qrow.png"
DESTINATION = ROOT / "assets/app-icons/qrow-256.png"
# The About dialog draws the icon at 64 points, so 256 pixels covers Retina
# displays and the largest interface scale. The executable embeds this file.
SIZE = 256


def build_asset(source: Path, destination: Path) -> None:
    with Image.open(source) as opened:
        image = opened.convert("RGBA")
    icon = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    destination.parent.mkdir(parents=True, exist_ok=True)
    icon.save(destination, format="PNG", optimize=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, nargs="?", default=SOURCE)
    parser.add_argument("destination", type=Path, nargs="?", default=DESTINATION)
    args = parser.parse_args()
    build_asset(args.source, args.destination)


if __name__ == "__main__":
    main()

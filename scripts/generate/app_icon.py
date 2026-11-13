#!/usr/bin/env python3
"""Build the in-application icon and the Windows icon from Qrow's flattened PNG icon."""

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
WINDOWS_DESTINATION = ROOT / "assets/app-icons/windows/qrow.ico"
# Windows picks the nearest size for the shell, the title bar, the taskbar,
# and Alt+Tab at each display scale.
WINDOWS_SIZES = (16, 20, 24, 32, 40, 48, 64, 256)


def build_asset(source: Path, destination: Path) -> None:
    with Image.open(source) as opened:
        image = opened.convert("RGBA")
    icon = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    destination.parent.mkdir(parents=True, exist_ok=True)
    icon.save(destination, format="PNG", optimize=True)


def build_windows_icon(source: Path, destination: Path) -> None:
    with Image.open(source) as opened:
        image = opened.convert("RGBA")
    # Resize each size from the full source, not from the next larger size.
    frames = [image.resize((size, size), Image.Resampling.LANCZOS) for size in WINDOWS_SIZES]
    destination.parent.mkdir(parents=True, exist_ok=True)
    frames[-1].save(destination, format="ICO", sizes=[(size, size) for size in WINDOWS_SIZES],
                    append_images=frames[:-1])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, nargs="?", default=SOURCE)
    parser.add_argument("destination", type=Path, nargs="?", default=DESTINATION)
    args = parser.parse_args()
    build_asset(args.source, args.destination)
    build_windows_icon(args.source, WINDOWS_DESTINATION)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Build the Qrow installer DMG: branded window, Applications shortcut, and icon layout."""
from __future__ import annotations

import argparse
import math
import plistlib
import subprocess
import tempfile
import time
from pathlib import Path

from ds_store import DSStore
from mac_alias import Alias
from PIL import Image, ImageDraw, ImageFont

# Finder draws the icon labels in black on a picture background in both the
# light and the dark appearance, so the picture is light. The accent is the
# Qrow primary color (src/one-dark.json), darkened to read on light gray.
TOP = (250, 250, 252)
BOTTOM = (234, 237, 242)
ACCENT = (66, 139, 214)
CAPTION_COLOR = (128, 134, 146)

# The Finder window that opens when the installer mounts. It has no toolbar,
# so the content area is the window less the title bar.
WINDOW_X, WINDOW_Y, WINDOW_WIDTH, WINDOW_HEIGHT = 200, 160, 640, 400
TITLE_BAR_HEIGHT = 32
BACKGROUND_SIZE = (WINDOW_WIDTH, WINDOW_HEIGHT - TITLE_BAR_HEIGHT)
# Finder picks the representation that matches the display from one TIFF.
BACKGROUND_SCALES = (1, 2)
BACKGROUND_NAME = "background.tiff"
DETACH_ATTEMPTS = 10
NORMAL_DETACH_ATTEMPTS = 3
APPLICATIONS = "Applications"
ICON_SIZE = 128
# Icon positions in the window content area: Finder reads Iloc as the center
# of the 128x128 icon box, so the boxes below are the centers plus half a box.
APP_POSITION = (170, 150)
APPLICATIONS_POSITION = (470, 150)
CAPTION = "Drag Qrow to Applications to install"
CAPTION_SIZE = 13
CAPTION_WEIGHT = b"Medium"
CAPTION_TOP = 284
FONT_CANDIDATES = (
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
)
# The arrow is a straight shaft with an open head on the line between the
# icon centers. Its mask is drawn large and scaled down for smooth edges.
ARROW_START = (APP_POSITION[0] + ICON_SIZE // 2 + 30, APP_POSITION[1])
ARROW_TIP = (APPLICATIONS_POSITION[0] - ICON_SIZE // 2 - 30, APPLICATIONS_POSITION[1])
ARROW_WIDTH = 3.0
ARROW_HEAD_LENGTH = 12.0
ARROW_HEAD_ANGLE = 42.0
SUPERSAMPLE = 4


def font(size: float) -> ImageFont.FreeTypeFont | None:
    for candidate in FONT_CANDIDATES:
        try:
            loaded = ImageFont.truetype(candidate, round(size))
        except OSError:
            continue
        try:
            loaded.set_variation_by_name(CAPTION_WEIGHT)
        except (OSError, ValueError):
            pass
        return loaded
    return None


def arrow_strokes() -> list[tuple[tuple[float, float], tuple[float, float]]]:
    """Return the shaft and the two strokes of the open head, in points."""
    strokes = [(ARROW_START, ARROW_TIP)]
    for side in (-1, 1):
        angle = math.radians(180.0 + side * ARROW_HEAD_ANGLE)
        end = (
            ARROW_TIP[0] + ARROW_HEAD_LENGTH * math.cos(angle),
            ARROW_TIP[1] + ARROW_HEAD_LENGTH * math.sin(angle),
        )
        strokes.append((ARROW_TIP, end))
    return strokes


def draw_arrow(image: Image.Image, scale: int) -> Image.Image:
    """Draw the arrow that points from the app to the Applications shortcut."""
    width, height = image.size
    factor = scale * SUPERSAMPLE
    mask = Image.new("L", (width * SUPERSAMPLE, height * SUPERSAMPLE), 0)
    draw = ImageDraw.Draw(mask)
    radius = ARROW_WIDTH * factor / 2
    for start, end in arrow_strokes():
        points = [(x * factor, y * factor) for x, y in (start, end)]
        draw.line(points, fill=255, width=round(ARROW_WIDTH * factor))
        # Round caps, so the head strokes meet the shaft in one smooth tip.
        for x, y in points:
            draw.ellipse([x - radius, y - radius, x + radius, y + radius], fill=255)
    mask = mask.resize((width, height), Image.Resampling.LANCZOS)
    return Image.composite(Image.new("RGB", (width, height), ACCENT), image, mask)


def draw_caption(image: Image.Image, scale: int) -> None:
    """Write the hint below the icons when the system has a usable font."""
    text_font = font(CAPTION_SIZE * scale)
    if text_font is None:
        return
    draw = ImageDraw.Draw(image)
    box = draw.textbbox((0, 0), CAPTION, font=text_font)
    center = (APP_POSITION[0] + APPLICATIONS_POSITION[0]) / 2 * scale
    position = (round(center - (box[2] - box[0]) / 2 - box[0]), CAPTION_TOP * scale)
    draw.text(position, CAPTION, font=text_font, fill=CAPTION_COLOR)


def render_background(path: Path, scale: int = 1) -> None:
    """Draw the installer background at a display scale: 1 or 2 pixels per point."""
    width, height = BACKGROUND_SIZE[0] * scale, BACKGROUND_SIZE[1] * scale
    image = Image.new("RGB", (width, height))
    draw = ImageDraw.Draw(image)
    for row in range(height):
        progress = row / max(height - 1, 1)
        color = tuple(round(TOP[i] + (BOTTOM[i] - TOP[i]) * progress) for i in range(3))
        draw.line([(0, row), (width, row)], fill=color)
    image = draw_arrow(image, scale)
    draw_caption(image, scale)
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, format="PNG", dpi=(72 * scale, 72 * scale))


def write_background(path: Path) -> None:
    """Write one TIFF with a representation for each display scale."""
    with tempfile.TemporaryDirectory(prefix="qrow-dmg-background-") as temporary:
        layers = []
        for scale in BACKGROUND_SCALES:
            layer = Path(temporary) / f"background-{scale}x.png"
            render_background(layer, scale)
            layers.append(str(layer))
        path.parent.mkdir(parents=True, exist_ok=True)
        run(["tiffutil", "-cathidpicheck", *layers, "-out", str(path)])


def stage(folder: Path, app: Path) -> str:
    """Fill an empty folder with the app, the Applications shortcut, and the background."""
    # ditto keeps the code signature and the extended attributes of the bundle.
    run(["ditto", str(app), str(folder / app.name)])
    (folder / APPLICATIONS).symlink_to("/Applications")
    write_background(folder / ".background" / BACKGROUND_NAME)
    return app.name


def write_layout(folder: Path, app_name: str, background_alias: bytes) -> None:
    """Write the window, view, and icon layout of the mounted installer volume."""
    origin = f"{WINDOW_X}, {WINDOW_Y}"
    size = f"{WINDOW_WIDTH}, {WINDOW_HEIGHT}"
    window_bounds = "{{" + origin + "}, {" + size + "}}"
    icon_view = {
        "viewOptionsVersion": 1,
        "backgroundType": 2,
        "backgroundColorRed": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorBlue": 1.0,
        "backgroundImageAlias": background_alias,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "gridSpacing": 100.0,
        "arrangeBy": "none",
        "showIconPreview": True,
        "showItemInfo": False,
        "labelOnBottom": True,
        "textSize": 13.0,
        "iconSize": float(ICON_SIZE),
        "scrollPositionX": 0.0,
        "scrollPositionY": 0.0,
    }
    window_view = {
        "ShowStatusBar": False,
        "WindowBounds": window_bounds,
        "ContainerShowSidebar": False,
        "PreviewPaneVisibility": False,
        "SidebarWidth": 0,
        "ShowTabView": False,
        "ShowToolbar": False,
        "ShowPathbar": False,
        "ShowSidebar": False,
    }
    with DSStore.open(str(folder / ".DS_Store"), "w+") as store:
        store["."]["vSrn"] = ("long", 1)
        store["."]["bwsp"] = window_view
        store["."]["icvp"] = icon_view
        store["."]["icvl"] = (b"type", b"icnv")
        store[app_name]["Iloc"] = APP_POSITION
        store[APPLICATIONS]["Iloc"] = APPLICATIONS_POSITION


def build(app: Path, output: Path, volume_name: str) -> None:
    """Package the app into an installer image with a branded Finder window."""
    app = app.resolve()
    if not app.is_dir() or not (app / "Contents" / "Info.plist").is_file():
        raise SystemExit(f"Not an application bundle: {app}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix="qrow-dmg-") as temporary:
        root = Path(temporary)
        staging = root / "staging"
        staging.mkdir()
        app_name = stage(staging, app)
        image = root / "installer.dmg"
        run([
            "hdiutil", "create", "-volname", volume_name, "-srcfolder", str(staging),
            "-format", "UDRW", "-ov", str(image),
        ])
        mount = root / "mount"
        mount.mkdir()
        device, mount = attach(image, mount)
        try:
            background = mount / ".background" / BACKGROUND_NAME
            write_layout(mount, app_name, Alias.for_file(str(background)).to_bytes())
            subprocess.run(["sync"], check=False)
        finally:
            detach(device)
        # The zlib level only costs seconds on a build this size.
        run([
            "hdiutil", "convert", str(image), "-format", "UDZO", "-imagekey", "zlib-level=9",
            "-o", str(output),
        ])
    run(["hdiutil", "imageinfo", str(output)])
    print(f"Built {output} from {app} for volume {volume_name}.")


def attach(image: Path, mount: Path) -> tuple[str, Path]:
    """Attach a writable image and return its device and mount point."""
    result = run([
        "hdiutil", "attach", "-readwrite", "-noverify", "-noautoopen", "-nobrowse",
        "-mountpoint", str(mount), "-plist", str(image),
    ])
    entities = plistlib.loads(result.stdout.encode())["system-entities"]
    mounted = [entity for entity in entities if entity.get("mount-point")]
    if not mounted:
        raise SystemExit(f"Image did not mount: {image}")
    # APFS mounts a synthesized volume on a second disk. Detach the backing
    # image, identified by its partition map, rather than that volume.
    device = next(
        entity["dev-entry"] for entity in entities
        if entity.get("content-hint") in {"GUID_partition_scheme", "Apple_partition_scheme"}
    )
    return device, Path(mounted[-1]["mount-point"])


def detach(device: str) -> None:
    """Detach the image device, with retries while a process holds it."""
    for attempt in range(DETACH_ATTEMPTS):
        command = ["hdiutil", "detach", device]
        if attempt >= NORMAL_DETACH_ATTEMPTS:
            command.insert(2, "-force")
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode == 0:
            return
        # Disk Arbitration can keep an image busy even after a forced eject.
        # Retry only EBUSY, and keep persistent failures fatal to packaging.
        if result.returncode != 16 or attempt == DETACH_ATTEMPTS - 1:
            raise SystemExit(f"Cannot detach {device}: {(result.stderr or result.stdout).strip()}")
        time.sleep(1.0)


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"{command[0]} failed: {(result.stderr or result.stdout).strip()}")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_command = commands.add_parser("build", help="Build the installer DMG")
    build_command.add_argument("--app", type=Path, required=True, help="Application bundle to package")
    build_command.add_argument("--output", type=Path, required=True, help="Path of the DMG to write")
    build_command.add_argument("--volume-name", required=True, help="Name of the mounted installer")
    background_command = commands.add_parser("background", help="Write the background image")
    background_command.add_argument("--output", type=Path, required=True, help="Path of the PNG to write")
    background_command.add_argument(
        "--scale", type=int, choices=BACKGROUND_SCALES, default=2, help="Pixels per point"
    )
    arguments = parser.parse_args()
    if arguments.command == "build":
        build(arguments.app, arguments.output, arguments.volume_name)
    else:
        render_background(arguments.output, arguments.scale)
        print(f"Wrote {arguments.output}.")


if __name__ == "__main__":
    main()

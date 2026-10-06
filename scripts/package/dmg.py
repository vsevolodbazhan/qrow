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
from PIL import Image, ImageDraw, ImageFilter, ImageFont

# The Qrow One Dark palette: src/one-dark.json.
BASE = (30, 32, 36)  # background
DEEP = (21, 23, 27)  # background, lower edge
ACCENT = (97, 175, 239)  # primary
CAPTION_COLOR = (139, 148, 164)  # muted foreground

# The Finder window that opens when the installer mounts.
WINDOW_X, WINDOW_Y, WINDOW_WIDTH, WINDOW_HEIGHT = 120, 120, 720, 440
TOOLBAR_HEIGHT = 52
BACKGROUND_SIZE = (WINDOW_WIDTH, WINDOW_HEIGHT - TOOLBAR_HEIGHT)
BACKGROUND_NAME = "background.png"
APPLICATIONS = "Applications"
ICON_SIZE = 128
# Icon positions in the window content area: Finder reads Iloc as the center
# of the 128x128 icon box, so the boxes below are the centers plus half a box.
APP_POSITION = (220, 116)
APPLICATIONS_POSITION = (500, 116)
# The hint between the two icons.
CAPTION = "Drag to Applications"
CAPTION_SIZE = 15
CAPTION_OFFSET = APP_POSITION[1] + ICON_SIZE // 2 + 64
FONT_CANDIDATES = (
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNS.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
)
ARROW_WIDTH = 6
# The arrow mask is drawn large and scaled down for smooth edges.
ARROW_SCALE = 4
# The hint leaves the app icon downhill, bends at the bottom, and rises onto the
# base line of the head, so the shaft and the head end in one point and one
# direction and nothing sticks out of the head.
ARROW_START = (APP_POSITION[0] + ICON_SIZE // 2 + 8, APP_POSITION[1] - 6)
ARROW_TIP = (APPLICATIONS_POSITION[0] - ICON_SIZE // 2 - 16, APPLICATIONS_POSITION[1])
ARROW_HEAD_ANGLE = -35.0
ARROW_HEAD_LENGTH = 30
ARROW_HEAD_SPREAD = 15
ARROW_LEAVE_ANGLE = 38.0
ARROW_CURVE = (45.0, 55.0)
ARROW_SAMPLES = 64


def font(size: int) -> ImageFont.FreeTypeFont | None:
    for candidate in FONT_CANDIDATES:
        try:
            return ImageFont.truetype(candidate, size)
        except OSError:
            continue
    return None


def bezier(controls: list[tuple[float, float]], step: float) -> tuple[float, float]:
    """Return one point of the curve that the control points describe."""
    points = list(controls)
    while len(points) > 1:
        points = [
            ((1 - step) * start[0] + step * end[0], (1 - step) * start[1] + step * end[1])
            for start, end in zip(points, points[1:])
        ]
    return points[0]


def arrow_geometry() -> tuple[list[tuple[float, float]], list[tuple[float, float]]]:
    """Return the curved shaft and the head triangle of the drag hint."""
    angle = math.radians(ARROW_HEAD_ANGLE)
    axis = (math.cos(angle), math.sin(angle))
    base = (
        ARROW_TIP[0] - ARROW_HEAD_LENGTH * axis[0],
        ARROW_TIP[1] - ARROW_HEAD_LENGTH * axis[1],
    )
    leave = math.radians(ARROW_LEAVE_ANGLE)
    controls = [
        ARROW_START,
        (ARROW_START[0] + ARROW_CURVE[0] * math.cos(leave), ARROW_START[1] + ARROW_CURVE[0] * math.sin(leave)),
        (base[0] - ARROW_CURVE[1] * axis[0], base[1] - ARROW_CURVE[1] * axis[1]),
        base,
    ]
    path = [bezier(controls, step / ARROW_SAMPLES) for step in range(ARROW_SAMPLES + 1)]
    perpendicular = (-axis[1], axis[0])
    head = [
        ARROW_TIP,
        (
            base[0] + ARROW_HEAD_SPREAD * perpendicular[0],
            base[1] + ARROW_HEAD_SPREAD * perpendicular[1],
        ),
        (
            base[0] - ARROW_HEAD_SPREAD * perpendicular[0],
            base[1] - ARROW_HEAD_SPREAD * perpendicular[1],
        ),
    ]
    return path, head


def draw_drag_arrow(image: Image.Image) -> Image.Image:
    """Draw the curved arrow that points from the app to the Applications shortcut."""
    width, height = image.size
    mask = Image.new("L", (width * ARROW_SCALE, height * ARROW_SCALE), 0)
    draw = ImageDraw.Draw(mask)
    path, head = arrow_geometry()
    draw.line(
        [(x * ARROW_SCALE, y * ARROW_SCALE) for x, y in path],
        fill=255,
        width=ARROW_WIDTH * ARROW_SCALE,
        joint="curve",
    )
    draw.polygon([(x * ARROW_SCALE, y * ARROW_SCALE) for x, y in head], fill=255)
    mask = mask.resize((width, height), Image.Resampling.LANCZOS)
    return Image.composite(Image.new("RGB", (width, height), ACCENT), image, mask)


def draw_caption(image: Image.Image) -> None:
    """Write the hint below the icons when the system has a usable font."""
    text_font = font(CAPTION_SIZE)
    if text_font is None:
        return
    draw = ImageDraw.Draw(image)
    box = draw.textbbox((0, 0), CAPTION, font=text_font)
    center = (APP_POSITION[0] + APPLICATIONS_POSITION[0]) // 2
    position = (center - (box[2] - box[0]) // 2 - box[0], CAPTION_OFFSET)
    draw.text(position, CAPTION, font=text_font, fill=CAPTION_COLOR)


def render_background(path: Path) -> None:
    """Draw the installer background in the project colors."""
    width, height = BACKGROUND_SIZE
    image = Image.new("RGB", (width, height))
    draw = ImageDraw.Draw(image)
    for row in range(height):
        progress = row / max(height - 1, 1)
        color = tuple(round(BASE[i] + (DEEP[i] - BASE[i]) * progress) for i in range(3))
        draw.line([(0, row), (width, row)], fill=color)
    glow = Image.new("L", (width, height), 0)
    radius = round(width * 0.42)
    center = (round(width * 0.74), round(height * 0.30))
    ImageDraw.Draw(glow).ellipse(
        [center[0] - radius, center[1] - radius, center[0] + radius, center[1] + radius], fill=110
    )
    glow = glow.filter(ImageFilter.GaussianBlur(radius * 0.55))
    image = Image.composite(Image.new("RGB", (width, height), ACCENT), image, glow)
    image = draw_drag_arrow(image)
    draw_caption(image)
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, format="PNG")


def stage(folder: Path, app: Path) -> str:
    """Fill an empty folder with the app, the Applications shortcut, and the background."""
    # ditto keeps the code signature and the extended attributes of the bundle.
    run(["ditto", str(app), str(folder / app.name)])
    (folder / APPLICATIONS).symlink_to("/Applications")
    render_background(folder / ".background" / BACKGROUND_NAME)
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
        "textSize": 14.0,
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
        "ShowToolbar": True,
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
        device, mount = attach(image)
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


def attach(image: Path) -> tuple[str, Path]:
    """Attach a writable image and return its device and mount point."""
    result = run([
        "hdiutil", "attach", "-readwrite", "-noverify", "-noautoopen", "-plist", str(image),
    ])
    entities = plistlib.loads(result.stdout.encode())["system-entities"]
    mounted = [entity for entity in entities if entity.get("mount-point")]
    if not mounted:
        raise SystemExit(f"Image did not mount: {image}")
    return mounted[-1]["dev-entry"], Path(mounted[-1]["mount-point"])


def detach(device: str) -> None:
    """Detach the image device, with retries while a process holds it."""
    for _ in range(3):
        result = subprocess.run(["hdiutil", "detach", device], capture_output=True, text=True)
        if result.returncode == 0:
            return
        time.sleep(1.0)
    forced = subprocess.run(["hdiutil", "detach", "-force", device], capture_output=True, text=True)
    if forced.returncode != 0:
        raise SystemExit(f"Cannot detach {device}: {(forced.stderr or forced.stdout).strip()}")


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
    arguments = parser.parse_args()
    if arguments.command == "build":
        build(arguments.app, arguments.output, arguments.volume_name)
    else:
        render_background(arguments.output)
        print(f"Wrote {arguments.output}.")


if __name__ == "__main__":
    main()

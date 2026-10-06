"""Check that the installer image gets a branded window and a drag hint."""
import importlib.util
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from PIL import Image

spec = importlib.util.spec_from_file_location(
    "dmg", Path(__file__).resolve().parents[1] / "package/dmg.py"
)
dmg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dmg)

ROOT = Path(__file__).resolve().parents[2]
# The background is drawn from this box, so the caption and the arrow live here.
CAPTION_BOX = (280, 236, 440, 268)
ARROW_BOX = (284, 96, 436, 160)


def fake_bundle(parent: Path) -> Path:
    app = parent / "Qrow.app"
    (app / "Contents" / "MacOS").mkdir(parents=True)
    (app / "Contents" / "Info.plist").write_text("<plist version='1.0'><dict/></plist>\n")
    (app / "Contents" / "MacOS" / "qrow").write_bytes(b"\x00" * 32)
    return app


def color_count(image: Image.Image, box: tuple[int, int, int, int], target, tolerance: int) -> int:
    count = 0
    for y in range(box[1], box[3]):
        for x in range(box[0], box[2]):
            pixel = image.getpixel((x, y))
            if sum(abs(a - b) for a, b in zip(pixel, target)) <= tolerance:
                count += 1
    return count


def records(store_path: Path) -> dict:
    found = {}
    with dmg.DSStore.open(str(store_path), "r") as store:
        for entry in store:
            code = entry.code.decode() if isinstance(entry.code, bytes) else entry.code
            found[(entry.filename, code)] = entry.value
    return found


class BackgroundTests(unittest.TestCase):
    def test_background_is_the_window_size_with_the_drag_arrow(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "background.png"
            dmg.render_background(path)
            image = Image.open(path).convert("RGB")
        self.assertEqual(image.size, dmg.BACKGROUND_SIZE)
        # The top left corner is the plain gradient, away from the glow.
        self.assertLess(sum(abs(a - b) for a, b in zip(image.getpixel((0, 0)), dmg.BASE)), 40)
        # The arrow between the two icons is the accent color.
        self.assertGreater(color_count(image, ARROW_BOX, dmg.ACCENT, 12), 500)

    def test_the_arrow_never_crosses_an_icon_box(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "background.png"
            dmg.render_background(path)
            image = Image.open(path).convert("RGB")
        half = dmg.ICON_SIZE // 2
        for name, position in (("Qrow.app", dmg.APP_POSITION), (dmg.APPLICATIONS, dmg.APPLICATIONS_POSITION)):
            box = (position[0] - half, position[1] - half, position[0] + half, position[1] + half)
            with self.subTest(icon=name):
                self.assertLess(color_count(image, box, dmg.ACCENT, 12), 40)

    def test_background_states_the_drag_hint_when_a_font_exists(self):
        if dmg.font(dmg.CAPTION_SIZE) is None:
            self.skipTest("no font for the drag hint")
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "background.png"
            dmg.render_background(path)
            image = Image.open(path).convert("RGB")
        self.assertGreater(color_count(image, CAPTION_BOX, dmg.CAPTION_COLOR, 30), 20)

    def test_arrow_spans_the_gap_on_the_icon_line(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "background.png"
            dmg.render_background(path)
            image = Image.open(path).convert("RGB")
        accent = [
            (x, y)
            for y in range(image.height)
            for x in range(image.width)
            if sum(abs(a - b) for a, b in zip(image.getpixel((x, y)), dmg.ACCENT)) <= 12
        ]
        self.assertGreater(len(accent), 500)
        half = dmg.ICON_SIZE // 2
        left, right = dmg.APP_POSITION[0] + half, dmg.APPLICATIONS_POSITION[0] - half
        xs = [point[0] for point in accent]
        # The arrow leaves the app icon and reaches for the Applications icon.
        self.assertGreaterEqual(min(xs), left)
        self.assertLessEqual(max(xs), right)
        self.assertLessEqual(min(xs), left + 16)
        self.assertGreaterEqual(max(xs), right - 24)
        # The head aims at the line that connects the two icon centers, even
        # though the shaft bends below it on the way.
        tip = max(accent, key=lambda point: point[0])
        self.assertAlmostEqual(tip[1], dmg.APP_POSITION[1], delta=8)

    def test_the_shaft_ends_on_the_base_of_the_head(self):
        path, head = dmg.arrow_geometry()
        angle = math.radians(dmg.ARROW_HEAD_ANGLE)
        axis = (math.cos(angle), math.sin(angle))
        base = (
            dmg.ARROW_TIP[0] - dmg.ARROW_HEAD_LENGTH * axis[0],
            dmg.ARROW_TIP[1] - dmg.ARROW_HEAD_LENGTH * axis[1],
        )
        # The shaft and the head share one end point, so the curve cannot run
        # out of the head where they meet.
        self.assertAlmostEqual(path[-1][0], base[0], delta=0.5)
        self.assertAlmostEqual(path[-1][1], base[1], delta=0.5)
        previous = path[-2]
        direction = (path[-1][0] - previous[0], path[-1][1] - previous[1])
        length = math.hypot(*direction)
        self.assertAlmostEqual(direction[0] / length, axis[0], delta=0.02)
        self.assertAlmostEqual(direction[1] / length, axis[1], delta=0.02)
        # The head sits on the tip side of its base line, and the shaft stays
        # on the icon side of that line.
        self.assertEqual(len(head), 3)
        self.assertEqual(head[0], dmg.ARROW_TIP)
        edge = (head[1][0] - base[0], head[1][1] - base[1])
        for point in path:
            offset = (point[0] - base[0], point[1] - base[1])
            self.assertGreaterEqual(edge[0] * offset[1] - edge[1] * offset[0], -1.0)


class LayoutTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.folder = Path(self.temporary.name)
        self.background = b"background-alias"
        dmg.write_layout(self.folder, "Qrow.app", self.background)
        self.records = records(self.folder / ".DS_Store")
        self.addCleanup(self.temporary.cleanup)

    def test_the_window_has_no_sidebar_and_fits_the_screen(self):
        window = self.records[(".", "bwsp")]
        self.assertFalse(window["ShowSidebar"])
        self.assertEqual(window["SidebarWidth"], 0)
        self.assertFalse(window["ShowStatusBar"])
        self.assertTrue(window["ShowToolbar"])
        self.assertEqual(
            window["WindowBounds"],
            "{{" + f"{dmg.WINDOW_X}, {dmg.WINDOW_Y}" + "}, {" + f"{dmg.WINDOW_WIDTH}, {dmg.WINDOW_HEIGHT}" + "}}",
        )
        # The background covers the whole content area below the toolbar, so
        # Finder has nothing to scroll.
        self.assertEqual(dmg.BACKGROUND_SIZE, (dmg.WINDOW_WIDTH, dmg.WINDOW_HEIGHT - dmg.TOOLBAR_HEIGHT))

    def test_the_icons_and_the_hint_fit_the_window(self):
        options = self.records[(".", "icvp")]
        self.assertEqual(options["backgroundType"], 2)
        self.assertEqual(options["backgroundImageAlias"], self.background)
        self.assertEqual(options["iconSize"], float(dmg.ICON_SIZE))
        self.assertEqual(options["arrangeBy"], "none")
        self.assertEqual((options["scrollPositionX"], options["scrollPositionY"]), (0.0, 0.0))
        self.assertEqual(self.records[("Qrow.app", "Iloc")], dmg.APP_POSITION)
        self.assertEqual(self.records[(dmg.APPLICATIONS, "Iloc")], dmg.APPLICATIONS_POSITION)
        width, height = dmg.BACKGROUND_SIZE
        half = dmg.ICON_SIZE // 2
        for name, position in (("Qrow.app", dmg.APP_POSITION), (dmg.APPLICATIONS, dmg.APPLICATIONS_POSITION)):
            with self.subTest(icon=name):
                # Finder draws the icon box centered on the stored position.
                self.assertGreaterEqual(position[0] - half, 0)
                self.assertGreaterEqual(position[1] - half, 0)
                self.assertLessEqual(position[0] + half, width)
                self.assertLessEqual(position[1] + half, height)
        # The arrow needs a gap between the two icons, and the hint sits below.
        self.assertGreater(
            dmg.APPLICATIONS_POSITION[0] - half, dmg.APP_POSITION[0] + half
        )
        self.assertLessEqual(dmg.CAPTION_OFFSET + dmg.CAPTION_SIZE, height)

    def test_the_view_is_the_icon_view(self):
        self.assertEqual(self.records[(".", "vSrn")], 1)
        self.assertEqual(self.records[(".", "icvl")], b"icnv")


@unittest.skipUnless(sys.platform == "darwin", "ditto and hdiutil run on macOS")
class ImageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.folder = Path(self.temporary.name)
        self.app = fake_bundle(self.folder)
        self.addCleanup(self.temporary.cleanup)

    def test_stage_adds_the_app_the_shortcut_and_the_background(self):
        staging = self.folder / "staging"
        staging.mkdir()
        self.assertEqual(dmg.stage(staging, self.app), "Qrow.app")
        self.assertTrue((staging / "Qrow.app" / "Contents" / "Info.plist").is_file())
        self.assertEqual((staging / dmg.APPLICATIONS).readlink(), Path("/Applications"))
        with Image.open(staging / ".background" / dmg.BACKGROUND_NAME) as background:
            self.assertEqual(background.size, dmg.BACKGROUND_SIZE)

    def test_build_writes_an_image_with_the_layout(self):
        output = self.folder / "Qrow-test.dmg"
        dmg.build(self.app, output, "Qrow test")
        self.assertGreater(output.stat().st_size, 0)
        mount = self.folder / "mount"
        mount.mkdir()
        subprocess.run(
            ["hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", str(mount), str(output)],
            capture_output=True, check=True,
        )
        try:
            self.assertTrue((mount / "Qrow.app" / "Contents" / "Info.plist").is_file())
            self.assertEqual((mount / dmg.APPLICATIONS).readlink(), Path("/Applications"))
            self.assertTrue((mount / ".background" / dmg.BACKGROUND_NAME).is_file())
            found = records(mount / ".DS_Store")
            self.assertFalse(found[(".", "bwsp")]["ShowSidebar"])
            self.assertEqual(found[("Qrow.app", "Iloc")], dmg.APP_POSITION)
            self.assertEqual(found[(dmg.APPLICATIONS, "Iloc")], dmg.APPLICATIONS_POSITION)
        finally:
            dmg.detach(str(mount))


if __name__ == "__main__":
    unittest.main()

"""Check that the installer image gets a light branded window and a drag hint."""
import importlib.util
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


def render(scale: int = 1) -> Image.Image:
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / "background.png"
        dmg.render_background(path, scale)
        with Image.open(path) as image:
            return image.convert("RGB")


def icon_boxes() -> dict[str, tuple[int, int, int, int]]:
    half = dmg.ICON_SIZE // 2
    return {
        name: (position[0] - half, position[1] - half, position[0] + half, position[1] + half)
        for name, position in (("Qrow.app", dmg.APP_POSITION), (dmg.APPLICATIONS, dmg.APPLICATIONS_POSITION))
    }


def luminance(pixel) -> float:
    return 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]


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
    def test_background_has_a_representation_for_each_display_scale(self):
        for scale in dmg.BACKGROUND_SCALES:
            with self.subTest(scale=scale):
                image = render(scale)
                self.assertEqual(image.size, (dmg.BACKGROUND_SIZE[0] * scale, dmg.BACKGROUND_SIZE[1] * scale))

    def test_background_is_light_so_the_black_icon_labels_stay_readable(self):
        image = render()
        # Finder draws the labels in black below each icon box.
        for name, box in icon_boxes().items():
            label = (box[0], box[3], box[2], box[3] + 24)
            with self.subTest(icon=name):
                darkest = min(
                    luminance(image.getpixel((x, y)))
                    for y in range(label[1], label[3])
                    for x in range(label[0], label[2])
                )
                self.assertGreater(darkest, 200)

    def test_the_arrow_sits_in_the_gap_on_the_icon_line(self):
        image = render()
        accent = [
            (x, y)
            for y in range(image.height)
            for x in range(image.width)
            if sum(abs(a - b) for a, b in zip(image.getpixel((x, y)), dmg.ACCENT)) <= 12
        ]
        self.assertGreater(len(accent), 150)
        boxes = icon_boxes()
        left, right = boxes["Qrow.app"][2], boxes[dmg.APPLICATIONS][0]
        xs = [point[0] for point in accent]
        ys = [point[1] for point in accent]
        # The arrow leaves room on both sides and never touches an icon box.
        self.assertGreater(min(xs), left + 8)
        self.assertLess(max(xs), right - 8)
        # The tip is on the line that connects the two icon centers, and the
        # head is no taller than its two strokes.
        tip = max(accent, key=lambda point: point[0])
        self.assertAlmostEqual(tip[1], dmg.APP_POSITION[1], delta=2)
        self.assertLess(max(ys) - min(ys), 2 * dmg.ARROW_HEAD_LENGTH + dmg.ARROW_WIDTH + 2)

    def test_the_head_strokes_meet_the_shaft_at_the_tip(self):
        shaft, *head = dmg.arrow_strokes()
        self.assertEqual(shaft[1], dmg.ARROW_TIP)
        self.assertEqual(len(head), 2)
        for start, end in head:
            with self.subTest(end=end):
                self.assertEqual(start, dmg.ARROW_TIP)
                # Each stroke points back toward the app and away from the shaft.
                self.assertLess(end[0], dmg.ARROW_TIP[0])
                self.assertNotAlmostEqual(end[1], dmg.ARROW_TIP[1], delta=1)
        self.assertAlmostEqual(head[0][1][1] + head[1][1][1], 2 * dmg.ARROW_TIP[1])

    def test_background_states_the_drag_hint_when_a_font_exists(self):
        if dmg.font(dmg.CAPTION_SIZE) is None:
            self.skipTest("no font for the drag hint")
        image = render()
        center = (dmg.APP_POSITION[0] + dmg.APPLICATIONS_POSITION[0]) // 2
        box = (center - 120, dmg.CAPTION_TOP, center + 120, dmg.CAPTION_TOP + 2 * dmg.CAPTION_SIZE)
        self.assertGreater(color_count(image, box, dmg.CAPTION_COLOR, 30), 20)
        # The hint is centered between the icons.
        columns = [
            x
            for y in range(box[1], box[3])
            for x in range(box[0], box[2])
            if sum(abs(a - b) for a, b in zip(image.getpixel((x, y)), dmg.CAPTION_COLOR)) <= 30
        ]
        self.assertAlmostEqual((min(columns) + max(columns)) / 2, center, delta=3)


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
        self.assertFalse(window["ShowToolbar"])
        self.assertEqual(
            window["WindowBounds"],
            "{{" + f"{dmg.WINDOW_X}, {dmg.WINDOW_Y}" + "}, {" + f"{dmg.WINDOW_WIDTH}, {dmg.WINDOW_HEIGHT}" + "}}",
        )
        # The background covers the whole content area below the title bar,
        # so Finder has nothing to scroll.
        self.assertEqual(dmg.BACKGROUND_SIZE, (dmg.WINDOW_WIDTH, dmg.WINDOW_HEIGHT - dmg.TITLE_BAR_HEIGHT))

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
        # The arrow needs a gap between the two icons, and the hint sits
        # below the labels.
        self.assertGreater(
            dmg.APPLICATIONS_POSITION[0] - half, dmg.APP_POSITION[0] + half
        )
        self.assertGreater(dmg.CAPTION_TOP, dmg.APP_POSITION[1] + half + 40)
        self.assertLessEqual(dmg.CAPTION_TOP + 2 * dmg.CAPTION_SIZE, height)

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
        # One TIFF holds the 1x and the 2x representation for Finder.
        with Image.open(staging / ".background" / dmg.BACKGROUND_NAME) as background:
            sizes = []
            for frame in range(background.n_frames):
                background.seek(frame)
                sizes.append(background.size)
        self.assertEqual(
            sorted(sizes),
            [(dmg.BACKGROUND_SIZE[0] * scale, dmg.BACKGROUND_SIZE[1] * scale) for scale in dmg.BACKGROUND_SCALES],
        )

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

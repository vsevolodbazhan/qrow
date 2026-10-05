"""Check the desktop fixtures against the application's settings contract."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


class AssistantFixtureTests(unittest.TestCase):
    def test_enabled_assistant_fixture_accepts_the_current_sharing_notice(self):
        model = (ROOT / "src/model.rs").read_text()
        driver = (ROOT / "tests/desktop/Driver.swift").read_text()
        current = re.search(r"pub const ASSISTANT_DATA_SHARING_NOTICE_VERSION: u32 = (\d+);", model)
        accepted = re.findall(r'"data_sharing_notice_version": (\d+)', driver)
        self.assertIsNotNone(current)
        self.assertTrue(accepted, "The desktop assistant fixture needs an accepted sharing notice")
        for version in accepted:
            self.assertEqual(version, current.group(1),
                             "Startup disables the assistant when the fixture's sharing notice is stale")


if __name__ == "__main__":
    unittest.main()

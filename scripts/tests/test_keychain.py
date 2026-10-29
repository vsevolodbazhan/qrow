import importlib.util
from pathlib import Path
import unittest
import uuid


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("keychain", ROOT / "scripts/e2e/keychain.py")
keychain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(keychain)


class KeychainTests(unittest.TestCase):
    def profile(self, name):
        return {
            "id": str(uuid.uuid4()),
            "name": name,
            "username": "qrow",
            "host": "127.0.0.1",
        }

    def test_accepts_native_fixture_profiles(self):
        profiles = [self.profile("Qrow E2E")]
        self.assertEqual(
            keychain.fixture_profile_ids(profiles),
            [profile["id"] for profile in profiles],
        )

    def test_rejects_non_fixture_profiles_before_cleanup(self):
        for name in ("User profile", "Qrow E2E copy"):
            with self.subTest(name=name):
                profiles = [self.profile("Qrow E2E"), self.profile(name)]
                with self.assertRaises(ValueError):
                    keychain.fixture_profile_ids(profiles)

    def test_cleans_only_the_desktop_directory_of_a_qtest_run(self):
        target = Path("/repo/target")
        runs = target / "qtest/runs"
        self.assertTrue(keychain.isolated_run(runs / "20260929-030413-94883/desktop", target))
        for path in [runs / "20260929-030413-94883", runs / "latest/desktop", runs / "x/desktop",
                     runs / "20260929-030413-94883/e2e", target / "e2e/qrow-e2e-abc", Path("/tmp/desktop")]:
            with self.subTest(path=path):
                self.assertFalse(keychain.isolated_run(path, target))


if __name__ == "__main__":
    unittest.main()

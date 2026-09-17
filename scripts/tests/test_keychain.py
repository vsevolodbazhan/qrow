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
        profiles = [self.profile(name) for name in ("Qrow E2E", "Qrow E2E copy", "Qrow E2E live")]
        self.assertEqual(
            keychain.fixture_profile_ids(profiles),
            [profile["id"] for profile in profiles],
        )

    def test_rejects_non_fixture_profiles_before_cleanup(self):
        profiles = [self.profile("Qrow E2E"), self.profile("User profile")]
        with self.assertRaises(ValueError):
            keychain.fixture_profile_ids(profiles)


if __name__ == "__main__":
    unittest.main()

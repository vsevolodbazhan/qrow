"""Check that packaging takes a usable version from Cargo metadata."""
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("version", Path(__file__).resolve().parents[1] / "package/version.py")
version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(version)


def metadata(*packages):
    return json.dumps({"packages": [{"name": name, "version": value} for name, value in packages]})


class VersionTests(unittest.TestCase):
    def test_selects_the_application_package(self):
        found = version.version(metadata(("thrift", "0.24.0"), ("qrow", "0.1.0")))
        self.assertEqual(found, "0.1.0")

    def test_missing_package_fails(self):
        with self.assertRaises(ValueError):
            version.version(metadata(("thrift", "0.24.0")))

    def test_version_macos_rejects_fails(self):
        with self.assertRaises(ValueError):
            version.version(metadata(("qrow", "0.2.0-rc1")))

    def test_repository_version_is_accepted(self):
        root = Path(__file__).resolve().parents[2]
        declared = next(
            line.split("=", 1)[1].strip().strip('"')
            for line in (root / "Cargo.toml").read_text().splitlines()
            if line.startswith("version =")
        )
        self.assertEqual(version.version(metadata(("qrow", declared))), declared)

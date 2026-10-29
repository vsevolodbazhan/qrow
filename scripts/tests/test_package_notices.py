"""License notices include the dependencies of the packaged target."""
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/package/notices.py"
ARM64 = "aarch64-apple-darwin"
INTEL = "x86_64-apple-darwin"


class NoticeTests(unittest.TestCase):
    def generate(self, host, target=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "LICENSE").write_text("Synthetic dependency license text")
            output = root / "notices.txt"

            def command(args, **kwargs):
                if args == ["rustc", "-vV"]:
                    return f"rustc test\nhost: {host}\n"
                self.assertEqual(args[:-1], ["cargo", "metadata", "--locked", "--format-version", "1",
                                            "--filter-platform"])
                name = {ARM64: "arm-only", INTEL: "intel-only"}[args[-1]]
                return json.dumps({"resolve": {"nodes": [{"id": name}]}, "packages": [
                    {"id": name, "name": name, "version": "1.0.0", "license": "MIT",
                     "manifest_path": str(root / "Cargo.toml"), "repository": "https://example.test/dependency"}
                ]})

            environment = {} if target is None else {"CARGO_BUILD_TARGET": target}
            with patch.dict(os.environ, environment, clear=True), \
                    patch.object(sys, "argv", [str(SCRIPT), str(output)]), \
                    patch.object(subprocess, "check_output", side_effect=command):
                runpy.run_path(str(SCRIPT), run_name="__main__")
            return output.read_text()

    def test_cross_target_notices_include_target_dependencies_and_license_text(self):
        for host, target, expected, excluded in ((ARM64, INTEL, "intel-only", "arm-only"),
                                                  (INTEL, ARM64, "arm-only", "intel-only")):
            with self.subTest(host=host, target=target):
                notices = self.generate(host, target)
                self.assertIn(f"{expected} 1.0.0", notices)
                self.assertNotIn(excluded, notices)
                self.assertIn("Synthetic dependency license text", notices)

    def test_native_notices_use_host_dependencies_without_an_explicit_target(self):
        notices = self.generate(ARM64)
        self.assertIn("arm-only 1.0.0", notices)
        self.assertNotIn("intel-only", notices)

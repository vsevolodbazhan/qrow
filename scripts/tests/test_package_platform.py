"""Packages select the Cargo target and use the same minimum as the build."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/package/macos_platform.py"
spec = importlib.util.spec_from_file_location("macos_platform", SCRIPT)
macos = importlib.util.module_from_spec(spec)
spec.loader.exec_module(macos)


class PlatformTests(unittest.TestCase):
    def test_minimum_uses_build_config_and_rejects_a_lower_override(self):
        config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
        self.assertEqual(macos.minimum(config), "12.0")
        self.assertEqual(macos.minimum(config, "12"), "12")
        self.assertEqual(macos.minimum(config, "13.5.1"), "13.5.1")
        for value in ("11.9", "0", "12.bad", "12.0.0.0", "12.<", "-12.0"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                macos.minimum(config, value)

    def test_explicit_targets_locate_their_own_executable_and_architecture(self):
        for target, arch in macos.TARGETS.items():
            for profile in ("debug", "release"):
                with self.subTest(target=target, profile=profile):
                    env = dict(os.environ, CARGO_TARGET_DIR="/tmp/qrow-build", QROW_BUILD_PROFILE=profile,
                               CARGO_BUILD_TARGET=target)
                    result = subprocess.run([sys.executable, str(SCRIPT), "executable"], env=env,
                                            text=True, capture_output=True, check=True)
                    self.assertEqual(result.stdout.strip(), f"/tmp/qrow-build/{target}/{profile}/qrow")
                    result = subprocess.run([sys.executable, str(SCRIPT), "architecture"], env=env,
                                            text=True, capture_output=True, check=True)
                    self.assertEqual(result.stdout.strip(), arch)

    def test_native_build_uses_target_root_and_invalid_targets_fail(self):
        self.assertEqual(macos.executable("build", "release"), Path("build/release/qrow"))
        for target in ("x86_64h-apple-darwin", "x86_64-unknown-linux-gnu", ""):
            with self.subTest(target=target), self.assertRaises(ValueError):
                macos.executable("build", "release", target)
        with self.assertRaises(ValueError):
            macos.executable("build", "perf")

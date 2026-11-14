"""Prepared native renderer checks use candidate assets without compiler fallbacks."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PreparedRendererAdapterTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.calls = self.root / "compiler-calls"
        for name in ("swiftc", "cargo", "uv"):
            self.tool(name, 'echo unexpected >> "$QROW_TOOL_CALLS"; exit 99')
        self.tool("uname", "echo Darwin")
        self.tool("xcode-select", "exit 0")
        self.tool("ditto", '''
            mkdir -p "$4/Qrow.app/Contents/MacOS"
            echo plist > "$4/Qrow.app/Contents/Info.plist"
            printf '#!/bin/sh\\nexit 0\\n' > "$4/Qrow.app/Contents/MacOS/qrow"
            chmod +x "$4/Qrow.app/Contents/MacOS/qrow"
        ''')
        self.driver = self.root / "target/e2e-tools/native-driver"
        self.driver.parent.mkdir(parents=True)
        self.driver.write_text("#!/bin/sh\nexit 99\n")
        self.driver.chmod(0o755)
        os.utime(self.driver, (1, 1))  # Older than the checkout: must not rebuild it.
        self.archive = self.root / "candidate.zip"
        self.archive.write_text("candidate passed to synthetic ditto")
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
                        QROW_NATIVE_PREPARED="true", CARGO_TARGET_DIR=str(self.root / "target"),
                        QROW_MACOS_PACKAGE=str(self.archive), QROW_E2E_ARTIFACTS=str(self.root / "artifacts"),
                        QROW_TOOL_CALLS=str(self.calls))
        self.env.pop("QROW_E2E_BUNDLE", None)

    def tool(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def run_adapter(self):
        return subprocess.run(["sh", "scripts/e2e/software.sh", "--prepare"], cwd=ROOT, env=self.env,
                              capture_output=True, text=True, timeout=10)

    def test_prepared_assets_do_not_rebuild_cargo_or_swift(self):
        result = self.run_adapter()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.calls.exists())
        self.assertTrue((self.root / "artifacts/package/Qrow.app/Contents/MacOS/qrow").is_file())

    def test_missing_assets_fail_without_compiler_fallback(self):
        for item, diagnostic in ((self.driver, "Missing prepared native driver"),
                                 (self.archive, "Missing prepared macOS package")):
            with self.subTest(item=item):
                saved = item.read_bytes()
                item.unlink()
                result = self.run_adapter()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(diagnostic, result.stderr)
                self.assertFalse(self.calls.exists())
                item.write_bytes(saved)
                item.chmod(0o755)

    def test_existing_bundle_is_never_overwritten(self):
        bundle = self.root / "artifacts/package/Qrow.app"
        bundle.mkdir(parents=True)
        marker = bundle / "keep"
        marker.write_text("keep")
        result = self.run_adapter()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Refusing to overwrite", result.stderr)
        self.assertEqual(marker.read_text(), "keep")
        self.assertFalse(self.calls.exists())

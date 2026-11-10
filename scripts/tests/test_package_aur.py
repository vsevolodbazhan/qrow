"""Update copies of the AUR package files from a synthetic SHA256SUMS."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/package/aur.py"
X86 = "a" * 64
ARM = "b" * 64


class AurPackageTest(unittest.TestCase):
    def run_update(self, version, sums):
        temporary = Path(self.enterContext(tempfile.TemporaryDirectory()))
        packages = temporary / "aur"
        shutil.copytree(ROOT / "packaging/aur", packages)
        (temporary / "SHA256SUMS").write_text(sums)
        result = subprocess.run([sys.executable, SCRIPT, version, "--sums", temporary / "SHA256SUMS",
                                 "--packages", packages], capture_output=True, text=True)
        return result, packages

    def test_sets_version_release_and_checksums(self):
        sums = (f"{'c' * 64}  ./Qrow-9.8.7-arm64.dmg\n"
                f"{X86}  ./Qrow-9.8.7-linux-x86_64.tar.gz\n"
                f"{ARM}  ./Qrow-9.8.7-linux-aarch64.tar.gz\n")
        result, packages = self.run_update("9.8.7", sums)
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("qrow", "qrow-bin"):
            text = (packages / name / "PKGBUILD").read_text()
            self.assertIn("\npkgver=9.8.7\n", text)
            self.assertIn("\npkgrel=1\n", text)
        binary = (packages / "qrow-bin/PKGBUILD").read_text()
        self.assertIn(f"sha256sums_x86_64=('{X86}')", binary)
        self.assertIn(f"sha256sums_aarch64=('{ARM}')", binary)
        self.assertIn("sha256sums=('SKIP')", (packages / "qrow/PKGBUILD").read_text())

    def test_rejects_missing_archive_and_nightly_version(self):
        result, packages = self.run_update("9.8.7", f"{X86}  ./Qrow-9.8.7-linux-x86_64.tar.gz\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("aarch64", result.stderr)
        self.assertEqual((packages / "qrow/PKGBUILD").read_text(), (ROOT / "packaging/aur/qrow/PKGBUILD").read_text())
        result, _ = self.run_update("0.4.0-nightly.20261109.44", "")
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()

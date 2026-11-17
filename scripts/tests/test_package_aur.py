"""Write the AUR PKGBUILD from a synthetic SHA256SUMS."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/package/aur.py"
TEMPLATE = ROOT / "packaging/aur/qrow-bin/PKGBUILD"
X86 = "a" * 64
ARM = "b" * 64


class AurPackageTest(unittest.TestCase):
    def run_script(self, version, sums):
        temporary = Path(self.enterContext(tempfile.TemporaryDirectory()))
        (temporary / "SHA256SUMS").write_text(sums)
        output = temporary / "aur"
        result = subprocess.run([sys.executable, SCRIPT, version, "--sums", temporary / "SHA256SUMS",
                                 "--output", output], capture_output=True, text=True)
        return result, output / "PKGBUILD"

    def test_sets_version_release_and_checksums(self):
        sums = (f"{'c' * 64}  ./Qrow-9.8.7-arm64.dmg\n"
                f"{X86}  ./Qrow-9.8.7-linux-x86_64.tar.gz\n"
                f"{ARM}  ./Qrow-9.8.7-linux-aarch64.tar.gz\n"
                f"{'d' * 64}  ./Qrow-9.8.7-windows-x86_64.zip\n")
        result, build = self.run_script("9.8.7", sums)
        self.assertEqual(result.returncode, 0, result.stderr)
        text = build.read_text()
        self.assertIn("\npkgver=9.8.7\n", text)
        self.assertIn("\npkgrel=1\n", text)
        self.assertIn(f"\nsha256sums_x86_64=('{X86}')\n", text)
        self.assertIn(f"\nsha256sums_aarch64=('{ARM}')\n", text)
        # Only the version and the checksums change.
        template = TEMPLATE.read_text()
        changed = {line for line in text.splitlines()} - set(template.splitlines())
        self.assertEqual(len(changed), 3)

    def test_rejects_missing_archive_and_nightly_version(self):
        result, build = self.run_script("9.8.7", f"{X86}  ./Qrow-9.8.7-linux-x86_64.tar.gz\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("aarch64", result.stderr)
        self.assertFalse(build.exists())
        nightly = "0.6.0-nightly.20261116.63"
        sums = (f"{X86}  ./Qrow-{nightly}-linux-x86_64.tar.gz\n"
                f"{ARM}  ./Qrow-{nightly}-linux-aarch64.tar.gz\n")
        result, build = self.run_script(nightly, sums)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("stable version", result.stderr)
        self.assertFalse(build.exists())

    def test_template_installs_the_files_of_the_linux_archive(self):
        template = TEMPLATE.read_text()
        archive = (ROOT / "scripts/package/linux.sh").read_text()
        for path in re.findall(r"install -Dm\d+ (\S+?)(?:/\*)? ", template):
            self.assertIn(f'"$qrow_package/{path}', archive)
        for architecture in ("x86_64", "aarch64"):
            self.assertIn(f'source_{architecture}=("$_release/Qrow-$pkgver-linux-{architecture}.tar.gz")', template)


if __name__ == "__main__":
    unittest.main()

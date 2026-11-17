"""Write the AUR PKGBUILDs from a synthetic SHA256SUMS."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/package/aur.py"
TEMPLATE = ROOT / "packaging/aur/PKGBUILD"
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

    @staticmethod
    def sums(version):
        return (f"{'c' * 64}  ./Qrow-{version}-arm64.dmg\n"
                f"{X86}  ./Qrow-{version}-linux-x86_64.tar.gz\n"
                f"{ARM}  ./Qrow-{version}-linux-aarch64.tar.gz\n"
                f"{'d' * 64}  ./Qrow-{version}-windows-x86_64.zip\n")

    def test_stable_release_updates_qrow_bin(self):
        result, build = self.run_script("9.8.7", self.sums("9.8.7"))
        self.assertEqual(result.returncode, 0, result.stderr)
        text = build.read_text()
        self.assertIn("\npkgname=qrow-bin\n", text)
        self.assertIn("\npkgdesc='Desktop SQL workbench (release build)'\n", text)
        self.assertIn("\n_version=9.8.7\n", text)
        self.assertIn("\npkgver=9.8.7\n", text)
        self.assertIn("\npkgrel=1\n", text)
        self.assertIn(f"\nsha256sums_x86_64=('{X86}')\n", text)
        self.assertIn(f"\nsha256sums_aarch64=('{ARM}')\n", text)
        # Only the versions and the checksums change.
        changed = set(text.splitlines()) - set(TEMPLATE.read_text().splitlines())
        self.assertEqual(len(changed), 4)

    def test_nightly_release_updates_qrow_nightly_bin_without_a_hyphen_in_pkgver(self):
        nightly = "0.6.0-nightly.20261116.63"
        result, build = self.run_script(nightly, self.sums(nightly))
        self.assertEqual(result.returncode, 0, result.stderr)
        text = build.read_text()
        self.assertIn("\npkgname=qrow-nightly-bin\n", text)
        self.assertIn("\npkgdesc='Desktop SQL workbench (nightly build)'\n", text)
        # The download URL uses the release version; pacman gets the version without hyphens.
        self.assertIn(f"\n_version={nightly}\n", text)
        self.assertIn("\npkgver=0.6.0.nightly.20261116.63\n", text)
        self.assertIn(f"\nsha256sums_x86_64=('{X86}')\n", text)

    def test_rejects_missing_archive_and_unknown_version(self):
        result, build = self.run_script("9.8.7", f"{X86}  ./Qrow-9.8.7-linux-x86_64.tar.gz\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("aarch64", result.stderr)
        self.assertFalse(build.exists())
        for version in ("0.6.0-rc.1", "0.6.0-nightly.2026111.63", "0.6"):
            result, build = self.run_script(version, self.sums(version))
            self.assertNotEqual(result.returncode, 0, version)
            self.assertIn("stable or nightly", result.stderr)
            self.assertFalse(build.exists())

    def test_template_installs_the_files_of_the_linux_archive(self):
        template = TEMPLATE.read_text()
        archive = (ROOT / "scripts/package/linux.sh").read_text()
        for path in re.findall(r"install -Dm\d+ (\S+?)(?:/\*)? ", template):
            self.assertIn(f'"$qrow_package/{path}', archive)
        for architecture in ("x86_64", "aarch64"):
            self.assertIn(f'source_{architecture}=("$_release/Qrow-$_version-linux-{architecture}.tar.gz")', template)


if __name__ == "__main__":
    unittest.main()

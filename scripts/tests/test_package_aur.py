"""Check the AUR packages that the release publishes."""
import importlib.util
import io
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("aur", Path(__file__).resolve().parents[1] / "package/aur.py")
aur = importlib.util.module_from_spec(spec)
spec.loader.exec_module(aur)

NIGHTLY = "0.4.0-nightly.20261107.34"
FILES = (
    "bin/qrow",
    "share/applications/qrow.desktop",
    "share/icons/hicolor/256x256/apps/qrow.png",
    "share/licenses/qrow/LICENSE",
)


def synthetic_archives(dist: Path, version: str) -> None:
    for arch in aur.ARCHES:
        with tarfile.open(dist / aur.archive(version, arch), "w:gz") as archive:
            for name in FILES:
                data = f"synthetic {name} for {arch}".encode()
                member = tarfile.TarInfo(f"./{name}")
                member.size = len(data)
                member.mode = 0o755 if name.startswith("bin/") else 0o644
                archive.addfile(member, io.BytesIO(data))


class AurPackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()

    def test_each_channel_has_its_package_and_the_archive_checksums(self):
        for channel, version, pkgver in (("stable", "0.4.0", "0.4.0"), ("nightly", NIGHTLY, "0.4.0.nightly.20261107.34")):
            synthetic_archives(self.dist, version)
            directory = aur.write(channel, version, self.dist, self.root / "aur")
            pkgname = aur.PACKAGES[channel]
            self.assertEqual(directory.name, pkgname)
            info = (directory / ".SRCINFO").read_text()
            self.assertIn(f"pkgbase = {pkgname}\n", info)
            self.assertIn(f"\tpkgver = {pkgver}\n", info)
            for arch in aur.ARCHES:
                digest = aur.sha256(self.dist / aur.archive(version, arch))
                self.assertIn(f"\tsha256sums_{arch} = {digest}\n", info)
                self.assertIn(f"/releases/download/v{version}/Qrow-{version}-linux-{arch}.tar.gz\n", info)
            # Every package that installs /usr/bin/qrow conflicts, except the package itself.
            self.assertNotIn(f"\tconflicts = {pkgname}\n", info)
            for other in ("qrow", "qrow-bin", *(name for name in aur.PACKAGES.values() if name != pkgname)):
                self.assertIn(f"\tconflicts = {other}\n", info)

    def test_missing_archives_and_invalid_versions_fail(self):
        synthetic_archives(self.dist, "0.4.0")
        (self.dist / aur.archive("0.4.0", "aarch64")).unlink()
        with self.assertRaisesRegex(ValueError, "Missing Linux archive"):
            aur.write("stable", "0.4.0", self.dist, self.root / "aur")
        with self.assertRaisesRegex(ValueError, "Invalid release version"):
            aur.write("stable", "0.4.0;rm", self.dist, self.root / "aur")
        with self.assertRaisesRegex(ValueError, "Unknown release channel"):
            aur.write("beta", "0.4.0", self.dist, self.root / "aur")

    @unittest.skipUnless(shutil.which("makepkg"), "needs makepkg from Arch Linux")
    def test_makepkg_reads_the_same_srcinfo_and_installs_the_archive(self):
        synthetic_archives(self.dist, NIGHTLY)
        directory = aur.write("nightly", NIGHTLY, self.dist, self.root / "aur")
        printed = subprocess.run(
            ["makepkg", "--printsrcinfo"], cwd=directory, check=True, capture_output=True, text=True
        ).stdout
        self.assertEqual(printed, (directory / ".SRCINFO").read_text())
        machine = subprocess.run(["uname", "-m"], check=True, capture_output=True, text=True).stdout.strip()
        if machine not in aur.ARCHES:
            self.skipTest(f"no archive for {machine}")
        # makepkg uses a source file that is already present instead of downloading it.
        shutil.copy(self.dist / aur.archive(NIGHTLY, machine), directory)
        subprocess.run(
            ["makepkg", "--force", "--nodeps", "--noconfirm"],
            cwd=directory,
            check=True,
            capture_output=True,
            env={"PATH": "/usr/bin:/bin", "PKGDEST": str(directory), "SRCDEST": str(directory),
                 "BUILDDIR": str(self.root / "build"), "HOME": str(self.root)},
        )
        package = next(directory.glob("qrow-desktop-nightly-bin-*.pkg.tar.*"))
        listed = subprocess.run(["bsdtar", "-tf", package], check=True, capture_output=True, text=True).stdout
        for path in ("usr/bin/qrow", "usr/share/applications/qrow.desktop",
                     "usr/share/icons/hicolor/256x256/apps/qrow.png",
                     "usr/share/licenses/qrow-desktop-nightly-bin/LICENSE"):
            self.assertIn(path, listed.splitlines())


if __name__ == "__main__":
    unittest.main()

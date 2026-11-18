"""Write the AUR PKGBUILDs from synthetic release data, and push them to a local repository."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/package/aur.py"
PUSH = ROOT / "scripts/package/aur-push.sh"
BINARY = ROOT / "packaging/aur/bin/PKGBUILD"
SOURCE = ROOT / "packaging/aur/source/PKGBUILD"
X86 = "a" * 64
ARM = "b" * 64
COMMIT = "c" * 40
ARCHIVE = "d" * 64


def sums(version):
    return (f"{'e' * 64}  ./Qrow-{version}-arm64.dmg\n"
            f"{X86}  ./Qrow-{version}-linux-x86_64.tar.gz\n"
            f"{ARM}  ./Qrow-{version}-linux-aarch64.tar.gz\n"
            f"{'f' * 64}  ./Qrow-{version}-windows-x86_64.zip\n")


def field(text, name):
    return re.search(rf"^{name}=(.*)$", text, re.MULTILINE).group(1)


class AurPackageTest(unittest.TestCase):
    def run_script(self, version, *arguments, sums_text=None):
        temporary = Path(self.enterContext(tempfile.TemporaryDirectory()))
        output = temporary / "aur"
        command = [sys.executable, SCRIPT, version, "--output", output, *arguments]
        if sums_text is not None:
            (temporary / "SHA256SUMS").write_text(sums_text)
            command += ["--sums", temporary / "SHA256SUMS"]
        result = subprocess.run(command, capture_output=True, text=True)
        return result, output / "PKGBUILD"

    def test_stable_release_updates_qrow_bin(self):
        result, build = self.run_script("9.8.7", sums_text=sums("9.8.7"))
        self.assertEqual(result.returncode, 0, result.stderr)
        text = build.read_text()
        self.assertEqual(field(text, "pkgname"), "qrow-bin")
        self.assertEqual(field(text, "pkgdesc"), "'Desktop SQL workbench (release build)'")
        self.assertEqual(field(text, "_version"), "9.8.7")
        self.assertEqual(field(text, "pkgver"), "9.8.7")
        self.assertEqual(field(text, "pkgrel"), "1")
        self.assertEqual(field(text, "sha256sums_x86_64"), f"('{X86}')")
        self.assertEqual(field(text, "sha256sums_aarch64"), f"('{ARM}')")
        # Only the versions and the checksums change.
        changed = set(text.splitlines()) - set(BINARY.read_text().splitlines())
        self.assertEqual(len(changed), 4)

    def test_nightly_release_updates_qrow_nightly_bin_without_a_hyphen_in_pkgver(self):
        nightly = "0.6.0-nightly.20261116.63"
        result, build = self.run_script(nightly, sums_text=sums(nightly))
        self.assertEqual(result.returncode, 0, result.stderr)
        text = build.read_text()
        self.assertEqual(field(text, "pkgname"), "qrow-nightly-bin")
        self.assertEqual(field(text, "pkgdesc"), "'Desktop SQL workbench (nightly build)'")
        # The download URL uses the release version; pacman gets the version without hyphens.
        self.assertEqual(field(text, "_version"), nightly)
        self.assertEqual(field(text, "pkgver"), "0.6.0.nightly.20261116.63")
        self.assertEqual(field(text, "sha256sums_x86_64"), f"('{X86}')")

    def test_stable_release_updates_the_qrow_source_package(self):
        result, build = self.run_script("9.8.7", "--package", "source", "--commit", COMMIT,
                                        "--source-sha256", ARCHIVE)
        self.assertEqual(result.returncode, 0, result.stderr)
        text = build.read_text()
        self.assertEqual(field(text, "pkgname"), "qrow")
        self.assertEqual(field(text, "pkgver"), "9.8.7")
        self.assertEqual(field(text, "_commit"), COMMIT)
        self.assertEqual(field(text, "sha256sums"), f"('{ARCHIVE}')")
        changed = set(text.splitlines()) - set(SOURCE.read_text().splitlines())
        self.assertEqual(len(changed), 3)

    def test_rejects_bad_versions_and_inputs(self):
        result, build = self.run_script("9.8.7", sums_text=f"{X86}  ./Qrow-9.8.7-linux-x86_64.tar.gz\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("aarch64", result.stderr)
        self.assertFalse(build.exists())
        for version in ("0.6.0-rc.1", "0.6.0-nightly.2026111.63", "0.6"):
            result, build = self.run_script(version, sums_text=sums(version))
            self.assertNotEqual(result.returncode, 0, version)
            self.assertIn("stable or nightly", result.stderr)
            self.assertFalse(build.exists())
        source = ("--package", "source", "--commit", COMMIT, "--source-sha256", ARCHIVE)
        result, build = self.run_script("0.6.0-nightly.20261116.63", *source)
        self.assertIn("Only stable releases", result.stderr)
        self.assertFalse(build.exists())
        result, build = self.run_script("9.8.7", "--package", "source", "--commit", "abc1234",
                                        "--source-sha256", ARCHIVE)
        self.assertIn("full SHA", result.stderr)
        self.assertFalse(build.exists())

    def test_packages_install_the_same_files_and_need_the_same_libraries(self):
        binary, source = BINARY.read_text(), SOURCE.read_text()
        for name in ("depends", "optdepends", "arch", "url", "license"):
            pattern = rf"^{name}=(\(.*?\)|[^\n]*)$"
            self.assertEqual(re.search(pattern, binary, re.M | re.S).group(1),
                             re.search(pattern, source, re.M | re.S).group(1), name)
        installed = re.compile(r'-t "\$pkgdir(/usr/\S+?)"|"\$pkgdir(/usr/\S+?)/[^/"]+"')
        self.assertEqual({a or b for a, b in installed.findall(binary)},
                         {a or b for a, b in installed.findall(source)})
        # The binary package takes the files of the Linux archive; the source package, those of the archive script.
        archive = (ROOT / "scripts/package/linux.sh").read_text()
        for path in re.findall(r"install -Dm\d+ (\S+?)(?:/\*)? ", binary):
            self.assertIn(f'"$qrow_package/{path}', archive)
        self.assertIn("cp assets/linux/qrow.desktop ", archive)
        self.assertIn("install -Dm644 assets/linux/qrow.desktop ", source)
        self.assertIn("cp assets/app-icons/qrow-256.png ", archive)
        self.assertIn("install -Dm644 assets/app-icons/qrow-256.png ", source)
        for architecture in ("x86_64", "aarch64"):
            self.assertIn(f'source_{architecture}=("$_release/Qrow-$_version-linux-{architecture}.tar.gz")', binary)
        # A source build must not update itself: pacman updates it.
        self.assertNotIn("QROW_RELEASE_VERSION=", source)
        # Each package that installs /usr/bin/qrow conflicts with the others through the qrow name.
        self.assertIn('provides=("$_pkgname=$pkgver")\nconflicts=("$_pkgname")', binary)


@unittest.skipIf(shutil.which("git") is None, "git is not installed")
class AurPushTest(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.remote = self.root / "remote.git"
        subprocess.run(["git", "init", "--quiet", "--bare", "--initial-branch=master", self.remote], check=True)
        # The history of an earlier owner of the name, with a file that the new package does not have.
        seed = self.root / "seed"
        subprocess.run(["git", "clone", "--quiet", self.remote, seed], check=True, capture_output=True)
        (seed / "PKGBUILD").write_text("pkgname=qrow-bin\npkgver=1.1.1\n")
        (seed / "old.install").write_text("post_install() { :; }\n")
        self.git(seed, "add", "--all")
        self.git(seed, "-c", "user.name=Earlier", "-c", "user.email=earlier@example.invalid",
                 "commit", "--quiet", "--message", "Update to 1.1.1")
        self.git(seed, "push", "--quiet", "origin", "HEAD:master")
        self.package = self.root / "aur"
        self.package.mkdir()
        (self.package / "PKGBUILD").write_text("pkgname=qrow-bin\npkgver=9.8.7\n")
        (self.package / ".SRCINFO").write_text("pkgbase = qrow-bin\n\tpkgver = 9.8.7\n\npkgname = qrow-bin\n")

    @staticmethod
    def git(directory, *arguments):
        return subprocess.run(["git", "-C", directory, *arguments], check=True, capture_output=True, text=True).stdout

    def push(self, key="synthetic key"):
        environment = dict(os.environ, AUR_SSH_PRIVATE_KEY=key, RELEASE_VERSION="9.8.7", AUR_REMOTE=str(self.remote),
                           TMPDIR=str(self.root))
        return subprocess.run(["sh", PUSH, self.package], capture_output=True, text=True, env=environment)

    def test_adds_one_commit_on_top_of_the_history_and_a_rerun_changes_nothing(self):
        result = self.push()
        self.assertEqual(result.returncode, 0, result.stderr)
        log = self.git(self.remote, "log", "--format=%s").splitlines()
        self.assertEqual(log, ["Update to 9.8.7", "Update to 1.1.1"])
        self.assertEqual(self.git(self.remote, "ls-tree", "--name-only", "HEAD").split(), [".SRCINFO", "PKGBUILD"])
        result = self.push()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("qrow-bin already has Qrow 9.8.7", result.stdout)
        self.assertEqual(len(self.git(self.remote, "log", "--format=%s").splitlines()), 2)
        # The key and the clone do not remain.
        self.assertEqual(sorted(path.name for path in self.root.iterdir()), ["aur", "remote.git", "seed"])

    def test_refuses_a_missing_key_and_an_unknown_package(self):
        result = self.push(key="")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("AUR_SSH_PRIVATE_KEY", result.stderr)
        (self.package / ".SRCINFO").write_text("pkgbase = other\n")
        result = self.push()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unexpected AUR package: other", result.stderr)
        self.assertEqual(len(self.git(self.remote, "log", "--format=%s").splitlines()), 1)


if __name__ == "__main__":
    unittest.main()

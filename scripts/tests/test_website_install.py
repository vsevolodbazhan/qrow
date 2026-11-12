"""Run the website install script against a synthetic local release."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "website/install.sh"
VERSION = "9.8.7"


def archive(binary):
    """A release archive with the layout of scripts/package/linux.sh."""
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode="w:gz") as bundle:
        for name, content, mode in (("bin/qrow", binary, 0o755),
                                    ("share/applications/qrow.desktop", b"[Desktop Entry]\nName=Qrow\n", 0o644),
                                    ("share/icons/hicolor/256x256/apps/qrow.png", b"synthetic icon", 0o644)):
            member = tarfile.TarInfo(f"./{name}")
            member.size, member.mode = len(content), mode
            bundle.addfile(member, io.BytesIO(content))
    return data.getvalue()


class WebsiteInstallScriptTest(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.root)], check=False)
        self.release = self.root / "release"
        self.release.mkdir()
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.prefix = self.root / "home/.local"
        self.publish({"x86_64": archive(b"qrow x86_64"), "aarch64": archive(b"qrow aarch64")})
        self.machine("x86_64")
        # A stand-in for minisign records its arguments, so the test does not depend on the real tool.
        self.tool("minisign", '#!/bin/sh\nprintf "%s\\n" "$@" > "$QROW_TEST_LOG"\n')

    def tool(self, name, source):
        path = self.tools / name
        path.write_text(source)
        path.chmod(0o755)

    def machine(self, arch):
        self.tool("uname", f'#!/bin/sh\ncase "$1" in -m) echo {arch};; -s) echo Linux;; esac\n')

    def publish(self, archives, sums=None):
        lines = []
        for arch, content in archives.items():
            name = f"Qrow-{VERSION}-linux-{arch}.tar.gz"
            (self.release / name).write_bytes(content)
            digest = (sums or {}).get(arch, hashlib.sha256(content).hexdigest())
            lines.append(f"{digest}  ./{name}")
        lines.append(f"{'0' * 64}  ./Qrow-{VERSION}-arm64.dmg")
        (self.release / "SHA256SUMS").write_text("\n".join(lines) + "\n")
        (self.release / "SHA256SUMS.minisig").write_text("synthetic signature\n")

    def install(self):
        env = dict(os.environ, PATH=f"{self.tools}:{os.environ['PATH']}", HOME=str(self.root / "home"),
                   QROW_DOWNLOAD_URL=self.release.as_uri(), QROW_TEST_LOG=str(self.root / "minisign.log"))
        env.pop("QROW_PREFIX", None)
        return subprocess.run(["sh", str(SCRIPT)], env=env, capture_output=True, text=True, check=False)

    def test_installs_the_archive_of_this_processor_into_local(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.prefix / "bin/qrow").read_bytes(), b"qrow x86_64")
        self.assertTrue(os.access(self.prefix / "bin/qrow", os.X_OK))
        # Self-update recognizes an archive install by the launcher in a writable prefix.
        self.assertTrue((self.prefix / "share/applications/qrow.desktop").is_file())
        self.assertTrue((self.prefix / "share/icons/hicolor/256x256/apps/qrow.png").is_file())
        self.assertEqual(list(self.prefix.glob(".qrow-install-*")), [])
        self.assertIn(f"Installed Qrow {VERSION} in {self.prefix}", result.stdout)
        self.assertIn(f"Add {self.prefix}/bin to your PATH", result.stdout)

    def test_replaces_an_installed_copy(self):
        self.assertEqual(self.install().returncode, 0)
        self.machine("aarch64")
        self.publish({"aarch64": archive(b"qrow update")})
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.prefix / "bin/qrow").read_bytes(), b"qrow update")

    def test_verifies_the_checksums_with_the_release_key(self):
        self.assertEqual(self.install().returncode, 0)
        arguments = (self.root / "minisign.log").read_text().split()
        key = (ROOT / "assets/release/minisign.pub").read_text().splitlines()[1]
        self.assertIn("-V", arguments)
        self.assertEqual(arguments[arguments.index("-P") + 1], key)
        self.assertTrue(arguments[arguments.index("-m") + 1].endswith("/SHA256SUMS"))

    def test_stops_on_a_wrong_checksum_or_signature(self):
        self.publish({"x86_64": archive(b"qrow x86_64")}, sums={"x86_64": "f" * 64})
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr)
        self.assertFalse((self.prefix / "bin/qrow").exists())

        self.publish({"x86_64": archive(b"qrow x86_64")})
        self.tool("minisign", "#!/bin/sh\nexit 1\n")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signature is not valid", result.stderr)
        self.assertFalse((self.prefix / "bin/qrow").exists())

    def test_stops_on_an_unsupported_system_or_processor(self):
        self.machine("riscv64")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no Qrow build for riscv64", result.stderr)
        self.tool("uname", '#!/bin/sh\ncase "$1" in -m) echo arm64;; -s) echo Darwin;; esac\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("installs Qrow on Linux", result.stderr)
        self.assertFalse(self.prefix.exists())


if __name__ == "__main__":
    unittest.main()

"""Run the website install script against a synthetic local release."""
import base64
import fcntl
import hashlib
import io
import os
from pathlib import Path
import select
import shutil
import subprocess
import tarfile
import tempfile
import termios
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "website/install.sh"
VERSION = "9.8.7"
# The commands that the script needs. Each test gets only these, so the tools
# that verify signatures are there only when a test adds them.
COMMANDS = ("base64", "cat", "cp", "curl", "cut", "dirname", "find", "grep", "gzip", "head", "mkdir", "mktemp",
            "mv", "od", "rm", "sed", "tail", "tar", "wc")
KEY_ID = b"qrowtest"


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


def openssl3():
    """An OpenSSL 3 executable, which can sign and verify Ed25519 data, or None."""
    for candidate in (shutil.which("openssl"), "/opt/homebrew/opt/openssl@3/bin/openssl",
                      "/usr/local/opt/openssl@3/bin/openssl"):
        if candidate and os.path.exists(candidate):
            version = subprocess.run([candidate, "version"], capture_output=True, text=True, check=False).stdout
            if version.startswith("OpenSSL 3"):
                return candidate
    return None


class WebsiteInstallScriptTest(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.release = self.root / "release"
        self.release.mkdir()
        self.tools = self.root / "tools"
        self.tools.mkdir()
        for name in COMMANDS:
            os.symlink(shutil.which(name), self.tools / name)
        checksum = shutil.which("sha256sum") or shutil.which("shasum")
        os.symlink(checksum, self.tools / Path(checksum).name)
        self.prefix = self.root / "home/.local"
        self.script = SCRIPT
        self.signature = "untrusted comment: synthetic\nAAAA\ntrusted comment: Qrow release v{version}\nAAAA\n"
        self.publish({"x86_64": archive(b"qrow x86_64"), "aarch64": archive(b"qrow aarch64")})
        self.machine("x86_64")
        # A stand-in for minisign records its arguments, so the test does not depend on the real tool.
        self.tool("minisign", '#!/bin/sh\n[ "$1" = -v ] && exit 0\nprintf "%s\\n" "$@" > "$QROW_TEST_LOG"\n')

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
        (self.release / "SHA256SUMS.minisig").write_text(self.signature.format(version=VERSION))

    def environment(self):
        env = dict(PATH=str(self.tools), HOME=str(self.root / "home"), QROW_DOWNLOAD_URL=self.release.as_uri(),
                   QROW_TEST_LOG=str(self.root / "minisign.log"))
        return env

    def install(self):
        # A new session has no terminal, so the script cannot ask on the terminal of the test run.
        return subprocess.run(["/bin/sh", str(self.script)], env=self.environment(), stdin=subprocess.DEVNULL,
                              capture_output=True, text=True, check=False, start_new_session=True, timeout=120)

    def install_on_terminal(self, answer):
        """Runs the script with a pseudo-terminal that answers its question."""
        controller, terminal = os.openpty()

        def attach():
            os.setsid()
            fcntl.ioctl(terminal, termios.TIOCSCTTY, 0)

        process = subprocess.Popen(["/bin/sh", str(self.script)], env=self.environment(), stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                   preexec_fn=attach, pass_fds=(terminal,))
        os.close(terminal)
        # Answer when the question shows, as a person does. Some systems drop
        # input that arrives before the script opens the terminal.
        shown, answered = b"", False
        deadline = time.monotonic() + 120
        while process.poll() is None or select.select([controller], [], [], 0)[0]:
            if time.monotonic() > deadline:
                process.kill()
                self.fail(f"the script did not finish; the terminal shows: {shown.decode()!r}")
            if not select.select([controller], [], [], 0.1)[0]:
                continue
            try:
                data = os.read(controller, 4096)
            except OSError:
                break
            if not data:
                break
            shown += data
            if not answered and b"[Y/n]" in shown:
                os.write(controller, answer)
                answered = True
        stdout, stderr = process.communicate(timeout=120)
        os.close(controller)
        return process.returncode, stdout, stderr, shown.decode()

    def without_signature_tools(self):
        (self.tools / "minisign").unlink()

    def with_openssl(self):
        """Signs the release with a test key, and gives the script that key and OpenSSL 3."""
        openssl = openssl3()
        if openssl is None:
            self.skipTest("OpenSSL 3 is not installed")
        self.without_signature_tools()
        os.symlink(openssl, self.tools / "openssl")
        self.openssl = openssl
        self.key = self.root / "key.pem"
        subprocess.run([openssl, "genpkey", "-algorithm", "ed25519", "-out", str(self.key)], check=True,
                       capture_output=True)
        public = subprocess.run([openssl, "pkey", "-in", str(self.key), "-pubout", "-outform", "DER"], check=True,
                                capture_output=True).stdout[-32:]
        self.script = self.root / "install.sh"
        source = SCRIPT.read_text().replace(
            "QROW_PUBLIC_KEY=RWR2pcgdcxt7jUi0QyCxd/bGK6kkC1Ggku3NNiniI/1PqQDEBbBkbzlG",
            f"QROW_PUBLIC_KEY={base64.b64encode(b'Ed' + KEY_ID + public).decode()}")
        self.assertNotEqual(source, SCRIPT.read_text())
        self.script.write_text(source)
        self.sign(f"Qrow release v{VERSION}")

    def ed25519(self, message):
        path = self.root / "message"
        path.write_bytes(message)
        return subprocess.run([self.openssl, "pkeyutl", "-sign", "-inkey", str(self.key), "-rawin", "-in", str(path)],
                              check=True, capture_output=True).stdout

    def sign(self, trusted):
        """Writes SHA256SUMS.minisig as minisign does: a prehashed signature and a global signature."""
        sums = (self.release / "SHA256SUMS").read_bytes()
        signature = self.ed25519(hashlib.blake2b(sums, digest_size=64).digest())
        signed = base64.b64encode(b"ED" + KEY_ID + signature).decode()
        global_signature = base64.b64encode(self.ed25519(signature + trusted.encode())).decode()
        (self.release / "SHA256SUMS.minisig").write_text(
            f"untrusted comment: synthetic\n{signed}\ntrusted comment: {trusted}\n{global_signature}\n")

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

    def test_verifies_the_checksums_with_minisign_and_the_release_key(self):
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
        self.tool("minisign", '#!/bin/sh\n[ "$1" = -v ] && exit 0\nexit 1\n')
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signature is not valid", result.stderr)
        self.assertFalse((self.prefix / "bin/qrow").exists())

    def test_stops_when_the_signature_names_another_release(self):
        self.signature = "untrusted comment: synthetic\nAAAA\ntrusted comment: Qrow release v1.0.0\nAAAA\n"
        self.publish({"x86_64": archive(b"qrow x86_64")})
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"not for Qrow {VERSION}", result.stderr)
        self.assertFalse((self.prefix / "bin/qrow").exists())

    def test_verifies_the_signature_with_openssl_without_minisign(self):
        self.with_openssl()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.prefix / "bin/qrow").read_bytes(), b"qrow x86_64")
        self.assertNotIn("cannot verify", result.stderr)

    def test_openssl_rejects_changed_checksums_and_other_releases(self):
        self.with_openssl()
        # Signed checksums that someone changed afterwards.
        sums = self.release / "SHA256SUMS"
        sums.write_text(sums.read_text().replace("0" * 64, "1" * 64))
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signature is not valid", result.stderr)

        # A valid signature whose trusted comment someone changed.
        self.publish({"x86_64": archive(b"qrow x86_64")})
        self.sign(f"Qrow release v{VERSION}")
        signature = self.release / "SHA256SUMS.minisig"
        signature.write_text(signature.read_text().replace(f"v{VERSION}", "v0.0.1"))
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("signature is not valid", result.stderr)

        # A valid signature of another release.
        self.sign("Qrow release v1.0.0")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"not for Qrow {VERSION}", result.stderr)
        self.assertFalse((self.prefix / "bin/qrow").exists())

    def broken_minisign(self):
        """A version manager shim without a selected version, as mise makes."""
        self.tool("minisign", "#!/bin/sh\necho 'mise ERROR No version is set for shim: minisign' >&2\nexit 1\n")

    def test_a_minisign_that_cannot_run_counts_as_missing(self):
        self.with_openssl()
        self.broken_minisign()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.prefix / "bin/qrow").read_bytes(), b"qrow x86_64")
        self.assertNotIn("cannot verify", result.stderr)

        self.without_signature_tools()
        (self.tools / "openssl").unlink()
        self.broken_minisign()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cannot verify the release signature", result.stderr)

    def test_an_openssl_that_cannot_verify_counts_as_missing(self):
        self.without_signature_tools()
        # Like LibreSSL or OpenSSL 1.1: pkeyutl has no -rawin option.
        self.tool("openssl", "#!/bin/sh\necho 'Usage: pkeyutl [options]'\nexit 1\n")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cannot verify the release signature", result.stderr)

    def test_continues_without_a_terminal_when_no_tool_can_verify(self):
        self.without_signature_tools()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("The archive could be compromised.", result.stderr)
        self.assertIn("no terminal to ask, continuing", result.stderr)
        self.assertEqual((self.prefix / "bin/qrow").read_bytes(), b"qrow x86_64")

    def test_asks_on_the_terminal_when_no_tool_can_verify(self):
        self.without_signature_tools()
        code, _, stderr, shown = self.install_on_terminal(b"n\n")
        self.assertNotEqual(code, 0)
        self.assertIn("Continue anyway? [Y/n]", shown)
        self.assertIn("install stopped", stderr)
        self.assertFalse(self.prefix.exists())

        code, stdout, stderr, _ = self.install_on_terminal(b"\n")
        self.assertEqual(code, 0, stderr)
        self.assertIn(f"Installed Qrow {VERSION}", stdout)

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

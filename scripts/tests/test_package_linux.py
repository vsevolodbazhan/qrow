"""Exercise Linux archive layout with an isolated, synthetic build."""
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "package"))
import linux_smoke  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


class LinuxPackageTest(unittest.TestCase):
    def test_archive_and_invalid_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "scripts/package").mkdir(parents=True)
            (root / "assets/app-icons").mkdir(parents=True)
            (root / "tools").mkdir()
            (root / "scripts/package/linux.sh").write_bytes((ROOT / "scripts/package/linux.sh").read_bytes())
            (root / "assets/app-icons/qrow-256.png").write_bytes(b"synthetic icon")
            for name in ("LICENSE", "NOTICE"):
                (root / name).write_text("synthetic notice")
            commands = {
                "cargo": '#!/bin/sh\nmkdir -p target/release\nprintf synthetic-binary > target/release/qrow\n',
                "uv": '#!/bin/sh\nfor last do :; done\nprintf synthetic-notices > "$last"\n',
                "uname": '#!/bin/sh\ncase "$1" in -m) echo aarch64;; -s) echo Linux;; esac\n',
            }
            for name, source in commands.items():
                path = root / "tools" / name
                path.write_text(source)
                path.chmod(0o755)
            env = dict(os.environ, PATH=f"{root / 'tools'}:{os.environ['PATH']}", QROW_RELEASE_VERSION="1.2.3")
            env.pop("CARGO_BUILD_TARGET", None)
            env.pop("CARGO_TARGET_DIR", None)
            subprocess.run(["sh", "scripts/package/linux.sh"], cwd=root, env=env, check=True, capture_output=True)
            archive = root / "target/package/Qrow-1.2.3-linux-aarch64.tar.gz"
            with tarfile.open(archive) as bundle:
                files = {member.name.removeprefix("./") for member in bundle.getmembers()}
                self.assertTrue({"bin/qrow", "share/applications/qrow.desktop", "share/icons/hicolor/256x256/apps/qrow.png", "share/licenses/qrow/THIRD_PARTY_NOTICES.txt"} <= files)
                self.assertEqual(bundle.extractfile("./bin/qrow").read(), b"synthetic-binary")
            env["QROW_RELEASE_VERSION"] = "../bad"
            result = subprocess.run(["sh", "scripts/package/linux.sh"], cwd=root, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)


class LinuxSmokeTest(unittest.TestCase):
    def archive(self, root, *, missing=None, executable=True):
        package = root / "package"
        for name in linux_smoke.REQUIRED:
            if name == missing:
                continue
            path = package / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"synthetic fixture")
        if executable:
            (package / "bin/qrow").chmod(0o755)
        archive = root / "archive.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            bundle.add(package, arcname=".")
        return archive

    def test_checks_libraries_and_desktop_entry(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = self.archive(Path(temporary))
            with patch.object(linux_smoke.subprocess, "run") as run:
                run.return_value.stdout = "libc.so.6 => /lib/libc.so.6"
                linux_smoke.check(archive)
                self.assertEqual([call.args[0][0] for call in run.call_args_list],
                                 ["ldd", "desktop-file-validate"])
                run.return_value.stdout = "libmissing.so => not found"
                with self.assertRaisesRegex(ValueError, "Unresolved shared library"):
                    linux_smoke.check(archive)

    def test_rejects_missing_files_and_nonexecutable_binary(self):
        for options in ({"missing": "share/licenses/qrow/NOTICE"}, {"executable": False}):
            with self.subTest(options=options), tempfile.TemporaryDirectory() as temporary:
                archive = self.archive(Path(temporary), **options)
                with self.assertRaises(ValueError):
                    linux_smoke.check(archive)


if __name__ == "__main__":
    unittest.main()

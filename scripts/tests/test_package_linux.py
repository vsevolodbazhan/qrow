"""Exercise Linux archive layout with an isolated, synthetic build."""
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

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


if __name__ == "__main__":
    unittest.main()

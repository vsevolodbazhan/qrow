import hashlib
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location("native_fixture", Path(__file__).resolve().parents[1] / "e2e/servers.py")
native_fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native_fixture)


class NativeFixtureTests(unittest.TestCase):
    def test_download_progress_reports_total_and_eta(self):
        with patch.object(native_fixture, "announce") as announce, patch.object(native_fixture.time, "monotonic", return_value=10):
            native_fixture.report_download_progress("spark", 100, 200, 0)
        message = announce.call_args.args[0]
        self.assertIn("100.0 B / 200.0 B received", message)
        self.assertIn("50.0%", message)
        self.assertIn("ETA 10s", message)

    def test_download_resumes_a_partial_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache = root / "target/e2e-downloads"
            cache.mkdir(parents=True)
            partial = cache / "fixture.partial"
            partial.write_bytes(b"ab")
            payload = b"abcd"
            item = {"url": "https://example.invalid/fixture.jar", "directory": "fixture.jar", "bytes": len(payload),
                    "sha512": hashlib.sha512(payload).hexdigest()}

            def start(command):
                Path(command[command.index("--output") + 1]).write_bytes(payload)
                process = MagicMock()
                process.poll.return_value = 0
                process.returncode = 0
                return process

            with patch.object(native_fixture, "ROOT", root), patch.object(native_fixture.subprocess, "Popen", side_effect=start) as popen, patch.object(native_fixture, "announce"):
                self.assertEqual(native_fixture.distribution(item), cache / "fixture.jar")
            command = popen.call_args.args[0]
            self.assertEqual(command[command.index("--continue-at") + 1], "-")

    def test_cached_download_is_verified_before_use(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache = root / "target/e2e-downloads"
            cache.mkdir(parents=True)
            archive = cache / "fixture.jar"
            archive.write_bytes(b"verified fixture")
            item = {"url": "https://example.invalid/fixture.jar", "directory": "fixture.jar",
                    "sha512": hashlib.sha512(archive.read_bytes()).hexdigest()}
            with patch.object(native_fixture, "ROOT", root), patch.object(native_fixture.subprocess, "run") as download:
                self.assertEqual(native_fixture.distribution(item), archive)
                archive.write_bytes(b"corrupted fixture")
                with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
                    native_fixture.distribution(item)
                download.assert_not_called()

    def test_cleanup_terminates_server_and_descendant(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = "import signal,time; from pathlib import Path; signal.signal(signal.SIGTERM, lambda *_: (Path('stopped').touch(), exit(0))); Path('ready').touch(); time.sleep(30)"
            parent = f"import subprocess,signal,sys,time; child=subprocess.Popen([sys.executable, '-c', {child!r}]); signal.signal(signal.SIGTERM, lambda *_: (child.wait(timeout=5), sys.exit(0))); time.sleep(30)"
            servers = native_fixture.Servers(root)
            try:
                servers.start("test-server", [sys.executable, "-c", parent], os.environ.copy())
                deadline = time.monotonic() + 5
                while not (root / "ready").exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertTrue((root / "ready").exists())
            finally:
                servers.stop()
            self.assertTrue((root / "stopped").exists())
            self.assertIsNotNone(servers.processes[0][1].poll())

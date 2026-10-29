import hashlib
import importlib.util
import json
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
    def test_progress_goes_to_stderr_so_json_output_stays_valid(self):
        import contextlib
        import io
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            native_fixture.announce("downloading")
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("downloading", stderr.getvalue())

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
            item = {"urls": ["https://example.invalid/fixture.jar"], "directory": "fixture.jar", "bytes": len(payload),
                    "sha512": hashlib.sha512(payload).hexdigest()}

            def start(command):
                Path(command[command.index("--output") + 1]).write_bytes(payload)
                process = MagicMock()
                process.poll.return_value = 0
                process.returncode = 0
                return process

            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}), patch.object(native_fixture.subprocess, "Popen", side_effect=start) as popen, patch.object(native_fixture, "announce"):
                self.assertEqual(native_fixture.distribution(item), cache / "fixture.jar")
            command = popen.call_args.args[0]
            self.assertEqual(command[command.index("--continue-at") + 1], "-")
            self.assertEqual(command[command.index("--max-time") + 1], str(native_fixture.DOWNLOAD_TIMEOUT_SECONDS))
            self.assertNotIn("--retry", command)

    def test_download_falls_back_to_the_next_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache = root / "target/e2e-downloads"
            payload = b"abcd"
            item = {"urls": ["https://mirror.invalid/fixture.jar", "https://origin.invalid/fixture.jar"],
                    "directory": "fixture.jar", "bytes": len(payload), "sha512": hashlib.sha512(payload).hexdigest()}

            def start(command):
                output = Path(command[command.index("--output") + 1])
                process = MagicMock()
                process.poll.return_value = 0
                if command[-1].startswith("https://mirror."):
                    output.write_bytes(payload[:2])
                    process.returncode = 28
                else:
                    self.assertEqual(output.read_bytes(), payload[:2])
                    output.write_bytes(payload)
                    process.returncode = 0
                return process

            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}), patch.object(native_fixture.subprocess, "Popen", side_effect=start) as popen, patch.object(native_fixture, "announce"):
                self.assertEqual(native_fixture.distribution(item), cache / "fixture.jar")
            mirror, origin = (call.args[0] for call in popen.call_args_list)
            self.assertEqual(mirror[-1], item["urls"][0])
            self.assertEqual(mirror[mirror.index("--speed-limit") + 1], str(native_fixture.DOWNLOAD_MINIMUM_RATE))
            self.assertEqual(origin[-1], item["urls"][1])
            self.assertNotIn("--speed-limit", origin)
            self.assertFalse((cache / "fixture.partial").exists())

    def test_download_fails_when_every_source_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            item = {"urls": ["https://mirror.invalid/fixture.jar", "https://origin.invalid/fixture.jar"],
                    "directory": "fixture.jar", "bytes": 4, "sha512": hashlib.sha512(b"abcd").hexdigest()}
            process = MagicMock()
            process.poll.return_value = 0
            process.returncode = 22
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}), patch.object(native_fixture.subprocess, "Popen", return_value=process) as popen, patch.object(native_fixture, "announce"):
                with self.assertRaises(native_fixture.subprocess.CalledProcessError):
                    native_fixture.distribution(item)
            self.assertEqual(popen.call_count, 2)

    def test_manifest_sources_share_one_archive_name(self):
        manifest = json.loads((native_fixture.ROOT / "tests/fixture/native-downloads.json").read_text())
        for name, item in manifest.items():
            with self.subTest(dependency=name):
                self.assertTrue(item["urls"])
                self.assertEqual(len({url.rsplit("/", 1)[1] for url in item["urls"]}), 1)
                self.assertTrue(all(url.startswith("https://") for url in item["urls"]))

    def test_cached_download_is_verified_before_use(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache = root / "target/e2e-downloads"
            cache.mkdir(parents=True)
            archive = cache / "fixture.jar"
            archive.write_bytes(b"verified fixture")
            item = {"urls": ["https://example.invalid/fixture.jar"], "directory": "fixture.jar",
                    "sha512": hashlib.sha512(archive.read_bytes()).hexdigest()}
            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(root / "target")}), patch.object(native_fixture.subprocess, "run") as download:
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

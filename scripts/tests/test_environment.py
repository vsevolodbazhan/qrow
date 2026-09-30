"""The shared helpers find the target directory, Docker, and a JDK as qtest and the fixture expect."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/core"))
import environment  # noqa: E402


class EnvironmentTests(unittest.TestCase):
    def test_target_directory_follows_cargo(self):
        with patch.dict(os.environ, {"CARGO_TARGET_DIR": "/tmp/qrow-target"}):
            self.assertEqual(environment.target_dir(), Path("/tmp/qrow-target"))
        with patch.dict(os.environ):
            os.environ.pop("CARGO_TARGET_DIR", None)
            self.assertEqual(environment.target_dir(), ROOT / "target")

    def test_docker_status_tells_a_stopped_daemon_from_a_missing_command(self):
        with patch.object(environment.shutil, "which", return_value=None):
            self.assertEqual(environment.docker_status(), "missing")
        with patch.object(environment.shutil, "which", return_value="/usr/bin/docker"):
            for result, status in [(subprocess.CompletedProcess([], 0), "running"),
                                   (subprocess.CompletedProcess([], 1), "stopped"),
                                   (subprocess.TimeoutExpired("docker", 20), "stopped")]:
                with self.subTest(status=status), patch.object(environment.subprocess, "run",
                                                               side_effect=[result]):
                    self.assertEqual(environment.docker_status(), status)

    def test_auto_runtime_uses_docker_only_when_it_runs(self):
        for status, runtime in [("running", "docker"), ("stopped", "native"), ("missing", "native")]:
            with self.subTest(status=status), patch.object(environment, "docker_status", return_value=status):
                self.assertEqual(environment.choose_runtime("auto"), runtime)
        with patch.object(environment, "docker_status") as probe:
            self.assertEqual(environment.choose_runtime("native"), "native")
            probe.assert_not_called()

    def test_jdk_needs_its_compiler_and_archiver(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            (home / "bin").mkdir()
            (home / "bin/java").touch()
            with patch.dict(os.environ, {"JAVA_HOME": directory}):
                self.assertEqual(environment.jdk_problem(), "JAVA_HOME is missing required JDK tools: javac, jar")
                for tool in ("javac", "jar"):
                    (home / "bin" / tool).touch()
                self.assertIsNone(environment.jdk_problem())
            with patch.dict(os.environ, {"JAVA_HOME": ""}):
                self.assertIn("must point to a JDK", environment.jdk_problem())


if __name__ == "__main__":
    unittest.main()

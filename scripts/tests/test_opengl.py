"""Hardware capability discovery must not hide renderer or tool failures."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "core"))
import opengl  # noqa: E402


class OpenGlProbeTests(unittest.TestCase):
    def probe(self, text, code=0, error=None):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "probe.log"
            log.write_text(text)
            execute = Mock(side_effect=error, return_value=code)
            result = opengl.probe(execute, log, object())
        return result, execute.call_args

    def test_only_an_explicit_unsupported_result_can_skip_hardware_suites(self):
        for status, available in (("available", True), ("unavailable", False)):
            with self.subTest(status=status):
                result, call = self.probe(f"QROW_OPENGL_CAPABILITY {status}: driver result\n")
                self.assertIs(result["available"], available)
                self.assertEqual(result["reason"], "driver result")
                self.assertIn("--exact", call.args[0])
                self.assertIn(opengl.PROBE, call.args[0])
                self.assertEqual(call.args[2], 1800)

    def test_failure_cannot_be_hidden_by_an_unavailable_marker(self):
        result, _ = self.probe("QROW_OPENGL_CAPABILITY unavailable: no GPU\nshader bug", 101)
        self.assertIn("failed (101)", result["error"])
        self.assertIsNone(result["available"])
        self.assertIn("shader bug", result["output"])

    def test_missing_or_ambiguous_markers_fail(self):
        for output in ("", "QROW_OPENGL_CAPABILITY available: yes\n" * 2):
            with self.subTest(output=output):
                result, _ = self.probe(output)
                self.assertIn("no unique result", result["error"])

    def test_timeout_retains_partial_output_from_process_group_executor(self):
        result, _ = self.probe("partial compiler output", code=None)
        self.assertIn("timed out", result["error"])
        self.assertEqual(result["output"], "partial compiler output")
        self.assertIsNone(result["available"])

    def test_execution_errors_retain_command_and_diagnostic(self):
        result, _ = self.probe("command", error=OSError("cannot execute cargo"))
        self.assertIn("cannot execute cargo", result["error"])
        self.assertEqual(result["command"][0], "cargo")

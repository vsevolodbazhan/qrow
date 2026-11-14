"""Hardware validation stays strict and source-bound outside required native plans."""
import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/qtest"))
sys.path.insert(0, str(ROOT / "scripts/core"))
import builds  # noqa: E402
import contract  # noqa: E402
import opengl  # noqa: E402
import runner  # noqa: E402

REASON = "hardware OpenGL Core pixel format unavailable (10002)"


def evidence(available):
    value = dict(source_sha="a" * 40, snapshot="snapshot", target="macos-arm64")
    report = dict(**value, build_manifest_sha256="manifest", status="passed", exit_code=0,
                  capability=dict(available=available, reason="hardware Core" if available else REASON),
                  suites=[dict(name=name, status="passed" if available else "skipped") for name in opengl.CI_HARDWARE])
    if available:
        report["suites"][0]["tests"] = [dict(count=14, failures=0)]
    else:
        for item in report["suites"]:
            item["reason"] = "Hardware OpenGL unavailable: " + REASON
    return value, report


class RendererEvidenceTests(unittest.TestCase):
    def test_only_complete_hardware_or_explicit_unavailable_evidence_passes(self):
        for available in (True, False):
            value, report = evidence(available)
            opengl.verify_hardware(report, value, "manifest")
            for key in ("source_sha", "snapshot", "target", "build_manifest_sha256"):
                changed = copy.deepcopy(report)
                changed[key] = "different"
                with self.subTest(available=available, key=key), self.assertRaises(ValueError):
                    opengl.verify_hardware(changed, value, "manifest")
            for suites in ([], report["suites"][:1], report["suites"] * 2):
                with self.subTest(suites=suites), self.assertRaises(ValueError):
                    opengl.verify_hardware(dict(report, suites=suites), value, "manifest")

    def test_shader_errors_empty_tests_and_unexpected_skips_fail(self):
        value, report = evidence(True)
        changes = [dict(status="failed"), dict(exit_code=1), dict(capability=dict(available=None, error="crash"))]
        for update in changes:
            with self.subTest(update=update), self.assertRaises(ValueError):
                opengl.verify_hardware(dict(report, **update), value, "manifest")
        for update in (dict(status="skipped"), dict(report_only=True), dict(tests=[]),
                       dict(tests=[dict(count=0, failures=0)]), dict(tests=[dict(count=14, failures=1)])):
            changed = copy.deepcopy(report)
            changed["suites"][0].update(update)
            with self.subTest(update=update), self.assertRaises(ValueError):
                opengl.verify_hardware(changed, value, "manifest")
        value, report = evidence(False)
        report["capability"]["reason"] = "shader initialization failed"
        with self.assertRaises(ValueError):
            opengl.verify_hardware(report, value, "manifest")

    def test_unexpected_unavailable_marker_is_a_probe_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "probe.log"
            log.write_text("QROW_OPENGL_CAPABILITY unavailable: shader compilation failed\n")
            result = opengl.probe(Mock(return_value=0), log, runner.Output(True))
            self.assertIsNone(result["available"])
            self.assertIn("Unexpected", result["error"])

    def test_preparation_failure_persists_required_failure_and_evidence(self):
        value, _ = evidence(False)
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"CARGO_TARGET_DIR": directory}), \
                patch.object(contract, "plan", return_value={}), \
                patch.object(builds, "Prepared", side_effect=ValueError("wrong target archive")), \
                patch.object(runner, "precheck", return_value=None):
            base = Path(directory) / "platform"
            manifest = Path(directory) / "manifest.json"
            manifest.write_text("{}")
            with self.assertRaisesRegex(ValueError, "capability probe"):
                opengl.run_hardware(ROOT, value, manifest, base, runner.Output(True))
            report = json.loads((base / "renderer-result.json").read_text())
            self.assertEqual(report["status"], "failed")
            self.assertEqual(report["suites"][0]["name"], "opengl-capability")
            self.assertIn("wrong target", report["capability"]["error"])
            self.assertTrue((Path(report["artifacts"]) / "opengl-capability.json").is_file())

    def test_explicit_local_hardware_commands_do_not_skip_from_ci_environment(self):
        from catalog import SUITES
        with patch.dict(os.environ, {"CI": "true"}), patch.object(runner.catalog, "HOST", "macos"), \
                patch.object(runner, "check_requirement", return_value=None):
            for name in opengl.CI_HARDWARE:
                with self.subTest(suite=name):
                    self.assertIsNone(runner.precheck(runner.Selected(SUITES[name]), {}))

"""Native renderer reports must contain actual backend and action evidence."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("renderers_perf", Path(__file__).resolve().parents[1] / "perf/renderers.py")
renderers = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(renderers)


class RendererEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name)
        self.log = "Qrow: using OpenGL rendering (test GPU; hardware accelerated)\n"
        self.log += "QROW_RENDER_FRAME backend=opengl time=1.000 ms=999.000\n" * 5
        self.log += "QROW_RENDER_FRAME backend=opengl time=3.000 ms=2.500\n" * 10
        self.log += "QROW_RENDER_FRAME backend=opengl time=5.000 ms=2.500\n" * 10
        self.log += "QROW_RENDER_FRAME backend=opengl time=7.000 ms=2.500\n" * 10
        (self.path / "qrow.log").write_text(self.log)
        lines = ["launch_to_accessible_new_connection_seconds=0.25"]
        lines += ["QROW_NATIVE_PERF " + json.dumps({"scenario": scenario, "start_seconds": 2 + index * 2, "end_seconds": 4 + index * 2,
                                                  "seconds": 2, "cpu_seconds": 0.1, "rss_mib": 10})
                  for index, scenario in enumerate(("results_vertical", "results_horizontal", "editor_vertical", "idle"))]
        (self.path / "qrow-resources.txt").write_text("\n".join(lines))

    def test_valid_report_excludes_startup_and_groups_action_intervals(self):
        report = renderers.measurements(self.path, "opengl")
        self.assertEqual(report["frames_ms"], {scenario: [2.5] * 10 for scenario in ("results_vertical", "results_horizontal", "editor_vertical")})
        self.assertEqual(report["launch_seconds"], 0.25)
        self.assertEqual(len(report["actions"]), 4)

    def test_wrong_backend_cannot_count_as_cpu_evidence(self):
        with self.assertRaisesRegex(RuntimeError, "did not select"):
            renderers.measurements(self.path, "software")

    def test_frame_failure_cannot_count_as_accelerated_evidence(self):
        (self.path / "qrow.log").write_text(self.log + "Qrow: OpenGL drawing failed: synthetic failure\n")
        with self.assertRaisesRegex(RuntimeError, "renderer failed"):
            renderers.measurements(self.path, "opengl")

    def test_incomplete_actions_cannot_count_as_a_comparison(self):
        resources = self.path / "qrow-resources.txt"
        resources.write_text("\n".join(resources.read_text().splitlines()[:-1]))
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            renderers.measurements(self.path, "opengl")

    def test_unmeasured_scroll_scenario_cannot_count_as_a_comparison(self):
        (self.path / "qrow.log").write_text(self.log.replace("time=7.000", "time=17.000"))
        with self.assertRaisesRegex(RuntimeError, "Too few.*editor_vertical"):
            renderers.measurements(self.path, "opengl")

import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("wait_for_core", Path(__file__).resolve().parents[1] / "wait-for-core.py")
wait_for_core = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wait_for_core)


class CoreDependencyTests(unittest.TestCase):
    def run_wait(self, responses):
        results = [subprocess.CompletedProcess([], 0, json.dumps(response)) for response in responses]
        with patch.dict("os.environ", {"GITHUB_REPOSITORY": "owner/repo", "CORE_SHA": "abc", "CORE_EVENT": "pull_request"}), patch.object(wait_for_core.subprocess, "run", side_effect=results) as run, patch.object(wait_for_core.time, "sleep"):
            wait_for_core.main()
            command = run.call_args.args[0]
            self.assertEqual(command[command.index("--commit") + 1], "abc")
            self.assertEqual(command[command.index("--event") + 1], "pull_request")

    def test_waits_for_matching_run_to_succeed(self):
        self.run_wait([[], [{"status": "in_progress"}], [{"status": "completed", "conclusion": "success", "url": "run"}]])

    def test_unsuccessful_core_prevents_e2e(self):
        for conclusion in ["failure", "cancelled", "skipped", "timed_out"]:
            with self.subTest(conclusion=conclusion), self.assertRaisesRegex(SystemExit, "Core did not pass"):
                self.run_wait([[{"status": "completed", "conclusion": conclusion, "url": "run"}]])

    def test_missing_run_times_out(self):
        with patch.object(wait_for_core.time, "monotonic", side_effect=[0, 2101]), self.assertRaisesRegex(SystemExit, "Timed out"):
            wait_for_core.main()

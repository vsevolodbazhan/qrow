"""Acceptance orchestration must fail closed and isolate every server mutation."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("e2e", ROOT / "scripts/e2e/run.py")
e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e2e)


class AcceptanceTests(unittest.TestCase):
    def test_native_evidence_uses_executor_and_driver_markers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(e2e.native_evidence_count(root, "query.started"), 0)
            (root / "query.interrupted").write_text("1\n")
            (root / "query.task").write_text("app-123-4")
            (root / "app-123-4.ended").write_text("1\n")
            self.assertEqual(e2e.native_evidence_count(root, "query.interrupted"), 1)
            self.assertEqual(e2e.native_evidence_count(root, "query.ended"), 1)
            for token in ["../query.started", "query.task", "/query.ended"]:
                with self.assertRaises(ValueError):
                    e2e.native_evidence_count(root, token)
            (root / "query.task").write_text("../outside")
            with self.assertRaises(ValueError):
                e2e.native_evidence_count(root, "query.ended")

    def test_compose_rejects_unscoped_project_before_invoking_docker(self):
        for name in ["", "production", "qrow-e2e-", "qrow-e2e-a;ls", "../qrow-e2e-x"]:
            with self.subTest(name=name), patch.dict(os.environ, {"QROW_E2E_PROJECT": name}), patch.object(e2e, "run") as run:
                with self.assertRaises(ValueError):
                    e2e.compose("down", "--volumes")
                run.assert_not_called()

    def test_mutations_are_scoped_to_the_fixture_project(self):
        with patch.dict(os.environ, {"QROW_E2E_PROJECT": "qrow-e2e-unit"}), patch.object(e2e, "run") as run:
            e2e.observe("kill-engine", "unused")
            args = run.call_args.args[0]
            self.assertEqual(args[:7], ["docker", "compose", "-f", str(e2e.COMPOSE), "-p", "qrow-e2e-unit", "exec"])
            self.assertIn("kyuubi", args)

    def test_evidence_paths_cannot_escape_volume(self):
        with patch.object(e2e, "compose") as compose:
            for token in ["../../passwd", "valid.started;id", "valid.unknown", "x/../x.started"]:
                with self.assertRaises(ValueError):
                    e2e.observe("count", token)
            compose.assert_not_called()

    def test_authentication_failures_do_not_count_as_readiness(self):
        failure = subprocess.CompletedProcess([], 1, "", "LDAP rejected")
        with patch.object(e2e, "compose", return_value=failure), patch.object(e2e.time, "monotonic", side_effect=[0, 1, 181]), patch.object(e2e.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "LDAP rejected"):
                e2e.ready()

    def test_timeout_kills_test_process_group(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(e2e.subprocess, "Popen") as popen, patch.object(e2e.os, "killpg") as kill:
            popen.return_value.pid = 12345
            popen.return_value.wait.side_effect = [subprocess.TimeoutExpired("cargo", 1), -9]
            with self.assertRaises(subprocess.TimeoutExpired):
                e2e.bounded_command(["cargo"], 1, Path(directory) / "test.log")
            kill.assert_called_once_with(12345, e2e.signal.SIGKILL)

    def test_nonzero_test_exit_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(e2e.subprocess, "Popen") as popen:
            popen.return_value.wait.return_value = 101
            with self.assertRaisesRegex(RuntimeError, "101"):
                e2e.bounded_command(["cargo"], 1, Path(directory) / "test.log")

    def test_failed_suite_still_collects_evidence_and_removes_volumes(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(e2e, "ROOT", Path(directory)), patch.dict(os.environ, {}, clear=True), patch.object(e2e.sys, "argv", ["e2e.py", "backend"]), patch.object(e2e, "require_commands"), patch.object(e2e, "free_port", return_value=23456), patch.object(e2e, "ready"), patch.object(e2e, "collect") as collect, patch.object(e2e, "compose", return_value=subprocess.CompletedProcess([], 0, "127.0.0.1:23456\n", "")) as compose, patch.object(e2e, "bounded_command", side_effect=RuntimeError("assertion failed")):
            with self.assertRaisesRegex(RuntimeError, "assertion failed"):
                e2e.main()
            collect.assert_called_once()
            self.assertIn(unittest.mock.call("down", "--volumes", "--remove-orphans", timeout=90), compose.call_args_list)
            failures = list(Path(directory).glob("target/e2e/*/failure.txt"))
            self.assertEqual(len(failures), 1)
            self.assertIn("assertion failed", failures[0].read_text())

    def gate_step(self, name):
        """Return the shell body of a named workflow step, as GitHub Actions would run it."""
        workflow = (ROOT / ".github/workflows/e2e.yml").read_text()
        body = workflow.split(f"      - name: {name}\n", 1)[1].split("        run: |\n", 1)[1]
        lines = []
        for line in body.splitlines():
            if line.strip() and not line.startswith(" " * 10):
                break
            lines.append(line[10:])
        return "\n".join(lines)

    def evaluate(self, changes="success", e2e_required="true", backend="success", ui="success"):
        """Run the gate's evaluate-suites step and return its step outputs."""
        script = self.gate_step("evaluate-suites")
        with tempfile.TemporaryDirectory() as directory:
            outputs = Path(directory) / "outputs"
            outputs.touch()
            result = subprocess.run(["/bin/bash", "-c", script], capture_output=True, text=True, check=False,
                                    env={"GITHUB_OUTPUT": str(outputs), "CHANGES_RESULT": changes,
                                         "E2E_REQUIRED": e2e_required, "BACKEND_RESULT": backend, "UI_RESULT": ui})
            self.assertEqual(result.returncode, 0, result.stderr)
            return dict(line.split("=", 1) for line in outputs.read_text().splitlines() if line)

    def test_required_gate_does_not_accept_skipped_jobs(self):
        workflow = (ROOT / ".github/workflows/e2e.yml").read_text()
        self.assertNotIn("pull_request_target", workflow)
        self.assertIn("needs: changes", workflow)
        self.assertIn("needs: [changes, backend, macos]", workflow)
        # Execute the actual gate's shell conditions for each possible Actions outcome.
        require = self.gate_step("require-both-suites")
        for backend, ui, expected in [("success", "success", 0), ("skipped", "skipped", 1),
                                      ("failure", "skipped", 1), ("success", "cancelled", 1),
                                      ("success", "failure", 1)]:
            with self.subTest(backend=backend, ui=ui):
                outputs = self.evaluate(backend=backend, ui=ui)
                result = subprocess.run(["/bin/bash", "-c", require], capture_output=True, check=False,
                                        env={"STATE": outputs["state"], "REASON": outputs["reason"]})
                self.assertEqual(result.returncode, expected)
        self.assertEqual(self.evaluate(e2e_required="false", backend="skipped", ui="skipped")["state"], "success")

    def test_gate_fails_closed_when_the_change_filter_is_unusable(self):
        """An unresolved e2e requirement must never be read as "no e2e needed"."""
        for changes, e2e_required in [("failure", ""), ("cancelled", ""), ("skipped", ""),
                                      ("success", ""), ("success", "unexpected")]:
            with self.subTest(changes=changes, e2e_required=e2e_required):
                outputs = self.evaluate(changes=changes, e2e_required=e2e_required, backend="skipped", ui="skipped")
                self.assertEqual(outputs["state"], "failure")

    def test_change_filter_resolves_the_repository_without_a_checkout(self):
        """gh needs an explicit repository: the filter job never checks the code out."""
        workflow = (ROOT / ".github/workflows/e2e.yml").read_text()
        filter_job = workflow.split("      - id: filter\n", 1)[1].split("\n  backend:", 1)[0]
        self.assertIn("GH_REPO: ${{ github.repository }}", filter_job)
        self.assertNotIn("actions/checkout", filter_job)

    def test_commit_gate_reports_both_suite_results(self):
        publish = self.gate_step("publish-commit-gate")
        with tempfile.TemporaryDirectory() as directory:
            gh = Path(directory) / "gh"
            gh.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
            gh.chmod(0o755)
            cases = [("success", "success", "success"), ("failure", "skipped", "failure"),
                     ("success", "cancelled", "failure")]
            for backend, ui, state in cases:
                with self.subTest(backend=backend, ui=ui):
                    outputs = self.evaluate(backend=backend, ui=ui)
                    self.assertEqual(outputs["state"], state)
                    result = subprocess.run(["/bin/bash", "-c", publish], check=True, capture_output=True, text=True,
                                            env={"PATH": directory, "GITHUB_REPOSITORY": "owner/repo", "TESTED_SHA": "candidate-sha",
                                                 "GITHUB_SERVER_URL": "https://github.com", "GITHUB_RUN_ID": "123",
                                                 "STATE": outputs["state"]})
                    self.assertIn("repos/owner/repo/statuses/candidate-sha", result.stdout)
                    self.assertIn(f"state={state}\n", result.stdout)
                    self.assertIn("context=e2e / gate\n", result.stdout)
            # A crashed or skipped evaluate-suites step leaves STATE empty and must publish a failure.
            result = subprocess.run(["/bin/bash", "-c", publish], check=True, capture_output=True, text=True,
                                    env={"PATH": directory, "GITHUB_REPOSITORY": "owner/repo", "TESTED_SHA": "candidate-sha",
                                         "GITHUB_SERVER_URL": "https://github.com", "GITHUB_RUN_ID": "123", "STATE": ""})
            self.assertIn("state=failure\n", result.stdout)

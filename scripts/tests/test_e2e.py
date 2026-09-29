"""The server fixture must fail closed and scope every mutation to disposable servers."""
import importlib.util
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/e2e"))
spec = importlib.util.spec_from_file_location("fixture", ROOT / "scripts/e2e/fixture.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class FakeServers:
    def __init__(self, answers):
        self.answers = answers
        self.users = []

    def check_alive(self):
        pass

    def beeline(self, user, _password):
        self.users.append(user)
        code, error = self.answers[user]
        return subprocess.CompletedProcess([], code, "", error)


class FixtureTests(unittest.TestCase):
    def test_native_evidence_uses_executor_and_driver_markers(self):
        with tempfile.TemporaryDirectory() as directory:
            native = fixture.NativeFixture("qrow-e2e-unit", 1, directory, [], [], {})
            root = Path(directory) / "executor-evidence"
            root.mkdir()
            self.assertEqual(native.evidence_count("query.started"), 0)
            (root / "query.interrupted").write_text("1\n")
            (root / "query.task").write_text("app-123-4")
            (root / "app-123-4.ended").write_text("1\n")
            self.assertEqual(native.evidence_count("query.interrupted"), 1)
            self.assertEqual(native.evidence_count("query.ended"), 1)
            for token in ["../query.started", "query.task", "/query.ended"]:
                with self.assertRaises(ValueError):
                    native.evidence_count(token)
            (root / "query.task").write_text("../outside")
            with self.assertRaises(ValueError):
                native.evidence_count("query.ended")

    def test_docker_fixture_rejects_an_unscoped_project_before_invoking_docker(self):
        for name in ["", "production", "qrow-e2e-", "qrow-e2e-a;ls", "../qrow-e2e-x"]:
            with self.subTest(name=name), patch.object(fixture.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    fixture.DockerFixture(name, 1, 1, "/tmp")
                run.assert_not_called()

    def test_mutations_are_scoped_to_the_fixture_project(self):
        docker = fixture.DockerFixture("qrow-e2e-unit", 23456, 1, "/tmp")
        with patch.object(fixture.subprocess, "run") as run:
            docker.kill_engine()
        args = run.call_args.args[0]
        self.assertEqual(args[:7], ["docker", "compose", "-f", str(fixture.COMPOSE), "-p", "qrow-e2e-unit", "exec"])
        self.assertIn("kyuubi", args)
        self.assertEqual(run.call_args.kwargs["env"]["QROW_E2E_PROJECT"], "qrow-e2e-unit")

    def test_evidence_paths_cannot_escape_the_volume(self):
        docker = fixture.DockerFixture("qrow-e2e-unit", 1, 1, "/tmp")
        with patch.object(docker, "compose") as compose:
            for token in ["../../passwd", "valid.started;id", "valid.unknown", "x/../x.started"]:
                with self.assertRaises(ValueError):
                    docker.evidence_count(token)
            compose.assert_not_called()

    def test_local_processes_cannot_stop_engines(self):
        native = fixture.NativeFixture("qrow-e2e-unit", 1, "/tmp", [], [], {})
        with patch.object(fixture.subprocess, "run") as run:
            for action in (native.kill_engine, native.restart_server):
                with self.assertRaisesRegex(RuntimeError, "Docker runtime"):
                    action()
            run.assert_not_called()

    def test_authentication_failures_do_not_count_as_readiness(self):
        servers = FakeServers({"qrow": (1, "LDAP rejected"), "other": (0, "")})
        with patch.object(fixture.time, "monotonic", side_effect=[0, 0, 0, 181]), patch.object(fixture.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "LDAP rejected"):
                fixture.wait_ready(servers)

    def test_readiness_starts_only_the_engine_of_the_test_user(self):
        servers = FakeServers({"qrow": (0, ""), "other": (1, "no room for a second engine")})
        fixture.wait_ready(servers)
        self.assertEqual(servers.users, [fixture.TEST_USER])

    def test_state_round_trip_keeps_the_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.json"
            fixture.save(fixture.DockerFixture("qrow-e2e-unit", 2, 3, directory), path)
            loaded = fixture.load(path)
        self.assertEqual((loaded.project, loaded.bind_port, loaded.port), ("qrow-e2e-unit", 2, 3))
        self.assertEqual(loaded.env(), {"QROW_E2E_PROJECT": "qrow-e2e-unit", "QROW_E2E_PORT": "3"})

class WorkflowTests(unittest.TestCase):
    def workflow_job(self, name, workflow_path=".github/workflows/test.yml"):
        workflow = (ROOT / workflow_path).read_text()
        match = re.search(rf"^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-z0-9-]+:\n|\Z)", workflow, re.MULTILINE | re.DOTALL)
        self.assertIsNotNone(match, f"Missing workflow job: {name}")
        return match.group("body")

    def test_unified_workflow_jobs_wait_only_for_their_inputs(self):
        workflow = (ROOT / ".github/workflows/test.yml").read_text()
        graph = [
            ("core-dependencies", None),
            ("core-scripts", "core-dependencies"),
            ("core-backend", "core-scripts"),
            ("core-macos", "core-backend"),
            ("e2e-backend", "core-backend"),
            # Backend E2E runs beside core-macos and gates the native UI suite.
            ("e2e-macos", "[core-macos, e2e-backend]"),
        ]
        for job, dependency in graph:
            with self.subTest(job=job):
                body = self.workflow_job(job)
                if dependency:
                    self.assertIn(f"needs: {dependency}", body)
                else:
                    self.assertNotIn("needs:", body)
                self.assertIn("ref: ${{ github.sha }}", body)
        self.assertNotIn("statuses:", workflow)

    def test_macos_package_is_built_once_and_reused_by_native_e2e(self):
        package = self.workflow_job("core-macos")
        native = self.workflow_job("e2e-macos")
        self.assertIn("sh scripts/package/macos.sh", package)
        self.assertIn("name: macos-package-and-performance\n", package)
        self.assertIn("name: macos-package-and-performance\n", native)
        # The headless E2E tests compile in the job, so Rust is always present.
        self.assertIn("run: ./qtest run e2e desktop --runtime native", native)
        self.assertEqual(len(re.findall(r"- name: (?:Select Rust toolchain|Restore Rust cache)\n\s+(?:run|uses):", native)), 2)

    def test_unified_workflow_handles_drafts_and_forks(self):
        workflow = (ROOT / ".github/workflows/test.yml").read_text()
        for job in ["core-dependencies", "core-scripts", "core-backend", "core-macos"]:
            with self.subTest(job=job):
                self.assertIn("!github.event.pull_request.draft", self.workflow_job(job))
                self.assertNotIn("head.repo.full_name", self.workflow_job(job))
        for job in ["e2e-backend", "e2e-macos"]:
            with self.subTest(job=job):
                body = self.workflow_job(job)
                self.assertIn("!github.event.pull_request.draft", body)
                self.assertIn("head.repo.full_name == github.repository", body)
        self.assertIn("converted_to_draft", workflow)

    def test_workflow_artifacts_expire_after_one_day(self):
        workflow = (ROOT / ".github/workflows/test.yml").read_text()
        self.assertEqual(workflow.count("retention-days: 1"), 4)
        self.assertNotIn("retention-days: 14", workflow)

    def test_native_e2e_downloads_come_from_the_mirror_without_an_actions_cache(self):
        # The mirror downloads the archives faster than an Actions cache restores them.
        for path, name in ((".github/workflows/test.yml", "e2e-macos"), (".github/workflows/release.yml", "test-e2e-macos")):
            with self.subTest(workflow=path):
                job = self.workflow_job(name, path)
                self.assertIn("timeout-minutes: 150", job)
                self.assertNotIn("actions/cache", job)
                self.assertNotIn("e2e-downloads", job)

    def test_release_workflow_runs_e2e_before_packaging(self):
        backend = self.workflow_job("test-e2e-backend", ".github/workflows/release.yml")
        native = self.workflow_job("test-e2e-macos", ".github/workflows/release.yml")
        package = self.workflow_job("package", ".github/workflows/release.yml")
        # The same waterfall as the test workflow: backend E2E runs beside core-macos.
        self.assertIn("- test-core-backend", backend)
        self.assertNotIn("- test-core-macos", backend)
        self.assertIn("./qtest run backend", backend)
        self.assertIn("- test-core-macos", native)
        self.assertIn("- test-e2e-backend", native)
        self.assertIn("./qtest run e2e desktop", native)
        self.assertIn("test-e2e-macos", package)

    def test_native_e2e_package_mode_validates_bundle(self):
        driver = (ROOT / "scripts/e2e/driver.sh").read_text()
        self.assertIn("QROW_E2E_REUSE_MACOS_PACKAGE", driver)
        self.assertIn("ditto -x -k", driver)
        self.assertIn("QROW_E2E_BUNDLE/Contents/MacOS/qrow", driver)

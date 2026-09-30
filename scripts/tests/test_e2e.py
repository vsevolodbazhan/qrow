"""The server fixture must fail closed and scope every mutation to disposable servers."""
import importlib.util
from pathlib import Path
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
    def test_both_runtimes_share_the_evidence_directory_with_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = str((Path(directory) / "executor-evidence").resolve())
            docker = fixture.DockerFixture("qrow-e2e-unit", 1, 2, directory)
            self.assertEqual(docker.env()["QROW_E2E_NATIVE_EVIDENCE"], evidence)
            with patch.object(fixture.subprocess, "run") as run:
                docker.compose("ps")
            self.assertEqual(run.call_args.kwargs["env"]["QROW_E2E_EVIDENCE"], evidence)
            native = fixture.NativeFixture("qrow-e2e-unit", 1, directory, [], [], {})
            self.assertEqual(native.env()["QROW_E2E_NATIVE_EVIDENCE"], str(Path(directory) / "executor-evidence"))

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

    def test_local_processes_cannot_stop_engines(self):
        native = fixture.NativeFixture("qrow-e2e-unit", 1, "/tmp", [], [], {})
        with patch.object(fixture.subprocess, "run") as run:
            for action in (native.kill_engine, native.restart_server):
                with self.assertRaisesRegex(RuntimeError, "Docker runtime"):
                    action()
            run.assert_not_called()

    def test_authentication_failures_do_not_count_as_readiness(self):
        servers = FakeServers({"qrow": (1, "LDAP rejected")})
        with patch.object(fixture.time, "monotonic", side_effect=[0, 0, 0, 181]), patch.object(fixture.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "LDAP rejected"):
                fixture.wait_ready(servers)

    def test_readiness_starts_only_the_engine_of_the_test_user(self):
        servers = FakeServers({"qrow": (0, "")})
        fixture.wait_ready(servers)
        self.assertEqual(servers.users, [fixture.TEST_USER])

    def test_state_round_trip_keeps_the_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.json"
            fixture.save(fixture.DockerFixture("qrow-e2e-unit", 2, 3, directory), path)
            loaded = fixture.load(path)
            evidence = str((Path(directory) / "executor-evidence").resolve())
        self.assertEqual((loaded.project, loaded.bind_port, loaded.port), ("qrow-e2e-unit", 2, 3))
        self.assertEqual(loaded.env(), {"QROW_E2E_PROJECT": "qrow-e2e-unit", "QROW_E2E_PORT": "3",
                                        "QROW_E2E_NATIVE_EVIDENCE": evidence})

class DriverTests(unittest.TestCase):
    def test_native_e2e_package_mode_validates_bundle(self):
        driver = (ROOT / "scripts/e2e/driver.sh").read_text()
        self.assertIn("QROW_E2E_REUSE_MACOS_PACKAGE", driver)
        self.assertIn("ditto -x -k", driver)
        self.assertIn("QROW_E2E_BUNDLE/Contents/MacOS/qrow", driver)

    def test_shell_scripts_build_in_the_cargo_target_directory(self):
        # Hook snapshots set CARGO_TARGET_DIR, so a fixed target/ path reads another build.
        for script in ("scripts/e2e/driver.sh", "scripts/package/macos.sh"):
            with self.subTest(script=script):
                text = (ROOT / script).read_text()
                self.assertEqual(text.count('qrow_target_dir="${CARGO_TARGET_DIR:-target}"'), 1)
                paths = [line for line in text.splitlines() if "target/" in line]
                self.assertEqual(paths, [])

    def test_only_preflight_mode_checks_permissions_and_only_the_driver_run_cleans_up(self):
        driver = (ROOT / "scripts/e2e/driver.sh").read_text()
        preflight = driver.index('if [ "${1:-}" = --preflight ]; then')
        self.assertEqual(driver.count('"$qrow_driver" --preflight'), 1)
        self.assertLess(preflight, driver.index('"$qrow_driver" --preflight'))
        self.assertLess(driver.index('"$qrow_driver" --preflight'), driver.index("    exit 0\nfi\n", preflight))
        prepare_exit = driver.index('test "${1:-}" != --prepare || exit 0')
        self.assertLess(prepare_exit, driver.index("trap cleanup 0"))
        self.assertEqual(driver.count("trap cleanup 0"), 1)

"""qtest selects, runs, and reports suites as docs/testing.md describes."""
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/qtest"))
import catalog  # noqa: E402
import cli  # noqa: E402
import runner  # noqa: E402

GUIDE = (ROOT / "docs/testing.md").read_text()


class CatalogTests(unittest.TestCase):
    def test_groups_name_existing_suites_without_server_suites(self):
        for name, (_, members) in catalog.GROUPS.items():
            for member in members:
                with self.subTest(group=name, suite=member):
                    self.assertIn(member, catalog.SUITES)
                    self.assertFalse(catalog.SUITES[member].explicit_only)

    def test_every_requirement_has_a_fix(self):
        for suite in catalog.SUITES.values():
            for requirement in suite.requires:
                with self.subTest(suite=suite.name, requirement=requirement):
                    if requirement != "fixture-runtime":
                        self.assertIn(requirement, catalog.REQUIREMENTS)

    def test_guide_lists_every_suite_and_group(self):
        for name in [*catalog.SUITES, *catalog.GROUPS]:
            with self.subTest(name=name):
                self.assertTrue(f"\n| `{name}`" in GUIDE, f"docs/testing.md has no table row for {name}")

    def test_guide_marks_exactly_the_filterable_suites(self):
        for suite in catalog.SUITES.values():
            with self.subTest(suite=suite.name):
                marked = f"| `{suite.name}` * |" in GUIDE
                self.assertEqual(marked, suite.filterable)

    def test_guide_topics_that_the_cli_names_exist(self):
        topics = cli.guide_sections()
        self.assertIn("write-a-ui-test", topics)
        self.assertIn("write-a-ui-test", cli.UI_TEST_TEMPLATE)
        self.assertIn("write-an-e2e-test", topics)
        self.assertIn("write-an-e2e-test", cli.E2E_TEST_TEMPLATE)


class ChangeRuleTests(unittest.TestCase):
    def test_paths_select_suites(self):
        cases = {
            "src/ui.rs": ["fmt", "clippy", "unit", "ui"],
            "tests/ui/support.rs": ["fmt", "clippy", "unit", "ui"],
            "vendor/gpui-base/src/lib.rs": ["fmt", "clippy", "unit", "ui"],
            "deny.toml": ["policy"],
            "Cargo.lock": ["fmt", "clippy", "unit", "ui", "policy"],
            ".github/workflows/test.yml": ["scripts", "policy"],
            "scripts/e2e/fixture.py": ["scripts"],
            "qtest": ["scripts"],
            "docs/testing.md": ["scripts"],
            "docs/queries.md": [],
            "scripts/core/preflight.sh": ["fmt", "clippy", "unit", "ui", "scripts", "policy"],
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                expected_in_order = [name for name in catalog.SUITES if name in expected]
                self.assertEqual(catalog.suites_for_changes([path]), expected_in_order)

    def test_hook_files_come_from_the_environment(self):
        with patch.dict(os.environ, {"QROW_CHANGED_FILES": "src/lib.rs\n\ndeny.toml\n"}):
            self.assertEqual(runner.changed_paths_from_git(), ["src/lib.rs", "deny.toml"])


class SelectionTests(unittest.TestCase):
    def names(self, selectors, changed=None):
        return [(item.suite.name, item.test_filter) for item in runner.resolve(selectors, changed)[0]]

    def test_no_selector_runs_the_default_group(self):
        self.assertEqual([name for name, _ in self.names([])], list(catalog.GROUPS["default"][1]))

    def test_groups_and_suites_are_not_repeated(self):
        self.assertEqual(self.names(["default", "unit", "fmt"]).count(("unit", None)), 1)

    def test_filter_selects_tests_of_a_suite(self):
        self.assertEqual(self.names(["ui/queries"]), [("ui", "queries")])

    def test_changed_without_matches_runs_nothing(self):
        self.assertEqual(self.names([], changed=["docs/queries.md"]), [])

    def test_unknown_selector_suggests_a_name(self):
        with self.assertRaisesRegex(runner.UsageError, "Did you mean unit"):
            runner.resolve(["unti"])

    def test_filter_needs_a_nextest_suite(self):
        with self.assertRaisesRegex(runner.UsageError, "does not accept a test filter"):
            runner.resolve(["fmt/x"])

    def test_nextest_command_combines_suite_and_test_filters(self):
        step = catalog.SUITES["ui"].steps[0]
        with patch.dict(os.environ, {"CI": "true"}):
            command = runner.render_command(step, {}, "queries", ["--no-capture"])
        self.assertEqual(command[-5:], ["--profile", "ci", "-E", "(binary(ui)) and test(queries)", "--no-capture"])

    def test_other_braces_stay_in_commands(self):
        command = runner.render_command(catalog.SUITES["scripts"].steps[0], {}, None, [])
        self.assertIn("-exec shellcheck {} +", command[-1])

    def test_python_steps_use_the_qtest_interpreter(self):
        command = runner.render_command(catalog.SUITES["policy"].steps[0], {}, None, [])
        self.assertEqual(command[0], sys.executable)


JUNIT = """<?xml version="1.0" encoding="UTF-8"?>
<testsuites><testsuite name="qrow::ui">
<testcase name="queries::passes" classname="qrow::ui"/>
<testcase name="queries::fails" classname="qrow::ui">
<failure message="panicked at tests/ui/queries.rs:53:5">thread 'queries::fails' panicked at tests/ui/queries.rs:53:5:
assertion `left == right` failed: deliberate
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace</failure>
</testcase></testsuite></testsuites>
"""


class ReportTests(unittest.TestCase):
    def test_junit_failures_keep_the_panic_message(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "junit.xml"
            path.write_text(JUNIT)
            failures = runner.junit_failures(path)
        self.assertEqual(len(failures), 1)
        self.assertEqual(failures[0]["test"], "qrow::ui::queries::fails")
        self.assertIn("deliberate", failures[0]["message"])
        self.assertNotIn("RUST_BACKTRACE", failures[0]["message"])


def fake_suite(name, *scripts, requires=(), timeout=30):
    steps = tuple(catalog.Step(("sh", "-c", script), timeout=timeout) for script in scripts)
    return catalog.Suite(name, f"Fake {name}.", requires, steps)


class RunTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        environment = patch.dict(os.environ, {"CARGO_TARGET_DIR": directory.name})
        environment.start()
        self.addCleanup(environment.stop)

    def run_suites(self, *suites, fail_fast=False, **options):
        selected = [runner.Selected(suite) for suite in suites]
        return runner.run(selected, {"repeat": 1, **options}, [], runner.Output(quiet=True), fail_fast)

    def test_statuses_exit_codes_and_logs(self):
        summary = self.run_suites(fake_suite("good", "echo fine"), fake_suite("bad", "echo broken; exit 4"))
        self.assertEqual((summary["status"], summary["exit_code"]), ("failed", runner.EXIT_FAILED))
        good, bad = summary["suites"]
        self.assertEqual(good["status"], "passed")
        self.assertIn("fine", Path(good["log"]).read_text())
        self.assertEqual(bad["reason"], "Step exited with 4.")
        saved = json.loads((Path(summary["artifacts"]) / "summary.json").read_text())
        self.assertEqual(saved, summary)
        self.assertEqual((runner.runs_dir() / "latest").resolve(), Path(summary["artifacts"]).resolve())

    def test_fail_fast_skips_later_suites(self):
        summary = self.run_suites(fake_suite("bad", "exit 1"), fake_suite("later", "exit 0"), fail_fast=True)
        self.assertEqual(summary["suites"][1]["status"], "skipped")

    def test_a_suite_stops_at_its_first_failed_step(self):
        summary = self.run_suites(fake_suite("steps", "exit 1", "echo second"))
        self.assertNotIn("second", Path(summary["suites"][0]["log"]).read_text().replace("echo second", ""))

    def test_missing_prerequisite_exits_with_three_and_gives_the_fix(self):
        with patch.object(runner, "check_requirement", return_value="Install the thing."):
            summary = self.run_suites(fake_suite("needs", "exit 0", requires=("cargo",)))
        self.assertEqual((summary["status"], summary["exit_code"]), ("missing", runner.EXIT_MISSING))
        self.assertEqual(summary["suites"][0]["missing"][0]["fix"], "Install the thing.")

    def test_step_limit_stops_the_command(self):
        summary = self.run_suites(fake_suite("slow", "sleep 30", timeout=1))
        self.assertEqual(summary["suites"][0]["reason"], "Step timed out.")

    def test_repeat_reports_the_failed_iteration(self):
        marker = Path(os.environ["CARGO_TARGET_DIR"]) / "count"
        script = f'n=$(cat {marker} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {marker}; [ $n -lt 3 ]'
        summary = self.run_suites(fake_suite("flaky", script), repeat=5)
        self.assertIn("Iteration 3 of 5", summary["suites"][0]["reason"])

    def test_docker_runtime_needs_docker_instead_of_java(self):
        with patch.object(runner.shutil, "which", return_value=None):
            fix = runner.check_requirement("fixture-runtime", {"runtime": "docker"})
        self.assertEqual(fix, catalog.REQUIREMENTS["docker"])

    def test_suites_that_stop_engines_reject_local_processes(self):
        fix = runner.check_requirement("fixture-runtime", {"runtime": "native", "fixture": "docker"})
        self.assertEqual(fix, catalog.REQUIREMENTS["fixture-docker"])


class FakeFixture:
    runtime = "docker"
    port = 4242

    def __init__(self, events):
        self.events = events

    def env(self):
        return {"QROW_E2E_PROJECT": "qrow-e2e-unit", "QROW_E2E_PORT": "4242"}

    def state(self):
        return {"runtime": "docker"}

    def collect(self, _artifacts):
        self.events.append("collect")

    def stop(self):
        self.events.append("stop")


def served_suite(name, script, prepare="true", kind="any"):
    return catalog.Suite(name, f"Fake {name}.", (), (catalog.Step(("sh", "-c", script)),), fixture=kind,
                         prepare=(catalog.Step(("sh", "-c", prepare)),))


class FixtureSessionTests(RunTests):
    def fake(self, reusable=None):
        events = []
        server = FakeFixture(events)

        def start(runtime, _artifacts):
            events.append(f"start {runtime}")
            return server
        return events, [patch.object(runner.fixture, "reusable", return_value=reusable and server),
                        patch.object(runner.fixture, "start", side_effect=start),
                        patch.object(runner.fixture, "save")]

    def run_with(self, patches, *suites, **options):
        for item in patches:
            item.start()
            self.addCleanup(item.stop)
        return self.run_suites(*suites, **options)

    def test_preparation_runs_before_servers_start_and_failures_still_stop_them(self):
        events, patches = self.fake()
        marker = Path(os.environ["CARGO_TARGET_DIR"]) / "prepared"
        summary = self.run_with(patches, served_suite("first", "exit 1", prepare=f"touch {marker}"),
                                served_suite("second", 'test "$QROW_E2E_PORT" = 4242'), runtime="auto")
        self.assertTrue(marker.exists())
        self.assertEqual(events, ["start auto", "collect", "stop"])
        self.assertEqual([suite["status"] for suite in summary["suites"]], ["failed", "passed"])

    def test_a_running_fixture_is_reused_and_not_stopped(self):
        events, patches = self.fake(reusable=True)
        summary = self.run_with(patches, served_suite("served", "exit 0"), runtime="auto")
        self.assertEqual(summary["status"], "passed")
        self.assertEqual(events, ["collect"])

    def test_failed_preparation_starts_no_servers(self):
        events, patches = self.fake()
        summary = self.run_with(patches, served_suite("served", "exit 0", prepare="exit 7"), runtime="auto")
        self.assertEqual(events, [])
        self.assertIn("Preparation failed", summary["suites"][0]["reason"])

    def test_a_docker_suite_starts_docker_servers(self):
        events, patches = self.fake()
        self.run_with(patches, served_suite("backend", "exit 0", kind="docker"), runtime="auto")
        self.assertEqual(events[0], "start docker")

    def test_cleanup_runs_after_a_failure(self):
        marker = Path(os.environ["CARGO_TARGET_DIR"]) / "cleaned"
        suite = catalog.Suite("cleaned", "Fake.", (), (catalog.Step(("sh", "-c", "exit 1")),),
                              cleanup=(catalog.Step(("sh", "-c", f"touch {marker}")),))
        summary = self.run_suites(suite)
        self.assertEqual(summary["suites"][0]["status"], "failed")
        self.assertTrue(marker.exists())


class CommandLineTests(unittest.TestCase):
    def call(self, *argv):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                code = cli.main(list(argv))
            except SystemExit as exit:
                code = exit.code
        return code, stdout.getvalue(), stderr.getvalue()

    def test_usage_errors_exit_with_two(self):
        self.assertEqual(self.call("run", "nosuch")[0], runner.EXIT_USAGE)
        self.assertEqual(self.call("run", "ui/bad filter")[0], runner.EXIT_USAGE)
        self.assertEqual(self.call("help", "no-such-topic")[0], runner.EXIT_USAGE)
        self.assertEqual(self.call("list", "--", "x")[0], runner.EXIT_USAGE)

    def test_help_lists_topics_and_prints_a_section(self):
        code, output, _ = self.call("help")
        self.assertEqual(code, 0)
        self.assertIn("write-a-ui-test", output)
        code, output, _ = self.call("help", "read-results")
        self.assertTrue(output.startswith("## Read results"))

    def test_list_json_describes_every_suite(self):
        code, output, _ = self.call("list", "--json")
        self.assertEqual(code, 0)
        self.assertEqual([suite["name"] for suite in json.loads(output)["suites"]], list(catalog.SUITES))

    def test_new_adds_a_failing_test_and_registers_its_module(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tests/ui").mkdir(parents=True)
            (root / "tests/ui/main.rs").write_text(
                '//! UI tests.\n#[path = "../support/mod.rs"]\nmod support;\n\nmod queries;\n')
            with patch.object(cli, "ROOT", root):
                self.assertEqual(self.call("new", "ui", "connections", "keeps_form")[0], 0)
                self.assertEqual(self.call("new", "ui", "connections", "keeps_form")[0], runner.EXIT_USAGE)
                self.assertEqual(self.call("new", "ui", "Bad", "x")[0], runner.EXIT_USAGE)
            suite = (root / "tests/ui/connections.rs").read_text()
            self.assertIn("fn keeps_form(cx: &mut TestAppContext)", suite)
            self.assertIn('panic!("Write the test keeps_form")', suite)
            self.assertEqual((root / "tests/ui/main.rs").read_text(),
                             '//! UI tests.\n#[path = "../support/mod.rs"]\nmod support;\n\n'
                             "mod connections;\nmod queries;\n")


if __name__ == "__main__":
    unittest.main()

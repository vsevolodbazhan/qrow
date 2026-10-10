"""qtest selects, runs, and reports suites as docs/testing.md describes."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
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


class SummaryTests(unittest.TestCase):
    def test_fixture_failure_without_step_log_reports_original_error(self):
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            cli.print_summary({"run": "fixture-failure", "status": "failed", "artifacts": "/tmp/evidence",
                               "suites": [{"name": "e2e", "status": "failed", "reason": "fixture timed out"}]})
        self.assertIn("fixture timed out", output.getvalue())
        self.assertIn("/tmp/evidence", output.getvalue())


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

    def test_clippy_app_is_the_application_pass_of_clippy(self):
        clippy, app = catalog.SUITES["clippy"], catalog.SUITES["clippy-app"]
        self.assertEqual(app.steps, (catalog.CLIPPY_APP,))
        self.assertTrue(app.macos_only)
        self.assertEqual(clippy.steps, (catalog.CLIPPY_CORE, catalog.CLIPPY_APP) if catalog.MACOS
                         else (catalog.CLIPPY_CORE,))
        self.assertIn("--no-default-features", catalog.CLIPPY_CORE.command)
        self.assertNotIn("--no-default-features", catalog.CLIPPY_APP.command)

    def test_postgres_has_its_own_docker_suite_and_backend_ci_job(self):
        postgres = catalog.SUITES["postgres"]
        self.assertTrue(postgres.explicit_only)
        self.assertIn("docker", postgres.requires)
        self.assertIsNone(postgres.fixture)
        self.assertIn("postgres", catalog.CI_JOBS["backend"].suites)

    def test_trino_has_its_own_docker_suite_and_backend_ci_job(self):
        trino = catalog.SUITES["trino"]
        self.assertTrue(trino.explicit_only)
        self.assertIn("docker", trino.requires)
        self.assertIsNone(trino.fixture)
        self.assertIn("trino", catalog.CI_JOBS["backend"].suites)

    def test_transfer_probes_keep_native_and_docker_ci_requirements_separate(self):
        kyuubi = catalog.SUITES["perf-e2e"]
        self.assertEqual(kyuubi.fixture, "any")
        self.assertNotIn("docker", kyuubi.requires)
        self.assertIn("transfer_perf::kyuubi", kyuubi.steps[1].nextest_filter)
        for step, binary in zip(kyuubi.steps, ("e2e", "integration")):
            self.assertEqual(step.command[step.command.index("--test") + 1], binary)
        for engine in ("postgres", "trino"):
            name = "perf-" + engine
            suite = catalog.SUITES[name]
            self.assertTrue(suite.explicit_only)
            self.assertIn("docker", suite.requires)
            self.assertIn("--perf", suite.steps[0].command)
            self.assertIn(name, catalog.CI_JOBS["backend"].report_only)
            self.assertNotIn(name, catalog.CI_JOBS["e2e"].report_only)

    def test_guide_topics_that_the_cli_names_exist(self):
        topics = cli.guide_sections()
        self.assertIn("write-a-ui-test", topics)
        self.assertIn("write-a-ui-test", cli.UI_TEST_TEMPLATE)
        self.assertIn("write-an-e2e-test", topics)
        self.assertIn("write-an-e2e-test", cli.E2E_TEST_TEMPLATE)


class ChangeRuleTests(unittest.TestCase):
    def test_paths_select_suites(self):
        rust = ["fmt", "clippy", "rustdoc", "unit", "ui"]
        cases = {
            "src/ui.rs": rust,
            "tests/ui/support.rs": rust,
            "tests/fixture/server/Blocking.java": [],
            "vendor/gpui-base/src/lib.rs": rust,
            "deny.toml": ["policy", "deps"],
            "Cargo.lock": [*rust, "policy", "deps"],
            ".github/workflows/test.yml": ["scripts", "policy"],
            "scripts/e2e/fixture.py": ["scripts"],
            "qtest": ["scripts"],
            "docs/testing.md": ["scripts"],
            "docs/queries.md": [],
            "scripts/core/preflight.sh": [*rust, "scripts", "policy"],
            "tests/desktop/fake-codex.py": [*rust, "scripts"],
            "tests/desktop/fake-codex.sh": [*rust, "scripts"],
            "tests/desktop/Driver.swift": rust,
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                expected_in_order = [name for name in catalog.SUITES if name in expected]
                self.assertEqual(catalog.suites_for_changes([path]), expected_in_order)

    def test_hooks_run_only_their_suites(self):
        cases = {
            ("pre-commit", "src/lib.rs"): ["fmt", "clippy"],
            ("pre-push", "src/lib.rs"): ["fmt", "clippy", "rustdoc", "unit", "ui"],
            ("pre-commit", "Cargo.lock"): ["fmt", "clippy", "policy"],
            ("pre-push", "Cargo.lock"): ["fmt", "clippy", "rustdoc", "unit", "ui", "policy", "deps"],
            ("pre-push", "scripts/qtest/cli.py"): ["scripts"],
            ("pre-push", "docs/queries.md"): [],
        }
        for (hook, path), expected in cases.items():
            with self.subTest(hook=hook, path=path):
                expected_in_order = [name for name in catalog.SUITES if name in expected]
                self.assertEqual(catalog.suites_for_hook(hook, [path]), expected_in_order)
        # CI runs the slow suites.
        for _, suites in catalog.HOOKS.values():
            self.assertNotIn("coverage", suites)
            self.assertNotIn("perf", suites)

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

    def test_e2e_test_filters_keep_the_kyuubi_fixture_boundary(self):
        step = catalog.SUITES["e2e"].steps[0]
        for selected in [None, "postgres", "trino", "queries"]:
            with self.subTest(selected=selected):
                command = runner.render_command(step, {}, selected, [])
                expression = command[command.index("-E") + 1]
                self.assertIn("not test(/^postgres::/)", expression)
                self.assertIn("not test(/^trino::/)", expression)
                self.assertIn("not test(/^perf::/)", expression)
                if selected:
                    self.assertTrue(expression.endswith(f" and test({selected})"))

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

    def test_a_report_only_failure_does_not_fail_the_run(self):
        selected = [runner.Selected(fake_suite("probe", "exit 1")), runner.Selected(fake_suite("check", "exit 0"))]
        summary = runner.run(selected, {"repeat": 1}, [], runner.Output(quiet=True), fail_fast=True,
                             report_only=("probe",))
        self.assertEqual((summary["status"], summary["exit_code"]), ("passed", runner.EXIT_PASSED))
        probe, check = summary["suites"]
        self.assertEqual((probe["status"], probe["report_only"]), ("failed", True))
        self.assertEqual(check["status"], "passed")
        self.assertNotIn("report_only", check)

    def test_a_missing_prerequisite_stops_a_run_that_requires_all(self):
        marker = Path(os.environ["CARGO_TARGET_DIR"]) / "ran"
        selected = [runner.Selected(fake_suite("first", f"touch {marker}")),
                    runner.Selected(fake_suite("needs", "exit 0", requires=("desktop",)))]
        with patch.object(runner, "check_requirement", side_effect=lambda name, _: "Grant it." if name == "desktop" else None):
            summary = runner.run(selected, {"repeat": 1}, [], runner.Output(quiet=True), require_all=True)
        self.assertFalse(marker.exists())
        self.assertEqual((summary["status"], summary["exit_code"]), ("missing", runner.EXIT_MISSING))
        self.assertEqual([suite["status"] for suite in summary["suites"]], ["skipped", "missing"])

    def test_ci_jobs_stop_before_they_build_when_a_prerequisite_is_missing(self):
        with patch.object(runner, "run", return_value={"exit_code": 0}) as run, \
                patch.object(cli, "print_summary"), contextlib.redirect_stderr(io.StringIO()):
            cli.main(["ci", "e2e"])
        self.assertTrue(run.call_args.kwargs["require_all"])

    def test_the_job_list_shows_the_jobs_that_pull_requests_skip(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cli.main(["ci"])
        lines = {line.split()[0]: line for line in output.getvalue().splitlines()}
        for name, job in catalog.CI_JOBS.items():
            with self.subTest(job=name):
                self.assertEqual(lines[name].endswith("not in pull requests"), not job.pull_requests)

    def test_a_nextest_step_that_writes_no_report_does_not_report_old_failures(self):
        report = runner.junit_report()
        report.parent.mkdir(parents=True)
        report.write_text(JUNIT)
        step = catalog.Step(("sh", "-c", "exit 1"), nextest_filter="all()")
        summary = self.run_suites(catalog.Suite("stale", "Fake stale.", (), (step,)))
        self.assertEqual(summary["suites"][0]["status"], "failed")
        self.assertNotIn("failures", summary["suites"][0])
        self.assertFalse(report.exists())

    def test_a_nextest_step_reports_the_failures_of_its_report(self):
        report = runner.junit_report()
        fresh = Path(os.environ["CARGO_TARGET_DIR"]) / "fresh.xml"
        fresh.write_text(JUNIT)
        step = catalog.Step(("sh", "-c", f'mkdir -p "{report.parent}" && cp "{fresh}" "{report}"; exit 1'),
                            nextest_filter="all()")
        summary = self.run_suites(catalog.Suite("fresh", "Fake fresh.", (), (step,)))
        self.assertEqual(summary["suites"][0]["failures"][0]["test"], "qrow::ui::queries::fails")

    def test_step_environment_names_the_target_directory(self):
        step = catalog.Step(("sh", "-c", 'echo "dir=$QROW_DIST_DIR"'), env=(("QROW_DIST_DIR", "{target}/package"),))
        summary = self.run_suites(catalog.Suite("env", "Fake env.", (), (step,)))
        self.assertIn(f"dir={os.environ['CARGO_TARGET_DIR']}/package", Path(summary["suites"][0]["log"]).read_text())

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

    def test_the_desktop_requirement_runs_the_driver_preflight(self):
        with patch.object(runner, "_succeeds", return_value=False) as succeeds:
            self.assertEqual(runner.check_requirement("desktop", {}), catalog.REQUIREMENTS["desktop"])
        self.assertEqual(succeeds.call_args.args[0], ["sh", "scripts/e2e/driver.sh", "--preflight"])

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

    def test_suites_that_share_a_preparation_step_run_it_once(self):
        _, patches = self.fake()
        counter = Path(os.environ["CARGO_TARGET_DIR"]) / "builds"
        build = f"echo build >> {counter}"
        summary = self.run_with(patches, served_suite("first", "exit 0", prepare=build),
                                served_suite("second", "exit 0", prepare=build), runtime="auto")
        self.assertEqual(summary["status"], "passed")
        self.assertEqual(counter.read_text().splitlines(), ["build"])

    def test_a_failed_shared_preparation_step_blocks_every_suite_that_needs_it(self):
        events, patches = self.fake()
        summary = self.run_with(patches, served_suite("first", "exit 0", prepare="exit 7"),
                                served_suite("second", "exit 0", prepare="exit 7"), runtime="auto")
        self.assertEqual(events, [])
        self.assertEqual([(suite["name"], suite["status"]) for suite in summary["suites"]],
                         [("first", "failed"), ("second", "failed")])
        self.assertIn("Preparation failed", summary["suites"][1]["reason"])

    def test_e2e_and_perf_e2e_share_the_build_of_the_test_binary(self):
        self.assertEqual(catalog.SUITES["e2e"].prepare, (catalog.E2E_BUILD,))
        self.assertEqual(catalog.SUITES["perf-e2e"].prepare,
                         (catalog.E2E_BUILD, catalog.TRANSFER_PERF_BUILD))

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


import compare  # noqa: E402


class PerformanceTests(unittest.TestCase):
    def test_metrics_come_from_probe_lines_in_the_log(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "perf.log"
            log.write_text('noise\nQROW_PERF {"probe": "a", "value": 1.5, "unit": "ms", "budget": 5}\n'
                           '    QROW_PERF {"probe": "b", "value": 2, "unit": "MB", "budget": 3}\nQROW_PERF {broken\n')
            self.assertEqual([m["probe"] for m in runner.metrics_in(log)], ["a", "b"])

    def test_a_probe_is_slower_above_the_threshold(self):
        base = {"fast": ("ms", [10, 11, 12]), "slow": ("ms", [10, 10, 10]), "only-base": ("ms", [1])}
        head = {"fast": ("ms", [9, 10, 11]), "slow": ("ms", [13, 13, 14])}
        rows = {row["probe"]: row for row in compare.summarize(base, head, 25)}
        self.assertEqual(set(rows), {"fast", "slow"})
        self.assertFalse(rows["fast"]["regression"])
        self.assertTrue(rows["slow"]["regression"])
        self.assertAlmostEqual(rows["slow"]["change_percent"], 30.0)

    def test_rounds_alternate_the_side_that_runs_first(self):
        calls = []

        def fake_run(tree, suites, target, output):
            calls.append("head" if tree == runner.ROOT else "base")
            return {"p": ("ms", [1.0])}

        with tempfile.TemporaryDirectory() as directory, \
                patch.object(compare, "base_tree", return_value=(Path(directory), "a" * 40)), \
                patch.object(compare, "run_once", side_effect=fake_run):
            (Path(directory) / "qtest").write_text("")
            report, code = compare.compare("main", ["perf"], 3, 25, lambda _: None)
        self.assertEqual(calls, ["base", "head", "head", "base", "base", "head"])
        self.assertEqual(code, runner.EXIT_PASSED)
        self.assertEqual(report["probes"][0]["base"], 1.0)

    def test_a_revision_without_qtest_cannot_be_compared(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(compare, "base_tree", return_value=(Path(directory), "a" * 40)):
            self.assertEqual(compare.compare("old", ["perf"], 1, 25, lambda _: None), (None, runner.EXIT_MISSING))


class ToolArchiveTests(unittest.TestCase):
    def test_pinned_archives_have_digests(self):
        for tool in catalog.TOOLS.values():
            for archive in tool.archives:
                with self.subTest(tool=tool.name, platform=archive.platform):
                    self.assertTrue(archive.url.startswith("https://github.com/"))
                    self.assertIn(tool.version, archive.url)
                    self.assertTrue(archive.member.endswith(tool.name))
                    self.assertRegex(archive.sha256, r"^[0-9a-f]{64}$")
            platforms = [archive.platform for archive in tool.archives]
            self.assertEqual(len(platforms), len(set(platforms)), tool.name)

    def test_every_tool_has_an_archive_for_the_ci_runners(self):
        for tool in catalog.TOOLS.values():
            for platform in ("linux-x86_64", "macos-aarch64"):
                with self.subTest(tool=tool.name, platform=platform):
                    self.assertIn(platform, [archive.platform for archive in tool.archives])

    def test_host_platform_names_the_system_and_the_processor(self):
        cases = {("Darwin", "arm64"): "macos-aarch64", ("Darwin", "x86_64"): "macos-x86_64",
                 ("Linux", "x86_64"): "linux-x86_64", ("Linux", "aarch64"): "linux-aarch64",
                 ("Linux", "AMD64"): "linux-x86_64", ("Windows", "AMD64"): None}
        for (system, machine), expected in cases.items():
            with self.subTest(system=system, machine=machine), \
                    patch.object(catalog.platform, "system", return_value=system), \
                    patch.object(catalog.platform, "machine", return_value=machine):
                self.assertEqual(catalog.host_platform(), expected)

    def make_archive(self, root, member):
        import hashlib
        import tarfile
        source = root / "source"
        (source / member).parent.mkdir(parents=True, exist_ok=True)
        (source / member).write_text("#!/bin/sh\n")
        (source / "README.md").write_text("Not the tool.\n")
        path = root / "tool.tar.gz"
        with tarfile.open(path, "w:gz") as bundle:
            bundle.add(source / member, arcname=member)
            bundle.add(source / "README.md", arcname="README.md")
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_an_archive_installs_only_with_its_pinned_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            member = "cargo-deny-0.20.2-aarch64-apple-darwin/cargo-deny"
            path, digest = self.make_archive(root, member)
            tool = catalog.TOOLS["cargo-deny"]
            with patch.dict(os.environ, {"CARGO_HOME": str(root / "home")}), contextlib.redirect_stderr(io.StringIO()):
                wrong = catalog.Archive("macos-aarch64", path.as_uri(), "0" * 64, member)
                self.assertFalse(cli.install_archive(tool, wrong))
                self.assertFalse((root / "home/bin/cargo-deny").exists())
                self.assertTrue(cli.install_archive(tool, catalog.Archive("macos-aarch64", path.as_uri(), digest, member)))
            self.assertTrue(os.access(root / "home/bin/cargo-deny", os.X_OK))
            self.assertEqual(sorted(item.name for item in (root / "home/bin").iterdir()), ["cargo-deny"])

    def run_install(self, tool, installs, checks):
        """Run install_tools for one fake tool. Returns the commands that it ran."""
        commands = []

        def run(command, **_options):
            commands.append(command)
            return subprocess.CompletedProcess(command, 0)
        with patch.dict(catalog.TOOLS, {tool.name: tool}), \
                patch.object(catalog, "host_platform", return_value="linux-x86_64"), \
                patch.object(cli, "install_archive", return_value=installs), \
                patch.object(runner, "check_requirement", side_effect=checks), \
                patch.object(cli.subprocess, "run", side_effect=run), \
                contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(cli.install_tools([tool.name]), runner.EXIT_PASSED)
        return commands

    def test_setup_runs_after_an_archive_and_a_build_runs_only_without_one(self):
        archive = catalog.Archive("linux-x86_64", "https://github.com/x/y/tool-1.0.tar.gz", "0" * 64, "tool")
        tool = catalog.Tool("tool", "1.0", ("tool", "--version"), "tool 1.0", (("build", "tool"),),
                            (archive,), setup=(("setup", "tool"),))
        self.assertEqual(self.run_install(tool, True, ["missing", None]), [("setup", "tool")])
        self.assertEqual(self.run_install(tool, False, ["missing"]), [("build", "tool"), ("setup", "tool")])
        self.assertEqual(self.run_install(tool, True, [None]), [])


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

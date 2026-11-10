"""Run suites: check requirements, execute steps, collect logs and failures."""
import dataclasses
from dataclasses import dataclass, field
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time
import uuid
import xml.etree.ElementTree as ElementTree

import catalog
from process import lease, spawn_options, stop_tree

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/core"))
sys.path.insert(0, str(ROOT / "scripts/e2e"))
import environment  # noqa: E402
from environment import target_dir  # noqa: E402
import fixture  # noqa: E402

# Exit codes are part of the CLI contract; docs/testing.md lists them.
EXIT_PASSED = 0
EXIT_FAILED = 1
EXIT_USAGE = 2
EXIT_MISSING = 3


def runs_dir():
    return target_dir() / "qtest" / "runs"


# Requirements -------------------------------------------------------------


def _succeeds(command, timeout=20):
    try:
        return subprocess.run(command, cwd=ROOT, capture_output=True, timeout=timeout).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _tool_version(tool):
    try:
        result = subprocess.run(tool.version_command, cwd=ROOT, capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


LINUX_APP_LIBS = ("xcb", "fontconfig", "freetype2", "xkbcommon", "xkbcommon-x11")


def check_requirement(name, options):
    """Return None when the requirement holds, else a fix for the user."""
    fixes = catalog.REQUIREMENTS
    if name in ("cargo", "uv", "shellcheck", "actionlint", "dbus-run-session", "gnome-keyring-daemon", "xvfb-run", "xdotool"):
        return None if shutil.which(name) else fixes[name]
    if name == "linux-app-libs":
        # The pkg-config names of the libraries that the application links.
        if not catalog.LINUX_HOST:
            return None
        return None if _succeeds(["pkg-config", "--exists", *LINUX_APP_LIBS]) else fixes[name]
    if name == "xcode":
        return None if shutil.which("xcode-select") and _succeeds(["xcode-select", "-p"]) else fixes[name]
    if name == "docker":
        status = environment.docker_status()
        if status == "missing":
            return fixes["docker"]
        return None if status == "running" else fixes["docker-running"]
    if name in catalog.TOOLS:
        tool = catalog.TOOLS[name]
        version = _tool_version(tool)
        return None if version and version.startswith(tool.version_prefix) else fixes[name]
    if name == "desktop":
        # `driver.sh --preflight` checks the permissions. The other modes of the
        # driver do not, so check them here, before the package build and the servers.
        return None if _succeeds(["sh", "scripts/e2e/driver.sh", "--preflight"], timeout=600) else fixes[name]
    if name == "fixture-runtime":
        runtime = options.get("runtime", "auto")
        if options.get("fixture") == "docker":
            if runtime == "native":
                return fixes["fixture-docker"]
            return check_requirement("docker", options)
        if runtime == "docker":
            return check_requirement("docker", options)
        if fixture.reusable(runtime) is not None:
            return None
        # `auto` chooses Docker only when its daemon answers.
        if environment.choose_runtime(runtime) == "docker":
            return None
        return None if environment.jdk_problem() is None else fixes["java"]
    raise ValueError(f"Unknown requirement: {name}")


# Selection ----------------------------------------------------------------


@dataclass
class Selected:
    suite: catalog.Suite
    test_filter: str | None = None
    explicit: bool = True


class UsageError(Exception):
    pass


def resolve(selectors, changed_paths=None):
    """Turn selectors into suites. Returns (selected, notes)."""
    selected = []
    notes = []

    def add(name, test_filter=None, explicit=True):
        if any(item.suite.name == name and item.test_filter == test_filter for item in selected):
            return
        selected.append(Selected(catalog.SUITES[name], test_filter, explicit))

    if changed_paths is not None:
        names = catalog.suites_for_changes(changed_paths)
        notes.append("Changed paths select: " + (", ".join(names) if names else "no suites"))
        for name in names:
            add(name, explicit=False)
    if not selectors and changed_paths is None:
        selectors = ["default"]
    for selector in selectors:
        name, _, test_filter = selector.partition("/")
        if name in catalog.GROUPS and not test_filter:
            for member in catalog.GROUPS[name][1]:
                add(member, explicit=False)
        elif name in catalog.SUITES:
            suite = catalog.SUITES[name]
            if test_filter and not suite.filterable:
                raise UsageError(f"Suite {name} does not accept a test filter.")
            add(name, test_filter or None)
        else:
            import difflib
            known = [*catalog.SUITES, *catalog.GROUPS]
            close = difflib.get_close_matches(name, known, n=1)
            hint = f" Did you mean {close[0]}?" if close else ""
            raise UsageError(f"Unknown suite or group: {name}.{hint} Run `./qtest list`.")
    return selected, notes


def changed_paths_from_git():
    """QROW_CHANGED_FILES (set by the pre-commit hook), else changes against origin/main."""
    if "QROW_CHANGED_FILES" in os.environ:
        return [line for line in os.environ["QROW_CHANGED_FILES"].splitlines() if line]
    paths = set()
    base = subprocess.run(["git", "merge-base", "HEAD", "origin/main"], cwd=ROOT,
                          capture_output=True, text=True)
    commands = [["git", "diff", "--name-only"], ["git", "diff", "--name-only", "--cached"],
                ["git", "ls-files", "--others", "--exclude-standard"]]
    if base.returncode == 0:
        commands.append(["git", "diff", "--name-only", base.stdout.strip(), "HEAD"])
    for command in commands:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        paths.update(line for line in result.stdout.splitlines() if line)
    return sorted(paths)


# Execution ----------------------------------------------------------------


@dataclass
class SuiteResult:
    name: str
    status: str  # passed, failed, skipped, missing
    duration_s: float = 0.0
    log: str | None = None
    reason: str | None = None
    failed_step: str | None = None
    missing: list = field(default_factory=list)
    failures: list = field(default_factory=list)
    metrics: list = field(default_factory=list)
    tests: list = field(default_factory=list)


class Output:
    """Writes progress to stderr unless quiet. Command output also goes to the suite log."""

    def __init__(self, quiet):
        self.quiet = quiet
        self.lock = threading.Lock()

    def note(self, message):
        if not self.quiet:
            with self.lock:
                print(f"[qtest] {message}", file=sys.stderr, flush=True)

    def stream(self, line):
        if not self.quiet:
            with self.lock:
                sys.stderr.write(line)
                sys.stderr.flush()


def render_command(step, options, test_filter, extra):
    # Replace only named placeholders; commands can contain other braces, like `find -exec {} +`.
    values = {"{python}": sys.executable, "{target}": str(target_dir()),
              "{runtime}": options.get("runtime", "native")}
    command = []
    for part in step.command:
        for placeholder, value in values.items():
            part = part.replace(placeholder, value)
        command.append(part)
    if step.nextest_filter is not None:
        expression = step.nextest_filter
        if test_filter:
            expression = f"({expression}) and test({test_filter})"
        command += ["--profile", nextest_profile(), "-E", expression, *extra]
    return command


def nextest_profile():
    return "ci" if os.environ.get("CI") else "default"


def junit_report():
    """The JUnit report that nextest writes, as .config/nextest.toml sets."""
    return target_dir() / "nextest" / nextest_profile() / "junit.xml"


def execute(command, env, timeout, log, output):
    """Run one command in its own process group. Returns the exit code, or None on timeout."""
    with log.open("a", encoding="utf-8") as sink:
        sink.write(f"$ {subprocess.list2cmdline(command)}\n")
        sink.flush()
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
                                   bufsize=1, **spawn_options())

        def forward():
            # Keep reading after a failed write. Otherwise the pipe fills and
            # the step blocks until its time limit.
            for line in iter(process.stdout.readline, ""):
                try:
                    sink.write(line)
                    sink.flush()
                    output.stream(line)
                except (OSError, UnicodeError):
                    pass

        reader = threading.Thread(target=forward, daemon=True)
        reader.start()
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            stop_tree(process)
            code = None
        except BaseException:
            stop_tree(process)
            raise
        finally:
            stop_tree(process)
        reader.join(timeout=5)
        process.stdout.close()
        if code is None:
            sink.write(f"[qtest] Stopped after the {timeout}s step limit.\n")
    return code


def junit_failures(path):
    """Failed test names and the start of their messages from a nextest JUnit report."""
    if not path.exists():
        return []
    failures = []
    for case in ElementTree.parse(path).getroot().iter("testcase"):
        problem = case.find("failure")
        if problem is None:
            problem = case.find("error")
        if problem is None:
            continue
        # The element text holds the panic message; drop the backtrace hint.
        text = problem.text or problem.get("message") or ""
        lines = [line for line in text.splitlines() if not line.startswith("note: run with")]
        failures.append({
            "test": f"{case.get('classname')}::{case.get('name')}",
            "message": "\n".join(lines).strip()[:2000],
        })
    return failures


def precheck(item, options):
    """A skipped or missing result, or None when the suite can run."""
    suite = item.suite
    if catalog.HOST not in suite.platforms:
        systems = " or ".join(catalog.PLATFORM_NAMES[name] for name in suite.platforms)
        if not item.explicit:
            return SuiteResult(suite.name, "skipped", reason=f"Requires {systems}.")
        fix = f"Run this suite on {systems}."
        return SuiteResult(suite.name, "missing", missing=[{"requirement": "platform", "fix": fix}], reason=fix)
    missing = []
    for requirement in suite.requires:
        fix = check_requirement(requirement, dict(options, fixture=suite.fixture))
        if fix:
            missing.append({"requirement": requirement, "fix": fix})
    if missing:
        return SuiteResult(suite.name, "missing", missing=missing,
                           reason="; ".join(entry["fix"] for entry in missing))
    return None


def suite_log(run_dir, suite):
    log = run_dir / "logs" / f"{suite.name}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    return log


def run_steps(item, steps, options, extra, env, run_dir, output, started, iteration=None):
    """Run steps in order. Returns a failed result, or None when all passed."""
    suite = item.suite
    log = suite_log(run_dir, suite)
    for step in steps:
        if item.test_filter and step in suite.steps and step.nextest_filter is None:
            continue
        command = render_command(step, options, item.test_filter, extra)
        prepared = options.get("prepared")
        if prepared and step.command[:2] == ("cargo", "test") and "--no-run" in step.command:
            continue
        if prepared and suite.name == "native-font":
            command = [str(target_dir() / "native-tools/tooltip-layout")]
        elif prepared and step.nextest_filter is not None:
            command = prepared.command(command)
        step_env = {key: value.replace("{target}", str(target_dir())) for key, value in step.env}
        if step.nextest_filter is not None and not options.get("isolated_reports"):
            # A step that fails before nextest writes a report must not report old failures.
            junit_report().unlink(missing_ok=True)
        report = junit_report()
        if step.nextest_filter is not None and options.get("isolated_reports"):
            store = run_dir / "nextest" / suite.name / uuid.uuid4().hex
            store.mkdir(parents=True)
            config = store / "config.toml"
            config.write_text('[store]\ndir = ' + json.dumps(str(store)) + '\n' +
                              (ROOT / ".config/nextest.toml").read_text(encoding="utf-8"))
            command += ["--config-file", str(config)]
            report = store / nextest_profile() / "junit.xml"
        adapter_env = {"QROW_PYTHON": sys.executable}
        if prepared:
            adapter_env["QROW_NEXTEST_RUNTIME"] = prepared.runtime
        evidence = None
        if suite.test_runs and step in suite.steps and options.get("isolated_reports"):
            evidence = run_dir / "adapters" / suite.name / uuid.uuid4().hex
            evidence.mkdir(parents=True)
            adapter_env["QROW_NEXTEST_EVIDENCE"] = str(evidence)
        code = execute(command, dict(os.environ, **options.get("env", {}), **env, **step_env, **adapter_env), step.timeout, log, output)
        if evidence:
            reports = [json.loads(path.read_text(encoding="utf-8")) for path in sorted(evidence.rglob("evidence.json"))]
            options.setdefault("test_evidence", []).extend(reports)
            if len(reports) != suite.test_runs:
                code = 1
        if step.nextest_filter is not None and options.get("isolated_reports") and report.exists():
            try:
                cases = list(ElementTree.parse(report).getroot().iter("testcase"))
                options.setdefault("test_evidence", []).append({
                    "count": sum(case.find("skipped") is None for case in cases),
                    "failures": sum(case.find("failure") is not None or case.find("error") is not None for case in cases),
                    "report": str(report),
                })
            except ElementTree.ParseError:
                code = 1
        if code == 0:
            continue
        result = SuiteResult(suite.name, "failed", time.monotonic() - started, str(log),
                             failed_step=subprocess.list2cmdline(command))
        result.reason = "Step timed out." if code is None else f"Step exited with {code}."
        if iteration:
            result.reason += f" Iteration {iteration[0]} of {iteration[1]}."
        if step.nextest_filter is not None:
            junit = report
            if junit.exists():
                saved = run_dir / "junit" / f"{suite.name}.xml"
                saved.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(junit, saved)
                result.failures = junit_failures(saved)
        return result
    return None


METRIC_PREFIX = "QROW_PERF "


def metrics_in(log):
    """The performance measurements that probes printed into a suite log."""
    if not log.exists():
        return []
    found = []
    for line in log.read_text(encoding="utf-8").splitlines():
        if METRIC_PREFIX in line:
            try:
                found.append(json.loads(line.split(METRIC_PREFIX, 1)[1]))
            except json.JSONDecodeError:
                continue
    return found


def run_suite(item, options, extra, run_dir, output, env=None):
    options = dict(options, test_evidence=[])
    result = run_suite_steps(item, options, extra, run_dir, output, env)
    result.metrics = metrics_in(suite_log(run_dir, item.suite))
    result.tests = options["test_evidence"]
    return result


def run_suite_steps(item, options, extra, run_dir, output, env=None):
    suite = item.suite
    started = time.monotonic()
    env = env or {}
    log = suite_log(run_dir, suite)
    label = f"{suite.name}/{item.test_filter}" if item.test_filter else suite.name
    output.note(f"{label}: {suite.summary} (log: {log})")
    repeat = options.get("repeat", 1)
    result = None
    try:
        for iteration in range(1, repeat + 1):
            if repeat > 1:
                output.note(f"{label}: iteration {iteration} of {repeat}")
            result = run_steps(item, suite.steps, options, extra, env, run_dir, output, started,
                               (iteration, repeat) if repeat > 1 else None)
            if result:
                break
    finally:
        cleanup = run_steps(item, suite.cleanup, options, extra, env, run_dir, output, started)
    result = result or cleanup
    return result or SuiteResult(suite.name, "passed", time.monotonic() - started, str(log))


class FixtureSession:
    """One server fixture for all fixture suites of a run.

    It reuses the fixture of `./qtest fixture up` when that fixture answers
    SQL. Otherwise it starts a disposable fixture and stops it at the end.
    """

    def __init__(self, options, run_dir, output, needs_docker):
        self.runtime = "docker" if needs_docker else options.get("runtime", "auto")
        self.run_dir = run_dir
        self.output = output
        self.fixture = None
        self.owned = False
        self.state = run_dir / "fixture.json"
        self.error = None

    def acquire(self):
        if self.fixture or self.error:
            return
        try:
            reused = fixture.reusable(self.runtime)
            if reused is not None:
                self.output.note(f"Using the running {reused.runtime} fixture on port {reused.port}. "
                                 "Stop it with `./qtest fixture down`.")
                self.fixture = reused
            else:
                self.fixture = fixture.start(self.runtime, self.run_dir / "fixture")
                self.owned = True
            fixture.save(self.fixture, self.state)
        except Exception as error:  # noqa: BLE001 - reported as the suite result
            self.error = f"The server fixture did not start: {error}"

    def env(self, suite):
        artifacts = self.run_dir / suite.name
        artifacts.mkdir(parents=True, exist_ok=True)
        return {"QROW_E2E_ARTIFACTS": str(artifacts), **self.fixture.env(),
                "QROW_FIXTURE_STATE": str(self.state)}

    def release(self):
        fixture = self.fixture
        self.fixture = None
        if fixture is None:
            return
        try:
            fixture.collect(self.run_dir / "fixture")
        finally:
            if self.owned:
                fixture.stop()


def new_run_dir():
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    path = runs_dir() / f"{stamp}-{os.getpid()}-{uuid.uuid4().hex[:8]}"
    path.mkdir(parents=True)
    latest = runs_dir() / "latest"
    from contract import atomic_json
    atomic_json(runs_dir() / "latest.json", {"run": path.name})
    if os.name != "nt":
        temporary = runs_dir() / f".latest-{uuid.uuid4().hex}"
        temporary.symlink_to(path.name)
        os.replace(temporary, latest)
    return path


def run(selected, options, extra, output, fail_fast=False, report_only=(), require_all=False):
    from contract import source_sha
    options = dict(options, env=dict(options.get("env", {})))
    # Give each run an immutable build identity. build.rs otherwise watches all
    # Git refs, which background fetches in another worktree can change.
    if not options["env"].get("QROW_COMMIT") and not os.environ.get("QROW_COMMIT"):
        sha = source_sha(ROOT)
        if sha:
            options["env"]["QROW_COMMIT"] = sha[:12]
    resources = ([f"cargo:{target_dir().resolve()}"]
                 if any("cargo" in item.suite.requires or item.suite.name == "package-smoke" for item in selected) else [])
    if any(item.suite.fixture for item in selected):
        resources.append(f"fixture:{fixture.state_path().resolve()}")
    if any(item.suite.name in ("desktop", "perf-app", "perf-package") for item in selected):
        resources.append("desktop")
    with lease(resources):
        return _run(selected, options, extra, output, fail_fast, report_only, require_all)


def _run(selected, options, extra, output, fail_fast=False, report_only=(), require_all=False):
    """Run the selected suites. A failure of a `report_only` suite does not fail the run.

    With `require_all`, a missing prerequisite of a suite that is not
    report-only stops the run before any suite, build, or server starts.
    """
    run_dir = new_run_dir()
    results = [None] * len(selected)
    blocked = [precheck(item, options) for item in selected]
    if require_all and any(result and result.status == "missing" and result.name not in report_only
                           for result in blocked):
        blocked = [result or SuiteResult(item.suite.name, "skipped", reason="A prerequisite of the run is missing.")
                   for item, result in zip(selected, blocked)]
    served = [index for index, item in enumerate(selected) if item.suite.fixture and blocked[index] is None]
    session = FixtureSession(options, run_dir, output,
                             any(selected[index].suite.fixture == "docker" for index in served)) if served else None
    prepared = False
    prepared_suites = set()
    finalized = set()
    interrupted = False
    cleanup_errors = []
    events = run_dir / "events.jsonl"

    def event(kind, **data):
        with events.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps({"event": kind, "time": time.monotonic(), **data}) + "\n")

    event("started", suites=[item.suite.name for item in selected])
    try:
        if options.get("reuse_builds") and any(result is None for result in blocked):
            import builds
            from contract import plan
            from targets import host_target
            value = plan(ROOT, host_target().name, list(dict.fromkeys(item.suite.name for item in selected)), options.get("runtime", "auto"))
            options = dict(options, prepared=builds.Prepared(options["reuse_builds"], value, ROOT, run_dir / "prepared", output))
        for index, item in enumerate(selected):
            name = item.suite.name
            if fail_fast and any(result and result.status in ("failed", "missing")
                                 and result.name not in report_only for result in results):
                results[index] = SuiteResult(name, "skipped", reason="An earlier suite did not pass.")
                continue
            if item.suite.fixture and blocked[index] is None and not prepared:
                # Build everything that the fixture suites need before servers use memory.
                prepared = True
                # Suites can share a step, like the build of one test binary. It runs once.
                done = {}
                for other in served:
                    suite = selected[other].suite
                    prepared_suites.add(other)
                    failure = None
                    for step in suite.prepare:
                        if step not in done:
                            done[step] = run_steps(selected[other], (step,), options, extra,
                                                   {"QROW_E2E_ARTIFACTS": str(run_dir / suite.name)},
                                                   run_dir, output, time.monotonic())
                        failure = done[step]
                        if failure:
                            failure = dataclasses.replace(failure, name=suite.name)
                            break
                    if failure:
                        failure.reason = "Preparation failed. " + failure.reason
                        blocked[other] = failure
                if any(blocked[other] is None for other in served):
                    session.acquire()
            if blocked[index]:
                results[index] = blocked[index]
            elif item.suite.fixture and session.error:
                results[index] = SuiteResult(name, "failed", reason=session.error)
            elif item.suite.fixture:
                finalized.add(index)
                results[index] = run_suite(item, options, extra, run_dir, output, session.env(item.suite))
            else:
                finalized.add(index)
                results[index] = run_suite(item, options, extra, run_dir, output)
            if session and index == served[-1]:
                try:
                    session.release()
                except Exception as error:
                    cleanup_errors.append(str(error))
                    event("cleanup_failed", reason=str(error))
            result = results[index]
            event("suite_finished", name=result.name, status=result.status, reason=result.reason)
            output.note(f"{result.name}: {result.status}" + (f" ({result.reason})" if result.reason else ""))
    except (Exception, KeyboardInterrupt) as error:
        interrupted = isinstance(error, KeyboardInterrupt)
        for index, result in enumerate(results):
            if result is None:
                results[index] = SuiteResult(selected[index].suite.name, "failed",
                                             reason="Cancelled." if interrupted else f"Execution error: {error}")
        if not interrupted:
            cleanup_errors.append(str(error))
        event("interrupted" if interrupted else "execution_error", reason=str(error))
    finally:
        for index in sorted(prepared_suites - finalized):
            item = selected[index]
            try:
                cleanup = run_steps(item, item.suite.cleanup, options, extra,
                                    {"QROW_E2E_ARTIFACTS": str(run_dir / item.suite.name)},
                                    run_dir, output, time.monotonic())
                if cleanup:
                    cleanup_errors.append(cleanup.reason)
            except Exception as error:
                cleanup_errors.append(str(error))
        if session:
            try:
                session.release()
            except Exception as error:
                cleanup_errors.append(str(error))
                event("cleanup_failed", reason=str(error))
    statuses = {result.status for result in results if result.name not in report_only}
    if interrupted:
        status, code = "cancelled", 130
    elif "failed" in statuses or cleanup_errors:
        status, code = "failed", EXIT_FAILED
    elif "missing" in statuses:
        status, code = "missing", EXIT_MISSING
    else:
        status, code = "passed", EXIT_PASSED
    summary = {
        "run": run_dir.name,
        "status": status,
        "exit_code": code,
        "artifacts": str(run_dir),
        "suites": [{**{key: value for key, value in vars(result).items() if value not in (None, [])},
                    **({"report_only": True} if result.name in report_only else {})}
                   for result in results],
    }
    if cleanup_errors:
        summary["cleanup_errors"] = cleanup_errors
    from contract import atomic_json
    atomic_json(run_dir / "summary.json", summary)
    event("finished", status=status, exit_code=code)
    measured = [metric for result in results for metric in result.metrics]
    if measured:
        (run_dir / "perf.json").write_text(json.dumps(measured, indent=2) + "\n", encoding="utf-8")
    return summary

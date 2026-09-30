"""Run suites: check requirements, execute steps, collect logs and failures."""
import dataclasses
from dataclasses import dataclass, field
import datetime
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ElementTree

import catalog

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/e2e"))
import fixture  # noqa: E402

# Exit codes are part of the CLI contract; docs/testing.md lists them.
EXIT_PASSED = 0
EXIT_FAILED = 1
EXIT_USAGE = 2
EXIT_MISSING = 3


def target_dir():
    return Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))


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


def check_requirement(name, options):
    """Return None when the requirement holds, else a fix for the user."""
    fixes = catalog.REQUIREMENTS
    if name in ("cargo", "uv", "shellcheck", "actionlint"):
        return None if shutil.which(name) else fixes[name]
    if name == "macos":
        return None if catalog.MACOS else fixes[name]
    if name == "xcode":
        return None if shutil.which("xcode-select") and _succeeds(["xcode-select", "-p"]) else fixes[name]
    if name == "docker":
        if not shutil.which("docker"):
            return fixes["docker"]
        return None if _succeeds(["docker", "info"]) else fixes["docker-running"]
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
        java_home = os.environ.get("JAVA_HOME")
        java = java_home and all((Path(java_home) / "bin" / tool).is_file() for tool in ("java", "javac", "jar"))
        if runtime == "native":
            return None if java else fixes["java"]
        # auto: Docker when its daemon answers, else local Java processes.
        if check_requirement("docker", options) is None or java:
            return None
        return fixes["java"]
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
        profile = "ci" if os.environ.get("CI") else "default"
        command += ["--profile", profile, "-E", expression, *extra]
    return command


def execute(command, env, timeout, log, output):
    """Run one command in its own process group. Returns the exit code, or None on timeout."""
    with log.open("a") as sink:
        sink.write(f"$ {subprocess.list2cmdline(command)}\n")
        sink.flush()
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, bufsize=1,
                                   start_new_session=True)

        def forward():
            for line in iter(process.stdout.readline, ""):
                sink.write(line)
                sink.flush()
                output.stream(line)

        reader = threading.Thread(target=forward, daemon=True)
        reader.start()
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            code = None
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
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
    if suite.macos_only and not catalog.MACOS and not item.explicit:
        return SuiteResult(suite.name, "skipped", reason="Requires macOS.")
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
        step_env = {key: value.replace("{target}", str(target_dir())) for key, value in step.env}
        code = execute(command, dict(os.environ, **env, **step_env), step.timeout, log, output)
        if code == 0:
            continue
        result = SuiteResult(suite.name, "failed", time.monotonic() - started, str(log),
                             failed_step=subprocess.list2cmdline(command))
        result.reason = "Step timed out." if code is None else f"Step exited with {code}."
        if iteration:
            result.reason += f" Iteration {iteration[0]} of {iteration[1]}."
        if step.nextest_filter is not None:
            profile = "ci" if os.environ.get("CI") else "default"
            junit = target_dir() / "nextest" / profile / "junit.xml"
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
    for line in log.read_text().splitlines():
        if METRIC_PREFIX in line:
            try:
                found.append(json.loads(line.split(METRIC_PREFIX, 1)[1]))
            except json.JSONDecodeError:
                continue
    return found


def run_suite(item, options, extra, run_dir, output, env=None):
    result = run_suite_steps(item, options, extra, run_dir, output, env)
    result.metrics = metrics_in(suite_log(run_dir, item.suite))
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
        if self.fixture is None:
            return
        try:
            self.fixture.collect(self.run_dir / "fixture")
        finally:
            if self.owned:
                self.fixture.stop()


def new_run_dir():
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    path = runs_dir() / f"{stamp}-{os.getpid()}"
    path.mkdir(parents=True)
    latest = runs_dir() / "latest"
    if latest.is_symlink() or latest.exists():
        latest.unlink()
    latest.symlink_to(path.name)
    return path


def run(selected, options, extra, output, fail_fast=False, report_only=(), require_all=False):
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
    try:
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
                results[index] = run_suite(item, options, extra, run_dir, output, session.env(item.suite))
            else:
                results[index] = run_suite(item, options, extra, run_dir, output)
            result = results[index]
            output.note(f"{result.name}: {result.status}" + (f" ({result.reason})" if result.reason else ""))
    finally:
        if session:
            session.release()
    statuses = {result.status for result in results if result.name not in report_only}
    if "failed" in statuses:
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
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    measured = [metric for result in results for metric in result.metrics]
    if measured:
        (run_dir / "perf.json").write_text(json.dumps(measured, indent=2) + "\n")
    return summary

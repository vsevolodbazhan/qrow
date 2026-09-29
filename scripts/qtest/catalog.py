"""What qtest can run: suites, groups, requirements, pinned tools, and path rules.

This file is the source of truth for the test catalog. docs/testing.md
describes the same suites; scripts/tests/test_qtest.py keeps them in sync.
"""
from dataclasses import dataclass
import platform
import re

MACOS = platform.system() == "Darwin"


@dataclass(frozen=True)
class Tool:
    """A pinned development tool that `qtest install` provides."""
    name: str
    version: str
    version_command: tuple[str, ...]
    version_prefix: str
    install: tuple[tuple[str, ...], ...]


TOOLS = {
    tool.name: tool
    for tool in [
        Tool("cargo-nextest", "0.9.146", ("cargo", "nextest", "--version"), "cargo-nextest 0.9.146",
             (("cargo", "install", "--locked", "cargo-nextest", "--version", "0.9.146"),)),
        Tool("cargo-deny", "0.20.2", ("cargo", "deny", "--version"), "cargo-deny 0.20.2",
             (("cargo", "install", "--locked", "cargo-deny", "--version", "0.20.2"),)),
        Tool("cargo-machete", "0.9.2", ("cargo", "machete", "--version"), "0.9.2",
             (("cargo", "install", "--locked", "cargo-machete", "--version", "0.9.2"),)),
        Tool("cargo-llvm-cov", "0.9.1", ("cargo", "llvm-cov", "--version"), "cargo-llvm-cov 0.9.1",
             (("cargo", "install", "--locked", "cargo-llvm-cov", "--version", "0.9.1"),
              ("rustup", "component", "add", "llvm-tools-preview"))),
    ]
}


@dataclass(frozen=True)
class Step:
    """One command of a suite. `nextest` steps produce JUnit results and accept filters."""
    command: tuple[str, ...]
    env: tuple[tuple[str, str], ...] = ()
    nextest_filter: str | None = None
    timeout: int = 30 * 60


@dataclass(frozen=True)
class Suite:
    name: str
    summary: str
    requires: tuple[str, ...]
    steps: tuple[Step, ...]
    macos_only: bool = False
    # Suites that start disposable servers or take over the desktop run only
    # when you select them by name.
    explicit_only: bool = False
    # "any" or "docker": the suite runs against the shared server fixture.
    fixture: str | None = None
    # Steps that run before the fixture starts, like builds, so that compilers
    # and servers do not use memory at the same time.
    prepare: tuple[Step, ...] = ()
    # Steps that run after the suite, also when it failed.
    cleanup: tuple[Step, ...] = ()

    @property
    def filterable(self):
        return any(step.nextest_filter is not None for step in self.steps)


def nextest(expression, *arguments):
    return Step(("cargo", "nextest", "run", "--locked", *arguments), nextest_filter=expression)


PYTHON = "{python}"  # Replaced with the interpreter that runs qtest.

_UNIT = (
    (nextest("not binary(ui)"), Step(("cargo", "test", "--locked", "--doc")))
    if MACOS else
    (nextest("all()", "--no-default-features"),
     Step(("cargo", "test", "--locked", "--no-default-features", "--doc")))
)

SUITES = {
    suite.name: suite
    for suite in [
        Suite("fmt", "Rust formatting.", ("cargo",),
              (Step(("cargo", "fmt", "--all", "--", "--check")),)),
        Suite("clippy", "Clippy for the core library and, on macOS, the application.", ("cargo",),
              (Step(("cargo", "clippy", "--locked", "--no-default-features", "--all-targets", "--", "-D", "warnings")),
               *((Step(("cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings")),) if MACOS else ()))),
        Suite("rustdoc", "Rust API documentation without warnings.", ("cargo",),
              (Step(("cargo", "doc", "--locked", "--no-default-features", "--no-deps"),
                    env=(("RUSTDOCFLAGS", "-D warnings"),)),)),
        Suite("unit", "Rust unit and integration tests that need no UI and no servers.",
              ("cargo", "cargo-nextest"), _UNIT),
        Suite("ui", "Headless tests of the real Qrow window, without servers.",
              ("cargo", "cargo-nextest", "macos"), (nextest("binary(ui)"),), macos_only=True),
        Suite("coverage", "Core line coverage with an 80% floor.", ("cargo", "cargo-llvm-cov"),
              (Step(("mkdir", "-p", "{target}/coverage")),
               Step(("cargo", "llvm-cov", "--locked", "--no-default-features", "--lib", "--tests",
                     "--ignore-filename-regex", "src/connector/t_c_l_i_service.rs|tests/|src/bin/",
                     "--fail-under-lines", "80", "--lcov", "--output-path", "{target}/coverage/core.lcov")),)),
        Suite("perf", "SQL validation benchmark with enforced budgets.", ("cargo",),
              (Step(("cargo", "bench", "--locked", "--no-default-features", "--bench", "sql")),)),
        Suite("scripts", "ShellCheck, actionlint, Ruff, and automation unit tests.",
              ("uv", "shellcheck", "actionlint"),
              (Step(("sh", "-c", "find scripts -type f -name '*.sh' -exec shellcheck {} + && shellcheck .githooks/* qtest")),
               # Name the files: hook snapshots have no Git repository to search.
               Step(("sh", "-c", "actionlint .github/workflows/*.yml")),
               Step((PYTHON, "-m", "ruff", "check", "scripts")),
               Step((PYTHON, "-m", "unittest", "discover", "-s", "scripts/tests")))),
        Suite("policy", "Dependency waiver dates and pinned CI actions.", ("uv",),
              (Step((PYTHON, "scripts/core/policy.py")),)),
        Suite("deps", "Dependency policy, unused dependencies, advisories, licenses, and sources.",
              ("uv", "cargo", "cargo-machete", "cargo-deny"),
              (Step((PYTHON, "scripts/core/policy.py")),
               Step(("cargo", "machete")),
               Step(("cargo", "deny", "--locked", "check")))),
        Suite("backend", "Connector and worker against the real servers, without the UI.",
              ("cargo", "cargo-nextest", "fixture-runtime"),
              (nextest("binary(backend)", "--no-default-features", "--run-ignored", "only"),),
              explicit_only=True, fixture="docker",
              prepare=(Step(("cargo", "test", "--locked", "--no-default-features", "--no-run", "--test", "backend")),)),
        Suite("e2e", "The real Qrow window, headless, against the real servers.",
              ("cargo", "cargo-nextest", "macos", "fixture-runtime"),
              (nextest("binary(e2e) & not test(/^perf::/)", "--run-ignored", "only"),),
              macos_only=True, explicit_only=True, fixture="any",
              prepare=(Step(("cargo", "test", "--locked", "--no-run", "--test", "e2e")),)),
        Suite("perf-ui", "Frame, scroll, and editor timings of the real window in a release-like build.",
              ("cargo", "cargo-nextest", "macos"),
              (nextest("binary(perf)", "--cargo-profile", "perf", "--run-ignored", "only", "--no-capture"),),
              macos_only=True, explicit_only=True),
        Suite("perf-e2e", "Query and page latency of the real window against the real servers.",
              ("cargo", "cargo-nextest", "macos", "fixture-runtime"),
              (nextest("binary(e2e) & test(/^perf::/)", "--run-ignored", "only", "--no-capture"),),
              macos_only=True, explicit_only=True, fixture="any",
              prepare=(Step(("cargo", "test", "--locked", "--no-run", "--test", "e2e")),)),
        Suite("perf-app", "Launch time, idle memory, and idle CPU of the release app on the desktop.",
              ("cargo", "uv", "macos"),
              (Step(("cargo", "build", "--locked", "--release", "--bin", "qrow"), timeout=40 * 60),
               Step((PYTHON, "scripts/perf/app.py"))),
              macos_only=True, explicit_only=True),
        Suite("package", "The release app package, built in the target directory, and its size budget.",
              ("cargo", "uv", "macos", "xcode"),
              (Step(("sh", "scripts/package/macos.sh"), env=(("QROW_DIST_DIR", "{target}/package"),), timeout=40 * 60),
               Step((PYTHON, "scripts/core/size.py"), env=(("QROW_DIST_DIR", "{target}/package"),))),
              macos_only=True, explicit_only=True),
        Suite("desktop", "Smoke checks of the packaged app on the desktop: the menu bar, Keychain, quit, and pixels.",
              ("uv", "cargo", "macos", "xcode", "desktop", "fixture-runtime"),
              (Step(("sh", "scripts/e2e/driver.sh", "--prepared"), timeout=90 * 60),),
              macos_only=True, explicit_only=True, fixture="any",
              prepare=(Step(("sh", "scripts/e2e/driver.sh", "--prepare"), timeout=30 * 60),),
              # The driver removes its synthetic Keychain items on exit. This
              # step also removes them when a time limit stopped the driver.
              cleanup=(Step((PYTHON, "scripts/e2e/keychain.py")),)),
    ]
}

# `./qtest compare` runs these suites when you name none. They need no servers.
COMPARE_SUITES = ("perf", "perf-ui", "perf-app")

GROUPS = {
    "default": ("Fast local checks. Runs when you give no selector.",
                ("fmt", "clippy", "unit", "ui")),
    "all": ("Every local suite without servers. CI runs these suites and the suites with servers.",
            ("scripts", "deps", "fmt", "clippy", "rustdoc", "unit", "ui", "perf", "coverage")),
}

# Changed paths select suites for `qtest run --changed` and the hooks.
RUST_PATHS = (r"^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|build\.rs|src/|tests/(?!fixture/)|benches/|vendor/"
              r"|themes/|assets/(app-icons|connection-type-icons)/|\.config/nextest\.toml$)")
RUST_SUITES = ("fmt", "clippy", "rustdoc", "unit", "ui")
CHANGE_RULES = (
    (RUST_PATHS, RUST_SUITES),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml|scripts/core/policy\.py|\.github/workflows/)",
     ("policy",)),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml)", ("deps",)),
    (r"^(\.githooks/|\.github/workflows/|pyproject\.toml$|uv\.lock$|scripts/|qtest$|docs/testing\.md$)",
     ("scripts",)),
    (r"^scripts/core/preflight\.sh$", (*RUST_SUITES, "policy")),
)

# `qtest hook NAME` runs the suites that the changed paths select, but only
# those of the hook. A commit gets the checks that take seconds. A push also
# gets the tests. CI runs everything, including `perf` and `coverage`.
HOOKS = {
    "pre-commit": ("Static checks of the staged files.", ("scripts", "policy", "fmt", "clippy")),
    "pre-push": ("Static checks and tests of the pushed changes.",
                 ("scripts", "policy", "deps", "fmt", "clippy", "rustdoc", "unit", "ui")),
}


def suites_for_changes(paths):
    """Return suite names in catalog order for the changed repository paths."""
    selected = set()
    for path in paths:
        for pattern, names in CHANGE_RULES:
            if re.search(pattern, path):
                selected.update(names)
    return [name for name in SUITES if name in selected]


def suites_for_hook(hook, paths):
    """Return the suites that the changed paths select and that the hook runs."""
    return [name for name in suites_for_changes(paths) if name in HOOKS[hook][1]]


@dataclass(frozen=True)
class CiJob:
    """One job of the checks workflow. `./qtest ci NAME` runs its suites."""
    name: str
    summary: str
    runner: str
    suites: tuple[str, ...]
    # Suites that run and report their measurements, but do not fail the job.
    report_only: tuple[str, ...] = ()
    # Jobs whose failure predicts a failure of this job, or whose output it uses.
    needs: tuple[str, ...] = ()
    # A pull request runs the job when it changes a path that matches.
    paths: str = r"^"
    runtime: str = "auto"


LINUX, MACOS_RUNNER = "ubuntu-24.04", "macos-15"
E2E_PATHS = r"^(tests/fixture/|scripts/e2e/)"
CI_JOBS = {
    job.name: job
    for job in [
        CiJob("static", "Formatting, lint, API docs, dependency policy, and scripts.", LINUX,
              ("scripts", "policy", "deps", "fmt", "clippy", "rustdoc")),
        CiJob("core", "Core unit tests with line coverage.", LINUX, ("coverage",), paths=RUST_PATHS),
        CiJob("ui", "Lint of the application, unit tests, and the headless window.", MACOS_RUNNER,
              ("clippy", "unit", "ui"), needs=("core",), paths=RUST_PATHS),
        CiJob("package", "The release package, its size, and the launch and idle probes of the app.",
              MACOS_RUNNER, ("package",), report_only=("perf-app",), needs=("core",),
              paths=rf"{RUST_PATHS}|^(scripts/package/|scripts/perf/|assets/|LICENSE$|NOTICE$)"),
        CiJob("backend", "Connector and worker against the servers in Docker.", LINUX, ("backend",),
              needs=("core",), paths=rf"{RUST_PATHS}|{E2E_PATHS}", runtime="docker"),
        CiJob("e2e", "The window and the package against local Java servers, and the query probes.",
              MACOS_RUNNER, ("e2e", "desktop"), report_only=("perf-e2e",), needs=("package",),
              paths=rf"{RUST_PATHS}|{E2E_PATHS}|^tests/desktop/", runtime="native"),
        CiJob("perf", "The SQL benchmark budgets, and the frame and editor probes.", MACOS_RUNNER,
              ("perf",), report_only=("perf-ui",), needs=("core",), paths=RUST_PATHS),
    ]
}
# A change to these paths can change any job, so it runs all of them.
CI_ALL_PATHS = r"^(\.github/workflows/|scripts/qtest/|qtest$|scripts/core/|pyproject\.toml$|uv\.lock$)"


def ci_jobs_for_changes(paths):
    """Return the CI jobs for the changed paths, with the jobs that they wait for."""
    selected = {"static"}
    for path in paths:
        if re.search(CI_ALL_PATHS, path):
            return list(CI_JOBS)
        selected.update(name for name, job in CI_JOBS.items() if re.search(job.paths, path))
    pending = list(selected)
    while pending:
        for need in CI_JOBS[pending.pop()].needs:
            if need not in selected:
                selected.add(need)
                pending.append(need)
    return [name for name in CI_JOBS if name in selected]


REQUIREMENTS = {
    "cargo": "Install Rust with rustup. rust-toolchain.toml selects the toolchain.",
    "uv": "Install uv: https://docs.astral.sh/uv/getting-started/installation/",
    "shellcheck": "Install ShellCheck, for example `brew install shellcheck`.",
    "actionlint": "Install actionlint, for example `brew install actionlint`.",
    "macos": "Run this suite on macOS.",
    "xcode": "Install the Xcode command-line tools with `xcode-select --install`.",
    "docker": "Install Docker with Compose v2.",
    "docker-running": "Docker is installed but not running. Start Docker Desktop, then run again.",
    "desktop": "Use an unlocked, logged-in macOS session. Grant Accessibility and Screen Recording "
               "to the terminal; see target/e2e-tools/preflight.log.",
    "java": "Set JAVA_HOME to a Java 17 JDK, or use `--runtime docker`.",
    "fixture-docker": "This suite stops engines and restarts Kyuubi, which needs the Docker runtime. "
                      "Use `--runtime docker` or `--runtime auto` with Docker running.",
    **{name: f"Run `./qtest install {name}` (pinned: {tool.version})." for name, tool in TOOLS.items()},
}

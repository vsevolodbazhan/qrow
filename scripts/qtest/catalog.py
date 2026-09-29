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
              (nextest("binary(e2e)", "--run-ignored", "only"),),
              macos_only=True, explicit_only=True, fixture="any",
              prepare=(Step(("cargo", "test", "--locked", "--no-run", "--test", "e2e")),)),
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

GROUPS = {
    "default": ("Fast local checks. Runs when you give no selector.",
                ("fmt", "clippy", "unit", "ui")),
    "all": ("Every local suite without servers. The pre-push hook runs this group.",
            ("scripts", "deps", "fmt", "clippy", "rustdoc", "unit", "ui", "perf", "coverage")),
}

# Changed paths select suites for `qtest run --changed` and the pre-commit hook.
RUST_PATHS = (r"^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|build\.rs|src/|tests/|benches/|vendor/"
              r"|themes/|assets/(app-icons|connection-type-icons)/|\.config/nextest\.toml$)")
CHANGE_RULES = (
    (RUST_PATHS, ("fmt", "clippy", "unit", "ui")),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml|scripts/core/policy\.py|\.github/workflows/)",
     ("policy",)),
    (r"^(\.githooks/|\.github/workflows/|pyproject\.toml$|uv\.lock$|scripts/|qtest$|docs/testing\.md$)",
     ("scripts",)),
    (r"^scripts/core/preflight\.sh$", ("fmt", "clippy", "unit", "ui", "policy")),
)


def suites_for_changes(paths):
    """Return suite names in catalog order for the changed repository paths."""
    selected = set()
    for path in paths:
        for pattern, names in CHANGE_RULES:
            if re.search(pattern, path):
                selected.update(names)
    return [name for name in SUITES if name in selected]


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

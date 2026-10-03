"""What qtest can run: suites, groups, requirements, pinned tools, and path rules.

This file is the source of truth for the test catalog. docs/testing.md
describes the same suites; scripts/tests/test_qtest.py keeps them in sync.
"""
from dataclasses import dataclass
import platform
import re

MACOS = platform.system() == "Darwin"


def host_platform():
    """The platform of the prebuilt tool archives for this computer, like `macos-aarch64`, or None."""
    system = {"Darwin": "macos", "Linux": "linux"}.get(platform.system())
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    return f"{system}-{machine}" if system else None


@dataclass(frozen=True)
class Archive:
    """A prebuilt release archive of a tool, pinned by its SHA-256 digest."""
    platform: str
    url: str
    sha256: str
    # The path of the executable in the archive.
    member: str


@dataclass(frozen=True)
class Tool:
    """A pinned development tool that `qtest install` provides."""
    name: str
    version: str
    version_command: tuple[str, ...]
    version_prefix: str
    install: tuple[tuple[str, ...], ...]
    # `qtest install` downloads the archive for this computer, checks its
    # digest, and falls back to `install` when there is none. A download takes
    # seconds, a build minutes.
    archives: tuple[Archive, ...] = ()
    # Commands that run after either kind of installation.
    setup: tuple[tuple[str, ...], ...] = ()


def github_archives(repository, tag, member, platforms):
    """Archives of a GitHub release. `platforms` maps a platform to (asset name, SHA-256)."""
    return tuple(Archive(key, f"https://github.com/{repository}/releases/download/{tag}/{asset}", digest,
                         member.format(asset=asset.removesuffix(".tar.gz")))
                 for key, (asset, digest) in platforms.items())


# The digests are the SHA-256 digests of the release assets, calculated from
# the downloaded files. CI uses Linux x86_64 and macOS ARM64 and x86_64.
NEXTEST_MACOS = ("cargo-nextest-0.9.146-universal-apple-darwin.tar.gz",
                 "39785160b3c2f6ed9a765049cf4fa79f3b39aa02eb7598a5a0e2a1a0b9ffb9a8")
LLVM_COV_MACOS = ("cargo-llvm-cov-universal-apple-darwin.tar.gz",
                  "cc00420e3a5500e4d603399fde223d76d84c1446102d1b60a9fbbbce1c1c2b52")
TOOLS = {
    tool.name: tool
    for tool in [
        Tool("cargo-nextest", "0.9.146", ("cargo", "nextest", "--version"), "cargo-nextest 0.9.146",
             (("cargo", "install", "--locked", "cargo-nextest", "--version", "0.9.146"),),
             github_archives("nextest-rs/nextest", "cargo-nextest-0.9.146", "cargo-nextest", {
                 "macos-aarch64": NEXTEST_MACOS,
                 "macos-x86_64": NEXTEST_MACOS,
                 "linux-x86_64": ("cargo-nextest-0.9.146-x86_64-unknown-linux-gnu.tar.gz",
                                  "682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428"),
                 "linux-aarch64": ("cargo-nextest-0.9.146-aarch64-unknown-linux-gnu.tar.gz",
                                   "b2e33d7c72de7ade0ff7b3a948ac37516b24f8a836b7a8870c1f634a94be9de9"),
             })),
        Tool("cargo-deny", "0.20.2", ("cargo", "deny", "--version"), "cargo-deny 0.20.2",
             (("cargo", "install", "--locked", "cargo-deny", "--version", "0.20.2"),),
             github_archives("EmbarkStudios/cargo-deny", "0.20.2", "{asset}/cargo-deny", {
                 "macos-aarch64": ("cargo-deny-0.20.2-aarch64-apple-darwin.tar.gz",
                                   "fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3"),
                 "linux-x86_64": ("cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz",
                                  "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"),
             })),
        Tool("cargo-machete", "0.9.2", ("cargo", "machete", "--version"), "0.9.2",
             (("cargo", "install", "--locked", "cargo-machete", "--version", "0.9.2"),),
             github_archives("bnjbvr/cargo-machete", "v0.9.2", "{asset}/cargo-machete", {
                 "macos-aarch64": ("cargo-machete-v0.9.2-aarch64-apple-darwin.tar.gz",
                                   "63e28fee386d82d33f2d12406c857f98e2d4697f3f7df7f71f34dff07fca0fde"),
                 "linux-x86_64": ("cargo-machete-v0.9.2-x86_64-unknown-linux-musl.tar.gz",
                                  "48200087f54c55aabcd4db4af1e25742b49846c02a1b1bfa134711945b35b2e9"),
             })),
        Tool("cargo-llvm-cov", "0.9.1", ("cargo", "llvm-cov", "--version"), "cargo-llvm-cov 0.9.1",
             (("cargo", "install", "--locked", "cargo-llvm-cov", "--version", "0.9.1"),),
             github_archives("taiki-e/cargo-llvm-cov", "v0.9.1", "cargo-llvm-cov", {
                 "macos-aarch64": LLVM_COV_MACOS,
                 "macos-x86_64": LLVM_COV_MACOS,
                 "linux-x86_64": ("cargo-llvm-cov-x86_64-unknown-linux-gnu.tar.gz",
                                  "b3f68e625481fed9b16444174f3fa5ebcdbde4a1878803a35eabe2dcefcdc41a"),
             }),
             setup=(("rustup", "component", "add", "llvm-tools-preview"),)),
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

CLIPPY_CORE = Step(("cargo", "clippy", "--locked", "--no-default-features", "--all-targets", "--", "-D", "warnings"))
CLIPPY_APP = Step(("cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings"))
# The e2e and perf-e2e suites use the same test binary. A run builds it once.
E2E_BUILD = Step(("cargo", "test", "--locked", "--no-run", "--test", "e2e"))

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
              (CLIPPY_CORE, *((CLIPPY_APP,) if MACOS else ()))),
        # The ui CI job lints the application. The static job lints the core library.
        Suite("clippy-app", "Clippy for the application with all features.", ("cargo", "macos"),
              (CLIPPY_APP,), macos_only=True),
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
              (Step(("sh", "-c", "find scripts -type f -name '*.sh' -exec shellcheck {} + "
                           "&& shellcheck .githooks/* qtest tests/desktop/*.sh")),
               # Name the files: hook snapshots have no Git repository to search.
               Step(("sh", "-c", "actionlint .github/workflows/*.yml")),
               Step((PYTHON, "-m", "ruff", "check", "scripts", "tests/desktop")),
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
              (nextest("binary(integration) & test(/^backend::/)", "--no-default-features", "--run-ignored", "only"),),
              explicit_only=True, fixture="docker",
              prepare=(Step(("cargo", "test", "--locked", "--no-default-features", "--no-run", "--test", "integration")),)),
        Suite("e2e", "The real Qrow window, headless, against the real servers.",
              ("cargo", "cargo-nextest", "macos", "fixture-runtime"),
              (nextest("binary(e2e) & not test(/^perf::/)", "--run-ignored", "only"),),
              macos_only=True, explicit_only=True, fixture="any",
              prepare=(E2E_BUILD,)),
        Suite("perf-ui", "Frame, scroll, editor, assistant, and Activity timings of the real window in a release-like build.",
              ("cargo", "cargo-nextest", "macos"),
              (nextest("binary(perf)", "--cargo-profile", "perf", "--run-ignored", "only", "--no-capture"),),
              macos_only=True, explicit_only=True),
        Suite("perf-e2e", "Query and page latency of the real window against the real servers.",
              ("cargo", "cargo-nextest", "macos", "fixture-runtime"),
              (nextest("binary(e2e) & test(/^perf::/)", "--run-ignored", "only", "--no-capture"),),
              macos_only=True, explicit_only=True, fixture="any",
              prepare=(E2E_BUILD,)),
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
              r"|\.cargo/config\.toml$|themes/|assets/(app-icons|connection-type-icons)/|\.config/nextest\.toml$)")
RUST_SUITES = ("fmt", "clippy", "rustdoc", "unit", "ui")
CHANGE_RULES = (
    (RUST_PATHS, RUST_SUITES),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml|scripts/core/policy\.py|\.github/workflows/)",
     ("policy",)),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml)", ("deps",)),
    # The scripts suite also lints the Python and shell helpers of the desktop driver.
    (r"^(\.githooks/|\.github/workflows/|pyproject\.toml$|uv\.lock$|scripts/|qtest$|docs/testing\.md$"
     r"|tests/desktop/[^/]+\.(py|sh)$)",
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
        CiJob("static", LINUX, ("scripts", "policy", "deps", "fmt", "clippy", "rustdoc")),
        CiJob("core", LINUX, ("coverage",), paths=RUST_PATHS),
        CiJob("ui", MACOS_RUNNER, ("clippy-app", "unit", "ui"), needs=("core",), paths=RUST_PATHS),
        CiJob("package", MACOS_RUNNER, ("package",), report_only=("perf-app",), needs=("core",),
              paths=rf"{RUST_PATHS}|^(scripts/package/|scripts/perf/|assets/|LICENSE$|NOTICE$)"),
        CiJob("backend", LINUX, ("backend",), needs=("core",), paths=rf"{RUST_PATHS}|{E2E_PATHS}",
              runtime="docker"),
        CiJob("e2e", MACOS_RUNNER, ("e2e", "desktop"), report_only=("perf-e2e",), needs=("package",),
              paths=rf"{RUST_PATHS}|{E2E_PATHS}|^tests/desktop/", runtime="native"),
        CiJob("perf", MACOS_RUNNER, ("perf",), report_only=("perf-ui",), needs=("core",), paths=RUST_PATHS),
        CiJob("ui-intel", "macos-15-intel", ("clippy-app", "unit", "ui"), needs=("core",), paths=RUST_PATHS),
        CiJob("package-intel", "macos-15-intel", ("package",), report_only=("perf-app",), needs=("core",),
              paths=rf"{RUST_PATHS}|^(scripts/package/|scripts/perf/|assets/|LICENSE$|NOTICE$)"),
        CiJob("e2e-intel", "macos-15-intel", ("e2e", "desktop"), report_only=("perf-e2e",),
              needs=("package-intel",), paths=rf"{RUST_PATHS}|{E2E_PATHS}|^tests/desktop/", runtime="native"),
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

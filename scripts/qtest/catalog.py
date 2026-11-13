"""What qtest can run: suites, groups, requirements, pinned tools, and path rules.

This file is the source of truth for the test catalog. docs/testing.md
describes the same suites; scripts/tests/test_qtest.py keeps them in sync.
"""
from dataclasses import dataclass
import platform
import re

MACOS = platform.system() == "Darwin"
LINUX_HOST = platform.system() == "Linux"
# The operating system of this computer, as the `platforms` of a suite name it.
HOST = "macos" if MACOS else "linux" if LINUX_HOST else "windows" if platform.system() == "Windows" else None
PLATFORM_NAMES = {"macos": "macOS", "linux": "Linux", "windows": "Windows"}


def host_platform():
    """The platform of the prebuilt tool archives for this computer, like `macos-aarch64`, or None."""
    system = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system())
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    return f"{system}-{machine}" if system else None


@dataclass(frozen=True)
class Archive:
    """A prebuilt release archive of a tool, pinned by its SHA-256 digest."""
    platform: str
    url: str
    sha256: str
    # The path of the executable in the archive. Its file name is the installed name.
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
# the downloaded files. Native validation uses Linux, macOS, and Windows on both architectures.
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
             }) + github_archives("nextest-rs/nextest", "cargo-nextest-0.9.146", "cargo-nextest.exe", {
                 "windows-x86_64": ("cargo-nextest-0.9.146-x86_64-pc-windows-msvc.tar.gz",
                                    "809b01f2c94d031ab6cf5f4d4a4f477c8f51b69c96ab3cb2fe4db7ecb222d0d3"),
                 "windows-aarch64": ("cargo-nextest-0.9.146-aarch64-pc-windows-msvc.tar.gz",
                                     "c94029615d05072af35429d9d66e0f83ba59886bb4e13163a191134e5f0ad47c"),
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
        # Not a suite requirement: qtest uses it as a build cache when it is installed.
        Tool("sccache", "0.19.0", ("sccache", "--version"), "sccache 0.19.0",
             (("cargo", "install", "--locked", "sccache", "--version", "0.19.0"),),
             github_archives("mozilla/sccache", "v0.19.0", "{asset}/sccache", {
                 "macos-aarch64": ("sccache-v0.19.0-aarch64-apple-darwin.tar.gz",
                                   "a35a0b7d622618774bc1bb8102d72562dfb21e619f88b153b370d7f89bc286af"),
                 "macos-x86_64": ("sccache-v0.19.0-x86_64-apple-darwin.tar.gz",
                                  "2db7344bb96de350df79cd9f0124dd4e9baa47b3de2fac1e63e1346b66b20684"),
                 "linux-x86_64": ("sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz",
                                  "aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd"),
                 "linux-aarch64": ("sccache-v0.19.0-aarch64-unknown-linux-musl.tar.gz",
                                   "3e47fc4a71b1ddd1b4294a0de9d34498922fdf41e3cd0bda5df3954e3d6deb9d"),
             }) + github_archives("mozilla/sccache", "v0.19.0", "{asset}/sccache.exe", {
                 "windows-x86_64": ("sccache-v0.19.0-x86_64-pc-windows-msvc.tar.gz",
                                    "130e4f8eb672e76590e9993c2ab890c4763b88238577bb11d8062158c7698353"),
                 "windows-aarch64": ("sccache-v0.19.0-aarch64-pc-windows-msvc.tar.gz",
                                     "a680e8e6058b033cd6ff56d56b2219ca13f1c1a45f50fb3b04f4ed80851f5c2b"),
             })),
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
    # The operating systems that the suite runs on. A group skips the suite
    # on other systems. Selected by name, it reports a missing prerequisite.
    platforms: tuple[str, ...] = ("macos", "linux")
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
    # Nextest commands used by fixture adapters; prepared together before servers.
    builds: tuple[tuple[str, ...], ...] = ()
    test_runs: int = 0

    @property
    def filterable(self):
        return any(step.nextest_filter is not None for step in self.steps)


def nextest(expression, *arguments):
    return Step(("cargo", "nextest", "run", "--locked", *arguments), nextest_filter=expression)


PYTHON = "{python}"  # Replaced with the interpreter that runs qtest.

CLIPPY_CORE = Step(("cargo", "clippy", "--locked", "--no-default-features", "--all-targets", "--", "-D", "warnings"))
# On Linux, the GPUI test support of the UI tests is behind the ui-test feature.
UI_TEST_FEATURE = ("--features", "ui-test") if LINUX_HOST else ()
CLIPPY_APP = Step(("cargo", "clippy", "--locked", *UI_TEST_FEATURE, "--all-targets", "--", "-D", "warnings"))
# The e2e and perf-e2e suites use the same test binary. A run builds it once.
E2E_BUILD = Step(("cargo", "test", "--locked", *UI_TEST_FEATURE, "--no-run", "--test", "e2e"))
TRANSFER_PERF_BUILD = Step(("cargo", "test", "--locked", "--profile", "perf", "--no-default-features", "--test", "integration", "--no-run"))

MACOS_ONLY = ("macos",)
# The application build needs the Linux desktop libraries. On macOS, this
# requirement always holds.
APP = ("cargo", "linux-app-libs")

RENDERER_FLAGS = ("--package", "qrow", "--package", "gpui-pre-macos",
                  "--features", "gpui-kit/test-support", "--lib")
RENDERER_UNITS = ("binary(gpui_macos) & ((test(/^opengl_renderer::/) & "
                  "not test(/^opengl_renderer::tests::/) & not test(hardware_capability_probe)) | "
                  "test(/^renderer::tests::/) | test(/^software_renderer::tests::/))")


def suites_for_platform(system):
    """Resolve suite commands from the requested OS, independently of this host."""
    linux = system == "linux"
    ui_feature = ("--features", "ui-test") if linux else ()
    clippy_app = Step(("cargo", "clippy", "--locked", *ui_feature, "--all-targets", "--", "-D", "warnings"))
    e2e_build = Step(("cargo", "test", "--locked", *ui_feature, "--no-run", "--test", "e2e"))
    return {
        suite.name: suite
        for suite in [
            Suite("opengl-unit", "GPU-independent OpenGL limits, atlas, scene, CPU composition and startup policy.",
                  ("cargo", "cargo-nextest"), (nextest(RENDERER_UNITS, *RENDERER_FLAGS),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("opengl-renderer", "Strict hardware Core OpenGL shader and pixel checks.",
                  ("cargo", "cargo-nextest"),
                  (nextest("binary(gpui_macos) & test(/^opengl_renderer::/)", *RENDERER_FLAGS),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("software-ui", "CPU rendering and native input in an isolated packaged demo.",
                  ("cargo", "uv", "xcode", "desktop"),
                  (Step(("sh", "scripts/e2e/software.sh"),
                        env=(("QROW_E2E_ARTIFACTS", "{artifacts}/software-ui"),)),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("opengl-ui", "Strict hardware Core OpenGL rendering and native input.",
                  ("cargo", "uv", "xcode", "desktop"),
                  (Step(("sh", "scripts/e2e/software.sh"),
                        env=(("QROW_E2E_ARTIFACTS", "{artifacts}/opengl-ui"),
                             ("QROW_E2E_RENDERER", "opengl"))),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("perf-renderers", "Native release CPU/OpenGL composition and scrolling measurements.",
                  ("cargo", "uv", "xcode", "desktop"),
                  (Step(("sh", "scripts/e2e/software.sh", "--prepare"),
                        env=(("QROW_E2E_ARTIFACTS", "{artifacts}/perf-renderers"),)),
                   Step((PYTHON, "scripts/perf/renderers.py"),
                        env=(("QROW_E2E_ARTIFACTS", "{artifacts}/perf-renderers"),), timeout=15 * 60)),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("windows-unit", "Portable core tests on Windows; server E2E needs Linux Docker.",
                  ("cargo", "cargo-nextest"),
                  (nextest("kind(lib)", "--no-default-features", "--lib"),),
                  platforms=("windows",), explicit_only=True),
            Suite("windows-storage", "Synthetic Windows Credential Manager round trip.",
                  ("cargo", "cargo-nextest"),
                  (nextest("test(credential_round_trip)", "--no-default-features", "--lib", "--run-ignored", "only"),),
                  platforms=("windows",), explicit_only=True),
            Suite("windows-package", "Build a native unsigned Windows installer.",
                  ("cargo", "uv"),
                  (Step((PYTHON, "scripts/package/windows.py"), timeout=60 * 60),),
                  platforms=("windows",), explicit_only=True),
            Suite("fmt", "Rust formatting.", ("cargo",),
                  (Step(("cargo", "fmt", "--all", "--", "--check")),), platforms=("macos", "linux", "windows")),
            # The static job lints the core library. The ui CI jobs lint the
            # application on each platform.
            Suite("clippy", "Clippy for the core library.", ("cargo",), (CLIPPY_CORE,)),
            Suite("clippy-app", "Clippy for the application with all features.", APP, (clippy_app,)),
            Suite("rustdoc", "Rust API documentation without warnings.", ("cargo",),
                  (Step(("cargo", "doc", "--locked", "--no-default-features", "--no-deps"),
                        env=(("RUSTDOCFLAGS", "-D warnings"),)),)),
            Suite("app-unit", "Application unit tests and doctests; portable core tests run in coverage.",
                  (*APP, "cargo-nextest"),
                  (nextest("kind(lib) & test(/^(ui|assets|themes)::/)", *ui_feature, "--lib"),
                   Step(("cargo", "test", "--locked", *ui_feature, "--doc")))),
            Suite("clippy-native", "Clippy for the production application on this target.", APP,
                  (Step(("cargo", "clippy", "--locked", *(("--bin", "qrow", "--lib") if system in ("linux", "windows") else ("--all-targets",)), "--", "-D", "warnings")),), platforms=("macos", "linux", "windows")),
            Suite("unit", "Rust unit and integration tests that need no UI and no servers.",
                  (*APP, "cargo-nextest"),
                  (nextest("not binary(ui)", *ui_feature), Step(("cargo", "test", "--locked", *ui_feature, "--doc")))),
            Suite("ui", "Headless tests of the real Qrow window, without servers.",
                  (*APP, "cargo-nextest"),
                  (nextest("binary(ui)", *ui_feature, "--test", "ui"),)),
            Suite("coverage", "Core line coverage with an 80% floor.", ("cargo", "cargo-llvm-cov"),
                  (Step(("mkdir", "-p", "{target}/coverage")),
                   Step(("cargo", "llvm-cov", "--locked", "--no-default-features", "--lib", "--tests",
                         "--ignore-filename-regex", "src/connector/t_c_l_i_service.rs|tests/|src/bin/",
                         "--fail-under-lines", "80", "--lcov", "--output-path", "{target}/coverage/core.lcov"),
                        env=(("RUST_TEST_THREADS", "1"),)),
                  )),
            Suite("perf", "SQL, dbt, and export benchmarks with enforced budgets.", ("cargo",),
                  (Step(("cargo", "bench", "--locked", "--no-default-features", "--bench", "sql", "--bench", "dbt", "--bench", "export")),)),
            Suite("scripts", "ShellCheck, actionlint, Ruff, and automation unit tests.",
                  ("uv", "shellcheck", "actionlint"),
                  (Step(("sh", "-c", "find scripts -type f -name '*.sh' -exec shellcheck {} + "
                               "&& shellcheck .githooks/* qtest tests/desktop/*.sh website/*.sh")),
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
            Suite("native-storage", "OS-specific credential tests with synthetic data.",
                  ("cargo", "cargo-nextest", *(("dbus-run-session", "dbus-send", "gnome-keyring-daemon") if linux else ())),
                  (Step(("sh", "scripts/testing/linux-secret-service.sh")),) if linux else
                  (Step((PYTHON, "scripts/testing/macos-keychain.py")),),
                  explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--lib"),) +
                         ((("cargo", "nextest", "run", "--no-default-features", "--test", "integration"),) if not linux else ()),
                  test_runs=1 if linux else 2),
            Suite("native-font", "macOS font and tooltip layout on the native main thread.", APP,
                  (Step(("cargo", "run", "--locked", "--example", "tooltip-layout")),), platforms=MACOS_ONLY, explicit_only=True),
            Suite("native-ui", "macOS branches of headless application assertions.", (*APP, "cargo-nextest"),
                  (nextest("binary(ui) & (test(=metadata::shortcuts_and_connection_fields_use_tooltip_parts) | test(=queries::activity_tooltips_keep_the_shortcut_above_unread_status) | test(=assistant_sql::appended_sql_keeps_earlier_queries_and_titles_the_conversation))", "--test", "ui"),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("native-assistant", "macOS branch of the assistant server interaction.", (*APP, "cargo-nextest", "fixture-runtime"),
                  (nextest("binary(e2e) & test(=assistant::assistant_turns_leave_a_connected_query_tab_idle_until_sql_runs)", "--test", "e2e", "--run-ignored", "only"),),
                  platforms=MACOS_ONLY, explicit_only=True, fixture="any"),
            Suite("postgres-core", "Postgres connector tests without GPUI.", ("cargo", "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/postgres.py", "--layer", "core")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--test", "integration"),), test_runs=1),
            Suite("trino-core", "Trino connector tests without GPUI.", ("cargo", "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/trino.py", "--layer", "core")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--test", "integration"),), test_runs=2),
            Suite("postgres-ui", "Postgres headless window scenarios.", (*APP, "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/postgres.py", "--layer", "ui")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", *ui_feature, "--test", "e2e"),), test_runs=1),
            Suite("trino-ui", "Trino headless window scenarios.", (*APP, "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/trino.py", "--layer", "ui")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", *ui_feature, "--test", "e2e"),), test_runs=2),
            Suite("postgres", "Postgres connector and real-window tests with a disposable server.",
                  (*APP, "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/postgres.py")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--test", "integration"),
                          ("cargo", "nextest", "run", *ui_feature, "--test", "e2e")), test_runs=2),
            Suite("trino", "Trino connector and real-window tests with a disposable server.",
                  (*APP, "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/trino.py")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--test", "integration"),
                          ("cargo", "nextest", "run", *ui_feature, "--test", "e2e")), test_runs=4),
            Suite("backend", "Connector and worker against the real servers, without the UI.",
                  ("cargo", "cargo-nextest", "fixture-runtime"),
                  (nextest("binary(integration) & test(/^backend::/)", "--no-default-features", "--test", "integration", "--run-ignored", "only"),),
                  explicit_only=True, fixture="docker",
                  prepare=(Step(("cargo", "test", "--locked", "--no-default-features", "--no-run", "--test", "integration")),)),
            Suite("e2e", "The real Qrow window, headless, against the real servers.",
                  (*APP, "cargo-nextest", "fixture-runtime"),
                  # `--test e2e` builds only the binary of the prepare step.
                  (nextest("binary(e2e) & not test(/^perf::/) & not test(/^postgres::/) & not test(/^trino::/)",
                           *ui_feature, "--test", "e2e", "--run-ignored", "only"),),
                  explicit_only=True, fixture="docker" if linux else "any",
                  prepare=(e2e_build,)),
            Suite("perf-ui", "Frame, scroll, editor, assistant, and Activity timings of the real window in a release-like build.",
                  ("cargo", "cargo-nextest"),
                  (nextest("binary(perf)", "--cargo-profile", "perf", "--test", "perf", "--run-ignored", "only", "--no-capture"),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("perf-e2e", "Window query latency against the real servers.",
                  ("cargo", "cargo-nextest", "fixture-runtime"),
                  (nextest("binary(e2e) & test(/^perf::/)", "--test", "e2e", "--run-ignored", "only", "--no-capture"),),
                  platforms=MACOS_ONLY, explicit_only=True, fixture="any",
                  prepare=(e2e_build,)),
            Suite("perf-kyuubi", "Kyuubi export transfer measurements against the real servers.",
                  ("cargo", "cargo-nextest", "fixture-runtime"),
                  (nextest("binary(integration) & test(/^transfer_perf::kyuubi$/)", "--cargo-profile", "perf",
                           "--no-default-features", "--test", "integration", "--run-ignored", "only", "--no-capture"),),
                  explicit_only=True, fixture="any", prepare=(TRANSFER_PERF_BUILD,)),
            Suite("perf-postgres", "Postgres export transfer and COPY measurements against a disposable server.",
                  ("cargo", "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/postgres.py", "--perf")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--cargo-profile", "perf", "--test", "integration"),), test_runs=1),
            Suite("perf-trino", "Trino export transfer measurements against a disposable coordinator.",
                  ("cargo", "cargo-nextest", "docker"),
                  (Step((PYTHON, "scripts/e2e/trino.py", "--perf")),), explicit_only=True,
                  builds=(("cargo", "nextest", "run", "--no-default-features", "--cargo-profile", "perf", "--test", "integration"),), test_runs=1),
            Suite("perf-app", "Launch time, idle memory, and idle CPU of the release app on the desktop.",
                  ("cargo", "uv"),
                  (Step(("cargo", "build", "--locked", "--release", "--bin", "qrow"), timeout=40 * 60),
                   Step((PYTHON, "scripts/perf/app.py"))),
                  platforms=MACOS_ONLY, explicit_only=True),
            # perf-app on a package from another job, without a release build.
            Suite("perf-package", "Launch time, idle memory, and idle CPU of the packaged app on the desktop.",
                  (),
                  (Step((PYTHON, "scripts/perf/app.py", "--package")),),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("package", "The release app package, built in the target directory, and its size budget.",
                  ("cargo", "uv", "xcode"),
                  (Step(("sh", "scripts/package/macos.sh"), env=(("QROW_DIST_DIR", "{target}/package"),), timeout=40 * 60),
                   Step((PYTHON, "scripts/core/size.py"), env=(("QROW_DIST_DIR", "{target}/package"),))),
                  platforms=MACOS_ONLY, explicit_only=True),
            Suite("linux-package", "Build the native Linux archive in target/package.",
                  (*APP, "uv"),
                  (Step(("sh", "scripts/package/linux.sh"), env=(("QROW_DIST_DIR", "{target}/package"),), timeout=60 * 60),),
                  platforms=("linux",), explicit_only=True),
            Suite("package-smoke", "Inspect and launch the extracted native package.",
                  ("uv", *(("xvfb-run", "xdotool") if linux else ())),
                  (Step((PYTHON, "scripts/package/qualify.py", "{target}/package", "--receipt", "{target}/platform/receipt.json")),),
                  platforms=("macos", "linux", "windows"), explicit_only=True),
            Suite("desktop", "Smoke checks of the packaged app on the desktop: the menu bar, Keychain, quit, and pixels.",
                  ("uv", "cargo", "xcode", "desktop", "fixture-runtime"),
                  (Step(("sh", "scripts/e2e/driver.sh", "--prepared"), timeout=90 * 60),),
                  platforms=MACOS_ONLY, explicit_only=True, fixture="any",
                  prepare=(Step(("sh", "scripts/e2e/driver.sh", "--prepare"), timeout=30 * 60),),
                  # The driver removes its synthetic Keychain items on exit. This
                  # step also removes them when a time limit stopped the driver.
                  cleanup=(Step((PYTHON, "scripts/e2e/keychain.py")),)),
        ]
    }


# Compatibility view for existing local commands and hooks. New plans resolve
# against an explicit target, using suites_for_platform.
SUITES = suites_for_platform(HOST)

# `./qtest compare` runs these suites when you name none. They need no servers.
COMPARE_SUITES = ("perf", "perf-ui", "perf-app")

GROUPS = {
    "default": ("Fast local checks. Runs when you give no selector.",
                ("fmt", "clippy", "clippy-app", "unit", "ui")),
    "all": ("Every local suite without servers. CI runs these suites and the suites with servers.",
            ("scripts", "deps", "fmt", "clippy", "clippy-app", "rustdoc", "unit", "ui", "perf", "coverage")),
}

# Changed paths select suites for `qtest run --changed` and the hooks.
RUST_PATHS = (r"^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|build\.rs|src/|tests/(?!fixture/)|benches/|vendor/"
              r"|\.cargo/config\.toml$|themes/|assets/(app-icons|connection-type-icons)/|\.config/nextest\.toml$)")
RUST_SUITES = ("fmt", "clippy", "clippy-app", "rustdoc", "unit", "ui")
CHANGE_RULES = (
    (RUST_PATHS, RUST_SUITES),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml|scripts/core/policy\.py|\.github/(workflows|actions)/)",
     ("policy",)),
    (r"^(Cargo\.toml|Cargo\.lock|deny\.toml|dependency-reviews\.toml)", ("deps",)),
    # The scripts suite also lints the Python and shell helpers of the desktop driver and the website scripts.
    (r"^(\.githooks/|\.github/(workflows|actions)/|pyproject\.toml$|uv\.lock$|scripts/|qtest(?:\.cmd)?$|docs/testing\.md$"
     r"|tests/desktop/[^/]+\.(py|sh)$|website/[^/]+\.sh$)",
     ("scripts",)),
    (r"^scripts/core/preflight\.sh$", (*RUST_SUITES, "policy")),
)

# `qtest hook NAME` runs the suites that the changed paths select, but only
# those of the hook. A commit gets the checks that take seconds. A push also
# gets the tests. CI runs everything, including `perf` and `coverage`.
HOOKS = {
    "pre-commit": ("Static checks of the staged files.", ("scripts", "policy", "fmt", "clippy", "clippy-app")),
    "pre-push": ("Static checks and tests of the pushed changes.",
                 ("scripts", "policy", "deps", "fmt", "clippy", "clippy-app", "rustdoc", "unit", "ui")),
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


REQUIREMENTS = {
    "dbus-run-session": "Install D-Bus to run the isolated secret-store test.",
    "dbus-send": "Install the D-Bus tools to run the isolated secret-store test.",
    "gnome-keyring-daemon": "Install GNOME Keyring to run the isolated secret-store test.",
    "xvfb-run": "Install Xvfb for isolated Linux package smoke checks.",
    "xdotool": "Install xdotool to observe the packaged Linux window.",
    "cargo": "Install Rust with rustup. rust-toolchain.toml selects the toolchain.",
    "uv": "Install uv: https://docs.astral.sh/uv/getting-started/installation/",
    "shellcheck": "Install ShellCheck, for example `brew install shellcheck`.",
    "actionlint": "Install actionlint, for example `brew install actionlint`.",
    "linux-app-libs": "Install the Linux desktop libraries of the application build. "
                      "On Ubuntu, run `sh scripts/ci/linux-build-deps.sh`.",
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

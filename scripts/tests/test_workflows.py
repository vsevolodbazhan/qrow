"""CI ownership, dependency order, source identity, and release artifact promotion."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/qtest"))
sys.path.insert(0, str(ROOT / "scripts/ci"))
import catalog  # noqa: E402
import platforms  # noqa: E402

VALIDATION = ".github/workflows/test.yml"
TEST = ".github/workflows/test.yml"
RELEASE = ".github/workflows/release.yml"


def read(path):
    return (ROOT / path).read_text()


def job(name, path=VALIDATION):
    match = re.search(rf"^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-z0-9-]+:\n|\Z)",
                      read(path), re.MULTILINE | re.DOTALL)
    if match is None:
        raise AssertionError(f"{path} has no job {name}")
    return match.group("body")


def needs(body):
    match = re.search(r"^    needs: (?:\[(.*)\]|(\S+))$", body, re.MULTILINE)
    return [name.strip() for name in (match.group(1) or match.group(2)).split(",")] if match else []


class ValidationGraphTests(unittest.TestCase):
    def test_common_gate_precedes_each_native_test_but_builds_overlap(self):
        targets = [target for target in platforms.TARGETS.values() if target.supported]
        for target in targets:
            key = target.name.replace("_", "-")
            build, test = job("build-" + key), job("test-" + key)
            self.assertEqual(needs(build), ["plan"])
            self.assertEqual(needs(test), ["common-required", "build-" + key])
            self.assertIn("runs-on: " + target.build_runner, build)
            self.assertIn("runs-on: " + target.runner, test)
            self.assertIn(f"native.py build --target {target.name} ", build)
            self.assertEqual(f"rust-target: {target.triple}" in build, target.build_runner != target.runner)
            for body in (build, test):
                self.assertIn("ref: ${{ inputs.ref || github.sha }}", body)
                self.assertNotIn("if: inputs.full", body)
            self.assertIn("name: prepared-" + target.name, build)
            self.assertIn("name: prepared-" + target.name, test)
            self.assertIn("name: candidate-" + target.name, test)
            self.assertNotIn("rust-cache@", test)
        self.assertEqual(needs(job("common-required")), ["plan", "common"])
        self.assertEqual(set(needs(job("ci-required"))), {"common-required", *("test-" + t.name.replace("_", "-") for t in targets)})
        self.assertIn("always()", job("common-required"))
        self.assertIn("always()", job("ci-required"))
        self.assertIn("verify-common evidence", job("common-required"))
        self.assertIn("platforms.py verify evidence", job("ci-required"))

    def test_intel_macos_compiles_on_apple_silicon_and_tests_on_intel(self):
        intel = platforms.TARGETS["macos-x86_64"]
        self.assertEqual(intel.build_runner, platforms.TARGETS["macos-arm64"].runner)
        self.assertEqual(intel.runner, "macos-15-intel")
        self.assertEqual([t.name for t in platforms.TARGETS.values() if t.supported and t.build_runner != t.runner],
                         ["macos-x86_64"])

    def test_builds_use_all_runner_cores(self):
        self.assertNotIn("CARGO_BUILD_JOBS", read(VALIDATION))

    def test_no_legacy_scheduler_or_workflow_remains(self):
        self.assertFalse((ROOT / ".github/workflows/checks.yml").exists())
        self.assertFalse(hasattr(catalog, "CI_JOBS"))
        self.assertFalse((ROOT / ".github/workflows/platforms.yml").exists())
        self.assertIn("workflow_call:", read(TEST))
        self.assertNotIn("uses: ./.github/workflows/", read(TEST))
        self.assertIn("uses: ./.github/workflows/test.yml", job("validation", RELEASE))
        self.assertNotIn("workflows/checks.yml", read(RELEASE))
        self.assertIn("platforms.py verify evidence", job("ci-required"))

    def test_backend_has_no_ui_dependency_or_feature(self):
        self.assertEqual(platforms.COMMON["backend"], ["backend", "postgres-core", "trino-core"])
        for name in platforms.COMMON["backend"]:
            suite = catalog.suites_for_platform("linux")[name]
            self.assertNotIn("linux-app-libs", suite.requires)
            commands = [*suite.builds, *(s.command for s in suite.steps if s.nextest_filter)]
            self.assertTrue(commands)
            self.assertTrue(all("--no-default-features" in command for command in commands))
            self.assertTrue(all("ui-test" not in command for command in commands))

    def test_portable_scenarios_have_one_owner_and_native_has_only_os_assertions(self):
        self.assertEqual(set(platforms.COMMON), {"static", "core", "backend", "headless-ui", "performance"})
        owned = [name for suites in platforms.COMMON.values() for name in suites]
        self.assertEqual(len(owned), len(set(owned)))
        self.assertEqual(set(platforms.COMMON["headless-ui"]), {"clippy-app", "app-unit", "ui", "e2e", "postgres-ui", "trino-ui"})
        for target in platforms.TARGETS.values():
            if not target.supported:
                continue
            suites = platforms.suites(target)
            self.assertEqual(suites, platforms.suites(target, True))
            self.assertTrue({"native-storage", "package-smoke"} <= set(suites))
            self.assertFalse(set(suites) & {"unit", "ui", "backend", "postgres", "trino", "e2e"})
            if target.system == "macos":
                self.assertTrue({"desktop", "native-font", "native-ui", "native-assistant"} <= set(suites))
        self.assertIn("--fail-under-lines", catalog.SUITES["coverage"].steps[1].command)
        self.assertIn("80", catalog.SUITES["coverage"].steps[1].command)
        self.assertIn("perf", platforms.COMMON["performance"])
        self.assertIn("--all-targets", catalog.suites_for_platform("macos")["clippy-native"].steps[0].command)

    def test_all_local_suites_have_ci_coverage_or_explicit_equivalents(self):
        owned = {name for suites in platforms.COMMON.values() for name in suites}
        owned.update(name for target in platforms.TARGETS.values() if target.supported for name in platforms.suites(target))
        # Production lint/package builds run in the build phase. Measurements
        # run once per baseline host in full mode. Local convenience suites
        # partition into CI suites instead of running again.
        owned.update({"clippy-native", "package", "linux-package", "perf-ui", "perf-e2e", "perf-package",
                      "perf-kyuubi", "perf-postgres", "perf-trino"})
        equivalents = {"unit": {"coverage", "app-unit"}, "postgres": {"postgres-core", "postgres-ui"},
                       "trino": {"trino-core", "trino-ui"}, "perf-app": {"perf-package"}}
        self.assertTrue(all(replacement <= owned for replacement in equivalents.values()))
        self.assertEqual(owned | equivalents.keys(), set(catalog.SUITES))

    def test_conditional_headless_assertions_keep_their_macos_branch(self):
        definitions = catalog.suites_for_platform("macos")
        for relative in ("tests/ui/queries.rs", "tests/ui/metadata.rs", "tests/ui/assistant_sql.rs", "tests/e2e/assistant.rs"):
            source = read(relative)
            expression = definitions["native-assistant" if "/e2e/" in relative else "native-ui"].steps[0].nextest_filter
            for position in re.finditer(r'cfg!\(target_os = "macos"\)', source):
                function = list(re.finditer(r"^fn (\w+)\(", source[:position.start()], re.MULTILINE))[-1][1]
                self.assertIn(function, expression)

    def test_evidence_does_not_upload_extracted_test_binaries(self):
        validation = read(VALIDATION)
        self.assertEqual(validation.count("            target/qtest/runs/\n"),
                         validation.count("            !target/qtest/runs/**/prepared/*/target/**\n"))
        self.assertEqual(validation.count("name: candidate-"), 8)
        for target in platforms.TARGETS.values():
            if target.supported:
                body = job("build-" + target.name.replace("_", "-"))
                self.assertIn("compression-level: 0", body)
                self.assertNotIn("path: target/package/\n", body)

    def test_runs_share_source_and_forks_need_no_secrets(self):
        validation = read(VALIDATION)
        self.assertNotIn("secrets.", validation)
        self.assertIn("${{ github.workflow }}-validation-", validation)
        self.assertNotIn("head.repo.full_name", validation)
        self.assertIn("converted_to_draft", read(TEST))
        self.assertIn("!github.event.pull_request.draft", job("plan", TEST))
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", read(TEST))
        self.assertIn("--repeat 100", job("common"))
        self.assertIn("--reuse-builds", job("common"))
        self.assertIn("cold-cache", read(TEST))
        self.assertIn("full: true", job("validation", RELEASE))

    def test_caches_have_single_saving_owners_and_tests_only_download_artifacts(self):
        validation = read(VALIDATION)
        keys = re.findall(r"shared-key: (.+)", validation)
        self.assertEqual(len(keys), len(set(keys)))
        for line in validation.splitlines():
            if "save-if:" in line:
                self.assertIn("github.ref == 'refs/heads/main' && github.event_name == 'push'", line)
        maintenance = read(".github/workflows/cache-cleanup.yml")
        self.assertIn("workflow_run:", maintenance)
        self.assertIn("workflows: [test]", maintenance)
        self.assertIn("branches: [main]", maintenance)
        self.assertIn("workflow_run.event == 'push'", maintenance)
        self.assertIn("head_repository.full_name == github.repository", maintenance)
        self.assertIn("actions: write", maintenance)
        self.assertIn("scripts/ci/rust_caches.py", maintenance)
        self.assertNotIn("ref:", job("cache-cleanup", ".github/workflows/cache-cleanup.yml"))
        self.assertNotIn("cache-cleanup:", validation)
        self.assertNotIn("cache-cleanup:", read(RELEASE))
        self.assertNotIn("actions: write", validation)



class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_promotes_qualified_candidates_without_cargo_builds(self):
        for name in ("dmg", "linux"):
            body = job(name, RELEASE)
            prerequisites = needs(body) or re.findall(r"^      - (\S+)$", body.split("strategy:")[0], re.MULTILINE)
            self.assertEqual(prerequisites, ["resolve-target", "validation"])
            self.assertNotIn("cargo build", body)
            self.assertIn("receipt", body)
            self.assertIn("sha256", body)
            self.assertIn("platform-evidence-", body)
        dmg = job("dmg", RELEASE)
        for architecture, runner in (("arm64", "macos-15"), ("x86_64", "macos-15-intel")):
            self.assertIn(f"arch: {architecture}\n            runner: {runner}", dmg)
        self.assertIn("scripts/package/dmg.py build", dmg)
        self.assertIn("hdiutil imageinfo", dmg)
        self.assertIn("scripts/package/qualify.py dist --dmg", dmg)
        self.assertIn("lipo -archs", dmg)
        self.assertIn('cp "$dmg_path" "dist/Qrow-${RELEASE_VERSION}.dmg"', dmg)
        self.assertIn("merge-multiple: true", job("publish", RELEASE))
        self.assertIn("Select the workflow branch at the candidate commit", job("resolve-target", RELEASE))


if __name__ == "__main__":
    unittest.main()

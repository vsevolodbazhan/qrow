"""Check that the workflows run the CI jobs of the qtest catalog."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/qtest"))
import catalog  # noqa: E402

CHECKS = ".github/workflows/checks.yml"
TEST = ".github/workflows/test.yml"
RELEASE = ".github/workflows/release.yml"


def read(path):
    return (ROOT / path).read_text()


def job(name, path=CHECKS):
    """The body of one job, up to the next job or the end of the file."""
    match = re.search(rf"^  {re.escape(name)}:\n(?P<body>.*?)(?=^(?:  # .*\n)*  [a-z0-9-]+:\n|\Z)",
                      read(path), re.MULTILINE | re.DOTALL)
    if match is None:
        raise AssertionError(f"{path} has no job {name}")
    return match.group("body")


def needs(body):
    match = re.search(r"^    needs: (?:\[(.*)\]|(\S+))$", body, re.MULTILINE)
    if not match:
        return []
    return [name.strip() for name in (match.group(1) or match.group(2)).split(",")]


class CatalogJobTests(unittest.TestCase):
    def test_every_catalog_job_is_a_workflow_job(self):
        workflow_jobs = re.findall(r"^  ([a-z0-9-]+):\n", read(CHECKS), re.MULTILINE)
        self.assertEqual(workflow_jobs, ["plan", *catalog.CI_JOBS])

    def test_workflow_jobs_follow_the_catalog(self):
        for name, ci_job in catalog.CI_JOBS.items():
            with self.subTest(job=name):
                body = job(name)
                self.assertIn(f"runs-on: {ci_job.runner}\n", body)
                self.assertIn(f"run: ./qtest ci {name} --install\n", body)
                self.assertEqual(needs(body), ["plan", *ci_job.needs])
                self.assertIn("ref: ${{ inputs.ref }}", body)
                if name == "static":
                    self.assertNotIn("if:", body.split("steps:")[0])
                else:
                    self.assertTrue(f"fromJSON(needs.plan.outputs.jobs).{name}" in body or
                                    f"fromJSON(needs.plan.outputs.jobs)['{name}']" in body)

    def test_suites_of_jobs_exist(self):
        for ci_job in catalog.CI_JOBS.values():
            for name in [*ci_job.suites, *ci_job.report_only]:
                self.assertIn(name, catalog.SUITES)
            for need in ci_job.needs:
                self.assertIn(need, catalog.CI_JOBS)

    def test_guide_lists_every_job_and_the_rules_list_every_required_check(self):
        guide = read("docs/testing.md")
        development = read("docs/development.md")
        for name in ["plan", *catalog.CI_JOBS]:
            with self.subTest(job=name):
                if name != "plan":
                    self.assertIn(f"\n| `{name}` | ", guide)
                # A pull request runs only pull request jobs, so only they can be required checks.
                required = name == "plan" or catalog.CI_JOBS[name].scope == "pull_request"
                self.assertEqual(f"\nchecks / {name}\n" in development, required)

    def test_ci_runs_every_suite(self):
        covered = {name for ci_job in catalog.CI_JOBS.values() for name in [*ci_job.suites, *ci_job.report_only]}
        self.assertEqual(covered, set(catalog.SUITES))


class PlanTests(unittest.TestCase):
    def test_paths_select_jobs_and_what_they_wait_for(self):
        everything = ["static", "core", "ui", "package", "backend", "e2e", "perf"]
        cases = {
            "docs/queries.md": ["static"],
            "scripts/tests/test_hooks.py": ["static"],
            "src/ui.rs": everything,
            ".cargo/config.toml": everything,
            "scripts/e2e/driver.sh": ["static", "backend", "e2e"],
            # The UI tests embed the synthetic Codex server.
            "tests/desktop/fake-codex.py": everything,
            "tests/fixture/server/Blocking.java": ["static", "backend", "e2e"],
            # The e2e job packages its debug app with the package scripts.
            "scripts/package/macos.sh": ["static", "package", "e2e"],
            "scripts/package/dmg.py": ["static", "package", "e2e"],
            ".github/workflows/checks.yml": everything,
            "scripts/qtest/catalog.py": everything,
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(catalog.ci_jobs_for_changes([path]), expected)

    def test_each_event_runs_its_scope_and_the_scopes_before_it(self):
        pull_requests = ["static", "core", "ui", "package", "backend", "e2e", "perf"]
        self.assertEqual(catalog.ci_jobs_for_event("pull_request"), pull_requests)
        self.assertEqual(catalog.ci_jobs_for_event("push"), [*pull_requests, "perf-ui", "perf-backend"])
        self.assertEqual(catalog.ci_jobs_for_event("manual"), list(catalog.CI_JOBS))
        manual = [name for name, ci_job in catalog.CI_JOBS.items() if ci_job.scope == "manual"]
        self.assertEqual(manual, ["desktop-package", "ui-intel", "package-intel", "e2e-intel"])
        # A job that waited for a job of a later scope would never run.
        for name, ci_job in catalog.CI_JOBS.items():
            for need in ci_job.needs:
                self.assertLessEqual(catalog.SCOPES.index(catalog.CI_JOBS[need].scope),
                                     catalog.SCOPES.index(ci_job.scope), name)

    def test_jobs_start_at_once_unless_they_use_a_package(self):
        # Waiting for a job that only predicts a failure, like core, made each
        # path of a pull request longer.
        waits = {name: ci_job.needs for name, ci_job in catalog.CI_JOBS.items() if ci_job.needs}
        self.assertEqual(waits, {"desktop-package": ("package",), "e2e-intel": ("package-intel",)})

    def test_pull_request_jobs_have_budgets_under_ten_minutes(self):
        for name in catalog.ci_jobs_for_event("pull_request"):
            with self.subTest(job=name):
                self.assertLessEqual(catalog.CI_JOBS[name].budget, 8)
        for ci_job in catalog.CI_JOBS.values():
            self.assertGreater(ci_job.budget, 0)

    def test_pull_requests_do_not_measure(self):
        self.assertIn("QROW_CI_MEASURE: ${{ github.event_name != 'pull_request' }}\n", read(CHECKS))
        # Jobs with only report-only suites check nothing, so pull requests skip them.
        for name, ci_job in catalog.CI_JOBS.items():
            if not ci_job.suites:
                self.assertNotEqual(ci_job.scope, "pull_request", name)

    def test_the_benchmarks_run_on_linux(self):
        self.assertEqual(catalog.CI_JOBS["perf"].runner, catalog.LINUX)
        self.assertNotIn("macos", catalog.SUITES["perf"].requires)

    def test_plan_job_selects_the_jobs_of_each_event(self):
        body = job("plan")
        self.assertIn("fetch-depth: 2", body)
        self.assertIn('git diff --name-only HEAD^1 HEAD', body)
        self.assertIn("push) ./qtest ci plan --push", body)
        self.assertIn("*) ./qtest ci plan --all", body)


class WorkflowTests(unittest.TestCase):
    def test_callers_share_the_checks_and_own_the_concurrency(self):
        checks = read(CHECKS)
        self.assertIn("workflow_call:", checks)
        # A concurrency group in the called workflow would let a release cancel a test run.
        self.assertNotIn("concurrency:", checks)
        self.assertNotIn("statuses:", checks)
        test = job("checks", TEST)
        self.assertIn("uses: ./.github/workflows/checks.yml", test)
        self.assertIn("ref: ${{ github.sha }}", test)
        release = job("checks", RELEASE)
        self.assertIn("uses: ./.github/workflows/checks.yml", release)
        self.assertIn("release-version: ${{ needs.resolve-target.outputs.release_version }}", release)

    def test_pull_requests_into_any_branch_get_checks(self):
        workflow = read(TEST)
        on = workflow[workflow.index("on:"):workflow.index("permissions:")]
        self.assertNotIn("branches: [main]\n    types", on)
        self.assertIn("converted_to_draft", on)
        self.assertIn("!github.event.pull_request.draft", job("checks", TEST))

    def test_e2e_jobs_skip_forks(self):
        for name in catalog.CI_JOBS:
            with self.subTest(job=name):
                forks_skip = "head.repo.full_name == github.repository" in job(name)
                self.assertEqual(forks_skip, name in ("backend", "e2e", "e2e-intel"))

    def test_the_tested_package_is_the_released_package(self):
        package = job("package")
        self.assertIn("QROW_RELEASE_VERSION: ${{ inputs.release-version }}", package)
        self.assertIn("name: macos-package\n", package)
        self.assertIn("path: target/package/Qrow-macos.zip", package)
        tested = job("desktop-package")
        self.assertIn("name: macos-package\n", tested)
        self.assertIn("QROW_E2E_REUSE_MACOS_PACKAGE: true", tested)
        self.assertEqual(catalog.CI_JOBS["desktop-package"].suites, ("desktop",))
        # The e2e job tests a debug app from its own test build and does not wait for the package.
        self.assertNotIn("macos-package", job("e2e"))
        self.assertEqual(dict(catalog.CI_JOBS["e2e"].env),
                         {"QROW_BUILD_PROFILE": "debug", "QROW_PACKAGE_PREBUILT": "true"})
        dmg = job("dmg", RELEASE)
        self.assertEqual(needs(dmg) or re.findall(r"^      - (\S+)$", dmg.split("strategy:")[0], re.MULTILINE),
                         ["resolve-target", "checks"])
        self.assertIn("name: ${{ matrix.package }}\n", dmg)
        self.assertNotIn("scripts/package/macos.sh", read(RELEASE))
        self.assertIn("- dmg", job("publish", RELEASE))

    def test_linux_release_runs_ui_and_e2e_before_publishing(self):
        linux = job("linux", RELEASE)
        self.assertIn("./qtest run ui", linux)
        self.assertIn("./qtest run e2e --runtime docker", linux)
        self.assertIn("scripts/package/linux.sh", linux)
        publish = job("publish", RELEASE)
        self.assertIn("- linux", publish)
        for arch, runner in [("x86_64", "ubuntu-24.04"), ("aarch64", "ubuntu-24.04-arm")]:
            self.assertIn(f"- arch: {arch}\n            runner: {runner}\n", linux)
            self.assertIn(f'"dist/Qrow-${{RELEASE_VERSION}}-linux-{arch}.tar.gz"', publish)
        self.assertIn("pattern: release-linux-*", publish)

    def test_the_aur_gets_the_released_linux_archives_after_publishing(self):
        aur = job("aur", RELEASE)
        self.assertIn("      - publish\n", aur)
        self.assertIn("pattern: release-linux-*", aur)
        self.assertIn("scripts/package/aur.py", aur)
        # Only this job reads the key, and it trusts only the published AUR host key.
        self.assertIn("    environment: aur\n", aur)
        self.assertEqual(read(RELEASE).count("secrets.AUR_SSH_PRIVATE_KEY"), 1)
        self.assertIn("StrictHostKeyChecking=yes", aur)
        self.assertIn("aur.archlinux.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEuBKrPzbawxA/k2g6NcyV5jmqwJ2s+zpgZGZ7tpLIcN", aur)

    def test_intel_package_is_tested_and_both_architectures_are_released(self):
        self.assertEqual(needs(job("e2e-intel")), ["plan", "package-intel"])
        for name in ("package-intel", "e2e-intel"):
            self.assertIn("name: macos-package-x86_64\n", job(name))
        # An ARM64 runner builds the Intel package. An Intel runner tests it.
        self.assertEqual(dict(catalog.CI_JOBS["package-intel"].env), {"CARGO_BUILD_TARGET": "x86_64-apple-darwin"})
        self.assertEqual(catalog.CI_JOBS["package-intel"].runner, catalog.MACOS_RUNNER)
        self.assertEqual(catalog.CI_JOBS["e2e-intel"].runner, "macos-15-intel")
        self.assertIn("perf-package", catalog.CI_JOBS["e2e-intel"].report_only)
        dmg = job("dmg", RELEASE)
        for arch, runner, artifact in (("arm64", "macos-15", "macos-package"),
                                       ("x86_64", "macos-15-intel", "macos-package-x86_64")):
            self.assertIn(f"- arch: {arch}\n            runner: {runner}\n            package: {artifact}\n", dmg)
            self.assertIn(f'"dist/Qrow-${{RELEASE_VERSION}}-{arch}.dmg"', job("publish", RELEASE))
        self.assertIn('lipo -archs dist/Qrow.app/Contents/MacOS/qrow', dmg)
        self.assertIn("name: release-dmg-${{ matrix.arch }}", dmg)
        self.assertIn("pattern: release-dmg-*", job("publish", RELEASE))
        self.assertIn("merge-multiple: true", job("publish", RELEASE))
        self.assertIn('if [ "$PACKAGE_ARCH" = arm64 ]; then', dmg)
        self.assertIn('cp "$dmg_path" "dist/Qrow-${RELEASE_VERSION}.dmg"', dmg)
        self.assertIn('"dist/Qrow-${RELEASE_VERSION}.dmg"', job("publish", RELEASE))

    def test_the_release_dmg_comes_from_the_packaging_script(self):
        # The installer window, the Applications shortcut, and the icon layout
        # are the job of scripts/package/dmg.py, not of a raw disk image.
        dmg = job("dmg", RELEASE)
        self.assertIn("uv run --locked python scripts/package/dmg.py build", dmg)
        self.assertIn('--volume-name "Qrow ${RELEASE_VERSION}"', dmg)
        self.assertIn("hdiutil imageinfo", dmg)
        self.assertNotIn("hdiutil create", dmg)
        # The packaging script lives in the repository at the released commit.
        self.assertIn("uses: actions/checkout@", dmg)
        self.assertIn("ref: ${{ needs.resolve-target.outputs.target_sha }}", dmg)
        self.assertIn("setup-uv", dmg)

    def test_only_main_saves_rust_caches(self):
        checks = read(CHECKS)
        caches = re.findall(r"rust-cache@.*\n(?:\s+.*\n)*?\s+save-if: (.*)\n", checks)
        self.assertEqual(len(caches), checks.count("rust-cache@"))
        self.assertTrue(all(value in ("${{ github.ref == 'refs/heads/main' }}", "false") for value in caches))

    def test_runs_that_save_rust_caches_delete_the_replaced_caches(self):
        for path in (TEST, RELEASE):
            with self.subTest(workflow=path):
                cleanup = job("cache-cleanup", path)
                self.assertEqual(needs(cleanup), ["checks"])
                # The same condition as save-if, also after a failed or cancelled check.
                self.assertIn("if: ${{ always() && github.ref == 'refs/heads/main' }}", cleanup)
                self.assertIn("actions: write", cleanup)
                self.assertIn("run: python3 scripts/ci/rust_caches.py\n", cleanup)
        # The checks run code from pull requests, so they must not delete caches.
        self.assertNotIn("actions: write", read(CHECKS))

    def test_runs_on_main_finish_and_pull_request_runs_cancel(self):
        # A cancelled run on main saves no build caches.
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n", read(TEST))

    def test_each_rust_cache_has_one_job_that_saves_it(self):
        caches = {}
        for name in catalog.CI_JOBS:
            match = re.search(r"shared-key: (\S+)\n\s+save-if: (.*)\n", job(name))
            if match:
                caches.setdefault(match.group(1), []).append((name, match.group(2) != "false"))
        for key, users in caches.items():
            with self.subTest(key=key):
                self.assertEqual(sum(saves for _, saves in users), 1, users)
        # Core builds only under cargo-llvm-cov, so its cache does not have the backend build.
        self.assertIn("backend", [name for name, _ in caches["backend"]])

    def test_the_no_default_features_lint_runs_in_one_job(self):
        self.assertIn("clippy", catalog.CI_JOBS["static"].suites)
        self.assertEqual(catalog.CI_JOBS["static"].runner, catalog.LINUX)
        linted = [name for name, ci_job in catalog.CI_JOBS.items() if "clippy" in ci_job.suites]
        self.assertEqual(linted, ["static"])
        self.assertIn("clippy-app", catalog.CI_JOBS["ui"].suites)

    def test_artifacts_expire_and_main_keeps_performance_history(self):
        checks = read(CHECKS)
        uploads = re.findall(r"upload-artifact@.*\n((?:\s{8,}.*\n)+)", checks)
        self.assertEqual(len(uploads), checks.count("upload-artifact@"))
        for upload in uploads:
            with self.subTest(upload=upload.split("\n")[1:2]):
                if "name: performance-" in upload:
                    self.assertIn("retention-days: 90", upload)
                    self.assertIn("perf.json", upload)
                else:
                    self.assertIn("retention-days: 1\n", upload)
        artifacts = {"package": "package", "perf-backend": "backend", "e2e": "e2e", "perf": "perf",
                     "perf-ui": "perf-ui"}
        for name, artifact in artifacts.items():
            self.assertIn(f"name: performance-{artifact}\n", job(name))
            self.assertIn("if: github.ref == 'refs/heads/main'", job(name))

    def test_the_desktop_suite_runs_the_only_permission_preflight(self):
        # `./qtest ci e2e` checks the permissions before it builds or starts servers.
        self.assertNotIn("driver.sh", job("e2e"))
        self.assertIn("desktop", catalog.SUITES["desktop"].requires)
        self.assertIn("e2e", [name for name, ci_job in catalog.CI_JOBS.items() if "desktop" in ci_job.suites])

    def test_native_e2e_downloads_come_from_the_mirror_without_an_actions_cache(self):
        # The mirror downloads the archives faster than an Actions cache restores them.
        body = job("e2e")
        self.assertIn("timeout-minutes: 150", body)
        self.assertNotIn("actions/cache", body)
        self.assertNotIn("e2e-downloads", body)


if __name__ == "__main__":
    unittest.main()

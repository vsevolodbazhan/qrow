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
                    self.assertIn(f"fromJSON(needs.plan.outputs.jobs).{name}", body)

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
                self.assertIn(f"\nchecks / {name}\n", development)

    def test_ci_runs_every_suite(self):
        covered = {name for ci_job in catalog.CI_JOBS.values() for name in [*ci_job.suites, *ci_job.report_only]}
        self.assertEqual(covered, set(catalog.SUITES))


class PlanTests(unittest.TestCase):
    def test_paths_select_jobs_and_what_they_wait_for(self):
        everything = list(catalog.CI_JOBS)
        cases = {
            "docs/queries.md": ["static"],
            "scripts/tests/test_hooks.py": ["static"],
            "src/ui.rs": everything,
            "scripts/e2e/driver.sh": ["static", "core", "package", "backend", "e2e"],
            # The UI tests embed the synthetic Codex server.
            "tests/desktop/fake-codex.py": everything,
            "tests/fixture/server/Blocking.java": ["static", "core", "package", "backend", "e2e"],
            "scripts/package/macos.sh": ["static", "core", "package"],
            ".github/workflows/checks.yml": everything,
            "scripts/qtest/catalog.py": everything,
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(catalog.ci_jobs_for_changes([path]), expected)

    def test_plan_job_selects_all_jobs_outside_pull_requests(self):
        body = job("plan")
        self.assertIn("fetch-depth: 2", body)
        self.assertIn('git diff --name-only HEAD^1 HEAD', body)
        self.assertIn("./qtest ci plan --all", body)


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
                self.assertEqual(forks_skip, name in ("backend", "e2e"))

    def test_the_tested_package_is_the_released_package(self):
        package = job("package")
        self.assertIn("QROW_RELEASE_VERSION: ${{ inputs.release-version }}", package)
        self.assertIn("name: macos-package\n", package)
        self.assertIn("path: target/package/Qrow-macos.zip", package)
        e2e = job("e2e")
        self.assertIn("name: macos-package\n", e2e)
        self.assertIn("QROW_E2E_REUSE_MACOS_PACKAGE: true", e2e)
        dmg = job("dmg", RELEASE)
        self.assertEqual(needs(dmg) or re.findall(r"^      - (\S+)$", dmg.split("runs-on")[0], re.MULTILINE),
                         ["resolve-target", "checks"])
        self.assertIn("name: macos-package\n", dmg)
        self.assertNotIn("scripts/package/macos.sh", read(RELEASE))
        self.assertIn("- dmg", job("publish", RELEASE))

    def test_only_main_saves_rust_caches(self):
        checks = read(CHECKS)
        caches = re.findall(r"rust-cache@.*\n(?:\s+.*\n)*?\s+save-if: (.*)\n", checks)
        self.assertEqual(len(caches), checks.count("rust-cache@"))
        self.assertTrue(all(value in ("${{ github.ref == 'refs/heads/main' }}", "false") for value in caches))

    def test_runs_that_save_rust_caches_delete_the_replaced_caches(self):
        cleanup = job("cache-cleanup", TEST)
        self.assertEqual(needs(cleanup), ["checks"])
        # The same condition as save-if, also after a failed check.
        self.assertIn("if: ${{ !cancelled() && github.ref == 'refs/heads/main' }}", cleanup)
        self.assertIn("actions: write", cleanup)
        self.assertIn("run: python3 scripts/ci/rust_caches.py\n", cleanup)
        # The checks run code from pull requests, so they must not delete caches.
        self.assertNotIn("actions: write", read(CHECKS))

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
        for name in ("package", "e2e", "perf"):
            self.assertIn(f"name: performance-{name}\n", job(name))
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

import datetime
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("policy", Path(__file__).resolve().parents[1] / "core/policy.py")
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class PolicyTests(unittest.TestCase):
    def run_policy(self, waiver, action):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".github/workflows").mkdir(parents=True)
            (root / "deny.toml").write_text(f"[advisories]\nignore = [{waiver}]\n")
            (root / ".github/workflows/check.yml").write_text(f"- uses: {action}\n")
            with patch.object(policy, "ROOT", root):
                return policy.check(datetime.date(2026, 9, 11))

    def test_expired_waiver_fails(self):
        errors = self.run_policy('{id="RUSTSEC-0000-0000",reason="review-by: 2026-09-10"}', 'actions/checkout@' + 'a'*40)
        self.assertTrue(any("expired" in error for error in errors))

    def test_bare_waiver_and_moving_action_tag_fail(self):
        errors = self.run_policy('"RUSTSEC-0000-0000"', 'actions/checkout@v4')
        self.assertEqual(len(errors), 2)

    def test_dated_waiver_and_full_pin_pass(self):
        errors = self.run_policy('{id="RUSTSEC-0000-0000",reason="upstream migration; review-by: 2026-12-11"}', 'actions/checkout@' + 'a'*40)
        self.assertEqual(errors, [])

    def test_license_exception_requires_current_review_for_exact_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "deny.toml").write_text('[advisories]\nignore=[]\n[[licenses.exceptions]]\nname="codec"\nversion="=1.0.0"\nallow=["bzip2-1.0.6"]\n')
            with patch.object(policy, "ROOT", root):
                today = datetime.date(2026, 9, 11)
                self.assertTrue(policy.check(today))
                review = root / "docs/dependency-reviews.toml"
                for version, date, fails in [("=1.0.1", "2026-12-11", True), ("=1.0.0", "2026-09-10", True), ("=1.0.0", "invalid", True), ("=1.0.0", "2026-12-11", False)]:
                    review.write_text(f'[[license]]\nname="codec"\nversion="{version}"\nreview_by="{date}"\nreason="Required upstream codec; supplied notices retained."\n')
                    self.assertEqual(bool(policy.check(today)), fails)

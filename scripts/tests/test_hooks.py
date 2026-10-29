"""Check that hooks test the index/commit without modifying the working tree."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class HookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = os.environ.copy()
        for key in list(self.env):
            if key.startswith("GIT_"):
                self.env.pop(key)
        self.git("init", "-q")
        (self.repo / "scripts/hooks").mkdir(parents=True)
        (self.repo / ".githooks").mkdir()
        for name in ["pre-commit", "pre-push"]:
            shutil.copy2(ROOT / ".githooks" / name, self.repo / ".githooks" / name)
        shutil.copy2(ROOT / "scripts/hooks/snapshot.sh", self.repo / "scripts/hooks/snapshot.sh")
        (self.repo / "scripts/core").mkdir()
        shutil.copy2(ROOT / "scripts/core/preflight.sh", self.repo / "scripts/core/preflight.sh")
        # A stand-in for qtest: it records its arguments and changed files, and
        # passes only when the snapshot has the good payload.
        self.log = self.repo.parent / f"{self.repo.name}.log"
        (self.repo / "qtest").write_text(
            '#!/bin/sh\nset -eu\n'
            f'printf "%s|%s\\n" "$*" "$(echo $QROW_CHANGED_FILES)" >> {self.log}\n'
            'test "$(cat payload)" = good\n')
        (self.repo / "payload").write_text("good")
        self.git("add", ".")

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.repo, env=self.env, text=True).strip()

    def hook(self, name, stdin=None):
        return subprocess.run(["sh", f".githooks/{name}"], input=stdin, cwd=self.repo, env=self.env, text=True, capture_output=True)

    def test_good_index_with_bad_worktree_passes_without_mutation(self):
        (self.repo / "payload").write_text("bad")
        tree = self.git("write-tree")
        result = self.hook("pre-commit")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("write-tree"), tree)
        self.assertEqual((self.repo / "payload").read_text(), "bad")

    def test_bad_index_with_good_worktree_is_rejected(self):
        (self.repo / "payload").write_text("bad")
        self.git("add", "payload")
        tree = self.git("write-tree")
        (self.repo / "payload").write_text("good")
        result = self.hook("pre-commit")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git("write-tree"), tree)
        self.assertEqual((self.repo / "payload").read_text(), "good")

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def commit(self, name, text="x"):
        (self.repo / name).write_text(text)
        self.git("add", name)
        self.git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", name)
        return self.git("rev-parse", "HEAD")

    def test_commit_runs_the_commit_hook_on_the_staged_files(self):
        (self.repo / "other").write_text("x")
        self.git("add", "other")
        result = self.hook("pre-commit")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[-1].split("|")[0], "hook pre-commit")
        self.assertIn("other", self.calls()[-1].split("|")[1].split())

    def test_push_checks_only_the_changes_that_the_remote_lacks(self):
        first = self.commit("first")
        second = self.commit("second")
        result = self.hook("pre-push", f"refs/heads/main {second} refs/heads/main {first}\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        arguments, changed = self.calls()[-1].split("|")
        self.assertEqual(arguments, "hook pre-push")
        self.assertEqual(changed.split(), ["second"])

    def test_new_branch_is_checked_against_the_default_branch(self):
        base = self.commit("base")
        self.git("update-ref", "refs/remotes/origin/main", base)
        tip = self.commit("feature")
        zeros = "0" * 40
        result = self.hook("pre-push", f"refs/heads/feature {tip} refs/heads/feature {zeros}\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[-1].split("|")[1].split(), ["feature"])

    def test_push_checks_requested_revision_and_skips_deletion(self):
        tree = self.git("write-tree")
        (self.repo / "payload").write_text("bad")
        zeros = "0" * 40
        result = self.hook("pre-push", f"refs/heads/main {tree} refs/heads/main {zeros}\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.hook("pre-push", f"(delete) {zeros} refs/heads/main {tree}\n")
        self.assertEqual(result.returncode, 0, result.stderr)

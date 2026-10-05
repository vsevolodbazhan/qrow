"""Check which Rust caches of `main` the cleanup deletes."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("rust_caches",
                                              Path(__file__).resolve().parents[1] / "ci/rust_caches.py")
rust_caches = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rust_caches)


def cache(id, key, created, ref="refs/heads/main", accessed="2026-10-05T23:00:00Z"):
    return {"id": id, "key": key, "ref": ref, "created_at": created, "last_accessed_at": accessed,
            "size_in_bytes": 1}


def deleted(*caches):
    return [entry["id"] for entry in rust_caches.superseded(list(caches))]


class SupersededTests(unittest.TestCase):
    def test_keeps_the_newest_cache_of_each_job(self):
        self.assertEqual(deleted(
            cache(1, "v0-rust-native-Darwin-arm64-31cc04a2-3e35e2ad", "2026-10-01T10:00:00Z"),
            cache(2, "v0-rust-native-Darwin-arm64-31cc04a2-577146a6", "2026-10-03T10:00:00Z"),
            cache(3, "v0-rust-native-Darwin-arm64-31cc04a2-9377a56a", "2026-10-05T10:00:00Z"),
            cache(4, "v0-rust-core-Linux-x64-c9181961-577146a6", "2026-10-03T10:00:00Z"),
            cache(5, "v0-rust-core-Linux-x64-c9181961-9377a56a", "2026-10-05T10:00:00Z"),
        ), [4, 1, 2])

    def test_a_toolchain_change_supersedes_the_old_cache(self):
        self.assertEqual(deleted(
            cache(1, "v0-rust-static-Linux-x64-c9181961-9377a56a", "2026-10-03T10:00:00Z"),
            cache(2, "v0-rust-static-Linux-x64-0badf00d-9377a56a", "2026-10-05T10:00:00Z"),
        ), [1])

    def test_a_pull_request_that_restores_an_old_cache_does_not_keep_it(self):
        # Pull requests restore the caches of main, which changes their time of last use.
        self.assertEqual(deleted(
            cache(1, "v0-rust-core-Linux-x64-c9181961-577146a6", "2026-10-03T10:00:00Z",
                  accessed="2026-10-05T12:01:00Z"),
            cache(2, "v0-rust-core-Linux-x64-c9181961-9377a56a", "2026-10-05T10:00:00Z",
                  accessed="2026-10-05T12:00:00Z"),
        ), [1])

    def test_jobs_on_other_architectures_are_separate(self):
        self.assertEqual(deleted(
            cache(1, "v0-rust-native-Darwin-arm64-31cc04a2-9377a56a", "2026-10-05T10:00:00Z"),
            cache(2, "v0-rust-native-Darwin-x64-5f00ba11-9377a56a", "2026-10-04T10:00:00Z"),
        ), [])

    def test_fractions_of_a_second_order_the_caches(self):
        self.assertEqual(deleted(
            cache(1, "v0-rust-core-Linux-x64-c9181961-9377a56a", "2026-10-05T14:41:10.990567Z"),
            cache(2, "v0-rust-core-Linux-x64-c9181961-577146a6", "2026-10-05T14:41:10Z"),
        ), [2])

    def test_other_caches_and_branches_stay(self):
        self.assertEqual(deleted(
            cache(1, "v0-rust-native-Darwin-arm64-31cc04a2-9377a56a", "2026-10-05T10:00:00Z"),
            cache(2, "v0-rust-native-Darwin-arm64-31cc04a2-577146a6", "2026-10-04T10:00:00Z",
                  ref="refs/pull/154/merge"),
            cache(3, "setup-uv-1-x86_64-unknown-linux-gnu-3.12.3-pruned-c137d2e2", "2026-10-01T10:00:00Z"),
            cache(4, "setup-uv-1-x86_64-unknown-linux-gnu-3.12.3-pruned-0badf00d", "2026-10-05T10:00:00Z"),
        ), [])


if __name__ == "__main__":
    unittest.main()

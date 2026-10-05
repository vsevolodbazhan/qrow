#!/usr/bin/env python3
"""Delete the Rust build caches of `main` that a newer cache replaced.

Swatinem/rust-cache names a cache `v0-rust-SHARED-OS-ARCH-ENV-LOCK`. The two
hashes change with the toolchain and with `Cargo.lock`, and each change saves
a new cache next to the old one. The old caches fill the repository cache
limit. Of the caches with the same name before the hashes, this script keeps
the one that a run restored or saved last, and deletes the others.
"""
import argparse
import datetime
import json
import os
import re
import subprocess
import sys

REF = "refs/heads/main"
RUST_KEY = re.compile(r"(?P<group>v0-rust-.+)-[0-9a-f]{8}-[0-9a-f]{8}")


def time(text):
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def superseded(caches):
    """The caches to delete: all Rust caches of `main` except the last used of each group."""
    groups = {}
    for cache in caches:
        match = RUST_KEY.fullmatch(cache["key"])
        if cache["ref"] == REF and match:
            groups.setdefault(match["group"], []).append(cache)
    stale = []
    for group in groups.values():
        group.sort(key=lambda cache: (time(cache["last_accessed_at"]), time(cache["created_at"]), cache["id"]))
        stale.extend(group[:-1])
    return sorted(stale, key=lambda cache: cache["key"])


def gh(*args):
    return subprocess.run(("gh", "api", *args), check=True, capture_output=True, text=True).stdout


def list_caches(repository):
    lines = gh("--paginate", "--jq", ".actions_caches[]",
               f"repos/{repository}/actions/caches?ref={REF}&per_page=100")
    return [json.loads(line) for line in lines.splitlines() if line]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY"),
                        help="OWNER/NAME. The default is GITHUB_REPOSITORY.")
    parser.add_argument("--dry-run", action="store_true", help="Show the caches to delete, and keep them.")
    options = parser.parse_args()
    if not options.repository:
        parser.error("set --repository or GITHUB_REPOSITORY")
    stale = superseded(list_caches(options.repository))
    for cache in stale:
        print(f"{'Would delete' if options.dry_run else 'Delete'} {cache['key']} "
              f"({cache['size_in_bytes'] / 1e9:.2f} GB, last used {cache['last_accessed_at']})")
        if not options.dry_run:
            gh("--method", "DELETE", f"repos/{options.repository}/actions/caches/{cache['id']}")
    total = sum(cache["size_in_bytes"] for cache in stale) / 1e9
    print(f"{len(stale)} superseded Rust caches, {total:.2f} GB.")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        sys.exit(f"gh api failed: {error.stderr.strip()}")

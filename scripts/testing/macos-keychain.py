"""Run macOS credential assertions with synthetic items and an isolated keychain."""
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qtest"))
import builds  # noqa: E402

def main():
    profile = "ci" if os.environ.get("CI") else "default"
    for args in (
        ["--lib", "-E", "test(/^storage::keychain_tests::/) & not test(create_foreign_token_items)"],
        ["--test", "integration", "--run-ignored", "only", "-E", "test(/^keychain::/)"],
    ):
        builds.run(["cargo", "nextest", "run", "--locked", "--no-default-features", "--profile", profile, *args])


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Reject expired dependency waivers and unpinned CI actions."""
import datetime
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]


def check(today=None):
    today = today or datetime.datetime.now(datetime.timezone.utc).date()
    errors = []
    policy = tomllib.loads((ROOT / "deny.toml").read_text())
    for waiver in policy["advisories"].get("ignore", []):
        if not isinstance(waiver, dict):
            errors.append(f"Advisory waiver requires a reason and review date: {waiver}")
            continue
        match = re.search(r"review-by: (\d{4}-\d{2}-\d{2})", waiver.get("reason", ""))
        if not match or datetime.date.fromisoformat(match[1]) < today:
            errors.append(f"Missing or expired review date: {waiver.get('id', waiver)}")
    exceptions = policy.get("licenses", {}).get("exceptions", [])
    reviews_path = ROOT / "docs/dependency-reviews.toml"
    reviews = tomllib.loads(reviews_path.read_text()).get("license", []) if reviews_path.exists() else []
    for exception in exceptions:
        review = next((entry for entry in reviews if
                       entry.get("name") == exception.get("name") and
                       entry.get("version") == exception.get("version")), None)
        if review is None or not review.get("reason", "").strip():
            errors.append(f"License exception requires a matching review: {exception}")
            continue
        try:
            expires = datetime.date.fromisoformat(review.get("review_by", ""))
        except ValueError:
            expires = datetime.date.min
        if expires < today:
            errors.append(f"Missing or expired license review date: {exception['name']}")
    for path in (ROOT / ".github/workflows").glob("*.yml"):
        for line in path.read_text().splitlines():
            match = re.search(r"\buses:\s*([^ #]+)", line)
            # Local reusable workflows run from the caller's commit.
            if match and re.fullmatch(r"\./\.github/workflows/[\w-]+\.ya?ml", match[1]):
                continue
            if match and not re.fullmatch(r"[\w.-]+/[\w./-]+@[a-f0-9]{40}", match[1]):
                errors.append(f"Pin action to a full commit SHA in {path.name}: {match[1]}")
    return errors


if __name__ == "__main__":
    problems = check()
    if problems:
        sys.exit("\n".join(problems))
    print("Dependency waiver dates and CI action pins passed.")

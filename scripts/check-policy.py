#!/usr/bin/env python3
"""Reject expired dependency waivers and unpinned CI actions."""
import datetime
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent


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
    for path in (ROOT / ".github/workflows").glob("*.yml"):
        for line in path.read_text().splitlines():
            match = re.search(r"\buses:\s*([^ #]+)", line)
            if match and not re.fullmatch(r"[\w.-]+/[\w./-]+@[a-f0-9]{40}", match[1]):
                errors.append(f"Pin action to a full commit SHA in {path.name}: {match[1]}")
    return errors


if __name__ == "__main__":
    problems = check()
    if problems:
        sys.exit("\n".join(problems))
    print("Dependency waiver dates and CI action pins passed.")

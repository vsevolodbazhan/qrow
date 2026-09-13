#!/usr/bin/env python3
"""Wait for the core workflow on this event and commit before starting E2E."""
import json
import os
import subprocess
import time


def main():
    deadline = time.monotonic() + 35 * 60
    while time.monotonic() < deadline:
        result = subprocess.run(
            ["gh", "run", "list", "--repo", os.environ["GITHUB_REPOSITORY"],
             "--workflow", "core.yml", "--commit", os.environ["CORE_SHA"],
             "--event", os.environ["CORE_EVENT"], "--limit", "1",
             "--json", "status,conclusion,url"],
            check=True, capture_output=True, text=True, timeout=60,
        )
        runs = json.loads(result.stdout)
        if runs and runs[0]["status"] == "completed":
            run = runs[0]
            if run["conclusion"] != "success":
                raise SystemExit(f"Core did not pass: {run['conclusion']} {run['url']}")
            print(f"Core passed: {run['url']}")
            return
        print("Waiting for core on this event and commit...", flush=True)
        time.sleep(15)
    raise SystemExit("Timed out waiting for core; E2E tests were not started.")


if __name__ == "__main__":
    main()

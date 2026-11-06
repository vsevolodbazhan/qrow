"""Four-target validation policy and a strict, offline matrix evidence gate."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qtest"))
import contract  # noqa: E402
from targets import TARGETS  # noqa: E402


def suites(target, full=False):
    result = ["clippy-app", "unit", "ui", "linux-package" if target.system == "linux" else "package", "package-smoke"]
    if full:
        result += ["e2e", "postgres", "trino"] if target.system == "linux" else ["e2e", "desktop"]
    return result


def matrix(full=False):
    return {"include": [{"target": target.name, "runner": target.runner, "system": target.system,
                         "runtime": "docker" if target.system == "linux" else "native",
                         "suites": " ".join(suites(target, full))} for target in TARGETS.values() if target.supported]}


def verify(directory, sha, full=False):
    expected = {name for name, target in TARGETS.items() if target.supported}
    seen = set()
    for path in Path(directory).rglob("result.json"):
        value = json.loads((path.parent / "plan.json").read_text())
        result = json.loads(path.read_text())
        receipt = json.loads((path.parent / "receipt.json").read_text())
        name = value.get("target")
        if name not in expected or name in seen:
            raise ValueError("Duplicate or unexpected target result")
        if value.get("source_sha") != sha or result.get("source_sha") != sha or receipt.get("source_sha") != sha:
            raise ValueError("Candidate source commit mismatch")
        runtime = "docker" if TARGETS[name].system == "linux" else "native"
        if value.get("selectors") != suites(TARGETS[name], full) or value.get("repeat") != 1 or value.get("runtime") != runtime:
            raise ValueError("Unexpected validation coverage")
        contract.verify(value, result)
        if receipt.get("schema") != 1 or receipt.get("target") != name or receipt.get("qualification") != "native-package-smoke":
            raise ValueError("Package qualification receipt mismatch")
        if len(receipt.get("sha256", "")) != 64:
            raise ValueError("Missing candidate digest")
        seen.add(name)
    if seen != expected:
        raise ValueError(f"Missing native target results: {sorted(expected - seen)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("matrix").add_argument("--full", action="store_true")
    gate = commands.add_parser("verify")
    gate.add_argument("directory", type=Path)
    gate.add_argument("sha")
    gate.add_argument("--full", action="store_true")
    args = parser.parse_args()
    if args.command == "matrix":
        print(json.dumps(matrix(args.full)))
    else:
        verify(args.directory, args.sha, args.full)
        print("All supported native targets and packages passed.")


if __name__ == "__main__":
    main()

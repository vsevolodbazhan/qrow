"""Common validation, native build/test ownership, and the offline evidence gate."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qtest"))
import contract  # noqa: E402
from targets import TARGETS  # noqa: E402

COMMON = {
    "static": ["scripts", "policy", "deps", "fmt"],
    "core": ["clippy", "rustdoc", "coverage"],
    "backend": ["backend", "postgres-core", "trino-core"],
    "headless-ui": ["clippy-app", "app-unit", "ui", "e2e", "postgres-ui", "trino-ui"],
    "performance": ["perf"],
}
COMMON_TARGET = "linux-x86_64"


def suites(target, full=False):
    # Native contracts contain only OS assertions and the production candidate.
    # Full mode adds measurements elsewhere, not another functional test set.
    return ["native-storage", "package-smoke", *(["native-font", "native-ui", "native-assistant", "desktop"]
                                                 if target.system == "macos" else [])]


def matrix(full=False):
    return {"include": [{"target": target.name, "runner": target.runner, "system": target.system,
                         "runtime": "docker" if target.system == "linux" else "native",
                         "suites": " ".join(suites(target, full))} for target in TARGETS.values() if target.supported]}


def common_matrix():
    return {"include": [{"job": name, "suites": " ".join(suites),
                         "prepare": name in ("backend", "headless-ui")} for name, suites in COMMON.items()]}


def check_plan(value, result, sha, selectors, target, runtime):
    if value.get("source_sha") != sha or result.get("source_sha") != sha:
        raise ValueError("Candidate source commit mismatch")
    if (value.get("target") != target or value.get("selectors") != selectors or
            value.get("repeat") != 1 or value.get("runtime") != runtime):
        raise ValueError("Unexpected validation coverage")
    contract.verify(value, result)


def verify_common(directory, sha, full=False):
    seen = set()
    for path in Path(directory).rglob("result.json"):
        value = json.loads((path.parent / "plan.json").read_text())
        result = json.loads(path.read_text())
        matches = [name for name, selectors in COMMON.items() if value.get("selectors") == selectors]
        if len(matches) != 1 or matches[0] in seen:
            raise ValueError("Duplicate or unexpected common result")
        check_plan(value, result, sha, COMMON[matches[0]], COMMON_TARGET, "docker")
        if matches[0] in ("backend", "headless-ui"):
            import builds
            manifest_path = path.parent / "build-manifest.json"
            manifest = json.loads(manifest_path.read_text())
            if (result.get("build_manifest_sha256") != builds.sha256(manifest_path) or
                    manifest.get("identity") != builds.identity(value)):
                raise ValueError("Common prepared build identity mismatch")
        if matches[0] == "headless-ui" and full:
            stress = json.loads((path.parent / "stress-plan.json").read_text())
            stress_result = json.loads((path.parent / "stress-result.json").read_text())
            if (stress.get("selectors") != ["ui/timing"] or stress.get("repeat") != 100 or
                    stress.get("source_sha") != sha or stress.get("target") != COMMON_TARGET or
                    stress_result.get("build_manifest_sha256") != result.get("build_manifest_sha256")):
                raise ValueError("Missing UI stress contract")
            contract.verify(stress, stress_result)
        seen.add(matches[0])
    if seen != set(COMMON):
        raise ValueError(f"Missing common results: {sorted(set(COMMON) - seen)}")


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
        runtime = "docker" if TARGETS[name].system == "linux" else "native"
        check_plan(value, result, sha, suites(TARGETS[name], full), name, runtime)
        if receipt.get("source_sha") != sha:
            raise ValueError("Candidate source commit mismatch")
        if receipt.get("schema") != 1 or receipt.get("target") != name or receipt.get("qualification") != "native-package-smoke":
            raise ValueError("Package qualification receipt mismatch")
        if len(receipt.get("sha256", "")) != 64:
            raise ValueError("Missing candidate digest")
        import builds
        manifest_path = path.parent / "build-manifest.json"
        manifest = json.loads(manifest_path.read_text())
        if (result.get("build_manifest_sha256") != builds.sha256(manifest_path) or
                manifest.get("identity") != builds.identity(value) or
                manifest.get("assets", {}).get("target/package/" + receipt.get("archive", "")) != receipt["sha256"]):
            raise ValueError("Prepared build or package identity mismatch")
        seen.add(name)
    if seen != expected:
        raise ValueError(f"Missing native target results: {sorted(expected - seen)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("matrix").add_argument("--full", action="store_true")
    commands.add_parser("common-matrix")
    for name in ("verify", "verify-common"):
        gate = commands.add_parser(name)
        gate.add_argument("directory", type=Path)
        gate.add_argument("sha")
        gate.add_argument("--full", action="store_true")
    args = parser.parse_args()
    if args.command == "matrix":
        print(json.dumps(matrix(args.full)))
    elif args.command == "common-matrix":
        print(json.dumps(common_matrix()))
    else:
        if args.command == "verify-common":
            verify_common(args.directory, args.sha, args.full)
        else:
            verify(args.directory, args.sha, args.full)
        print("All expected validation evidence passed.")


if __name__ == "__main__":
    main()

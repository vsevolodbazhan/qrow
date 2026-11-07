"""Common validation, native build/test ownership, and the offline evidence gate."""
import argparse
import json
import os
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
# Pull requests qualify one native target for fast feedback. main and release
# candidates qualify all supported targets. A merge queue would too, but this
# personal-account repository cannot have one.
PR_TARGETS = ("linux-x86_64",)
# A test job name uses hyphens: test-macos-x86-64 tests macos-x86_64.
TEST_JOB = "test-{}"


def tier(event, full=False):
    """pr, merge, or full. Full mode adds timing stress and measurements."""
    if full:
        return "full"
    return {"pull_request": "pr", "merge_group": "merge"}.get(event, "full")


def native_targets(selected_tier):
    supported = [name for name, target in TARGETS.items() if target.supported]
    return [name for name in supported if selected_tier != "pr" or name in PR_TARGETS]


def outputs(event, full=False):
    selected = tier(event, full)
    return {"common": json.dumps(common_matrix()), "tier": selected,
            "full": "true" if selected == "full" else "false",
            "targets": json.dumps(native_targets(selected))}


def gate(results, targets):
    """Require the plan, common validation, and the tests of the selected targets. Other targets must not run."""
    if set(targets) - set(native_targets("full")) or not set(PR_TARGETS) <= set(targets):
        raise ValueError(f"Unexpected native target selection: {targets}")
    for name in ("plan", "common-required"):
        if results.get(name, {}).get("result") != "success":
            raise ValueError(f"{name} did not pass")
    for name in native_targets("full"):
        job = TEST_JOB.format(name.replace("_", "-"))
        expected = "success" if name in targets else "skipped"
        if results.get(job, {}).get("result") != expected:
            raise ValueError(f"{job} must be {expected}")


# Production Clippy runs on one architecture of each OS. Other architectures
# add little lint coverage and a complete check build.
LINT_TARGETS = ("macos-arm64", "linux-x86_64")


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


def verify(directory, sha, full=False, targets=None):
    supported = {name for name, target in TARGETS.items() if target.supported}
    expected = supported if targets is None else set(targets)
    if not expected <= supported or not set(PR_TARGETS) <= expected:
        raise ValueError(f"Unexpected native target selection: {sorted(expected)}")
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
    plan = commands.add_parser("outputs", help="Print the plan job outputs for GITHUB_OUTPUT.")
    plan.add_argument("--event", required=True)
    plan.add_argument("--full", action="store_true")
    commands.add_parser("gate", help="Check the job results in RESULTS for the targets in TARGETS.")
    for name in ("verify", "verify-common"):
        evidence = commands.add_parser(name)
        evidence.add_argument("directory", type=Path)
        evidence.add_argument("sha")
        evidence.add_argument("--full", action="store_true")
        if name == "verify":
            evidence.add_argument("--targets", type=json.loads, help="JSON list of the selected native targets.")
    args = parser.parse_args()
    if args.command == "matrix":
        print(json.dumps(matrix(args.full)))
    elif args.command == "common-matrix":
        print(json.dumps(common_matrix()))
    elif args.command == "outputs":
        for key, value in outputs(args.event, args.full).items():
            print(f"{key}={value}")
    elif args.command == "gate":
        try:
            gate(json.loads(os.environ["RESULTS"]), json.loads(os.environ["TARGETS"]))
        except (KeyError, ValueError) as error:
            sys.exit(str(error))
        print("All required jobs passed.")
    else:
        if args.command == "verify-common":
            verify_common(args.directory, args.sha, args.full)
        else:
            verify(args.directory, args.sha, args.full, args.targets)
        print("All expected validation evidence passed.")


if __name__ == "__main__":
    main()

"""Common validation, native build/test ownership, and the offline evidence gate."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qtest"))
import contract  # noqa: E402
from targets import TARGETS  # noqa: E402

COMMON = {
    "static": ["scripts", "policy", "deps", "fmt"],
    "core": ["clippy", "rustdoc", "coverage"],
    "backend": ["backend", "postgres-core", "trino-core"],
    "headless-ui": ["clippy-app", "app-unit", "ui"],
    # Server-window scenarios run beside the server-free UI tests.
    "server-ui": ["e2e", "postgres-ui", "trino-ui"],
    "performance": ["perf"],
}
COMMON_TARGET = "linux-x86_64"
# Jobs that prepare their test binaries once and verify the build identity.
PREPARED = ("backend", "headless-ui", "server-ui")
# server-ui compiles the same UI-test dependencies as headless-ui. It restores
# that cache and does not save one, so each cache keeps a single saving job.
CACHES = {"server-ui": "headless-ui"}
# Pull requests qualify one native target for fast feedback. main and release
# candidates qualify all supported targets. A merge queue would too, but this
# personal-account repository cannot have one.
PR_TARGETS = ("linux-x86_64",)
# A test job name uses hyphens: test-macos-x86-64 tests macos-x86_64.
TEST_JOB = "test-{}"


# Changes that cannot alter a Rust build, a test binary, a fixture, or the
# validation graph. The static job checks them. Any other path selects the
# complete level. Vendored crates can embed their README, so only these
# Markdown files qualify.
STATIC_ONLY = re.compile(r"(docs/.+|[^/]+\.md|scripts/README\.md|scripts/tests/.+|scripts/ci/rust_caches\.py"
                         r"|\.agents/.+|\.claude|\.plans/.+|\.github/workflows/(maintenance|release)\.yml)")
# The levels that each event can select. Other combinations fail the gate.
EVENT_TIERS = {"pull_request": ("static", "pr", "full"), "merge_group": ("merge", "full")}


def changed_paths(base, root=Path(".")):
    """Paths that differ between `base` and HEAD.

    Without rename detection, a moved file reports its old path too. Thus a
    file that moves from tests/ into docs/ still selects the complete level."""
    output = subprocess.run(["git", "diff", "--no-renames", "--name-only", "-z", base, "HEAD"],
                            cwd=root, capture_output=True, check=True).stdout
    return [os.fsdecode(name) for name in output.split(b"\0") if name]


def static_only(paths):
    return bool(paths) and all(STATIC_ONLY.fullmatch(path) for path in paths)


def tier(event, full=False, paths=None):
    """static, pr, merge, or full. Full mode adds timing stress and measurements.

    A pull request that changes only STATIC_ONLY paths runs the static job."""
    if full:
        return "full"
    if event == "pull_request" and paths is not None and static_only(paths):
        return "static"
    return {"pull_request": "pr", "merge_group": "merge"}.get(event, "full")


def native_targets(selected_tier):
    supported = [name for name, target in TARGETS.items() if target.supported]
    if selected_tier == "static":
        return []
    return [name for name in supported if selected_tier != "pr" or name in PR_TARGETS]


def common_jobs(selected_tier):
    return ["static"] if selected_tier == "static" else list(COMMON)


def outputs(event, full=False, paths=None):
    selected = tier(event, full, paths)
    return {"common": json.dumps(common_matrix(selected)), "common_jobs": json.dumps(common_jobs(selected)),
            "tier": selected, "full": "true" if selected == "full" else "false",
            "targets": json.dumps(native_targets(selected))}


def gate(results, targets, selected_tier, event):
    """Require the plan, common validation, and the tests of the selected targets. Other targets must not run."""
    if selected_tier not in EVENT_TIERS.get(event, ("full",)):
        raise ValueError(f"A {event} run cannot use the {selected_tier} level")
    if list(targets) != native_targets(selected_tier):
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


def common_matrix(selected_tier="full"):
    return {"include": [{"job": name, "suites": " ".join(COMMON[name]),
                         "prepare": name in PREPARED, "cache": CACHES.get(name, name)}
                        for name in common_jobs(selected_tier)]}


def check_plan(value, result, sha, selectors, target, runtime):
    if value.get("source_sha") != sha or result.get("source_sha") != sha:
        raise ValueError("Candidate source commit mismatch")
    if (value.get("target") != target or value.get("selectors") != selectors or
            value.get("repeat") != 1 or value.get("runtime") != runtime):
        raise ValueError("Unexpected validation coverage")
    contract.verify(value, result)


def verify_common(directory, sha, full=False, jobs=None):
    expected = set(COMMON if jobs is None else jobs)
    if not expected or not expected <= set(COMMON) or "static" not in expected:
        raise ValueError(f"Unexpected common job selection: {sorted(expected)}")
    seen = set()
    for path in Path(directory).rglob("result.json"):
        value = json.loads((path.parent / "plan.json").read_text())
        result = json.loads(path.read_text())
        matches = [name for name, selectors in COMMON.items() if value.get("selectors") == selectors]
        if len(matches) != 1 or matches[0] in seen or matches[0] not in expected:
            raise ValueError("Duplicate or unexpected common result")
        check_plan(value, result, sha, COMMON[matches[0]], COMMON_TARGET, "docker")
        if matches[0] in PREPARED:
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
    if seen != expected:
        raise ValueError(f"Missing common results: {sorted(expected - seen)}")


def verify(directory, sha, full=False, targets=None):
    supported = {name for name, target in TARGETS.items() if target.supported}
    expected = supported if targets is None else set(targets)
    # An empty selection belongs to the static level, which gate() checks.
    if not expected <= supported or (expected and not set(PR_TARGETS) <= expected):
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
    plan.add_argument("--changed-since", help="Select by the paths that changed since this commit.")
    commands.add_parser("gate", help="Check the job results in RESULTS for TARGETS, TIER, and EVENT.")
    for name in ("verify", "verify-common"):
        evidence = commands.add_parser(name)
        evidence.add_argument("directory", type=Path)
        evidence.add_argument("sha")
        evidence.add_argument("--full", action="store_true")
        if name == "verify":
            evidence.add_argument("--targets", type=json.loads, help="JSON list of the selected native targets.")
        else:
            evidence.add_argument("--jobs", type=json.loads, help="JSON list of the selected common jobs.")
    args = parser.parse_args()
    if args.command == "matrix":
        print(json.dumps(matrix(args.full)))
    elif args.command == "common-matrix":
        print(json.dumps(common_matrix()))
    elif args.command == "outputs":
        paths = changed_paths(args.changed_since) if args.changed_since else None
        for key, value in outputs(args.event, args.full, paths).items():
            print(f"{key}={value}")
    elif args.command == "gate":
        try:
            gate(json.loads(os.environ["RESULTS"]), json.loads(os.environ["TARGETS"]), os.environ["TIER"],
                 os.environ["EVENT"])
        except (KeyError, ValueError) as error:
            sys.exit(str(error))
        print("All required jobs passed.")
    else:
        if args.command == "verify-common":
            verify_common(args.directory, args.sha, args.full, args.jobs)
        else:
            verify(args.directory, args.sha, args.full, args.targets)
        print("All expected validation evidence passed.")


if __name__ == "__main__":
    main()

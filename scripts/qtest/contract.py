"""Versioned plans and evidence. Planning is separate from host preflight and execution."""
from dataclasses import asdict
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import tomllib
import uuid

import catalog
import targets

SCHEMA = 1
BUILD_ENV = ("RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTFLAGS", "RUSTDOCFLAGS", "CARGO_BUILD_TARGET", "MACOSX_DEPLOYMENT_TARGET",
             "QROW_RELEASE_VERSION", "QROW_COMMIT", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN",
             "CC", "CXX", "CFLAGS", "CXXFLAGS", "SDKROOT", "CARGO_HOME", "QROW_E2E_REUSE_MACOS_PACKAGE", "QROW_MACOS_PACKAGE")


def cargo_config_paths(root):
    root = Path(root).resolve()
    paths = {}
    for depth, directory in enumerate((root, *root.parents)):
        label = "workspace" if depth == 0 else f"ancestor-{depth}"
        for name in ("config", "config.toml"):
            path = directory / ".cargo" / name
            if path.is_file():
                paths[f"{label}/.cargo/{name}"] = path
    home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    for name in ("config", "config.toml"):
        path = home / name
        if path.is_file():
            paths = {label: item for label, item in paths.items() if item != path}
            paths[f"cargo-home/{name}"] = path
    return paths


def cargo_configuration(root):
    # Logical locations let an unchanged plan move between checkout paths.
    return {label: hashlib.sha256(path.read_bytes()).hexdigest() for label, path in cargo_config_paths(root).items()}


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def atomic_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}-{uuid.uuid4().hex}")
    try:
        temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def snapshot(root):
    """Hash source bytes and executable bits, including dirty/untracked source and hook exports."""
    root = Path(root)
    result = subprocess.run(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
                            cwd=root, capture_output=True, check=False)
    top = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=root, capture_output=True, text=True)
    if result.returncode == 0 and top.returncode == 0 and Path(top.stdout.strip()).resolve() == root.resolve():
        names = sorted(set(os.fsdecode(name) for name in result.stdout.split(b"\0") if name))
    else:
        # Hook snapshots have no .git directory. These generated directories are
        # never source inputs. Every other exported file contributes to identity.
        names = sorted(str(path.relative_to(root)) for path in root.rglob("*")
                       if not set(path.relative_to(root).parts) & {".git", "target", ".venv", ".plans", "__pycache__"}
                       and (path.is_file() or path.is_symlink()))
    entries = []
    for name in names:
        path = root / name
        if path.is_symlink():
            entries.append((name, "symlink", os.readlink(path)))
        elif path.is_file():
            entries.append((name, bool(path.stat().st_mode & 0o111), hashlib.sha256(path.read_bytes()).hexdigest()))
        else:
            entries.append((name, "missing"))
    return digest(entries)


def source_sha(root):
    result = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else None


def definitions(target, selectors):
    suites = catalog.suites_for_platform(target.system)
    chosen = []
    seen = set()
    for selector in selectors or ["default"]:
        name, _, test_filter = selector.partition("/")
        names = catalog.GROUPS[name][1] if name in catalog.GROUPS and not test_filter else (name,)
        for item in names:
            if item not in suites:
                raise ValueError(f"Unknown suite: {item}")
            suite = suites[item]
            if target.system not in suite.platforms:
                raise ValueError(f"{item} cannot run on {target.name}")
            if test_filter and (not suite.filterable or not re.fullmatch(r"[A-Za-z0-9_:]+", test_filter)):
                raise ValueError(f"Invalid filter: {selector}")
            if item in seen:
                raise ValueError(f"Select each suite once: {item}")
            seen.add(item)
            chosen.append({"name": item, "filter": test_filter or None, "definition": asdict(suite)})
    return chosen


def plan(root, target_name, selectors, runtime="auto", repeat=1):
    target = targets.TARGETS[target_name]
    if not target.supported:
        raise ValueError(f"{target.name} is planned, not supported")
    if runtime not in ("auto", "native", "docker") or type(repeat) is not int or repeat < 1:
        raise ValueError("Invalid runtime or repeat count")
    build_names = set(BUILD_ENV) | {name for name in os.environ
                                  if name.startswith(("CARGO_PROFILE_", "CARGO_TARGET_", "CC_", "CXX_"))
                                  and name != "CARGO_TARGET_DIR"}
    value = {"schema": SCHEMA, "target": target.name, "source_sha": source_sha(root),
             "snapshot": snapshot(root), "selectors": selectors, "runtime": runtime, "repeat": repeat,
             "environment": {name: os.environ.get(name) for name in sorted(build_names)},
             "cargo_configuration": cargo_configuration(root),
             "suites": definitions(target, selectors)}
    value = json.loads(json.dumps(value))
    value["id"] = digest(value)
    return value


def validate_plan(value, root, build=False):
    """Check that the plan matches this source and configuration.

    Execution needs the plan target as host. A build can also run on another
    architecture of the same OS: it then compiles for the plan target."""
    if value.get("schema") != SCHEMA:
        raise ValueError("Unsupported plan schema")
    try:
        expected = plan(root, value["target"], value["selectors"], value["runtime"], value["repeat"])
    except (KeyError, TypeError) as error:
        raise ValueError("Invalid plan") from error
    if value != expected:
        raise ValueError("Plan differs from the source snapshot, configuration, or suite definitions. Plan again.")
    host = targets.host_target()
    target = targets.TARGETS[value["target"]]
    if host is None or (host.name != target.name and not (build and host.system == target.system)):
        raise ValueError(f"Native execution requires {value['target']}; this host is {host.name if host else 'unknown'}")
    configured = os.environ.get("CARGO_BUILD_TARGET")
    if configured and configured != target.triple:
        raise ValueError("CARGO_BUILD_TARGET conflicts with the native plan target")
    if not configured:
        for path in cargo_config_paths(root).values():
            configured = tomllib.loads(Path(path).read_text(encoding="utf-8")).get("build", {}).get("target")
            if configured and configured != target.triple:
                raise ValueError(f"Cargo build.target conflicts with the native plan target: {path}")


def verify(value, result):
    """Evidence cannot override a failed/missing suite, mismatched source, or empty test selection."""
    if value.get("schema") != SCHEMA or value.get("id") != digest({key: item for key, item in value.items() if key != "id"}):
        raise ValueError("Invalid plan identity")
    target = targets.TARGETS.get(value.get("target"))
    if not target or not target.supported or value.get("suites") != json.loads(json.dumps(definitions(target, value["selectors"]))):
        raise ValueError("Invalid target or suite definitions")
    if result.get("schema") != SCHEMA or result.get("plan_id") != value.get("id"):
        raise ValueError("Result does not belong to this plan")
    if result.get("target") != value["target"] or result.get("snapshot") != value["snapshot"]:
        raise ValueError("Result target or source mismatch")
    if result.get("source_sha") != value.get("source_sha"):
        raise ValueError("Result commit mismatch")
    if result.get("status") != "passed" or result.get("exit_code") != 0:
        raise ValueError("Execution did not pass")
    expected = [item["name"] for item in value["suites"]]
    actual = result.get("suites", [])
    if not expected or [item.get("name") for item in actual] != expected:
        raise ValueError("Missing, duplicate, or unexpected suite results")
    for definition, item in zip(value["suites"], actual):
        if item.get("status") != "passed" or item.get("report_only"):
            raise ValueError(f"Required suite did not pass: {item.get('name')}")
        count = sum(step["nextest_filter"] is not None for step in definition["definition"]["steps"]) + definition["definition"]["test_runs"]
        if count:
            reports = item.get("tests", [])
            if len(reports) != value["repeat"] * count or any(report.get("count", 0) < 1 or report.get("failures") != 0 for report in reports):
                raise ValueError(f"Missing or empty test evidence: {item['name']}")


def execute(value, root, output, reuse_builds=None):
    import runner

    validate_plan(value, root)
    target = targets.TARGETS[value["target"]]
    suites = catalog.suites_for_platform(target.system)
    selected = [runner.Selected(suites[item["name"]], item["filter"]) for item in value["suites"]]
    result = runner.run(selected, {"runtime": value["runtime"], "repeat": value["repeat"],
                                   "isolated_reports": True, "reuse_builds": reuse_builds}, [], output, require_all=True)
    if reuse_builds:
        from builds import sha256
        result["build_manifest_sha256"] = sha256(reuse_builds)
    result.update(schema=SCHEMA, plan_id=value["id"], target=target.name, snapshot=value["snapshot"],
                  source_sha=value["source_sha"])
    if snapshot(root) != value["snapshot"] or source_sha(root) != value["source_sha"]:
        result.update(status="failed", exit_code=1, reason="Source changed during execution")
    if result["exit_code"] == 0:
        try:
            verify(value, result)
        except ValueError as error:
            result.update(status="failed", exit_code=1, reason=str(error))
    atomic_json(Path(result["artifacts"]) / "summary.json", result)
    return result

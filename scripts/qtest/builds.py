"""Prepare nextest variants once and execute source-bound archives without Cargo builds."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid
from xml.etree import ElementTree

import contract

# Fail closed on unknown build flags. Runtime flags stay in the nextest command.
VALUE_FLAGS = {"--features", "--cargo-profile", "--target"}
BOOL_FLAGS = {"--no-default-features", "--all-features", "--release"}
TARGET_FLAGS = {"--test", "--bin", "--example", "--bench"}


def split(command):
    if list(command[:3]) != ["cargo", "nextest", "run"]:
        raise ValueError("Expected cargo nextest run")
    build, selection, runtime = [], [], []
    args = iter(command[3:])
    for arg in args:
        if arg in VALUE_FLAGS:
            build.extend((arg, next(args)))
        elif arg in BOOL_FLAGS:
            build.append(arg)
        elif arg in TARGET_FLAGS:
            name = next(args)
            selection.extend((arg, name))
            runtime.extend(("-E", f"binary(={name})"))
        elif arg == "--lib":
            selection.append(arg)
            runtime.extend(("-E", "kind(lib)"))
        elif arg in ("--locked", "--offline", "--frozen"):
            continue
        elif arg in ("--tests", "--all-targets", "--bins", "--examples", "--benches", "--target-dir", "--manifest-path", "--config", "-Z", "--build-jobs"):
            raise ValueError(f"Unsupported prepared build flag: {arg}")
        else:
            runtime.append(arg)
    # Flag order does not create a different compiler variant.
    values = {}
    args = iter(build)
    for arg in args:
        values[arg] = next(args) if arg in VALUE_FLAGS else None
    build = [item for flag in sorted(values) for item in ((flag, values[flag]) if values[flag] is not None else (flag,))]
    filters, retained = [], []
    args = iter(runtime)
    for arg in args:
        if arg in ("-E", "--filterset"):
            filters.append(next(args))
        else:
            retained.append(arg)
    if filters:
        retained.extend(("-E", " & ".join(f"({expression})" for expression in filters)))
    return build, selection, retained


def commands(value):
    for suite in value["suites"]:
        definition = suite["definition"]
        for step in definition["steps"]:
            if step["nextest_filter"] is not None:
                yield step["command"]
        yield from definition["builds"]


def variants(value):
    groups = {}
    for command in commands(value):
        flags, selected, _ = split(command)
        key = contract.digest(flags)[:16]
        group = groups.setdefault(key, {"flags": flags, "targets": []})
        # An unqualified command means all test targets. Do not accidentally
        # narrow that union when another suite names only one binary.
        if not selected:
            group["all"] = True
        for index in range(0, len(selected)):
            if selected[index].startswith("--"):
                pair = selected[index:index + (1 if selected[index] == "--lib" else 2)]
                if pair not in group["targets"]:
                    group["targets"].append(pair)
    return groups


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def identity(value):
    return {key: value[key] for key in ("source_sha", "snapshot", "target", "environment", "cargo_configuration")}


def prepare(value, root, directory, output):
    import runner
    with runner.lease([f"cargo:{runner.target_dir().resolve()}"]):
        return _prepare(value, root, directory, output)


def _prepare(value, root, directory, output):
    import runner
    contract.validate_plan(value, root)
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    manifest = {"schema": 1, "identity": identity(value), "toolchain": toolchain(root), "variants": {}}
    env = os.environ.copy()
    if value["source_sha"] and not env.get("QROW_COMMIT"):
        env["QROW_COMMIT"] = value["source_sha"][:12]
    for key, group in variants(value).items():
        archive = directory / f"{key}.tar.zst"
        selection = [] if group.get("all") else [arg for pair in group["targets"] for arg in pair]
        command = ["cargo", "nextest", "archive", "--locked", *group["flags"], *selection,
                   "--archive-file", str(archive)]
        if runner.execute(command, env, 90 * 60, directory / "build.log", output) != 0:
            raise ValueError("Test preparation failed")
        manifest["variants"][key] = {"flags": group["flags"], "all": group.get("all", False),
                                      "targets": group["targets"], "archive": archive.name, "sha256": sha256(archive)}
    contract.validate_plan(value, root)
    contract.atomic_json(directory / "manifest.json", manifest)
    return manifest


def toolchain(root):
    return subprocess.run(["rustc", "-Vv"], cwd=root, capture_output=True, text=True, check=True).stdout


class Prepared:
    def __init__(self, path, value, root, directory, output):
        import runner
        self.path = Path(path).resolve()
        self.value = json.loads(self.path.read_text())
        if self.value.get("schema") != 1 or self.value.get("identity") != identity(value):
            raise ValueError("Prepared binaries differ from source, target, or compiler configuration")
        if self.value.get("toolchain") != toolchain(root):
            raise ValueError("Prepared binaries use a different Rust toolchain")
        self.arguments = {}
        for key, required in variants(value).items():
            group = self.value["variants"].get(key)
            if not group or group["flags"] != required["flags"] or (not group["all"] and
                    (required.get("all") or any(pair not in group["targets"] for pair in required["targets"]))):
                raise ValueError("Missing prepared test variant or binary")
            archive = self.path.parent / group["archive"]
            if archive.parent != self.path.parent or sha256(archive) != group["sha256"]:
                raise ValueError("Prepared test archive digest mismatch")
            destination = Path(directory) / key
            destination.mkdir(parents=True)
            command = ["cargo", "nextest", "list", "--archive-file", str(archive), "--extract-to", str(destination),
                       "--workspace-remap", str(root), "--list-type", "binaries-only"]
            if runner.execute(command, os.environ.copy(), 600, Path(directory) / "extract.log", output) != 0:
                raise ValueError("Cannot extract prepared test archive")
            target = destination / "target"
            self.arguments[key] = ["--cargo-metadata", str(target / "nextest/cargo-metadata.json"),
                                   "--binaries-metadata", str(target / "nextest/binaries-metadata.json"),
                                   "--target-dir-remap", str(target), "--workspace-remap", str(root)]
        contract.atomic_json(Path(directory) / "runtime.json", self.arguments)
        self.runtime = str(Path(directory) / "runtime.json")

    def command(self, command):
        return reuse(command, self.arguments)


def reuse(command, arguments):
    flags, _, runtime = split(command)
    key = contract.digest(flags)[:16]
    if key not in arguments:
        raise ValueError("No prepared build for this nextest command")
    return ["cargo", "nextest", "run", *arguments[key], *runtime]


def run(command, env=None):
    """Fixture adapter entry point. Keep its runtime/filter, use qtest's prepared build."""
    env = dict(os.environ if env is None else env)
    if env.get("QROW_NEXTEST_RUNTIME"):
        command = reuse(command, json.loads(Path(env["QROW_NEXTEST_RUNTIME"]).read_text()))
    evidence = env.get("QROW_NEXTEST_EVIDENCE")
    report = None
    if evidence:
        store = Path(evidence) / uuid.uuid4().hex
        store.mkdir(parents=True)
        config = store / "config.toml"
        root = Path(__file__).resolve().parents[2]
        config.write_text('[store]\ndir = ' + json.dumps(str(store)) + '\n' + (root / ".config/nextest.toml").read_text())
        command = [*command, "--config-file", str(config)]
        report = store / ("ci" if env.get("CI") else "default") / "junit.xml"
    subprocess.run(command, env=env, check=True)
    if report:
        cases = list(ElementTree.parse(report).getroot().iter("testcase"))
        result = {"count": sum(case.find("skipped") is None for case in cases),
                  "failures": sum(case.find("failure") is not None or case.find("error") is not None for case in cases),
                  "report": str(report)}
        contract.atomic_json(report.parent / "evidence.json", result)
        if result["count"] < 1 or result["failures"]:
            raise ValueError("Fixture adapter produced empty or failed test evidence")


if __name__ == "__main__":
    run(sys.argv[1:])

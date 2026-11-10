"""Build native candidates, then verify their source-bound artifacts on a test runner."""
import argparse
import contextlib
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qtest"))
import builds  # noqa: E402
import cli  # noqa: E402
import contract  # noqa: E402
import runner  # noqa: E402
from targets import TARGETS, host_target  # noqa: E402
import platforms  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / "target/platform"
PREPARED = BASE / "builds"
SWIFT_ARCH = {"aarch64": "arm64", "x86_64": "x86_64"}


CARGO_CONFIG = ROOT / ".cargo/config.toml"


def macos_minimum():
    return tomllib.loads(CARGO_CONFIG.read_text())["env"]["MACOSX_DEPLOYMENT_TARGET"]


def checked(args):
    if cli.main(args):
        raise ValueError("Native validation command failed: " + " ".join(args))


def asset_paths(target):
    package = ROOT / "target/package"
    if target.system in ("linux", "windows"):
        return sorted(package.glob("*.zip" if target.system == "windows" else "*.tar.gz"))
    return [package / "Qrow-macos.zip", *(ROOT / "target/native-tools").glob("*"),
            ROOT / "target/e2e-tools/native-driver"]


@contextlib.contextmanager
def cargo_target(target, cross):
    """Make Cargo compile for `target` during the block. Plans exclude this variable, so the test host can validate them."""
    if not cross:
        yield
        return
    previous = os.environ.get("CARGO_BUILD_TARGET")
    os.environ["CARGO_BUILD_TARGET"] = target.triple
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop("CARGO_BUILD_TARGET", None)
        else:
            os.environ["CARGO_BUILD_TARGET"] = previous


def build(full, target_name=None):
    host = host_target()
    target = TARGETS[target_name] if target_name else host
    if target is None or host is None:
        raise ValueError("Unsupported native target")
    cross = host != target
    if cross and host.system != target.system:
        raise ValueError(f"Cannot build {target.name} on {host.name}")
    runtime = "docker" if target.system == "linux" else "native"
    selectors = platforms.suites(target, full)
    value = contract.plan(ROOT, target.name, selectors, runtime)
    contract.atomic_json(BASE / "plan.json", value)
    measurement = full and target.name == "macos-arm64"
    preparation = contract.plan(ROOT, target.name, selectors + (["perf-ui", "perf-e2e"] if measurement else []), runtime)
    if cli.install_tools(["cargo-nextest"]):
        raise ValueError("Cannot install nextest")
    builds.prepare(preparation, ROOT, PREPARED, runner.Output(False))
    package = {"linux": "linux-package", "windows": "windows-package"}.get(target.system, "package")
    with cargo_target(target, cross):
        checked(["run", *(["clippy-native"] if target.name in platforms.LINT_TARGETS else []), package, "--fail-fast"])
    if target.system == "macos":
        tools = ROOT / "target/native-tools"
        tools.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, QROW_COMMIT=value["source_sha"][:12])
        cross_flags = ["--target", target.triple] if cross else []
        swift_target = ["-target", f"{SWIFT_ARCH[target.machine]}-apple-macos{macos_minimum()}"]
        # The font probe needs test-support. Reuse native-ui/e2e debug dependencies.
        commands = [
            ["cargo", "build", "--locked", *cross_flags, "--example", "tooltip-layout"],
            ["swiftc", *swift_target, "tests/desktop/window_ready.swift", "-o", str(tools / "window-ready")],
            ["swiftc", *swift_target, "-warnings-as-errors", "tests/desktop/Driver.swift", "-o", "target/e2e-tools/native-driver"],
        ]
        (ROOT / "target/e2e-tools").mkdir(parents=True, exist_ok=True)
        for command in commands:
            if runner.execute(command, env, 40 * 60, BASE / "tools.log", runner.Output(False)) != 0:
                raise ValueError("Native adapter preparation failed")
        output = ROOT / "target" / (target.triple if cross else "") / "debug/examples/tooltip-layout"
        shutil.copy2(output, tools / "tooltip-layout")
    manifest_path = PREPARED / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    assets = asset_paths(target)
    if not assets:
        raise ValueError("Missing native candidate")
    manifest["assets"] = {str(path.relative_to(ROOT)): builds.sha256(path) for path in assets}
    contract.validate_plan(value, ROOT, build=True)
    contract.atomic_json(manifest_path, manifest)
    # One tar fixes the transfer layout and preserves executable permissions.
    with tarfile.open(ROOT / "target/native-transfer.tar", "w") as bundle:
        for name in ("target/platform/plan.json", "target/platform/builds", *manifest["assets"]):
            # Candidate archives have their own artifact and are not duplicated.
            if not name.startswith("target/package/"):
                bundle.add(ROOT / name, arcname=name)


def unpack():
    with tarfile.open(ROOT / "target/native-transfer.tar") as bundle:
        for member in bundle.getmembers():
            path = Path(member.name)
            if (path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != "target" or
                    not (member.isfile() or member.isdir())):
                raise ValueError("Unsafe prepared transfer member")
        bundle.extractall(ROOT, filter="data")


def verify_assets():
    value = json.loads((BASE / "plan.json").read_text())
    contract.validate_plan(value, ROOT)
    manifest = json.loads((PREPARED / "manifest.json").read_text())
    target = host_target()
    expected = {str(path.relative_to(ROOT)) for path in asset_paths(target)}
    if set(manifest.get("assets", {})) != expected or not expected:
        raise ValueError("Missing or unexpected native build assets")
    for name, digest in manifest["assets"].items():
        path = ROOT / name
        if builds.sha256(path) != digest:
            raise ValueError("Native asset digest mismatch: " + name)
        if name.startswith(("target/native-tools/", "target/e2e-tools/")):
            path.chmod(0o755)
    return value


def test(full):
    unpack()
    value = verify_assets()
    shutil.copyfile(PREPARED / "manifest.json", BASE / "build-manifest.json")
    result = contract.execute(value, ROOT, runner.Output(False), PREPARED / "manifest.json")
    contract.atomic_json(BASE / "result.json", result)
    cli.print_summary(result)
    if result["exit_code"]:
        raise ValueError("Native tests failed")
    if full and host_target().name == "macos-arm64":
        checked(["run", "perf-ui", "perf-e2e", "perf-package", "--runtime", "native", "--report-only",
                 "--reuse-builds", str(PREPARED / "manifest.json")])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("build", "test"))
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--target", choices=sorted(name for name, item in TARGETS.items() if item.supported),
                        help="Build for this target. The default is this host.")
    args = parser.parse_args()
    try:
        if args.command == "build":
            build(args.full, args.target)
        else:
            if args.target and args.target != getattr(host_target(), "name", None):
                raise ValueError("Native tests run on their own target")
            test(args.full)
    except (ValueError, OSError) as error:
        sys.exit(str(error))

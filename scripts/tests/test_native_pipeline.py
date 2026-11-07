"""Native preparation cannot qualify stale outputs or changed transfer assets."""
import io
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/ci"))
sys.path.insert(0, str(ROOT / "scripts/qtest"))
import builds  # noqa: E402
import contract  # noqa: E402
import native  # noqa: E402
from targets import TARGETS, host_target  # noqa: E402


class NativePipelineTests(unittest.TestCase):
    def test_timed_out_adapter_cannot_seal_an_old_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for code in (None, 1):
                with self.subTest(code=code), patch.object(native, "ROOT", root), \
                        patch.object(native, "BASE", root / "target/platform"), \
                        patch.object(native, "PREPARED", root / "target/platform/builds"), \
                        patch.object(native, "host_target", return_value=TARGETS["macos-arm64"]), \
                        patch.object(native.contract, "plan", return_value={"source_sha": "a" * 40}), \
                        patch.object(native.builds, "prepare"), patch.object(native.cli, "install_tools", return_value=0), \
                        patch.object(native, "checked"), patch.object(native.runner, "execute", return_value=code), \
                        patch.object(native.shutil, "copy2") as copy:
                    with self.assertRaisesRegex(ValueError, "adapter preparation failed"):
                        native.build(False)
                    copy.assert_not_called()
                    self.assertFalse((root / "target/platform/builds/manifest.json").exists())

    def build_calls(self, host, target_name):
        """Run a mocked native build. Returns the qtest arguments, adapter commands, and CARGO_BUILD_TARGET values."""
        calls = {"checked": [], "commands": [], "cargo_target": []}

        def checked(args):
            calls["checked"].append(args)
            calls["cargo_target"].append(os.environ.get("CARGO_BUILD_TARGET"))

        def execute(command, *args):
            calls["commands"].append(command)
            return 0

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = root / "target/platform"
            contract.atomic_json(base / "builds/manifest.json", {})
            with patch.object(native, "ROOT", root), patch.object(native, "BASE", base), \
                    patch.object(native, "PREPARED", base / "builds"), \
                    patch.object(native, "host_target", return_value=TARGETS[host]), \
                    patch.object(native.contract, "plan", return_value={"source_sha": "a" * 40}), \
                    patch.object(native.contract, "validate_plan"), \
                    patch.object(native.builds, "prepare"), patch.object(native.cli, "install_tools", return_value=0), \
                    patch.object(native, "checked", side_effect=checked), \
                    patch.object(native.runner, "execute", side_effect=execute), \
                    patch.object(native.shutil, "copy2") as copy, \
                    patch.object(native, "asset_paths", return_value=[]), \
                    patch.dict(os.environ, {}, clear=False):
                os.environ.pop("CARGO_BUILD_TARGET", None)
                with self.assertRaisesRegex(ValueError, "Missing native candidate"):
                    native.build(False, target_name)
                self.assertNotIn("CARGO_BUILD_TARGET", os.environ)
                calls["copied"] = [call.args[0].relative_to(root) for call in copy.call_args_list]
        return calls

    def test_intel_macos_builds_on_apple_silicon_without_clippy(self):
        calls = self.build_calls("macos-arm64", "macos-x86_64")
        self.assertEqual(calls["checked"], [["run", "package", "--fail-fast"]])
        self.assertEqual(calls["cargo_target"], ["x86_64-apple-darwin"])
        cargo, observer, driver = calls["commands"]
        self.assertEqual(cargo[:5], ["cargo", "build", "--locked", "--target", "x86_64-apple-darwin"])
        for command in (observer, driver):
            self.assertEqual(command[1:3], ["-target", "x86_64-apple-macos" + native.macos_minimum()])
        self.assertEqual(calls["copied"], [Path("target/x86_64-apple-darwin/debug/examples/tooltip-layout")])

    def test_native_builds_lint_one_architecture_per_os(self):
        for host, linted in (("macos-arm64", True), ("linux-x86_64", True), ("linux-arm64", False)):
            with self.subTest(host=host):
                calls = self.build_calls(host, host)
                self.assertEqual("clippy-native" in calls["checked"][0], linted)
                self.assertEqual(calls["cargo_target"], [None])
        calls = self.build_calls("macos-arm64", "macos-arm64")
        self.assertEqual(calls["commands"][0][:4], ["cargo", "build", "--locked", "--example"])
        self.assertEqual(calls["copied"], [Path("target/debug/examples/tooltip-layout")])
        self.assertEqual({target.system for target in TARGETS.values() if target.name in native.platforms.LINT_TARGETS},
                         {"macos", "linux"})

    def test_cross_builds_stay_within_one_os(self):
        with patch.object(native, "host_target", return_value=TARGETS["linux-x86_64"]):
            with self.assertRaisesRegex(ValueError, "Cannot build macos-x86_64 on linux-x86_64"):
                native.build(False, "macos-x86_64")

    def test_changed_candidate_fails_before_test_execution(self):
        target = host_target()
        if target is None or target.system != "linux":
            self.skipTest("Synthetic Linux candidate")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / "target/package/candidate.tar.gz"
            candidate.parent.mkdir(parents=True)
            candidate.write_bytes(b"synthetic candidate")
            value = contract.plan(root, target.name, native.platforms.suites(target), "docker")
            base = root / "target/platform"
            contract.atomic_json(base / "plan.json", value)
            manifest = {"assets": {"target/package/candidate.tar.gz": builds.sha256(candidate)}}
            contract.atomic_json(base / "builds/manifest.json", manifest)
            with patch.object(native, "ROOT", root), patch.object(native, "BASE", base), \
                    patch.object(native, "PREPARED", base / "builds"):
                native.verify_assets()
                candidate.write_bytes(b"changed candidate")
                with self.assertRaisesRegex(ValueError, "digest mismatch"):
                    native.verify_assets()

    def test_prepared_transfer_rejects_paths_outside_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target").mkdir()
            for name in ("../outside", "src/main.rs", "/tmp/outside"):
                with tarfile.open(root / "target/native-transfer.tar", "w") as bundle:
                    member = tarfile.TarInfo(name)
                    member.size = 4
                    bundle.addfile(member, io.BytesIO(b"fake"))
                with self.subTest(name=name), patch.object(native, "ROOT", root):
                    with self.assertRaisesRegex(ValueError, "Unsafe"):
                        native.unpack()
                self.assertFalse((root / "src/main.rs").exists())

    def test_fixture_stops_before_independent_connector_adapters(self):
        import catalog
        import runner
        from unittest.mock import MagicMock
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, CARGO_TARGET_DIR=directory):
            fixture = MagicMock()
            fixture.env.return_value = {}
            marker = Path(directory) / "order.json"
            def stop():
                marker.write_text(json.dumps({"stopped": True}))
            fixture.stop.side_effect = stop
            first = catalog.Suite("served", "Synthetic shared fixture.", (), (), fixture="any")
            second = catalog.Suite("independent", "Check fixture release.", (),
                                   (catalog.Step((sys.executable, "-c", 'from pathlib import Path; assert Path(' + repr(str(marker)) + ').exists()')),))
            with patch.object(runner.fixture, "reusable", return_value=None), \
                    patch.object(runner.fixture, "start", return_value=fixture), patch.object(runner.fixture, "save"):
                result = runner.run([runner.Selected(first), runner.Selected(second)], {"runtime": "native"}, [], runner.Output(True))
            self.assertEqual(result["status"], "passed", result)
            fixture.stop.assert_called_once()


if __name__ == "__main__":
    unittest.main()

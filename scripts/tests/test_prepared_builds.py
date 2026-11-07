"""Prepared nextest execution must preserve filters and work without Cargo builds."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/qtest"))
import builds  # noqa: E402
import contract  # noqa: E402
import runner  # noqa: E402
import targets  # noqa: E402


class VariantTests(unittest.TestCase):
    def test_flag_order_unifies_variants_and_filters_are_intersected(self):
        first = ["cargo", "nextest", "run", "--features", "ui-test", "--cargo-profile", "perf", "--test", "e2e",
                 "--run-ignored", "only", "--profile", "ci", "-E", "test(/^postgres::/)"]
        second = ["cargo", "nextest", "run", "--cargo-profile", "perf", "--features", "ui-test", "--lib"]
        flags, selection, runtime = builds.split(first)
        self.assertEqual(flags, builds.split(second)[0])
        self.assertEqual(selection, ["--test", "e2e"])
        self.assertIn("(binary(=e2e)) & (test(/^postgres::/))", runtime)
        self.assertEqual(runtime.count("-E"), 1)
        self.assertIn("--run-ignored", runtime)
        self.assertIn("ci", runtime)

    def test_backend_and_connector_suites_share_one_no_ui_archive(self):
        value = contract.plan(ROOT, targets.host_target().name, ["backend", "postgres-core", "trino-core"], "docker")
        variants = builds.variants(value)
        self.assertEqual(len(variants), 1)
        group = next(iter(variants.values()))
        self.assertEqual(group["flags"], ["--no-default-features"])
        self.assertEqual(group["targets"], [["--test", "integration"]])
        self.assertFalse(group.get("all"))

    def test_no_prepared_match_fails_without_fallback(self):
        with self.assertRaisesRegex(ValueError, "No prepared build"):
            builds.reuse(["cargo", "nextest", "run", "--no-default-features"], {})
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            builds.split(["cargo", "nextest", "run", "--manifest-path", "elsewhere/Cargo.toml"])

    def test_fixture_layer_selection_preserves_nextest_commands(self):
        for engine in ("postgres", "trino"):
            spec = importlib.util.spec_from_file_location("adapter_" + engine, ROOT / f"scripts/e2e/{engine}.py")
            adapter = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(adapter)
            core = ("cargo", "nextest", "run", "--no-default-features", "--test", "integration")
            ui = ("cargo", "nextest", "run", "--test", "e2e")
            for layer, expected in (("core", [list(core)]), ("ui", [list(ui)]), ("all", [list(core), list(ui)])):
                with self.subTest(engine=engine, layer=layer), patch.dict(os.environ, QROW_CONNECTOR_LAYER=layer), \
                        patch.object(builds, "run") as run:
                    adapter.command(*core)
                    adapter.command(*ui)
                    self.assertEqual([call.args[0] for call in run.call_args_list], expected)


class RealPreparedExecutionTests(unittest.TestCase):
    def test_archive_runs_on_relocated_runner_with_no_build_and_rejects_corruption(self):
        if not shutil.which("cargo") or subprocess.run(["cargo", "nextest", "--version"], capture_output=True).returncode:
            self.skipTest("cargo-nextest is not installed")
        target = targets.host_target()
        if target is None:
            self.skipTest("No native target")
        with tempfile.TemporaryDirectory(prefix="qrow-prepared-test-") as directory:
            base = Path(directory)
            source = base / "source"
            source.mkdir()
            (source / "Cargo.toml").write_text('[package]\nname="qrow-prepared-probe"\nversion="0.1.0"\nedition="2021"\n')
            (source / "src").mkdir()
            (source / "src/lib.rs").write_text('pub fn value() -> usize { 42 }\n')
            (source / "tests").mkdir()
            (source / "tests/integration.rs").write_text('''
#[test] fn selected() { assert_eq!(qrow_prepared_probe::value(), 42); }
#[test] fn excluded() { panic!("binary and test filters must both apply"); }
''')
            (source / ".gitignore").write_text('/target/\n')
            # A hook exports GIT_DIR and GIT_INDEX_FILE. Keep these commands in the synthetic repository.
            git_env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
            subprocess.run(["git", "init", "-q", source], check=True, env=git_env)
            subprocess.run(["cargo", "generate-lockfile"], cwd=source, check=True, capture_output=True)
            subprocess.run(["git", "add", "."], cwd=source, check=True, env=git_env)
            subprocess.run(["git", "-c", "user.name=Synthetic", "-c", "user.email=synthetic@example.invalid",
                            "commit", "-qm", "Synthetic test input"], cwd=source, check=True, env=git_env)
            environment = dict(os.environ, CARGO_TARGET_DIR=str(source / "target"))
            with patch.dict(os.environ, environment), patch.object(runner, "ROOT", source):
                value = contract.plan(source, target.name, ["backend"], "docker")
                builds.prepare(value, source, source / "target/prepared", runner.Output(True))
            relocated = base / "relocated"
            shutil.copytree(source, relocated, ignore=shutil.ignore_patterns("target"))
            shutil.copytree(source / "target/prepared", relocated / "target/prepared")
            tools = base / "tools"
            tools.mkdir()
            cargo = shutil.which("cargo")
            guard = tools / "cargo"
            guard.write_text('#!' + sys.executable + '\nimport os,sys\n' +
                             'args=sys.argv[1:]\n' +
                             'assert args[0] == "nextest" and ("--archive-file" in args or "--binaries-metadata" in args), args\n' +
                             'os.execv(' + repr(cargo) + ', [' + repr(cargo) + ', *args])\n')
            guard.chmod(0o755)
            environment.update(PATH=str(tools) + os.pathsep + os.environ["PATH"], CARGO_TARGET_DIR=str(relocated / "target"))
            with patch.dict(os.environ, environment), patch.object(runner, "ROOT", relocated):
                path = relocated / "target/prepared/manifest.json"
                prepared = builds.Prepared(path, value, relocated, relocated / "target/extracted", runner.Output(True))
                command = prepared.command(["cargo", "nextest", "run", "--locked", "--no-default-features", "--test", "integration", "-E", "test(=selected)"])
                result = subprocess.run(command, cwd=relocated, env=environment, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("1 test run", result.stderr)
                manifest = json.loads(path.read_text())
                archive = path.parent / next(iter(manifest["variants"].values()))["archive"]
                archive.write_bytes(b"corrupted synthetic archive")
                with self.assertRaisesRegex(ValueError, "digest"):
                    builds.Prepared(path, value, relocated, relocated / "target/corrupted", runner.Output(True))
                changed = dict(value, snapshot="changed")
                with self.assertRaisesRegex(ValueError, "differ from source"):
                    builds.Prepared(path, changed, relocated, relocated / "target/changed", runner.Output(True))


if __name__ == "__main__":
    unittest.main()

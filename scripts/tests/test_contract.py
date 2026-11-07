"""Source-bound plans, native target policy, and real launcher execution."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/qtest'))
import catalog  # noqa: E402
import contract  # noqa: E402
import targets  # noqa: E402
from process import lease  # noqa: E402


class ContractTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'source.txt').write_text('synthetic')

    def plan(self, selectors=None, target='linux-x86_64'):
        return contract.plan(self.root, target, selectors or ['fmt'])

    def test_targets_resolve_independently_of_host(self):
        linux = self.plan(['unit'])
        mac = self.plan(['unit'], 'macos-arm64')
        self.assertIn('ui-test', linux['suites'][0]['definition']['steps'][0]['command'])
        self.assertNotIn('ui-test', mac['suites'][0]['definition']['steps'][0]['command'])
        for target in targets.TARGETS.values():
            if target.supported:
                self.assertEqual(self.plan(target=target.name)['target'], target.name)
        with self.assertRaisesRegex(ValueError, 'planned'):
            self.plan(target='windows-x86_64')

    def test_plan_roundtrip_and_source_change(self):
        value = json.loads(json.dumps(self.plan()))
        with patch.object(targets, 'host_target', return_value=targets.TARGETS['linux-x86_64']):
            contract.validate_plan(value, self.root)
            (self.root / 'source.txt').write_text('changed')
            with self.assertRaisesRegex(ValueError, 'snapshot'):
                contract.validate_plan(value, self.root)

    def test_nested_export_snapshot_is_not_parent_git_tree(self):
        subprocess.run(['git', 'init', '-q', self.root], check=True)
        export = self.root / 'export'
        export.mkdir()
        (export / 'input').write_text('before')
        before = contract.snapshot(export)
        (export / 'input').write_text('after')
        self.assertNotEqual(before, contract.snapshot(export))

    def test_unchanged_plan_moves_between_checkout_paths(self):
        (self.root / '.cargo').mkdir()
        (self.root / '.cargo/config.toml').write_text('[build]\njobs = 2\n')
        value = self.plan()
        with tempfile.TemporaryDirectory() as directory:
            other = Path(directory)
            (other / 'source.txt').write_text('synthetic')
            (other / '.cargo').mkdir()
            (other / '.cargo/config.toml').write_text('[build]\njobs = 2\n')
            with patch.object(targets, 'host_target', return_value=targets.TARGETS['linux-x86_64']):
                contract.validate_plan(value, other)

    def test_wrong_host_and_configuration_fail(self):
        value = self.plan()
        with patch.object(targets, 'host_target', return_value=targets.TARGETS['macos-arm64']):
            with self.assertRaisesRegex(ValueError, 'Native execution'):
                contract.validate_plan(value, self.root)
        with patch.dict(os.environ, RUSTFLAGS='--cfg synthetic'):
            with self.assertRaisesRegex(ValueError, 'configuration'):
                contract.validate_plan(value, self.root)

    def test_conflicting_cargo_target_is_rejected(self):
        with patch.dict(os.environ, CARGO_BUILD_TARGET='aarch64-unknown-linux-gnu'):
            value = self.plan()
            with patch.object(targets, 'host_target', return_value=targets.TARGETS['linux-x86_64']):
                with self.assertRaisesRegex(ValueError, 'conflicts'):
                    contract.validate_plan(value, self.root)

    def test_evidence_rejects_missing_duplicate_empty_and_failed_tests(self):
        value = self.plan(['ui'])
        result = dict(schema=1, plan_id=value['id'], target=value['target'], snapshot=value['snapshot'], source_sha=value['source_sha'],
                      status='passed', exit_code=0, suites=[dict(name='ui', status='passed', tests=[dict(count=1, failures=0)])])
        contract.verify(value, result)
        altered = copy.deepcopy(value)
        altered['suites'] = []
        with self.assertRaisesRegex(ValueError, 'identity'):
            contract.verify(altered, result)
        for mutation in ('empty', 'duplicate', 'no-tests', 'failed', 'cancelled', 'stale'):
            bad = copy.deepcopy(result)
            if mutation == 'empty':
                bad['suites'] = []
            elif mutation == 'duplicate':
                bad['suites'] *= 2
            elif mutation == 'no-tests':
                bad['suites'][0]['tests'][0]['count'] = 0
            elif mutation == 'failed':
                bad['suites'][0]['tests'][0]['failures'] = 1
            elif mutation == 'cancelled':
                bad['status'] = 'cancelled'
            else:
                bad['plan_id'] = 'stale'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                contract.verify(value, bad)

    def test_real_portable_launcher_plan_execute_verify(self):
        target = targets.host_target()
        if target is None or not target.supported:
            self.skipTest('No supported native target')
        # An existing portable policy check exercises subprocess execution without a Rust build.
        environment = dict(os.environ, CARGO_TARGET_DIR=str(self.root / 'output'))
        plan = self.root / 'plan.json'
        result = self.root / 'result.json'
        for arguments in (['plan', 'policy', '--output', str(plan)],
                          ['execute', str(plan), '--output', str(result)],
                          ['verify', str(plan), str(result)]):
            completed = subprocess.run([sys.executable, str(ROOT / 'scripts/qtest'), *arguments],
                                       cwd=ROOT, env=environment, capture_output=True, text=True)
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        self.assertEqual(json.loads(result.read_text())['suites'][0]['status'], 'passed')

    def test_resource_lease_blocks_another_process_then_releases(self):
        resource = 'synthetic-' + self.root.name
        script = 'import sys; from process import lease\nwith lease([sys.argv[1]], timeout=.1): pass'
        env = dict(os.environ, PYTHONPATH=str(ROOT / 'scripts/qtest'))
        with lease([resource]):
            blocked = subprocess.run([sys.executable, '-c', script, resource], env=env, capture_output=True)
            self.assertNotEqual(blocked.returncode, 0)
        subprocess.run([sys.executable, '-c', script, resource], env=env, check=True, capture_output=True)

    def test_import_host_view_does_not_affect_explicit_definitions(self):
        with patch.object(catalog, 'HOST', 'windows'):
            self.assertEqual(self.plan()['suites'][0]['name'], 'fmt')


class FinalizerTests(unittest.TestCase):
    def test_cancelled_fixture_start_cleans_prepared_artifacts(self):
        import runner
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, CARGO_TARGET_DIR=directory):
            marker = Path(directory) / 'cleaned'
            cleanup = catalog.Step((sys.executable, '-c',
                                    'import os,pathlib; pathlib.Path(' + repr(str(marker)) +
                                    ').write_text(os.environ["QROW_E2E_ARTIFACTS"])'))
            suite = catalog.Suite('synthetic', 'Synthetic fixture.', (), (), fixture='any',
                                  prepare=(catalog.Step((sys.executable, '-c', 'pass')),), cleanup=(cleanup,))
            with patch.object(runner.fixture, 'reusable', return_value=None), \
                    patch.object(runner.fixture, 'start', side_effect=KeyboardInterrupt):
                result = runner.run([runner.Selected(suite)], {'runtime': 'native'}, [], runner.Output(True))
            self.assertEqual(result['status'], 'cancelled')
            self.assertEqual(marker.read_text(), str(Path(result['artifacts']) / 'synthetic'))
            self.assertTrue((Path(result['artifacts']) / 'events.jsonl').exists())

    def test_missing_preflight_does_not_run_cleanup(self):
        import runner
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, CARGO_TARGET_DIR=directory):
            marker = Path(directory) / 'unexpected'
            suite = catalog.Suite('synthetic', 'Synthetic missing tool.', ('cargo',), (),
                                  cleanup=(catalog.Step((sys.executable, '-c',
                                           'from pathlib import Path; Path(' + repr(str(marker)) + ').touch()')),))
            with patch.object(runner, 'check_requirement', return_value='Synthetic missing tool'):
                result = runner.run([runner.Selected(suite)], {}, [], runner.Output(True), require_all=True)
            self.assertEqual(result['status'], 'missing')
            self.assertFalse(marker.exists())


class BuildIdentityTests(unittest.TestCase):
    def test_child_builds_receive_one_commit_without_mutating_parent_environment(self):
        import runner
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, CARGO_TARGET_DIR=directory):
            os.environ.pop('QROW_COMMIT', None)
            suite = catalog.Suite('identity', 'Synthetic build identity.', (),
                                  (catalog.Step((sys.executable, '-c', 'import os; print(os.environ["QROW_COMMIT"])')),))
            with patch.object(contract, 'source_sha', return_value='a' * 40):
                result = runner.run([runner.Selected(suite)], {}, [], runner.Output(True))
            self.assertEqual(result['exit_code'], 0)
            self.assertIn('a' * 12, Path(result['suites'][0]['log']).read_text())
            self.assertNotIn('QROW_COMMIT', os.environ)

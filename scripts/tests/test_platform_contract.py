"""Reject incomplete four-target validation and unsafe package contents."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/ci'))
sys.path.insert(0, str(ROOT / 'scripts/qtest'))
import contract  # noqa: E402
import platforms  # noqa: E402

spec = importlib.util.spec_from_file_location('qualify', ROOT / 'scripts/package/qualify.py')
qualify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualify)


class MatrixGateTests(unittest.TestCase):
    def test_matrix_has_all_supported_native_targets_and_no_windows_claim(self):
        matrix = platforms.matrix()['include']
        self.assertEqual({item['target'] for item in matrix},
                         {'linux-x86_64', 'linux-arm64', 'macos-arm64', 'macos-x86_64'})
        self.assertTrue(all(item['runner'] and 'package-smoke' in item['suites'] for item in matrix))
        self.assertTrue(all('e2e' in item['suites'] for item in platforms.matrix(True)['include']))

    def evidence(self, root, full=False):
        sha = 'a' * 40
        for target in platforms.TARGETS.values():
            if not target.supported:
                continue
            directory = root / target.name
            directory.mkdir()
            runtime = 'docker' if target.system == 'linux' else 'native'
            value = contract.plan(root, target.name, platforms.suites(target, full), runtime)
            value['source_sha'] = sha
            value['id'] = contract.digest({key: item for key, item in value.items() if key != 'id'})
            result = dict(schema=1, plan_id=value['id'], target=target.name, snapshot=value['snapshot'],
                          source_sha=sha, status='passed', exit_code=0, suites=[])
            for item in value['suites']:
                evidence = dict(name=item['name'], status='passed')
                if any(step['nextest_filter'] is not None for step in item['definition']['steps']):
                    evidence['tests'] = [dict(count=1, failures=0)]
                result['suites'].append(evidence)
            receipt = dict(schema=1, target=target.name, source_sha=sha, sha256='b' * 64,
                           qualification='native-package-smoke')
            for name, data in [('plan', value), ('result', result), ('receipt', receipt)]:
                (directory / f'{name}.json').write_text(json.dumps(data))
        return sha

    def test_complete_basic_and_full_matrices_pass_and_partial_matrix_fails(self):
        for full in (False, True):
            with self.subTest(full=full), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                sha = self.evidence(root, full)
                platforms.verify(root, sha, full)
                (root / 'linux-arm64/result.json').unlink()
                with self.assertRaisesRegex(ValueError, 'Missing'):
                    platforms.verify(root, sha, full)

    def test_stale_and_cancelled_results_fail_even_with_passing_other_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.evidence(root)
            path = root / 'macos-arm64/result.json'
            original = json.loads(path.read_text())
            for update in ({'source_sha': 'c' * 40}, {'status': 'cancelled'}, {'suites': []}):
                changed = copy.deepcopy(original)
                changed.update(update)
                path.write_text(json.dumps(changed))
                with self.subTest(update=update), self.assertRaises(ValueError):
                    platforms.verify(root, sha)


class PackageInspectionTests(unittest.TestCase):
    def archive(self, root, machine=62, extra=None):
        archive = root / 'candidate.tar.gz'
        files = ['bin/qrow', 'share/applications/qrow.desktop', 'share/icons/hicolor/256x256/apps/qrow.png',
                 'share/licenses/qrow/LICENSE', 'share/licenses/qrow/NOTICE', 'share/licenses/qrow/THIRD_PARTY_NOTICES.txt']
        with tarfile.open(archive, 'w:gz') as bundle:
            for name in files + ([extra] if extra else []):
                contents = bytearray(32) if name == 'bin/qrow' else bytearray(b'synthetic')
                if name == 'bin/qrow':
                    contents[:6] = b'\x7fELF\x02\x01'
                    contents[18:20] = machine.to_bytes(2, 'little')
                member = tarfile.TarInfo('./' + name)
                member.size = len(contents)
                member.mode = 0o755 if name == 'bin/qrow' else 0o644
                bundle.addfile(member, io.BytesIO(contents))
        return archive

    def test_valid_architecture_and_missing_libraries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = self.archive(root)
            with patch.object(qualify.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'libc => /lib/libc.so')):
                self.assertEqual(qualify.linux_archive(archive, root / 'out', 'x86_64').name, 'qrow')
            with patch.object(qualify.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'libc => not found')):
                with self.assertRaisesRegex(ValueError, 'Missing runtime'):
                    qualify.linux_archive(archive, root / 'missing', 'x86_64')

    def test_wrong_architecture_and_traversal_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, 'architecture'):
                qualify.linux_archive(self.archive(root, 183), root / 'wrong', 'x86_64')
            with self.assertRaisesRegex(ValueError, 'Unsafe'):
                qualify.linux_archive(self.archive(root, extra='../outside'), root / 'unsafe', 'x86_64')
            self.assertFalse((root / 'outside').exists())


class PublicationTests(unittest.TestCase):
    def run_publication(self, prerelease=False, corrupt=False):
        import os
        import shutil
        import textwrap
        if shutil.which('sha256sum') is None:
            self.skipTest('Publication runs on Linux with sha256sum')
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        step = workflow.split('      - name: publish-github-release', 1)[1]
        script = textwrap.dedent(step.split('        run: |\n', 1)[1])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dist = root / 'dist'
            dist.mkdir()
            for suffix in ['', '-arm64', '-x86_64']:
                (dist / f'Qrow-1.2.3{suffix}.dmg').write_bytes(b'synthetic dmg')
            for arch in ['x86_64', 'aarch64']:
                (dist / f'Qrow-1.2.3-linux-{arch}.tar.gz').write_bytes(b'synthetic archive')
            (root / 'release-notes.md').write_text('Synthetic release')
            tools = root / 'tools'
            tools.mkdir()
            gh = tools / 'gh'
            gh.write_text('#!' + sys.executable + '\n' + textwrap.dedent('''
                import json, os, pathlib, shutil, sys
                arguments = sys.argv[1:]
                with open('calls.jsonl', 'a') as stream:
                    stream.write(json.dumps(arguments) + '\\n')
                if arguments[:2] == ['release', 'download']:
                    target = pathlib.Path(arguments[arguments.index('--dir') + 1])
                    for source in pathlib.Path('dist').iterdir():
                        shutil.copyfile(source, target / source.name)
                    if os.environ.get('CORRUPT') == 'true':
                        (target / 'Qrow-1.2.3-arm64.dmg').write_text('corrupted')
                '''))
            gh.chmod(0o755)
            environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                               RELEASE_VERSION='1.2.3', RELEASE_TAG='v1.2.3', RELEASE_TITLE='Synthetic',
                               TARGET_SHA='a' * 40, PRERELEASE=str(prerelease).lower(),
                               GITHUB_REPOSITORY='synthetic/repository', GH_TOKEN='synthetic',
                               CORRUPT=str(corrupt).lower())
            result = subprocess.run(['bash', '-c', script], cwd=root, env=environment, capture_output=True)
            calls = [json.loads(line) for line in (root / 'calls.jsonl').read_text().splitlines()]
            return result, calls

    def test_stable_publication_checks_remote_bytes_before_publishing(self):
        result, calls = self.run_publication()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[:2] for call in calls], [['release', 'create'], ['release', 'download'], ['release', 'edit']])
        self.assertIn('--draft', calls[0])
        self.assertIn('--latest=false', calls[0])
        self.assertIn('--latest', calls[-1])

    def test_corrupt_upload_stays_unpublished(self):
        result, calls = self.run_publication(corrupt=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(['release', 'edit'], [call[:2] for call in calls])

    def test_nightly_is_not_marked_latest(self):
        result, calls = self.run_publication(prerelease=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--prerelease', calls[0])
        self.assertIn('--latest=false', calls[-1])


class SmokeReadinessTests(unittest.TestCase):
    def test_constructor_log_cannot_qualify_a_missing_native_window(self):
        from unittest.mock import MagicMock
        process = MagicMock(pid=12345)
        process.poll.return_value = None
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory) / 'smoke.log'

            def start(*_args, **kwargs):
                kwargs['stdout'].write(b'Qrow GPUI initialized\n')
                kwargs['stdout'].flush()
                return process

            def run(command, **_kwargs):
                return subprocess.CompletedProcess(command, 0 if command[0] == 'swiftc' else 1)

            with patch.object(qualify.subprocess, 'Popen', side_effect=start), \
                    patch.object(qualify.subprocess, 'run', side_effect=run) as observer, \
                    patch.object(qualify.time, 'monotonic', side_effect=[0, 1, 61]), \
                    patch.object(qualify.time, 'sleep'), patch.object(qualify, 'stop_tree') as stop:
                with self.assertRaisesRegex(TimeoutError, 'native window'):
                    qualify.smoke(Path('/synthetic/qrow'), 'macos', evidence)
                self.assertEqual(observer.call_count, 2)
                stop.assert_called_once_with(process)

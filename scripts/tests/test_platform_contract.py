"""Reject incomplete native-target validation and unsafe package contents."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import os
import textwrap
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
    def test_matrix_has_all_supported_native_targets_including_windows(self):
        matrix = platforms.matrix()['include']
        self.assertEqual({item['target'] for item in matrix},
                         {'linux-x86_64', 'linux-arm64', 'macos-arm64', 'macos-x86_64', 'windows-x86_64', 'windows-arm64'})
        self.assertTrue(all(item['runner'] and 'package-smoke' in item['suites'] for item in matrix))
        self.assertEqual(platforms.matrix(), platforms.matrix(True))

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
                count = sum(step['nextest_filter'] is not None for step in item['definition']['steps']) + item['definition']['test_runs']
                if count:
                    evidence['tests'] = [dict(count=1, failures=0)] * count
                result['suites'].append(evidence)
            receipt = dict(schema=1, target=target.name, source_sha=sha, sha256='b' * 64,
                           qualification='native-package-smoke', archive='candidate.tar.gz')
            assets = {'target/package/candidate.tar.gz': 'b' * 64}
            if target.system == 'windows':
                receipt.update(portable='candidate.zip', portable_sha256='c' * 64)
                assets['target/package/candidate.zip'] = 'c' * 64
            import builds
            manifest = dict(identity=builds.identity(value), assets=assets)
            manifest_path = directory / 'build-manifest.json'
            contract.atomic_json(manifest_path, manifest)
            result['build_manifest_sha256'] = builds.sha256(manifest_path)
            if target.system == 'macos':
                reason = 'hardware OpenGL Core pixel format unavailable (10002)'
                hardware = dict(source_sha=sha, snapshot=value['snapshot'], target=target.name,
                                build_manifest_sha256=result['build_manifest_sha256'], status='passed', exit_code=0,
                                capability=dict(available=False, reason=reason),
                                suites=[dict(name=name, status='skipped', reason='Hardware OpenGL unavailable: ' + reason)
                                        for name in ('opengl-renderer', 'opengl-ui')])
                (directory / 'renderer-result.json').write_text(json.dumps(hardware))
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

    def test_macos_evidence_requires_the_optional_hardware_phase(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.evidence(root)
            path = root / 'macos-arm64/renderer-result.json'
            original = path.read_text()
            path.unlink()
            with self.assertRaises(FileNotFoundError):
                platforms.verify(root, sha)
            changed = json.loads(original)
            changed['build_manifest_sha256'] = 'different'
            path.write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError, 'Renderer prepared'):
                platforms.verify(root, sha)

    def test_windows_evidence_needs_the_digest_of_the_built_portable_archive(self):
        for change in ('missing', 'different'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                sha = self.evidence(root)
                path = root / 'windows-arm64/receipt.json'
                receipt = json.loads(path.read_text())
                if change == 'missing':
                    del receipt['portable_sha256']
                else:
                    receipt['portable_sha256'] = 'd' * 64
                path.write_text(json.dumps(receipt))
                with self.assertRaisesRegex(ValueError, 'Portable archive identity'):
                    platforms.verify(root, sha)

    def test_pull_request_evidence_needs_only_its_target_and_rejects_extra_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.evidence(root)
            # Evidence of other targets is unexpected in a pull request run.
            with self.assertRaisesRegex(ValueError, 'unexpected'):
                platforms.verify(root, sha, targets=platforms.native_targets('pr'))
            for name in set(platforms.native_targets('full')) - set(platforms.native_targets('pr')):
                for path in (root / name).iterdir():
                    path.unlink()
            platforms.verify(root, sha, targets=platforms.native_targets('pr'))
            with self.assertRaisesRegex(ValueError, 'Missing'):
                platforms.verify(root, sha)
            for targets in (['macos-arm64'], ['linux-x86_64', 'unknown-target']):
                with self.subTest(targets=targets), self.assertRaisesRegex(ValueError, 'selection'):
                    platforms.verify(root, sha, targets=targets)

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
    def archive(self, root, machine=62, extra=None, root_mode=0o755):
        archive = root / 'candidate.tar.gz'
        files = ['bin/qrow', 'share/applications/qrow.desktop', 'share/icons/hicolor/256x256/apps/qrow.png',
                 'share/licenses/qrow/LICENSE', 'share/licenses/qrow/NOTICE', 'share/licenses/qrow/THIRD_PARTY_NOTICES.txt']
        with tarfile.open(archive, 'w:gz') as bundle:
            directory = tarfile.TarInfo('./')
            directory.type = tarfile.DIRTYPE
            directory.mode = root_mode
            bundle.addfile(directory)
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

    def test_private_or_writable_archive_root_fails(self):
        # Extraction applies the root mode to the target folder, such as ~/.local.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for mode in (0o700, 0o777):
                with self.assertRaisesRegex(ValueError, 'Unsafe archive mode'):
                    qualify.linux_archive(self.archive(root, root_mode=mode), root / f'mode-{mode:o}', 'x86_64')


# A stand-in for minisign. It binds each signature to the key, the file bytes, and the trusted comment.
FAKE_MINISIGN = textwrap.dedent("""
    import hashlib, json, pathlib, sys
    arguments = sys.argv[1:]
    def option(name):
        return arguments[arguments.index(name) + 1]
    message = pathlib.Path(option('-m'))
    signature = pathlib.Path(str(message) + '.minisig')
    digest = hashlib.sha256(message.read_bytes()).hexdigest()
    if arguments[0] == '-S':
        key = hashlib.sha256(pathlib.Path(option('-s')).read_bytes().strip()).hexdigest()
        signature.write_text(json.dumps({'key': key, 'digest': digest, 'comment': option('-t')}))
    elif arguments[0] == '-V':
        public = pathlib.Path(option('-p')).read_text().splitlines()[1]
        signed = json.loads(signature.read_text())
        if signed['key'] != public or signed['digest'] != digest:
            sys.exit('Signature verification failed')
        print('Signature and comment signature verified')
        print('Trusted comment: ' + signed['comment'])
    else:
        sys.exit(2)
""")


def step_script(workflow, name):
    step = workflow.split(f'      - name: {name}\n', 1)[1].split('\n      - name: ', 1)[0]
    return textwrap.dedent(step.split('        run: |\n', 1)[1])


class PublicationTests(unittest.TestCase):
    def run_publication(self, prerelease=False, corrupt=False, key='matching', real_minisign=False):
        import hashlib
        import shutil
        if shutil.which('sha256sum') is None:
            self.skipTest('Publication runs on Linux with sha256sum')
        if real_minisign and (shutil.which('minisign') is None or
                              subprocess.run(['minisign', '-v'], capture_output=True).returncode != 0):
            self.skipTest('minisign is not installed')
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        scripts = [step_script(workflow, 'sign-checksums'), step_script(workflow, 'publish-github-release')]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dist = root / 'dist'
            dist.mkdir()
            for suffix in ['', '-arm64', '-x86_64']:
                (dist / f'Qrow-1.2.3{suffix}.dmg').write_bytes(b'synthetic dmg')
            for arch in ['x86_64', 'aarch64']:
                (dist / f'Qrow-1.2.3-linux-{arch}.tar.gz').write_bytes(b'synthetic archive')
            for arch in ['x86_64', 'aarch64']:
                (dist / f'Qrow-1.2.3-windows-{arch}-setup.exe').write_bytes(b'synthetic installer')
                (dist / f'Qrow-1.2.3-windows-{arch}.zip').write_bytes(b'synthetic portable archive')
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
                    if os.environ.get('CORRUPT') == 'signature':
                        (target / 'SHA256SUMS.minisig').write_text('{}')
                '''))
            gh.chmod(0o755)
            # Synthetic keys only: the release key never enters tests.
            public = root / 'assets/release/minisign.pub'
            public.parent.mkdir(parents=True)
            secrets = {}
            for name in ('matching', 'other'):
                if real_minisign:
                    subprocess.run(['minisign', '-G', '-W', '-p', root / f'{name}.pub', '-s', root / f'{name}.key'],
                                   check=True, capture_output=True)
                    secrets[name] = (root / f'{name}.key').read_text()
                    if name == 'matching':
                        shutil.copyfile(root / f'{name}.pub', public)
                else:
                    secrets[name] = f'synthetic {name} secret key'
                    if name == 'matching':
                        digest = hashlib.sha256(secrets[name].encode()).hexdigest()
                        public.write_text(f'untrusted comment: synthetic\n{digest}\n')
            if not real_minisign:
                minisign = tools / 'minisign'
                minisign.write_text('#!' + sys.executable + '\n' + FAKE_MINISIGN)
                minisign.chmod(0o755)
            runner_temp = root / 'runner-temp'
            runner_temp.mkdir()
            environment = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                               RELEASE_VERSION='1.2.3', RELEASE_TAG='v1.2.3', RELEASE_TITLE='Synthetic',
                               TARGET_SHA='a' * 40, PRERELEASE=str(prerelease).lower(),
                               GITHUB_REPOSITORY='synthetic/repository', GH_TOKEN='synthetic',
                               CORRUPT=str(corrupt).lower() if isinstance(corrupt, bool) else corrupt,
                               RELEASE_SIGNING_KEY=secrets.get(key, ''), RUNNER_TEMP=str(runner_temp))
            for script in scripts:
                result = subprocess.run(['bash', '-c', script], cwd=root, env=environment, capture_output=True)
                if result.returncode != 0:
                    break
            calls_file = root / 'calls.jsonl'
            calls = [json.loads(line) for line in calls_file.read_text().splitlines()] if calls_file.exists() else []
            self.assertEqual(list(runner_temp.iterdir()), [], 'the secret key file must not remain')
            self.signature = (dist / 'SHA256SUMS.minisig').read_text() if (dist / 'SHA256SUMS.minisig').exists() else None
            return result, calls

    def test_stable_publication_checks_remote_bytes_before_publishing(self):
        result, calls = self.run_publication()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[:2] for call in calls], [['release', 'create'], ['release', 'download'], ['release', 'edit']])
        self.assertIn('--draft', calls[0])
        self.assertIn('--latest=false', calls[0])
        self.assertIn('--latest', calls[-1])
        self.assertIn('dist/SHA256SUMS.minisig', calls[0])
        self.assertIn('"comment": "Qrow release v1.2.3"', self.signature)
        for arch in ('x86_64', 'aarch64'):
            self.assertIn(f'dist/Qrow-1.2.3-windows-{arch}.zip', calls[0])

    def test_corrupt_upload_stays_unpublished(self):
        result, calls = self.run_publication(corrupt=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(['release', 'edit'], [call[:2] for call in calls])

    def test_corrupt_signature_upload_stays_unpublished(self):
        result, calls = self.run_publication(corrupt='signature')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(['release', 'edit'], [call[:2] for call in calls])

    def test_missing_or_wrong_signing_key_creates_no_release(self):
        for key in ('missing', 'other'):
            with self.subTest(key=key):
                result, calls = self.run_publication(key=key)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])

    def test_nightly_is_not_marked_latest(self):
        result, calls = self.run_publication(prerelease=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--prerelease', calls[0])
        self.assertIn('--latest=false', calls[-1])

    def test_real_minisign_signs_and_rejects_a_wrong_key(self):
        result, calls = self.run_publication(real_minisign=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('trusted comment: Qrow release v1.2.3', self.signature)
        result, calls = self.run_publication(real_minisign=True, key='other')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, [])


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

class CommonGateTests(unittest.TestCase):
    def evidence(self, root, full=False):
        import builds
        source = root / 'source'
        source.mkdir()
        sha = 'a' * 40
        for name, selectors in platforms.COMMON.items():
            directory = root / name
            directory.mkdir()
            value = contract.plan(source, platforms.COMMON_TARGET, selectors, 'docker')
            value['source_sha'] = sha
            value['id'] = contract.digest({key: item for key, item in value.items() if key != 'id'})
            result = dict(schema=1, plan_id=value['id'], target=value['target'], snapshot=value['snapshot'],
                          source_sha=sha, status='passed', exit_code=0, suites=[])
            for item in value['suites']:
                count = sum(step['nextest_filter'] is not None for step in item['definition']['steps']) + item['definition']['test_runs']
                result['suites'].append(dict(name=item['name'], status='passed', tests=[dict(count=1, failures=0)] * count))
            if name in platforms.PREPARED:
                manifest_path = directory / 'build-manifest.json'
                contract.atomic_json(manifest_path, dict(identity=builds.identity(value)))
                result['build_manifest_sha256'] = builds.sha256(manifest_path)
            contract.atomic_json(directory / 'plan.json', value)
            contract.atomic_json(directory / 'result.json', result)
        return sha

    def test_complete_common_graph_passes_and_missing_or_changed_evidence_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.evidence(root)
            platforms.verify_common(root, sha)
            with self.assertRaises(FileNotFoundError):
                platforms.verify_common(root, sha, full=True)
            path = root / 'backend/build-manifest.json'
            original = path.read_text()
            path.write_text('{}')
            with self.assertRaisesRegex(ValueError, 'build identity'):
                platforms.verify_common(root, sha)
            path.write_text(original)
            (root / 'backend/result.json').unlink()
            with self.assertRaisesRegex(ValueError, 'Missing common'):
                platforms.verify_common(root, sha)

    def test_static_level_needs_only_static_evidence_and_rejects_other_jobs(self):
        import shutil
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.evidence(root)
            with self.assertRaisesRegex(ValueError, 'unexpected common result'):
                platforms.verify_common(root, sha, jobs=['static'])
            for name in platforms.COMMON:
                if name != 'static':
                    shutil.rmtree(root / name)
            platforms.verify_common(root, sha, jobs=['static'])
            with self.assertRaisesRegex(ValueError, 'Missing common'):
                platforms.verify_common(root, sha)
            for jobs in ([], ['core'], ['static', 'unknown']):
                with self.subTest(jobs=jobs), self.assertRaisesRegex(ValueError, 'selection'):
                    platforms.verify_common(root, sha, jobs=jobs)

    def test_full_common_gate_requires_stress_from_the_same_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sha = self.evidence(root)
            ui = root / 'headless-ui'
            functional = json.loads((ui / 'result.json').read_text())
            value = contract.plan(root / 'source', platforms.COMMON_TARGET, ['ui/timing'], 'docker', 100)
            value['source_sha'] = sha
            value['id'] = contract.digest({key: item for key, item in value.items() if key != 'id'})
            result = dict(schema=1, plan_id=value['id'], target=value['target'], snapshot=value['snapshot'],
                          source_sha=sha, status='passed', exit_code=0,
                          build_manifest_sha256=functional['build_manifest_sha256'],
                          suites=[dict(name='ui', status='passed', tests=[dict(count=3, failures=0)] * 100)])
            contract.atomic_json(ui / 'stress-plan.json', value)
            contract.atomic_json(ui / 'stress-result.json', result)
            platforms.verify_common(root, sha, full=True)
            result['build_manifest_sha256'] = 'changed'
            contract.atomic_json(ui / 'stress-result.json', result)
            with self.assertRaisesRegex(ValueError, 'stress contract'):
                platforms.verify_common(root, sha, full=True)


class WindowsInstallerTests(unittest.TestCase):
    """A synthetic installer and uninstaller stand in for Inno Setup."""

    def installer(self, root, machine=0x8664, extra=None, uninstall=True):
        executable = bytearray(128)
        executable[:2] = b'MZ'
        executable[60:64] = (64).to_bytes(4, 'little')
        executable[64:68] = b'PE\0\0'
        executable[68:70] = machine.to_bytes(2, 'little')
        (root / 'payload.bin').write_bytes(executable)
        registry = root / 'registered'
        uninstaller = textwrap.dedent(f'''
            import pathlib, shutil
            pathlib.Path({str(registry)!r}).unlink()
            {"shutil.rmtree(pathlib.Path(__file__).parent)" if uninstall else ""}
        ''')
        script = root / 'setup.exe'
        script.write_text('#!' + sys.executable + '\n' + textwrap.dedent(f'''
            import pathlib, shutil, sys
            app = pathlib.Path(next(item[5:] for item in sys.argv if item.startswith('/DIR=')))
            app.mkdir(parents=True)
            shutil.copyfile({str(root / 'payload.bin')!r}, app / 'qrow.exe')
            for name in ('LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.txt', 'unins000.dat', *{[extra] if extra else []!r}):
                (app / name).write_text('synthetic')
            (app / 'unins000.exe').write_text('#!' + sys.executable + '\\n' + {uninstaller!r})
            (app / 'unins000.exe').chmod(0o755)
            pathlib.Path({str(registry)!r}).write_text('synthetic')
        '''))
        script.chmod(0o755)
        return script, registry.exists

    @unittest.skipIf(os.name == 'nt', 'The synthetic installer is a script')
    def test_install_checks_pe_architecture_and_closed_contents_then_uninstalls(self):
        cases = [({}, 'x86_64', None), ({'machine': 0xaa64}, 'aarch64', None),
                 ({'machine': 0xaa64}, 'x86_64', 'architecture'), ({'extra': 'extra.dll'}, 'x86_64', 'Unexpected'),
                 ({'uninstall': False}, 'x86_64', 'did not remove')]
        for options, machine, error in cases:
            with self.subTest(options=options, machine=machine), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                installer, installed = self.installer(root, **options)
                (root / 'install').mkdir()
                context = qualify.windows_installer(installer, root / 'install', machine, installed, timeout=1)
                if error:
                    with self.assertRaisesRegex(ValueError, error), context:
                        pass
                else:
                    with context as executable:
                        self.assertEqual(executable, root / 'install/Qrow/qrow.exe')
                        self.assertTrue(installed())
                    self.assertFalse(executable.parent.exists())
                self.assertFalse(installed())

    def test_the_portable_archive_must_hold_the_installed_files(self):
        import zipfile
        files = {name: f'synthetic {name}'.encode() for name in qualify.WINDOWS_FILES}
        cases = [({}, None), ({'qrow.exe': b'other build'}, 'different qrow.exe'),
                 ({'extra.dll': b'extra'}, 'Unexpected'), ({'LICENSE': None}, 'Unexpected')]
        for change, error in cases:
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                app = root / 'Qrow'
                app.mkdir()
                for name, data in files.items():
                    (app / name).write_bytes(data)
                archive = root / 'portable.zip'
                with zipfile.ZipFile(archive, 'w') as bundle:
                    for name, data in {**files, **change}.items():
                        if data is not None:
                            bundle.writestr(name, data)
                if error:
                    with self.assertRaisesRegex(ValueError, error):
                        qualify.windows_portable(archive, app)
                else:
                    qualify.windows_portable(archive, app)

    def test_an_existing_install_stops_the_check(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(qualify.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'already installed'), \
                    qualify.windows_installer(Path('setup.exe'), Path(directory), installed=lambda: True):
                pass
            run.assert_not_called()

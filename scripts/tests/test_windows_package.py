"""Windows packages contain the native executable and required license files."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('windows_package', ROOT / 'scripts/package/windows.py')
windows = importlib.util.module_from_spec(spec)
spec.loader.exec_module(windows)


class WindowsPackageTests(unittest.TestCase):
    def test_both_archives_use_default_version_when_manual_input_is_empty(self):
        for machine, architecture in [('AMD64', 'x86_64'), ('ARM64', 'aarch64')]:
            with self.subTest(machine=machine), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'target/release').mkdir(parents=True)
                (root / 'target/release/qrow.exe').write_bytes(b'synthetic PE')
                for name in ('LICENSE', 'NOTICE'):
                    (root / name).write_text('synthetic license')
                metadata = json.dumps({'packages': [{'name': 'qrow', 'version': '1.2.3'}],
                                       'target_directory': str(root / 'target')})

                def run(command, **kwargs):
                    if 'scripts/package/notices.py' in command:
                        Path(command[-1]).write_text('synthetic notices')

                previous = Path.cwd()
                try:
                    os.chdir(root)
                    with patch.object(windows.sys, 'platform', 'win32'), \
                            patch.object(windows.platform, 'machine', return_value=machine), \
                            patch.dict(os.environ, {'QROW_RELEASE_VERSION': '', 'CARGO_BUILD_TARGET': ''}), \
                            patch.object(windows.subprocess, 'check_output', return_value=metadata) as metadata_call, \
                            patch.object(windows.subprocess, 'run', side_effect=run) as calls:
                        windows.main()
                    self.assertEqual(metadata_call.call_args.kwargs["encoding"], "utf-8")
                    self.assertEqual(calls.call_args_list[0].args[0],
                                     ['cargo', 'build', '--locked', '--release', '--bin', 'qrow'])
                    with zipfile.ZipFile(root / f'target/package/Qrow-1.2.3-windows-{architecture}.zip') as archive:
                        self.assertEqual(set(archive.namelist()),
                                         {'qrow.exe', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.txt'})
                        self.assertEqual(archive.read('qrow.exe'), b'synthetic PE')
                finally:
                    os.chdir(previous)

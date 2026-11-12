"""Windows installers contain the native executable and required license files."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import struct
import tempfile
import unittest
import zipfile
from unittest.mock import MagicMock, patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('windows_package', ROOT / 'scripts/package/windows.py')
windows = importlib.util.module_from_spec(spec)
spec.loader.exec_module(windows)


def synthetic_pe(resource_types, pe32_plus=True):
    """A minimal PE file with one section that holds a resource directory."""
    optional_size = 240 if pe32_plus else 224
    header, section_table = 0x40, 0x40 + 4 + 20 + optional_size
    raw, address = 0x200, 0x1000
    directory = struct.pack("<IIHHHH", 0, 0, 0, 0, 0, len(resource_types))
    directory += b"".join(struct.pack("<II", kind, 0x80000000) for kind in resource_types)
    data = bytearray(raw + len(directory))
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, header)
    data[header:header + 4] = b"PE\0\0"
    coff = header + 4
    struct.pack_into("<HH", data, coff, 0x8664, 1)
    struct.pack_into("<H", data, coff + 16, optional_size)
    optional = coff + 20
    struct.pack_into("<H", data, optional, 0x20B if pe32_plus else 0x10B)
    directories = optional + (112 if pe32_plus else 96)
    if resource_types:
        struct.pack_into("<II", data, directories + 16, address, len(directory))
    data[section_table:section_table + 8] = b".rsrc\0\0\0"
    struct.pack_into("<IIII", data, section_table + 8, len(directory), address, len(directory), raw)
    data[raw:] = directory
    return bytes(data)


class ResourceTests(unittest.TestCase):
    def test_an_executable_needs_an_icon_and_a_version_resource(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "qrow.exe"
            for pe32_plus in (True, False):
                executable.write_bytes(synthetic_pe([3, windows.RT_GROUP_ICON, windows.RT_VERSION], pe32_plus))
                self.assertEqual(windows.resource_types(executable), {3, 14, 16})
                windows.require_resources(executable)
            executable.write_bytes(synthetic_pe([3, windows.RT_GROUP_ICON]))
            with self.assertRaisesRegex(ValueError, "no Windows version resource"):
                windows.require_resources(executable)
            executable.write_bytes(synthetic_pe([]))
            with self.assertRaisesRegex(ValueError, "no Windows icon, version resource"):
                windows.require_resources(executable)
            executable.write_bytes(b"synthetic PE")
            with self.assertRaisesRegex(ValueError, "not a PE file"):
                windows.require_resources(executable)


class WindowsPackageTests(unittest.TestCase):
    def test_both_installers_use_default_version_when_manual_input_is_empty(self):
        for machine, architecture, inno in [('AMD64', 'x86_64', 'x64compatible'), ('ARM64', 'aarch64', 'arm64')]:
            with self.subTest(machine=machine), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'target/release').mkdir(parents=True)
                (root / 'target/release/qrow.exe').write_bytes(synthetic_pe([14, 16]))
                for name in ('LICENSE', 'NOTICE'):
                    (root / name).write_text('synthetic license')
                metadata = json.dumps({'packages': [{'name': 'qrow', 'version': '1.2.3'}],
                                       'target_directory': str(root / 'target')})
                staged = {}

                def run(command, **kwargs):
                    if 'scripts/package/notices.py' in command:
                        Path(command[-1]).write_text('synthetic notices')
                    if command[0] == 'ISCC.exe':
                        options = dict(item[2:].split('=', 1) for item in command if item.startswith('/D'))
                        source = Path(options['Source'])
                        staged.update({path.name: path.read_bytes() for path in source.iterdir()})
                        staged['options'] = options
                        (Path(options['OutputDir']) / (options['OutputName'] + '.exe')).write_bytes(b'synthetic setup')

                previous = Path.cwd()
                try:
                    os.chdir(root)
                    with patch.object(windows.sys, 'platform', 'win32'), \
                            patch.object(windows.platform, 'machine', return_value=machine), \
                            patch.object(windows, 'compiler', return_value=Path('ISCC.exe')), \
                            patch.dict(os.environ, {'QROW_RELEASE_VERSION': '', 'CARGO_BUILD_TARGET': ''}), \
                            patch.object(windows.subprocess, 'check_output', return_value=metadata) as metadata_call, \
                            patch.object(windows.subprocess, 'run', side_effect=run) as calls:
                        windows.main()
                    self.assertEqual(metadata_call.call_args.kwargs["encoding"], "utf-8")
                    self.assertEqual(calls.call_args_list[0].args[0],
                                     ['cargo', 'build', '--locked', '--release', '--bin', 'qrow'])
                    installer = root / f'target/package/Qrow-1.2.3-windows-{architecture}-setup.exe'
                    self.assertEqual(installer.read_bytes(), b'synthetic setup')
                    portable = root / f'target/package/Qrow-1.2.3-windows-{architecture}.zip'
                    self.assertEqual(sorted(path.name for path in (root / 'target/package').iterdir()),
                                     sorted([installer.name, portable.name]))
                    with zipfile.ZipFile(portable) as bundle:
                        self.assertEqual(sorted(bundle.namelist()), sorted(windows.PACKAGE_FILES))
                        self.assertEqual({name: bundle.read(name) for name in bundle.namelist()},
                                         {name: data for name, data in staged.items() if name != 'options'})
                    options = staged.pop('options')
                    self.assertEqual(set(staged), set(windows.PACKAGE_FILES))
                    self.assertEqual(staged['qrow.exe'], synthetic_pe([14, 16]))
                    self.assertEqual((options['Version'], options['FileVersion'], options['Architecture']),
                                     ('1.2.3', '1.2.3.0', inno))
                    self.assertTrue(options['Icon'].endswith('qrow.ico'))
                finally:
                    os.chdir(previous)

    def test_portable_archive_is_reproducible(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory, 'source')
            source.mkdir()
            for name in windows.PACKAGE_FILES:
                (source / name).write_text(f'synthetic {name}')
            first, second = Path(directory, 'first.zip'), Path(directory, 'second.zip')
            windows.portable_archive(source, first)
            os.utime(source / 'qrow.exe', (0, 0))
            windows.portable_archive(source, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as bundle:
                self.assertEqual(bundle.namelist(), sorted(windows.PACKAGE_FILES))
                self.assertTrue(all(item.date_time == (1980, 1, 1, 0, 0, 0) for item in bundle.infolist()))

    def test_file_version_is_numeric(self):
        self.assertEqual(windows.file_version('1.2.3'), '1.2.3.0')
        self.assertEqual(windows.file_version('0.4.0-nightly.20261111.12'), '0.4.0.0')
        for invalid in ('nightly', '1.2', '1.70000.0'):
            with self.subTest(version=invalid), self.assertRaisesRegex(ValueError, 'Invalid release version'):
                windows.file_version(invalid)


class CompilerTests(unittest.TestCase):
    def download(self, payload):
        response = MagicMock()
        response.__enter__.return_value.read.return_value = payload
        return patch.object(windows.urllib.request, 'urlopen', return_value=response)

    def test_pinned_compiler_installs_once_in_portable_mode(self):
        payload = b'synthetic installer'
        with tempfile.TemporaryDirectory() as directory, self.download(payload), \
                patch.object(windows, 'INNO_SETUP_SHA256', hashlib.sha256(payload).hexdigest()), \
                patch.dict(os.environ, {'QROW_ISCC': ''}):
            root = Path(directory)
            target = root / 'target/tools' / f'innosetup-{windows.INNO_SETUP_VERSION}'

            def install(command, **kwargs):
                target.mkdir(parents=True)
                (target / 'ISCC.exe').write_bytes(b'synthetic compiler')

            with patch.object(windows.subprocess, 'run', side_effect=install) as calls:
                self.assertEqual(windows.compiler(root), target / 'ISCC.exe')
                self.assertEqual(windows.compiler(root), target / 'ISCC.exe')
            self.assertEqual(calls.call_count, 1)
            command = calls.call_args.args[0]
            self.assertIn('/PORTABLE=1', command)
            self.assertIn('/VERYSILENT', command)
            self.assertIn(f'/DIR={target.resolve()}', command)

    def test_a_changed_download_does_not_run(self):
        with tempfile.TemporaryDirectory() as directory, self.download(b'changed installer'), \
                patch.dict(os.environ, {'QROW_ISCC': ''}), \
                patch.object(windows.subprocess, 'run') as calls:
            with self.assertRaisesRegex(ValueError, 'does not match the pinned'):
                windows.compiler(Path(directory))
            calls.assert_not_called()

    def test_override_selects_another_compiler(self):
        with patch.dict(os.environ, {'QROW_ISCC': r'C:\synthetic\ISCC.exe'}):
            self.assertEqual(windows.compiler(Path('unused')), Path(r'C:\synthetic\ISCC.exe'))


class ScriptTests(unittest.TestCase):
    def test_script_takes_every_definition_the_build_supplies(self):
        script = (ROOT / 'scripts/package/qrow.iss').read_text()
        command = windows.compile_command(Path('ISCC.exe'), '1.2.3', 'x86_64', Path('source'), Path('out/Qrow-setup.exe'))
        for name in (item[2:].split('=', 1)[0] for item in command if item.startswith('/D')):
            self.assertIn(f'{{#{name}}}', script)
            self.assertIn(f'!Defined({name})', script)
        for name in windows.PACKAGE_FILES:
            self.assertIn(f'Source: "{name}"', script)
        self.assertIn('PrivilegesRequired=lowest', script)

    def test_a_silent_update_starts_qrow_again_as_the_user(self):
        script = (ROOT / 'scripts/package/qrow.iss').read_text()
        relaunch = [line for line in script.splitlines() if 'Check: IsUpdateRelaunch' in line]
        self.assertEqual(len(relaunch), 1)
        # The installer can run as administrator; Qrow must not.
        self.assertIn('runasoriginaluser', relaunch[0])
        self.assertNotIn('skipifsilent', relaunch[0])
        # Only Qrow's update command starts it; a silent install by an
        # administrator or a deployment tool does not.
        self.assertIn("WizardSilent and (ExpandConstant('{param:QROWRELAUNCH|0}') = '1')", script)
        # The flag matches the command line that Qrow passes.
        self.assertIn('"/QROWRELAUNCH=1"', (ROOT / 'src/update.rs').read_text())

    def test_qualification_finds_the_install_by_the_app_id(self):
        qualify = (ROOT / 'scripts/package/qualify.py').read_text()
        app_id = re.search(r'^AppId=\{(\{[0-9A-F-]{36}\})$', (ROOT / 'scripts/package/qrow.iss').read_text(), re.MULTILINE)
        self.assertIsNotNone(app_id)
        self.assertIn(app_id.group(1) + '_is1', qualify)

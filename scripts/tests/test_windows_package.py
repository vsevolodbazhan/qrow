"""Windows packages contain the native executable and required license files."""
import importlib.util
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile

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
    def test_both_archives_use_default_version_when_manual_input_is_empty(self):
        for machine, architecture in [('AMD64', 'x86_64'), ('ARM64', 'aarch64')]:
            with self.subTest(machine=machine), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'target/release').mkdir(parents=True)
                (root / 'target/release/qrow.exe').write_bytes(synthetic_pe([14, 16]))
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
                        self.assertEqual(archive.read('qrow.exe'), synthetic_pe([14, 16]))
                finally:
                    os.chdir(previous)

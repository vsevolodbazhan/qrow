"""Build the unsigned native Windows release archive."""
import json
import os
from pathlib import Path
import platform
import re
import struct
import subprocess
import sys
import tempfile
import zipfile


# The resource types that Explorer, the window, and File Properties use.
RT_GROUP_ICON = 14
RT_VERSION = 16


def resource_types(executable):
    """The numeric resource types in the resource directory of a PE file."""
    data = Path(executable).read_bytes()
    if data[:2] != b"MZ":
        raise ValueError(f"{executable} is not a PE file")
    header = struct.unpack_from("<I", data, 0x3C)[0]
    if data[header:header + 4] != b"PE\0\0":
        raise ValueError(f"{executable} is not a PE file")
    coff = header + 4
    sections, optional_size = struct.unpack_from("<H", data, coff + 2)[0], struct.unpack_from("<H", data, coff + 16)[0]
    optional = coff + 20
    magic = struct.unpack_from("<H", data, optional)[0]
    directories = optional + {0x10B: 96, 0x20B: 112}[magic]
    resource_rva, resource_size = struct.unpack_from("<II", data, directories + 2 * 8)
    if not resource_rva or not resource_size:
        return set()
    for index in range(sections):
        section = optional + optional_size + index * 40
        virtual_size, address, raw_size, raw = struct.unpack_from("<IIII", data, section + 8)
        if address <= resource_rva < address + max(virtual_size, raw_size):
            directory = raw + resource_rva - address
            break
    else:
        raise ValueError(f"{executable} has a resource directory outside its sections")
    named, numbered = struct.unpack_from("<HH", data, directory + 12)
    entries = directory + 16 + named * 8
    return {struct.unpack_from("<I", data, entries + index * 8)[0] & 0xFFFF for index in range(numbered)}


def require_resources(executable):
    missing = {RT_GROUP_ICON: "icon", RT_VERSION: "version"}.keys() - resource_types(executable)
    if missing:
        names = ", ".join({RT_GROUP_ICON: "icon", RT_VERSION: "version"}[kind] for kind in sorted(missing))
        raise ValueError(f"{executable} has no Windows {names} resource")


def main():
    if (sys.platform != "win32" or platform.machine().lower() not in ("amd64", "x86_64", "arm64", "aarch64")
            or os.environ.get("CARGO_BUILD_TARGET")):
        raise ValueError("A native Windows build is required; unset CARGO_BUILD_TARGET")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True, encoding="utf-8"))
    package = next(item for item in metadata["packages"] if item["name"] == "qrow")
    version = os.environ.get("QROW_RELEASE_VERSION") or package["version"]
    if not re.fullmatch(r"[a-zA-Z0-9.+-]+", version):
        raise ValueError("Invalid release version")
    subprocess.run(["cargo", "build", "--locked", "--release", "--bin", "qrow"], check=True)
    output = Path("target/package")
    output.mkdir(parents=True, exist_ok=True)
    architecture = "aarch64" if platform.machine().lower() in ("arm64", "aarch64") else "x86_64"
    archive = output / f"Qrow-{version}-windows-{architecture}.zip"
    executable = Path(metadata["target_directory"]) / "release/qrow.exe"
    require_resources(executable)
    with tempfile.TemporaryDirectory(prefix="qrow-notices-") as directory:
        notices = Path(directory) / "THIRD_PARTY_NOTICES.txt"
        subprocess.run([sys.executable, "scripts/package/notices.py", str(notices)], check=True)
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            bundle.write(executable, "qrow.exe")
            for name in ("LICENSE", "NOTICE"):
                bundle.write(name, name)
            bundle.write(notices, notices.name)
    print(archive)


if __name__ == "__main__":
    main()

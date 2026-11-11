"""Build the unsigned native Windows installer."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.request


ROOT = Path(__file__).resolve().parents[2]
# The pinned Inno Setup compiler. Its own installer has a portable mode that
# writes no registry entries and no uninstaller.
INNO_SETUP_VERSION = "7.1.1"
INNO_SETUP_URL = ("https://github.com/jrsoftware/issrc/releases/download/"
                  f"is-{INNO_SETUP_VERSION.replace('.', '_')}/innosetup-{INNO_SETUP_VERSION}-x64.exe")
INNO_SETUP_SHA256 = "8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0"
# Inno Setup architecture identifiers. The x64 installer also runs on ARM64
# Windows 11, which emulates x64.
INNO_ARCHITECTURES = {"x86_64": "x64compatible", "aarch64": "arm64"}
PACKAGE_FILES = ("qrow.exe", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.txt")

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


def file_version(version):
    """The four-part numeric file version of a release version, such as 1.2.3.0 for 1.2.3-nightly.4."""
    match = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
    if not match or any(int(part) > 65535 for part in match.groups()):
        raise ValueError("Invalid release version")
    return ".".join(match.groups()) + ".0"


def compiler(root=ROOT):
    """The Inno Setup command-line compiler. QROW_ISCC selects another one;
    otherwise the pinned compiler installs once into target/tools."""
    if os.environ.get("QROW_ISCC"):
        return Path(os.environ["QROW_ISCC"])
    directory = root / "target/tools" / f"innosetup-{INNO_SETUP_VERSION}"
    iscc = directory / "ISCC.exe"
    if iscc.exists():
        return iscc
    with tempfile.TemporaryDirectory(prefix="qrow-innosetup-") as download:
        setup = Path(download) / f"innosetup-{INNO_SETUP_VERSION}.exe"
        print(f"Downloading {INNO_SETUP_URL}", file=sys.stderr)
        with urllib.request.urlopen(INNO_SETUP_URL, timeout=120) as response:
            setup.write_bytes(response.read())
        digest = hashlib.sha256(setup.read_bytes()).hexdigest()
        if digest != INNO_SETUP_SHA256:
            raise ValueError(f"Inno Setup SHA-256 {digest} does not match the pinned {INNO_SETUP_SHA256}")
        subprocess.run([str(setup), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/CURRENTUSER",
                        "/PORTABLE=1", f"/DIR={directory.resolve()}", f"/LOG={Path(download) / 'setup.log'}"],
                       check=True, timeout=10 * 60)
    if not iscc.exists():
        raise ValueError(f"Inno Setup did not install {iscc}")
    return iscc


def compile_command(iscc, version, architecture, source, output):
    return [str(iscc), "/Qp",
            f"/DVersion={version}",
            f"/DFileVersion={file_version(version)}",
            f"/DArchitecture={INNO_ARCHITECTURES[architecture]}",
            f"/DSource={source.resolve()}",
            f"/DIcon={(ROOT / 'assets/app-icons/windows/qrow.ico').resolve()}",
            f"/DOutputDir={output.parent.resolve()}",
            f"/DOutputName={output.stem}",
            str(ROOT / "scripts/package/qrow.iss")]


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
    file_version(version)
    subprocess.run(["cargo", "build", "--locked", "--release", "--bin", "qrow"], check=True)
    output = Path("target/package")
    output.mkdir(parents=True, exist_ok=True)
    architecture = "aarch64" if platform.machine().lower() in ("arm64", "aarch64") else "x86_64"
    installer = output / f"Qrow-{version}-windows-{architecture}-setup.exe"
    executable = Path(metadata["target_directory"]) / "release/qrow.exe"
    require_resources(executable)
    iscc = compiler()
    with tempfile.TemporaryDirectory(prefix="qrow-windows-package-") as directory:
        staging = Path(directory)
        subprocess.run([sys.executable, "scripts/package/notices.py", str(staging / "THIRD_PARTY_NOTICES.txt")],
                       check=True)
        shutil.copyfile(executable, staging / "qrow.exe")
        for name in ("LICENSE", "NOTICE"):
            shutil.copyfile(name, staging / name)
        installer.unlink(missing_ok=True)
        subprocess.run(compile_command(iscc, version, architecture, staging, installer), check=True)
    if not installer.is_file():
        raise ValueError(f"Inno Setup did not create {installer}")
    print(installer)


if __name__ == "__main__":
    main()

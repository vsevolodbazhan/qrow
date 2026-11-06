"""Target definitions. Host discovery is explicit and never selects CI policy."""
from dataclasses import dataclass
import platform


@dataclass(frozen=True)
class Target:
    name: str
    system: str
    machine: str
    triple: str
    runner: str
    supported: bool = True


TARGETS = {item.name: item for item in (
    Target("macos-arm64", "macos", "aarch64", "aarch64-apple-darwin", "macos-15"),
    Target("macos-x86_64", "macos", "x86_64", "x86_64-apple-darwin", "macos-15-intel"),
    Target("linux-x86_64", "linux", "x86_64", "x86_64-unknown-linux-gnu", "ubuntu-24.04"),
    Target("linux-arm64", "linux", "aarch64", "aarch64-unknown-linux-gnu", "ubuntu-24.04-arm"),
    Target("windows-x86_64", "windows", "x86_64", "x86_64-pc-windows-msvc", "", False),
)}


def host_target():
    system = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system())
    machine = platform.machine().lower()
    machine = {"arm64": "aarch64", "amd64": "x86_64"}.get(machine, machine)
    return next((item for item in TARGETS.values() if (item.system, item.machine) == (system, machine)), None)

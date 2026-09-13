"""Collect license metadata and supplied license texts for the native build."""
import json
import pathlib
import subprocess
import sys

target = subprocess.check_output(["rustc", "-vV"], text=True).split("host: ")[1].splitlines()[0]
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target], text=True
))
resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
sections = ["Qrow: third-party license notices\n\nIncludes native dependencies and build/test dependencies.\n"]
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["id"] not in resolved or package["name"] == "qrow":
        continue
    root = pathlib.Path(package["manifest_path"]).parent
    sections.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package.get('license') or 'see supplied license file'}\nSource: {package.get('repository') or package.get('source')}\n")
    files = set()
    for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "license*", "licence*"):
        files.update(p for p in root.glob(pattern) if p.is_file())
    if package.get("license_file"):
        files.add(root / package["license_file"])
    for path in sorted(files):
        sections.append(f"\n--- {path.name} ---\n{path.read_text(errors='replace')}\n")
pathlib.Path(sys.argv[1]).write_text("".join(sections))

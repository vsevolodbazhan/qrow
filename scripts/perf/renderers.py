#!/usr/bin/env python3
"""Compare native release CPU and OpenGL rendering with isolated demo workspaces."""
import json
import os
from pathlib import Path
import re
import statistics
import subprocess


def measurements(directory, backend):
    log = (directory / "qrow.log").read_text()
    expected = "OpenGL" if backend == "opengl" else "CPU"
    if f"Qrow: using {expected} rendering" not in log:
        raise RuntimeError(f"The {backend} test did not select its renderer")
    if backend == "opengl" and ("hardware accelerated" not in log or "OpenGL drawing failed" in log):
        raise RuntimeError("The hardware OpenGL renderer failed")
    frames = [(float(timestamp), float(value)) for timestamp, value in
              re.findall(rf"QROW_RENDER_FRAME backend={backend} time=([0-9.]+) ms=([0-9.]+)", log)]
    resources = (directory / "qrow-resources.txt").read_text().splitlines()
    actions = [json.loads(line.removeprefix("QROW_NATIVE_PERF ")) for line in resources
               if line.startswith("QROW_NATIVE_PERF ")]
    if {item["scenario"] for item in actions} != {"results_vertical", "results_horizontal", "editor_vertical", "idle"}:
        raise RuntimeError("Native performance actions are incomplete")
    grouped = {}
    for action in actions:
        if action["scenario"] != "idle":
            selected = [value for timestamp, value in frames if action["start_seconds"] <= timestamp <= action["end_seconds"]]
            if len(selected) < 10:
                raise RuntimeError(f"Too few measured {backend} frames in {action['scenario']}: {len(selected)}")
            grouped[action["scenario"]] = selected
    launch = float(next(line.split("=", 1)[1] for line in resources
                        if line.startswith("launch_to_accessible_new_connection_seconds=")))
    return {"backend": backend, "frames_ms": grouped, "actions": actions, "launch_seconds": launch}


def main():
    root = Path(__file__).resolve().parents[2]
    output = Path(os.environ["QROW_E2E_ARTIFACTS"]).resolve()
    bundle = output / "package/Qrow.app"
    subprocess.run(["sh", "scripts/e2e/driver.sh", "--preflight"], cwd=root, check=True)
    driver = Path(os.environ.get("CARGO_TARGET_DIR", root / "target")) / "e2e-tools/native-driver"
    runs = []
    for repeat in range(3):
        for backend in ("software", "opengl"):
            directory = output / f"{backend}-{repeat + 1}"
            (directory / "workspace").mkdir(parents=True, exist_ok=False)
            environment = dict(os.environ, QROW_RENDERER=backend, QROW_RENDER_METRICS="1",
                               QROW_E2E_BUNDLE=str(bundle), QROW_E2E_ARTIFACTS=str(directory),
                               QROW_DATA_DIR=str(directory / "workspace"))
            environment.pop("QROW_SOFTWARE_RENDERER", None)
            subprocess.run([str(driver), "--renderer-performance-only"], cwd=root, env=environment, check=True)
            runs.append(measurements(directory, backend))
    summary = {}
    for backend in ("software", "opengl"):
        selected = [run for run in runs if run["backend"] == backend]
        scenarios = {}
        for scenario in ("results_vertical", "results_horizontal", "editor_vertical"):
            frames = [value for run in selected for value in run["frames_ms"][scenario]]
            scenarios[scenario] = {"composition_median_ms": statistics.median(frames),
                                   "composition_p95_ms": statistics.quantiles(frames, n=100, method="inclusive")[94],
                                   "slowest_run_p95_ms": max(statistics.quantiles(run["frames_ms"][scenario], n=100, method="inclusive")[94]
                                                             for run in selected), "samples": len(frames)}
        active_cpu = [sum(action["cpu_seconds"] for action in run["actions"] if action["scenario"] != "idle")
                      for run in selected]
        idle_cpu = [action["cpu_seconds"] / action["seconds"] * 100
                    for run in selected for action in run["actions"] if action["scenario"] == "idle"]
        summary[backend] = {"scenarios": scenarios,
                            "active_cpu_seconds": statistics.median(active_cpu),
                            "idle_cpu_percent": statistics.median(idle_cpu),
                            "launch_seconds": statistics.median(run["launch_seconds"] for run in selected),
                            "peak_sampled_rss_mib": max(action["rss_mib"] for run in selected for action in run["actions"])}
    (output / "measurements.json").write_text(json.dumps({"summary": summary, "runs": runs}, indent=2) + "\n")
    print(json.dumps(summary, indent=2), flush=True)
    cpu_reference = summary["software"]["active_cpu_seconds"]
    if cpu_reference <= 0:
        raise RuntimeError("CPU reference did not record process CPU time")
    probes = [(f"renderers.opengl.{scenario}.composition_p95", values["slowest_run_p95_ms"], "ms", 33)
              for scenario, values in summary["opengl"]["scenarios"].items()]
    probes.append(("renderers.active_cpu_ratio", summary["opengl"]["active_cpu_seconds"] / cpu_reference, "ratio", 1))
    for probe, value, unit, budget in probes:
        print("QROW_PERF " + json.dumps({"probe": probe, "value": value, "unit": unit, "budget": budget}), flush=True)
    if any(values["slowest_run_p95_ms"] > 33 for values in summary["opengl"]["scenarios"].values()):
        raise RuntimeError("OpenGL composition exceeds the initial 33 ms p95 target")
    if summary["opengl"]["active_cpu_seconds"] >= summary["software"]["active_cpu_seconds"]:
        raise RuntimeError("OpenGL did not reduce process CPU time in the scripted actions")


if __name__ == "__main__":
    main()

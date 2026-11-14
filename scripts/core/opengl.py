"""Probe production Core OpenGL context/limits through qtest's process-group executor.

Only an unavailable hardware pixel format is a supported negative result.
Every execution or parsing error is persisted and fails the CI run.
"""
import os
import re

HARDWARE_SUITES = frozenset(("opengl-renderer", "opengl-ui", "perf-renderers"))
PROBE = "opengl_renderer::context::tests::hardware_capability_probe"


def probe(execute, log, output, command=None, env=None):
    command = command or ["cargo", "test", "--locked", "-p", "qrow", "-p", "gpui-pre-macos",
               "--features", "gpui-kit/test-support", "--lib", PROBE, "--", "--exact", "--nocapture"]
    result = {"available": None, "command": command, "log": str(log)}
    try:
        # The shared executor kills the complete Cargo/compiler/test process
        # group on timeout and keeps the partial output in this run's log.
        code = execute(command, dict(os.environ, **(env or {})), 1800, log, output)
    except OSError as error:
        result["error"] = f"OpenGL capability probe could not start: {error}"
    else:
        if code is None:
            result["error"] = "OpenGL capability probe timed out."
        elif code != 0:
            result["error"] = f"OpenGL capability probe failed ({code})."
    result["output"] = log.read_text() if log.exists() else ""
    if "error" not in result:
        markers = re.findall(r"^(?:test " + re.escape(PROBE) + r" \.\.\. )?QROW_OPENGL_CAPABILITY (available|unavailable): (.+)$",
                             result["output"], re.MULTILINE)
        if len(markers) != 1:
            result["error"] = "OpenGL capability probe returned no unique result."
        else:
            status, reason = markers[0]
            if status == "unavailable" and not reason.endswith("pixel format unavailable (10002)"):
                result["error"] = "Unexpected OpenGL unavailable result: " + reason
            else:
                result.update(available=status == "available", reason=reason)
    return result


CI_HARDWARE = ("opengl-renderer", "opengl-ui")


def verify_hardware(result, value, manifest_sha):
    """Validate optional GPU evidence without weakening required-plan verification."""
    for key in ("source_sha", "snapshot", "target"):
        if result.get(key) != value[key]:
            raise ValueError("Renderer evidence identity mismatch: " + key)
    if result.get("build_manifest_sha256") != manifest_sha:
        raise ValueError("Renderer prepared build identity mismatch")
    capability = result.get("capability", {})
    available = capability.get("available")
    if capability.get("error") or type(available) is not bool:
        raise ValueError("Renderer capability probe did not pass")
    if result.get("status") != "passed" or result.get("exit_code") != 0:
        raise ValueError("Hardware renderer checks did not pass")
    suites = result.get("suites", [])
    if [item.get("name") for item in suites] != list(CI_HARDWARE):
        raise ValueError("Missing or unexpected hardware renderer suite")
    expected = "passed" if available else "skipped"
    if any(item.get("status") != expected or item.get("report_only") for item in suites):
        raise ValueError("Invalid hardware renderer result")
    if not available:
        reason = capability.get("reason", "")
        if not reason.endswith("pixel format unavailable (10002)"):
            raise ValueError("Only CGL 10002 permits hardware skips")
        if any(item.get("reason") != "Hardware OpenGL unavailable: " + reason for item in suites):
            raise ValueError("Hardware skip diagnostic mismatch")
    else:
        reports = suites[0].get("tests", [])
        if len(reports) != 1 or reports[0].get("count", 0) < 1 or reports[0].get("failures") != 0:
            raise ValueError("Missing hardware pixel-test evidence")


def run_hardware(root, value, manifest, base, output):
    """Execute source-bound prepared GPU tests; preserve every controlled probe failure."""
    import builds
    import catalog
    import contract
    import runner

    base.mkdir(parents=True, exist_ok=True)
    log = base / "renderer-probe.log"
    options = {"runtime": "native", "repeat": 1, "isolated_reports": True,
               "reuse_builds": manifest, "env": {"QROW_COMMIT": value["source_sha"][:12]}}
    try:
        with runner.lease([f"cargo:{runner.target_dir().resolve()}"]):
            hardware = contract.plan(root, value["target"], list(CI_HARDWARE), "native")
            prepared = builds.Prepared(manifest, hardware, root, base / "renderer-probe-prepared", output)
            step = catalog.nextest("test(=" + PROBE + ")",
                                   *catalog.RENDERER_FLAGS, "--no-capture")
            command = prepared.command(runner.render_command(step, options, None, []))
            capability = probe(runner.execute, log, output, command, options["env"])
    except (OSError, ValueError) as error:
        capability = {"available": None, "error": "Renderer probe preparation failed: " + str(error),
                      "log": str(log), "output": log.read_text() if log.exists() else ""}
    options["ci_opengl_capability"] = capability
    suites = catalog.suites_for_platform("macos")
    selected = [runner.Selected(suites[name]) for name in CI_HARDWARE]
    result = runner.run(selected, options, [], output, require_all=True)
    result.update(source_sha=value["source_sha"], snapshot=value["snapshot"], target=value["target"],
                  build_manifest_sha256=builds.sha256(manifest), capability=capability)
    contract.atomic_json(base / "renderer-result.json", result)
    verify_hardware(result, value, result["build_manifest_sha256"])
    return result

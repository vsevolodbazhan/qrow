"""Probe production Core OpenGL context/limits through qtest's process-group executor.

Only an unavailable hardware pixel format is a supported negative result.
Every execution or parsing error is persisted and fails the CI run.
"""
import os
import re

HARDWARE_SUITES = frozenset(("opengl-renderer", "opengl-ui", "perf-renderers"))
PROBE = "opengl_renderer::context::tests::hardware_capability_probe"


def probe(execute, log, output):
    command = ["cargo", "test", "--locked", "-p", "qrow", "-p", "gpui-pre-macos",
               "--features", "gpui-kit/test-support", "--lib", PROBE, "--", "--exact", "--nocapture"]
    result = {"available": None, "command": command, "log": str(log)}
    try:
        # The shared executor kills the complete Cargo/compiler/test process
        # group on timeout and keeps the partial output in this run's log.
        code = execute(command, dict(os.environ), 1800, log, output)
    except OSError as error:
        result["error"] = f"OpenGL capability probe could not start: {error}"
    else:
        if code is None:
            result["error"] = "OpenGL capability probe timed out."
        elif code != 0:
            result["error"] = f"OpenGL capability probe failed ({code})."
    result["output"] = log.read_text() if log.exists() else ""
    if "error" not in result:
        markers = re.findall(r"^QROW_OPENGL_CAPABILITY (available|unavailable): (.+)$",
                             result["output"], re.MULTILINE)
        if len(markers) != 1:
            result["error"] = "OpenGL capability probe returned no unique result."
        else:
            status, reason = markers[0]
            result.update(available=status == "available", reason=reason)
    return result

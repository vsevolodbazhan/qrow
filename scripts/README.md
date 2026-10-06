# Scripts

See [Testing](../docs/testing.md) for `./qtest`, which runs all checks and tests.
See [Development](../docs/development.md) for hooks, dependency
maintenance, and binding generation. Build and package commands are in the
[project README](../README.md#build).

| Path | Purpose |
| --- | --- |
| `ci/` | CI maintenance: the deletion of Rust build caches that newer caches replaced. |
| `qtest/` | The [`./qtest`](../docs/testing.md) command: suite catalog, runner, and command line. |
| `core/` | Shared prerequisite checks, the size check, and the dependency policy check. `environment.py` finds the target directory, Docker, and the JDK for the Python scripts. |
| `e2e/` | Disposable test servers and their downloads, the native driver entry point, and synthetic credential cleanup. |
| `package/` | macOS packaging, the architecture and minimum-version reader, the version reader, icon conversion, and dependency notices. |
| `perf/` | The launch time, idle memory, and idle CPU probe of the release app (the `perf-app` suite). |
| `hooks/` | Hook installation and isolated Git snapshot checks. |
| `generate/` | Reproducible generation of the Thrift bindings and the small application icon. |
| `tests/` | Automation, fixture, policy, and hook tests. |

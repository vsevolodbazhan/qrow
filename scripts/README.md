# Scripts

See [Testing](../docs/testing.md) for `./qtest`, which runs all checks and tests.
See [Development](../docs/development.md) for hooks, dependency
maintenance, and binding generation. Build and package commands are in the
[project README](../README.md#build).

| Path | Purpose |
| --- | --- |
| `ci/` | Common test ownership, native build/test adapters, strict evidence verification, and removal of replaced Rust build caches. |
| `qtest/` | The [`./qtest`](../docs/testing.md#plan-and-execute-checks) command: target definitions, source-bound plans, prepared nextest builds, execution, evidence, and command line. |
| `core/` | Shared prerequisite checks, the size check, and the dependency policy check. `environment.py` finds the target directory, Docker, and the JDK for the Python scripts. |
| `e2e/` | Disposable test servers and their downloads, the native driver entry point, synthetic credential cleanup, and the DuckDB CSV reader check. |
| `package/` | Native archive qualification and macOS packaging: the app bundle, the zip, the installer DMG and its background, the architecture and minimum-version reader, the version reader, icon conversion, and dependency notices. |
| `perf/` | The launch time, idle memory, and idle CPU probe of the release app (the `perf-app` suite). |
| `hooks/` | Hook installation and isolated Git snapshot checks. |
| `generate/` | Reproducible generation of the Thrift bindings and the small application icon. |
| `tests/` | Automation, fixture, policy, and hook tests. |

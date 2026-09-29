# Scripts

See [Testing](../docs/testing.md) for `./qtest`, which runs all checks and tests.
See [Development](../docs/development.md) for hooks, dependency
maintenance, and binding generation. Build and package commands are in the
[project README](../README.md#build).

| Path | Purpose |
| --- | --- |
| `qtest/` | The [`./qtest`](../docs/testing.md) command: suite catalog, runner, and command line. |
| `core/` | Shared prerequisite checks, the size check, and the dependency policy check. |
| `e2e/` | Disposable test servers and their downloads, the native driver entry point, and synthetic credential cleanup. |
| `package/` | macOS packaging, icon conversion, and dependency notices. |
| `hooks/` | Hook installation and isolated Git snapshot checks. |
| `generate/` | Reproducible Thrift binding generation. |
| `tests/` | Automation, fixture, policy, and hook tests. |

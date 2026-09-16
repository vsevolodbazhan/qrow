# Scripts

See [Development](../docs/development.md) for local checks, hooks, dependency
maintenance, and binding generation. See
[End-to-end testing](../docs/end-to-end-testing.md) for real-server and native UI
tests. Build and package commands are in the [project README](../README.md#build).

| Path | Purpose |
| --- | --- |
| `check.sh` | Select a local check or an explicit end-to-end suite. |
| `core/` | Local checks, tool installation, coverage, performance, size, and dependency policy. |
| `e2e/` | Disposable servers, test orchestration, native driver entry point, and synthetic credential cleanup. |
| `package/` | macOS packaging, icon conversion, and dependency notices. |
| `hooks/` | Hook installation and isolated Git snapshot checks. |
| `generate/` | Reproducible Thrift binding generation. |
| `tests/` | Automation, fixture, policy, and hook tests. |

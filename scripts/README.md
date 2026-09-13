# Scripts

Check entry points locate the repository root themselves and mirror the CI
workflow and job names:

| CI check | Entry point |
| --- | --- |
| core / dependencies | `sh scripts/core/dependencies.sh` |
| core / backend | `sh scripts/core/backend.sh` |
| core / macos | `sh scripts/core/macos.sh` |
| e2e / backend | `sh scripts/e2e/backend.sh` |
| e2e / macos | `sh scripts/e2e/macos.sh` |

`sh scripts/check.sh` runs all offline checks. Use a scoped name, such as
`sh scripts/check.sh core/backend`, to run one entry point. E2E stays opt-in.
Native UI E2E uses Java 17 by default; append `--runtime docker` to use Docker.

| Directory | Purpose |
| --- | --- |
| `core/` | Offline checks, coverage, performance, size and dependency policy; `install.sh` installs check tools. |
| `e2e/` | Backend and macOS entry points, server orchestration, native driver and synthetic Keychain cleanup. |
| `package/` | macOS packaging, icon conversion and dependency notices. |
| `hooks/` | Hook installation and isolated Git snapshot checks. |
| `generate/` | Reproducible Thrift binding generation. |
| `tests/` | Tests for automation, fixtures, policy and hook isolation. |

CI installs platform tools before invoking checks and runs coverage, script
checks, performance and packaging as additional steps. The `e2e / gate` remains
in the workflow because it reports GitHub job outcomes on the tested commit.

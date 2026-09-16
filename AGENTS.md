- Consult relevant pages in [docs/](docs/) when needed. Do not read every page
  by default. Use the [README](README.md) for build and launch instructions.
- Investigate disagreements between code and documentation before changing either.
- Run the checks appropriate to the change. Follow
  [Development](docs/development.md) for verification and maintenance procedures.
  Do not weaken checks or bypass failing hooks.
- Use isolated workspaces and synthetic credentials for tests. Never overwrite
  the user's workspace or use their credentials as fixtures.
- Preserve the user's working files and staged changes. Do not replace the
  packaged app while the user is testing it.
- Report what changed, what you verified, and any remaining verification gaps.
  Do not present attempted checks as successful checks.
- Write pull request titles and descriptions in English.
- Update affected docs in the same change as behavior, configuration, development
  commands, or testing workflows. Remove obsolete statements.
- Describe current behavior and known limitations. Keep limitations with their topics.
- Start pages with purpose and everyday use, then explain high-level design.
  Keep implementation details in code. Link to existing explanations instead
  of duplicating them.
- Use ASD-STE100 Simplified Technical English when writing or updating documentation.
- Keep this file limited to agent working instructions. Put product behavior,
  architecture, and workflow details in the relevant docs.

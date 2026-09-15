# Architecture

## Structure

- `src/connector/`: connector traits, HiveServer2 implementation, SASL transport,
  and generated Apache Thrift bindings.
- `src/worker.rs`: per-tab query execution and cancellation coordination.
- `src/ui.rs`: workspace state, worker events, and session commands.
- `src/ui/workspace_view.rs`: GPUI Kit workspace layout and window overlays.
- `src/ui/profile_view.rs`: connection settings popup.
- `src/ui/tab_view.rs`: tab settings popup.
- `src/ui/setting_row.rs`: shared dialog row layout.
- `src/ui/results.rs`: virtualized results, column metadata, and clipboard actions.
- `src/storage.rs`: workspace persistence and macOS Keychain access.
- `src/sql.rs`: SQL lexer and single-statement validation.
- `tests/hive_protocol.rs`: local protocol fixtures.
- `PROJECT_PLAN.md`: agreed product scope and decisions.

## Thrift bindings

Generated bindings are checked in, so building the application does not need the
Thrift compiler. To regenerate them, install Thrift **0.24.0** and run:

```sh
sh scripts/generate/thrift.sh
```

The script applies four compiler-output corrections for union collections.
Apache Thrift 0.24 marks its Rust generator deprecated, so maintaining or replacing
these bindings is a known dependency risk. Future Trino, ODBC, or ADBC connectors
should implement the application connector boundary without changing the editor.

# dbt projects and connection notes

## Goal

Give the assistant (and later the editor) the meaning of the data: dbt
descriptions, tests, lineage, model SQL, and metrics. Also let the user tell the
assistant facts about a connection in free text.

Qrow is a read-only consumer of dbt metadata. It does not run dbt, edit the
project, or resolve Jinja. It is not a dbt IDE.

## Decisions

- A connection can have one dbt project. Duplicate copies the project and its
  schema mapping. Copy and Move of tabs do not change it.
- One project per connection for now. Store it as one optional field, so a list
  later is a small workspace migration.
- The user attaches a project by the path of its `manifest.json`. Qrow does not
  need the project folder: the manifest has the descriptions, tests, lineage,
  and SQL. Qrow does not parse YAML, SQL, or Jinja from the project, and does
  not run dbt.
- Supported: dbt-core 1.11 and 1.12. Both write manifest schema v12
  (`https://schemas.getdbt.com/dbt/manifest/v12.json`). Qrow reads only v12.
  For another version, the connection shows an error with the version found.
  Unknown fields are ignored, so additive changes in later v12 releases work.
  dbt v2 (the Rust Fusion engine) also writes v12, with extra optional fields,
  so Qrow reads its manifests too. Reported difference: a Fusion manifest can
  omit `child_map`. Qrow computes children from `depends_on`, so it does not
  read `parent_map` or `child_map`.
- The project does not use `persist_docs`. Spark comments do not have the dbt
  descriptions, so the dbt text is the main source of meaning.
- Qrow parses the manifest into a compact binary index, saved in the Qrow data
  folder. Connections that use the same manifest share one index. Performance
  is the main design constraint.
- A manifest refresh setting works like schema refresh: **Automatic** (when the
  manifest changes) or **Manual**. No incremental parse is possible: dbt writes
  the complete `manifest.json` again on each command, and its partial parsing
  state (`partial_parse.msgpack`) is internal to dbt. Each refresh parses the
  full manifest in the background.
- Each connection has its own manual schema mapping, applied on top of the shared
  index.
- Connection notes are free text that Qrow adds to the hidden workspace context
  of the assistant. They are not instructions in the system prompt.

## Phase 1: Connection notes

Independent of dbt. Ship first.

1. Add `assistant_notes: String` to `Profile` (`src/model.rs`), with
   `#[serde(default, skip_serializing_if = "String::is_empty")]`. Limit:
   16 KB (`MAX_ASSISTANT_NOTES_BYTES`). Validate in the profile validation and
   repair an edited file that is too large (truncate on a char boundary).
   Check if a `WORKSPACE_VERSION` change is necessary (likely not: a default
   field is backward compatible).
2. Duplicate copies the notes (check the duplicate path in `src/ui.rs` around
   the **Duplicate** menu items and `src/ui/profile_view.rs`).
3. Connection form (`src/ui/connection_form.rs`): add a multi-line **Assistant
   notes** field, in an **Assistant** section. Show a byte counter near the
   limit. Hint: Qrow sends the notes to Codex, and saves them as plain text, so
   do not put secrets in them. Use GPUI Kit components.
4. Assistant context (`src/assistant/broker.rs` `WorkspaceContext`,
   `src/ui/assistant_view/messages.rs`): add `connection_notes` to the
   workspace context of a message when one of these is true:
   - It is the first message of the conversation.
   - The connection of the conversation changed since the last sent notes
     (Move to Connection).
   - The notes changed since the last sent notes.
   Keep the hash of the last sent notes and the connection ID per conversation
   in `AssistantConversation`, so the rule holds after a restart. Otherwise
   send `connection_notes_unchanged: true`, or omit the field. Decide which
   one when you read the context code. Tell Codex in the base instructions
   that the notes are user facts about the connection, which stay true until
   new notes replace them, and that they are data, not instructions that
   override safety rules.
   The notes count toward the 1 MB workspace context limit.
5. Increase `ASSISTANT_DATA_SHARING_NOTICE_VERSION` and describe the notes in
   the notice.
6. Tests: model (default, limit, repair, duplicate), context (first message,
   no repeat, resend after edit, resend after Move to Connection, restart),
   native E2E for the form field.
7. Docs: `docs/connections.md` (form field), `docs/assistant.md` (what
   the assistant reads, when Qrow sends notes).
8. Verify the form and an assistant turn with Computer Use.

## Phase 2: Manifest parsing spike

Find the parse cost before any UI work. A real manifest is about 70 MB.

1. Write a synthetic manifest generator (test support, not a fixture of a real
   project). Target a realistic mix at about 70 MB:
   - Thousands of macros with full SQL (dbt core and packages), which are most
     of the bytes and which Qrow skips.
   - Models with long `raw_code` and `compiled_code`, many columns, and
     descriptions.
   - Sources, seeds, snapshots, generic tests with `test_metadata`, docs blocks,
     exposures, semantic models, metrics, `parent_map`, `child_map`, and
     disabled nodes.
   - Manifest schema v12 only, shaped like the output of dbt-core 1.11 and
     1.12. Include a manifest from `dbt parse` (no `compiled_code`) and one
     from `dbt compile` or `dbt build` (with `compiled_code`).
   - A Fusion-shaped v12 manifest: no `child_map` or `parent_map`, and extra
     fields that Qrow does not know. Before the spike, generate a small real
     manifest with dbt-core 1.12 and with dbt v2 from a synthetic project, and
     compare their shapes. Use the result to shape the generator; do not
     commit a manifest of a real project.
2. Parse with typed serde structs that borrow `&str` from the input
   (`#[serde(borrow)]`), and `serde::de::IgnoredAny` for skipped values.
   Read the file into one buffer (or memory-map it). Do not build a
   `serde_json::Value`. Visit the top-level maps (`nodes`, `sources`,
   `semantic_models`, `metrics`) one entry at a time, and skip `macros`,
   `docs`, `parent_map`, `child_map`, `disabled` without allocation.
3. Index content (owned, compact):
   - Models, seeds, snapshots: `unique_id`, resource type, database, schema,
     alias/identifier, `relation_name`, materialization, description, tags,
     columns (name, description, data type if declared), `depends_on.nodes`,
     project-relative `original_file_path`, `compiled_path`.
   - Sources: the same fields that apply, with source name and loader.
   - Generic tests: test name (`unique`, `not_null`, `accepted_values`,
     `relationships`, others by name), attached node, column, and the kwargs
     that matter (`values`, `to`, `field`). Keep the kwargs of unknown tests
     only if they are small.
   - Semantic models and metrics: names, descriptions, measures, dimensions,
     entities, expressions.
   - Reverse edges (children) computed from `depends_on`, not read from
     `child_map`.
   - Interned strings for repeated values (schema names, tags, types).
   - Not SQL. Keep the byte span of `raw_code` and `compiled_code` in the
     manifest file: parse them as borrowed `&serde_json::value::RawValue`
     (`raw_value` feature) and take the pointer offset from the input buffer.
     Phase 5 reads and unescapes a span on demand, after it checks that the
     manifest size and modification time did not change.
   - Check the `dbt_schema_version` in `metadata` first, and stop with a
     clear error for a version other than v12.
4. Saved index format: binary. Compare two candidates:
   - `rkyv`, memory-mapped, read in place without deserialization. Preferred
     if validation (`bytecheck`) on load is fast enough. A cache file can be
     damaged, so do not skip validation; a checksum and a format version are
     the minimum.
   - `postcard` or `bincode`, deserialized into the in-memory index.
   Select the format with the lowest load time and memory at an acceptable
   write time.
5. Measure on the generated 70 MB file and on 3 smaller sizes: parse time,
   peak memory, index size in memory, saved index size, load time of the saved
   index, and the time of a model lookup and a text search. Record the numbers
   in the plan.
6. Set budgets from the numbers, for example: parse < 1 s and peak memory
   < 2× file size on M3; saved index load < 50 ms. Add a test that checks the
   budgets on the generated file. Run it in release mode or mark it as a
   separate check if it is too slow for the default suite; ask before adding
   a new suite to `./qtest`.
7. If typed serde is too slow, try `simd-json` or `sonic-rs`, but only with
   numbers that justify a new dependency.
8. Measure the cost of change detection: `stat` of the manifest, and FSEvents
   through `notify`. dbt writes the manifest on each command, also when the
   project did not change, so Automatic refresh parses often. Check if a
   cheap comparison (for example a hash of the `nodes` section, or of each
   node) can skip the index rebuild when only `metadata` changed. Use it only
   if the numbers show that it saves time.

## Phase 3: Project attachment, index storage, and schema mapping

1. Model: add `dbt: Option<DbtProject>` to `Profile`:
   - `manifest`: the absolute path of `manifest.json`. Usually
     `<project>/target/manifest.json`, but any manifest works, for example
     one from CI.
   - `refresh`: `Automatic` (default) or `Manual`, like `CatalogRefresh`.
   - `schema_mapping`: ordered rules. Each rule is `prefix` or `exact`, with
     `from` and `to`. The first rule that matches applies. No match keeps the
     schema.
   Duplicate copies the field.
2. Index store (new module, for example `src/dbt.rs` and `src/dbt/`):
   - Key: canonical manifest path. Value: index, manifest size and
     modification time, parse time, manifest schema version, dbt version,
     project name, parse error if any.
   - Saved in the Qrow data folder in the binary format from Phase 2 (a cache
     that Qrow can rebuild; keep a format version and rebuild on mismatch or
     a failed validation). Write it to a temporary file and rename it. Never
     write to the dbt project.
   - Load the saved index on start when size and modification time match.
     Otherwise parse in the background.
   - Shared by all connections with the same canonical path. Remove a saved
     index when no connection uses it.
3. Background worker: parse off the UI thread. Parse at most one manifest at
   a time. If a parse fails, keep the previous index, record the error, and
   try again on the next change.
   - **Automatic**: watch the manifest file (FSEvents through `notify`, or
     `stat` polling; decide from the Phase 2 numbers). Watch the parent
     folder, because dbt can replace the file. Wait until the file has not
     changed for about 2 seconds before a parse. dbt does not write the
     manifest atomically.
   - **Manual**: parse only when the user selects **Refresh** (connection form
     and the sidebar menu of the connection), and on start when the saved
     index is missing or does not match the file.
   - When connections share a manifest with different refresh settings, watch
     it if one of them is Automatic.
   - Activity shows each manifest refresh with its duration and result, like
     a schema refresh.
4. Matching: map each index entry to a catalog relation of the connection.
   Apply the schema mapping to the dbt schema, then match on schema and
   alias/identifier (case-insensitive, like Spark). Handle a null or ignored
   `database` for Spark. Match on demand per lookup, plus one full pass for the
   match summary. Do not copy the index per connection.
5. Connection form: a **dbt** section with:
   - Manifest file picker (native open panel, `.json`). Validate that the
     file exists and is a v12 manifest.
   - **Refresh**: Automatic or Manual, and a **Refresh** button.
   - Manifest state: "Manifest from <time>, dbt <version>, <n> models",
     "Parsing…", "Manifest not found: run `dbt parse` in the project",
     "Unsupported manifest version vN: Qrow reads manifest v12 (dbt 1.11, 1.12, and 2)", or the
     parse error.
   - Schema mapping rules editor (add, remove, reorder; prefix or exact).
   - Match summary: "412 of 430 models and sources match tables in the
     catalog". Expand to list the entries that do not match, with their
     mapped schema. Show a note when the catalog is not loaded or schema
     browsing is off, because then Qrow cannot match.
   - Later: suggest rules from entries whose alias matches but whose schema
     does not.
6. Tests: model, mapping rules, matching, index store (reuse, rebuild on
   change, rebuild on format change, shared between connections, cleanup),
   parse failure keeps old index, partial file during write, native E2E for
   the form with a synthetic project in an isolated workspace.
7. Docs: `docs/connections.md` (dbt section, mapping, match summary,
   limitations: one project, manifest only, Qrow does not run dbt).

## Phase 4: dbt data for the assistant

1. **Describe table** (`src/assistant/catalog.rs` `relation`): when the
   relation matches a dbt entry, add a `dbt` object: unique ID, resource
   type, materialization, description, tags, per-column descriptions,
   tests (keys from `unique`/`not_null`, `accepted_values` values,
   `relationships` targets with the mapped relation name), direct parents
   and children (relation names when they match, otherwise unique IDs),
   manifest time. Keep the existing byte limits (`MAX_TEXT_BYTES`, total
   result size) and add truncation flags.
2. Per-message context (`CatalogContext::new`): for the referenced relations,
   add a short dbt summary (description, keys, relationships) within the same
   16 KB budget or a separate small budget. Also add the project state:
   attached, manifest time, model count, match count.
3. Increase `ASSISTANT_DATA_SHARING_NOTICE_VERSION`; the notice says that Qrow
   sends dbt descriptions, tests, lineage, and (Phase 5) model SQL.
4. Base instructions: use dbt descriptions and tests for meaning and joins;
   use relationships tests for join keys; say when the manifest is old.
5. Tests and docs (`docs/assistant.md` **Look up tables and columns**).

## Phase 5: New assistant tools, sidebar, and editor

1. Tools (`src/assistant/tools.rs`), all read-only and paged:
   - `search_dbt_models`: by name, description text, tag, resource type;
     returns unique IDs, relation names, and short descriptions.
   - `describe_dbt_model`: everything in Phase 4 for an entry that has no
     catalog match (for example an ephemeral model or a missing table).
   - `read_dbt_model_sql`: raw or compiled SQL, read on demand from the byte
     spans in the manifest, with the 32 KB paging of `read_tab_sql`. If the
     manifest changed after the index was built, refresh first. Tell when
     the manifest has no compiled SQL (a manifest from `dbt parse`).
   - `dbt_lineage`: upstream or downstream to a depth limit.
   - `list_dbt_metrics` / `describe_dbt_metric`, if the project has a semantic
     layer.
2. Sidebar (`src/ui/catalog_tree.rs`): a dbt badge and the materialization on
   matched relations; description in the tooltip.
3. Tab action: **Open Compiled SQL** for a matched relation, in a new tab.
4. Editor hover and completion with dbt descriptions: after Qrow has
   autocomplete. Do not resolve `{{ ref() }}` in the editor for now.

## Open questions

- File watching: `notify` (FSEvents) or `stat` polling. Decide from the
  Phase 2 numbers.
- Binary index format: `rkyv` (preferred) or `postcard`/`bincode`. Decide from
  the Phase 2 numbers.
- Skip a rebuild when only manifest `metadata` changed: only if Phase 2 shows
  that it saves time.
- `catalog.json` (from `dbt docs generate`) is out of scope for now. Qrow's
  schema catalog already has the column types.

## Verification for each phase

- `./qtest` checks for the change; Docker native E2E where it applies.
- Synthetic projects and credentials only; isolated workspaces.
- Computer Use for each UI change.
- A cross-model review on each commit.
- Update docs in the same change.

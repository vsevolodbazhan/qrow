# Editor: completion, formatting, style lint

Make the SQL editor more than highlighting, with three optional features:
completion from the catalog, formatting on demand, and style linting.

## Decisions

- **Scope:** catalog completion, Format SQL on demand, style lint. Out of
  scope: correctness checks against the catalog, server checks (`EXPLAIN`),
  fetching function lists from the server, and automatic formatting.
- **Completion uses only the catalog cache and built-in lists.** It never
  opens a session or starts a refresh by itself; only the Refresh link does.
  When the cache is missing data, the editor tells the user.
- **Formatting runs only on demand.** The assistant keeps formatting the SQL
  it inserts (`broker.rs`), because that is its own output.
- **Spark (Kyuubi) is the first dialect.** Postgres and Trino follow with the
  same design.
- **The package size budget may increase** for this feature. Measure the
  increase and record the reason in `scripts/core/size.py`.
- No external language server process. Implement the GPUI Kit editor
  providers (`vendor/gpui-base/src/input/editor/lsp/`) in Qrow.

## Design

- A core module `src/sql/analysis/` with no UI. Input: dialect, text, cursor,
  and a catalog snapshot (`Arc`). Output: completions and diagnostics. Unit
  tests run without GPUI.
- The UI attaches a `CompletionProvider` to each tab editor and writes style
  diagnostics into the editor's `DiagnosticSet`.
- The provider returns a ready `Task`: completion is synchronous over a name
  index per catalog snapshot. Rebuild the index when the UI receives a
  catalog event (`src/ui/catalog_tree.rs`), not on each key press. Measure
  the rebuild cost while a connection refresh publishes many updates.
- Style lint runs on the background executor after a debounce, one statement
  at a time. The cache key is dialect, rule set, and statement text.
  Diagnostic ranges are relative to the statement start and move with it.
  Results for an old editor revision are dropped.

## Phase 0: sqruff spike

1. Add `sqruff-lib` in a branch. Measure the executable and zip size change.
   It enables all 26 dialects by default; check if a vendored patch can limit
   it to `sparksql`, `postgres`, and `trino`.
2. Measure parse and lint time on a synthetic Spark corpus (short queries, a
   500-line query, a 50-statement tab).
3. Count false parse errors on valid Spark syntax: backticks, `LATERAL VIEW`,
   lambdas (`x -> x + 1`), struct fields, `INSERT OVERWRITE`, hints, `CACHE
   TABLE`. Use synthetic SQL only.
4. Result: go or no-go for sqruff. If no-go, report the alternatives before
   phase 3.

## Phase 1: Format SQL on demand

1. Action **Format SQL** (**⌥⌘L**, plus a menu item). Selection if any,
   else the statement at the cursor.
   A selection with several statements formats each statement; text between
   them stays as it is.
2. `format_statement` returns `Option` today, and `None` means too short,
   unsupported statement, unchanged, or rejected. Return a distinct outcome
   for each, and add a manual mode without the 80-character minimum. Keep the
   minimum at both assistant call sites (`broker.rs` insert and edit).
3. Keep the `same_tokens` guard. If the guard rejects the result, keep the
   text and show a short notice. An unsupported statement shows its own
   notice.
4. One undo step restores the original text.
5. `same_tokens` uses the Spark lexer and Postgres layout rules. Before
   Postgres and Trino, check their tokens (dollar quotes, double-quoted
   names) and the dialect setting.
6. Tests: literals, comments, hints, quoted names, joined operators (`>=`),
   unsupported statements, multi-statement selections, undo, and the
   assistant minimum.
7. Docs: `queries.md`, `workspace.md` (Keyword Case now also applies here).

## Phase 2: catalog completion (Spark)

Approved mockups: [editor-intelligence-mockups.html](editor-intelligence-mockups.html).

1. Context scanner on the existing lexer, over the statement at the cursor:
   clause, qualifier before the cursor (`alias.`, `schema.`), relations in
   scope with aliases, and CTE names.
2. Default schema: the connection's initial database. Do not infer `USE db`
   from editor text; the text may not have run, may have failed, or may
   belong to an earlier session.
3. Candidates: schemas, relations, columns, keywords, and the built-in Spark
   function list. A column row shows its type only as `detail`. Rank columns
   of in-scope relations first, then prefix matches, then fuzzy matches.
4. Each item sets `text_edit` with the range of the typed prefix. With only
   `insert_text`, the editor inserts at the cursor and keeps the prefix.
   Quote names that need backticks.
5. Side panel of the selected item (`documentation`): the fully qualified
   name (`sales.orders.amount`), then the comment, or "No description" when
   the catalog has none. Escape catalog comments before they go into
   Markdown. In a narrow window the menu stacks the panel and keeps only its
   first line; patch it to keep the description.
6. Incomplete catalog: a note row at the bottom of the completion menu, as
   a plain sentence. The menu opens with only the note when there are no
   items. Keyboard navigation skips the note. One note per missing object;
   with several, the first and a count ("and 2 more").

   | Cache state (`src/catalog.rs`) | Note | Refresh link does |
   |---|---|---|
   | Relation `columns: None` | Columns of sales.orders are not loaded. Refresh. | `Scope::Relation` |
   | Relation has `error` | Columns of sales.orders could not be loaded. Refresh. | `Scope::Relation` |
   | Relation unknown, schema relations loaded | sales.orders_v2 is not in the catalog. Refresh. | `Scope::Relation` (adds it if it exists) |
   | Schema `relations: None` | Tables of sales are not loaded. Refresh. | `Scope::Schema` (also reads all columns) |
   | Schema hidden by filters | sales is hidden by the catalog filters. | no link |
   | Schema unknown | sales is not in the catalog. | no link: a schema refresh needs the schema in the cache |
   | Relation `columns: Some([])` | no note | — |

7. **Refresh** is a link. Like Refresh in the catalog tree
   (`src/ui/catalog_tree.rs`), it may open a session with the credentials of
   the catalog member. While it runs, the note shows "Loading…". When the
   catalog event arrives, if the menu is still open and the cursor has not
   moved, compute the items again and call `present_completion_items`.
   Keep the selected item. If the relation still does not exist, the note
   says so ("sales.orders_v2 was not found."). Failures show the catalog
   error.
8. Menu patches (record them in `QROW-PATCH.md`): a note slot outside the
   selectable list, set through the overlay host, not a fake completion
   item; open with only the note, in all three paths that close an empty
   menu (`gpui-base/.../lsp/completions.rs`, `.../lsp/overlay.rs`,
   `gpui-component/.../completion_menu.rs`); keep the full side panel in
   the stacked layout.
9. Setting: **Autocomplete** (default on).
10. Docs: replace the "not available" statement in `queries.md`; describe the
   cache as the source and its limits.

## Phase 3: style lint (sqruff)

1. A curated rule set, not all SQLFluff rules. Skip layout rules that Format
   SQL already covers.
2. Lint only statements without parse errors; do not show sqruff parse
   errors as diagnostics.
3. Quick fixes as code actions where sqruff offers a fix. This needs both a
   `CodeActionProvider` and its executor; one fix is one undo step. Check
   the editor revision before applying.
4. Clear diagnostics when the setting is off, the tab changes dialect, or
   the statement no longer exists. Bound the cache size.
5. Setting: **Style Lint** (default off).

## Tests

- Unit tests for the scanner, ranking, replacement ranges (mid-word,
  qualifiers, backtick names, non-ASCII), and lint mapping on synthetic SQL.
- UI integration tests: the completion menu, each note state, a note-only
  menu, keyboard navigation past the note, a narrow window, late catalog
  updates after the menu closes or the tab changes, Format SQL, and lint
  diagnostics.
- Completion with no session never starts a session or a refresh.
- End-to-end tests in Docker (Kyuubi): Refresh from the note on a cold
  connection, refresh failure, cancel, and a shared catalog with several
  members.
- Perf probes: completion latency with 5,000 relations, index rebuild during
  a connection refresh, lint time per statement, and the package size
  change.

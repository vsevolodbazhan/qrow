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
- Completion is synchronous over a name index per catalog snapshot. Rebuild
  the index on catalog `Updated` events, not on each key press.
- Style lint runs on the background executor after a debounce, one statement
  at a time, with results cached by statement text. Stale results are
  dropped.

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
2. Remove the 80-character minimum for this action. Keep it for the
   assistant path.
3. Keep the `same_tokens` guard. If the guard rejects the result, keep the
   text and show a short notice.
4. One undo step restores the original text.
5. Docs: `queries.md`, `workspace.md` (Keyword Case now also applies here).

## Phase 2: catalog completion (Spark)

Approved mockups: [editor-intelligence-mockups.html](editor-intelligence-mockups.html).

1. Context scanner on the existing lexer, over the statement at the cursor:
   clause, qualifier before the cursor (`alias.`, `schema.`), relations in
   scope with aliases, and CTE names.
2. Default schema: the connection's initial database, changed by `USE db`
   earlier in the tab.
3. Candidates: schemas, relations, columns, keywords, and the built-in Spark
   function list. A column row shows its type only as detail. Rank columns of
   in-scope relations first, then prefix matches, then fuzzy matches.
4. Side panel of the selected item: the fully qualified name
   (`sales.orders.amount`), then the comment, or "No description" when the
   catalog has none.
5. Incomplete catalog: when the cache has no columns for a relation in scope,
   or does not know the relation, show a note row at the bottom of the
   completion menu, as a plain sentence: "Columns of sales.refunds are not
   loaded. Refresh." or "sales.orders_v2 is not in the catalog. Refresh."
   The menu opens with only the note when there are no items. Keyboard
   navigation skips the note.
6. **Refresh** is a link. Missing columns: refresh that relation. Unknown
   relation: refresh the relation list of its schema. Like Refresh in the
   catalog tree, it may open a session. While it runs, the note shows
   "Loading…"; when the catalog updates, the menu queries again and shows the
   new items in place.
7. Items 5 and 6 need a small patch to the vendored completion menu
   (`vendor/gpui-component/src/input/popovers/completion_menu.rs`); record it
   in `QROW-PATCH.md`.
8. Setting: **Autocomplete** (default on).
9. Docs: replace the "not available" statement in `queries.md`; describe the
   cache as the source and its limits.

## Phase 3: style lint (sqruff)

1. A curated rule set, not all SQLFluff rules. Skip layout rules that Format
   SQL already covers.
2. Lint only statements without parse errors; do not show sqruff parse
   errors as diagnostics.
3. Quick fixes as code actions where sqruff offers a fix.
4. Setting: **Style Lint** (default off).

## Tests

- Unit tests for the scanner, ranking, and lint mapping on synthetic SQL.
- UI integration tests for the completion menu and its footer, Format SQL,
  and lint diagnostics.
- Perf probes: completion latency with 5,000 relations, lint time per
  statement, and the package size change.

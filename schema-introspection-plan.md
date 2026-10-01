# Schema introspection

Cache the schemas, relations, and columns of each connection. Show them in a
tree in the sidebar and give them to the assistant.

## Decisions

- A refresh that Qrow starts by itself never opens a session on a cold
  connection. Automatic refresh runs only while a tab of the connection has a
  live session. An explicit **Refresh** can open a session, just like **Run**.
- Each connection has more than 1,000 tables. Every fetch is limited to one
  schema or one relation. No single call covers the whole catalog.
  Include/exclude schema filters and a tree search field ship in phase 1.
- There is one catalog (`spark_catalog`). The tree has no catalog level. The
  cache has a format version; a catalog level can bump it, and Qrow then reads
  the schemas again.
- The tree replaces the Connections list. Each connection is a root node
  (DataGrip style).

## Phase 1: metadata, cache, manual refresh, tree (implemented on feat/schema-introspection)

### Connector
1. Done differently: one `Session::execute_metadata(MetadataRequest)` starts a
   catalog request as the current operation. The existing poll, columns, and
   fetch read its rows, with JDBC `DatabaseMetaData` column names.
2. Implement them in `HiveSession` with `GetSchemas`, `GetTables`, and
   `GetColumns`. Use a separate operation handle so a catalog call never
   replaces a result cursor. Names go to the server unchanged (`_` is a
   pattern there, and escapes differ between Spark versions); the parser keeps
   only exact names.
3. If `GetColumns(schema, '%')` fails (for example on a broken view), call it
   again for each relation. Record the error on the relation that fails, so
   the rest of the schema still loads.

### Catalog model (`src/catalog.rs`, core library, no UI)
4. `Catalog { profile_id, connection_fingerprint, schemas: BTreeMap<String,
   SchemaNode> }`. Each node keeps `fetched_at: Option<SystemTime>` and
   `error: Option<String>`. Relations keep columns as `Option<Vec<_>>`, where
   `None` means not loaded.
5. Merge rules: a refresh of a relation list replaces the list, but keeps the
   loaded columns of relations that still exist. A schema refresh replaces its
   relations and their columns. Schemas that the filters exclude are hidden at
   once and removed at the next save.
6. Filters: `include` and `exclude` lists of glob patterns (`*`, `?`) that
   match schema names without case sensitivity. An empty include list means
   all schemas.

### Cache storage (`src/storage.rs`)
7. Store one file per profile at `<data dir>/catalog/<profile-uuid>.json`.
   Follow `QROW_DATA_DIR`. The file has a format version; Qrow discards a file
   with an unknown version. Write to a temporary file and rename it, on a
   background thread. The workspace lock already protects the data
   directory.
8. Load a profile's cache only when the profile is first shown or used.
   Delete the file when the profile is deleted. Clear the cache when
   `connection_identity_eq` (`src/model.rs`) reports a change to host, port,
   username, or session parameters. A change to the initial database does not
   clear it.

### Catalog worker (`src/catalog/worker.rs`)
9. Create one worker thread per profile, only when it is first needed. Its
   commands are `Refresh(Scope)`, `Cancel`, `UpdateProfile`, `SetWarm(bool)`,
   and `Shutdown`, with `Scope = Connection | Schema(name) | Relation(schema,
   name)`. Its events are `Started(scope)`, `Progress { done, total }`,
   `Updated(scope, delta)`, `Failed(scope, message)`, and `Finished`.
10. The worker opens its own session with the profile parameters and closes
    it when its queue is empty. It never touches a tab's idle or keep-alive
    timers. It cancels through the existing separate cancel transport.
11. What each scope fetches:
    - **Connection:** the schema list, then a schema refresh (relations and
      columns) for each schema, one schema at a time, with progress events.
      Changed after user testing: the user expects columns by default.
    - **Schema:** relations, then `list_columns(schema, None)`.
    - **Relation:** `list_columns(schema, Some(rel))`.

    The worker merges queued requests: a schema refresh absorbs the relation
    refreshes of that schema. A connection refresh reads no columns, so it does
    not absorb them.
12. Profile edit, profile delete, and quit cancel work in progress. Quit
    waits for the worker, sharing the existing 2 s shutdown budget.

### Tree (`src/ui/catalog_tree.rs`)
13. Replace the Connections list in `workspace_view.rs` with GPUI Kit `Tree`
    / `TreeState` (vendored at `vendor/gpui-base/src/tree.rs`, virtualized).
    Build `TreeItem` children only for expanded nodes. A collapsed node that
    has children gets one placeholder child, so it renders as a folder. Use
    `TreeEvent::Expanded` to build the children.
14. **Spike first:** check that a root row can keep everything it has today:
    - Selecting the profile on click, separately from expand and collapse.
    - The query busy spinner and the unread-error icon.
    - The accessibility label and tooltip.
    - The current right-click menu.

    If `Tree` cannot separate click from toggle, add a disclosure button to
    the row and tell me before any further work.
15. Rows:
    - **Connection:** the existing content. During a connection refresh, its
      first child is a progress row such as "Loading schemas… 12/140".
    - **Schema:** shows the relation count.
    - **Relation:** a table or view icon. The comment is in the tooltip.
    - **Column:** the name, with the type in muted text.

    The node under refresh shows a spinner. A node with an error shows a
    warning icon, and its tooltip has the message. On a cold connection, an
    unloaded node shows one row: "Not loaded · Refresh".
16. Expanding a relation with no loaded columns fetches them only while the
    connection is warm. Otherwise the row shows "Not loaded · Refresh".
17. Context menus:
    - **Connection:** the existing Edit, Duplicate, and Delete items, plus
      **Refresh** (or **Stop Refresh** while one runs).
    - **Schema:** **Refresh** and **Copy Name**.
    - **Relation:** **Refresh**, **Copy Qualified Name**, and **Insert into
      Editor**.
    - **Column:** **Copy Name**.

    A double-click on a relation or column inserts its name at the cursor.
18. A search field above the tree matches relation and schema names in the
    whole cache, not only expanded nodes. It expands the paths that match and
    shows at most 500 matches, with a "Refine your search" row after them.

### Profile form
19. Add a **Schemas** section to `connection_form.rs` with the include and
    exclude patterns. Changes to these fields, like the Lifecycle fields, do
    not release sessions.

## Phase 2: refresh while connected (implemented on feat/schema-refresh-policy)

Both the refresh period and the refresh timeout are configurable for each
connection (user decision, 2026-10-01).

Schema browsing is off by default (user decision, 2026-10-01). The
**Schema refresh** choice is the first field of the Schemas section:
**Disabled** (the default for new connections), **Manual**, or **While
connected**. Disabled hides the tree under the connection, its refresh menu
items, and the other Schemas fields. Saved connections keep their behavior:
settings saved with the schema tree but without a choice use Manual.

20. Add `CatalogPolicy` to `CatalogSettings`, with `#[serde(default)]`:
    - `refresh: Manual | WhileConnected { minutes }`. The default is
      `WhileConnected { minutes: 60 }`. Valid periods are 5 to 10,080
      minutes.
    - `timeout_minutes`: the longest time of one connection refresh, manual
      or automatic. The default is 30 minutes. Valid values are 1 to 1,440
      minutes.
21. Form, in the **Schemas** section, like **When idle**: a **Schema refresh**
    dropdown (**Manual** / **While connected**), a **Refresh period** field in
    minutes that shows only for **While connected**, and a **Refresh timeout**
    field in minutes. Saving these fields does not release sessions.
22. The UI sends `SetWarm(true)` when any tab of the profile reaches
    `Connected`, and `SetWarm(false)` when the last live session of the
    profile ends. While the connection is warm, the worker refreshes at the
    deadline `catalog.fetched_at + period`, and also as soon as the
    connection becomes warm if the cache is stale. While the connection is
    cold, the deadline stays pending and does not wake the thread.
23. The automatic refresh is a connection refresh: the schema list, then the
    relations and columns of each schema (as in phase 1 after user testing).
    It reads the schemas in order of their oldest `fetched_at` first, so a
    refresh that the timeout stops does not leave the same schemas stale each
    time.
24. Timeout: the worker stops a refresh when its time passes. It cancels the
    request in progress through the cancel transport, keeps everything that it
    read, records "Refresh stopped after N minutes" on the connection, and
    logs it when **Schema refresh logs** is enabled. Schema and relation
    refreshes have the same limit.
25. Tests: deadline calculation (warm, cold, stale, policy change), the
    oldest-first order, and the timeout with a blocking fake server (the
    catalog keeps the schemas read before the limit). UI test for the form
    fields and their validation. Docs in `connections.md`.

## Phase 2b: shared catalogs (implemented on feat/shared-catalogs)

Connections that read the same metastore through different users or compute
clusters (for example analytics-s and analytics-m) can share one catalog.
Decisions from the user, 2026-10-01: explicit membership, any member can
refresh, one refresh at a time, scan settings belong to the shared catalog,
and an explicit preferred member for automatic refreshes.

26. Model: `SharedCatalog { id, name, settings: CatalogSettings,
    preferred: Option<Uuid> }` in the workspace. `Profile` gets
    `shared_catalog: Option<Uuid>`; `None` keeps a private catalog (the
    default and the phase 1 behavior). Bump the workspace version.
27. Settings that decide what a refresh reads move to the shared catalog for
    its members: **Show schemas**, **Hide schemas**, the refresh period, and
    the refresh timeout. **Schema refresh logs** stays per connection, because
    it only decides where entries go. A profile keeps its own values for when
    it leaves.
28. Cache and worker: one cache file `catalog/<shared-id>.json` and one worker
    for each shared catalog. The worker queue runs one refresh at a time and
    merges covered scopes across members. Each refresh runs with the profile
    of the member that asked for it (its session, credentials, and compute).
    The cache identity is the shared catalog: editing a member's host or user
    does not clear it, because membership states that the members read the
    same metastore.
29. Status belongs to the member that runs the refresh. The connection-row
    spinner keeps its meaning: this connection is up and in use. Only the
    running member shows the spinner, the "Loading schemas… N/M" row, and a
    connection-level failure. A member whose request waits shows "Waiting…".
    Other members show no refresh state; their trees show the shared data as
    it arrives. Errors on schema or relation nodes are catalog data, so every
    member shows them.
30. Automatic refresh (phase 2 policy, from the shared settings): when the
    period is due and a member has a live session, run it with the
    **Preferred for refreshes** member if that member is live, else with the
    first live member in sidebar order. It never opens a session on a member
    without one.
31. Form: a **Catalog** dropdown (**This connection** / shared catalogs /
    **New shared catalog…**). For a member, the Schemas section edits the
    shared settings and shows the shared catalog name and the **Preferred for
    refreshes** choice. A note says that members must read the same metastore
    with the same permissions and Spark catalog, because Qrow cannot check it.
32. Membership changes: a connection that leaves gets an empty private
    catalog. A shared catalog without members is deleted with its cache file.
    Deleting the preferred member clears the preference.
33. Logs entries go to the tab of the member that runs the refresh. The
    phase 3 assistant tools read the shared catalog for any member.
34. Tests: worker queue across members (one at a time, merged scopes, each
    refresh with its own profile), status only on the running member,
    preferred-member selection and fallback, membership changes and cache
    deletion, workspace migration. UI tests for the form and for status
    placement. Docs in `connections.md` and `workspace.md`.

## Phase 3: assistant

23. Add three read-only tools in `src/assistant/tools.rs` and
    `src/ui/assistant_tools.rs`. Each result has a byte limit, plus
    `fetched_at` and `stale`:
    - `list_schemas`.
    - `list_relations(schema, pattern?, offset, limit)`.
    - `describe_relation(schema, relation)`: if the columns are not loaded
      and the connection is warm, it fetches them; this follows the async
      completion of `tool_fetch`. Otherwise it returns `not_cached` with a
      hint to ask the user to refresh, or to use `DESCRIBE` through
      `run_selected_tab_query`, which keeps query approval.
24. Workspace context: add `catalog: {loaded, fetched_at, schema_count,
    relation_count}` for the connection of the conversation. Also add the
    cached columns of the relations that the tab SQL references, at most
    16 KB. To find them, match names against the cache; this is best-effort.
25. Add one line to `BASE_INSTRUCTIONS`: look up relations and columns with
    the catalog tools instead of guessing them. Older conversations do not
    get the new tools; document this.
26. Increase `ASSISTANT_DATA_SHARING_NOTICE_VERSION` and update the notice:
    schema, table, and column names, and their comments, go to the model
    provider.

## Tests

- **Unit:** merge rules, glob filters, cache version handling and atomic
  write, invalidation from `connection_identity_eq`, deadline calculation
  (warm, cold, stale, policy change), request merging.
- **Integration (local protocol fixture, `tests/integration/hive_protocol.rs`):**
  the metadata requests, pattern escaping, and the fallback to one
  `GetColumns` call per relation.
- **Worker:** with a fake connector: the scopes, cancellation, no session
  while cold, the refresh when a stale connection becomes warm, the session
  closes after its queue is empty, and quit stays inside its budget.
- **Backend (Docker Kyuubi):** each test seeds its own schema
  (`qrow_catalog_<uuid>`) with a table, a view, comments, a partitioned
  table, and a broken view. Check the decoded types and the per-relation
  error.
- **UI (headless, seeded cache, no servers):**
  - The root row still selects the profile and keeps its spinner, error
    icon, and menu.
  - Expand, search with its 500-match limit, context menus, and
    double-click insertion.
  - Update the existing `tests/ui/connections.rs` selectors.
- **End-to-end (Docker):**
  - The first Run on a stale profile fills the tree.
  - `ALTER TABLE … ADD COLUMNS` followed by a relation refresh shows the new
    column.
  - Phase 3: the assistant tools through `tests/support/assistant.rs`.
- **Performance:**
  - `perf-ui`: frame timings for the tree with a synthetic 2,000 relations ×
    40 columns.
  - `perf-app`: idle memory before and after, against the README number.
- **Manual check of the UI with Computer Use** for each phase.

## Docs

- `connections.md`: the tree, Refresh, filters, and the policy.
- `architecture.md`: the catalog worker and the cache.
- `workspace.md`: the cache files, which are separate from the workspace.
- `assistant.md`: the tools and data sharing (phase 3).
- `testing.md`: fixture seeding, if it changes.
- `docs/README.md`: the index.

Known limitations to document:
- With Kyuubi share level `CONNECTION`, a catalog session starts its own
  engine.
- The tree does not show temporary views from tab sessions.
- The tree does not mark partition columns, because `GetColumns` does not
  report them.

## Delivery

- One draft PR per phase, branched from `main` (not `perf/idle-memory`).
- Verification gap: the time `GetTables` takes on the real warehouse with
  more than 1,000 tables. Ask the user to time the first connection refresh;
  do not use their credentials in tests.

# Results

Qrow displays a bounded preview of query results. Rows appear as they arrive.
The preview does not change the SQL sent to Spark.

## Browse and copy

Each page contains up to 1,000 rows. **Next** fetches another page when needed.
**Previous** and **Next** reuse downloaded pages without executing SQL again.
You can browse downloaded pages while a fetch runs.

Row numbers refer to the full result. If Next finds no more rows, the last
populated page stays visible. Changing pages clears the selection and resets
the scroll position. New batches preserve the current scroll position.

Scroll vertically to see more rows and horizontally to see more columns.
Drag a column boundary to change its width. Drag the divider above Results
to change the editor height.

Right-click a cell to use **Copy cell** or **Copy row**. Copy actions use full
stored values from the displayed page. Long cells show a shortened preview,
but their stored and copied values are not shortened. File export is not
implemented.

## Value representation

| Value | Display |
| --- | --- |
| Null | `NULL` |
| Empty string | An empty cell |
| Decimal or textual timestamp | The server's text representation |
| Binary | Hexadecimal text |
| Nested value | The text returned by HiveServer2 |

Qrow preserves the distinction between null and an empty string. It does not
convert exact decimal text to floating-point values for display.

## Preview limits

The preview can store up to 100,000 rows or approximately 64 MiB per tab.
If an incoming batch would exceed either limit, Qrow discards that batch and
closes the cursor. Previously downloaded rows remain available. The memory
limit measures retained row storage, not the total application memory.

Qrow fetches batches of up to 250 rows. It does not add a SQL `LIMIT` clause.
Client preview limits do not guarantee less server work. A separate 64 MiB
transport frame limit can also reject large server responses.

Cancellation and fetch failures retain downloaded rows. Disconnecting releases
unfetched rows. A new query or a profile switch clears the previous preview.
Results are not restored after application restart.

## Design

The [worker](../src/worker.rs) bounds fetching and storage. The
[HiveServer2 connector](../src/connector/hive.rs) uses an empty fetch to confirm
the end of results. Some servers report `hasMoreRows` incorrectly, so that field
alone cannot safely determine whether fetching is complete.

[Pagination](../src/pagination.rs) selects a range of downloaded rows. The
[results UI](../src/ui/results.rs) owns rendering, selection, scrolling, and
clipboard behavior. It virtualizes both rows and columns to keep wide result
sets responsive. Table overlays and scrollbars stay inside the table container.

The UI includes explicit horizontal-wheel and splitter-release handlers.
Changing these handlers requires pointer checks. Verify dragging, release, the
next click, both scroll axes, and modal overlays. Earlier automation could not
establish horizontal trackpad behavior or column resizing. A passing native
build does not close those verification gaps.

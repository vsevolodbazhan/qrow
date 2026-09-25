# Results

Qrow displays a bounded preview of query results. Rows appear as they arrive.
The preview does not change the SQL sent to Spark.

Select **Results** beside **Logs** to view the current preview. Qrow keeps
one Results panel. A new execution replaces the preview. The [Queries](queries.md)
page describes the Logs panel.

If a result has no columns, the panel shows a status message without a table
header. Column headers remain visible when a query returns columns but no rows.

## Browse and copy

Each page contains up to 1,000 rows. **Next** fetches another page when needed.
**Previous** and **Next** reuse downloaded pages without executing SQL again.
You can browse downloaded pages while a fetch runs.

Row numbers refer to the full result. If Next finds no more rows, the last
populated page stays visible. Changing pages clears the selection and resets
the scroll position. New batches preserve the current scroll position.

Scroll vertically to see more rows and horizontally to see more columns.
The scrollbars overlay the right and bottom edges of the table.
Drag a column boundary to change its width. Drag the divider above Results
to change the editor height.

Right-click a cell to use **Copy Cell** or **Copy Row**. Copy actions use full
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
Client preview limits do not guarantee less server work. Each server response
has a 64 MiB byte limit across all transport frames. Each frame also has a
64 MiB limit. Before the decoder allocates strings or containers, it checks
the declared sizes against a separate 64 MiB allocation budget. Container
estimates are conservative and can reject a response below the byte limit.

Before it builds display rows, the connector checks the requested row count,
column lengths, and expanded storage size. This includes binary values that
expand to hexadecimal text. A rejected response leaves downloaded rows
available and requires reconnection. These limits do not cap total application
memory. Encoded values, display rows, and other tabs can occupy memory at the
same time.

Cancellation and fetch failures retain downloaded rows. Disconnecting releases
unfetched rows, but the downloaded rows remain visible. Selecting another
connection keeps the preview and its session. **Next** can fetch remaining
rows from that session, even when another connection is selected. The next
accepted query replaces the previous preview, regardless of the selected
connection.

Results are not restored after application restart. Logs history is separate and
is also not restored after application restart.

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

# Results

Qrow displays a bounded preview of query results. Rows appear as they arrive.
The preview does not change the SQL sent to Spark.

Each query tab has a tab strip below the editor. The strip starts with
**Logs**, followed by one or more result tabs. Select a result tab to view
its preview. The [Queries](queries.md) page describes the Logs panel.

## Result tabs

A new execution fills an unpinned result tab and shows it. Qrow uses the shown
result tab if it is not pinned. Otherwise, it uses the last unpinned result
tab. If all result tabs are pinned, Qrow opens a new result tab. New result
tabs get the names **Result 1**, **Result 2**, and so on.

To keep a result, click **Pin result** in the result toolbar, or right-click
the result tab and choose **Pin**. A pinned result tab shows a pin icon. A
query tab can pin up to 10 result tabs. Each one can hold a full preview, so
the limit bounds memory use. Click **Unpin result** or choose **Unpin** to let
the next execution replace the result.

Right-click a result tab for these commands:

- **Rename…** gives the result tab a name. The name must be unique in the
  query tab. Renaming also pins the result tab, if the pin limit allows it.
- **Pin** or **Unpin** changes whether the next execution can replace it.
- **Copy query** copies the SQL statement that made the result.
- **Close** removes the result tab and its downloaded rows.

**Copy query** in the result toolbar also copies the statement. It copies the
statement that Qrow sent, not the current editor text. Point to a result tab
to see its name and the start of its statement.

A query tab always has at least one result tab. Closing the last result tab
creates an empty replacement. The result tab of a running query cannot close.

If a result has no columns, the panel shows a status message without a table
header. Column headers remain visible when a query returns columns but no rows.

## Browse and copy

Each page contains up to 1,000 rows. **Next page** fetches another page when needed.
The bar above the table shows neutral tags for the visible row range,
downloaded row count, column count, and query duration. **Loaded** counts the
rows in the preview. It does not give the total number of rows on the server.
The tag tooltips use the same names as the rows in **Result Details**:
**Visible Rows**, **Loaded Rows**, **Columns**, and **Query Duration**.
When the tags do not fit, the bar keeps the row range and shows an information
button. Click **Show result details** to read all four values as neutral tags.
The heading uses semibold text. At the smallest widths, the row range also
moves into **Result Details**. The page buttons show arrows at all widths.
Their tooltips show **Previous page** and **Next page**.
At large UI scales, a wide sidebar can leave too little space for the page
buttons. Reduce the sidebar width if the buttons do not fit.

With focus in the toolbar, use Tab to focus **Show result details**.
Press Enter or Space to open it.
Press Escape or click outside to close it. Escape returns focus to the button.
The values update as rows arrive. The query duration is absent before the
first query.
**Previous page** and **Next page** reuse downloaded pages without
executing SQL again.
You can browse downloaded pages while a fetch runs.

Row numbers refer to the full result. If **Next page** finds no more rows,
the last populated page stays visible. Changing pages clears the selection and resets
the scroll position. New batches preserve the current scroll position.

Scroll vertically to see more rows and horizontally to see more columns.
The scrollbars overlay the right and bottom edges of the table.
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

Each result tab can store up to 100,000 rows or approximately 64 MiB.
If an incoming batch would exceed either limit, Qrow discards that batch and
closes the cursor. Previously downloaded rows remain available. The memory
limit measures retained row storage, not the total application memory.

Qrow asks for each page of 1,000 rows in one request. When the server sends
fewer rows, Qrow asks for the remaining rows of the page. It does not add a
SQL `LIMIT` clause.
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
connection keeps the preview and its session. **Next page** can fetch remaining
rows from that session, even when another connection is selected. The next
accepted query fills the unpinned result tab, regardless of the selected
connection.

The session of a query tab keeps one open cursor. Only the result tab of the
last execution can fetch more rows. The next execution closes the cursor of the
earlier result, so a pinned result keeps only its downloaded rows. When a
cursor closes before the server sends all rows, the result toolbar shows a
**Partial** tag. Run the query again to load more rows.

Result tabs and their rows are not restored after application restart. Logs
history is separate and is also not restored after application restart.

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

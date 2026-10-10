# Results

Qrow displays a bounded preview of query results. Rows appear as they arrive.
The preview does not change the SQL sent to the server. Postgres can show the
first page while SQL runs. The tab stays busy until the server finishes.
Trino shows the first page while it reads the result stream.
See [Postgres limits](connections.md#postgres-limits) and
[Trino limits](connections.md#trino-limits).

Select **Results** beside **Logs** to view the current preview. Qrow keeps
one Results panel. A new execution replaces the preview. The [Queries](queries.md)
page describes the Logs panel.

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

### Select and copy cells

Select a rectangle of cells on the current page, then press ⌘C to copy it.

| Action | Selects |
| --- | --- |
| Click a cell | That cell. |
| Drag from a cell | The rectangle from that cell to the pointer. |
| Shift-click a cell | The rectangle from the first selected cell to the clicked cell. |
| Click a column header | That column on the current page. Shift-click another header to add the columns between. |
| Click a row number | That row. Shift-click another row number to add the rows between. |
| Click the `#` header, or press ⌘A in the table | All cells of the current page. |
| Arrow keys | The next cell. Shift with an arrow key extends the rectangle. |
| Home, End | The first or last cell of the row. |
| Page Up, Page Down | The cell one screen up or down. |

The selected cells use the selection color. In a rectangle, the cell that the
keys move also has a focus outline. Escape clears the selection. A click
outside the results area or a page change also clears it.

During a drag, keep the pointer at an edge to scroll rows or columns.
Release the mouse button to finish the selection, including outside the table.
The rectangle stays on the current page and excludes the row numbers.

⌘C copies one cell as its value. It copies a rectangle as tab-separated text
without column names, one line for each row, so it pastes into a spreadsheet as
cells. A value that contains a tab, a line break, or a double quote is quoted,
and its double quotes are doubled. A null copies as `NULL`.

Right-click a cell for **Copy cell**, **Copy selection**, or **Copy row**. A
right-click inside the selection keeps it. A right-click outside selects that
cell. **Copy row** copies the whole row, even when the selection is narrower.

Copy actions use full stored values from the displayed page. Long cells show
a shortened preview, but their stored and copied values are not shortened.
Use **Copy as → CSV** to copy the selection with column names and the saved
CSV options. A saved tab separator changes to a comma for this action.
Use **Copy as → TSV** to copy it with tab separators and column names.
Use **Copy as → Markdown** or **Copy as → JSON** for the other text formats.
These actions run in the background. Text above 10 MiB must be saved to a file.

## Export downloaded rows

Click **Export results…** in the Results toolbar. The button is available
when the result has columns. Choose a format, then **Selection** or **Downloaded rows**.
The selection includes only the selected rows and columns. Downloaded rows
includes all pages that the preview has stored.

The dialog takes a snapshot when it opens. A later page fetch or query does
not change these choices. Selection and downloaded-row exports do not run
SQL or fetch rows from the server.
If the preview is incomplete, the dialog shows a message.

For CSV or TSV, choose a preset, then adjust the separator, line ending, null marker,
column names, quoting, byte-order mark, and formula escaping. A change to
these options selects **Custom**. The preview shows up to five rows. During a preview update, the dialog shows
**Preparing preview…**.

| Preset | Separator | Line ending | UTF-8 byte-order mark | Formula escaping |
| --- | --- | --- | --- | --- |
| Standard | Comma | CRLF | No | No |
| Excel | Comma | CRLF | Yes | Yes |
| Excel (semicolon) | Semicolon | CRLF | Yes | Yes |
| Tab-separated | Tab | LF | No | No |

**Copy** puts up to 10 MiB of text on the clipboard. **Save…** opens the
native save panel. Tab separators use the `.tsv` extension. Other separators
use `.csv`. After a save, use **Reveal in Finder** to locate the file.
A successful copy or save stores the options for the next export. A save
also stores the last output directory.

**Cancel**, Escape, and the dialog close button stop an active export.
Quit asks for confirmation while an export runs. **Keep working** continues
the export. **Quit anyway** cancels it and waits for cleanup within a deadline.
Qrow writes a temporary file next to the destination. It replaces the
destination only after the write completes. A write error or cancellation
leaves the destination unchanged. Failed writes keep the dialog open.
An application crash can leave a `.qrow-export-*.tmp` file next to the
destination. You can delete that temporary file.

Save checks free disk space before each write. Exports on the same volume
share disk reservations. An export stops if the reserved writes would leave
less than 1 GiB free. Other programs can still use that space. A disk error
names the affected volume and leaves the destination unchanged.

Export stops if a cell exceeds 8 MiB or a row exceeds 16 MiB. These limits
apply to each format. They do not limit a server message before Qrow receives
the message.
Export also stops if the selected column headers exceed 16 MiB.

### CSV values and limits

CSV uses double quotes and doubles each quote inside a quoted value.
Separators are comma, semicolon, tab, or pipe. Null markers are an empty
field, `NULL`, `\N`, or custom text. Choose **Custom** in **Null Values** to
enter a marker. Exclude the selected separator, double quotes, and line breaks.
Invalid markers disable Copy and Save. A null marker is never quoted. An empty string
is always quoted. A literal value that equals a non-empty null marker is
also quoted. Set the CSV reader to treat only unquoted markers as null.
Some readers ignore quotes when they detect null values.

With one column and the empty null marker, a null row is an empty line.
Some CSV readers skip that line. Choose a non-empty marker to preserve the
row count. The dialog shows this warning for a one-column export.

Formula escaping adds an apostrophe before text that starts with `=`, `+`,
`-`, `@`, a tab, or a carriage return. It also applies to column names.
Numeric and Boolean values keep their server text. CSV does not change
decimal values to floating-point numbers.

Exports share immutable preview batches. Concurrent exports can retain up
to 1 GiB across retained sources and reserved writer and spool working memory.
Shared batches count once. Encoding writes
cell slices directly for text formats; it does not copy a wide cell into an escape buffer.
The clipboard limit applies while text is written.

For Kyuubi, [All rows](#export-all-rows-and-reuse-a-download) can read the
remaining cursor without the preview limits.

### Markdown

Choose **Table** for a Markdown table. Numeric columns align to the right.
The writer escapes pipes and backslashes. It changes line breaks to `<br>`.
Null values use `NULL`. Empty strings stay empty.

Choose **Code block** for a padded text table in a fence. Set **Maximum Cell
Width** from 1 to 1000 display columns. Longer cells end with an ellipsis. Line
breaks and other control characters become spaces. This changes only the
exported text. **Include row numbers** adds the result row numbers.

Markdown above 40,000 characters can exceed message limits. Copy asks you
to choose **Copy anyway** before it changes the clipboard. Save keeps the
text in a `.md` file.

### JSON

Choose **JSON array** for an array of row objects. **Pretty output** adds
indentation. Choose **JSON Lines** for one compact object on each line.
Files use `.json` or `.jsonl` respectively.

**Typed values** writes Boolean, integer, and finite floating-point values
as JSON values. Null stays `null`. Non-finite floating-point values stay
strings. Decimal values stay exact strings by default. **Decimals as numbers**
writes their exact digits as JSON numbers. Some readers change those numbers
to floating-point values. Keep decimals as strings for those readers.

Dates and timestamps keep the server text. Binary values use base64.
Nested values become JSON when their server text is valid JSON. Other nested
values stay strings. Turn off **Typed values** to keep all non-null values
as server text strings.

Repeated column names get numeric suffixes, such as `id_2`. Existing names
keep their names. Suffixes do not overwrite an existing column.

## Parquet

Select **Parquet** to save a binary file. **Copy** is disabled for this format.
Choose **Snappy**, **Gzip**, or **None** for compression. Snappy is the default.

**Typed** column types preserve booleans, integers, floating-point values,
exact decimals, dates, timestamps, and binary values. Nested values remain
text. Repeated column names follow the JSON naming rules. **Text** column
types keep every non-null value as server text.

**Numeric As** defaults to **Decimal (exact)**. For Postgres `numeric`, the
writer checks all downloaded values before it chooses one common precision
and scale. For example, `9999` and `1.2345` use `DECIMAL(8,4)`.
A numeric column remains text if its common precision exceeds 38, its scale
is outside the declared precision, all values are null, or a special value
such as `NaN` occurs. **Double (approximate)** can change numeric values.
Use **Text** to keep their exact server representation.

Timestamps with up to 6 fractional digits use microseconds. Timestamps with
7 to 9 digits use nanoseconds. Wall-clock timestamps keep their local date
and time. Timestamps with a time zone represent UTC instants. Named zones
use their time-zone rules. Hive `TIMESTAMP LOCAL TZ` also needs an offset
or zone in the result text. A missing or unexpected zone stops the export.
An unresolved daylight-saving gap or overlap
stops the export. Precision above 9, an unsupported date, or a timestamp
outside the selected integer range also stops the export. Choose **Text**
column types to keep such values without rounding.

For Postgres results, the connector records DateStyle, IntervalStyle, and
TimeZone before execution. It does not change those settings. When the result
uses a non-ISO DateStyle, dates and timestamps remain text. The export form
shows this condition.

The writer uses Parquet 1.0 data pages, dictionaries, and statistics. It
limits each row group to approximately 64 MiB of source values. A group can
close earlier to stay within the writer memory allowance. The writer prepares
one column at a time. Row-group metadata stays in memory
until the file closes and grows with the number of row groups. These
limits do not cap total application memory. File metadata contains the
application version and no query text. A failed or cancelled save preserves
an existing destination file.

## Export all rows and reuse a download

Choose **All rows**, then **Save…**, to export the complete result. A complete
preview needs no server work. For an incomplete Kyuubi result, the tab worker
reads the remaining cursor into a temporary spool. It includes the preview
and any batch held at the preview limit, in their original order. It does
not submit the SQL again. A page fetched after the dialog opened can make
its snapshot stale. Reopen Export in that case.

The tab is busy during the download. **Run** and **Next page** are disabled.
The form closes when saving starts. The results bar shows downloaded rows,
bytes, average speed, and elapsed time. The format
writer reads committed spool batches. Parquet exact decimals and Markdown
code blocks need a complete first pass. After the download, you can run
another query or close the tab while the file writer continues.

**Cancel export** in the results bar stops the writer and the download.
**Details…** opens the form without stopping the job. Closing that form
leaves the job running. The worker sends cancellation
to the operation that owns the download and completes protocol cleanup
before the session can accept another query. Cleanup has a 2-second deadline.
If cleanup fails or times out, the transport is interrupted and the session
is discarded. The deadline starts when protocol cancellation begins.
It does not limit credential lookup, token refresh, DNS lookup, or a TCP
connection that has not yet returned a socket on every connector. See the
[connection limits](connections.md) for each connector. Export jobs stay active
until the producer and writer return. A stopped drain consumes its cursor. To
get all rows after that, use **Run again and export**.

After download completion, writer cancellation and errors affect only that
file. They cannot cancel a later query. A writer error stays in the results
bar and keeps the completed spool. **Retry…** opens the form and uses that
spool without running SQL. You can change the format or destination.
**Dismiss** releases this job's handle.

**Export again as…** uses the tab's retained spool, with its row count and
size shown in the Rows list. **Settings → Export → Replay Limit** controls
the maximum retained size. The default is 2048 MiB. Zero disables retention.
A larger spool remains available to its running writer and failed job.
A new query or tab close releases only the tab's replay handle. The
anonymous spool disappears when its last handle closes, including after a
crash. Disconnecting keeps a completed replay spool but releases a live
cursor.

## Run and export

Open the menu beside **Run**, then choose **Run and export…**. Select a format and
destination. The form captures the selected SQL or the statement at the
cursor, and its connection, before the save panel opens. It shows that SQL
and connection for review. Cancelling the save panel submits no SQL.

The export runs in the tab's session. Session settings, temporary views,
and an open transaction still apply. A new execution replaces the result.
The grid keeps up to 1,000 rows and 64 MiB from its first page. The spool
and file receive all rows from the same fetch sequence. The preview is
complete only when the whole result fits in it.

For an incomplete result, choose **Run again and export (all rows)**
in the Rows list to submit its original SQL. Edited editor text does not
change this choice. This is also the way to replace a consumed or closed
cursor. A completed spool can still be saved without a new execution.

If the original session ended or its connection changed, the form explains
that its settings, temporary tables, and open transaction are gone. Review
the SQL before the rerun. The form checks the session again after the save
panel and browser sign-in. A change during that time stops submission and
requires another explicit **Run again and export…** choice.

The tab stays busy until the download ends. **Cancel** stops the download;
**Cancel export** also stops its file writer. Cancellation can interrupt
an established socket before the operation handle or schema arrives.
If SQL reached the server before its handle arrived, the server can still
continue that statement after the socket closes. Qrow does not submit it
again automatically. See [cleanup limits](#export-all-rows-and-reuse-a-download).

A result with columns and no rows produces a valid empty file. A successful
statement with no result set shows a message and writes no file. It is not
submitted again to obtain columns. A failed download requires an explicit
rerun. A failed file write with a complete spool uses **Retry…** without SQL.
Postgres exports keep exact numeric text and the session's date settings.
For typed Parquet, non-ISO dates and timestamps remain text. The form shows
this rule before submission. It shows the result's DateStyle warning when
that setting is known.

Trino exports keep exact decimal text and the full server type. A separate
heartbeat keeps an active query alive while the download pauses. The results
bar and export details show downloaded rows, spool bytes, rows per second,
and elapsed time. They also show Trino query progress when the server sends
it. Query progress is separate from download and file completion. See
[Trino limits](connections.md#trino-limits) for heartbeat and transfer limits.

## Value representation

| Value | Display |
| --- | --- |
| Null | `NULL` |
| Empty string | An empty cell |
| Decimal or textual timestamp | The server's text representation |
| Binary | Kyuubi hexadecimal text, Postgres server text, or Trino base64 text |
| Nested value | The text returned by the server |

Qrow preserves the distinction between null and an empty string. It does not
convert exact decimal text to floating-point values for display.

## Export transfer limits

The connection's [transfer policy](connections.md#export-transfer-policy)
sets the export count and the Kyuubi fetch estimate and speed limit.
A busy connection rejects another source download before SQL starts.
File writers currently use shared memory and shutdown accounting. Snapshot
and replay writers do not yet use the connection count.

An oversized Kyuubi response stops the export. Qrow does not retry an
advancing fetch. The target file is not published, and the cursor becomes
**Consumed**. Choose **Run again and export** to start an explicit new
execution. Normal preview requests keep their 1000-row limit.

## Preview limits

The preview can store up to 100,000 rows or approximately 64 MiB per tab.
For Kyuubi, if an incoming batch would exceed either limit, Qrow keeps that
batch outside the preview and keeps the cursor open for **All rows**. The
grid cannot fetch more pages at the preview limit. The memory
limit measures retained row storage, not the total application memory.

For Kyuubi, Qrow asks for each page of 1,000 rows in one request. When the server sends
fewer rows, Qrow asks for the remaining rows of the page. It does not add a
SQL `LIMIT` clause.
Client preview limits do not guarantee less server work. For Kyuubi, each server response
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

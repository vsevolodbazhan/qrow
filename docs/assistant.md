# Assistant

The optional AI assistant helps you write and run SQL in Qrow. Each
conversation has its own query tab. You and the assistant work in the same
tab, and you can edit its SQL at any time. Several conversations can work at
the same time in different tabs.

## Set up the assistant

Install the Codex command-line program separately. Sign in to Codex with
ChatGPT to use your subscription. Qrow does not include Codex or store its
credentials.

Open **Qrow → Settings… → Assistant**. In **General**, turn on **Enabled** and
read the data sharing notice. When the notice changes, for example when the
assistant gets access to schema names, Qrow turns the assistant off until you
read the new notice and turn it on again. Qrow looks for `codex` on your `PATH` and in the
usual Homebrew folders. If Qrow cannot find it, enter the full path in
**Codex → Executable**. The assistant is off by default. Qrow does not start
Codex until you open the assistant pane.

Select the default query mode in **General → Query Execution**. A new
conversation starts with this mode. Each conversation keeps its own mode after
you change it. The two modes are **Ask before running** and **Run
automatically**.

## Work with SQL

Select a query tab. Click the assistant button near the right end of the
status bar, select **Toggle assistant** in the **View** menu, or press **⌘J**. Write a message and press **Enter** or
**⌘Enter** to send it. Press **Shift-Enter** to start a new line. While Codex works, the Send button becomes
**Cancel**. While Codex starts a new conversation after its first message, you
cannot send or cancel. Type a follow-up and press **Enter** to steer the current turn.
**Cancel** interrupts the Codex turn. It does not cancel a database query that
already started. The Send button returns when the turn ends.

A message can have up to 64 KB of text. The workspace context that Qrow adds
to a message has a separate limit of 1 MB. If your text is larger than 64 KB,
Qrow does not send it. Qrow shows a notice and keeps the text in the message
field. If Codex does not accept a message, Qrow puts its text back in the
message field and shows the Codex error in the conversation.

You can draft a message while Codex starts. The model, reasoning, service tier,
and Send controls stay disabled until Codex is ready. The pane does not show
routine status text above the message field.

While Codex works on a reply, a shimmering **Working…** line shows at the end of
the conversation. It shows from the time that you send a message until the
turn ends or fails. It also shows while Codex writes text or uses tools. It does
not show while a query waits for your approval.

See [Conversation state](#conversation-state) for the status that the thread
list and the assistant toggle show.

Qrow renders your messages, assistant replies, and errors as Markdown. Messages
can show headings, lists, code, links, and tables.

Codex sends a reply in parts of different sizes. Qrow shows the reply word by
word at an even speed, about 0.3 seconds after the text arrives, and each word
fades in. Qrow does not split bold text, inline code, a link, or a table row.
If you reduce motion in macOS, each part shows when it arrives.

Your messages show at the right. Assistant replies and errors use the full width
of the conversation. If a table is too wide for a reply, its columns become
narrower and their text wraps. If the columns are at their minimum width, you
can scroll the table horizontally.

Each Qrow tool call shows as a card across the full width of the conversation.
The card is collapsed. It shows the tool icon, the tool name, for example
**Edit query** or **Run query**, and the state at the right:

- A spinner and the current step, for example **Executing**, show while a
  query runs.
- A finished query shows its downloaded rows and duration. **500+ rows** means
  that more rows are available.
- **Failed** and an error icon show when the tool call fails.
- **Cancelled** shows when Qrow does not run a query request, for example when
  you cancel it.

Select the card to show or hide the query tab, the tool arguments, and the
result. The arguments and result are plain text.

Use **Settings… → Appearance → Assistant** to change the message font family,
font size, and line height. These settings do not change the message field or
tool call cards.

The assistant can read connection names, connector types, initial databases,
query tabs, the SQL of its tab, query status, and requested result rows and
Logs of query tabs. It cannot read [Activity](activity.md). It can also read the [schema catalog](#look-up-tables-and-columns) that
Qrow keeps for each connection, and the [notes](#connection-notes) of the
connection of its tab.
Each message includes the SQL of the tab of the conversation. If this SQL is
larger than 32 KB, Qrow sends only a 32 KB part around the selection or the
cursor. Codex uses **Read query** to read the other parts, up to 32 KB at a
time. The statement ranges in the message include only the statements in the
part that Qrow sends.
When an assistant query or row fetch ends, Qrow sends its first downloaded rows
to Codex with the result: up to 20 rows and 16 KB. Codex then does not need a
separate request to read them. Codex can use **Read results** to read later
downloaded rows. Each result gives the
offset for the next read and tells Codex if more downloaded rows remain. If a
row is too large for a tool result, Qrow lists its offset and continues with
later rows. Codex cannot read that row through **Read results**.

Codex appends each new query to the tab of the conversation. It keeps existing queries and
selects the new query so it can run that statement alone. If the previous
query has no final semicolon, Qrow adds one. The assistant starts each query
that it writes with a short `--` comment that tells what the query does, for
example `-- Paid bookings by gate`. When the assistant changes a query, it keeps
the comment correct. Qrow selects only the statement below the comment, so the
assistant runs the statement without the comment. The assistant can select a
specific statement in a tab with several queries before it runs that statement.
When the appended statement is still selected, the assistant can omit the
editor revision in the same turn. Qrow uses the revision from the append.
For other runs, the assistant must give the current editor revision. It must
also give the revision if the selected statement, tab, or connection changes.
A rename keeps the tab ID. Read tools can use another open tab. If a read tool
uses an ID that is not in the open tabs, Qrow uses the tab of the conversation
and returns its ID. Qrow always changes and runs SQL in the tab of the
conversation, even if a tool call names another open tab. It checks that the
tab and connection have not changed during the turn.
Ask the assistant to change existing SQL when you want an edit or replacement.

The assistant can write SQL before you select a connection. Select a connection
before it runs SQL. It cannot read connection passwords. Qrow does not send
workspace context when you open the pane. It sends context when you send a
message or when Codex calls a Qrow tool.

Select the keyword case in **Settings… → Appearance → Editor → SQL Keyword
Case**. It is **Uppercase** or **Lowercase**. The default is Uppercase. The
indent is the Editor **Tab Size**. Qrow sends this style to the assistant with
each message, and the assistant writes SQL in it. A change applies to the next
message, also in a conversation that already exists. The keyword case does not
change SQL that you write.

Qrow formats a statement from the assistant when the statement is longer than 80
characters, also when the assistant wrote it on more than one line. This applies
to a new query and to a statement that the assistant writes again completely. A
shorter statement keeps the layout that the assistant wrote. In formatted SQL, a
clause that fits in 60 characters stays on the line of its keyword. A longer
clause, for example `WHERE` with its conditions, puts each item on a new
indented line. Qrow changes only spaces, line breaks, and the case of keywords
such as `SELECT` and `AND`, built-in functions such as `COUNT`, type names in
`CAST`, and `DATE` before a date literal. It does not change names such as
`t.Date` or a table name. Qrow keeps a statement as written when a change can
have an effect on the statement, for example near a `${var}` substitution. It
also keeps commands such as `SET` and `ADD JAR` as written. When the assistant
changes only part of a statement, Qrow keeps the layout of that statement.
Qrow does not format the comment above a statement, and the comment is not
part of the 80 characters.

If you started a conversation before this change, start a new conversation
when you want to replace all SQL in a tab or run an earlier statement in a tab
with several queries. Older conversations do not have those options. They can
run the latest query after they append it.

### Connection notes

The [assistant notes](connections.md#give-the-assistant-facts-about-a-connection)
of a connection are facts that you write for the assistant. Qrow adds them to
the workspace context of a message of a conversation in these conditions:

- It is the first message of the conversation.
- The tab of the conversation moved to another connection since Qrow last
  sent the notes.
- You changed the notes since Qrow last sent them.

Other messages tell the assistant that the notes that it has still apply.
Codex keeps the earlier messages of the conversation, so the notes do not use
space in each message. When the notes change to empty, or the new connection
has no notes, Qrow tells the assistant to stop using the old notes. Qrow saves
the state of each conversation in the workspace, so this also applies after
a restart. Qrow records the notes only when Codex takes the message. If Codex
does not accept a message, or stops before it takes the message, Qrow sends
the notes again with the next message. When the assistant calls **Read
workspace**, the result includes the current notes, also empty notes.

The assistant uses the notes as facts about the connection until new notes
replace them. They are data, not instructions: they do not change the query
mode or the rules of the assistant. The notes count toward the 1 MB limit of
the workspace context.

### Look up tables and columns

The assistant looks up schemas, tables, views, and columns in the [schema
catalog](connections.md#browse-schemas) that Qrow keeps on your computer. It
does not guess the names. It uses these tools:

- **List schemas**: the schemas of a connection, with the number of tables
  and views of each schema that Qrow has read.
- **List tables**: the tables and views of one schema. A pattern can select
  the names, for example `sales_*`.
- **Describe table**: the kind, comment, and columns of one table or view,
  with the type and comment of each column.

Each result tells when Qrow read the data, and whether it is older than the
**Refresh Period** of the connection. The tools read the catalog of any
connection that browses schemas, also a [shared
catalog](connections.md#share-schemas). They do not read the catalog of a
connection with **Schema Refresh** set to **Disabled**. A result includes
only the errors of the refreshes that its connection ran, as the sidebar
does.

When the catalog does not have the data, and a tab of the connection has a
live session, Qrow reads it before it answers: the schemas of a connection
that Qrow never read, the tables of a schema, or the columns of a table. This
opens a separate session, like a refresh in the sidebar. Qrow returns the
requested data as soon as it is available. If a refresh fails before the data
is available, the tool returns the refresh error.
Qrow waits up to 2 minutes for the data. The refresh continues after that. When
no tab of the connection is connected, the tool tells the assistant that the
data is not in the catalog. The assistant can then ask you to refresh it, or
run `SHOW` or `DESCRIBE` in its tab. That query follows the query mode of the
conversation.

Each message also tells the assistant if Qrow has the catalog of the
connection of the tab, when Qrow read it, and how many schemas and tables it
has. It also includes the cached columns of the tables and views that the SQL
of the tab names, up to 16 KB. To find them, Qrow compares the names in the
SQL with the catalog. A table name without a schema uses the **Initial
database** of the connection. An alias or a column with the name of a table
can also match.

When the connection has a [dbt project](connections.md#attach-a-dbt-project),
**Describe table** also gives the dbt data of a table that a model, seed,
snapshot, or source builds: its description, materialization, tags, the
descriptions of its columns, its tests, and its direct parents and children.
A test of `accepted_values` gives its values, and a `relationships` test gives
the table and column that it refers to. Lineage and test targets name a
catalog table when one [matches](connections.md#match-models-with-tables),
otherwise the dbt unique ID. Each list gives its full count. When the data
does not fit in a tool result, Qrow first shortens the largest part, usually
the column descriptions and then the columns, so that short lists like the
children stay complete. A cut list tells so.

Each message then also tells the assistant when dbt wrote the manifest, the
dbt version, the numbers of models, sources, and tests, and how many models
match tables. For each table of the tab SQL with a dbt resource, it adds a
short summary: the description, the columns with `unique` and `not_null`
tests, and the relationships. Each summary has at most 1 KB, and the summaries
count toward the 16 KB of the cached columns.

Conversations that started before the assistant had these tools do not have
them. Start a new conversation to use them. An older conversation can run
`SHOW` and `DESCRIBE` statements.

For a message during an active turn, Qrow adds the current workspace context
to the text sent to Codex. Qrow omits that context from the conversation,
including messages loaded from Codex history.

In **Ask before running**, Qrow shows the exact SQL, tab, and connection before
an assistant query starts. The request shows in the pane of its conversation.
Select **Run** or **Cancel**. An SQL edit does not
need approval. In **Run automatically**, assistant queries start without this
card. SQL can change or delete data and schema. Qrow cannot prove that SQL is
read-only. Select **Run automatically** only if you accept this risk.

Use the thread list to change conversations. The list is to the right of the current
conversation when the pane is wide. Qrow lists recent conversations first.
Each row shows the conversation title, its connection, the time of its last
activity, and its [state](#conversation-state). The connection and age use
separate labels. **Tab closed** shows beside the connection when its tab is
closed. Older saved conversations
without an activity time show **Earlier**. On a narrow pane, select **Toggle
Conversation List** to open the list. When you select a conversation there,
Qrow selects its tab and connection, and the pane shows that conversation. On a
wide pane, the list stays open. Search the list by title or connection name.
When you send the first message, Qrow sends it to Codex in a separate, unsaved
title request. Codex returns a short title. Qrow shows
this title in the pane header and in the thread list. If you opened the tab with
**New conversation**, its name follows the conversation title until you rename
the tab. The tab name has a
limit of 60 characters. If another tab on the same connection has the name,
Qrow adds a copy suffix. This request uses the selected model. It does not
include workspace context. If Codex cannot make a title, the tab keeps its
default name, the conversation shows **New conversation**, and Qrow tries
again when you send another message. Qrow stops a title request that does not
end in 60 seconds and handles it as a failed request. Qrow makes up to 8
titles at the same time. A request that a rename or a delete cancels does
not count toward this limit. You can continue to work while Qrow makes a title.
The conversation title shimmers during the request. The tab name also shimmers
when it follows the conversation title. The shimmer stops when the request
ends, including when it fails. If you reduce motion in macOS, the titles stay
still.
If you rename the tab, Qrow keeps your tab name when the conversation title changes.
Open **Conversation Actions** in the pane header to rename the current
conversation, make a new title, or delete it. To use these actions on a
conversation in the thread list, right-click it. You cannot rename or delete a
conversation while it works or waits for approval. Two conversations can have the same title.
**Rename…** opens the same dialog as a query tab rename. Type a title with 1 to
120 characters and select **Rename** or press **⌘Enter**.
Automatic title requests do not replace a title that you set. Select
**Regenerate title** to ask Codex for a new title from the conversation
messages. This title replaces a title that you set. If the conversation is not
open, Qrow first reads its messages from Codex. If Codex cannot make a title,
Qrow keeps the current title and shows a notice. When you
delete a conversation, Qrow asks Codex to remove it. The tab stays open without
a conversation. This action does not change SQL, sessions, Logs, or results. Qrow
saves thread IDs and titles in the workspace. Codex stores conversation text
in its own data directory.
Codex creates a conversation when you send its first message, and Qrow saves
it then.
If Codex cannot find a saved conversation, Qrow keeps its entry. If you restore
the Codex history, you can try to open it again. You can also delete the entry
from Qrow, even when Codex has no history to delete. Qrow cannot recover missing
conversation text from its workspace.
Select **Load older messages** to read earlier conversation text.
Codex sends each message to Qrow on one line of up to 8 MB. Qrow discards a
larger line and continues to use Codex. Only the request that the line
answers fails. If a page of conversation history is too large, Qrow reads
smaller pages. If the start of a large line does not identify its request,
that request fails when its time limit ends. Select **Jump
to Latest** to return to the newest message.
Qrow scrolls to the latest message when you send a message or Codex starts a
new reply. New text in that reply follows the bottom while you stay near it.

The SQL mode menu beside **Send** shows **Ask first** or **Auto run**. Select
**Ask before running** or **Run automatically** from this menu to change the mode.
Changing the mode does not send a message. Select a model, reasoning level,
and service tier below the message field. Codex supplies
the available choices. Qrow selects Codex's default model when the workspace
has no model choice. Qrow shows that model's default reasoning level as the
selected level. Select **Default** for service tier to use the tier that Codex
chooses. The controls show icons when the pane is narrow. Your change applies
to the next message. If a saved model is no longer available, Qrow selects
Codex's default model and shows a notice. If a saved reasoning level or service
tier is no longer available, Qrow uses the Codex default for that control and
shows a notice.

Codex thinks before each step, for example before each tool call and before the
reply. A higher reasoning level makes each step slower. A turn that edits and
runs a query has several steps. If assistant turns are slow, select a lower
reasoning level.

## Conversations and query tabs

Each conversation belongs to one query tab. The tab belongs to a connection,
so the conversation uses the connection of its tab. The pane shows the
conversation of the selected tab.

- If the selected tab does not have a conversation, the pane shows a new
  conversation. Your first message starts the conversation in this tab. The
  assistant can then work with the SQL that is already in the tab.
- Select **New conversation** in the pane header to open a new tab under the
  current connection. The pane shows the new conversation.
- Right-click a tab and select **Start conversation** to open the pane for that
  tab. This item is available only for a tab without a conversation.
- An unsent message stays with its tab when you select another tab.

A conversation continues to work when you select another tab or connection.
You can start a turn in each conversation, and the turns work at the same time.
Each conversation changes and runs SQL only in its own tab. It can read the
other tabs. Parallel turns use your Codex plan limits faster.

A tab cannot close while its conversation works or waits for approval. When
you close the tab, the conversation stays in the thread list. The list shows
**Tab closed**. Select the conversation to read its messages in the pane.
The conversation stays without a tab until you send another message. Qrow
then opens a new tab under the conversation's last connection. Qrow uses the
conversation title for the new tab if a title is available. Qrow does not
keep the SQL of the closed tab.

To move a conversation to another connection, right-click its tab and select
**Move to connection…**. The next message uses the new connection. You cannot
move the tab while its conversation works or waits for approval.
**Duplicate** and **Copy to connection…** do not copy the conversation.

## Conversation state

Qrow shows the state of each conversation in the thread list.
The assistant button in the status bar keeps its assistant icon. Its dot is at
the top right corner of the button.
The dot shows the most urgent state of all conversations.
The thread list shows a dot for each conversation.

- A blue dot means that Codex works on a turn.
- A yellow dot means that a query waits for your approval.
- A green dot means that an unread reply is ready.
- A red dot means that a turn ended with an unread error.
- An idle conversation has no dot.

The dots use this order of priority: approval, error, work, unread reply.
A ready reply or an error stays until its transcript shows. The thread list
and Activity do not read replies. Approval stays until you approve or cancel
the request. Tooltips and accessible names give the state in words.
Conversation tooltips show the title and a short status beside its first line.
The title and status share a vertical center. The
assistant toggle tooltip shows **Assistant** and its keyboard shortcut first.
Its status appears below. The name and shortcut share a vertical center.
Status text uses secondary styling.

The [query tab dot](queries.md#query-tab-state) shows SQL execution and session
state. Assistant turns and approval requests do not change that dot. When the
assistant runs SQL, the query tab shows its execution state.

## Pane and connection state

Opening the pane does not hide the Connections sidebar. In a small window, the
pane uses its minimum width. Press **⌘B** to hide the sidebar if you need more
space.
Drag the pane's left edge to change its width. A narrow pane shows either the
thread list or the current conversation. Qrow saves the width and starts
with the pane closed. Closing the pane does not stop a database query.

Codex continues to run after you close the pane. If the pane stays closed for
10 minutes and Codex has no work, Qrow stops Codex to save memory and CPU
time. Each Codex command or message starts the 10 minutes again. Qrow does not
stop Codex while one of these items continues:

- A turn, or a new conversation that Codex creates for its first message.
- A query that waits for your approval, or a query that the assistant runs.
- A title request, a history request, or a rename.
- A sign-in.
- A conversation without a turn. A new Codex process cannot open it.

When you open the pane again, Qrow starts Codex. The pane shows the usual
startup state, and the controls wait for Codex. The conversations and their
messages stay in the pane. Qrow loads the history of each conversation again
from Codex when you show it. Signed-in accounts stay signed in, because Codex
keeps the account.

Assistant notices use an alert in the pane. These notices report an unavailable
saved option, a title error, or another exceptional condition. Routine status
does not add text above the message field.

If you quit Qrow while Codex is working, Qrow uses the [workspace close
confirmation](workspace.md#quit-and-save) before it stops the turn. At quit,
Qrow waits up to 2 seconds for Codex to stop. If Codex does not stop in 1.5
seconds, Qrow stops Codex and its child processes.

If Codex disconnects, Qrow keeps your unsent draft for the current app session.
A first message that did not start its conversation goes back to the message
field of its tab.
**Reconnect** replaces **Send** and **Cancel** below the message field. Its
tooltip shows the error. Select **Reconnect** to try again. Qrow stops the
old Codex process in the background, so the window does not wait for it.
Qrow does not send the draft for you. A database query that already started can
finish while Codex is disconnected.

If Codex is signed out, the assistant pane shows **Sign in to Codex** in place
of the conversation. Select **Sign in with ChatGPT…**. Codex opens its sign-in
page in your browser. Qrow shows that sign-in continues in the browser. Select
**Reopen page** if you closed the page. Select **Cancel** to stop the sign-in.
If the sign-in fails, Qrow shows the Codex error below the button until you
sign in. If you sign in with another Codex client, the pane opens the
conversation. If Codex uses an API key, Codex applies API-key billing. Manage
sign-out in Codex, because its account is shared with other Codex clients.

The demo uses synthetic query data and an in-memory workspace. Qrow asks Codex
to delete demo conversations on normal exit. A conversation can remain if
Codex does not confirm deletion before exit or if Qrow crashes.

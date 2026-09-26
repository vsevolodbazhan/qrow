# AI assistant

The optional AI assistant helps you write and run SQL in Qrow. It shares the
selected query tab with you. You can edit the SQL while the assistant is open.

## Set up the assistant

Install the Codex command-line program separately. Sign in to Codex with
ChatGPT to use your subscription. Qrow does not include Codex or store its
credentials.

Open **Qrow → Settings… → AI Assistant**. Enable the assistant and read the data
sharing notice. If Qrow cannot find `codex` on your `PATH`, enter the full path
to the Codex executable. The assistant is off by default. Qrow does not start
Codex until you open the assistant pane.

Select the default query mode in Settings. A new conversation starts with this
mode. Each conversation keeps its own mode after you change it. The two modes
are **Ask before running** and **Run automatically**.

## Work with SQL

Select a query tab. Select **Toggle Assistant** in the tab strip or press
**⌘J**. Write a message and press **Enter** or **⌘Enter** to send it. Press
**Shift-Enter** to start a new line. While Codex works, the Send button becomes
**Cancel**. Type a follow-up and press **Enter** to steer the current turn.
**Cancel** interrupts the Codex turn. It does not cancel a database query that
already started. The Send button returns when the turn ends.

You can draft a message while Codex starts. The model, reasoning, service tier,
and Send controls stay disabled until Codex is ready. The pane does not show
routine status text above the message field.

After you send a message, a shimmering **Thinking…** reply appears in the
conversation. The reply replaces it when Codex starts to answer. Qrow removes
the placeholder if the turn ends or fails without a reply.

Qrow renders your messages, assistant replies, and errors as Markdown. Messages
can show headings, lists, code, links, and tables.

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
query tabs, selected SQL, query status, and requested result rows and Logs. It
appends each new query to the selected tab. It keeps existing queries and
selects the new query so it can run that statement alone. If the previous
query has no final semicolon, Qrow adds one. The assistant can select a
specific statement in a tab with several queries before it runs that statement.
If you switch or rename a tab during a turn, the assistant can read the
workspace again to use the current selected tab. A rename keeps the tab ID.
If an assistant tool uses an ID that is not in the open tabs, Qrow uses the
selected tab and returns its ID. Qrow rejects an ID for a different open tab
when the tool must act on the selected tab.
Ask the assistant to change existing SQL when you want an edit or replacement.
The assistant can write SQL before you select a connection. Select a connection
before it runs SQL. It cannot read connection passwords. Qrow does not send
workspace context when you open the pane. It sends context when you send a
message or when Codex calls a Qrow tool.

If you started a conversation before this change, start a new conversation
when you want to replace all SQL in a tab or run an earlier statement in a tab
with several queries. Older conversations do not have those options. They can
run the latest query after they append it.

For a message during an active turn, Qrow adds the current workspace context
to the text sent to Codex. Qrow omits that context from the conversation,
including messages loaded from Codex history.

In **Ask before running**, Qrow shows the exact SQL, tab, and connection before
an assistant query starts. Select **Run** or **Cancel**. An SQL edit does not
need approval. In **Run automatically**, assistant queries start without this
card. SQL can change or delete data and schema. Qrow cannot prove that SQL is
read-only. Select **Run automatically** only if you accept this risk.

Use the thread list to change conversations. The list is to the right of the current
conversation when the pane is wide. Qrow lists recent conversations first.
Older saved conversations without an activity time show **Earlier**. On a
narrow pane, select **Toggle conversation list** to open the list. Search the
list by title. Select **New
Conversation** in the pane header to start a separate conversation.
After the first reply, Qrow sends recent messages from the conversation to
Codex in a separate, unsaved request. Codex returns a short title. Qrow shows
this title in the pane header and in the thread list. This request uses the
selected model. It does not include workspace context. If Codex cannot make a
title, Qrow shows **New conversation** and tries again after the next reply.
Open **Conversation actions** in the pane header to rename the current
conversation, make a new title, or delete it. To use these actions on a
conversation in the thread list, right-click it. You cannot use them on a
conversation while Codex works on it. Two conversations can have the same title.
**Rename…** opens the same dialog as a query tab rename. Type a title with 1 to
120 characters and select **Rename** or press **⌘Enter**.
Automatic title requests do not replace a title that you set. Select
**Regenerate Title** to ask Codex for a new title from the conversation
messages. This title replaces a title that you set. If the conversation is not
open, Qrow first reads its messages from Codex. If Codex cannot make a title,
Qrow keeps the current title and shows a notice. When you
delete a conversation, Qrow asks Codex to remove it. This action does not
change query tabs, SQL, sessions, Logs, or results. Qrow
saves thread IDs and titles in the workspace. Codex stores conversation text
in its own data directory.
Qrow saves a new conversation after you send its first message. Codex does not
keep a conversation without messages after Codex stops. If you close Qrow or
reconnect Codex before you send a message, Qrow starts a new conversation.
If Codex cannot find a saved conversation, Qrow keeps its entry. If you restore
the Codex history, you can try to open it again. You can also delete the entry
from Qrow, even when Codex has no history to delete. Qrow cannot recover missing
conversation text from its workspace.
Select **Load older messages** to read earlier conversation text. Select **Jump
to latest** to return to the newest message.
Qrow scrolls to the latest message when you send a message or Codex starts a
new reply. New text in that reply follows the bottom while you stay near it.

The Send button shows the current query mode, **Ask** or **Run**. Select **Ask
before running** or **Run automatically** from its menu to change the mode.
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

## Pane and connection state

Opening the pane does not hide the Connections sidebar. In a small window, the
pane uses its minimum width. Press **⌘B** to hide the sidebar if you need more
space.
Drag the pane's left edge to change its width. A narrow pane shows either the
thread list or the current conversation. Qrow saves the width and starts
with the pane closed. Closing the pane does not stop Codex or a database query.
The tab strip shows **Done** next to the assistant toggle if a hidden turn
finishes. Reopen the pane to clear that state.

If Codex disconnects, Qrow keeps your unsent draft for the current app session.
The **Reconnect** tooltip shows the error. Select **Reconnect** to try again.
Qrow does not send the draft for you. A database query that already started can
finish while Codex is disconnected.

If Codex is signed out, select **Sign in with ChatGPT**. Codex opens its sign-in
page in your browser. If Codex uses an API key, Codex applies API-key billing.
Manage sign-out in Codex, because its account is shared with other Codex
clients.

The demo uses synthetic query data and an in-memory workspace. Qrow asks Codex
to delete demo conversations on normal exit. A conversation can remain if
Codex does not confirm deletion before exit or if Qrow crashes.

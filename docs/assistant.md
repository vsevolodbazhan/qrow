# AI assistant

The optional AI assistant helps you write and run SQL in Qrow. It shares the
selected query tab with you. You can edit the SQL while the assistant is open.

## Set up the assistant

Install the Codex command-line program separately. Sign in to Codex with
ChatGPT to use your subscription. Qrow does not include Codex or store its
credentials.

Open **Qrow → Settings… → Assistant**. Enable the assistant and read the data
sharing notice. If Qrow cannot find `codex` on your `PATH`, enter the full path
to the Codex executable. The assistant is off by default. Qrow does not start
Codex until you open the assistant pane.

Select the default query mode in Settings. A new conversation starts with this
mode. Each conversation keeps its own mode after you change it. The two modes
are **Ask before running** and **Run automatically**.

## Work with SQL

Select a query tab. Select **Assistant** in the toolbar or press **⌘J**. Write
a message and press **Enter** or **⌘Enter** to send it. Press **Shift-Enter** to
start a new line. You can send another message while Codex works. The message
steers the current turn. **Stop** stops the Codex turn. It does not cancel a
database query that already started.

The assistant can read connection names, connector types, initial databases,
query tabs, selected SQL, query status, and requested result rows and Logs. It
can edit the selected query tab and run its one SQL statement through Qrow's
query worker. It cannot read connection passwords. Qrow does not send workspace
context when you open the pane. It sends context when you send a message or
when Codex calls a Qrow tool.

In **Ask before running**, Qrow shows the exact SQL, tab, and connection before
an assistant query starts. Select **Run** or **Cancel**. An SQL edit does not
need approval. In **Run automatically**, assistant queries start without this
card. SQL can change or delete data and schema. Qrow cannot prove that SQL is
read-only. Select **Run automatically** only if you accept this risk.

Use the conversation picker to change conversations. **New** starts a separate
conversation. **Rename** changes its title. **Delete** asks Codex to delete the
selected conversation. Deleting a conversation does not change query tabs,
SQL, sessions, Logs, or results. Qrow saves thread IDs and titles in the
workspace. Codex stores conversation text in its own data directory.

Select a model, reasoning level, and service tier in the pane. Codex supplies
the available choices. Your change applies to the next message. If a saved
choice is no longer available, Qrow uses a Codex default and shows a notice.

## Pane and connection state

Drag the pane's left edge to change its width. Qrow saves this width and starts
with the pane closed. Closing the pane does not stop Codex or a database query.
The toolbar shows **Assistant · Done** if a hidden turn finishes. Reopen the
pane to clear that state.

If Codex disconnects, Qrow keeps your unsent draft for the current app session.
Select **Reconnect**. Qrow does not send the draft for you. A database query
that already started can finish while Codex is disconnected.

If Codex is signed out, select **Sign in with ChatGPT**. Codex opens its sign-in
page in your browser. If Codex uses an API key, the pane says that API-key
billing applies. Manage sign-out in Codex, because its account is shared with
other Codex clients.

The demo uses synthetic query data and an in-memory workspace. Qrow asks Codex
to delete demo conversations on normal exit. A conversation can remain if
Codex does not confirm deletion before exit or if Qrow crashes.

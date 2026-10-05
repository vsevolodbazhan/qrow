//! The assistant notes of a connection: the form field, and when a message
//! sends them.
use crate::support::assistant::{FakeCodex, REPLY_TIMEOUT};
use crate::support::{MemoryCredentials, TestApp, connection_row, offline_profile};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::model::{MAX_ASSISTANT_NOTES_BYTES, SavedTab, Workspace};
use std::time::Duration;
use uuid::Uuid;

/// Connections Alpha, with notes, and Beta, without notes, with one tab each.
fn launch(cx: &mut TestAppContext) -> (TestApp, FakeCodex, Uuid, Uuid) {
    let (directory, codex) = FakeCodex::new();
    let mut alpha = offline_profile("Alpha");
    alpha.assistant_notes = "Dates are UTC.".into();
    let beta = offline_profile("Beta");
    let (a, b) = (alpha.id, beta.id);
    let mut workspace = codex.workspace(Workspace {
        profiles: vec![alpha, beta],
        tabs: vec![SavedTab::new(1, Some(a)), SavedTab::new(1, Some(b))],
        ..Workspace::default()
    });
    workspace.settings.assistant.panel_width = 536.;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    (app, codex, a, b)
}

/// Opens Connection Settings of `profile` and replaces its notes.
fn edit_notes(app: &TestApp, cx: &mut TestAppContext, profile: Uuid, notes: &str) {
    app.context_menu(cx, connection_row(profile));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.scroll_to(cx, "connection-assistant-notes");
    app.update(cx, |window, cx| {
        window.click("connection-assistant-notes", cx);
        window.press("cmd-a", cx);
        if notes.is_empty() {
            window.press("backspace", cx);
        } else {
            window.input(notes, cx);
        }
    });
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the saved notes", Duration::from_secs(10), |_, _| {
        app.saved()
            .profiles
            .iter()
            .any(|p| p.id == profile && p.assistant_notes == notes)
    });
}

#[gpui_kit::test]
fn a_message_sends_the_notes_only_when_they_are_new_to_the_conversation(cx: &mut TestAppContext) {
    let (app, _codex, alpha, beta) = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);

    app.send(cx, "Report notes 1");
    app.wait_reply(
        cx,
        r#"Report notes 1: notes "Dates are UTC.", unchanged False"#,
    );
    app.wait_idle(cx);
    app.send(cx, "Report notes 2");
    app.wait_reply(cx, "Report notes 2: notes null, unchanged True");
    app.wait_idle(cx);

    // An edit sends the new notes once.
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    edit_notes(&app, cx, alpha, "Dates are local.");
    app.click(cx, "toggle-assistant");
    app.show_conversation(cx);
    app.send(cx, "Report notes 3");
    app.wait_reply(
        cx,
        r#"Report notes 3: notes "Dates are local.", unchanged False"#,
    );
    app.wait_idle(cx);

    // A workspace read gives the notes too, so the next message does not
    // repeat them. A change back to the notes of the first message sends
    // them again.
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    edit_notes(&app, cx, alpha, "Dates are UTC.");
    app.click(cx, "toggle-assistant");
    app.show_conversation(cx);
    app.send(cx, "Read workspace notes 1");
    app.wait_reply(cx, r#"Read workspace notes 1: notes "Dates are UTC.""#);
    app.wait_idle(cx);
    app.send(cx, "Report notes 3b");
    app.wait_reply(cx, "Report notes 3b: notes null, unchanged True");
    app.wait_idle(cx);
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    edit_notes(&app, cx, alpha, "Dates are local.");
    app.click(cx, "toggle-assistant");
    app.show_conversation(cx);
    app.send(cx, "Report notes 3c");
    app.wait_reply(
        cx,
        r#"Report notes 3c: notes "Dates are local.", unchanged False"#,
    );
    app.wait_idle(cx);

    // The record survives a restart.
    let app = app.relaunch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "Report notes 4");
    app.wait_reply(cx, "Report notes 4: notes null, unchanged True");
    app.wait_idle(cx);

    // Beta has no notes, so the move removes the notes of Alpha.
    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Move to Connection…", "Beta");
    app.wait_until(cx, "the moved conversation", REPLY_TIMEOUT, |_, _| {
        let saved = app.saved();
        let tab = saved.assistant.conversations[0].tab_id;
        saved
            .tabs
            .iter()
            .any(|t| Some(t.id) == tab && t.profile == Some(beta))
    });
    app.send(cx, "Report notes 5");
    app.wait_reply(cx, r#"Report notes 5: notes "", unchanged False"#);
    app.wait_idle(cx);
    app.send(cx, "Report notes 6");
    app.wait_reply(cx, "Report notes 6: notes null, unchanged False");
}

#[gpui_kit::test]
fn a_rejected_message_sends_the_notes_again(cx: &mut TestAppContext) {
    let (app, _codex, alpha, _) = launch(cx);
    app.open_assistant(cx);
    app.show_conversation(cx);
    app.send(cx, "Report notes first");
    app.wait_reply(cx, r#"Report notes first: notes "Dates are UTC.""#);
    app.wait_idle(cx);
    // The cleared notes must reach Codex, also when it rejects the first
    // message after the change.
    app.click(cx, "toggle-assistant");
    app.wait_gone(cx, "assistant-composer");
    edit_notes(&app, cx, alpha, "");
    app.click(cx, "toggle-assistant");
    // Qrow puts the rejected text back in the message field.
    app.type_message(cx, "Reject this message");
    app.click(cx, "assistant-send");
    app.wait_reply(cx, "Synthetic rejection");
    app.wait_idle(cx);
    app.send(cx, "Report notes after rejection");
    app.wait_reply(
        cx,
        r#"Report notes after rejection: notes "", unchanged False"#,
    );
    app.wait_idle(cx);
    app.send(cx, "Report notes later");
    app.wait_reply(cx, "Report notes later: notes null, unchanged False");
}

#[gpui_kit::test]
fn the_notes_field_rejects_notes_above_the_limit(cx: &mut TestAppContext) {
    let (app, _codex, alpha, _) = launch(cx);
    // A duplicate keeps the notes.
    app.context_menu(cx, connection_row(alpha));
    app.choose(cx, "popup-menu", "Duplicate");
    app.wait_for(cx, "connection-name");
    app.fill(cx, "connection-password", "synthetic-password");
    app.click(cx, "save-profile");
    app.wait_until(cx, "the duplicate", Duration::from_secs(10), |_, _| {
        app.saved()
            .profiles
            .iter()
            .any(|p| p.name == "Alpha copy" && p.assistant_notes == "Dates are UTC.")
    });

    // Notes at the limit save. One more character does not.
    let full = "x".repeat(MAX_ASSISTANT_NOTES_BYTES - "Dates are UTC.".len());
    app.context_menu(cx, connection_row(alpha));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.scroll_to(cx, "connection-assistant-notes");
    app.update(cx, |window, cx| {
        window.click("connection-assistant-notes", cx);
        window.press("cmd-end", cx);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(full.clone()));
        window.press("cmd-v", cx);
        window.input("y", cx);
    });
    app.click(cx, "save-profile");
    app.wait_until(
        cx,
        "the limit error",
        Duration::from_secs(10),
        |window, _| {
            crate::support::label(window, "connection-form-error-accessibility").as_deref()
                == Some("Assistant notes must be 16 KB or less.")
        },
    );
    assert!(
        app.saved()
            .profiles
            .iter()
            .any(|p| p.id == alpha && p.assistant_notes == "Dates are UTC.")
    );
}

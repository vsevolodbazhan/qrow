use super::*;
use crate::assistant::inbox::CommandSender;

struct Fixture {
    _directory: tempfile::TempDir,
    executable: PathBuf,
    state: PathBuf,
    cwd: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-claude.py");
        std::fs::write(
            &script,
            include_str!("../../../tests/desktop/fake-claude.py"),
        )
        .unwrap();
        let state = directory.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let executable = directory.path().join("claude");
        crate::assistant::write_test_executable(
            &executable,
            &format!(
                "#!/bin/sh\nQROW_FAKE_CLAUDE_STATE='{}' exec python3 '{}' \"$@\"\n",
                state.display(),
                script.display()
            ),
        );
        let cwd = directory.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        Self {
            _directory: directory,
            executable,
            state,
            cwd,
        }
    }

    fn mark(&self, name: &str) {
        std::fs::write(self.state.join(name), "").unwrap();
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.state.join("log")).unwrap_or_default()
    }

    fn launch(&self) -> (CommandSender, ClaudeHarness) {
        let (commands, inbox) = Inbox::channel(16);
        let harness = ClaudeHarness::launch_with_inbox(
            &self.executable,
            &self.cwd,
            inbox,
            ProcessIds::default(),
        )
        .unwrap();
        (commands, harness)
    }
}

fn next_event(harness: &mut ClaudeHarness) -> AssistantEvent {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match harness.next_input(Some(deadline)).unwrap() {
            Some(Input::Event(event)) => return event,
            Some(_) => continue,
            None => panic!("no event before the deadline"),
        }
    }
}

/// The events until the turn ends, with the turn end last.
fn turn_events(harness: &mut ClaudeHarness) -> Vec<AssistantEvent> {
    let mut events = Vec::new();
    loop {
        let event = next_event(harness);
        let done = matches!(event, AssistantEvent::TurnCompleted { .. });
        events.push(event);
        if done {
            return events;
        }
    }
}

fn request(thread: &str, text: &str) -> TurnRequest {
    TurnRequest {
        thread_id: thread.into(),
        text: text.into(),
        context: json!({
            "version": 1,
            "selected_tab": {
                "id": "00000000-0000-0000-0000-0000000000aa",
                "connection_id": null,
                "editor_revision": 3,
            },
        }),
        model: None,
        reasoning_effort: None,
        service_tier: None,
    }
}

fn reply_text(events: &[AssistantEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AssistantEvent::MessageCompleted { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn the_snapshot_lists_models_with_a_default_reasoning_level_and_the_account() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let snapshot = harness.snapshot().unwrap();
    assert_eq!(
        snapshot.account().kind(),
        &AccountKind::Claude {
            plan: Some("Claude Pro".into())
        }
    );
    assert!(!snapshot.features().steer && !snapshot.features().sign_in);
    let models = snapshot.models();
    assert_eq!(models[0].id(), "default");
    assert!(models[0].is_default());
    assert_eq!(models[0].default_reasoning_effort(), DEFAULT_EFFORT);
    let efforts: Vec<_> = models[0]
        .reasoning_efforts()
        .iter()
        .map(|effort| effort.id())
        .collect();
    assert_eq!(
        efforts,
        ["default", "low", "medium", "high", "xhigh", "max"]
    );
    // A model without reasoning levels has no reasoning control.
    assert!(models[1].reasoning_efforts().is_empty());

    fixture.mark("signed-out");
    assert_eq!(
        harness.snapshot().unwrap().account().kind(),
        &AccountKind::SignedOut
    );
    assert!(harness.begin_login().is_err());
}

#[test]
fn an_old_claude_code_does_not_start() {
    let fixture = Fixture::new();
    fixture.mark("old-version");
    let (_commands, inbox) = Inbox::channel(16);
    let error = match ClaudeHarness::launch_with_inbox(
        &fixture.executable,
        &fixture.cwd,
        inbox,
        ProcessIds::default(),
    ) {
        Ok(_) => panic!("an old version started"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("too old"), "{error}");
    assert_eq!(parse_version("2.1.285 (Claude Code)"), Some((2, 1, 285)));
    assert_eq!(parse_version("unknown"), None);
}

#[test]
fn a_turn_streams_its_reply_and_ends() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let tools = crate::assistant::tools::definitions();
    let conversation = harness.create_conversation(&tools).unwrap();
    // The Qrow tools reach Claude Code as one MCP server.
    let listed = std::fs::read_to_string(fixture.state.join("tools")).unwrap();
    assert!(
        listed.lines().any(|name| name == "tab-read-sql"),
        "{listed}"
    );

    let turn = harness
        .start_turn(request(&conversation.id, "Explain SELECT 1"))
        .unwrap();
    let events = turn_events(&mut harness);
    let deltas: String = events
        .iter()
        .filter_map(|event| match event {
            AssistantEvent::MessageDelta { turn_id, text, .. } => {
                assert_eq!(turn_id, &turn.id);
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(deltas, "Claude reply: Explain SELECT 1");
    assert_eq!(reply_text(&events), "Claude reply: Explain SELECT 1");
    match events.last().unwrap() {
        AssistantEvent::TurnCompleted {
            turn: ended, error, ..
        } => {
            assert_eq!(ended.id, turn.id);
            assert_eq!(ended.status, "completed");
            assert_eq!(error, &None);
        }
        other => panic!("unexpected {other:?}"),
    }
    // The workspace context follows the message text, as for a steer.
    assert!(fixture.log().contains("turn"));
}

#[test]
fn a_tool_call_goes_to_qrow_and_its_answer_back() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let conversation = harness
        .create_conversation(&crate::assistant::tools::definitions())
        .unwrap();
    harness
        .start_turn(request(&conversation.id, "Write SELECT 1 into this tab"))
        .unwrap();
    let call = loop {
        match next_event(&mut harness) {
            AssistantEvent::ToolCall(call) => break call,
            AssistantEvent::MessageDelta { .. } | AssistantEvent::MessageCompleted { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(call.name, "tab-append-sql");
    assert_eq!(call.thread_id, conversation.id);
    assert_eq!(call.arguments["sql"], "SELECT 1");
    assert_eq!(call.arguments["editor_revision"], 3);
    harness
        .answer_tool_call(
            &call,
            ToolResult {
                success: true,
                content: json!({"version": 1, "editor_revision": 4}),
            },
        )
        .unwrap();
    let events = turn_events(&mut harness);
    assert_eq!(reply_text(&events), "I updated the SQL.");
    assert!(
        fixture
            .log()
            .contains(r#"tool result {"version":1,"editor_revision":4}"#),
        "{}",
        fixture.log()
    );
}

#[test]
fn an_interrupted_turn_ends_without_an_error() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let conversation = harness
        .create_conversation(&crate::assistant::tools::definitions())
        .unwrap();
    let turn = harness
        .start_turn(request(&conversation.id, "Hold the reply"))
        .unwrap();
    assert!(matches!(
        next_event(&mut harness),
        AssistantEvent::MessageDelta { .. }
    ));
    harness.interrupt_turn(&conversation.id, &turn.id).unwrap();
    match turn_events(&mut harness).last().unwrap() {
        AssistantEvent::TurnCompleted { turn, error, .. } => {
            assert_eq!(turn.status, "interrupted");
            assert_eq!(error, &None);
        }
        other => panic!("unexpected {other:?}"),
    }
    // A failed turn shows the reason of Claude Code.
    harness
        .start_turn(request(&conversation.id, "Fail now"))
        .unwrap();
    match turn_events(&mut harness).last().unwrap() {
        AssistantEvent::TurnCompleted { turn, error, .. } => {
            assert_eq!(turn.status, "failed");
            assert_eq!(error.as_deref(), Some("Synthetic failure."));
        }
        other => panic!("unexpected {other:?}"),
    }
    // A message during a turn waits for Qrow; Claude Code would queue it.
    assert!(
        harness
            .steer_turn(&conversation.id, &turn.id, "More", &json!({}))
            .is_err()
    );
}

#[test]
fn a_saved_conversation_resumes_in_a_new_process() {
    let fixture = Fixture::new();
    let id = {
        let (_commands, mut harness) = fixture.launch();
        let conversation = harness
            .create_conversation(&crate::assistant::tools::definitions())
            .unwrap();
        harness
            .start_turn(request(&conversation.id, "First message"))
            .unwrap();
        turn_events(&mut harness);
        conversation.id
    };
    let (_commands, mut harness) = fixture.launch();
    // The next message starts the process of a saved conversation.
    harness.start_turn(request(&id, "Second message")).unwrap();
    assert_eq!(
        reply_text(&turn_events(&mut harness)),
        "Claude reply: Second message"
    );
    let missing = Uuid::new_v4().to_string();
    let error = harness.resume_conversation(&missing).unwrap_err();
    assert!(
        format!("{error:#}").contains("No conversation found"),
        "{error:#}"
    );
    assert!(harness.resume_conversation("../not-a-session").is_err());
}

#[test]
fn model_and_reasoning_changes_apply_to_the_next_turn() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let conversation = harness
        .create_conversation(&crate::assistant::tools::definitions())
        .unwrap();
    let mut first = request(&conversation.id, "One");
    first.model = Some("synthetic-fast".into());
    first.reasoning_effort = Some("low".into());
    harness.start_turn(first).unwrap();
    turn_events(&mut harness);
    let mut second = request(&conversation.id, "Two");
    second.reasoning_effort = Some(DEFAULT_EFFORT.into());
    harness.start_turn(second).unwrap();
    turn_events(&mut harness);
    // `max` starts the process again with the launch option.
    let mut third = request(&conversation.id, "Three");
    third.reasoning_effort = Some("max".into());
    harness.start_turn(third).unwrap();
    turn_events(&mut harness);
    let log = fixture.log();
    assert!(
        log.contains("turn ") && log.contains("model=synthetic-fast effort=low One"),
        "{log}"
    );
    assert!(log.contains("model=None effort=None Two"), "{log}");
    assert!(log.contains("effort=max Three"), "{log}");
}

#[test]
fn titles_come_from_claude_code_and_can_fail() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let thread = Uuid::new_v4().to_string();
    let title = |thread: &str| TitleRequest {
        thread_id: thread.into(),
        messages: vec![("user", "Count paid bookings by gate".into())],
        model: None,
        reasoning_effort: None,
    };
    harness.generate_title(title(&thread)).unwrap();
    assert_eq!(
        next_event(&mut harness),
        AssistantEvent::TitleChanged {
            thread_id: thread.clone(),
            title: "Title: Count paid bookings".into(),
        }
    );
    fixture.mark("title-fails");
    harness.generate_title(title(&thread)).unwrap();
    assert_eq!(
        next_event(&mut harness),
        AssistantEvent::TitleFailed { thread_id: thread }
    );
}

#[test]
fn a_delete_removes_only_the_files_of_its_session() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let config = fixture.state.join("config");
    harness.config_dir = Some(config.clone());
    let id = Uuid::new_v4().to_string();
    let other = Uuid::new_v4().to_string();
    let project = config.join("projects").join("-some-project");
    std::fs::create_dir_all(project.join(&id)).unwrap();
    for name in [&id, &other] {
        std::fs::write(project.join(format!("{name}.jsonl")), "{}").unwrap();
    }
    harness.delete_conversation(&id).unwrap();
    assert!(!project.join(format!("{id}.jsonl")).exists());
    assert!(!project.join(&id).exists());
    assert!(project.join(format!("{other}.jsonl")).exists());
    // A conversation without files deletes without an error.
    harness.delete_conversation(&id).unwrap();
}

#[test]
fn a_crashed_process_ends_its_turn_with_the_reason() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let conversation = harness
        .create_conversation(&crate::assistant::tools::definitions())
        .unwrap();
    harness
        .start_turn(request(&conversation.id, "Exit now"))
        .unwrap();
    match next_event(&mut harness) {
        AssistantEvent::TurnCompleted { turn, error, .. } => {
            assert_eq!(turn.status, "failed");
            let error = error.unwrap();
            assert!(error.contains("Synthetic crash"), "{error}");
        }
        other => panic!("unexpected {other:?}"),
    }
    // The conversation starts again with its next message.
    harness
        .start_turn(request(&conversation.id, "Again"))
        .unwrap();
    assert_eq!(
        reply_text(&turn_events(&mut harness)),
        "Claude reply: Again"
    );
}

#[test]
fn idle_conversation_processes_stop_beyond_the_limit() {
    let fixture = Fixture::new();
    let (_commands, mut harness) = fixture.launch();
    let tools = crate::assistant::tools::definitions();
    let mut ids = Vec::new();
    for index in 0..=MAX_LIVE_SESSIONS {
        let conversation = harness.create_conversation(&tools).unwrap();
        harness
            .start_turn(request(&conversation.id, &format!("Message {index}")))
            .unwrap();
        turn_events(&mut harness);
        ids.push(conversation.id);
    }
    assert!(harness.sessions.len() <= MAX_LIVE_SESSIONS);
    // The least recently used one stopped; its next message starts it again.
    assert!(!harness.sessions.contains_key(&ids[0]));
    harness.start_turn(request(&ids[0], "Back")).unwrap();
    assert_eq!(reply_text(&turn_events(&mut harness)), "Claude reply: Back");

    harness.session_idle_timeout = Duration::ZERO;
    harness.stop_idle_sessions(MAX_LIVE_SESSIONS, Instant::now());
    assert!(harness.sessions.is_empty());
}

#[test]
fn the_output_reader_waits_for_room_until_its_process_closes() {
    let (_commands, mut inbox) = Inbox::channel(1);
    inbox.set_process_capacity(1);
    let sender = inbox.process_sender(1);
    let closing = Arc::new(AtomicBool::new(false));
    let reader_closing = Arc::clone(&closing);
    let reader = thread::spawn(move || {
        read_lines(
            std::io::Cursor::new(b"{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n".to_vec()),
            &sender,
            &reader_closing,
        )
    });
    // One message fits in the inbox; the reader waits for room for the next.
    assert!(matches!(inbox.next(None), Some(Message::Output(1, Ok(message))) if message["n"] == 1));
    assert!(matches!(inbox.next(None), Some(Message::Output(1, Ok(message))) if message["n"] == 2));
    thread::sleep(Duration::from_millis(50));
    assert!(!reader.is_finished());
    // A process that stops does not wait for the worker to read.
    closing.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !reader.is_finished() {
        assert!(Instant::now() < deadline, "The reader still waits");
        thread::sleep(Duration::from_millis(10));
    }
    reader.join().unwrap();
}

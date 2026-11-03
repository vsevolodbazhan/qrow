//! The harness process: start, stop, idle stop, commands, and the model
//! settings that the harness offers.
use super::*;

pub(super) fn reasoning_effort_label(id: &str) -> String {
    match id {
        "xhigh" => "Extra high".into(),
        crate::assistant::DEFAULT_REASONING_EFFORT => "Default".into(),
        _ => id
            .split(['_', '-', ' '])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| format!("{}{}", first.to_uppercase(), chars.as_str()))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Finds the executable of `harness`: the configured path, or the first
/// match on `PATH` and in the usual install folders.
pub(super) fn discover_harness(
    harness: AssistantHarness,
    configured: Option<&str>,
) -> Result<PathBuf, String> {
    let name = harness.name();
    if let Some(path) = configured {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path).ok_or_else(|| {
            format!(
                "The configured {name} executable was not found. Choose another path in Settings."
            )
        });
    }
    let program = match harness {
        AssistantHarness::Codex => "codex",
        AssistantHarness::Claude => "claude",
    };
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(program)));
    }
    candidates.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .map(|dir| PathBuf::from(dir).join(program)),
    );
    if harness == AssistantHarness::Claude
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
    {
        // The native installer of Claude Code uses these folders.
        candidates.push(home.join(".local/bin/claude"));
        candidates.push(home.join(".claude/local/claude"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!("{name} was not found. Install {name}, or select its executable in Settings.")
        })
}

impl Qrow {
    /// Replaces saved assistant settings that the Codex snapshot does not offer.
    pub(super) fn reconcile_assistant_settings(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.assistant_state.snapshot.clone() else {
            return;
        };
        let models = snapshot.models();
        if self
            .settings
            .assistant
            .choice()
            .model
            .as_ref()
            .is_some_and(|id| !models.iter().any(|model| model.id() == id))
        {
            self.settings.assistant.choice_mut().model = None;
            self.settings.assistant.choice_mut().reasoning_effort = None;
            self.settings.assistant.choice_mut().service_tier = None;
            self.assistant_state.notice = Some(AssistantNotice::info(format!(
                "Saved model is unavailable. {} default model selected.",
                self.settings.assistant.harness.name()
            )));
            self.changed(cx);
        }
        let default_model = models
            .iter()
            .find(|model| model.is_default())
            .or_else(|| models.first());
        if self.settings.assistant.choice().model.is_none()
            && let Some(model) = default_model
        {
            self.settings.assistant.choice_mut().model = Some(model.id().to_owned());
            self.changed(cx);
        }
        let model = self
            .settings
            .assistant
            .choice()
            .model
            .as_deref()
            .and_then(|id| models.iter().find(|model| model.id() == id))
            .or(default_model);
        let efforts = model.map(|model| model.reasoning_efforts()).unwrap_or(&[]);
        if self
            .settings
            .assistant
            .choice()
            .reasoning_effort
            .as_ref()
            .is_some_and(|id| !efforts.iter().any(|effort| effort.id() == id))
        {
            self.settings.assistant.choice_mut().reasoning_effort = None;
            self.assistant_state.notice = Some(AssistantNotice::info(format!(
                "Saved reasoning level is unavailable. {} default is in use.",
                self.settings.assistant.harness.name()
            )));
            self.changed(cx);
        }
        let tiers = model.map(|model| model.service_tiers()).unwrap_or(&[]);
        if self
            .settings
            .assistant
            .choice()
            .service_tier
            .as_ref()
            .is_some_and(|id| !tiers.iter().any(|tier| tier.id() == id))
        {
            self.settings.assistant.choice_mut().service_tier = None;
            self.assistant_state.notice = Some(AssistantNotice::info(format!(
                "Saved service tier is unavailable. {} default is in use.",
                self.settings.assistant.harness.name()
            )));
            self.changed(cx);
        }
    }

    pub(super) fn select_assistant_model(&mut self, label: &str, cx: &mut Context<Self>) {
        let Some(model_id) = self
            .assistant_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .models()
                    .iter()
                    .find(|model| model.display_name() == label)
            })
            .map(|model| model.id().to_owned())
        else {
            return;
        };
        self.settings.assistant.choice_mut().model = Some(model_id);
        self.settings.assistant.choice_mut().reasoning_effort = None;
        self.settings.assistant.choice_mut().service_tier = None;
        self.changed(cx);
    }

    pub(super) fn select_assistant_reasoning(&mut self, label: &str, cx: &mut Context<Self>) {
        self.settings.assistant.choice_mut().reasoning_effort = self
            .assistant_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.settings
                    .assistant
                    .choice()
                    .model
                    .as_deref()
                    .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                    .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
                    .or_else(|| snapshot.models().first())
            })
            .and_then(|model| {
                model
                    .reasoning_efforts()
                    .iter()
                    .find(|effort| reasoning_effort_label(effort.id()) == label)
            })
            .map(|effort| effort.id().to_owned());
        self.changed(cx);
    }

    pub(super) fn select_assistant_tier(&mut self, label: &str, cx: &mut Context<Self>) {
        let tier = if label == "Default" {
            None
        } else {
            let Some(tier) = self.assistant_state.snapshot.as_ref().and_then(|snapshot| {
                let model = self
                    .settings
                    .assistant
                    .choice()
                    .model
                    .as_deref()
                    .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                    .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
                    .or_else(|| snapshot.models().first())?;
                model
                    .service_tiers()
                    .iter()
                    .find(|tier| tier.name() == label)
                    .map(|tier| tier.id().to_owned())
            }) else {
                return;
            };
            Some(tier)
        };
        self.settings.assistant.choice_mut().service_tier = tier;
        self.changed(cx);
    }

    pub(in crate::ui) fn toggle_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.assistant.enabled {
            return;
        }
        self.assistant_state.open = !self.assistant_state.open;
        if self.assistant_state.open {
            self.assistant_state.previous_focus = window.focused(cx);
            if self.assistant_transcript_visible(window, cx)
                && let Some(thread) = self.displayed_thread()
            {
                self.thread_run_mut(&thread).unread = None;
            }
            self.assistant_state.idle_stop = None;
            self.start_assistant(cx);
            // A saved transcript shows while the harness starts.
            if let Some(thread) = self.displayed_thread() {
                self.load_assistant_thread(&thread, cx);
            }
            self.assistant_composer(cx)
                .update(cx, |composer, cx| composer.focus(window, cx));
        } else {
            if let Some(focus) = self.assistant_state.previous_focus.take() {
                focus.focus(window, cx);
            }
            if self.assistant_state.service.is_some() {
                self.note_codex_activity(cx);
                self.schedule_idle_codex_stop(CODEX_IDLE_TIMEOUT, window, cx);
            }
        }
        cx.notify();
    }

    /// Starts the idle period again after a Codex command or event.
    pub(super) fn note_codex_activity(&mut self, cx: &App) {
        self.assistant_state.idle_since = cx.background_executor().now();
    }

    pub(super) fn schedule_idle_codex_stop(
        &mut self,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant_state.idle_stop = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, window, cx| this.stop_idle_codex(window, cx));
        }));
    }

    /// Stops Codex after `CODEX_IDLE_TIMEOUT` with the pane closed and without
    /// Codex activity. Work that a new process would lose delays the stop.
    pub(super) fn stop_idle_codex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = &self.assistant_state;
        if panel.open || panel.service.is_none() {
            self.assistant_state.idle_stop = None;
            return;
        }
        let idle = cx
            .background_executor()
            .now()
            .saturating_duration_since(panel.idle_since);
        if idle < CODEX_IDLE_TIMEOUT {
            self.schedule_idle_codex_stop(CODEX_IDLE_TIMEOUT - idle, window, cx);
        } else if self.codex_has_work() {
            // The end of the work is Codex activity, which starts the idle
            // period again.
            self.schedule_idle_codex_stop(CODEX_IDLE_TIMEOUT, window, cx);
        } else {
            self.assistant_state.stop();
            self.reset_assistant_runs(window, cx);
            // The next open starts Codex like the first open, without an error.
            self.assistant_state.status = Status::Idle;
            cx.notify();
        }
    }

    pub(super) fn start_assistant(&mut self, cx: &mut Context<Self>) {
        if self.assistant_state.service.is_some() {
            return;
        }
        let harness = self.settings.assistant.harness;
        let executable = match discover_harness(harness, self.settings.assistant.executable()) {
            Ok(path) => path,
            Err(error) => {
                self.assistant_state.status = Status::Disconnected(error);
                cx.notify();
                return;
            }
        };
        let wake = self.wake.clone();
        // Claude Code files sessions by working folder, so its folder stays
        // the same next to the workspace. The demo uses a temporary folder.
        let working_directory = match harness {
            AssistantHarness::Claude => self
                .assistant_state
                .store
                .as_ref()
                .and_then(|store| store.directory().parent())
                .map(|assistant| assistant.join("claude")),
            AssistantHarness::Codex => None,
        };
        match Service::launch_harness(
            crate::assistant::service::Launch {
                harness,
                executable,
                working_directory,
            },
            Arc::new(move || {
                let _ = wake.try_send(());
            }),
        ) {
            Ok(service) => {
                self.assistant_state.service = Some(service);
                self.assistant_state.harness = Some(harness);
                self.assistant_state.status = Status::Starting;
                self.assistant_state.sign_in = SignIn::Idle;
                // A new Codex process has no title requests from the old one.
                self.assistant_state.regenerating_titles.clear();
                self.assistant_state.pending_titles.clear();
                self.assistant_state.title_history_reads.clear();
            }
            Err(error) => {
                self.assistant_state.status =
                    Status::Disconnected(format!("Could not start assistant: {error}"))
            }
        }
        cx.notify();
    }

    /// Selects the harness of new conversations and starts it. Running turns
    /// of the old harness stop, so Qrow asks first while one runs.
    pub(in crate::ui) fn select_assistant_harness(
        &mut self,
        harness: AssistantHarness,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.assistant.harness == harness {
            return;
        }
        if !self.assistant_working() {
            self.switch_assistant_harness(harness, window, cx);
            return;
        }
        let weak = cx.weak_entity();
        let name = harness.name();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            alert
                .title(format!("Switch the assistant to {name}?"))
                .description("Running assistant turns stop. Queries that already started continue.")
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-switch-harness")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-switch-harness")
                                .primary()
                                .label("Switch")
                                .on_click(move |_, window, cx| {
                                    let _ = confirm.update(cx, |this, cx| {
                                        this.switch_assistant_harness(harness, window, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn switch_assistant_harness(
        &mut self,
        harness: AssistantHarness,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.assistant.harness = harness;
        self.assistant_state.stop();
        self.reset_assistant_runs(window, cx);
        self.assistant_state.snapshot = None;
        self.assistant_state.sign_in = SignIn::Idle;
        self.assistant_state.notice = None;
        self.assistant_state.status = Status::Idle;
        if self.assistant_state.open {
            self.start_assistant(cx);
        }
        if let Some(thread) = self.displayed_thread() {
            self.load_assistant_thread(&thread, cx);
        }
        self.changed(cx);
    }

    pub(in crate::ui) fn reconnect_assistant(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant_state.stop();
        self.reset_assistant_runs(window, cx);
        self.start_assistant(cx);
    }

    pub(in crate::ui) fn stop_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread_id) = self.displayed_thread() else {
            return;
        };
        // Messages that wait for the turn do not start after Cancel.
        self.restore_queued_messages(&thread_id, window, cx);
        let Some(turn_id) = self
            .thread_run(&thread_id)
            .and_then(|run| run.active_turn.clone())
        else {
            return;
        };
        self.assistant_command(
            AssistantCommand::Interrupt {
                thread_id: thread_id.clone(),
                turn_id,
            },
            cx,
        );
        self.thread_run_mut(&thread_id).pending_reply = false;
        cx.notify();
    }

    pub(in crate::ui) fn toggle_assistant_mode(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.mode_target() else {
            return;
        };
        if self.displayed_mode() == AssistantExecutionMode::RunAutomatically {
            self.set_mode(&target, AssistantExecutionMode::AskBeforeRunning, cx);
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            let target = target.clone();
            alert.title("Run assistant queries automatically?")
                .description("The assistant can run SQL that changes or deletes data and schema. Qrow cannot confirm that a statement is read-only.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-conversation-auto-run").label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-conversation-auto-run").with_variant(ButtonVariant::Danger)
                        .label("Run automatically")
                        .on_click(move |_, window, cx| {
                            let _ = confirm.update(cx, |this, cx| {
                                this.set_mode(&target, AssistantExecutionMode::RunAutomatically, cx);
                            });
                            window.close_dialog(cx);
                        })))
        });
    }

    pub(in crate::ui) fn assistant_command(
        &mut self,
        command: AssistantCommand,
        cx: &mut Context<Self>,
    ) -> bool {
        let result = self
            .assistant_state
            .service
            .as_ref()
            .map(|service| service.send(command));
        match result {
            Some(Ok(())) => {
                self.note_codex_activity(cx);
                true
            }
            Some(Err(error)) => {
                self.assistant_state.status = Status::Disconnected(error.into());
                cx.notify();
                false
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn reasoning_labels_keep_codex_ids_separate_from_display_text() {
        assert_eq!(reasoning_effort_label("low"), "Low");
        assert_eq!(reasoning_effort_label("medium"), "Medium");
        assert_eq!(reasoning_effort_label("high"), "High");
        assert_eq!(reasoning_effort_label("xhigh"), "Extra high");
        assert_eq!(reasoning_effort_label("max"), "Max");
    }
}

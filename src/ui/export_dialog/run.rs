//! Captured SQL and session checks for executions started by the export form.
use super::*;
use std::io;

#[derive(Clone)]
pub(super) struct Intent {
    pub profile: Profile,
    pub sql: String,
    pub expected: Option<Uuid>,
    pub warning: bool,
}

impl Qrow {
    pub(in crate::ui) fn open_run_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.active;
        if self.tabs[index].busy || self.form.is_some() || self.settings_open {
            return;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| Some(profile.id) == self.tabs[index].saved.profile)
            .cloned()
        else {
            window.push_notification("Choose a connection before running SQL.", cx);
            return;
        };
        let sql = match self.selected_tab_sql(index, window, cx) {
            Ok(sql) => sql,
            Err(message) => {
                window.push_notification(message, cx);
                return;
            }
        };
        if let Err(error) = crate::sql::validate_single_for(&sql, profile.database_type)
            .and_then(|_| profile.validate())
        {
            window.push_notification(error.to_string(), cx);
            return;
        }
        let tab = &self.tabs[index];
        let intent = Intent {
            expected: tab
                .worker
                .as_ref()
                .and_then(|worker| worker.session_generation(&profile)),
            profile,
            sql,
            warning: false,
        };
        let source = match if self.demo {
            let data = tab.table.read(cx).delegate();
            Snapshot::new(&data.columns, &data.rows)
        } else {
            Snapshot::new(&[], &export::Rows::default())
        } {
            Ok(source) => Arc::new(source),
            Err(error) => {
                window.push_notification(error.to_string(), cx);
                return;
            }
        };
        let result = ExportResult {
            tab: tab.saved.id,
            execution: None,
            cursor: crate::worker::Cursor::Unavailable,
            replay: None,
        };
        let settings = self.settings.export.clone();
        let filename = tab.saved.title.clone();
        let owner = cx.weak_entity();
        let view = cx.new(|cx| {
            let mut view = ExportDialog::new(
                source, None, settings, filename, true, result, owner, window, cx,
            );
            view.run = Some(intent);
            view.scopes = vec![Scope::Run];
            view.choices[0] = vec!["Run and export (all rows)".into()];
            view.scope = Scope::Run;
            view.controls[0].update(cx, |control, cx| {
                control.set_items(SearchableVec::new(view.choices[0].clone()), window, cx);
            });
            view.sync(window, cx);
            view
        });
        ExportDialog::open(&view, window, cx);
    }

    pub(super) fn export_result_intent(&self, tab: &Tab) -> Option<Intent> {
        let original = tab.result_profile.as_ref()?;
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.id == original.id)?
            .clone();
        let expected = tab
            .worker
            .as_ref()
            .and_then(|worker| worker.session_generation(&profile));
        Some(Intent {
            warning: expected.is_none()
                || expected != tab.result_session
                || !original.connection_identity_eq(&profile),
            profile,
            sql: tab.submitted_sql.clone()?,
            expected,
        })
    }

    pub(super) fn submit_export(
        &mut self,
        tab_id: Uuid,
        intent: &Intent,
        jobs: &export::Jobs,
        cancel: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) -> io::Result<(ExecutionId, Arc<crate::worker::Download>)> {
        self.check_export_intent(tab_id, intent)?;
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.saved.id == tab_id)
            .ok_or_else(|| io::Error::other("The result tab closed."))?;
        let credentials = self.credential_provider();
        let tab = &mut self.tabs[index];
        if tab.busy {
            return Err(io::Error::other("The query tab is busy."));
        }
        if tab.release_pending {
            tab.release_pending = false;
            if let Some(worker) = tab.worker.take() {
                worker.shutdown();
            }
            tab.worker_profile = None;
            tab.connected = false;
        }
        if tab.worker.is_none() {
            let wake = self.wake.clone();
            tab.worker = Some(Worker::with_connector(
                Arc::new(move || {
                    let _ = wake.try_send(());
                }),
                self.connector.clone(),
                credentials,
            ));
        }
        let execution = Self::allocate_execution_id(tab);
        let download = tab.worker.as_ref().unwrap().run_and_export(
            intent.profile.clone(),
            intent.sql.clone(),
            execution,
            intent.expected,
            jobs,
            cancel,
        )?;
        tab.table.update(cx, |table, cx| {
            table.delegate_mut().clear();
            table.delegate_mut().empty_message = Some("Waiting for query results…");
            table.clear_selection(cx);
            table
                .horizontal_scroll_handle
                .set_offset(point(px(0.), px(0.)));
            table.scroll_to_row(0, cx);
            table.refresh(cx);
        });
        tab.more = false;
        tab.pending_page = None;
        tab.elapsed = None;
        tab.preview_complete = false;
        tab.cursor = crate::worker::Cursor::Unavailable;
        tab.replay = None;
        tab.download = Some(download.clone());
        tab.busy = true;
        tab.panel.execution_started();
        tab.cancelling = false;
        tab.started = Some(Instant::now());
        tab.set_status("Preparing export…");
        tab.current_execution = Some(execution);
        tab.worker_profile = Some(intent.profile.id);
        tab.result_profile = Some(intent.profile.clone());
        tab.result_session = None;
        tab.submitted_sql = Some(intent.sql.clone());
        let submission = LogEvent::new(
            Some(execution),
            Severity::Info,
            LogKind::Submitted,
            format!("Submitted query:\n{}", intent.sql),
        )
        .with_connection(intent.profile.name.clone())
        .with_sql(intent.sql.clone());
        let entry = crate::activity::from_tab(&submission, tab.saved.id, &tab.saved.title);
        Self::record_log(tab, submission);
        if let Some(entry) = entry {
            self.record_activity(intent.profile.id, entry, cx);
        }
        cx.notify();
        Ok((execution, download))
    }

    fn check_export_intent(&self, tab_id: Uuid, intent: &Intent) -> io::Result<()> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == tab_id)
            .ok_or_else(|| io::Error::other("The result tab closed."))?;
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.id == intent.profile.id)
            .ok_or_else(|| io::Error::other("The export connection was removed."))?;
        if !profile.connection_identity_eq(&intent.profile) {
            return Err(io::Error::other(
                "The connection changed. Close this form and review the export again.",
            ));
        }
        let generation = tab
            .worker
            .as_ref()
            .and_then(|worker| worker.session_generation(profile));
        if generation != intent.expected || tab.release_pending && intent.expected.is_some() {
            return Err(io::Error::other(crate::worker::SessionChanged));
        }
        Ok(())
    }
}

impl ExportDialog {
    pub(super) fn authenticate_run(&self, cx: &mut Context<Self>) -> io::Result<Option<Uuid>> {
        if !self.scope.executes() {
            return Ok(None);
        }
        let intent = self
            .run
            .as_ref()
            .ok_or_else(|| io::Error::other("No captured SQL is available."))?;
        self.owner
            .update(cx, |owner, cx| {
                owner.check_export_intent(self.result.tab, intent)?;
                let Some(sign_in) = intent.profile.authentication.sign_in() else {
                    return Ok(None);
                };
                if owner.sign_in_changing(sign_in)
                    || intent.expected.is_none() && owner.sign_in_needs_browser(sign_in)
                {
                    owner.start_sign_in(sign_in, cx);
                    Ok(Some(sign_in))
                } else {
                    Ok(None)
                }
            })
            .map_err(|_| io::Error::other("The result window closed."))?
    }

    pub(super) fn sign_in_ready(&self, id: Uuid, cx: &mut Context<Self>) -> io::Result<bool> {
        export::check_cancelled(&self.cancel)?;
        self.owner
            .update(cx, |owner, _| {
                if owner.sign_in_changing(id) {
                    return Ok(false);
                }
                match owner.oidc.status(id) {
                    crate::oidc::Status::SignedIn(_) => Ok(true),
                    crate::oidc::Status::SignInRequired(_, message)
                    | crate::oidc::Status::NetworkFailure(_, message) => Err(io::Error::other(
                        format!("Sign-in failed. The SQL did not run: {message}"),
                    )),
                    crate::oidc::Status::SignedOut => Err(io::Error::other(
                        "Sign-in did not complete. The SQL did not run.",
                    )),
                }
            })
            .map_err(|_| io::Error::other("The result window closed."))?
    }

    pub(super) fn refresh_session_review(&mut self, cx: &mut Context<Self>) {
        let Some(intent) = &mut self.run else {
            return;
        };
        let _ = self.owner.update(cx, |owner, _| {
            intent.expected = owner
                .tabs
                .iter()
                .find(|tab| tab.saved.id == self.result.tab)
                .and_then(|tab| tab.worker.as_ref())
                .and_then(|worker| worker.session_generation(&intent.profile));
            intent.warning = true;
        });
    }

    pub(super) fn refresh_preview_source(&mut self, cx: &mut Context<Self>) {
        if self.run.is_none() || self.download.is_none() {
            return;
        }
        let source = self.owner.upgrade().and_then(|owner| {
            let tab = owner.read(cx).tabs.iter().find(|tab| {
                tab.saved.id == self.result.tab && tab.current_execution == self.result.execution
            })?;
            let data = tab.table.read(cx).delegate();
            if data.columns.is_empty()
                || self.source.column_count() > 0 && data.rows.len() == self.source.row_count()
            {
                return None;
            }
            Snapshot::new(&data.columns, &data.rows).ok()
        });
        if let Some(source) = source {
            self.source = Arc::new(source);
            self.update_preview(cx);
        }
    }

    pub(super) fn run_source(
        &mut self,
        jobs: &export::Jobs,
        cx: &mut Context<Self>,
    ) -> io::Result<Arc<crate::worker::Download>> {
        let intent = self
            .run
            .as_ref()
            .ok_or_else(|| io::Error::other("No captured SQL is available."))?
            .clone();
        let preview = Arc::new(Snapshot::new(&[], &export::Rows::default())?);
        let (execution, download) = self
            .owner
            .update(cx, |owner, cx| {
                owner.submit_export(self.result.tab, &intent, jobs, self.cancel.clone(), cx)
            })
            .map_err(|_| io::Error::other("The result window closed."))??;
        self.result.execution = Some(execution);
        self.source = preview;
        self.update_preview(cx);
        self.result.replay = None;
        self.retained = None;
        self.download = Some(download.clone());
        if let Some(intent) = &mut self.run {
            // The execution's actual generation is refreshed when a failed
            // source requires a reviewed rerun.
            intent.warning = false;
        }
        Ok(download)
    }
}

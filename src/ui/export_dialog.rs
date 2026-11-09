use super::*;
pub(super) mod background;
mod run;
mod transfer;
use crate::export::{
    self, Format, Snapshot,
    csv::{CsvOptions, LineEnding, NullMarker, Preset, Separator},
};
use gpui_kit::component::{
    alert::Alert,
    checkbox::Checkbox,
    form::{field, v_form},
    notification::Notification,
    select::{Select, SelectState},
    v_flex,
};
use std::{
    hash::{Hash, Hasher},
    ops::{Range, RangeInclusive},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

type RangeSelection = (Range<usize>, RangeInclusive<usize>);
type Choice = Entity<SelectState<SearchableVec<String>>>;

#[derive(Clone)]
struct ExportResult {
    tab: Uuid,
    execution: Option<ExecutionId>,
    cursor: crate::worker::Cursor,
    replay: Option<Arc<export::spool::Spool>>,
}

#[derive(Clone, Copy)]
enum Scope {
    Complete,
    All,
    Replay,
    Run,
    RunAgain,
}

impl Scope {
    fn needs_full_source(self) -> bool {
        matches!(self, Self::All | Self::Replay | Self::Run | Self::RunAgain)
    }
    fn executes(self) -> bool {
        matches!(self, Self::Run | Self::RunAgain)
    }
}

enum ResultSource {
    Ready(Arc<export::spool::Spool>),
    Pending(Arc<crate::worker::Download>),
}
impl ResultSource {
    fn spool(&self) -> std::io::Result<Arc<export::spool::Spool>> {
        match self {
            Self::Ready(spool) => Ok(spool.clone()),
            Self::Pending(download) => download.wait_spool(),
        }
    }
}

impl Global for export::Jobs {}

struct ExportDialog {
    source: Arc<Snapshot>,
    scope: Scope,
    result: ExportResult,
    retained: Option<Arc<export::spool::Spool>>,
    download: Option<Arc<crate::worker::Download>>,
    run: Option<run::Intent>,
    profile: Option<Profile>,
    settings: export::Settings,
    filename: String,
    incomplete: bool,
    controls: Vec<Choice>,
    choices: Vec<Vec<String>>,
    null_text: Entity<InputState>,
    cell_width: Entity<InputState>,
    large_copy: bool,
    error: Option<String>,
    running: bool,
    id: Uuid,
    background: bool,
    output_bytes: Arc<AtomicU64>,
    started: Option<Instant>,
    output_path: Option<PathBuf>,
    progress_task: Option<Task<()>>,
    notice_generation: u64,
    cancel: Arc<AtomicBool>,
    owner: WeakEntity<Qrow>,
    dialog: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for ExportDialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel_download();
    }
}

impl Qrow {
    pub(super) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &self.tabs[self.active];
        let shown = tab.shown_result();
        let data = shown.table.read(cx).delegate();
        if data.columns.is_empty() {
            return;
        }
        let source = match Snapshot::new(&data.columns, &data.rows) {
            Ok(source) => Arc::new(source),
            Err(error) => {
                window.push_notification(
                    toast::error(
                        "export-error-toast",
                        "Could not export results",
                        error.to_string(),
                    ),
                    cx,
                );
                return;
            }
        };
        let settings = self.settings.export.clone();
        let filename = tab.saved.title.clone();
        let incomplete = !tab.result_complete(shown.id);
        // Only the live result has a cursor and a replay spool. An earlier
        // result exports its loaded rows, or runs its query again.
        let result = if tab.live == Some(shown.id) {
            ExportResult {
                tab: tab.saved.id,
                execution: tab.current_execution,
                cursor: tab.cursor,
                replay: tab
                    .replay
                    .clone()
                    .or_else(|| self.export_spool(tab.saved.id, tab.current_execution, None, cx)),
            }
        } else {
            ExportResult {
                tab: tab.saved.id,
                execution: None,
                cursor: crate::worker::Cursor::Unavailable,
                replay: None,
            }
        };
        let owner = cx.weak_entity();
        let intent = self.export_result_intent(shown);
        let profile = self
            .profiles
            .iter()
            .find(|profile| {
                Some(profile.id)
                    == shown
                        .profile
                        .as_ref()
                        .map(|profile| profile.id)
                        .or(tab.saved.profile)
            })
            .cloned()
            .or_else(|| shown.profile.clone());
        let view = cx.new(|cx| {
            let mut view = ExportDialog::new(
                source, settings, filename, incomplete, result, profile, owner, window, cx,
            );
            view.run = intent;
            if incomplete
                && view.result.replay.is_none()
                && view.result.cursor != crate::worker::Cursor::Available
                && view.run.is_some()
            {
                view.scope = Scope::RunAgain;
            }
            view
        });
        ExportDialog::open(&view, window, cx);
    }

    fn remember_export(&mut self, settings: export::Settings, cx: &mut Context<Self>) {
        self.settings.export = settings;
        for table in self.tabs.iter().flat_map(Tab::result_tables) {
            table.update(cx, |table, _| {
                table.delegate_mut().export = self.settings.export.clone();
            });
        }
        self.changed(cx);
    }
}

impl ExportDialog {
    fn open(view: &Entity<Self>, window: &mut Window, cx: &mut App) {
        view.update(cx, |view, cx| {
            if view.background && !view.running && matches!(view.scope, Scope::RunAgain) {
                view.refresh_run_session(cx);
            }
        });
        let dialog_view = view.clone();
        let view = view.clone();
        let close_view = view.downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close_view = close_view.clone();
            let submit = view.downgrade();
            dialog
                .title("Export Results")
                .w(setting_row::Rows::dialog_width(window, 36.))
                .overlay_closable(false)
                .child(view.clone())
                .footer(view.update(cx, |view, cx| view.render_footer(cx)))
                .on_ok(move |_, window, cx| {
                    let _ = submit.update(cx, |this, cx| this.start(true, window, cx));
                    false
                })
                .on_close(move |_, _, cx| {
                    let _ = close_view.update(cx, |this, _| {
                        if !this.background {
                            this.cancel.store(true, Ordering::Relaxed);
                            this.cancel_download();
                        }
                    });
                })
        });
        dialog_view.update(cx, |view, cx| {
            view.dialog = Root::read(window, cx).dialog_focus_handle().cloned();
        });
    }
    #[allow(clippy::too_many_arguments)]
    fn new(
        source: Arc<Snapshot>,
        mut settings: export::Settings,
        filename: String,
        incomplete: bool,
        result: ExportResult,
        profile: Option<Profile>,
        owner: WeakEntity<Qrow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if matches!(settings.format, Format::Json | Format::JsonLines) {
            settings.json.lines = settings.format == Format::JsonLines;
        }
        let choices = vec![
            Preset::ALL
                .iter()
                .map(|p| p.label().to_owned())
                .chain(["Custom".into()])
                .collect(),
            Separator::ALL
                .iter()
                .map(|s| s.label().to_owned())
                .collect(),
            LineEnding::ALL
                .iter()
                .map(|s| s.label().to_owned())
                .collect(),
            NullMarker::ALL
                .iter()
                .map(|s| s.label().to_owned())
                .chain(["Custom".into()])
                .collect(),
            vec![
                "CSV / TSV".into(),
                "Markdown".into(),
                "JSON".into(),
                "Parquet".into(),
            ],
            vec!["Table".into(), "Code block".into()],
            export::parquet::Compression::ALL
                .iter()
                .map(|value| value.label().to_owned())
                .collect(),
            vec!["Typed".into(), "Text".into()],
            export::parquet::Numeric::ALL
                .iter()
                .map(|value| value.label().to_owned())
                .collect(),
            vec!["Array".into(), "Lines".into()],
        ];
        let options = settings.csv.clone();
        let indices = option_indices(&settings);
        let controls: Vec<_> = choices
            .iter()
            .zip(indices)
            .map(|(labels, index)| {
                cx.new(|cx| {
                    SelectState::new(
                        SearchableVec::new(labels.clone()),
                        Some(gpui_kit::component::IndexPath::default().row(index)),
                        window,
                        cx,
                    )
                })
            })
            .collect();
        let null_text = cx.new(|cx| {
            InputState::new(window, cx).default_value(match &options.null {
                NullMarker::Custom(value) => value.clone(),
                _ => String::new(),
            })
        });
        let cell_width = cx.new(|cx| {
            InputState::new(window, cx).default_value(settings.markdown.max_cell_width.to_string())
        });
        let mut subscriptions: Vec<_> = controls
            .iter()
            .enumerate()
            .map(|(index, control)| {
                cx.subscribe_in(control, window, move |this, _, event, window, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else {
                        return;
                    };
                    let Some(choice) = this.choices[index].iter().position(|label| label == value)
                    else {
                        return;
                    };
                    match index {
                        0 => {
                            if let Some(preset) = Preset::ALL.get(choice) {
                                this.settings.csv = preset.options();
                            }
                        }
                        1 => this.settings.csv.separator = Separator::ALL[choice],
                        2 => this.settings.csv.line_ending = LineEnding::ALL[choice],
                        3 => {
                            this.settings.csv.null =
                                NullMarker::ALL.get(choice).cloned().unwrap_or_else(|| {
                                    NullMarker::Custom(this.null_text.read(cx).value().to_string())
                                })
                        }
                        4 => {
                            this.settings.format = match choice {
                                0 => Format::Csv,
                                1 => Format::Markdown,
                                2 if this.settings.json.lines => Format::JsonLines,
                                2 => Format::Json,
                                3 => Format::Parquet,
                                _ => unreachable!(),
                            }
                        }
                        5 => {
                            this.settings.markdown.style = if choice == 0 {
                                export::markdown::Style::Table
                            } else {
                                export::markdown::Style::CodeBlock
                            }
                        }
                        6 => {
                            this.settings.parquet.compression =
                                export::parquet::Compression::ALL[choice]
                        }
                        7 => {
                            this.settings.parquet.column_types = if choice == 0 {
                                export::parquet::ColumnTypes::Typed
                            } else {
                                export::parquet::ColumnTypes::Text
                            }
                        }
                        8 => this.settings.parquet.numeric = export::parquet::Numeric::ALL[choice],
                        9 => {
                            this.settings.json.lines = choice == 1;
                            this.settings.format = if choice == 1 {
                                Format::JsonLines
                            } else {
                                Format::Json
                            };
                        }
                        _ => unreachable!(),
                    }
                    this.sync(window, cx);
                })
            })
            .collect();
        subscriptions.push(cx.subscribe_in(
            &null_text,
            window,
            |this, input, event, window, cx| {
                if matches!(event, InputEvent::Change)
                    && matches!(this.settings.csv.null, NullMarker::Custom(_))
                {
                    this.settings.csv.null = NullMarker::Custom(input.read(cx).value().to_string());
                    this.sync(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &cell_width,
            window,
            |this, input, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.settings.markdown.max_cell_width =
                        input.read(cx).value().parse().unwrap_or(0);
                    this.sync(window, cx);
                }
            },
        ));
        Self {
            source,
            scope: if result.replay.is_some() {
                Scope::Replay
            } else if incomplete {
                Scope::All
            } else {
                Scope::Complete
            },
            result,
            retained: None,
            download: None,
            run: None,
            profile,
            settings,
            filename,
            incomplete,
            controls,
            choices,
            null_text,
            cell_width,
            large_copy: false,
            error: None,
            running: false,
            id: Uuid::new_v4(),
            background: false,
            output_bytes: Arc::new(AtomicU64::new(0)),
            started: None,
            output_path: None,
            progress_task: None,
            notice_generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            owner,
            dialog: None,
            _subscriptions: subscriptions,
        }
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let indices = option_indices(&self.settings);
        self.large_copy = false;
        for (control, index) in self.controls.iter().zip(indices) {
            control.update(cx, |select, cx| {
                select.set_selected_index(
                    Some(gpui_kit::component::IndexPath::default().row(index)),
                    window,
                    cx,
                )
            });
        }
        self.error = self
            .settings
            .validate()
            .err()
            .map(|error| error.to_string());
        cx.notify();
    }

    fn start(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.start_with_writer(save, background::writer, window, cx);
    }

    fn start_with_writer(
        &mut self,
        save: bool,
        writer: impl FnOnce() -> std::io::Result<background::WriterSender> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running || !save && self.settings.format == Format::Parquet {
            return;
        }
        if let Err(error) = self.settings.validate() {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.running = true;
        self.error = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let source = self.source.clone();
        let options = self.settings.clone();
        let jobs = cx.global::<export::Jobs>().clone();
        let directory = self
            .settings
            .directory
            .clone()
            .filter(|path| path.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let suggested = export_filename(&self.filename, options.extension());
        let prompt = save.then(|| cx.prompt_for_new_path(&directory, Some(&suggested)));
        let request = (!save).then(|| CopyRequest::new(cx));
        cx.notify();
        cx.spawn_in(window, async move |weak, cx| {
            let path = if let Some(prompt) = prompt {
                match prompt.await {
                    Ok(Ok(Some(path))) => Some(path),
                    Ok(Ok(None)) => {
                        let _ = weak.update_in(cx, |this, _, cx| {
                            this.running = false;
                            cx.notify();
                        });
                        return;
                    }
                    error => {
                        let _ = weak.update_in(cx, |this, _, cx| {
                            this.running = false;
                            this.error = Some(format!("Could not open the save panel: {error:?}"));
                            cx.notify();
                        });
                        return;
                    }
                }
            } else {
                None
            };
            let guard = match weak.update_in(cx, |this, _, _| {
                export::check_cancelled(&cancel)?;
                match this.transfer_profile() {
                    Some(profile) => jobs.register_for_profile(cancel.clone(), None, profile.id,
                        profile.transfer.settings().concurrent_exports),
                    None => Ok(jobs.register(cancel.clone())),
                }
            }) {
                Ok(Ok(guard)) => guard,
                result => {
                    let _ = weak.update_in(cx, |this, window, cx| {
                        let error = match result { Ok(Err(error)) => error, _ => std::io::Error::other("The export form closed.") };
                        this.complete(Err(error), path, request, window, cx);
                    });
                    return;
                }
            };
            let authentication = weak.update_in(cx, |this, _, cx| this.authenticate_run(cx));
            let authentication = match authentication {
                Ok(Ok(authentication)) => authentication,
                error => {
                    let _ = weak.update_in(cx, |this, window, cx| {
                        let error = match error { Ok(Err(error)) => error, _ => std::io::Error::other("The export form closed.") };
                        this.complete(Err(error), path, request, window, cx);
                    });
                    return;
                }
            };
            if let Some(id) = authentication {
                loop {
                    match weak.update_in(cx, |this, _, cx| this.sign_in_ready(id, cx)) {
                        Ok(Ok(true)) => break,
                        Ok(Ok(false)) => cx.background_executor().timer(Duration::from_millis(100)).await,
                        error => {
                            let _ = weak.update_in(cx, |this, window, cx| {
                                let error = match error { Ok(Err(error)) => error, _ => std::io::Error::other("The export form closed.") };
                                this.complete(Err(error), path, request, window, cx);
                            });
                            return;
                        }
                    }
                }
            }
            let writer = match writer() {
                Ok(writer) => writer,
                Err(error) => {
                    let _ = weak.update_in(cx, |this, window, cx| {
                        this.complete(Err(error), path, request, window, cx);
                    });
                    return;
                }
            };
            let all_source = match weak.update_in(cx, |this, _, cx| this.all_source(&jobs, cx)) {
                Ok(Ok(source)) => source,
                result => {
                    let _ = weak.update_in(cx, |this, _, cx| {
                        this.running = false;
                        if matches!(&result, Ok(Err(error)) if error.get_ref().is_some_and(|cause| cause.is::<crate::worker::SessionChanged>())) {
                            this.refresh_session_review(cx);
                        }
                        this.error = Some(match result {
                            Ok(Err(error)) => error.to_string(),
                            _ => "The export dialog closed.".into(),
                        });
                        cx.notify();
                    });
                    return;
                }
            };
            let download = weak
                .update(cx, |this, _| this.download.clone())
                .ok()
                .flatten();
            let output_path = path.clone();
            let bytes = weak
                .update_in(cx, |this, window, cx| {
                    this.output_bytes.store(0, Ordering::Relaxed);
                    this.started = Some(Instant::now());
                    this.output_path = path.clone();
                    if path.is_some() && this.scope.needs_full_source() {
                        this.start_background(window, cx);
                    } else if path.is_none() && this.scope.needs_full_source() {
                        this.start_progress(cx);
                    }
                    this.output_bytes.clone()
                })
                .unwrap_or_else(|_| Arc::new(AtomicU64::new(0)));
            let failed_download = download.clone();
            let copying = path.is_none();
            let (done, finished) = async_channel::bounded(1);
            let admitted = writer.send(Box::new(move || {
                let _guard = guard;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let table = source.table(None);
                    let result: std::io::Result<(usize, Option<export::Text>)> =
                        if let Some(path) = output_path {
                            export::save(&path, &cancel, |out| {
                                let mut out = background::Output { out, bytes: &bytes };
                                if let Some(spool) = &all_source {
                                    export::stream::write(&mut out, &spool.spool()?, &options, &cancel)
                                } else {
                                    export::write(&mut out, &table, &options, &cancel)
                                }
                            })
                            .map(|count| (count, None))
                        } else {
                            let mut out = export::LimitedWriter::new(export::CLIPBOARD_BYTES);
                            let count = {
                                let mut output = background::Output { out: &mut out, bytes: &bytes };
                                if let Some(spool) = &all_source {
                                    spool.spool().and_then(|spool| {
                                        spool.wait_complete(&cancel)?;
                                        export::stream::write(&mut output, &spool, &options, &cancel)
                                    })
                                } else {
                                    export::write(&mut output, &table, &options, &cancel)
                                }
                            };
                            count
                                .and_then(|count| out.into_text().map(|text| (count, Some(text))))
                        };
                    result
                }))
                .unwrap_or_else(|_| Err(std::io::Error::other("The export writer panicked.")));
                if let Err(error) = &result
                    && let Some(download) = &download
                    && !(copying && download.spool().is_some_and(|spool| {
                        matches!(spool.status(), export::spool::Status::Complete { .. })
                    }))
                {
                    if error
                        .get_ref()
                        .is_some_and(|cause| cause.is::<export::Cancelled>())
                    {
                        download.cancel();
                    } else {
                        download.fail(error.to_string());
                    }
                }
                let _ = done.send_blocking(result);
            }));
            let result = match admitted {
                Ok(()) => match finished.recv().await {
                    Ok(result) => result,
                    Err(_) => writer_stopped(
                        failed_download,
                        std::io::Error::other("The export writer stopped unexpectedly."),
                    ),
                },
                Err(_) => writer_stopped(
                    failed_download,
                    std::io::Error::other("The export writer stopped before receiving its work."),
                ),
            };
            let _ = weak.update_in(cx, |this, window, cx| {
                this.complete(result, path, request, window, cx);
            });
        })
        .detach();
    }

    fn cancel_download(&self) {
        if let Some(download) = &self.download {
            download.cancel_in_background();
        }
    }

    fn all_source(
        &mut self,
        jobs: &export::Jobs,
        cx: &mut Context<Self>,
    ) -> std::io::Result<Option<ResultSource>> {
        if !self.scope.needs_full_source() {
            return Ok(None);
        }
        export::check_cancelled(&self.cancel)?;
        if self.scope.executes() {
            if self
                .owner
                .upgrade()
                .is_some_and(|owner| owner.read(cx).demo)
            {
                return Ok(None);
            }
            return self
                .run_source(jobs, cx)
                .map(|download| Some(ResultSource::Pending(download)));
        }
        let replay = self
            .retained
            .as_ref()
            .filter(|spool| matches!(spool.status(), export::spool::Status::Complete { .. }))
            .or(self.result.replay.as_ref())
            .cloned()
            .or_else(|| {
                self.owner.upgrade().and_then(|owner| {
                    owner.read(cx).export_spool(
                        self.result.tab,
                        self.result.execution,
                        Some(cx.entity().entity_id()),
                        cx,
                    )
                })
            });
        if let Some(spool) = replay {
            self.retained = Some(spool.clone());
            return Ok(Some(ResultSource::Ready(spool)));
        }
        if self.result.cursor == crate::worker::Cursor::Complete || !self.incomplete {
            return Ok(None);
        }
        let result = self.result.clone();
        let source = self.source.clone();
        let cancel = self.cancel.clone();
        let transfer = self
            .profile
            .as_ref()
            .map(|profile| profile.transfer.clone());
        let download = self
            .owner
            .update(cx, |owner, cx| -> std::io::Result<_> {
                let tab = owner
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.saved.id == result.tab)
                    .ok_or_else(|| std::io::Error::other("The result tab closed."))?;
                if tab.busy
                    || tab.current_execution != result.execution
                    || tab.cursor != crate::worker::Cursor::Available
                {
                    return Err(std::io::Error::other(
                        "The result cursor is unavailable. Run the query again to export all rows.",
                    ));
                }
                let execution = result
                    .execution
                    .ok_or_else(|| std::io::Error::other("No result execution is available."))?;
                let download = tab
                    .worker
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("The query worker stopped."))?
                    .drain_with_transfer(execution, source, jobs, cancel, transfer)?;
                tab.download = Some(download.clone());
                tab.busy = true;
                tab.cancelling = false;
                tab.set_status("Downloading export…");
                cx.notify();
                Ok(download)
            })
            .map_err(|_| std::io::Error::other("The result window closed."))??;
        let spool = download
            .spool()
            .ok_or_else(|| std::io::Error::other("The cursor drain has no result spool."))?;
        self.retained = Some(spool.clone());
        self.download = Some(download);
        Ok(Some(ResultSource::Ready(spool)))
    }
    fn complete(
        &mut self,
        mut result: std::io::Result<(usize, Option<export::Text>)>,
        path: Option<PathBuf>,
        request: Option<CopyRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        self.progress_task = None;
        self.refresh_preview_source(cx);
        if let Some(spool) = self.download.as_ref().and_then(|download| download.spool()) {
            self.retained = Some(spool);
        }
        if self.download.is_some() {
            if let Some(spool) = &self.retained
                && let export::spool::Status::Complete { rows } = spool.status()
            {
                self.scope = Scope::Replay;
                self.incomplete = rows != self.source.row_count() as u64;
            } else if self.run.is_some() {
                self.scope = Scope::RunAgain;
            }
            // Internal source changes must not discard a pending large-copy
            // confirmation. User changes still clear it through `sync`.
            let large_copy = self.large_copy;
            self.sync(window, cx);
            self.large_copy = large_copy;
        }
        if self.download.is_some() {
            self.refresh_run_session(cx);
        }
        // A producer failure also stops format work. Preserve its cause when
        // the writer observes the stop flag before the failed spool record.
        if result.as_ref().err().is_some_and(|error| {
            error
                .get_ref()
                .is_some_and(|inner| inner.is::<export::Cancelled>())
        }) && let Some(spool) = &self.retained
            && let export::spool::Status::Failed(message) = spool.status()
        {
            result = Err(std::io::Error::other(message));
        }
        if self.cancel.load(Ordering::Relaxed)
            && result.as_ref().err().is_some_and(|error| {
                error
                    .get_ref()
                    .is_some_and(|inner| inner.is::<export::Cancelled>())
            })
        {
            self.remove_background(window, cx);
            cx.notify();
            return;
        }
        match result {
            Ok((count, text)) => {
                if let Some(text) = text {
                    if self.settings.format == Format::Markdown
                        && text.chars().count() > 40_000
                        && !self.large_copy
                    {
                        self.large_copy = true;
                        cx.notify();
                        return;
                    }
                    if !request
                        .as_ref()
                        .is_some_and(|request| request.is_current(cx))
                    {
                        self.error =
                            Some("The clipboard changed. Choose Copy to try again.".into());
                        cx.notify();
                        return;
                    }
                    copy_text(text.into_string(), cx);
                }
                if let Some(path) = &path {
                    self.settings.directory = path.parent().map(PathBuf::from);
                }
                let settings = self.settings.clone();
                let _ = self
                    .owner
                    .update(cx, |owner, cx| owner.remember_export(settings, cx));
                if let Some(handle) = &self.dialog {
                    Root::update(window, cx, |root, window, cx| {
                        root.close_dialog_for(handle, window, cx)
                    });
                }
                if let Some(path) = path {
                    // A background job can finish while the user works elsewhere,
                    // so this notification stays until it is closed.
                    window.push_notification(
                        crate::ui::toast::saved_file(
                            crate::ui::toast::SavedFileIds {
                                notice: "export-complete-notice",
                                title: "export-complete-detail",
                                filename: "export-notification-filename",
                                reveal: "reveal-export",
                            },
                            format!("Exported {count} rows"),
                            path,
                        ),
                        cx,
                    );
                }
                self.remove_background(window, cx);
            }
            Err(error) => {
                if error
                    .get_ref()
                    .is_some_and(|cause| cause.is::<crate::worker::SessionChanged>())
                {
                    self.refresh_session_review(cx);
                }
                self.error = Some(error.to_string());
                if self.background {
                    self.show_progress_notification(window, cx);
                }
            }
        }
        cx.notify();
    }
}

fn writer_stopped(
    download: Option<Arc<crate::worker::Download>>,
    error: std::io::Error,
) -> std::io::Result<(usize, Option<export::Text>)> {
    if let Some(download) = download {
        download.abort(error.to_string());
    }
    Err(error)
}

fn option_indices(settings: &export::Settings) -> [usize; 10] {
    let options = &settings.csv;
    [
        preset_index(options),
        Separator::ALL
            .iter()
            .position(|value| *value == options.separator)
            .unwrap(),
        LineEnding::ALL
            .iter()
            .position(|value| *value == options.line_ending)
            .unwrap(),
        NullMarker::ALL
            .iter()
            .position(|value| *value == options.null)
            .unwrap_or(3),
        match settings.format {
            Format::Csv => 0,
            Format::Markdown => 1,
            Format::Json | Format::JsonLines => 2,
            Format::Parquet => 3,
        },
        usize::from(settings.markdown.style == export::markdown::Style::CodeBlock),
        export::parquet::Compression::ALL
            .iter()
            .position(|value| *value == settings.parquet.compression)
            .unwrap(),
        usize::from(settings.parquet.column_types == export::parquet::ColumnTypes::Text),
        export::parquet::Numeric::ALL
            .iter()
            .position(|value| *value == settings.parquet.numeric)
            .unwrap(),
        usize::from(settings.json.lines),
    ]
}

fn preset_index(options: &CsvOptions) -> usize {
    options
        .preset()
        .and_then(|preset| Preset::ALL.iter().position(|p| *p == preset))
        .unwrap_or(4)
}

fn export_filename(title: &str, extension: &str) -> String {
    let title: String = title
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | ':' | '\\') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    format!(
        "{}.{extension}",
        if title.trim().is_empty() {
            "results"
        } else {
            title.trim()
        }
    )
}

impl Render for ExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.running;
        let format = self.settings.format;
        let labels = [
            "Preset",
            "Separator",
            "Line Ending",
            "Null Values",
            "Format",
            "Style",
            "Compression",
            "Column Types",
            "Numeric As",
            "Style",
        ];
        let ids = [
            "export-preset",
            "export-separator",
            "export-line-ending",
            "export-null",
            "export-format",
            "export-markdown-style",
            "export-parquet-compression",
            "export-parquet-types",
            "export-parquet-numeric",
            "export-json-style",
        ];
        let options = &self.settings.csv;
        let flags = [
            ("export-header", "Include column names", options.header),
            (
                "export-bom",
                "UTF-8 byte-order mark",
                options.byte_order_mark,
            ),
            ("export-quotes", "Quote all values", options.quote_all),
            (
                "export-formulas",
                "Escape spreadsheet formulas",
                options.escape_formulas,
            ),
        ];
        let json_flags = [
            (
                "export-json-typed",
                "Typed values",
                self.settings.json.typed,
            ),
            (
                "export-json-pretty",
                "Pretty output",
                self.settings.json.pretty,
            ),
            (
                "export-json-decimals",
                "Decimals as numbers",
                self.settings.json.decimals_as_numbers,
            ),
        ];
        let custom_null = format == Format::Csv && matches!(options.null, NullMarker::Custom(_));
        v_flex()
            .id("export-form")
            .gap_4()
            .w_full()
            .child(
                v_form().columns(2).gap_x_4().children(
                    [4, 0, 1, 2, 3, 5, 9, 6, 7, 8]
                        .into_iter()
                        .filter(|index| {
                            *index == 4
                                || format == Format::Csv && (0..=3).contains(index)
                                || format == Format::Markdown && *index == 5
                                || matches!(format, Format::Json | Format::JsonLines) && *index == 9
                                || format == Format::Parquet && (6..=8).contains(index)
                        })
                        .map(|index| {
                            field().label(labels[index]).child(
                                Select::new(&self.controls[index])
                                    .id(ids[index])
                                    .disabled(
                                        busy || index == 8
                                            && self.settings.parquet.column_types
                                                == export::parquet::ColumnTypes::Text,
                                    )
                                    .w_full()
                                    .accessibility_label(labels[index]),
                            )
                        }),
                ),
            )
            .when(custom_null, |view| {
                view.child(
                    field()
                        .label("Null Marker")
                        .description("Exclude separators, quotes, and line breaks.")
                        .child(
                            Input::new(&self.null_text)
                                .id("export-null-text")
                                .disabled(busy)
                                .aria_label("Null Marker"),
                        ),
                )
            })
            .when(format == Format::Csv, |view| {
                view.child(v_flex().gap_2().children(flags.into_iter().enumerate().map(
                    |(index, (id, label, checked))| {
                        Checkbox::new(id)
                            .small()
                            .label(label)
                            .checked(checked)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, checked, window, cx| {
                                match index {
                                    0 => this.settings.csv.header = *checked,
                                    1 => this.settings.csv.byte_order_mark = *checked,
                                    2 => this.settings.csv.quote_all = *checked,
                                    3 => this.settings.csv.escape_formulas = *checked,
                                    _ => unreachable!(),
                                }
                                this.sync(window, cx);
                            }))
                    },
                )))
            })
            .when(format == Format::Markdown, |view| {
                view.when(
                    self.settings.markdown.style == export::markdown::Style::CodeBlock,
                    |view| {
                        view.child(
                            field()
                                .label("Maximum Cell Width")
                                .description("Use 1 to 1000 display columns.")
                                .child(
                                    Input::new(&self.cell_width)
                                        .id("export-cell-width")
                                        .disabled(busy)
                                        .aria_label("Maximum Cell Width"),
                                ),
                        )
                    },
                )
                .child(
                    v_flex()
                        .gap_2()
                        .when(
                            self.settings.markdown.style == export::markdown::Style::Table,
                            |view| {
                                view.child(
                                    Checkbox::new("export-markdown-pretty")
                                        .small()
                                        .label("Pretty output")
                                        .checked(self.settings.markdown.pretty)
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, checked, window, cx| {
                                            this.settings.markdown.pretty = *checked;
                                            this.sync(window, cx);
                                        })),
                                )
                            },
                        )
                        .child(
                            Checkbox::new("export-row-numbers")
                                .small()
                                .label("Include row numbers")
                                .checked(self.settings.markdown.row_numbers)
                                .disabled(busy)
                                .on_click(cx.listener(|this, checked, window, cx| {
                                    this.settings.markdown.row_numbers = *checked;
                                    this.sync(window, cx);
                                })),
                        ),
                )
            })
            .when(matches!(format, Format::Json | Format::JsonLines), |view| {
                view.child(
                    v_flex().gap_2().children(
                        json_flags
                            .into_iter()
                            .enumerate()
                            .filter(|(index, _)| *index != 1 || format == Format::Json)
                            .map(|(index, (id, label, checked))| {
                                Checkbox::new(id)
                                    .small()
                                    .label(label)
                                    .checked(checked)
                                    .disabled(busy || index == 2 && !self.settings.json.typed)
                                    .on_click(cx.listener(move |this, checked, window, cx| {
                                        match index {
                                            0 => this.settings.json.typed = *checked,
                                            1 => this.settings.json.pretty = *checked,
                                            2 => this.settings.json.decimals_as_numbers = *checked,
                                            _ => unreachable!(),
                                        }
                                        this.sync(window, cx);
                                    }))
                            }),
                    ),
                )
            })
            .when(
                format == Format::Parquet
                    && self.settings.parquet.column_types == export::parquet::ColumnTypes::Typed
                    && self.settings.parquet.numeric == export::parquet::Numeric::Double,
                |view| {
                    view.child(warning(
                        "export-double-warning",
                        "Double can change numeric values.",
                    ))
                },
            )
            .when(
                format == Format::Parquet && !self.source.table(None).rows.context().iso_dates(),
                |view| {
                    view.child(warning(
                        "export-date-style-warning",
                        "The result uses a non-ISO DateStyle. Dates and timestamps remain text.",
                    ))
                },
            )
            .when(self.large_copy, |view| {
                view.child(warning(
                    "export-copy-warning",
                    "Markdown exceeds 40,000 characters. It can exceed message limits.",
                ))
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .id("export-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .child(Alert::error("export-error-alert", error)),
                )
            })
            .when(busy, |view| {
                view.child(
                    div()
                        .id("export-progress")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(self.status_text())
                        .w_full()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.status_text()),
                )
            })
    }
}

/// Wraps a warning alert so tests and assistive technology can find it.
fn warning(id: &'static str, text: &'static str) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Alert)
        .aria_label(text)
        .child(Alert::warning(
            SharedString::from(format!("{id}-alert")),
            text,
        ))
}

impl ExportDialog {
    fn render_footer(&mut self, cx: &mut Context<Self>) -> DialogFooter {
        let busy = self.running;
        let invalid = self.settings.validate().is_err();
        let format = self.settings.format;
        DialogFooter::new()
            .justify_end()
            .child(
                Button::new("export-cancel")
                    .label(if self.background && !busy {
                        "Dismiss"
                    } else if self.background {
                        "Cancel export"
                    } else {
                        "Cancel"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel.store(true, Ordering::Relaxed);
                        this.cancel_download();
                        this.remove_background(window, cx);
                        window.close_dialog(cx);
                    })),
            )
            .child(
                Button::new("export-copy")
                    .label(if self.large_copy {
                        "Copy anyway"
                    } else {
                        "Copy"
                    })
                    .disabled(busy || invalid || format == Format::Parquet)
                    .on_click(cx.listener(|this, _, window, cx| this.start(false, window, cx))),
            )
            .child(
                Button::new("export-save")
                    .label("Save")
                    .primary()
                    .disabled(busy || invalid)
                    .on_click(cx.listener(|this, _, window, cx| this.start(true, window, cx))),
            )
    }
}

/// Context-menu copy shares the same writer and clipboard bound as the dialog.
pub(super) fn copy_format(
    source: Arc<Snapshot>,
    range: Option<RangeSelection>,
    options: export::Settings,
    profile: Option<(Uuid, u8)>,
    window: &mut Window,
    cx: &mut App,
) {
    let request = CopyRequest::new(cx);
    let format = options.format;
    let cancel = Arc::new(AtomicBool::new(false));
    let jobs = cx.global::<export::Jobs>();
    let guard = match profile {
        Some((id, limit)) => jobs.register_for_profile(cancel.clone(), None, id, limit),
        None => Ok(jobs.register(cancel.clone())),
    };
    let guard = match guard {
        Ok(guard) => guard,
        Err(error) => {
            window.push_notification(
                toast::error(
                    "copy-error-toast",
                    "Could not copy results",
                    error.to_string(),
                ),
                cx,
            );
            return;
        }
    };
    let task = cx.background_executor().spawn(async move {
        let _guard = guard;
        let mut out = export::LimitedWriter::new(export::CLIPBOARD_BYTES);
        export::write(&mut out, &source.table(range), &options, &cancel)?;
        out.into_text()
    });
    let window = window.window_handle();
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update_window(window, |_, window, cx| match result {
            Ok(text) if request.is_current(cx) => {
                if format == Format::Markdown && text.chars().count() > 40_000 {
                    let pending = std::rc::Rc::new(std::cell::RefCell::new(Some((request, text))));
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let button_pending = pending.clone();
                        let enter_pending = pending.clone();
                        alert
                            .title("Large Markdown")
                            .description(
                                "Markdown exceeds 40,000 characters. It can exceed message limits.",
                            )
                            .on_ok(move |_, window, cx| {
                                confirm_markdown_copy(&enter_pending, window, cx);
                                true
                            })
                            .footer(
                                DialogFooter::new()
                                    .justify_end()
                                    .child(
                                        Button::new("markdown-copy-cancel")
                                            .label("Cancel")
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        Button::new("markdown-copy-anyway")
                                            .label("Copy anyway")
                                            .primary()
                                            .on_click(move |_, window, cx| {
                                                confirm_markdown_copy(&button_pending, window, cx);
                                                window.close_dialog(cx);
                                            }),
                                    ),
                            )
                    });
                } else {
                    copy_text(text.into_string(), cx);
                }
            }
            Ok(_) => {}
            Err(error) => window.push_notification(
                toast::error(
                    "copy-error-toast",
                    "Could not copy results",
                    error.to_string(),
                ),
                cx,
            ),
        });
    })
    .detach();
}

fn confirm_markdown_copy(
    pending: &std::cell::RefCell<Option<(CopyRequest, export::Text)>>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some((request, text)) = pending.borrow_mut().take() {
        if request.is_current(cx) {
            copy_text(text.into_string(), cx);
        } else {
            window.push_notification(
                toast::error(
                    "copy-error-toast",
                    "Could not copy results",
                    "The clipboard changed. Choose Copy to try again.",
                ),
                cx,
            );
        }
    }
}

#[derive(Default)]
struct ClipboardCopies(u64);
impl Global for ClipboardCopies {}

struct CopyRequest {
    generation: u64,
    previous: Option<u64>,
}

impl CopyRequest {
    fn new(cx: &mut App) -> Self {
        let generation = next_copy(cx);
        Self {
            generation,
            previous: clipboard_fingerprint(cx),
        }
    }

    fn is_current(&self, cx: &mut App) -> bool {
        cx.global::<ClipboardCopies>().0 == self.generation
            && clipboard_fingerprint(cx) == self.previous
    }
}

fn next_copy(cx: &mut App) -> u64 {
    if !cx.has_global::<ClipboardCopies>() {
        cx.set_global(ClipboardCopies::default());
    }
    let generation = cx.global_mut::<ClipboardCopies>();
    generation.0 = generation.0.wrapping_add(1);
    generation.0
}

fn clipboard_fingerprint(cx: &mut App) -> Option<u64> {
    cx.read_from_clipboard().map(|item| {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        item.entries().len().hash(&mut hash);
        for entry in item.entries() {
            std::mem::discriminant(entry).hash(&mut hash);
            match entry {
                ClipboardEntry::String(text) => {
                    text.text.hash(&mut hash);
                    text.metadata.hash(&mut hash);
                }
                ClipboardEntry::Image(image) => {
                    image.format().hash(&mut hash);
                    image.hash(&mut hash);
                }
                ClipboardEntry::ExternalPaths(paths) => paths.0.hash(&mut hash),
            }
        }
        hash.finish()
    })
}

pub(super) fn copy_text(text: String, cx: &mut App) {
    next_copy(cx);
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

#[cfg(test)]
mod tests;

use super::*;
pub(super) mod background;
mod run;
use crate::export::{
    self, Format, Snapshot,
    csv::{CsvOptions, LineEnding, NullMarker, Preset, Separator},
};
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    checkbox::Checkbox,
    form::{field, v_form},
    h_flex,
    notification::Notification,
    scroll::ScrollableElement,
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
    Downloaded,
    Selection,
    All,
    Replay,
    Run,
    RunAgain,
}

impl Scope {
    fn file_only(self) -> bool {
        matches!(self, Self::All | Self::Replay | Self::Run | Self::RunAgain)
    }
    fn executes(self) -> bool {
        matches!(self, Self::Run | Self::RunAgain)
    }
}

enum FileSource {
    Ready(Arc<export::spool::Spool>),
    Pending(Arc<crate::worker::Download>),
}
impl FileSource {
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
    selection: Option<RangeSelection>,
    selected: bool,
    scope: Scope,
    scopes: Vec<Scope>,
    result: ExportResult,
    retained: Option<Arc<export::spool::Spool>>,
    download: Option<Arc<crate::worker::Download>>,
    run: Option<run::Intent>,
    settings: export::Settings,
    filename: String,
    incomplete: bool,
    controls: Vec<Choice>,
    choices: Vec<Vec<String>>,
    null_text: Entity<InputState>,
    cell_width: Entity<InputState>,
    large_copy: bool,
    preview: String,
    preview_pending: bool,
    preview_generation: u64,
    preview_cancel: Arc<AtomicBool>,
    preview_task: Option<Task<()>>,
    error: Option<String>,
    running: bool,
    id: Uuid,
    background: bool,
    output_bytes: Arc<AtomicU64>,
    started: Option<Instant>,
    output_path: Option<PathBuf>,
    progress_task: Option<Task<()>>,
    cancel: Arc<AtomicBool>,
    owner: WeakEntity<Qrow>,
    dialog: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for ExportDialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.cancel_download();
    }
}

impl Qrow {
    pub(super) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &self.tabs[self.active];
        let data = tab.table.read(cx).delegate();
        if data.columns.is_empty() {
            return;
        }
        let source = match Snapshot::new(&data.columns, &data.rows) {
            Ok(source) => Arc::new(source),
            Err(error) => {
                window.push_notification(error.to_string(), cx);
                return;
            }
        };
        let selection = data.selection.map(|selection| {
            (
                *selection.rows().start()..selection.rows().end() + 1,
                selection.columns(),
            )
        });
        let settings = self.settings.export.clone();
        let filename = tab.saved.title.clone();
        let incomplete = !tab.preview_complete;
        let result = ExportResult {
            tab: tab.saved.id,
            execution: tab.current_execution,
            cursor: tab.cursor,
            replay: tab
                .replay
                .clone()
                .or_else(|| self.export_spool(tab.saved.id, tab.current_execution, None, cx)),
        };
        let owner = cx.weak_entity();
        let intent = self.export_result_intent(tab);
        let view = cx.new(|cx| {
            let mut view = ExportDialog::new(
                source, selection, settings, filename, incomplete, result, owner, window, cx,
            );
            if incomplete && let Some(intent) = intent {
                view.run = Some(intent);
                view.scopes.push(Scope::RunAgain);
                view.choices[0].push("Run again and export (all rows)".into());
                view.controls[0].update(cx, |control, cx| {
                    control.set_items(SearchableVec::new(view.choices[0].clone()), window, cx)
                });
            }
            view
        });
        ExportDialog::open(&view, window, cx);
    }

    fn remember_export(&mut self, settings: export::Settings, cx: &mut Context<Self>) {
        self.settings.export = settings;
        for tab in &self.tabs {
            tab.table.update(cx, |table, _| {
                table.delegate_mut().export = self.settings.export.clone();
            });
        }
        self.changed(cx);
    }
}

impl ExportDialog {
    fn open(view: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let dialog_view = view.clone();
        let view = view.clone();
        let close_view = view.downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let close_view = close_view.clone();
            let submit = view.downgrade();
            dialog
                .title("Export Results")
                .w(setting_row::Rows::dialog_width(window, 36.))
                .overlay_closable(false)
                .child(view.clone())
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
        selection: Option<RangeSelection>,
        settings: export::Settings,
        filename: String,
        incomplete: bool,
        result: ExportResult,
        owner: WeakEntity<Qrow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut scopes = vec![format!("Downloaded rows ({})", source.row_count())];
        let mut row_scopes = vec![Scope::Downloaded];
        if let Some((rows, columns)) = &selection {
            scopes.push(format!(
                "Selection ({} rows, {} columns)",
                rows.len(),
                columns.clone().count()
            ));
            row_scopes.push(Scope::Selection);
        }
        scopes.push("All rows".into());
        row_scopes.push(Scope::All);
        if let Some(spool) = &result.replay {
            let count = match spool.status() {
                export::spool::Status::Complete { rows } => rows,
                _ => 0,
            };
            scopes.push(format!(
                "Export again as… ({count} rows, {:.1} MiB)",
                spool.bytes() as f64 / (1024. * 1024.)
            ));
            row_scopes.push(Scope::Replay);
        }
        let choices = vec![
            scopes,
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
            Format::ALL
                .iter()
                .map(|format| format.label().to_owned())
                .collect(),
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
        ];
        let selected = selection.is_some();
        let options = settings.csv.clone();
        let indices = option_indices(selected, &settings);
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
                            this.scope = this.scopes[choice];
                            this.selected = matches!(this.scope, Scope::Selection);
                        }
                        1 => {
                            if let Some(preset) = Preset::ALL.get(choice) {
                                this.settings.csv = preset.options();
                            }
                        }
                        2 => this.settings.csv.separator = Separator::ALL[choice],
                        3 => this.settings.csv.line_ending = LineEnding::ALL[choice],
                        4 => {
                            this.settings.csv.null =
                                NullMarker::ALL.get(choice).cloned().unwrap_or_else(|| {
                                    NullMarker::Custom(this.null_text.read(cx).value().to_string())
                                })
                        }
                        5 => this.settings.format = Format::ALL[choice],
                        6 => {
                            this.settings.markdown.style = if choice == 0 {
                                export::markdown::Style::Table
                            } else {
                                export::markdown::Style::CodeBlock
                            }
                        }
                        7 => {
                            this.settings.parquet.compression =
                                export::parquet::Compression::ALL[choice]
                        }
                        8 => {
                            this.settings.parquet.column_types = if choice == 0 {
                                export::parquet::ColumnTypes::Typed
                            } else {
                                export::parquet::ColumnTypes::Text
                            }
                        }
                        9 => this.settings.parquet.numeric = export::parquet::Numeric::ALL[choice],
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
        let mut this = Self {
            source,
            selection,
            selected,
            scope: if selected {
                Scope::Selection
            } else {
                Scope::Downloaded
            },
            scopes: row_scopes,
            result,
            retained: None,
            download: None,
            run: None,
            settings,
            filename,
            incomplete,
            controls,
            choices,
            null_text,
            cell_width,
            large_copy: false,
            preview: String::new(),
            preview_pending: false,
            preview_generation: 0,
            preview_cancel: Arc::new(AtomicBool::new(false)),
            preview_task: None,
            error: None,
            running: false,
            id: Uuid::new_v4(),
            background: false,
            output_bytes: Arc::new(AtomicU64::new(0)),
            started: None,
            output_path: None,
            progress_task: None,
            cancel: Arc::new(AtomicBool::new(false)),
            owner,
            dialog: None,
            _subscriptions: subscriptions,
        };
        this.update_preview(cx);
        this
    }

    fn range(&self) -> Option<RangeSelection> {
        if self.selected {
            self.selection.clone()
        } else {
            None
        }
    }

    fn update_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_task = None;
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview.clear();
        self.preview_pending = false;
        if matches!(self.scope, Scope::Run) || self.source.column_count() == 0 {
            self.preview = "The preview is available after the statement runs.".into();
            return;
        }
        if let Err(error) = self.settings.validate() {
            self.error = Some(error.to_string());
            self.preview.clear();
            return;
        }
        if self.settings.format == Format::Parquet {
            self.preview = "A Parquet file has no text preview.".into();
            return;
        }
        self.preview_cancel = Arc::new(AtomicBool::new(false));
        self.preview_pending = true;
        let cancel = self.preview_cancel.clone();
        let source = self.source.clone();
        let range = self.range();
        let settings = self.settings.clone();
        let generation = self.preview_generation;
        let task = cx.background_executor().spawn(async move {
            let mut table = source.table(range);
            table.row_indices.end = table.row_indices.end.min(table.row_indices.start + 5);
            let mut out = export::LimitedWriter::new(8192);
            let truncated = export::write(&mut out, &table, &settings, &cancel).is_err();
            out.into_preview(truncated)
        });
        self.preview_task = Some(cx.spawn(async move |weak, cx| {
            let preview = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.preview_generation == generation {
                    this.preview = preview.into_string();
                    this.preview_pending = false;
                    cx.notify();
                }
            });
        }));
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut indices = option_indices(self.selected, &self.settings);
        indices[0] = self
            .scopes
            .iter()
            .position(|scope| std::mem::discriminant(scope) == std::mem::discriminant(&self.scope))
            .unwrap();
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
        self.error = None;
        self.update_preview(cx);
        cx.notify();
    }

    fn start(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.start_with_writer(save, background::writer, window, cx);
    }

    fn start_with_writer(
        &mut self,
        save: bool,
        writer: fn() -> std::io::Result<background::WriterSender>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running
            || !save && (self.settings.format == Format::Parquet || self.scope.file_only())
        {
            return;
        }
        if let Err(error) = self.settings.validate() {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.running = true;
        self.error = None;
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let source = self.source.clone();
        let range = self.range();
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
                let _guard = jobs.register(cancel.clone());
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
                    if path.is_some() && this.scope.file_only() {
                        this.start_background(window, cx);
                    }
                    this.output_bytes.clone()
                })
                .unwrap_or_else(|_| Arc::new(AtomicU64::new(0)));
            let guard = jobs.register(cancel.clone());
            let failed_download = download.clone();
            let (done, finished) = async_channel::bounded(1);
            let admitted = writer.send(Box::new(move || {
                let _guard = guard;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let table = source.table(range);
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
                            export::write(&mut out, &table, &options, &cancel)
                                .and_then(|count| out.into_text().map(|text| (count, Some(text))))
                        };
                    result
                }))
                .unwrap_or_else(|_| Err(std::io::Error::other("The export writer panicked.")));
                if let Err(error) = &result
                    && let Some(download) = &download
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
    ) -> std::io::Result<Option<FileSource>> {
        if !self.scope.file_only() {
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
                .map(|download| Some(FileSource::Pending(download)));
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
            return Ok(Some(FileSource::Ready(spool)));
        }
        if self.result.cursor == crate::worker::Cursor::Complete || !self.incomplete {
            return Ok(None);
        }
        let result = self.result.clone();
        let source = self.source.clone();
        let cancel = self.cancel.clone();
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
                    .drain(execution, source, jobs, cancel)?;
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
        Ok(Some(FileSource::Ready(spool)))
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
        if self.download.is_some() && self.scope.executes() {
            self.selection = None;
            self.selected = false;
            if let Some(spool) = &self.retained
                && let export::spool::Status::Complete { rows } = spool.status()
            {
                self.scopes = vec![Scope::Downloaded, Scope::Replay, Scope::RunAgain];
                self.choices[0] = vec![
                    format!("Downloaded rows ({})", self.source.row_count()),
                    format!(
                        "Export again as… ({rows} rows, {:.1} MiB)",
                        spool.bytes() as f64 / (1024. * 1024.)
                    ),
                    "Run again and export (all rows)".into(),
                ];
                self.scope = Scope::Replay;
                self.incomplete = rows != self.source.row_count() as u64;
            } else {
                self.scopes = vec![Scope::RunAgain];
                self.choices[0] = vec!["Run again and export (all rows)".into()];
                self.scope = Scope::RunAgain;
            }
            self.controls[0].update(cx, |control, cx| {
                control.set_items(SearchableVec::new(self.choices[0].clone()), window, cx)
            });
            self.sync(window, cx);
        }
        if self.download.is_some()
            && let Some(intent) = &mut self.run
        {
            let _ = self.owner.update(cx, |owner, _| {
                if let Some(tab) = owner
                    .tabs
                    .iter()
                    .find(|tab| tab.saved.id == self.result.tab)
                    && tab.current_execution == self.result.execution
                    && tab.result_session.is_some()
                {
                    intent.expected = tab.result_session;
                }
            });
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
            self.remove_background(cx);
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
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    window.push_notification(
                        Notification::new()
                            .message(format!("Exported {count} rows to {filename}"))
                            .action(move |_, _, _| {
                                let path = path.clone();
                                Button::new("reveal-export")
                                    .label("Reveal in Finder")
                                    .on_click(move |_, _, cx| cx.reveal_path(&path))
                            }),
                        cx,
                    );
                }
                self.remove_background(cx);
            }
            Err(error) => {
                if error
                    .get_ref()
                    .is_some_and(|cause| cause.is::<crate::worker::SessionChanged>())
                {
                    self.refresh_session_review(cx);
                }
                self.error = Some(error.to_string());
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

fn option_indices(selected: bool, settings: &export::Settings) -> [usize; 10] {
    let options = &settings.csv;
    [
        usize::from(selected),
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
        Format::ALL
            .iter()
            .position(|value| *value == settings.format)
            .unwrap(),
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
        let invalid = self.settings.validate().is_err();
        let format = self.settings.format;
        let labels = [
            "Rows",
            "Preset",
            "Separator",
            "Line Ending",
            "Null Values",
            "Format",
            "Style",
            "Compression",
            "Column Types",
            "Numeric As",
        ];
        let ids = [
            "export-rows",
            "export-preset",
            "export-separator",
            "export-line-ending",
            "export-null",
            "export-format",
            "export-markdown-style",
            "export-parquet-compression",
            "export-parquet-types",
            "export-parquet-numeric",
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
        let one_column_null = format == Format::Csv
            && self.source.table(self.range()).column_indices.count() == 1
            && options.null.as_str().is_empty();
        let custom_null = format == Format::Csv && matches!(options.null, NullMarker::Custom(_));
        let preview = if self.preview_pending {
            "Preparing preview…".to_owned()
        } else {
            self.preview.clone()
        };
        v_flex().id("export-form").gap_4().w_full()
            .child(v_form().columns(2).children([5,0,1,2,3,4,6,7,8,9].into_iter()
                .filter(|index| matches!(index, 0|5) || format == Format::Csv && (1..=4).contains(index) || format == Format::Markdown && *index == 6 || format == Format::Parquet && (7..=9).contains(index))
                .map(|index| field().label(labels[index]).child(
                    Select::new(&self.controls[index]).id(ids[index]).focus_ring(false)
                        .disabled(busy || index == 9 && self.settings.parquet.column_types == export::parquet::ColumnTypes::Text).w_full().accessibility_label(labels[index])))))
            .when(custom_null, |view| view.child(field().label("Null Marker")
                .description("Exclude separators, quotes, and line breaks.")
                .child(Input::new(&self.null_text).id("export-null-text").focus_ring(false).disabled(busy).aria_label("Null Marker"))))
            .when(format == Format::Csv, |view| view.child(v_flex().gap_2().children(
                flags.into_iter().enumerate().map(|(index,(id,label,checked))|
                    Checkbox::new(id).label(label).checked(checked).disabled(busy)
                        .on_click(cx.listener(move |this,checked,window,cx| {
                            match index {0 => this.settings.csv.header = *checked,
                                1 => this.settings.csv.byte_order_mark = *checked,
                                2 => this.settings.csv.quote_all = *checked,
                                3 => this.settings.csv.escape_formulas = *checked, _ => unreachable!()}
                            this.sync(window,cx);
                        }))))))
            .when(format == Format::Markdown, |view| view
                .when(self.settings.markdown.style == export::markdown::Style::CodeBlock, |view| view.child(
                    field().label("Maximum Cell Width").description("Use 1 to 1000 display columns.")
                        .child(Input::new(&self.cell_width).id("export-cell-width").focus_ring(false).disabled(busy).aria_label("Maximum Cell Width"))))
                .child(Checkbox::new("export-row-numbers").label("Include row numbers").checked(self.settings.markdown.row_numbers).disabled(busy)
                    .on_click(cx.listener(|this,checked,window,cx| {this.settings.markdown.row_numbers = *checked; this.sync(window,cx);}))))
            .when(matches!(format, Format::Json | Format::JsonLines), |view| view.child(v_flex().gap_2().children(
                json_flags.into_iter().enumerate().map(|(index,(id,label,checked))|
                    Checkbox::new(id).label(label).checked(checked).disabled(busy || index == 1 && format == Format::JsonLines || index == 2 && !self.settings.json.typed)
                        .on_click(cx.listener(move |this,checked,window,cx| {
                            match index {0 => this.settings.json.typed = *checked, 1 => this.settings.json.pretty = *checked,
                                2 => this.settings.json.decimals_as_numbers = *checked, _ => unreachable!()}
                            this.sync(window,cx);
                        }))))))
            .when(format == Format::Parquet && self.settings.parquet.column_types == export::parquet::ColumnTypes::Typed && self.settings.parquet.numeric == export::parquet::Numeric::Double, |view| view.child(div().text_sm().text_color(cx.theme().warning).child("Double can change numeric values.")))
            .when(format == Format::Parquet && !self.source.table(self.range()).rows.context().iso_dates(), |view| view.child(div().id("export-date-style-warning").test_support().role(Role::Label)
                .aria_label("The result uses a non-ISO DateStyle. Dates and timestamps remain text.")
                .text_sm().text_color(cx.theme().muted_foreground).child("The result uses a non-ISO DateStyle. Dates and timestamps remain text.")))
            .when(format == Format::Parquet && self.scope.executes() && self.source.table(self.range()).rows.context().iso_dates()
                && self.run.as_ref().is_some_and(|intent| intent.profile.database_type == crate::model::DatabaseType::Postgres), |view| view.child(div().id("export-session-date-style").test_support().role(Role::Label)
                .aria_label("Non-ISO session dates and timestamps remain text.")
                .text_sm().text_color(cx.theme().muted_foreground).child("Non-ISO session dates and timestamps remain text.")))
            .when(self.incomplete && !self.scope.file_only(), |view| view.child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child("Only the downloaded rows are included. More rows may be available.")))
            .when(matches!(self.scope, Scope::All | Scope::Replay | Scope::RunAgain), |view| view.child(div().text_sm().text_color(cx.theme().muted_foreground).child("Preview shows the first downloaded rows.")))
            .when(self.scope.executes(), |view| view
                .when_some(self.run.as_ref(), |view, intent| view
                    .child(div().id("export-captured-connection").test_support().role(Role::Label).aria_label(intent.profile.name.clone()).text_sm().child(format!("Connection: {}", intent.profile.name)))
                    .child(div().id("export-captured-sql").test_support().role(Role::Label).aria_label(intent.sql.clone())
                        .child(div().h_16().overflow_y_scrollbar().p_2().bg(cx.theme().muted).font_family(cx.theme().mono_font_family.clone()).text_xs().child(intent.sql.clone())))
                    .when(intent.warning, |view| view.child(div().id("export-session-warning").test_support().role(Role::Alert)
                        .aria_label("The original session ended or changed. Its settings, temporary tables, and open transaction are gone. This export runs in the current session.")
                        .text_sm().text_color(cx.theme().warning).child("The original session ended or changed. Its settings, temporary tables, and open transaction are gone. This export runs in the current session.")))))
            .when(one_column_null, |view| view.child(div().text_sm().text_color(cx.theme().warning)
                .child("Empty null rows can be skipped by CSV readers. Choose a non-empty null marker.")))
            .when(self.large_copy, |view| view.child(div().id("export-copy-warning").test_support().role(Role::Alert)
                .aria_label("Markdown exceeds 40,000 characters. It can exceed message limits.")
                .text_sm().text_color(cx.theme().warning).child("Markdown exceeds 40,000 characters. It can exceed message limits.")))
            .child(v_flex().gap_2().child("Preview")
                .child(div().id("export-preview").test_support().role(Role::Label).aria_label(preview.clone())
                    .w_full().max_h_32().overflow_hidden().p_2().rounded(cx.theme().radius)
                    .bg(cx.theme().muted).font_family(cx.theme().mono_font_family.clone()).text_xs().child(preview)))
            .when_some(self.error.clone(), |view,error| view.child(div().id("export-error").test_support().role(Role::Alert)
                .aria_label(error.clone()).text_sm().text_color(cx.theme().danger).child(error)))
            .when(busy, |view| view.child(div().id("export-progress").test_support().role(Role::Status)
                .aria_label(self.status_text()).w_full().text_sm().text_color(cx.theme().muted_foreground).child(self.status_text())))
            .child(h_flex().gap_2().child(div().flex_1())
                .child(Button::new("export-cancel").label(if self.background && !busy { "Dismiss" } else if self.background { "Cancel export" } else { "Cancel" }).on_click(cx.listener(|this,_,window,cx| {
                    this.cancel.store(true,Ordering::Relaxed);
                    this.cancel_download();
                    this.remove_background(cx);
                    window.close_dialog(cx);
                })))
                .child(Button::new("export-copy").label(if self.large_copy {"Copy anyway"} else {"Copy"}).disabled(busy || invalid || format == Format::Parquet || self.scope.file_only())
                    .on_click(cx.listener(|this,_,window,cx| this.start(false,window,cx))))
                .child(Button::new("export-save").label(if self.error.is_some() && self.retained.as_ref().is_some_and(|spool| matches!(spool.status(), export::spool::Status::Complete { .. })) { "Retry…" } else if matches!(self.scope, Scope::Run) { "Run and export…" } else if matches!(self.scope, Scope::RunAgain) { "Run again and export…" } else { "Save…" }).primary().disabled(busy || invalid)
                    .on_click(cx.listener(|this,_,window,cx| this.start(true,window,cx)))))
    }
}

/// Context-menu copy shares the same writer and clipboard bound as the dialog.
pub(super) fn copy_format(
    source: Arc<Snapshot>,
    range: Option<RangeSelection>,
    options: export::Settings,
    window: &mut Window,
    cx: &mut App,
) {
    let request = CopyRequest::new(cx);
    let format = options.format;
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = cx.global::<export::Jobs>().register(cancel.clone());
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
            Err(error) => window.push_notification(error.to_string(), cx),
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
            window.push_notification("The clipboard changed. Choose Copy to try again.", cx);
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
mod tests {
    use super::{CopyRequest, copy_text};
    use gpui_kit::{ClipboardItem, Image, ImageFormat, TestAppContext};

    #[gpui_kit::test]
    fn writer_thread_failure_leaves_the_cursor_available_and_reports_the_error(
        cx: &mut TestAppContext,
    ) {
        use super::*;
        cx.update(crate::ui::init);
        let mut owner = None;
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
            let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
            owner = Some(qrow.downgrade());
            crate::ui::root(qrow, window, cx)
        });
        let owner = owner.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.csv");
        let dialog = cx
            .update_window(window.into(), |_, window, cx| {
                let (source, result) = owner
                    .update(cx, |owner, cx| {
                        let tab = &mut owner.tabs[0];
                        tab.cursor = crate::worker::Cursor::Available;
                        let data = tab.table.read(cx).delegate();
                        (
                            Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                            ExportResult {
                                tab: tab.saved.id,
                                execution: None,
                                cursor: crate::worker::Cursor::Available,
                                replay: None,
                            },
                        )
                    })
                    .unwrap();
                let dialog = cx.new(|cx| {
                    ExportDialog::new(
                        source,
                        None,
                        export::Settings::default(),
                        "thread-failure".into(),
                        true,
                        result,
                        owner.clone(),
                        window,
                        cx,
                    )
                });
                ExportDialog::open(&dialog, window, cx);
                dialog.update(cx, |dialog, cx| {
                    dialog.scope = Scope::All;
                    dialog.start_with_writer(
                        true,
                        || Err(std::io::Error::other("No thread resources")),
                        window,
                        cx,
                    );
                });
                dialog
            })
            .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, _, cx| {
            let dialog = dialog.read(cx);
            assert!(!dialog.running);
            assert_eq!(dialog.error.as_deref(), Some("No thread resources"));
            assert!(dialog.download.is_none());
            assert!(dialog.retained.is_none());
            let owner = owner.upgrade().unwrap();
            let owner = owner.read(cx);
            assert!(!owner.tabs[0].busy);
            assert_eq!(owner.tabs[0].cursor, crate::worker::Cursor::Available);
            assert!(owner.tabs[0].download.is_none());
            assert!(owner.exports.is_empty());
            assert_eq!(cx.global::<export::Jobs>().active_count(), 0);
        })
        .unwrap();
        assert!(!path.exists());
    }

    #[gpui_kit::test]
    fn background_export_survives_tab_close_and_keeps_failure_until_dismissed(
        cx: &mut TestAppContext,
    ) {
        use super::*;
        use gpui_kit::test::TestWindowExt;
        cx.update(crate::ui::init);
        let mut owner = None;
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
            let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
            owner = Some(qrow.downgrade());
            crate::ui::root(qrow, window, cx)
        });
        let owner = owner.unwrap();
        let mut retained = None;
        cx.update_window(window.into(), |_, window, cx| {
            let (source, mut result) = owner
                .update(cx, |owner, cx| {
                    let tab = &owner.tabs[0];
                    let data = tab.table.read(cx).delegate();
                    (
                        Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                        ExportResult {
                            tab: tab.saved.id,
                            execution: None,
                            cursor: crate::worker::Cursor::Complete,
                            replay: None,
                        },
                    )
                })
                .unwrap();
            let old_tab = result.tab;
            let columns = [crate::model::Column {
                name: "n".into(),
                data_type: "INT".into(),
            }];
            let (spool, producer) =
                export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            let mut producer = producer;
            producer
                .append(
                    &[
                        vec![Some("1".into())],
                        vec![Some("2".into())],
                        vec![Some("3".into())],
                    ],
                    &AtomicBool::new(false),
                )
                .unwrap();
            producer.finish(&AtomicBool::new(false)).unwrap();
            retained = Some(Arc::downgrade(&spool));
            result.replay = Some(spool.clone());
            let dialog = cx.new(|cx| {
                ExportDialog::new(
                    source,
                    None,
                    export::Settings::default(),
                    "background".into(),
                    false,
                    result,
                    owner.clone(),
                    window,
                    cx,
                )
            });
            ExportDialog::open(&dialog, window, cx);
            dialog.update(cx, |dialog, cx| {
                dialog.running = true;
                dialog.scope = Scope::Replay;
                dialog.result.replay = Some(spool);
                let jobs = cx.global::<export::Jobs>().clone();
                assert_eq!(
                    dialog
                        .all_source(&jobs, cx)
                        .unwrap()
                        .unwrap()
                        .spool()
                        .unwrap()
                        .row_count(),
                    3
                );
                dialog.start_background(window, cx);
            });
            assert!(Root::read(window, cx).dialog_focus_handle().is_none());
            assert!(!dialog.read(cx).cancel.load(Ordering::Relaxed));
            let id = dialog.read(cx).id;
            window.render_frame(cx);
            assert!(
                window
                    .within(SharedString::from(format!("export-job-{id}")))
                    .find("status")
                    .label()
                    .unwrap()
                    .contains("Writing 3 rows")
            );
            owner
                .update(cx, |owner, cx| {
                    assert_eq!(owner.exports.len(), 1);
                    owner.close_tab(0, window, cx);
                    assert!(owner.tabs.iter().all(|tab| tab.saved.id != old_tab));
                    assert_eq!(owner.exports.len(), 1);
                })
                .unwrap();
            ExportDialog::open(&dialog, window, cx);
            window.close_dialog(cx);
            assert!(!dialog.read(cx).cancel.load(Ordering::Relaxed));
            dialog.update(cx, |dialog, cx| {
                dialog.complete(
                    Err(std::io::Error::other("Output volume is full")),
                    None,
                    None,
                    window,
                    cx,
                )
            });
            let id = dialog.read(cx).id;
            window.render_frame(cx);
            assert_eq!(
                window
                    .find(SharedString::from(format!("export-details-{id}")))
                    .label(),
                Some("Retry…")
            );
            assert_eq!(
                dialog.read(cx).error.as_deref(),
                Some("Output volume is full")
            );
            dialog.update(cx, |dialog, cx| {
                dialog.settings.format = Format::JsonLines;
                dialog.sync(window, cx);
            });
            window.render_frame(cx);
            assert!(
                window
                    .within(SharedString::from(format!("export-job-{id}")))
                    .find("status")
                    .label()
                    .unwrap()
                    .contains("Ready to retry")
            );
            assert!(retained.as_ref().unwrap().upgrade().is_some());
            window.click(SharedString::from(format!("export-stop-{id}")), cx);
            assert!(owner.upgrade().unwrap().read(cx).exports.is_empty());
        })
        .unwrap();
        cx.run_until_parked();
        assert!(retained.unwrap().upgrade().is_none());
    }

    #[gpui_kit::test]
    fn writer_failure_keeps_retry_and_disconnect_preserves_a_complete_preview(
        cx: &mut TestAppContext,
    ) {
        use super::*;
        use gpui_kit::test::TestWindowExt;
        cx.update(crate::ui::init);
        let mut owner = None;
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
            let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
            owner = Some(qrow.downgrade());
            crate::ui::root(qrow, window, cx)
        });
        let owner = owner.unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let (source, result, incomplete) = owner
                .update(cx, |owner, cx| {
                    Qrow::apply_worker_event(
                        &mut owner.tabs[0],
                        Event::Disconnected,
                        true,
                        &[],
                        2048 * 1024 * 1024,
                        cx,
                    );
                    let tab = &owner.tabs[0];
                    assert!(tab.preview_complete);
                    let data = tab.table.read(cx).delegate();
                    (
                        Arc::new(Snapshot::new(&data.columns, &data.rows).unwrap()),
                        ExportResult {
                            tab: tab.saved.id,
                            execution: tab.current_execution,
                            cursor: tab.cursor,
                            replay: None,
                        },
                        !tab.preview_complete,
                    )
                })
                .unwrap();
            let dialog = cx.new(|cx| {
                ExportDialog::new(
                    source,
                    None,
                    export::Settings::default(),
                    "test".into(),
                    incomplete,
                    result,
                    owner.clone(),
                    window,
                    cx,
                )
            });
            let jobs = cx.global::<export::Jobs>().clone();
            dialog.update(cx, |dialog, cx| {
                dialog.scope = Scope::All;
                assert!(dialog.all_source(&jobs, cx).unwrap().is_none());
            });
            let columns = [crate::model::Column {
                name: "n".into(),
                data_type: "INT".into(),
            }];
            let (spool, producer) =
                export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            producer.finish(&AtomicBool::new(false)).unwrap();
            ExportDialog::open(&dialog, window, cx);
            dialog.update(cx, |dialog, cx| {
                dialog.retained = Some(spool);
                // Failure propagation stops the download with this flag. It is
                // distinct from the user dismissing a cancelled operation.
                dialog.cancel.store(true, Ordering::Relaxed);
                dialog.complete(
                    Err(std::io::Error::other("Output volume is full")),
                    None,
                    None,
                    window,
                    cx,
                );
                assert_eq!(dialog.error.as_deref(), Some("Output volume is full"));
                assert!(!dialog.running);
            });
            window.render_frame(cx);
            assert_eq!(
                window.try_find("export-save").unwrap().label(),
                Some("Retry…")
            );
            let (failed, _producer) =
                export::spool::Spool::new(&columns, &export::Context::default()).unwrap();
            failed.fail("Spool volume is full");
            dialog.update(cx, |dialog, cx| {
                dialog.retained = Some(failed);
                dialog.complete(
                    Err(std::io::Error::other(export::Cancelled)),
                    None,
                    None,
                    window,
                    cx,
                );
                assert_eq!(dialog.error.as_deref(), Some("Spool volume is full"));
            });
            window.close_dialog(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn completed_export_preserves_an_unanswered_quit_warning(cx: &mut TestAppContext) {
        use super::*;
        use gpui_kit::test::TestWindowExt;
        use std::sync::mpsc;

        cx.update(crate::ui::init);
        let mut owner = None;
        let window = cx.open_window(size(px(1280.), px(820.)), |window, cx| {
            let qrow = cx.new(|cx| Qrow::new(Environment::demo(), Instant::now(), window, cx));
            owner = Some(qrow.downgrade());
            crate::ui::root(qrow, window, cx)
        });
        let owner = owner.unwrap();
        let source = Arc::new(
            Snapshot::new(
                &[crate::model::Column {
                    name: "value".into(),
                    data_type: "STRING".into(),
                }],
                &vec![vec![Some("complete".into())]].into(),
            )
            .unwrap(),
        );
        let (dialog, cancel, jobs, original_focus) = cx
            .update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                let original_focus = window.focused(cx);
                let dialog = cx.new(|cx| {
                    ExportDialog::new(
                        source.clone(),
                        None,
                        export::Settings::default(),
                        "test".into(),
                        false,
                        ExportResult {
                            tab: Uuid::nil(),
                            execution: None,
                            cursor: crate::worker::Cursor::Complete,
                            replay: None,
                        },
                        owner.clone(),
                        window,
                        cx,
                    )
                });
                ExportDialog::open(&dialog, window, cx);
                dialog.update(cx, |dialog, _| dialog.running = true);
                (
                    dialog.clone(),
                    dialog.read(cx).cancel.clone(),
                    cx.global::<export::Jobs>().clone(),
                    original_focus,
                )
            })
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("result.csv");
        let writer_path = path.clone();
        let guard = jobs.register(cancel.clone());
        let (started, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = guard;
            export::save(&writer_path, &cancel, |out| {
                let count =
                    export::write_csv(out, &source.table(None), &CsvOptions::default(), &cancel)?;
                started.send(()).unwrap();
                released.recv().unwrap();
                Ok(count)
            })
            .map(|count| (count, None))
        });
        ready.recv().unwrap();
        assert_eq!(jobs.active_count(), 1);
        cx.update_window(window.into(), |_, window, cx| {
            owner
                .update(cx, |qrow, cx| qrow.request_quit(window, cx))
                .unwrap();
            window.render_frame(cx);
            assert!(window.try_find("keep-working").is_some());
        })
        .unwrap();
        release.send(()).unwrap();
        let result = worker.join().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "value\r\ncomplete\r\n"
        );
        assert_eq!(jobs.active_count(), 0);
        cx.update_window(window.into(), |_, window, cx| {
            let warning = Root::read(window, cx)
                .dialog_focus_handle()
                .cloned()
                .unwrap();
            dialog.update(cx, |dialog, cx| {
                dialog.complete(result, Some(path), None, window, cx)
            });
            window.render_frame(cx);
            assert_eq!(Root::read(window, cx).dialog_focus_handle(), Some(&warning));
            assert!(window.try_find("keep-working").is_some());
            assert!(window.try_find("export-save").is_none());
            assert!(owner.upgrade().unwrap().read(cx).quit_warning_open);
            window.click("keep-working", cx);
            window.render_frame(cx);
            assert!(!owner.upgrade().unwrap().read(cx).quit_warning_open);
            assert!(Root::read(window, cx).dialog_focus_handle().is_none());
            assert_eq!(window.focused(cx), original_focus);
            // A new active export can still prompt after declining the first quit.
            let guard = jobs.register(Arc::new(AtomicBool::new(false)));
            owner
                .update(cx, |qrow, cx| qrow.request_quit(window, cx))
                .unwrap();
            window.render_frame(cx);
            assert!(window.try_find("keep-working").is_some());
            window.click("keep-working", cx);
            drop(guard);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn later_copy_requests_and_external_clipboard_changes_reject_old_results(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            copy_text("before".into(), cx);
            let first = CopyRequest::new(cx);
            let second = CopyRequest::new(cx);
            assert!(!first.is_current(cx));
            assert!(second.is_current(cx));
            copy_text("cell".into(), cx);
            assert!(!second.is_current(cx));
            let external = CopyRequest::new(cx);
            cx.write_to_clipboard(ClipboardItem::new_string("another app".into()));
            assert!(!external.is_current(cx));
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
            let empty = CopyRequest::new(cx);
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                vec![1, 2, 3],
            )));
            assert!(!empty.is_current(cx));
            let image = CopyRequest::new(cx);
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                vec![4, 5, 6],
            )));
            assert!(!image.is_current(cx));
        });
    }
}

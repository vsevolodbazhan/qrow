use super::*;
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
    select::{Select, SelectState},
    v_flex,
};
use std::{
    hash::{Hash, Hasher},
    ops::{Range, RangeInclusive},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
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
            replay: tab.replay.clone(),
        };
        let owner = cx.weak_entity();
        let view = cx.new(|cx| {
            ExportDialog::new(
                source, selection, settings, filename, incomplete, result, owner, window, cx,
            )
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
        let cancel = view.read(cx).cancel.clone();
        let dialog_view = view.clone();
        let view = view.clone();
        let close_view = view.downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let cancel = cancel.clone();
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
                    cancel.store(true, Ordering::Relaxed);
                    let _ = close_view.update(cx, |this, _| this.cancel_download());
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
        if self.running
            || !save
                && (self.settings.format == Format::Parquet
                    || matches!(self.scope, Scope::All | Scope::Replay))
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
            let all_source = match weak.update_in(cx, |this, _, cx| this.all_source(&jobs, cx)) {
                Ok(Ok(source)) => source,
                result => {
                    let _ = weak.update_in(cx, |this, _, cx| {
                        this.running = false;
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
            let guard = jobs.register(cancel.clone());
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _guard = guard;
                    let table = source.table(range);
                    let result: std::io::Result<(usize, Option<export::Text>)> =
                        if let Some(path) = output_path {
                            export::save(&path, &cancel, |out| {
                                if let Some(spool) = &all_source {
                                    export::stream::write(out, spool, &options, &cancel)
                                } else {
                                    export::write(out, &table, &options, &cancel)
                                }
                            })
                            .map(|count| (count, None))
                        } else {
                            let mut out = export::LimitedWriter::new(export::CLIPBOARD_BYTES);
                            export::write(&mut out, &table, &options, &cancel)
                                .and_then(|count| out.into_text().map(|text| (count, Some(text))))
                        };
                    if let Err(error) = &result
                        && let Some(download) = &download
                    {
                        download.fail(error.to_string());
                    }
                    result
                })
                .await;
            let _ = weak.update_in(cx, |this, window, cx| {
                this.complete(result, path, request, window, cx);
            });
        })
        .detach();
    }

    fn cancel_download(&self) {
        if let Some(download) = &self.download {
            let download = download.clone();
            std::thread::spawn(move || download.cancel());
        }
    }

    fn all_source(
        &mut self,
        jobs: &export::Jobs,
        cx: &mut Context<Self>,
    ) -> std::io::Result<Option<Arc<export::spool::Spool>>> {
        if !matches!(self.scope, Scope::All | Scope::Replay) {
            return Ok(None);
        }
        export::check_cancelled(&self.cancel)?;
        if let Some(spool) = self
            .retained
            .as_ref()
            .filter(|spool| matches!(spool.status(), export::spool::Status::Complete { .. }))
            .or(self.result.replay.as_ref())
        {
            return Ok(Some(spool.clone()));
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
        self.retained = Some(download.spool().clone());
        self.download = Some(download.clone());
        Ok(Some(download.spool().clone()))
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
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }
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
            .when(self.incomplete && !matches!(self.scope, Scope::All | Scope::Replay), |view| view.child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child("Only the downloaded rows are included. More rows may be available.")))
            .when(matches!(self.scope, Scope::All | Scope::Replay), |view| view.child(div().text_sm().text_color(cx.theme().muted_foreground).child("Preview shows the first downloaded rows.")))
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
            .child(h_flex().gap_2().child(div().flex_1())
                .when(busy, |row| row.child(div().text_sm().child("Exporting…")))
                .child(Button::new("export-cancel").label("Cancel").on_click(cx.listener(|this,_,window,cx| {
                    this.cancel.store(true,Ordering::Relaxed); window.close_dialog(cx);
                })))
                .child(Button::new("export-copy").label(if self.large_copy {"Copy anyway"} else {"Copy"}).disabled(busy || invalid || format == Format::Parquet || matches!(self.scope, Scope::All | Scope::Replay))
                    .on_click(cx.listener(|this,_,window,cx| this.start(false,window,cx))))
                .child(Button::new("export-save").label(if self.error.is_some() && self.retained.as_ref().is_some_and(|spool| matches!(spool.status(), export::spool::Status::Complete { .. })) { "Retry…" } else { "Save…" }).primary().disabled(busy || invalid)
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

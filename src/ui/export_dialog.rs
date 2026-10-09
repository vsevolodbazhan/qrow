use super::*;
use crate::export::{
    self, Snapshot,
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

impl Global for export::Jobs {}

struct ExportDialog {
    source: Arc<Snapshot>,
    selection: Option<RangeSelection>,
    selected: bool,
    settings: export::Settings,
    filename: String,
    incomplete: bool,
    controls: Vec<Choice>,
    choices: Vec<Vec<String>>,
    null_text: Entity<InputState>,
    preview: String,
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
        let incomplete = tab.more || tab.status != "Complete";
        let owner = cx.weak_entity();
        let view = cx.new(|cx| {
            ExportDialog::new(
                source, selection, settings, filename, incomplete, owner, window, cx,
            )
        });
        ExportDialog::open(&view, window, cx);
    }

    fn remember_export(&mut self, settings: export::Settings, cx: &mut Context<Self>) {
        self.settings.export = settings;
        for tab in &self.tabs {
            tab.table.update(cx, |table, _| {
                table.delegate_mut().csv = self.settings.export.csv.clone();
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
        window.open_dialog(cx, move |dialog, window, _| {
            let cancel = cancel.clone();
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
                .on_close(move |_, _, _| {
                    cancel.store(true, Ordering::Relaxed);
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
        owner: WeakEntity<Qrow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut scopes = vec![format!("Downloaded rows ({})", source.row_count())];
        if let Some((rows, columns)) = &selection {
            scopes.push(format!(
                "Selection ({} rows, {} columns)",
                rows.len(),
                columns.clone().count()
            ));
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
        ];
        let selected = selection.is_some();
        let options = settings.csv.clone();
        let indices = [
            usize::from(selected),
            preset_index(&options),
            Separator::ALL
                .iter()
                .position(|s| *s == options.separator)
                .unwrap(),
            LineEnding::ALL
                .iter()
                .position(|s| *s == options.line_ending)
                .unwrap(),
            NullMarker::ALL
                .iter()
                .position(|s| *s == options.null)
                .unwrap_or(3),
        ];
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
                        0 => this.selected = choice == 1,
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
        let mut this = Self {
            source,
            selection,
            selected,
            settings,
            filename,
            incomplete,
            controls,
            choices,
            null_text,
            preview: String::new(),
            error: None,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            owner,
            dialog: None,
            _subscriptions: subscriptions,
        };
        this.update_preview();
        this
    }

    fn range(&self) -> Option<RangeSelection> {
        if self.selected {
            self.selection.clone()
        } else {
            None
        }
    }

    fn update_preview(&mut self) {
        if let Err(error) = self.settings.csv.validate() {
            self.error = Some(error.to_string());
            self.preview.clear();
            return;
        }
        let mut table = self.source.table(self.range());
        table.row_indices.end = table.row_indices.end.min(table.row_indices.start + 5);
        let mut out = export::LimitedWriter::new(8192);
        let truncated = export::write_csv(
            &mut out,
            &table,
            &self.settings.csv,
            &AtomicBool::new(false),
        )
        .is_err();
        self.preview = String::from_utf8_lossy(&out.into_bytes())
            .trim_start_matches('\u{feff}')
            .to_owned();
        if truncated {
            self.preview.push_str("\n…");
        }
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let options = self.settings.csv.clone();
        let indices = [
            usize::from(self.selected),
            preset_index(&options),
            Separator::ALL
                .iter()
                .position(|s| *s == options.separator)
                .unwrap(),
            LineEnding::ALL
                .iter()
                .position(|s| *s == options.line_ending)
                .unwrap(),
            NullMarker::ALL
                .iter()
                .position(|s| *s == options.null)
                .unwrap_or(3),
        ];
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
        self.update_preview();
        cx.notify();
    }

    fn start(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        if let Err(error) = self.settings.csv.validate() {
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
        let options = self.settings.csv.clone();
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
            let output_path = path.clone();
            let guard = jobs.register(cancel.clone());
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _guard = guard;
                    let table = source.table(range);
                    if let Some(path) = output_path {
                        export::save(&path, &cancel, |out| {
                            export::write_csv(out, &table, &options, &cancel)
                        })
                        .map(|count| (count, None))
                    } else {
                        let mut out = export::LimitedWriter::new(export::CLIPBOARD_BYTES);
                        let count = export::write_csv(&mut out, &table, &options, &cancel)?;
                        Ok((
                            count,
                            Some(String::from_utf8(out.into_bytes()).expect("CSV is UTF-8")),
                        ))
                    }
                })
                .await;
            let _ = weak.update_in(cx, |this, window, cx| {
                this.complete(result, path, request, window, cx);
            });
        })
        .detach();
    }
    fn complete(
        &mut self,
        result: std::io::Result<(usize, Option<String>)>,
        path: Option<PathBuf>,
        request: Option<CopyRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        if self.cancel.load(Ordering::Relaxed) {
            cx.notify();
            return;
        }
        match result {
            Ok((count, text)) => {
                if let Some(text) = text {
                    if !request
                        .as_ref()
                        .is_some_and(|request| request.is_current(cx))
                    {
                        self.error =
                            Some("The clipboard changed. Choose Copy to try again.".into());
                        cx.notify();
                        return;
                    }
                    copy_text(text, cx);
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
        let invalid = self.settings.csv.validate().is_err();
        let labels = ["Rows", "Preset", "Separator", "Line Ending", "Null Values"];
        let ids = [
            "export-rows",
            "export-preset",
            "export-separator",
            "export-line-ending",
            "export-null",
        ];
        let options = self.settings.csv.clone();
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
        let table = self.source.table(self.range());
        let one_column_null = table.column_indices.count() == 1 && options.null.as_str().is_empty();
        v_flex().id("export-form").gap_4().w_full()
            .child(v_form().columns(2).children(self.controls.iter().enumerate().map(|(index, control)| {
                field().label(labels[index]).child(Select::new(control).id(ids[index]).focus_ring(false)
                    .disabled(busy || index == 0 && self.selection.is_none()).w_full().accessibility_label(labels[index]))
            })))
            .when(matches!(options.null, NullMarker::Custom(_)), |view| view.child(
                field().label("Null Marker").description("Exclude separators, quotes, and line breaks.")
                    .child(Input::new(&self.null_text).id("export-null-text").focus_ring(false)
                        .disabled(busy).aria_label("Null Marker"))))
            .child(v_flex().gap_2().children(flags.into_iter().enumerate().map(|(index, (id, label, checked))| {
                Checkbox::new(id).label(label).checked(checked).disabled(busy)
                    .on_click(cx.listener(move |this, checked, window, cx| {
                        match index { 0 => this.settings.csv.header = *checked,
                            1 => this.settings.csv.byte_order_mark = *checked,
                            2 => this.settings.csv.quote_all = *checked,
                            3 => this.settings.csv.escape_formulas = *checked,
                            _ => unreachable!() }
                        this.sync(window, cx);
                    }))
            })))
            .when(self.incomplete, |view| view.child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child("Only the downloaded rows are included. More rows may be available.")))
            .when(one_column_null, |view| view.child(div().text_sm().text_color(cx.theme().warning)
                .child("Empty null rows can be skipped by CSV readers. Choose a non-empty null marker.")))
            .child(v_flex().gap_2().child("Preview")
                .child(div().id("export-preview").test_support().role(Role::Label).aria_label(self.preview.clone())
                    .w_full().max_h_32().overflow_hidden().p_2().rounded(cx.theme().radius)
                    .bg(cx.theme().muted).font_family(cx.theme().mono_font_family.clone()).text_xs()
                    .child(self.preview.clone())))
            .when_some(self.error.clone(), |view, error| view.child(div().id("export-error").test_support()
                .role(Role::Alert).aria_label(error.clone()).text_sm().text_color(cx.theme().danger).child(error)))
            .child(h_flex().gap_2().child(div().flex_1())
                .when(busy, |row| row.child(div().text_sm().child("Exporting…")))
                .child(Button::new("export-cancel").label("Cancel").on_click(cx.listener(|this, _, window, cx| {
                    this.cancel.store(true, Ordering::Relaxed);
                    window.close_dialog(cx);
                })))
                .child(Button::new("export-copy").label("Copy").disabled(busy || invalid)
                    .on_click(cx.listener(|this, _, window, cx| this.start(false, window, cx))))
                .child(Button::new("export-save").label("Save…").primary().disabled(busy || invalid)
                    .on_click(cx.listener(|this, _, window, cx| this.start(true, window, cx)))))
    }
}

/// Context-menu copy shares the same writer and clipboard bound as the dialog.
pub(super) fn copy_csv(
    source: Arc<Snapshot>,
    range: Option<RangeSelection>,
    options: CsvOptions,
    window: &mut Window,
    cx: &mut App,
) {
    let request = CopyRequest::new(cx);
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = cx.global::<export::Jobs>().register(cancel.clone());
    let task = cx.background_executor().spawn(async move {
        let _guard = guard;
        let mut out = export::LimitedWriter::new(export::CLIPBOARD_BYTES);
        export::write_csv(&mut out, &source.table(range), &options, &cancel)?;
        Ok::<_, std::io::Error>(String::from_utf8(out.into_bytes()).expect("CSV is UTF-8"))
    });
    let window = window.window_handle();
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update_window(window, |_, window, cx| match result {
            Ok(text) if request.is_current(cx) => copy_text(text, cx),
            Ok(_) => {}
            Err(error) => window.push_notification(error.to_string(), cx),
        });
    })
    .detach();
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

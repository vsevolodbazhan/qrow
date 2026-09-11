use eframe::egui::{self, Color32, FontId, RichText, TextFormat};
use egui_extras::{Column as TableColumn, TableBuilder};
use qrow::{
    model::{Column, Profile, Row, SavedTab, Workspace},
    sql::{self, Kind},
    storage::{self, Saver},
    worker::{Event, Worker},
};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const BG: Color32 = Color32::from_rgb(21, 24, 29);
const PANEL: Color32 = Color32::from_rgb(27, 31, 37);
const BORDER: Color32 = Color32::from_rgb(47, 54, 63);
const MUTED: Color32 = Color32::from_rgb(143, 154, 169);
const TEXT: Color32 = Color32::from_rgb(221, 228, 236);
const ACCENT: Color32 = Color32::from_rgb(113, 219, 180);
const ERROR: Color32 = Color32::from_rgb(248, 140, 143);

struct Tab {
    saved: SavedTab,
    worker: Option<Worker>,
    busy: bool,
    cancelling: bool,
    connected: bool,
    more: bool,
    columns: Vec<Column>,
    rows: Vec<Row>,
    status: String,
    error: Option<String>,
    selection: Option<Range<usize>>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
    selected_cell: Option<(usize, usize)>,
    highlight_source: String,
    highlight: egui::text::LayoutJob,
}

impl Tab {
    fn new(saved: SavedTab) -> Self {
        Self {
            saved,
            worker: None,
            busy: false,
            cancelling: false,
            connected: false,
            more: false,
            columns: vec![],
            rows: vec![],
            status: "Not connected".into(),
            error: None,
            selection: None,
            started: None,
            elapsed: None,
            selected_cell: None,
            highlight_source: String::new(),
            highlight: Default::default(),
        }
    }
    fn reset_results(&mut self) {
        self.columns.clear();
        self.rows.clear();
        self.more = false;
        self.error = None;
        self.selected_cell = None;
        self.elapsed = None;
    }
    fn switch_profile(&mut self, profile: Option<Uuid>) {
        if self.saved.profile == profile {
            return;
        }
        self.saved.profile = profile;
        if let Some(worker) = &self.worker {
            worker.disconnect();
        }
        self.connected = false;
        self.reset_results();
        self.status = "Not connected".into();
    }
    fn events(&mut self) {
        let events: Vec<_> = self
            .worker
            .as_ref()
            .map(|w| w.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Connecting => self.status = "Connecting to Kyuubi…".into(),
                Event::Connected => self.connected = true,
                Event::Running => self.status = "Executing…".into(),
                Event::Columns(columns) => {
                    self.columns = columns;
                    self.status = "Fetching preview…".into();
                }
                Event::Rows(rows) => self.rows.extend(rows),
                Event::Ready { more, limited } => {
                    self.more = more;
                    self.busy = false;
                    self.cancelling = false;
                    self.elapsed = self.started.take().map(|t| t.elapsed());
                    self.status = if limited {
                        "Preview limit reached · 64 MiB or 100,000 rows"
                    } else if more {
                        "Preview · more rows may be available"
                    } else {
                        "Complete"
                    }
                    .into();
                }
                Event::Cancelled => {
                    self.busy = false;
                    self.cancelling = false;
                    self.more = false;
                    self.elapsed = self.started.take().map(|t| t.elapsed());
                    self.status = if self.columns.is_empty() {
                        "Cancelled"
                    } else {
                        "Fetching stopped · query may have completed"
                    }
                    .into();
                }
                Event::Error {
                    message,
                    disconnected,
                } => {
                    self.busy = false;
                    self.cancelling = false;
                    self.more = false;
                    if disconnected {
                        self.connected = false;
                    }
                    self.elapsed = self.started.take().map(|t| t.elapsed());
                    self.status = if disconnected {
                        "Disconnected · run again to reconnect"
                    } else {
                        "Query failed"
                    }
                    .into();
                    self.error = Some(message);
                }
                Event::CancelError(message) => {
                    self.error = Some(message);
                    self.cancelling = false;
                }
                Event::Disconnected => {
                    self.connected = false;
                    if !self.busy {
                        self.status = "Not connected".into();
                    }
                }
            }
        }
    }
}

struct ProfileEditor {
    profile: Profile,
    parameters: Vec<(String, String)>,
    password: Zeroizing<String>,
    is_new: bool,
    error: Option<String>,
    saving: Option<mpsc::Receiver<Result<Profile, String>>>,
}

impl ProfileEditor {
    fn new(profile: Profile, is_new: bool) -> Self {
        let parameters = profile
            .parameters
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self {
            profile,
            parameters,
            password: Zeroizing::new(String::new()),
            is_new,
            error: None,
            saving: None,
        }
    }
}

pub struct Qrow {
    profiles: Vec<Profile>,
    tabs: Vec<Tab>,
    active: usize,
    editor: Option<ProfileEditor>,
    saver: Option<Saver>,
    dirty: Option<Instant>,
    message: Option<String>,
    demo: bool,
    started: Instant,
    first_frame: bool,
}

impl Qrow {
    pub fn new(cc: &eframe::CreationContext<'_>, demo: bool, started: Instant) -> Self {
        let ctx = &cc.egui_ctx;
        let mut style = (*ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = PANEL;
        style.visuals.extreme_bg_color = Color32::from_rgb(17, 20, 24);
        style.visuals.faint_bg_color = Color32::from_rgb(26, 30, 36);
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = Color32::from_rgb(43, 80, 73);
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(36, 42, 50);
        style.spacing.item_spacing = egui::vec2(10.0, 9.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(14.0));
        ctx.set_style(style);
        let (workspace, message, can_save) = if demo {
            (demo_workspace(), None, false)
        } else {
            match storage::load(&storage::workspace_path()) {
                Ok(w) => (w, None, true),
                Err(e) => (
                    Workspace::default(),
                    Some(format!(
                        "{e:#}. Automatic saving is disabled for this session."
                    )),
                    false,
                ),
            }
        };
        let mut app = Self {
            profiles: workspace.profiles,
            tabs: workspace.tabs.into_iter().map(Tab::new).collect(),
            active: workspace.active_tab,
            editor: None,
            saver: can_save.then(|| Saver::new(storage::workspace_path())),
            dirty: None,
            message,
            demo,
            started,
            first_frame: true,
        };
        if demo {
            app.seed_demo();
        }
        app
    }

    fn snapshot(&self) -> Workspace {
        Workspace {
            version: 1,
            profiles: self.profiles.clone(),
            tabs: self.tabs.iter().map(|t| t.saved.clone()).collect(),
            active_tab: self.active,
        }
    }
    fn changed(&mut self) {
        self.dirty = Some(Instant::now());
    }
    fn new_tab(&mut self) {
        let profile = self
            .tabs
            .get(self.active)
            .and_then(|t| t.saved.profile)
            .or_else(|| self.profiles.first().map(|p| p.id));
        self.tabs
            .push(Tab::new(SavedTab::new(self.tabs.len() + 1, profile)));
        self.active = self.tabs.len() - 1;
        self.changed();
    }
    fn run(&mut self, ctx: &egui::Context) {
        if self.demo {
            self.seed_demo();
            return;
        }
        let tab = &mut self.tabs[self.active];
        if tab.busy {
            return;
        }
        let query = match &tab.selection {
            Some(range) if !range.is_empty() => tab
                .saved
                .sql
                .chars()
                .skip(range.start)
                .take(range.len())
                .collect::<String>(),
            _ => tab.saved.sql.clone(),
        };
        if let Err(error) = sql::validate_single(&query) {
            tab.error = Some(error.to_string());
            return;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|p| Some(p.id) == tab.saved.profile)
            .cloned()
        else {
            tab.error = Some("Choose a connection profile before running SQL.".into());
            return;
        };
        if tab.worker.is_none() {
            let ctx = ctx.clone();
            tab.worker = Some(Worker::new(Arc::new(move || ctx.request_repaint())));
        }
        tab.reset_results();
        tab.busy = true;
        tab.cancelling = false;
        tab.started = Some(Instant::now());
        tab.status = "Preparing query…".into();
        tab.worker.as_ref().unwrap().run(profile, query);
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("connections")
            .exact_width(228.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(18.0))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Q").size(30.0).strong().color(ACCENT));
                    ui.vertical(|ui| {
                        ui.label(RichText::new("qrow").size(23.0).strong());
                        ui.label(RichText::new("SQL WORKBENCH").size(9.0).color(MUTED));
                    });
                });
                ui.add_space(34.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("CONNECTIONS")
                            .size(10.0)
                            .color(MUTED)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("+")
                            .on_hover_text("New connection")
                            .clicked()
                            && !self.demo
                        {
                            self.editor = Some(ProfileEditor::new(Profile::default(), true));
                        }
                    });
                });
                ui.add_space(12.0);
                if self.profiles.is_empty() {
                    ui.label(RichText::new("Your query connections\nappear here.").color(MUTED));
                    ui.add_space(12.0);
                    if ui.button("+ Add connection").clicked() {
                        self.editor = Some(ProfileEditor::new(Profile::default(), true));
                    }
                }
                let selected = self.tabs[self.active].saved.profile;
                let mut switch = None;
                for profile in &self.profiles {
                    let active = selected == Some(profile.id);
                    let response = ui.add_sized(
                        [ui.available_width(), 38.0],
                        egui::Button::new(
                            RichText::new(format!("○  {}", profile.name)).color(if active {
                                ACCENT
                            } else {
                                TEXT
                            }),
                        )
                        .fill(if active {
                            Color32::from_rgb(35, 57, 53)
                        } else {
                            Color32::TRANSPARENT
                        }),
                    );
                    if response.clicked() && !self.tabs[self.active].busy {
                        switch = Some(profile.id);
                    }
                    response
                        .on_hover_text(format!(
                            "{}@{}:{}\nDatabase: {}\nRight-click to edit",
                            profile.username, profile.host, profile.port, profile.database
                        ))
                        .context_menu(|ui| {
                            if ui.button("Edit connection").clicked() && !self.demo {
                                self.editor = Some(ProfileEditor::new(profile.clone(), false));
                                ui.close_menu();
                            }
                            if ui.button("Duplicate connection").clicked() && !self.demo {
                                let mut duplicate = profile.clone();
                                duplicate.id = Uuid::new_v4();
                                duplicate.name.push_str(" copy");
                                self.editor = Some(ProfileEditor::new(duplicate, true));
                                ui.close_menu();
                            }
                        });
                }
                if let Some(id) = switch {
                    self.tabs[self.active].switch_profile(Some(id));
                    self.changed();
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        RichText::new(if self.demo {
                            "DEMO · SAMPLE DATA"
                        } else {
                            "SPARK / KYUUBI"
                        })
                        .size(10.0)
                        .color(ACCENT),
                    );
                    ui.label(
                        RichText::new("Personal SQL workspace")
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.separator();
                });
            });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = None;
        let mut activate = None;
        egui::ScrollArea::horizontal()
            .id_salt("tab_strip")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, tab) in self.tabs.iter().enumerate() {
                        ui.push_id(tab.saved.id, |ui| {
                            let fill = if index == self.active {
                                Color32::from_rgb(39, 47, 55)
                            } else {
                                PANEL
                            };
                            egui::Frame::new()
                                .fill(fill)
                                .corner_radius(5.0)
                                .inner_margin(egui::vec2(10.0, 5.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if tab.busy {
                                            ui.spinner();
                                        }
                                        if ui
                                            .selectable_label(
                                                index == self.active,
                                                &tab.saved.title,
                                            )
                                            .clicked()
                                        {
                                            activate = Some(index);
                                        }
                                        if ui
                                            .add_enabled(
                                                !tab.busy,
                                                egui::Button::new("×").frame(false),
                                            )
                                            .on_hover_text("Close tab")
                                            .clicked()
                                        {
                                            close = Some(index);
                                        }
                                    });
                                });
                        });
                    }
                    if ui.button("+").on_hover_text("New query tab · ⌘T").clicked() {
                        self.new_tab();
                    }
                });
            });
        if let Some(index) = activate {
            self.active = index;
            self.changed();
        }
        if let Some(index) = close {
            self.tabs.remove(index);
            if index < self.active {
                self.active -= 1;
            }
            if self.tabs.is_empty() {
                self.tabs.push(Tab::new(SavedTab::new(
                    1,
                    self.profiles.first().map(|p| p.id),
                )));
            }
            self.active = self.active.min(self.tabs.len() - 1);
            self.changed();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut run = false;
        let mut changed = false;
        let tab = &mut self.tabs[self.active];
        ui.horizontal(|ui| {
            let current = self
                .profiles
                .iter()
                .find(|p| Some(p.id) == tab.saved.profile)
                .map(|p| p.name.as_str())
                .unwrap_or("Choose connection");
            let mut selected = tab.saved.profile;
            ui.add_enabled_ui(!tab.busy, |ui| {
                egui::ComboBox::from_id_salt("profile_selector")
                    .selected_text(current)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for p in &self.profiles {
                            ui.selectable_value(&mut selected, Some(p.id), &p.name);
                        }
                    });
            });
            if selected != tab.saved.profile {
                tab.switch_profile(selected);
                changed = true;
            }
            if let Some(p) = self
                .profiles
                .iter()
                .find(|p| Some(p.id) == tab.saved.profile)
            {
                ui.label(RichText::new(&p.database).monospace().color(MUTED));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if tab.busy {
                    if ui
                        .add_enabled(
                            !tab.cancelling,
                            egui::Button::new(if tab.cancelling {
                                "Cancelling…"
                            } else {
                                "■  Cancel"
                            }),
                        )
                        .clicked()
                    {
                        tab.cancelling = true;
                        tab.status = "Cancelling…".into();
                        if let Some(worker) = &tab.worker {
                            worker.cancel();
                        }
                    }
                } else {
                    run = ui
                        .add(
                            egui::Button::new(RichText::new("▶  Run query").color(BG).strong())
                                .fill(ACCENT),
                        )
                        .on_hover_text("Run selected text, or the entire editor · ⌘Enter")
                        .clicked();
                }
                ui.label(RichText::new("⌘Enter").size(12.0).color(MUTED));
            });
        });
        if changed {
            self.changed();
        }
        if run {
            self.run(ctx);
        }
    }

    fn sql_editor(&mut self, ui: &mut egui::Ui) {
        let tab = &mut self.tabs[self.active];
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("SQL EDITOR").size(10.0).color(MUTED).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} lines", tab.saved.sql.lines().count().max(1)))
                        .size(11.0)
                        .color(MUTED),
                );
                ui.label(RichText::new("Spark SQL").size(11.0).color(MUTED));
            });
        });
        ui.add_space(8.0);
        egui::ScrollArea::both()
            .id_salt(("sql_scroll", tab.saved.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let cached_source = &mut tab.highlight_source;
                let cached_job = &mut tab.highlight;
                let mut layouter = |ui: &egui::Ui, text: &str, _width: f32| {
                    if text != cached_source || cached_job.sections.is_empty() {
                        *cached_source = text.to_owned();
                        *cached_job = highlight_sql(text);
                    }
                    ui.fonts(|fonts| fonts.layout_job(cached_job.clone()))
                };
                let output = egui::TextEdit::multiline(&mut tab.saved.sql)
                    .id(egui::Id::new(("sql", tab.saved.id)))
                    .code_editor()
                    .font(egui::TextStyle::Monospace)
                    .desired_width(ui.available_width().max(600.0))
                    .desired_rows(12)
                    .frame(false)
                    .hint_text("SELECT …\n\nWrite a query, choose a connection, and press ⌘Enter.")
                    .layouter(&mut layouter)
                    .show(ui);
                changed = output.response.changed();
                tab.selection = output
                    .cursor_range
                    .map(|range| range.as_sorted_char_range());
            });
        if changed {
            self.changed();
        }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let tab = &mut self.tabs[self.active];
        ui.horizontal(|ui| {
            ui.label(RichText::new("RESULTS").size(10.0).color(MUTED).strong());
            if !tab.columns.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} rows · {} columns",
                        tab.rows.len(),
                        tab.columns.len()
                    ))
                    .size(12.0)
                    .color(ACCENT),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        tab.more && !tab.busy && !self.demo,
                        egui::Button::new("Load 1,000 more"),
                    )
                    .clicked()
                {
                    tab.busy = true;
                    tab.started = Some(Instant::now());
                    tab.status = "Fetching more rows…".into();
                    if let Some(worker) = &tab.worker {
                        worker.more();
                    }
                }
                if tab.busy {
                    ui.spinner();
                }
            });
        });
        if let Some(error) = &tab.error {
            egui::Frame::new()
                .fill(Color32::from_rgb(52, 32, 37))
                .inner_margin(12.0)
                .corner_radius(5.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(error).color(ERROR));
                        });
                });
        }
        if tab.columns.is_empty() {
            ui.add_space((ui.available_height() * 0.27).max(12.0));
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if tab.busy {
                        "Your query is running"
                    } else if tab.elapsed.is_some() && tab.error.is_none() {
                        "No result table"
                    } else {
                        "Run a query to see results"
                    })
                    .size(23.0)
                    .color(TEXT),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new(if tab.busy {
                        "Keep writing, or run another query in a new tab."
                    } else {
                        "Query results appear here, 1,000 rows at a time."
                    })
                    .color(MUTED),
                );
            });
            return;
        }
        ui.add_space(5.0);
        // TableBuilder virtualizes rows; horizontal clipping bounds painting for wide schemas.
        egui::ScrollArea::horizontal()
            .id_salt(("result_x", tab.saved.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = (tab.columns.len() as f32 * 170.0 + 54.0).max(ui.available_width());
                ui.set_min_width(width);
                TableBuilder::new(ui)
                    .id_salt(("results", tab.saved.id))
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(TableColumn::exact(48.0))
                    .columns(
                        TableColumn::initial(170.0).at_least(75.0).clip(true),
                        tab.columns.len(),
                    )
                    .header(43.0, |mut header| {
                        header.col(|ui| {
                            ui.label(RichText::new("#").color(MUTED));
                        });
                        for column in &tab.columns {
                            header.col(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&column.name).strong());
                                    ui.label(
                                        RichText::new(&column.data_type).size(9.0).color(MUTED),
                                    );
                                });
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(29.0, tab.rows.len(), |mut row| {
                            let index = row.index();
                            row.col(|ui| {
                                ui.label(
                                    RichText::new((index + 1).to_string())
                                        .size(11.0)
                                        .color(MUTED),
                                );
                            });
                            for (column, value) in tab.rows[index].iter().enumerate() {
                                row.col(|ui| {
                                    let label = value.as_deref().unwrap_or("NULL");
                                    // A single huge value must not create an enormous text layout every frame.
                                    let display = preview_cell(label);
                                    let text = RichText::new(display)
                                        .monospace()
                                        .size(12.0)
                                        .color(if value.is_some() { TEXT } else { MUTED });
                                    let response = ui.selectable_label(
                                        tab.selected_cell == Some((index, column)),
                                        text,
                                    );
                                    if response.clicked() {
                                        tab.selected_cell = Some((index, column));
                                    }
                                    response.context_menu(|ui| {
                                        if ui.button("Copy cell").clicked() {
                                            ui.ctx().copy_text(label.to_owned());
                                            ui.close_menu();
                                        }
                                        if ui.button("Copy row").clicked() {
                                            ui.ctx().copy_text(
                                                tab.rows[index]
                                                    .iter()
                                                    .map(|c| c.as_deref().unwrap_or("NULL"))
                                                    .collect::<Vec<_>>()
                                                    .join("\t"),
                                            );
                                            ui.close_menu();
                                        }
                                    });
                                });
                            }
                        });
                    });
            });
    }

    fn connection_dialog(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut completed = None;
        if let Some(rx) = &editor.saving
            && let Ok(result) = rx.try_recv()
        {
            editor.saving = None;
            match result {
                Ok(profile) => completed = Some(profile),
                Err(error) => editor.error = Some(error),
            }
        }
        if let Some(profile) = completed {
            let id = profile.id;
            if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == id) {
                *existing = profile;
            } else {
                self.profiles.push(profile);
            }
            if self.tabs[self.active].saved.profile.is_none() {
                self.tabs[self.active].switch_profile(Some(id));
            }
            self.editor = None;
            self.changed();
            return;
        }
        let mut open = true;
        let mut save = false;
        let title = if editor.is_new {
            "New connection"
        } else {
            "Edit connection"
        };
        egui::Window::new(title)
            .id(egui::Id::new("connection_editor"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(editor.saving.is_none(), |ui| {
                    ui.label(RichText::new("Spark SQL through Kyuubi · LDAP").color(MUTED));
                    ui.add_space(12.0);
                    egui::Grid::new("profile_fields")
                        .num_columns(2)
                        .spacing([18.0, 12.0])
                        .show(ui, |ui| {
                            for (name, value) in [
                                ("Name", &mut editor.profile.name),
                                ("Host", &mut editor.profile.host),
                            ] {
                                ui.label(name);
                                ui.add(egui::TextEdit::singleline(value).desired_width(345.0));
                                ui.end_row();
                            }
                            ui.label("Port");
                            ui.add(egui::DragValue::new(&mut editor.profile.port).range(1..=65535));
                            ui.end_row();
                            ui.label("Username");
                            ui.add(
                                egui::TextEdit::singleline(&mut editor.profile.username)
                                    .desired_width(345.0),
                            );
                            ui.end_row();
                            ui.label("Password");
                            ui.add(
                                egui::TextEdit::singleline(&mut *editor.password)
                                    .password(true)
                                    .desired_width(345.0)
                                    .hint_text(if editor.is_new {
                                        "Saved in macOS Keychain"
                                    } else {
                                        "Leave blank to keep saved password"
                                    }),
                            );
                            ui.end_row();
                            ui.label("Database");
                            ui.add(
                                egui::TextEdit::singleline(&mut editor.profile.database)
                                    .desired_width(345.0),
                            );
                            ui.end_row();
                        });
                    ui.add_space(12.0);
                    ui.separator();
                    ui.label(RichText::new("Session parameters").strong());
                    ui.label(
                        RichText::new("Applied when each tab opens a session.")
                            .size(12.0)
                            .color(MUTED),
                    );
                    let mut remove = None;
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for (i, (key, value)) in editor.parameters.iter_mut().enumerate() {
                                ui.push_id(i, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(key)
                                                .desired_width(275.0)
                                                .hint_text("kyuubi.engine.share.level.subdomain"),
                                        );
                                        ui.add(
                                            egui::TextEdit::singleline(value)
                                                .desired_width(150.0)
                                                .hint_text("Value"),
                                        );
                                        if ui.small_button("×").clicked() {
                                            remove = Some(i);
                                        }
                                    });
                                });
                            }
                        });
                    if let Some(i) = remove {
                        editor.parameters.remove(i);
                    }
                    if ui.small_button("+ Add parameter").clicked() {
                        editor.parameters.push((String::new(), String::new()));
                    }
                    if let Some(error) = &editor.error {
                        ui.colored_label(ERROR, error);
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Passwords stay in your macOS Keychain.")
                                .size(12.0)
                                .color(MUTED),
                        );
                        save = ui.button("Save connection").clicked();
                    });
                });
                if editor.saving.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Saving to Keychain…");
                    });
                }
            });
        if save {
            let validation = (|| -> anyhow::Result<()> {
                editor.profile.validate()?;
                anyhow::ensure!(
                    !editor.is_new || !editor.password.is_empty(),
                    "Enter a password for this connection."
                );
                let mut parameters = BTreeMap::new();
                for (key, value) in &editor.parameters {
                    if key.trim().is_empty() && value.is_empty() {
                        continue;
                    }
                    anyhow::ensure!(
                        !key.trim().is_empty(),
                        "Session parameter keys cannot be empty."
                    );
                    anyhow::ensure!(
                        parameters
                            .insert(key.trim().to_string(), value.clone())
                            .is_none(),
                        "Duplicate session parameter: {key}"
                    );
                }
                editor.profile.parameters = parameters;
                Ok(())
            })();
            match validation {
                Err(error) => editor.error = Some(error.to_string()),
                Ok(()) => {
                    let (tx, rx) = mpsc::channel();
                    let profile = editor.profile.clone();
                    let password =
                        std::mem::replace(&mut editor.password, Zeroizing::new(String::new()));
                    let ctx = ctx.clone();
                    editor.saving = Some(rx);
                    std::thread::spawn(move || {
                        let result = if password.is_empty() {
                            Ok(())
                        } else {
                            storage::set_password(profile.id, &password)
                        };
                        let _ = tx.send(result.map(|_| profile).map_err(|e| format!("{e:#}")));
                        ctx.request_repaint();
                    });
                }
            }
        }
        if !open && editor.saving.is_none() {
            self.editor = None;
        }
    }

    fn seed_demo(&mut self) {
        let tab = &mut self.tabs[self.active];
        tab.columns = vec![
            Column {
                name: "route".into(),
                data_type: "STRING".into(),
            },
            Column {
                name: "departures".into(),
                data_type: "BIGINT".into(),
            },
            Column {
                name: "avg_fare".into(),
                data_type: "DECIMAL".into(),
            },
            Column {
                name: "currency".into(),
                data_type: "STRING".into(),
            },
            Column {
                name: "updated_at".into(),
                data_type: "TIMESTAMP".into(),
            },
        ];
        let routes = [
            "BKK → SIN",
            "LHR → JFK",
            "ARN → HEL",
            "HND → ICN",
            "CDG → FCO",
            "BER → LIS",
            "AMS → BCN",
            "DXB → BOM",
        ];
        tab.rows = (0..1000)
            .map(|i| {
                vec![
                    Some(routes[i % routes.len()].into()),
                    Some((2480 - i).to_string()),
                    Some(format!("{}.{:02}", 140 + i % 500, i % 100)),
                    Some("USD".into()),
                    Some("2026-09-11 09:15:00".into()),
                ]
            })
            .collect();
        tab.more = true;
        tab.status = "Demo preview · 1,000 sample rows · no server connection".into();
        tab.elapsed = Some(Duration::from_millis(248));
    }
}

impl eframe::App for Qrow {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for tab in &mut self.tabs {
            tab.events();
        }
        if let Some(saver) = &self.saver {
            for error in saver.errors.try_iter() {
                self.message = Some(error);
            }
        }
        if self.editor.is_none() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::T)) {
                self.new_tab();
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
                self.run(ctx);
            }
        }
        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::vec2(18.0, 8.0)),
            )
            .show(ctx, |ui| {
                let tab = &self.tabs[self.active];
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        dot.center(),
                        3.0,
                        if tab.connected || self.demo {
                            ACCENT
                        } else {
                            MUTED
                        },
                    );
                    ui.label(RichText::new(&tab.status).size(12.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if self.demo {
                                "Sample data"
                            } else if self.saver.is_none() {
                                "Saving disabled"
                            } else if self.dirty.is_some() {
                                "Saving…"
                            } else {
                                "Workspace saved"
                            })
                            .size(11.0)
                            .color(MUTED),
                        );
                        if let Some(elapsed) = tab.started.map(|t| t.elapsed()).or(tab.elapsed) {
                            ui.label(
                                RichText::new(format!("{:.2}s", elapsed.as_secs_f64()))
                                    .size(12.0)
                                    .color(ACCENT),
                            );
                        }
                    });
                });
            });
        self.sidebar(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(18.0))
            .show(ctx, |ui| {
                self.tab_bar(ui);
                ui.add_space(10.0);
                self.toolbar(ui, ctx);
                ui.add_space(8.0);
                ui.separator();
                if let Some(message) = &self.message {
                    ui.colored_label(ERROR, message);
                }
                egui::TopBottomPanel::top("sql_editor_panel")
                    .resizable(true)
                    .default_height(285.0)
                    .height_range(140.0..=650.0)
                    .frame(egui::Frame::new().inner_margin(egui::vec2(4.0, 12.0)))
                    .show_inside(ui, |ui| self.sql_editor(ui));
                ui.add_space(10.0);
                self.results(ui);
            });
        self.connection_dialog(ctx);
        if self
            .dirty
            .is_some_and(|time| time.elapsed() >= Duration::from_millis(500))
        {
            if let Some(saver) = &self.saver {
                saver.save(self.snapshot());
            }
            self.dirty = None;
        }
        if self.dirty.is_some() || self.tabs.iter().any(|t| t.busy) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.first_frame {
            eprintln!(
                "Qrow first UI frame: {} ms",
                self.started.elapsed().as_millis()
            );
            self.first_frame = false;
        }
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.shutdown();
            }
        }
        let snapshot = self.snapshot();
        if let Some(saver) = &mut self.saver {
            saver.finish(snapshot);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.wait_for_shutdown(deadline.saturating_duration_since(Instant::now()));
            }
        }
    }
}

fn highlight_sql(sql: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (range, kind) in sql::tokens(sql) {
        let color = match kind {
            Kind::Keyword => Color32::from_rgb(157, 176, 255),
            Kind::String => ACCENT,
            Kind::Comment => Color32::from_rgb(112, 127, 136),
            Kind::Number => Color32::from_rgb(235, 185, 125),
            Kind::Identifier => Color32::from_rgb(141, 208, 229),
            _ => TEXT,
        };
        job.append(
            &sql[range],
            0.0,
            TextFormat {
                font_id: FontId::monospace(15.0),
                color,
                ..Default::default()
            },
        );
    }
    job
}

fn preview_cell(value: &str) -> String {
    let mut chars = value.chars();
    let mut preview: String = chars
        .by_ref()
        .take(180)
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}

fn demo_workspace() -> Workspace {
    let profiles: Vec<_> = ["rivendell-s", "rivendell-xl", "analytics-s"]
        .into_iter()
        .map(|name| Profile {
            name: name.into(),
            host: "demo.local".into(),
            username: format!("kyuubi-{name}"),
            ..Default::default()
        })
        .collect();
    let mut tab = SavedTab::new(1, Some(profiles[0].id));
    tab.title = "Route overview".into();
    tab.sql = "-- A quick look at route performance\nSELECT\n    route,\n    COUNT(*) AS departures,\n    ROUND(AVG(fare), 2) AS avg_fare,\n    currency,\n    MAX(updated_at) AS updated_at\nFROM flight_events\nWHERE departure_date >= '2026-09-01'\nGROUP BY route, currency\nORDER BY departures DESC;".into();
    Workspace {
        version: 1,
        profiles: profiles.clone(),
        tabs: vec![tab, SavedTab::new(2, Some(profiles[1].id))],
        active_tab: 0,
    }
}

mod results;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{
    Disableable, IconName, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    table::{Table, TableState},
};
use qrow::{
    model::{Profile, SavedTab, Workspace},
    sql,
    storage::{self, Saver},
    worker::{Event, Worker},
};
use results::Results;
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::Zeroizing;

actions!(qrow, [RunQuery, NewTab, CloseTab, ToggleSidebar, Quit]);
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-enter", RunQuery, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![
        Menu {
            name: "Qrow".into(),
            items: vec![MenuItem::action("Quit Qrow", Quit)],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Query Tab", NewTab),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", gpui_component::input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", gpui_component::input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", gpui_component::input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", gpui_component::input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", gpui_component::input::Paste, OsAction::Paste),
                MenuItem::os_action(
                    "Select All",
                    gpui_component::input::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        },
        Menu {
            name: "Query".into(),
            items: vec![
                MenuItem::action("Run Query", RunQuery),
                MenuItem::action("Toggle Connections", ToggleSidebar),
            ],
        },
    ]);
}
struct Tab {
    saved: SavedTab,
    input: Entity<InputState>,
    table: Entity<TableState<Results>>,
    _subscription: Subscription,
    worker: Option<Worker>,
    busy: bool,
    cancelling: bool,
    connected: bool,
    more: bool,
    status: String,
    error: Option<String>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
}
struct ProfileEditor {
    profile: Profile,
    fields: Vec<Entity<InputState>>,
    is_new: bool,
    error: Option<String>,
    saving: Option<mpsc::Receiver<Result<Profile, String>>>,
    confirm_delete: bool,
}
pub struct Qrow {
    profiles: Vec<Profile>,
    tabs: Vec<Tab>,
    active: usize,
    form: Option<ProfileEditor>,
    saver: Option<Saver>,
    dirty: Option<Instant>,
    message: Option<String>,
    demo: bool,
    sidebar: bool,
    sidebar_width: Pixels,
    editor_height: Pixels,
    resize: Option<(bool, Point<Pixels>, Pixels)>,
    focus: FocusHandle,
    _quit: Subscription,
    wake: async_channel::Sender<()>,
}
impl Qrow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, demo: bool, started: Instant) -> Self {
        let (wake, notifications) = async_channel::bounded(1);
        let save_wake = wake.clone();
        let path = storage::workspace_path();
        let (workspace, message, saver) = if demo {
            (demo_workspace(), None, None)
        } else {
            match storage::load(&path) {
                Ok(w) => (
                    w,
                    None,
                    Some(Saver::with_wake(path, move || {
                        let _ = save_wake.try_send(());
                    })),
                ),
                Err(e) => (
                    Workspace::default(),
                    Some(format!(
                        "Cannot load workspace: {e}. Saving disabled to protect the existing file."
                    )),
                    None,
                ),
            }
        };
        let quit = cx.on_app_quit(|this, cx| {
            this.finish(cx);
            async {}
        });
        let mut this = Self {
            profiles: workspace.profiles,
            tabs: vec![],
            active: workspace.active_tab,
            form: None,
            saver,
            dirty: None,
            message,
            demo,
            sidebar: true,
            sidebar_width: px(200.),
            editor_height: px(285.),
            resize: None,
            focus: cx.focus_handle(),
            _quit: quit,
            wake,
        };
        for tab in workspace.tabs {
            let tab = this.make_tab(tab, window, cx);
            this.tabs.push(tab);
        }
        if this.tabs.is_empty() {
            let tab = this.make_tab(
                SavedTab::new(1, this.profiles.first().map(|p| p.id)),
                window,
                cx,
            );
            this.tabs.push(tab);
        }
        this.active = this.active.min(this.tabs.len() - 1);
        if demo {
            this.seed_demo(cx);
        }
        this.tabs[this.active]
            .input
            .update(cx, |s, cx| s.focus(window, cx));
        cx.spawn_in(window, async move |weak, cx| {
            let mut pending = false;
            loop {
                if pending {
                    Timer::after(Duration::from_millis(50)).await;
                } else if notifications.recv().await.is_err() {
                    break;
                }
                match weak.update(cx, |this, cx| this.tick(cx)) {
                    Ok(keep_polling) => pending = keep_polling,
                    Err(_) => break,
                }
            }
        })
        .detach();
        // Save on window release as well as Cmd-Q; closing the last window releases its root first.
        cx.on_release(|this, cx| this.finish(cx)).detach();
        eprintln!(
            "Qrow GPUI initialized in {:.0} ms{}",
            started.elapsed().as_secs_f64() * 1000.,
            if demo { " (demo)" } else { "" }
        );
        this
    }
    fn make_tab(&self, saved: SavedTab, window: &mut Window, cx: &mut Context<Self>) -> Tab {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .soft_wrap(false)
                .default_value(saved.sql.clone())
        });
        let subscription = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.changed(cx);
            }
        });
        let table =
            cx.new(|cx| TableState::new(Results::default(), window, cx).col_selectable(false));
        Tab {
            saved,
            input,
            table,
            _subscription: subscription,
            worker: None,
            busy: false,
            cancelling: false,
            connected: false,
            more: false,
            status: "Not connected".into(),
            error: None,
            started: None,
            elapsed: None,
        }
    }
    fn snapshot(&self, cx: &App) -> Workspace {
        Workspace {
            version: 1,
            profiles: self.profiles.clone(),
            tabs: self
                .tabs
                .iter()
                .map(|t| {
                    let mut saved = t.saved.clone();
                    saved.sql = t.input.read(cx).value().to_string();
                    saved
                })
                .collect(),
            active_tab: self.active,
        }
    }
    fn finish(&mut self, cx: &App) {
        if let Some(mut saver) = self.saver.take() {
            saver.finish(self.snapshot(cx));
        }
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.shutdown();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.wait_for_shutdown(deadline.saturating_duration_since(Instant::now()));
            }
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = Some(Instant::now());
        let _ = self.wake.try_send(());
        cx.notify();
    }
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            let events: Vec<_> = tab
                .worker
                .as_ref()
                .map(|w| w.events.try_iter().collect())
                .unwrap_or_default();
            changed |= !events.is_empty();
            for event in events {
                match event {
                    Event::Connecting => tab.status = "Connecting to Kyuubi…".into(),
                    Event::Connected => tab.connected = true,
                    Event::Running => tab.status = "Executing…".into(),
                    Event::Columns(columns) => {
                        tab.table.update(cx, |t, cx| {
                            t.delegate_mut().schema(columns);
                            t.refresh(cx);
                        });
                        tab.status = "Fetching preview…".into();
                    }
                    Event::Rows(rows) => tab.table.update(cx, |t, cx| {
                        t.delegate_mut().rows.extend(rows);
                        cx.notify();
                    }),
                    Event::Ready { more, limited } => {
                        tab.more = more;
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.elapsed = tab.started.take().map(|t| t.elapsed());
                        tab.status = if limited {
                            "Preview limit reached · 64 MiB or 100,000 rows"
                        } else if more {
                            "Preview · more rows available"
                        } else {
                            "Complete"
                        }
                        .into();
                    }
                    Event::Cancelled => {
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.more = false;
                        tab.elapsed = tab.started.take().map(|t| t.elapsed());
                        tab.status = "Cancelled · partial preview retained".into();
                    }
                    Event::Error {
                        message,
                        disconnected,
                    } => {
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.more = false;
                        if disconnected {
                            tab.connected = false;
                        }
                        tab.elapsed = tab.started.take().map(|t| t.elapsed());
                        tab.status = if disconnected {
                            "Disconnected · run again to reconnect"
                        } else {
                            "Query failed"
                        }
                        .into();
                        tab.error = Some(message);
                    }
                    Event::CancelError(message) => {
                        tab.error = Some(message);
                        tab.cancelling = false;
                    }
                    Event::Disconnected => {
                        tab.connected = false;
                        if !tab.busy {
                            tab.status = "Not connected".into();
                        }
                    }
                }
            }
        }
        if self
            .dirty
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(400))
        {
            if let Some(saver) = &self.saver {
                saver.save(self.snapshot(cx));
            }
            self.dirty = None;
            changed = true;
        }
        if let Some(saver) = &self.saver {
            for error in saver.errors.try_iter() {
                self.message = Some(error);
                changed = true;
            }
        }
        let result = self
            .form
            .as_ref()
            .and_then(|f| f.saving.as_ref())
            .and_then(|r| r.try_recv().ok());
        if let Some(result) = result {
            match result {
                Ok(profile) => {
                    let id = profile.id;
                    if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == id) {
                        *existing = profile;
                    } else {
                        self.profiles.push(profile);
                    }
                    for tab in &mut self.tabs {
                        if tab.saved.profile == Some(id) {
                            if let Some(worker) = tab.worker.take() {
                                worker.shutdown();
                            }
                            tab.connected = false;
                            tab.status = "Not connected".into();
                        }
                    }
                    if self.tabs[self.active].saved.profile.is_none() {
                        self.tabs[self.active].saved.profile = Some(id);
                    }
                    self.form = None;
                    self.dirty = Some(Instant::now());
                }
                Err(error) => {
                    let form = self.form.as_mut().unwrap();
                    form.error = Some(error);
                    form.saving = None;
                }
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
        self.dirty.is_some() || self.form.as_ref().is_some_and(|f| f.saving.is_some())
    }
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index;
        self.tabs[index]
            .input
            .update(cx, |s, cx| s.focus(window, cx));
        self.changed(cx);
    }
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() {
            return;
        }
        let number = self
            .tabs
            .iter()
            .filter_map(|t| {
                t.saved
                    .title
                    .strip_prefix("Query ")
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let profile = self.tabs[self.active].saved.profile;
        let tab = self.make_tab(SavedTab::new(number, profile), window, cx);
        self.tabs.push(tab);
        self.activate(self.tabs.len() - 1, window, cx);
    }
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() || self.tabs[index].busy {
            return;
        }
        if let Some(w) = &self.tabs[index].worker {
            w.shutdown();
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            let t = self.make_tab(
                SavedTab::new(1, self.profiles.first().map(|p| p.id)),
                window,
                cx,
            );
            self.tabs.push(t);
        }
        let active = if index < self.active {
            self.active - 1
        } else {
            self.active.min(self.tabs.len() - 1)
        };
        self.activate(active, window, cx);
    }
    fn switch_profile(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        if tab.busy || tab.saved.profile == Some(id) {
            return;
        }
        if let Some(worker) = tab.worker.take() {
            worker.shutdown();
        }
        tab.saved.profile = Some(id);
        tab.connected = false;
        tab.more = false;
        tab.error = None;
        tab.elapsed = None;
        tab.status = "Not connected".into();
        tab.table.update(cx, |t, cx| {
            t.delegate_mut().clear();
            t.clear_selection(cx);
            t.horizontal_scroll_handle.set_offset(point(px(0.), px(0.)));
            t.scroll_to_row(0, cx);
            t.refresh(cx);
        });
        self.changed(cx);
    }
    fn run(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() || self.tabs[self.active].busy {
            return;
        }
        if self.demo {
            self.seed_demo(cx);
            cx.notify();
            return;
        }
        let tab = &mut self.tabs[self.active];
        let query = tab.input.update(cx, |s, cx| {
            let selected = s
                .selected_text_range(false, window, cx)
                .filter(|s| !s.range.is_empty());
            selected
                .and_then(|r| s.text_for_range(r.range, &mut None, window, cx))
                .unwrap_or_else(|| s.value().to_string())
        });
        if let Err(e) = sql::validate_single(&query) {
            tab.error = Some(e.to_string());
            cx.notify();
            return;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|p| Some(p.id) == tab.saved.profile)
            .cloned()
        else {
            tab.error = Some("Choose a connection before running SQL.".into());
            cx.notify();
            return;
        };
        if tab.worker.is_none() {
            let wake = self.wake.clone();
            tab.worker = Some(Worker::new(Arc::new(move || {
                let _ = wake.try_send(());
            })));
        }
        tab.table.update(cx, |t, cx| {
            t.delegate_mut().clear();
            t.clear_selection(cx);
            t.horizontal_scroll_handle.set_offset(point(px(0.), px(0.)));
            t.scroll_to_row(0, cx);
            t.refresh(cx);
        });
        tab.more = false;
        tab.error = None;
        tab.elapsed = None;
        tab.busy = true;
        tab.cancelling = false;
        tab.started = Some(Instant::now());
        tab.status = "Preparing query…".into();
        tab.worker.as_ref().unwrap().run(profile, query);
        cx.notify();
    }
    fn more(&mut self, cx: &mut Context<Self>) {
        let t = &mut self.tabs[self.active];
        if t.busy || !t.more {
            return;
        }
        if let Some(w) = &t.worker {
            t.busy = true;
            t.started = Some(Instant::now());
            t.status = "Fetching preview…".into();
            w.more();
        }
        cx.notify();
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        let t = &mut self.tabs[self.active];
        if t.busy
            && !t.cancelling
            && let Some(w) = &t.worker
        {
            w.cancel();
            t.cancelling = true;
            t.status = "Cancelling…".into();
        }
        cx.notify();
    }
    fn edit_profile(
        &mut self,
        profile: Profile,
        is_new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !is_new
            && self
                .tabs
                .iter()
                .any(|t| t.saved.profile == Some(profile.id) && t.busy)
        {
            self.message =
                Some("Wait for queries using this connection to finish before editing it.".into());
            cx.notify();
            return;
        }
        let values = [
            profile.name.clone(),
            profile.host.clone(),
            profile.port.to_string(),
            profile.username.clone(),
            String::new(),
            profile.database.clone(),
            serde_json::to_string_pretty(&profile.parameters).unwrap(),
        ];
        let fields = values
            .into_iter()
            .enumerate()
            .map(|(i, value)| {
                cx.new(|cx| {
                    let input = InputState::new(window, cx).default_value(value);
                    if i == 4 {
                        input.masked(true).placeholder(if is_new {
                            "LDAP password"
                        } else {
                            "Leave blank to keep stored password"
                        })
                    } else if i == 6 {
                        input.multi_line(true).rows(5)
                    } else {
                        input
                    }
                })
            })
            .collect::<Vec<_>>();
        fields[0].update(cx, |s, cx| s.focus(window, cx));
        self.form = Some(ProfileEditor {
            profile,
            fields,
            is_new,
            error: None,
            saving: None,
            confirm_delete: false,
        });
        cx.notify();
    }
    fn save_profile(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.saving.is_some() {
            return;
        }
        let values: Vec<String> = form
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                if i == 4 {
                    String::new()
                } else {
                    f.read(cx).value().to_string()
                }
            })
            .collect();
        let mut profile = form.profile.clone();
        profile.name = values[0].trim().into();
        profile.host = values[1].trim().into();
        profile.username = values[3].trim().into();
        profile.database = values[5].trim().into();
        let parse = (|| -> anyhow::Result<()> {
            profile.port = values[2]
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Port must be a number from 1 to 65535."))?;
            profile.parameters = serde_json::from_str::<BTreeMap<String, String>>(&values[6])
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Session parameters must be a JSON object with string values: {e}"
                    )
                })?;
            profile.validate()?;
            Ok(())
        })();
        if let Err(e) = parse {
            form.error = Some(e.to_string());
            cx.notify();
            return;
        }
        let password = Zeroizing::new(form.fields[4].read(cx).unmask_value().to_string());
        if form.is_new && password.is_empty() && !self.demo {
            form.error = Some("Enter the LDAP password to store in Keychain.".into());
            cx.notify();
            return;
        }
        let (send, recv) = mpsc::channel();
        form.saving = Some(recv);
        let _ = self.wake.try_send(());
        form.error = None;
        let demo = self.demo;
        std::thread::spawn(move || {
            let result = if !demo && !password.is_empty() {
                storage::set_password(profile.id, &password).map_err(|e| e.to_string())
            } else {
                Ok(())
            };
            let _ = send.send(result.map(|()| profile));
        });
        cx.notify();
    }
    fn delete_profile(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if !form.confirm_delete {
            form.confirm_delete = true;
            cx.notify();
            return;
        }
        let id = form.profile.id;
        self.profiles.retain(|p| p.id != id);
        for tab in &mut self.tabs {
            if tab.saved.profile == Some(id) {
                if let Some(w) = tab.worker.take() {
                    w.shutdown();
                }
                tab.saved.profile = None;
                tab.connected = false;
                tab.more = false;
                tab.status = "Not connected".into();
            }
        }
        self.form = None;
        self.changed(cx);
    }
    fn seed_demo(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        tab.table.update(cx, |t, cx| {
            let data = t.delegate_mut();
            data.clear();
            let mut columns: Vec<_> = [
                ("route", "STRING"),
                ("departures", "BIGINT"),
                ("avg_fare", "DOUBLE"),
                ("currency", "STRING"),
                ("updated_at", "TIMESTAMP"),
                ("carrier", "STRING"),
                ("load_factor", "DOUBLE"),
                ("cancelled", "BOOLEAN"),
            ]
            .into_iter()
            .map(|(name, data_type)| qrow::model::Column {
                name: name.into(),
                data_type: data_type.into(),
            })
            .collect();
            columns.extend((9..=141).map(|n| qrow::model::Column {
                name: format!("metric_{n}"),
                data_type: "DOUBLE".into(),
            }));
            data.schema(columns);
            data.rows = (0..1000)
                .map(|i| {
                    let mut row = vec![
                        Some(
                            [
                                "BKK → SIN",
                                "LHR → JFK",
                                "HEL → ARN",
                                "CDG → FCO",
                                "DXB → BKK",
                                "NRT → ICN",
                            ][i % 6]
                                .into(),
                        ),
                        Some((3821 - i).to_string()),
                        Some(format!("{:.2}", 142.5 + (i % 93) as f64 * 3.2)),
                        Some("USD".into()),
                        Some("2026-09-11 09:41:03".into()),
                        if i % 7 == 0 {
                            None
                        } else {
                            Some(["AY", "BA", "SQ"][i % 3].into())
                        },
                        Some(format!("0.{:02}", 70 + i % 29)),
                        Some("false".into()),
                    ];
                    row.extend((9..=141).map(|c| Some(format!("{:.2}", (i * c) as f64 / 10.))));
                    row
                })
                .collect();
            t.refresh(cx);
        });
        tab.status = "Complete · demo data".into();
        tab.elapsed = Some(Duration::from_millis(842));
    }
    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = self.form.as_ref().unwrap();
        let saving = form.saving.is_some();
        let labels = [
            "Name",
            "Host",
            "Port",
            "LDAP username",
            "Password · macOS Keychain",
            "Default database",
            "Session parameters · JSON string values",
        ];
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(black().opacity(0.55))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(520.))
                    .max_h(relative(0.95))
                    .bg(rgb(0x24272e))
                    .border_1()
                    .border_color(rgb(0x414650))
                    .rounded_lg()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if form.is_new {
                                "New connection"
                            } else {
                                "Edit connection"
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0x9ca6b5))
                            .child("Kyuubi / Spark · HiveServer2 · LDAP"),
                    )
                    .child(
                        div()
                            .id("connection-fields")
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(form.fields.iter().enumerate().map(|(i, f)| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(rgb(0xa7b0bf))
                                            .child(labels[i]),
                                    )
                                    .child(
                                        Input::new(f)
                                            .small()
                                            .disabled(saving)
                                            .when(i == 6, |input| input.h(px(90.))),
                                    )
                            })),
                    )
                    .when_some(form.error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(0xef9292))
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(!form.is_new, |el| {
                                el.child(
                                    Button::new("delete-profile")
                                        .small()
                                        .ghost()
                                        .label(if form.confirm_delete {
                                            "Confirm delete"
                                        } else {
                                            "Delete"
                                        })
                                        .disabled(saving)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.delete_profile(cx)),
                                        ),
                                )
                            })
                            .when(!form.is_new, |el| {
                                el.child(
                                    Button::new("duplicate-profile")
                                        .small()
                                        .ghost()
                                        .label("Duplicate")
                                        .disabled(saving)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let mut profile =
                                                this.form.as_ref().unwrap().profile.clone();
                                            profile.id = Uuid::new_v4();
                                            profile.name.push_str(" copy");
                                            this.edit_profile(profile, true, window, cx);
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                Button::new("cancel-profile")
                                    .small()
                                    .label("Cancel")
                                    .disabled(saving)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.form = None;
                                        this.tabs[this.active]
                                            .input
                                            .update(cx, |s, cx| s.focus(window, cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("save-profile")
                                    .small()
                                    .primary()
                                    .label(if saving {
                                        "Saving…"
                                    } else {
                                        "Save connection"
                                    })
                                    .disabled(saving)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}
impl Render for Qrow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.editor_height = self
            .editor_height
            .min(window.viewport_size().height - px(260.))
            .max(px(100.));
        let tab = &self.tabs[self.active];
        let active_profile = tab.saved.profile;
        let busy = tab.busy;
        let profiles = self.profiles.clone();
        let weak = cx.weak_entity();
        let profile = self.profiles.iter().find(|p| Some(p.id) == active_profile);
        let profile_name = profile
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Choose connection".into());
        let database = profile.map(|p| p.database.clone()).unwrap_or_default();
        let data = tab.table.read(cx).delegate();
        let count = format!("{} rows · {} columns", data.rows.len(), data.columns.len());
        let elapsed = tab
            .elapsed
            .map(|d| format!("{:.2}s", d.as_secs_f64()))
            .unwrap_or_default();
        let error = tab.error.clone();
        let status = tab.status.clone();
        let input = tab.input.clone();
        let table = tab.table.clone();
        let more = tab.more;
        let cancelling = tab.cancelling;
        let connected = tab.connected;
        let sidebar = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x20232a))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(0x9ca5b4))
                    .child("CONNECTIONS")
                    .child(
                        Button::new("add-connection")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("New connection")
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.edit_profile(Profile::default(), true, w, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("connections-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_1()
                    .children(self.profiles.iter().enumerate().map(|(i, p)| {
                        let id = p.id;
                        let edit = p.clone();
                        div()
                            .id(("profile", i))
                            .h(px(30.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_sm()
                            .text_size(px(12.))
                            .when(active_profile == Some(id), |el| el.bg(rgb(0x303745)))
                            .hover(|el| el.bg(rgb(0x2b303a)))
                            .cursor_pointer()
                            .child(
                                gpui_component::Icon::new(IconName::SquareTerminal)
                                    .size(px(13.))
                                    .text_color(rgb(0x8595ac)),
                            )
                            .child(div().flex_1().truncate().child(p.name.clone()))
                            .child(
                                Button::new(("edit-profile", i))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Settings2)
                                    .tooltip("Edit connection")
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        cx.stop_propagation();
                                        this.edit_profile(edit.clone(), false, w, cx);
                                    })),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.switch_profile(id, cx)),
                            )
                    })),
            )
            .when(self.profiles.is_empty(), |el| {
                el.child(
                    div()
                        .p_3()
                        .text_size(px(12.))
                        .text_color(rgb(0x808a9b))
                        .child("Add a Kyuubi connection to get started."),
                )
            })
            .child(
                div()
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(rgb(0x7f899a))
                    .child("Spark / Kyuubi"),
            );
        let tabs = div()
            .h(px(35.))
            .flex()
            .items_center()
            .bg(rgb(0x20232a))
            .border_b_1()
            .border_color(rgb(0x323640))
            .child(
                Button::new("sidebar-toggle")
                    .ghost()
                    .small()
                    .icon(IconName::PanelLeft)
                    .tooltip("Toggle connections · ⌘B")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar = !this.sidebar;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("tabs")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(self.tabs.iter().enumerate().map(|(i, t)| {
                        div()
                            .id(("tab", i))
                            .h(px(35.))
                            .min_w(px(130.))
                            .max_w(px(220.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_r_1()
                            .border_color(rgb(0x323640))
                            .cursor_pointer()
                            .when(i == self.active, |el| {
                                el.bg(rgb(0x282c34))
                                    .border_b_2()
                                    .border_color(rgb(0x7aa2f7))
                            })
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(if t.busy { 0xe5c07b } else { 0x7aa2f7 }))
                                    .child(if t.busy { "●" } else { "SQL" }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(12.))
                                    .child(t.saved.title.clone()),
                            )
                            .child(
                                Button::new(("close-tab", i))
                                    .ghost()
                                    .xsmall()
                                    .disabled(t.busy)
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        cx.stop_propagation();
                                        this.close_tab(i, w, cx);
                                    })),
                            )
                            .on_click(cx.listener(move |this, _, w, cx| this.activate(i, w, cx)))
                    })),
            )
            .child(
                Button::new("new-tab")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip("New query · ⌘T")
                    .on_click(cx.listener(|this, _, w, cx| this.new_tab(&NewTab, w, cx))),
            );
        let toolbar = div()
            .h(px(38.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(rgb(0x323640))
            .child(
                Button::new("connection-picker")
                    .ghost()
                    .small()
                    .label(profile_name)
                    .icon(IconName::ChevronDown)
                    .disabled(busy)
                    .dropdown_menu(move |mut menu, _, _| {
                        for p in &profiles {
                            let id = p.id;
                            let weak = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(p.name.clone())
                                    .checked(active_profile == Some(id))
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            weak.update(cx, |this, cx| this.switch_profile(id, cx));
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0x929cab))
                    .child(database),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x7f899a))
                    .child("⌘↵"),
            )
            .when(!busy, |el| {
                el.child(
                    Button::new("run")
                        .small()
                        .primary()
                        .icon(IconName::ArrowRight)
                        .label("Run")
                        .on_click(cx.listener(|this, _, w, cx| this.run(&RunQuery, w, cx))),
                )
            })
            .when(busy, |el| {
                el.child(
                    Button::new("cancel")
                        .small()
                        .label(if cancelling {
                            "Cancelling…"
                        } else {
                            "Cancel"
                        })
                        .disabled(cancelling)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
            });
        let editor = div().size_full().py_2().bg(rgb(0x282c34)).child(
            Input::new(&input)
                .appearance(false)
                .font_family("Menlo")
                .text_size(px(13.))
                .size_full(),
        );
        let results = div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .bg(rgb(0x242830))
                    .border_b_1()
                    .border_color(rgb(0x353a44))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Results"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x9ba6b6))
                            .child(count),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("copy-cell")
                            .ghost()
                            .xsmall()
                            .label("Copy cell")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tabs[this.active]
                                    .table
                                    .update(cx, |t, cx| t.delegate().copy_cell(cx));
                            })),
                    )
                    .child(
                        Button::new("more")
                            .ghost()
                            .xsmall()
                            .label("Load 1,000 more")
                            .disabled(!more || busy)
                            .on_click(cx.listener(|this, _, _, cx| this.more(cx))),
                    ),
            )
            .when_some(error, |el, error| {
                el.child(
                    div()
                        .id("query-error")
                        .max_h(px(100.))
                        .overflow_y_scroll()
                        .p_3()
                        .bg(rgb(0x3c2c32))
                        .text_color(rgb(0xefaaaa))
                        .text_size(px(12.))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .min_w_0()
                    .child(Table::new(&table).small().stripe(true).bordered(false))
                    .when(self.form.is_none(), |el| {
                        el.child(results::horizontal_scroll(&table))
                    }),
            );
        let vertical_handle = div()
            .id("editor-splitter")
            .h(px(5.))
            .w_full()
            .flex_shrink_0()
            .bg(rgb(0x242830))
            .hover(|s| s.bg(rgb(0x617dad)))
            .cursor(CursorStyle::ResizeUpDown)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.resize = Some((false, e.position, this.editor_height));
                    cx.stop_propagation();
                }),
            );
        let horizontal_handle = div()
            .id("sidebar-splitter")
            .w(px(4.))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(0x242830))
            .hover(|s| s.bg(rgb(0x617dad)))
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    this.resize = Some((true, e.position, this.sidebar_width));
                    cx.stop_propagation();
                }),
            );
        let center = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(tabs)
            .child(toolbar)
            .child(div().h(self.editor_height).flex_shrink_0().child(editor))
            .child(vertical_handle)
            .child(div().flex_1().min_h_0().child(results));
        div()
            .relative()
            .key_context("Qrow")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x282c34))
            .text_color(rgb(0xcdd3de))
            .font_family(".AppleSystemUIFont")
            .text_size(px(13.))
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(|this, _: &CloseTab, w, cx| this.close_tab(this.active, w, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar = !this.sidebar;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                let Some((horizontal, start, initial)) = this.resize else {
                    return;
                };
                if e.pressed_button != Some(MouseButton::Left) {
                    this.resize = None;
                    return;
                }
                if horizontal {
                    this.sidebar_width =
                        (initial + e.position.x - start.x).clamp(px(150.), px(360.));
                } else {
                    this.editor_height = (initial + e.position.y - start.y)
                        .clamp(px(100.), window.viewport_size().height - px(260.));
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar, |el| {
                        el.child(
                            div()
                                .w(self.sidebar_width)
                                .flex_shrink_0()
                                .h_full()
                                .child(sidebar),
                        )
                        .child(horizontal_handle)
                    })
                    .child(center),
            )
            .when_some(self.message.clone(), |el, message| {
                el.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(rgb(0xe5c07b))
                        .child(message),
                )
            })
            .child(
                div()
                    .h(px(25.))
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(rgb(0x20232a))
                    .border_t_1()
                    .border_color(rgb(0x323640))
                    .text_size(px(11.))
                    .text_color(rgb(0x99a4b5))
                    .child(
                        div()
                            .text_color(rgb(if busy {
                                0xe5c07b
                            } else if connected || self.demo {
                                0x98c379
                            } else {
                                0x7f899a
                            }))
                            .child("●"),
                    )
                    .child(status)
                    .child(div().flex_1())
                    .child(elapsed)
                    .child(if self.demo {
                        "Demo · nothing is saved"
                    } else if self.saver.is_none() {
                        "Workspace saving disabled"
                    } else if self.dirty.is_some() {
                        "Saving…"
                    } else {
                        "Workspace saved"
                    }),
            )
            .when(self.form.is_some(), |el| el.child(self.render_form(cx)))
    }
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

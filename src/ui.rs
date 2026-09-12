mod connection_form;
mod profile_view;
mod results;
mod workspace_view;
pub(crate) use workspace_view::WindowView;

use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    input::{EditorState, Input, InputEvent, InputState, TextareaState},
    table::TableState,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
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

actions!(
    qrow,
    [
        RunQuery,
        NewTab,
        CloseTab,
        ToggleSidebar,
        SaveConnection,
        Quit
    ]
);
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-enter", RunQuery, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-enter", SaveConnection, Some("ConnectionSettings")),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![
        Menu {
            disabled: false,
            name: "Qrow".into(),
            items: vec![MenuItem::action("Quit Qrow", Quit)],
        },
        Menu {
            disabled: false,
            name: "File".into(),
            items: vec![
                MenuItem::action("New Query Tab", NewTab),
                MenuItem::action("Close Tab", CloseTab),
            ],
        },
        Menu {
            disabled: false,
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", gpui_kit::component::input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", gpui_kit::component::input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", gpui_kit::component::input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", gpui_kit::component::input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", gpui_kit::component::input::Paste, OsAction::Paste),
                MenuItem::os_action(
                    "Select All",
                    gpui_kit::component::input::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        },
        Menu {
            disabled: false,
            name: "Query".into(),
            items: vec![
                MenuItem::action("Run Query", RunQuery),
                MenuItem::action("Toggle sidebar", ToggleSidebar),
            ],
        },
    ]);
}
struct Tab {
    saved: SavedTab,
    input: Entity<EditorState>,
    table: Entity<TableState<Results>>,
    _subscription: Subscription,
    worker: Option<Worker>,
    busy: bool,
    cancelling: bool,
    connected: bool,
    more: bool,
    pending_page: Option<usize>,
    status: String,
    error: Option<String>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
}
struct ProfileEditor {
    profile: Profile,
    fields: Vec<Entity<InputState>>,
    parameters: Entity<TextareaState>,
    is_new: bool,
    error: Option<String>,
    saving: Option<mpsc::Receiver<Result<Profile, String>>>,
    confirm_delete: bool,
    keep_connected: bool,
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
            sidebar_width: px(240.),
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
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                } else if notifications.recv().await.is_err() {
                    break;
                }
                match weak.update_in(cx, |this, window, cx| this.tick(window, cx)) {
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
            EditorState::new(window, cx)
                .language("sql")
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
            pending_page: None,
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
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            let events: Vec<_> = tab
                .worker
                .as_ref()
                .map(|w| w.events.try_iter().collect())
                .unwrap_or_default();
            changed |= !events.is_empty();
            for event in events {
                if tab.started.is_some() {
                    tab.table.update(cx, |table, cx| {
                        if table.delegate_mut().query_event(&event, tab.cancelling) {
                            cx.notify();
                        }
                    });
                }
                match event {
                    Event::Connecting => {
                        tab.busy = true;
                        tab.status = "Connecting to Spark (HiveServer2)…".into();
                    }
                    Event::Connected => tab.connected = true,
                    Event::Running => {
                        tab.busy = true;
                        tab.status = "Executing…".into();
                    }
                    Event::KeepAliveStarted => {
                        tab.busy = true;
                        tab.status = "Sending keep-alive…".into();
                    }
                    Event::KeepAliveFinished => {
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.status = "Connected · keep-alive enabled".into();
                    }
                    Event::Columns(columns) => {
                        tab.table.update(cx, |t, cx| {
                            t.delegate_mut().schema(columns);
                            t.refresh(cx);
                        });
                        tab.status = "Fetching preview…".into();
                    }
                    Event::Rows(rows) => {
                        tab.table.update(cx, |t, cx| {
                            t.delegate_mut().rows.extend(rows);
                            cx.notify();
                        });
                        if let Some(page) = tab.pending_page.take() {
                            results::select_page(&tab.table, page, cx);
                        }
                    }
                    Event::Ready { more, limited } => {
                        tab.more = more;
                        tab.pending_page = None;
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
                        tab.pending_page = None;
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
                        tab.pending_page = None;
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
                    Event::Disconnected | Event::IdleDisconnected => {
                        tab.connected = false;
                        tab.more = false;
                        tab.pending_page = None;
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.status = if matches!(event, Event::IdleDisconnected) {
                            "Disconnected after idle timeout · run to reconnect"
                        } else {
                            "Disconnected · run to reconnect"
                        }
                        .into();
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
                            tab.busy = false;
                            tab.cancelling = false;
                            tab.more = false;
                            tab.pending_page = None;
                            tab.status = "Not connected".into();
                        }
                    }
                    if self.tabs[self.active].saved.profile.is_none() {
                        self.tabs[self.active].saved.profile = Some(id);
                    }
                    self.form = None;
                    window.close_dialog(cx);
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
        tab.pending_page = None;
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
            t.delegate_mut().empty_message = Some("Waiting for query results…");
            t.clear_selection(cx);
            t.horizontal_scroll_handle.set_offset(point(px(0.), px(0.)));
            t.scroll_to_row(0, cx);
            t.refresh(cx);
        });
        tab.more = false;
        tab.pending_page = None;
        tab.error = None;
        tab.elapsed = None;
        tab.busy = true;
        tab.cancelling = false;
        tab.started = Some(Instant::now());
        tab.status = "Preparing query…".into();
        tab.worker.as_ref().unwrap().run(profile, query);
        cx.notify();
    }
    fn next_page(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        let data = tab.table.read(cx).delegate();
        let page = data.pagination.page() + 1;
        if page < data.pagination.pages(data.rows.len()) {
            tab.pending_page = None;
            results::select_page(&tab.table, page, cx);
        } else if !tab.busy
            && tab.more
            && let Some(worker) = &tab.worker
        {
            tab.pending_page = Some(page);
            tab.busy = true;
            tab.cancelling = false;
            tab.error = None;
            tab.started = Some(Instant::now());
            tab.status = "Fetching next page…".into();
            worker.more();
        }
        cx.notify();
    }

    fn previous_page(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        let page = tab.table.read(cx).delegate().pagination.page();
        tab.pending_page = None;
        results::select_page(&tab.table, page.saturating_sub(1), cx);
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
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        if tab.busy || !tab.connected || self.form.is_some() {
            return;
        }
        if let Some(worker) = &tab.worker {
            worker.disconnect();
            tab.busy = true;
            tab.status = "Disconnecting…".into();
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
            profile.lifecycle.idle_seconds.to_string(),
            if profile.lifecycle.keep_alive_seconds == 0 {
                "300".into()
            } else {
                profile.lifecycle.keep_alive_seconds.to_string()
            },
            profile.lifecycle.keep_alive_sql.clone(),
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
                    } else {
                        input
                    }
                })
            })
            .collect::<Vec<_>>();
        let parameters = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(5)
                .default_value(serde_json::to_string_pretty(&profile.parameters).unwrap())
        });
        self.form = Some(ProfileEditor {
            parameters,
            keep_connected: profile.lifecycle.keep_alive_seconds > 0,
            profile,
            fields,
            is_new,
            error: None,
            saving: None,
            confirm_delete: false,
        });
        self.open_profile_dialog(window, cx);
        cx.notify();
    }
    fn save_profile(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        if form.saving.is_some() {
            return;
        }
        let mut values: Vec<String> = form
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
        values[6] = form.parameters.read(cx).value().to_string();
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
            profile.lifecycle = connection_form::parse_lifecycle(
                &values[7..],
                form.keep_connected,
                &profile.lifecycle,
            )?;
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
            data.rows = (0..2250)
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

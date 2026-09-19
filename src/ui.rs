mod about_view;
mod button_pair;
mod connection_form;
mod output;
mod profile_view;
mod results;
mod setting_row;
mod settings_view;
mod tab_view;
mod workspace_view;
pub(crate) use workspace_view::WindowView;

use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants},
    dialog::DialogFooter,
    input::{EditorState, Input, InputEvent, InputState, TextareaState},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    table::TableState,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use qrow::{
    activity::{
        ActivityEvent, ActivityKind, ActivityLog, ExecutionId, Panel, PanelState, Severity,
    },
    model::{
        LINE_HEIGHT_STEP, MAX_EDITOR_FONT_SIZE, MAX_LINE_HEIGHT, MAX_TAB_TITLE, MAX_UI_SCALE,
        MIN_EDITOR_FONT_SIZE, MIN_LINE_HEIGHT, MIN_UI_SCALE, Profile, SavedTab, Settings,
        UI_SCALE_STEP, Workspace, copied_tab_title, unique_tab_title,
    },
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
        OpenAbout,
        OpenSettings,
        IncreaseUiScale,
        DecreaseUiScale,
        SaveConnection,
        RenameTab,
        Quit
    ]
);
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-enter", RunQuery, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-=", IncreaseUiScale, None),
        KeyBinding::new("cmd-+", IncreaseUiScale, None),
        KeyBinding::new("cmd--", DecreaseUiScale, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-enter", SaveConnection, Some("ConnectionSettings")),
        KeyBinding::new("cmd-enter", RenameTab, Some("TabSettings")),
    ]);
    cx.set_menus(vec![
        Menu {
            disabled: false,
            name: "Qrow".into(),
            items: vec![
                MenuItem::action("About Qrow", OpenAbout),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Qrow", Quit),
            ],
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
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
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
    // The last Run target can differ from the profile selected for the next Run.
    worker_profile: Option<Uuid>,
    busy: bool,
    cancelling: bool,
    connected: bool,
    more: bool,
    pending_page: Option<usize>,
    status: String,
    started: Option<Instant>,
    elapsed: Option<Duration>,
    output: ActivityLog,
    panel: PanelState,
    output_scroll: ScrollHandle,
    current_execution: Option<ExecutionId>,
    next_execution_id: u64,
}
impl Tab {
    fn can_disconnect(&self) -> bool {
        !self.busy
            && self.connected
            && self.worker_profile.is_some()
            && self.worker_profile == self.saved.profile
    }
}
struct ProfileEditor {
    profile: Profile,
    fields: Vec<Entity<InputState>>,
    parameters: Entity<TextareaState>,
    is_new: bool,
    error: Option<String>,
    saving: Option<mpsc::Receiver<Result<ProfileSave, String>>>,
    keep_connected: bool,
}
struct ProfileSave {
    profile: Profile,
    password_changed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileSaveAction {
    Keep,
    Update,
    Reconnect,
}

fn profile_save_action(
    previous: Option<&Profile>,
    updated: &Profile,
    password_changed: bool,
) -> ProfileSaveAction {
    let Some(previous) = previous else {
        return ProfileSaveAction::Keep;
    };
    if password_changed || !previous.connection_identity_eq(updated) {
        ProfileSaveAction::Reconnect
    } else if previous != updated {
        ProfileSaveAction::Update
    } else {
        ProfileSaveAction::Keep
    }
}
/// A tab keeps its identity while it is renamed, so the editor holds the tab's
/// id rather than its position in the bar.
struct TabEditor {
    tab: Uuid,
    title: Entity<InputState>,
    error: Option<String>,
}
/// Kit's `ContextMenu` wrapper cannot be handed back to `TabBar`, which accepts
/// `Tab` values only. Qrow therefore owns its context menus itself.
struct ContextMenu {
    position: Point<Pixels>,
    view: Entity<PopupMenu>,
    _subscription: Subscription,
}

/// Sidebar labels truncate long connection names without adding information.
const MAX_PROFILE_DISPLAY_NAME: usize = 40;

fn truncate_display_name(name: &str) -> String {
    let mut chars = name.chars();
    let truncated: String = chars.by_ref().take(MAX_PROFILE_DISPLAY_NAME).collect();
    if chars.next().is_none() {
        truncated
    } else {
        let mut display: String = name
            .chars()
            .take(MAX_PROFILE_DISPLAY_NAME.saturating_sub(1))
            .collect();
        display.push('…');
        display
    }
}

fn installed_fonts(cx: &App) -> Vec<String> {
    let mut fonts = cx.text_system().all_font_names();
    fonts.retain(|font| !font.is_empty());
    if !fonts.iter().any(|font| font == "Menlo") {
        fonts.insert(0, "Menlo".into());
    }
    fonts
}

fn apply_ui_theme(settings: &Settings, window: &mut Window, cx: &mut App) {
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.font_family = settings.ui_font_family.clone().into();
    theme.font_size = px(14. * settings.ui_scale);
    theme.mono_font_size = px(13. * settings.ui_scale);
    window.set_rem_size(theme.font_size);
    window.refresh();
}

fn panel_empty_state(message: &'static str, cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .font_family(cx.theme().font_family.clone())
        .text_size(rems(13. / 14.))
        .text_color(cx.theme().muted_foreground)
        .child(message)
}

pub struct Qrow {
    settings: Settings,
    fonts: Vec<String>,
    settings_open: bool,
    about_open: bool,
    settings_form: Option<settings_view::SettingsForm>,
    profiles: Vec<Profile>,
    tabs: Vec<Tab>,
    active: usize,
    active_tabs: BTreeMap<Uuid, Uuid>,
    form: Option<ProfileEditor>,
    tab_form: Option<TabEditor>,
    menu: Option<ContextMenu>,
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
    pending_quit: Option<(Workspace, storage::SaveReceipt)>,
    quit_confirmed: bool,
    finished: bool,
    wake: async_channel::Sender<()>,
}
impl Qrow {
    fn ui_px(&self, value: f32) -> Pixels {
        px(self.settings.ui_scale * value)
    }
    pub fn new(window: &mut Window, cx: &mut Context<Self>, demo: bool, started: Instant) -> Self {
        let (wake, notifications) = async_channel::bounded(1);
        let save_wake = wake.clone();
        let path = storage::workspace_path();
        let (mut workspace, mut message, saver) = if demo {
            (demo_workspace(), None, None)
        } else {
            match Saver::open(path, move || {
                let _ = save_wake.try_send(());
            }) {
                Ok((workspace, saver)) => (workspace, None, Some(saver)),
                Err(e) => (
                    Workspace::default(),
                    Some(format!(
                        "Cannot open workspace: {e}. Saving disabled to protect the existing file."
                    )),
                    None,
                ),
            }
        };
        let fonts = installed_fonts(cx);
        workspace.normalize();
        workspace.settings.sanitize();
        let mut unavailable_font = !fonts
            .iter()
            .any(|font| font == &workspace.settings.editor_font_family);
        if unavailable_font {
            workspace.settings.editor_font_family = Settings::default().editor_font_family;
            message.get_or_insert_with(|| {
                "The saved editor font is unavailable, so Qrow is using Menlo.".into()
            });
        }
        if !fonts
            .iter()
            .any(|font| font == &workspace.settings.logs_font_family)
        {
            workspace.settings.logs_font_family = Settings::default().logs_font_family;
            unavailable_font = true;
            let warning = "The saved Logs font is unavailable, so Qrow is using Menlo.";
            if let Some(message) = &mut message {
                message.push(' ');
                message.push_str(warning);
            } else {
                message = Some(warning.into());
            }
        }
        if workspace.settings.ui_font_family != Settings::default().ui_font_family
            && !fonts.contains(&workspace.settings.ui_font_family)
        {
            workspace.settings.ui_font_family = Settings::default().ui_font_family;
            unavailable_font = true;
            let warning =
                "The saved interface font is unavailable, so Qrow is using the system font.";
            if let Some(message) = &mut message {
                message.push(' ');
                message.push_str(warning);
            } else {
                message = Some(warning.into());
            }
        }
        let scale = workspace.settings.ui_scale;
        apply_ui_theme(&workspace.settings, window, cx);
        let quit = cx.on_app_quit(|this, cx| {
            this.finish(cx);
            async {}
        });
        let mut this = Self {
            settings: workspace.settings,
            fonts,
            settings_open: false,
            about_open: false,
            settings_form: None,
            profiles: workspace.profiles,
            tabs: vec![],
            active: workspace.active_tab,
            active_tabs: workspace.active_tabs.clone(),
            form: None,
            tab_form: None,
            menu: None,
            saver,
            dirty: (!demo && unavailable_font).then(Instant::now),
            message,
            demo,
            sidebar: true,
            sidebar_width: px(240. * scale),
            editor_height: px(285. * scale),
            resize: None,
            focus: cx.focus_handle(),
            _quit: quit,
            pending_quit: None,
            quit_confirmed: false,
            finished: false,
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
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| {
                this.request_quit(window, cx);
            })
            .is_err()
        });
        // Native termination may bypass our Quit action and the window close guard.
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
        let scale = self.settings.ui_scale;
        let table = cx.new(|cx| {
            let mut results = Results::default();
            results.set_ui_scale(scale);
            TableState::new(results, window, cx).col_selectable(false)
        });
        Tab {
            saved,
            input,
            table,
            _subscription: subscription,
            worker: None,
            worker_profile: None,
            busy: false,
            cancelling: false,
            connected: false,
            more: false,
            pending_page: None,
            status: "Not connected".into(),
            started: None,
            elapsed: None,
            output: ActivityLog::default(),
            panel: PanelState::default(),
            output_scroll: ScrollHandle::new(),
            current_execution: None,
            next_execution_id: 1,
        }
    }
    fn snapshot(&self, cx: &App) -> Workspace {
        Workspace {
            version: 2,
            settings: self.settings.clone(),
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
            active_tabs: self.active_tabs.clone(),
        }
    }
    fn finish(&mut self, cx: &App) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(mut saver) = self.saver.take() {
            let result = if self.quit_confirmed {
                saver.stop()
            } else {
                saver.finish(self.snapshot(cx))
            };
            if let Err(error) = result {
                eprintln!("Could not save workspace during native termination: {error:#}");
            }
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
    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_quit.is_some() || self.quit_confirmed {
            return;
        }
        if self.demo {
            self.quit_confirmed = true;
            cx.quit();
            return;
        }
        if self.form.as_ref().is_some_and(|form| form.saving.is_some()) {
            self.message =
                Some("Wait for the connection to finish saving, then quit again.".into());
            cx.notify();
            return;
        }
        let snapshot = self.snapshot(cx);
        let result = self
            .saver
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!("Workspace saving is disabled. Your edits have not been saved.")
            })
            .and_then(|saver| saver.flush(snapshot.clone()));
        match result {
            Ok(receipt) => {
                self.pending_quit = Some((snapshot, receipt));
                self.message = Some("Saving workspace before quitting…".into());
                let _ = self.wake.try_send(());
            }
            Err(error) => self.quit_failed(format!("{error:#}"), window, cx),
        }
        cx.notify();
    }

    fn quit_failed(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        self.message = Some(error.clone());
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let retry = weak.clone();
            let discard = weak.clone();
            alert
                .title("Could not save workspace")
                .description(format!(
                    "{error} Your edits are still available in this window."
                ))
                .width(px(560.))
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("quit-without-saving")
                                .label("Quit Without Saving")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, _, cx| {
                                    let _ = discard.update(cx, |this, cx| {
                                        this.quit_confirmed = true;
                                        cx.quit();
                                    });
                                }),
                        )
                        .child(Button::new("keep-editing").label("Keep Editing").on_click(
                            |_, window, cx| {
                                window.close_dialog(cx);
                            },
                        ))
                        .child(
                            Button::new("retry-save-and-quit")
                                .primary()
                                .label("Retry Save and Quit")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ =
                                        retry.update(cx, |this, cx| this.request_quit(window, cx));
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = Some(Instant::now());
        let _ = self.wake.try_send(());
        cx.notify();
    }
    fn record_activity(tab: &mut Tab, event: ActivityEvent) {
        let at_bottom =
            tab.output_scroll.offset().y <= -tab.output_scroll.max_offset().y + px(8. * 1.);
        tab.output.record(event);
        if at_bottom {
            tab.output_scroll.scroll_to_bottom();
        }
    }
    fn record_local_activity(
        tab: &mut Tab,
        severity: Severity,
        kind: ActivityKind,
        text: impl Into<String>,
    ) {
        Self::record_activity(
            tab,
            ActivityEvent::new(tab.current_execution, severity, kind, text),
        );
    }
    fn record_failure(tab: &mut Tab, active: bool) {
        tab.output_scroll.scroll_to_bottom();
        tab.panel.failure(active);
    }
    fn select_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.tabs[self.active].panel.user_select(panel);
        cx.notify();
    }
    fn allocate_execution_id(tab: &mut Tab) -> ExecutionId {
        let execution_id = ExecutionId(tab.next_execution_id);
        tab.next_execution_id = tab.next_execution_id.saturating_add(1);
        execution_id
    }
    fn clear_output(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        tab.output.clear();
        tab.output_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }
    fn copy_output(&self, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.tabs[self.active].output.copy_all(),
        ));
    }
    fn copy_output_error(&self, cx: &mut App) {
        if let Some(error) = self.tabs[self.active].output.copy_error() {
            cx.write_to_clipboard(ClipboardItem::new_string(error));
        }
    }
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let activities: Vec<_> = tab
                .worker
                .as_ref()
                .map(|worker| worker.activities.try_iter().collect())
                .unwrap_or_default();
            changed |= !activities.is_empty();
            for activity in activities {
                let error = activity.severity == Severity::Error;
                Self::record_activity(tab, activity);
                if error {
                    Self::record_failure(tab, index == self.active);
                }
            }
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
                        tab.connected = false;
                        tab.busy = true;
                        tab.status = "Connecting…".into();
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
                        tab.status = if self
                            .profiles
                            .iter()
                            .find(|profile| Some(profile.id) == tab.worker_profile)
                            .is_some_and(|profile| profile.lifecycle.keep_alive_seconds > 0)
                        {
                            "Connected · Keep-alive enabled"
                        } else {
                            "Connected"
                        }
                        .into();
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
                        let was_cancelling = tab.cancelling;
                        tab.more = more;
                        tab.pending_page = None;
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.elapsed = tab.started.take().map(|t| t.elapsed());
                        tab.status = if limited {
                            "Preview · Limit reached"
                        } else if more {
                            "Preview · More rows available"
                        } else {
                            "Complete"
                        }
                        .into();
                        if !was_cancelling {
                            tab.panel.success();
                        }
                    }
                    Event::Cancelled => {
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.more = false;
                        tab.pending_page = None;
                        tab.elapsed = tab.started.take().map(|t| t.elapsed());
                        tab.status = "Cancelled · Partial preview retained".into();
                    }
                    Event::Error {
                        message: _,
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
                            "Error · Connection lost"
                        } else {
                            "Error · Query failed"
                        }
                        .into();
                        Self::record_failure(tab, index == self.active);
                    }
                    Event::CancelError(message) => {
                        Self::record_local_activity(
                            tab,
                            Severity::Error,
                            ActivityKind::Error,
                            message,
                        );
                        Self::record_failure(tab, index == self.active);
                        tab.cancelling = false;
                    }
                    Event::Disconnected | Event::IdleDisconnected => {
                        tab.connected = false;
                        tab.more = false;
                        tab.pending_page = None;
                        tab.busy = false;
                        tab.cancelling = false;
                        tab.status = if matches!(event, Event::IdleDisconnected) {
                            "Disconnected · Idle timeout"
                        } else {
                            "Disconnected"
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
            if self.pending_quit.is_none()
                && let Some(saver) = &self.saver
                && let Err(error) = saver.save(self.snapshot(cx))
            {
                self.message = Some(format!("{error:#}"));
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
                Ok(saved) => {
                    let ProfileSave {
                        profile,
                        password_changed,
                    } = saved;
                    let id = profile.id;
                    let is_new = self.form.as_ref().is_some_and(|form| form.is_new);
                    let had_profiles = !self.profiles.is_empty();
                    let previous = self.profiles.iter().find(|p| p.id == id).cloned();
                    let action = profile_save_action(previous.as_ref(), &profile, password_changed);
                    if let Some(existing) = self.profiles.iter_mut().find(|p| p.id == id) {
                        *existing = profile.clone();
                    } else {
                        self.profiles.push(profile.clone());
                    }
                    if is_new {
                        if had_profiles {
                            let tab = self.make_tab(SavedTab::new(1, Some(id)), window, cx);
                            self.tabs.push(tab);
                            self.activate(self.tabs.len() - 1, window, cx);
                        } else {
                            for tab in &mut self.tabs {
                                tab.saved.profile = Some(id);
                            }
                            if let Some(index) = self
                                .tabs
                                .iter()
                                .position(|tab| tab.saved.profile == Some(id))
                            {
                                self.activate(index, window, cx);
                            }
                        }
                    }
                    if action == ProfileSaveAction::Reconnect {
                        for tab in &mut self.tabs {
                            if tab.worker_profile != Some(id) {
                                continue;
                            }
                            if let Some(worker) = tab.worker.take() {
                                worker.shutdown();
                            }
                            tab.worker_profile = None;
                            tab.connected = false;
                            tab.busy = false;
                            tab.cancelling = false;
                            tab.more = false;
                            tab.pending_page = None;
                            tab.status = "Not connected".into();
                        }
                    } else if action == ProfileSaveAction::Update {
                        for tab in &mut self.tabs {
                            if tab.worker_profile == Some(id) {
                                if let Some(worker) = &tab.worker {
                                    let _ = worker.update_profile(profile.clone());
                                }
                                if profile.lifecycle.keep_alive_seconds == 0
                                    && tab.status == "Connected · Keep-alive enabled"
                                {
                                    tab.status = "Connected".into();
                                }
                            }
                        }
                    }
                    if !is_new && self.tabs[self.active].saved.profile.is_none() {
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
        let quit_result =
            self.pending_quit
                .as_ref()
                .and_then(|(_, receipt)| match receipt.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                        "Workspace saver stopped before confirming the save".into(),
                    )),
                });
        if let Some(result) = quit_result {
            let (saved, _) = self.pending_quit.take().unwrap();
            match result {
                Ok(())
                    if saved == self.snapshot(cx)
                        && !self.form.as_ref().is_some_and(|f| f.saving.is_some()) =>
                {
                    self.quit_confirmed = true;
                    cx.quit();
                }
                Ok(()) => self.request_quit(window, cx),
                Err(error) => self.quit_failed(error, window, cx),
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
        self.dirty.is_some()
            || self.pending_quit.is_some()
            || self.form.as_ref().is_some_and(|f| f.saving.is_some())
    }
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        if let Some(profile) = self.tabs[index].saved.profile {
            self.active_tabs.insert(profile, self.tabs[index].saved.id);
        }
        self.tabs[index].panel.output_visible();
        self.tabs[index]
            .input
            .update(cx, |s, cx| s.focus(window, cx));
        self.changed(cx);
    }
    fn active_profile(&self) -> Option<Uuid> {
        self.tabs.get(self.active).and_then(|tab| tab.saved.profile)
    }
    fn visible_tab_indices(&self) -> Vec<usize> {
        let profile = self.active_profile();
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| (tab.saved.profile == profile).then_some(index))
            .collect()
    }
    fn active_tab_for_profile(&self, profile: Uuid) -> Option<usize> {
        self.active_tabs
            .get(&profile)
            .and_then(|id| self.tabs.iter().position(|tab| tab.saved.id == *id))
            .filter(|index| self.tabs[*index].saved.profile == Some(profile))
            .or_else(|| {
                self.tabs
                    .iter()
                    .position(|tab| tab.saved.profile == Some(profile))
            })
    }
    /// A modal owns the window, so tab and profile commands wait for it.
    fn dialog_open(&self) -> bool {
        self.form.is_some() || self.settings_open || self.about_open || self.tab_form.is_some()
    }
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        let profile = self.active_profile();
        if profile.is_none() {
            return;
        }
        let number = self
            .tabs
            .iter()
            .filter(|tab| tab.saved.profile == profile)
            .filter_map(|t| {
                t.saved
                    .title
                    .strip_prefix("Query ")
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let tab = self.make_tab(SavedTab::new(number, profile), window, cx);
        self.tabs.push(tab);
        self.activate(self.tabs.len() - 1, window, cx);
    }
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() || index >= self.tabs.len() || self.tabs[index].busy {
            return;
        }
        // The menu names a tab that is about to disappear.
        self.menu = None;
        if let Some(w) = &self.tabs[index].worker {
            w.shutdown();
        }
        let profile = self.tabs[index].saved.profile;
        self.tabs.remove(index);
        if let Some(profile) = profile
            && !self
                .tabs
                .iter()
                .any(|tab| tab.saved.profile == Some(profile))
        {
            let t = self.make_tab(SavedTab::new(1, Some(profile)), window, cx);
            self.tabs.push(t);
        }
        if self.tabs.is_empty() {
            let t = self.make_tab(
                SavedTab::new(1, self.profiles.first().map(|p| p.id)),
                window,
                cx,
            );
            self.tabs.push(t);
        }
        let old_active = self.active;
        let active = if index < old_active {
            old_active - 1
        } else if index > old_active {
            old_active
        } else {
            self.tabs
                .iter()
                .enumerate()
                .find(|(candidate, tab)| *candidate >= index && tab.saved.profile == profile)
                .map(|(candidate, _)| candidate)
                .or_else(|| {
                    self.tabs
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(candidate, tab)| *candidate < index && tab.saved.profile == profile)
                        .map(|(candidate, _)| candidate)
                })
                .or_else(|| self.tabs.len().checked_sub(1))
                .unwrap_or(0)
        };
        self.activate(active, window, cx);
    }
    fn switch_profile(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if !self.profiles.iter().any(|profile| profile.id == id) {
            return;
        }
        if let Some(index) = self.active_tab_for_profile(id) {
            if self.tabs[self.active].saved.profile != Some(id) {
                let connection_name = self
                    .profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .map(|profile| profile.name.clone())
                    .unwrap_or_else(|| "Unknown connection".into());
                Self::record_activity(
                    &mut self.tabs[self.active],
                    ActivityEvent::new(
                        None,
                        Severity::Info,
                        ActivityKind::ConnectionChanged,
                        format!("Selected connection: {connection_name}"),
                    ),
                );
            }
            self.activate(index, window, cx);
        }
    }
    fn run(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() || self.settings_open || self.tabs[self.active].busy {
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
            let message = e.to_string();
            Self::record_activity(
                tab,
                ActivityEvent::new(None, Severity::Error, ActivityKind::Error, message.clone()),
            );
            Self::record_failure(tab, true);
            tab.status = format!("Rejected · {message}");
            cx.notify();
            return;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|p| Some(p.id) == tab.saved.profile)
            .cloned()
        else {
            let message = "Choose a connection before running SQL.";
            Self::record_activity(
                tab,
                ActivityEvent::new(None, Severity::Error, ActivityKind::Error, message),
            );
            Self::record_failure(tab, true);
            tab.status = format!("Rejected · {message}");
            cx.notify();
            return;
        };
        if let Err(error) = profile.validate() {
            let message = error.to_string();
            Self::record_activity(
                tab,
                ActivityEvent::new(None, Severity::Error, ActivityKind::Error, message.clone()),
            );
            Self::record_failure(tab, true);
            tab.status = format!("Rejected · {message}");
            cx.notify();
            return;
        }
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
        tab.elapsed = None;
        tab.busy = true;
        tab.panel.execution_started();
        tab.cancelling = false;
        tab.started = Some(Instant::now());
        tab.status = "Preparing query…".into();
        let execution_id = Self::allocate_execution_id(tab);
        tab.worker_profile = Some(profile.id);
        tab.worker
            .as_ref()
            .unwrap()
            .run_with_id(profile.clone(), query.clone(), execution_id);
        tab.current_execution = Some(execution_id);
        let submission = ActivityEvent::new(
            Some(execution_id),
            Severity::Info,
            ActivityKind::Submitted,
            format!("Submitted query:\n{query}"),
        )
        .with_connection(profile.name)
        .with_sql(query);
        Self::record_activity(tab, submission);
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
        if t.busy && !t.cancelling && t.worker.is_some() {
            Self::record_local_activity(
                t,
                Severity::Info,
                ActivityKind::CancelRequested,
                "Cancellation requested by user",
            );
            t.worker.as_ref().unwrap().cancel();
            t.cancelling = true;
            t.status = "Cancelling…".into();
        }
        cx.notify();
    }
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        if !tab.can_disconnect() || self.form.is_some() || self.settings_open {
            return;
        }
        if tab.worker.is_some() {
            Self::record_activity(
                tab,
                ActivityEvent::new(
                    None,
                    Severity::Info,
                    ActivityKind::Disconnected,
                    "Disconnect requested",
                ),
            );
            tab.worker.as_ref().unwrap().disconnect();
            tab.busy = true;
            tab.status = "Disconnecting…".into();
        }
        cx.notify();
    }
    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dialog_open() {
            self.settings_open = true;
            self.init_settings_form(window, cx);
            self.open_settings_dialog(window, cx);
            cx.notify();
        }
    }
    fn open_about(&mut self, _: &OpenAbout, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dialog_open() {
            self.about_open = true;
            self.open_about_dialog(window, cx);
            cx.notify();
        }
    }
    fn apply_ui_scale(&mut self, scale: f32, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.settings.ui_scale;
        self.settings.ui_scale = scale;
        self.settings.sanitize();
        let scale = self.settings.ui_scale;
        if scale == previous {
            return;
        }
        let ratio = scale / previous;
        self.sidebar_width *= ratio;
        self.editor_height *= ratio;
        apply_ui_theme(&self.settings, window, cx);
        for tab in &self.tabs {
            tab.table.update(cx, |table, cx| {
                table.delegate_mut().set_ui_scale(scale);
                table.refresh(cx);
            });
        }
        self.changed(cx);
    }
    fn adjust_ui_scale(&mut self, change: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_ui_scale(self.settings.ui_scale + change, window, cx);
    }
    fn increase_ui_scale(
        &mut self,
        _: &IncreaseUiScale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_ui_scale(UI_SCALE_STEP, window, cx);
    }
    fn decrease_ui_scale(
        &mut self,
        _: &DecreaseUiScale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_ui_scale(-UI_SCALE_STEP, window, cx);
    }
    fn set_editor_font(&mut self, font: String, cx: &mut Context<Self>) {
        if self.fonts.iter().any(|available| available == &font)
            && self.settings.editor_font_family != font
        {
            self.settings.editor_font_family = font;
            self.changed(cx);
        }
    }
    fn set_logs_font(&mut self, font: String, cx: &mut Context<Self>) {
        if self.fonts.iter().any(|available| available == &font)
            && self.settings.logs_font_family != font
        {
            self.settings.logs_font_family = font;
            self.changed(cx);
        }
    }
    fn set_ui_font(&mut self, font: String, window: &mut Window, cx: &mut Context<Self>) {
        if (font == Settings::default().ui_font_family || self.fonts.contains(&font))
            && self.settings.ui_font_family != font
        {
            self.settings.ui_font_family = font;
            apply_ui_theme(&self.settings, window, cx);
            self.changed(cx);
        }
    }
    fn reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::default();
        self.settings.ui_font_family = settings.ui_font_family;
        self.settings.editor_font_family = settings.editor_font_family;
        self.settings.editor_font_size = settings.editor_font_size;
        self.settings.editor_line_height = settings.editor_line_height;
        self.settings.logs_font_family = settings.logs_font_family;
        self.settings.logs_font_size = settings.logs_font_size;
        self.settings.logs_line_height = settings.logs_line_height;
        self.apply_ui_scale(settings.ui_scale, window, cx);
        apply_ui_theme(&self.settings, window, cx);
        self.changed(cx);
    }
    /// Open a context menu at the pointer. Callers defer this from their right
    /// mouse down so an already open menu dismisses itself first.
    fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        items: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Ask the window rather than mirroring "a dialog is up" into a flag of
        // our own: a flag only stays true until some close path forgets to
        // clear it, and then no menu ever opens again.
        if self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        // Dismissing the menu returns focus to the SQL editor rather than the
        // row or tab that was clicked.
        let restore = self.tabs[self.active].input.read(cx).focus_handle(cx);
        let view = PopupMenu::build(window, cx, move |menu, window, cx| {
            items(menu.action_context(restore), window, cx)
        });
        let subscription = cx.subscribe(&view, |this, dismissed, _: &DismissEvent, cx| {
            // A replacement menu may already be open; only drop the one dismissed.
            if this
                .menu
                .as_ref()
                .is_some_and(|menu| menu.view.entity_id() == dismissed.entity_id())
            {
                this.menu = None;
                cx.notify();
            }
        });
        view.focus_handle(cx).focus(window, cx);
        self.menu = Some(ContextMenu {
            position,
            view,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn open_tab_menu(
        &mut self,
        tab: Uuid,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.iter().any(|t| t.saved.id == tab) {
            return;
        }
        let Some(source) = self.tabs.iter().find(|t| t.saved.id == tab) else {
            return;
        };
        let source_profile = source.saved.profile;
        let source_busy = source.busy;
        let edit =
            cx.listener(move |this, _: &ClickEvent, window, cx| this.edit_tab(tab, window, cx));
        let duplicate = cx.listener(move |this, _: &ClickEvent, window, cx| {
            if let Some(profile) = this
                .tabs
                .iter()
                .find(|t| t.saved.id == tab)
                .and_then(|t| t.saved.profile)
            {
                this.copy_tab(tab, profile, false, window, cx);
            }
        });
        let copy = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_tab_destination_picker(tab, false, window, cx);
        });
        let move_tab = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_tab_destination_picker(tab, true, window, cx);
        });
        let has_destinations = self
            .profiles
            .iter()
            .any(|profile| Some(profile.id) != source_profile);
        self.open_context_menu(
            position,
            move |menu, _, _| {
                menu.item(PopupMenuItem::new("Edit Tab…").on_click(edit))
                    .item(PopupMenuItem::new("Duplicate").on_click(duplicate))
                    .item(
                        PopupMenuItem::new("Copy to Connection…")
                            .on_click(copy)
                            .disabled(!has_destinations),
                    )
                    .item(
                        PopupMenuItem::new("Move to Connection…")
                            .on_click(move_tab)
                            .disabled(!has_destinations || source_busy),
                    )
            },
            window,
            cx,
        );
    }
    fn open_tab_destination_picker(
        &mut self,
        tab: Uuid,
        move_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.tabs.iter().find(|t| t.saved.id == tab) else {
            return;
        };
        if move_tab && source.busy {
            return;
        }
        let source_profile = source.saved.profile;
        let destinations: Vec<_> = self
            .profiles
            .iter()
            .filter(|profile| Some(profile.id) != source_profile)
            .map(|profile| (profile.id, profile.name.clone()))
            .collect();
        if destinations.is_empty() {
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let title = if move_tab {
                "Move tab to connection"
            } else {
                "Copy tab to connection"
            };
            let footer = destinations.iter().cloned().fold(
                DialogFooter::new().justify_end().child(
                    Button::new("cancel-tab-destination")
                        .label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
                |footer, (id, name)| {
                    let weak = weak.clone();
                    footer.child(
                        Button::new(SharedString::from(format!("tab-destination-{id}")))
                            .label(name)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.copy_tab(tab, id, move_tab, window, cx)
                                });
                                window.close_dialog(cx);
                            }),
                    )
                },
            );
            alert.title(title).footer(footer)
        });
    }
    fn copy_tab(
        &mut self,
        tab_id: Uuid,
        destination: Uuid,
        move_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source_index) = self.tabs.iter().position(|tab| tab.saved.id == tab_id) else {
            return;
        };
        if !self
            .profiles
            .iter()
            .any(|profile| profile.id == destination)
        {
            return;
        }
        if move_tab && self.tabs[source_index].busy {
            return;
        }
        let source_profile = self.tabs[source_index].saved.profile;
        let source_was_active =
            source_profile.is_some_and(|profile| self.active_tabs.get(&profile) == Some(&tab_id));
        let source_title = self.tabs[source_index].saved.title.clone();
        let title = if move_tab {
            unique_tab_title(&source_title, |candidate| {
                self.tabs.iter().enumerate().any(|(index, tab)| {
                    index != source_index
                        && tab.saved.profile == Some(destination)
                        && tab.saved.title == candidate
                })
            })
        } else {
            copied_tab_title(&source_title, |candidate| {
                self.tabs.iter().any(|tab| {
                    tab.saved.profile == Some(destination) && tab.saved.title == candidate
                })
            })
        };
        let mut saved = self.tabs[source_index].saved.clone();
        saved.profile = Some(destination);
        saved.title = title;
        saved.sql = self.tabs[source_index].input.read(cx).value().to_string();
        if move_tab {
            saved.id = tab_id;
            if let Some(worker) = self.tabs[source_index].worker.take() {
                worker.shutdown();
            }
            self.tabs.remove(source_index);
        } else {
            saved.id = Uuid::new_v4();
        }
        let new_tab = self.make_tab(saved, window, cx);
        self.tabs.push(new_tab);
        let new_index = self.tabs.len() - 1;
        if move_tab
            && let Some(source_profile) = source_profile
            && !self
                .tabs
                .iter()
                .any(|tab| tab.saved.profile == Some(source_profile))
        {
            let replacement = self.make_tab(SavedTab::new(1, Some(source_profile)), window, cx);
            let replacement_id = replacement.saved.id;
            self.tabs.push(replacement);
            self.active_tabs.insert(source_profile, replacement_id);
        } else if move_tab
            && source_was_active
            && let Some(source_profile) = source_profile
            && let Some(source_tab) = self
                .tabs
                .iter()
                .find(|tab| tab.saved.profile == Some(source_profile))
        {
            self.active_tabs.insert(source_profile, source_tab.saved.id);
        }
        self.activate(new_index, window, cx);
    }
    fn edit_tab(&mut self, tab: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        let Some(current) = self.tabs.iter().find(|t| t.saved.id == tab) else {
            return;
        };
        // The current name is the placeholder, so an untouched field keeps it.
        let title = current.saved.title.clone();
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(title));
        self.tab_form = Some(TabEditor {
            tab,
            title,
            error: None,
        });
        self.open_tab_dialog(window, cx);
        cx.notify();
    }
    fn rename_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.tab_form else {
            return;
        };
        let title = form.title.read(cx).value().trim().to_owned();
        if title.chars().count() > MAX_TAB_TITLE {
            if let Some(form) = &mut self.tab_form {
                form.error = Some(format!(
                    "Tab name must be {MAX_TAB_TITLE} characters or fewer."
                ));
            }
            cx.notify();
            return;
        }
        let tab = form.tab;
        if !title.is_empty()
            && let Some(profile) = self
                .tabs
                .iter()
                .find(|current| current.saved.id == tab)
                .and_then(|current| current.saved.profile)
            && self.tabs.iter().any(|current| {
                current.saved.id != tab
                    && current.saved.profile == Some(profile)
                    && current.saved.title == title
            })
        {
            if let Some(form) = &mut self.tab_form {
                form.error = Some("A tab with this name already exists on this connection.".into());
            }
            cx.notify();
            return;
        }
        if !title.is_empty()
            && let Some(current) = self.tabs.iter_mut().find(|t| t.saved.id == tab)
        {
            current.saved.title = title;
        }
        self.tab_form = None;
        // Programmatic close_dialog does not invoke Dialog::on_close.
        window.close_dialog(cx);
        self.changed(cx);
    }
    fn edit_profile(
        &mut self,
        profile: Profile,
        is_new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !is_new && self.profile_busy(profile.id) {
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
                            ""
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
            form.error = Some("Enter the LDAP password for this connection.".into());
            cx.notify();
            return;
        }
        let (send, recv) = mpsc::channel();
        form.saving = Some(recv);
        let _ = self.wake.try_send(());
        form.error = None;
        let demo = self.demo;
        let password_changed = !password.is_empty();
        std::thread::spawn(move || {
            let result = if !demo && !password.is_empty() {
                storage::set_password(profile.id, &password).map_err(|e| e.to_string())
            } else {
                Ok(())
            };
            let _ = send.send(result.map(|()| ProfileSave {
                profile,
                password_changed,
            }));
        });
        cx.notify();
    }
    fn confirm_delete_profile(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_busy(id) {
            return;
        }
        let Some(profile) = self.profiles.iter().find(|p| p.id == id) else {
            return;
        };
        let name = profile.name.clone();
        let display_name = truncate_display_name(&name);
        let tab_count = self
            .tabs
            .iter()
            .filter(|tab| tab.saved.profile == Some(id))
            .count();
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            alert
                .width(px(360.))
                .title(format!("Delete connection \"{display_name}\"?"))
                .description(format!(
                    "This deletes the connection, its {tab_count} query tab{} and all SQL in those tabs. This cannot be undone.",
                    if tab_count == 1 { "" } else { "s" }
                ))
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-delete-connection")
                                .label("Cancel")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("confirm-delete-connection")
                                .label("Delete connection")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, window, cx| {
                                    let _ = confirm.update(cx, |this, cx| {
                                        this.delete_profile(id, window, cx)
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
        cx.notify();
    }
    fn delete_profile(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_busy(id) || !self.profiles.iter().any(|p| p.id == id) {
            return;
        }
        let old_active_profile = self.active_profile();
        let replacement_profile = self
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .and_then(|index| {
                self.profiles
                    .get(index + 1)
                    .or_else(|| {
                        index
                            .checked_sub(1)
                            .and_then(|previous| self.profiles.get(previous))
                    })
                    .map(|profile| profile.id)
            });
        for tab in self
            .tabs
            .iter_mut()
            .filter(|tab| tab.saved.profile == Some(id))
        {
            if let Some(worker) = tab.worker.take() {
                worker.shutdown();
            }
        }
        self.tabs.retain(|tab| tab.saved.profile != Some(id));
        self.profiles.retain(|p| p.id != id);
        self.active_tabs.remove(&id);
        if self.tabs.is_empty() {
            let tab = self.make_tab(
                SavedTab::new(1, self.profiles.first().map(|profile| profile.id)),
                window,
                cx,
            );
            self.tabs.push(tab);
        }
        let target_profile = if old_active_profile == Some(id) {
            replacement_profile.filter(|profile| {
                self.profiles
                    .iter()
                    .any(|candidate| candidate.id == *profile)
            })
        } else {
            old_active_profile.filter(|profile| {
                self.profiles
                    .iter()
                    .any(|candidate| candidate.id == *profile)
            })
        };
        let active = target_profile
            .and_then(|profile| self.active_tab_for_profile(profile))
            .unwrap_or_else(|| self.active.min(self.tabs.len() - 1));
        self.activate(active, window, cx);
        // Keychain work must stay off the GPUI thread. A failure here can only
        // leave the secret behind, which is what the old behaviour did anyway.
        if !self.demo {
            std::thread::spawn(move || {
                let _ = storage::delete_password(id);
            });
        }
        self.form = None;
        self.changed(cx);
    }
    fn profile_busy(&self, id: Uuid) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.worker_profile == Some(id) && tab.busy)
    }
    fn seed_demo(&mut self, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        let execution_id = Self::allocate_execution_id(tab);
        let sql = tab.input.read(cx).value().to_string();
        tab.current_execution = Some(execution_id);
        Self::record_activity(
            tab,
            ActivityEvent::new(
                Some(execution_id),
                Severity::Info,
                ActivityKind::Connected,
                "Connected to rivendell-s (demo)",
            )
            .with_connection("rivendell-s"),
        );
        Self::record_activity(
            tab,
            ActivityEvent::new(
                Some(execution_id),
                Severity::Info,
                ActivityKind::Submitted,
                format!("Submitted query:\n{sql}"),
            )
            .with_connection("rivendell-s")
            .with_sql(sql),
        );
        Self::record_activity(
            tab,
            ActivityEvent::new(
                Some(execution_id),
                Severity::Info,
                ActivityKind::ExecutionCompleted,
                "Execution completed on the server, result set: true (demo)",
            ),
        );
        Self::record_activity(
            tab,
            ActivityEvent::new(
                Some(execution_id),
                Severity::Info,
                ActivityKind::FetchCompleted,
                "Fetched preview page 1: rows 1–2250, 2250 rows, 2250 retained, more rows: false (demo)",
            ),
        );
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
        tab.panel.success();
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
        version: 2,
        settings: Settings::default(),
        profiles: profiles.clone(),
        tabs: vec![tab, SavedTab::new(2, Some(profiles[1].id))],
        active_tab: 0,
        active_tabs: BTreeMap::new(),
    }
}

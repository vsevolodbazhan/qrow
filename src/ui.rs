mod about_view;
mod activity_view;
mod assistant_tools;
mod assistant_view;
mod button_pair;
mod catalog_tree;
mod connection_form;
mod dbt;
mod dbt_details;
mod demo;
mod environment;
mod output;
mod profile_view;
mod result_toolbar;
mod results;
mod setting_row;
mod settings_view;
mod sign_in_view;
mod status_dot;
mod status_tooltip;
mod tab_view;
mod workspace_view;
pub use crate::assets::Assets;
pub use assistant_view::CODEX_IDLE_TIMEOUT;
pub use environment::{Browser, Environment};
pub use workspace_view::WindowView;

use crate::themes;
use crate::{
    connector::DatabaseConnector,
    logs::{ExecutionId, LogEvent, LogHistory, LogKind, Panel, PanelState, Severity},
    model::{
        AssistantWorkspace, Authentication, CatalogRefresh, CatalogSettings, LINE_HEIGHT_STEP,
        MAX_EDITOR_FONT_SIZE, MAX_LINE_HEIGHT, MAX_TAB_TITLE, MAX_UI_SCALE, MIN_EDITOR_FONT_SIZE,
        MIN_LINE_HEIGHT, MIN_UI_SCALE, Profile, SYSTEM_FONT_FAMILY, SYSTEM_THEME, SavedTab,
        Settings, SharedCatalog, UI_SCALE_STEP, WORKSPACE_VERSION, Workspace,
        conversation_tab_title, copied_tab_title, unique_tab_title,
    },
    oidc, sql,
    storage::{self, Saver},
    worker::{Event, Worker},
};
use gpui_kit::component::{
    ActiveTheme, Disableable, IconName, Root, Sizable, Theme, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants},
    dialog::DialogFooter,
    highlighter::{LanguageConfig, LanguageRegistry},
    input::{EditorState, Input, InputEvent, InputState, TabSize, TextareaState},
    menu::{PopupMenu, PopupMenuItem},
    select::{SearchableVec, SelectEvent},
    table::TableState,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use results::Results;
use status_dot::DotStatus;
use status_tooltip::StatusTooltip;
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
        SendAssistantMessage,
        NewTab,
        CloseTab,
        ToggleSidebar,
        ShowConnections,
        ShowSignIns,
        ToggleAssistant,
        ToggleActivity,
        OpenAbout,
        OpenSettings,
        IncreaseUiScale,
        DecreaseUiScale,
        SaveConnection,
        SaveSignIn,
        SubmitRename,
        CopyCatalogName,
        InsertCatalogName,
        Quit
    ]
);
/// Initializes GPUI Kit, themes, the SQL language, key bindings, and menus.
/// Call once before opening a Qrow window.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    themes::init(cx);
    // Wide result sets need a persistent, discoverable horizontal scrollbar.
    Theme::set_scrollbar_mode(gpui_kit::component::scroll::ScrollbarMode::Always, cx);
    LanguageRegistry::singleton().register(
        "sql",
        &LanguageConfig::new(
            "sql",
            tree_sitter_sequel::LANGUAGE.into(),
            vec![],
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
    cx.bind_keys([
        KeyBinding::new("cmd-enter", RunQuery, None),
        KeyBinding::new(
            "cmd-enter",
            SendAssistantMessage,
            Some("AssistantComposer > Input"),
        ),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-b", ShowConnections, None),
        KeyBinding::new("cmd-j", ToggleAssistant, None),
        KeyBinding::new("cmd-shift-u", ToggleActivity, None),
        KeyBinding::new(
            "escape",
            activity_view::CloseActivity,
            Some(activity_view::CONTEXT),
        ),
        KeyBinding::new("cmd-=", IncreaseUiScale, None),
        KeyBinding::new("cmd-+", IncreaseUiScale, None),
        KeyBinding::new("cmd--", DecreaseUiScale, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-enter", SaveConnection, Some("ConnectionSettings")),
        KeyBinding::new("cmd-enter", SaveSignIn, Some("SignInSettings")),
        KeyBinding::new("cmd-enter", SubmitRename, Some("RenameDialog")),
        KeyBinding::new(
            "cmd-c",
            CopyCatalogName,
            Some(gpui_kit::base::tree_key_context()),
        ),
        KeyBinding::new(
            "shift-enter",
            InsertCatalogName,
            Some(gpui_kit::base::tree_key_context()),
        ),
    ]);
    set_menus(cx, false);
}

/// Wraps a Qrow view in its window shell and the GPUI Kit root.
pub fn root(qrow: Entity<Qrow>, window: &mut Window, cx: &mut Context<Root>) -> Root {
    let shell = cx.new(|_| WindowView::new(qrow));
    Root::new(shell, window, cx)
}

fn set_menus(cx: &mut App, assistant_enabled: bool) {
    let mut menus = vec![
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
                MenuItem::action("New query tab", NewTab),
                MenuItem::action("Close tab", CloseTab),
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
                    "Select all",
                    gpui_kit::component::input::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        },
        Menu {
            disabled: false,
            name: "Query".into(),
            items: vec![MenuItem::action("Run query", RunQuery)],
        },
    ];
    let mut view = vec![
        MenuItem::action("Connections", ShowConnections),
        MenuItem::action("Sign-Ins", ShowSignIns),
        MenuItem::action("Toggle sidebar", ToggleSidebar),
        MenuItem::separator(),
    ];
    if assistant_enabled {
        view.push(MenuItem::action("Toggle assistant", ToggleAssistant));
    }
    view.push(MenuItem::action("Activity", ToggleActivity));
    menus.push(Menu {
        disabled: false,
        name: "View".into(),
        items: view,
    });
    cx.set_menus(menus);
}
/// The panel of the left sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarPanel {
    Connections,
    SignIns,
}

struct Tab {
    saved: SavedTab,
    revision: u64,
    pending_assistant_edit: Option<String>,
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
    status_detail: Option<String>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
    output: LogHistory,
    panel: PanelState,
    output_scroll: ScrollHandle,
    current_execution: Option<ExecutionId>,
    next_execution_id: u64,
    /// The SQL of the last Run, for a run after a browser sign-in.
    submitted_sql: Option<String>,
    /// The query waits for this browser sign-in, and then runs.
    sign_in_wait: Option<sign_in_view::SignInWait>,
    cancelled_sign_in: Option<Uuid>,
    /// The query runs after an automatic sign-in, so a second sign-in
    /// error stays an error.
    signed_in_for_run: bool,
    /// A sign-out or another account released the session while the tab was
    /// busy. The next query closes it first.
    release_pending: bool,
}
impl Tab {
    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_detail = None;
    }

    fn set_status_detail(&mut self, status: &str, detail: impl Into<String>) {
        self.set_status(status);
        self.status_detail = Some(detail.into());
    }

    fn status_label(&self) -> String {
        match &self.status_detail {
            Some(detail) => format!("{}: {detail}", self.status),
            None => self.status.clone(),
        }
    }

    /// Show the tab name and dot state. A tab without a dot has the tooltip
    /// on the tab itself.
    fn status_tooltip(&self) -> StatusTooltip {
        StatusTooltip::new(
            self.saved.title.clone(),
            match self.dot_status() {
                Some(DotStatus::Connected) => "Idle",
                Some(DotStatus::Connecting) => "Connecting",
                Some(DotStatus::Working) => "Running",
                Some(DotStatus::Ready) => "Unread result",
                Some(DotStatus::Error) => "Unread error",
                Some(DotStatus::Attention) => unreachable!("SQL never needs approval"),
                None => "Not connected",
            },
        )
    }

    fn dot_status(&self) -> Option<DotStatus> {
        if self.panel.unread_error {
            Some(DotStatus::Error)
        } else if self.busy && !self.connected {
            Some(DotStatus::Connecting)
        } else if self.busy {
            Some(DotStatus::Working)
        } else if self.panel.has_unread_success() {
            Some(DotStatus::Ready)
        } else if self.connected {
            Some(DotStatus::Connected)
        } else {
            None
        }
    }

    fn status_suffix(&self) -> String {
        let mut label = String::new();
        if self.connected {
            label.push_str(if self.busy {
                ", connected"
            } else {
                ", connected, idle"
            });
        }
        label.push_str(self.work_suffix());
        if self.panel.unread_error {
            label.push_str(", unread error");
        }
        if self.panel.has_unread_success() {
            label.push_str(", unread query result");
        }
        label
    }

    fn work_suffix(&self) -> &'static str {
        if !self.busy {
            ""
        } else if self.connected {
            ", running"
        } else {
            ", connecting"
        }
    }

    fn can_disconnect(&self) -> bool {
        !self.busy
            && self.connected
            && self.worker_profile.is_some()
            && self.worker_profile == self.saved.profile
    }
}
struct ProfileEditor {
    database_type: connection_form::RowSelect,
    _database_type_subscription: Subscription,
    postgres_ssl_mode: connection_form::RowSelect,
    _postgres_ssl_mode_subscription: Subscription,
    profile: Profile,
    fields: Vec<Entity<InputState>>,
    parameters: Entity<TextareaState>,
    /// The assistant notes. A change shows the byte count near the limit.
    assistant_notes: Entity<TextareaState>,
    /// Whether the form shows the byte count of the notes.
    notes_counted: bool,
    _assistant_notes_subscription: Subscription,
    idle_behavior: connection_form::ChoiceSelect,
    _idle_behavior_subscription: Subscription,
    /// Manual or automatic schema refresh.
    schema_refresh: connection_form::ChoiceSelect,
    _schema_refresh_subscription: Subscription,
    column_reads: connection_form::RowSelect,
    /// The catalog that the connection uses. A change loads the settings of
    /// the chosen catalog into the Schemas fields.
    catalog_select: connection_form::RowCombobox,
    catalog_choices: Vec<(connection_form::CatalogChoice, String)>,
    /// The catalog whose settings the fields show. The dialog compares it
    /// with the list, because the list does not report each choice.
    catalog_choice: connection_form::CatalogChoice,
    /// The ID that a new shared catalog gets.
    new_catalog: Uuid,
    shared_name: Entity<InputState>,
    preferred_select: connection_form::RowSelect,
    preferred_choices: Vec<(Option<Uuid>, String)>,
    authentication: connection_form::AuthenticationSelect,
    /// The Sign-in list and its choices, in the order of the sign-ins.
    sign_in: connection_form::RowCombobox,
    sign_in_choices: Vec<(Uuid, String)>,
    tls: bool,
    _authentication_subscription: Subscription,
    _sign_in_subscription: Subscription,
    /// The dbt project fields.
    dbt: dbt::DbtForm,
    is_new: bool,
    error: Option<String>,
    saving: Option<mpsc::Receiver<Result<ProfileSave, String>>>,
}
struct ProfileSave {
    profile: Profile,
    /// The shared catalog that the connection uses, with its edited settings.
    shared: Option<SharedCatalog>,
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

fn font_available(font: &str, installed: &[String]) -> bool {
    font == SYSTEM_FONT_FAMILY || installed.iter().any(|available| available == font)
}

/// The SQL editor indents with spaces, so a formatted query and a typed indent
/// look the same.
fn editor_tab_size(settings: &Settings) -> TabSize {
    TabSize {
        tab_size: usize::from(settings.editor_tab_size),
        hard_tabs: false,
    }
}

fn apply_ui_theme(settings: &Settings, window: &mut Window, cx: &mut App) {
    let font_size = {
        let theme = gpui_kit::component::Theme::global_mut(cx);
        theme.font_family = settings.ui_font_family.clone().into();
        theme.font_size = px(14. * settings.ui_scale);
        theme.mono_font_size = px(13. * settings.ui_scale);
        theme.font_size
    };
    gpui_kit::component::Theme::sync_base(cx);
    window.set_rem_size(font_size);
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
    assistant: AssistantWorkspace,
    assistant_state: assistant_view::AssistantState,
    /// The assistant pane view. Qrow renders it beside the workspace.
    assistant_pane: Entity<assistant_view::AssistantPane>,
    fonts: Vec<String>,
    settings_open: bool,
    /// The page that Settings shows when it opens.
    about_open: bool,
    settings_form: Option<settings_view::SettingsForm>,
    profiles: Vec<Profile>,
    /// The schema catalogs that several connections share.
    shared_catalogs: Vec<SharedCatalog>,
    tabs: Vec<Tab>,
    active: usize,
    active_tabs: BTreeMap<Uuid, Uuid>,
    /// Scrolls the query tab strip to show the active tab.
    tab_scroll: ScrollHandle,
    form: Option<ProfileEditor>,
    tab_form: Option<TabEditor>,
    menu: Option<ContextMenu>,
    saver: Option<Saver>,
    dirty: Option<Instant>,
    message: Option<String>,
    demo: bool,
    credentials: Arc<dyn storage::Credentials>,
    /// Reusable sign-ins. The service owns their tokens and identities.
    sign_ins: Vec<crate::model::SignIn>,
    oidc: Arc<oidc::Service>,
    external_auth: Arc<crate::external_auth::Service>,
    sign_in_ui: sign_in_view::SignInState,
    connector: Arc<DatabaseConnector>,
    sidebar: bool,
    /// The panel that the sidebar shows, also while it is hidden.
    sidebar_panel: SidebarPanel,
    sidebar_width: Pixels,
    /// The focus inside the sidebar, which a hidden sidebar gives back.
    sidebar_focus: FocusHandle,
    editor_height: Pixels,
    resize: Option<(bool, Point<Pixels>, Pixels)>,
    focus: FocusHandle,
    _quit: Subscription,
    _appearance: Subscription,
    /// Gives the focus back when the focused element leaves the window.
    _focus_lost: Subscription,
    _assistant_submit: Subscription,
    pending_quit: Option<(Workspace, storage::SaveReceipt)>,
    quit_warning_open: bool,
    quit_work_confirmed: bool,
    quit_confirmed: bool,
    finished: bool,
    wake: async_channel::Sender<()>,
    catalog: catalog_tree::CatalogTree,
    _catalog_subscriptions: [Subscription; 2],
    /// The background work of each connection. It covers the main area of
    /// the window while it is open.
    activity: Entity<activity_view::ActivityView>,
    _activity_events: Subscription,
    /// The indexes of the dbt manifests of the connections.
    dbt: dbt::DbtProjects,
    /// The dbt resource of the open dbt details sheet.
    dbt_details: Option<Entity<dbt_details::DbtDetailsView>>,
    _demo_manifest: Option<tempfile::NamedTempFile>,
}
impl Qrow {
    fn ui_px(&self, value: f32) -> Pixels {
        px(self.settings.ui_scale * value)
    }
    pub fn new(
        environment: Environment,
        started: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (wake, notifications) = async_channel::bounded(1);
        let save_wake = wake.clone();
        let demo = environment.is_demo();
        let (demo_manifest, demo_error) = if demo {
            match demo::manifest() {
                Ok(manifest) => (Some(manifest), None),
                Err(error) => (
                    None,
                    Some(format!("Cannot open the demo dbt project: {error}")),
                ),
            }
        } else {
            (None, None)
        };
        let (mut workspace, mut message, saver) = if let Some(path) = environment.workspace() {
            let path = path.clone();
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
        } else {
            (demo_workspace(demo_manifest.as_ref()), demo_error, None)
        };
        let fonts = installed_fonts(cx);
        workspace.normalize();
        workspace.settings.sanitize();
        let mut unavailable_theme = false;
        if !themes::is_available(&workspace.settings.theme, cx) {
            workspace.settings.theme = Settings::default().theme;
            unavailable_theme = true;
            let warning = "The saved theme is unavailable, so Qrow is using System.";
            if let Some(message) = &mut message {
                message.push(' ');
                message.push_str(warning);
            } else {
                message = Some(warning.into());
            }
        }
        let mut unavailable_font = !font_available(&workspace.settings.editor_font_family, &fonts);
        if unavailable_font {
            workspace.settings.editor_font_family = Settings::default().editor_font_family;
            let warning = "The saved editor font is unavailable, so Qrow is using Menlo.";
            if let Some(message) = &mut message {
                message.push(' ');
                message.push_str(warning);
            } else {
                message = Some(warning.into());
            }
        }
        if !font_available(&workspace.settings.logs_font_family, &fonts) {
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
        if !font_available(&workspace.settings.assistant_font_family, &fonts) {
            workspace.settings.assistant_font_family = Settings::default().assistant_font_family;
            unavailable_font = true;
            let warning =
                "The saved assistant font is unavailable, so Qrow is using the system font.";
            if let Some(message) = &mut message {
                message.push(' ');
                message.push_str(warning);
            } else {
                message = Some(warning.into());
            }
        }
        if !font_available(&workspace.settings.ui_font_family, &fonts) {
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
        set_menus(cx, workspace.settings.assistant.enabled);
        themes::apply(&workspace.settings.theme, Some(window), cx);
        apply_ui_theme(&workspace.settings, window, cx);
        let quit = cx.on_app_quit(|this, cx| {
            this.finish(cx);
            async {}
        });
        let assistant_state = assistant_view::AssistantState::new(cx);
        let assistant_pane = {
            let qrow = cx.entity();
            cx.new(|cx| assistant_view::AssistantPane::new(&qrow, window, cx))
        };
        let composer = assistant_pane.read(cx).composer().clone();
        let assistant_submit = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send_assistant(window, cx);
                }
            },
        );
        let external_auth = Arc::new(crate::external_auth::Service::new(environment.browser()));
        external_auth.configure(&workspace.sign_ins, &workspace.profiles);
        let external_wake = wake.clone();
        external_auth.set_on_change(Arc::new(move || {
            let _ = external_wake.try_send(());
        }));
        let oidc = oidc::Service::new(environment.tokens(), environment.trust());
        oidc.configure(&workspace.sign_ins);
        let status_wake = wake.clone();
        oidc.set_on_change(Arc::new(move || {
            let _ = status_wake.try_send(());
        }));
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.settings.theme == SYSTEM_THEME {
                themes::apply(SYSTEM_THEME, Some(window), cx);
                apply_ui_theme(&this.settings, window, cx);
                cx.notify();
            }
        });
        // A list draws only its rows on the screen, and a closed part draws
        // no content, so a focused field can leave the window. The keys then
        // go to no element, and Escape and the close button of a sheet or a
        // dialog do nothing. The focus goes to the nearest element around
        // the field that is still in the window, for example the sheet. In
        // the workspace, like after a failed query hides the focused result
        // table, the focus goes to the SQL editor, so that typing and
        // shortcuts like ⌘B continue to operate.
        let focus_lost = cx.on_focus_lost(window, |this: &mut Self, window, cx| {
            if let Some(target) = window.focus_lost_restore_target(cx)
                && !target.contains(&this.focus, window)
            {
                window.focus(&target, cx);
            } else {
                let editor = this.tabs[this.active].input.clone();
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            }
        });
        let catalog = catalog_tree::CatalogTree::new(environment.workspace().cloned(), window, cx);
        let catalog_subscriptions = [
            cx.subscribe(
                &catalog.state,
                |this, _, event: &gpui_kit::base::TreeEvent, cx| {
                    this.catalog_expansion_changed(event, cx)
                },
            ),
            cx.subscribe(&catalog.search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.catalog_search_changed(cx);
                }
            }),
        ];
        let activity = cx.new(|cx| activity_view::ActivityView::new(window, cx));
        // The saved dbt indexes are next to the workspace, which its lock
        // protects. Without a saved workspace, they stay in memory.
        let dbt_directory = environment
            .workspace()
            .filter(|_| saver.is_some() && !demo)
            .map(|workspace| storage::dbt_directory(workspace));
        let dbt = dbt::DbtProjects::new(dbt_directory, wake.clone());
        let activity_events = cx.subscribe_in(&activity, window, |this, _, event, window, cx| {
            this.activity_event(event, window, cx)
        });
        let mut this = Self {
            settings: workspace.settings,
            assistant: workspace.assistant,
            assistant_state,
            assistant_pane,
            fonts,
            settings_open: false,
            about_open: false,
            settings_form: None,
            profiles: workspace.profiles,
            shared_catalogs: workspace.shared_catalogs,
            tabs: vec![],
            active: workspace.active_tab,
            active_tabs: workspace.active_tabs.clone(),
            tab_scroll: ScrollHandle::new(),
            form: None,
            tab_form: None,
            menu: None,
            saver,
            dirty: (!demo && (unavailable_font || unavailable_theme)).then(Instant::now),
            message,
            demo,
            credentials: environment.credentials(),
            sign_ins: workspace.sign_ins,
            oidc,
            external_auth,
            sign_in_ui: sign_in_view::SignInState::new(environment.browser()),
            connector: Arc::new(DatabaseConnector::new(environment.trust())),
            sidebar: true,
            sidebar_panel: SidebarPanel::Connections,
            sidebar_width: px(240. * scale),
            sidebar_focus: cx.focus_handle(),
            editor_height: px(285. * scale),
            resize: None,
            focus: cx.focus_handle(),
            _quit: quit,
            _appearance: appearance,
            _focus_lost: focus_lost,
            _assistant_submit: assistant_submit,
            pending_quit: None,
            quit_warning_open: false,
            quit_work_confirmed: false,
            quit_confirmed: false,
            finished: false,
            wake,
            catalog,
            _catalog_subscriptions: catalog_subscriptions,
            activity,
            _activity_events: activity_events,
            dbt,
            dbt_details: None,
            _demo_manifest: demo_manifest,
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
        this.sync_catalog_keys();
        // The worker loads the saved dbt indexes and starts its watches.
        this.sync_dbt();
        this.assistant_state.composer_target = this
            .tabs
            .get(this.active)
            .map(|tab| assistant_view::ComposerTarget::Tab(tab.saved.id));
        if demo {
            this.seed_demo(this.active, cx);
        }
        if demo {
            this.expand_demo_catalog();
        }
        this.rebuild_catalog_tree(cx);
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
        let tab_id = saved.id;
        let input = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .soft_wrap(false)
                .tab_size(editor_tab_size(&self.settings))
                .default_value(saved.sql.clone())
        });
        let subscription = cx.subscribe(&input, move |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.saved.id == tab_id) {
                    let current = tab.input.read(cx).value().to_string();
                    if tab.pending_assistant_edit.take().as_deref() != Some(current.as_str()) {
                        tab.revision = tab.revision.saturating_add(1);
                    }
                }
                this.changed(cx);
            }
        });
        let scale = self.settings.ui_scale;
        let table = cx.new(|cx| {
            let mut results = Results::default();
            results.set_ui_scale(scale, cx);
            TableState::new(results, window, cx).col_selectable(false)
        });
        Tab {
            saved,
            revision: 0,
            pending_assistant_edit: None,
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
            status_detail: None,
            started: None,
            elapsed: None,
            output: LogHistory::default(),
            panel: PanelState::default(),
            output_scroll: ScrollHandle::new(),
            current_execution: None,
            submitted_sql: None,
            sign_in_wait: None,
            cancelled_sign_in: None,
            signed_in_for_run: false,
            release_pending: false,
            next_execution_id: 1,
        }
    }
    fn snapshot(&self, cx: &App) -> Workspace {
        Workspace {
            version: WORKSPACE_VERSION,
            settings: self.settings.clone(),
            assistant: {
                let mut assistant = self.assistant.clone();
                assistant.remove_unstarted(&self.assistant_state.unstarted_threads);
                assistant
            },
            profiles: self.profiles.clone(),
            shared_catalogs: self.shared_catalogs.clone(),
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
            sign_ins: self.sign_ins.clone(),
        }
    }
    fn finish(&mut self, cx: &App) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.sign_in_ui.cancel_all();
        if self.demo {
            self.assistant_state.shutdown_demo(
                self.assistant
                    .conversations
                    .iter()
                    .map(|conversation| conversation.thread_id.clone())
                    .collect(),
            );
        } else {
            self.assistant_state.shutdown();
        }
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
        self.catalog.shutdown();
        let deadline = Instant::now() + Duration::from_secs(1);
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                worker.wait_for_shutdown(deadline.saturating_duration_since(Instant::now()));
            }
        }
        self.catalog.wait_for_shutdown(deadline);
    }
    fn active_work_description(&self) -> Option<&'static str> {
        let assistant = self.assistant_working();
        let query = self.tabs.iter().any(|tab| tab.busy);
        match (assistant, query) {
            (true, true) => {
                Some("An assistant turn and a query are still running. Quit now to stop both?")
            }
            (true, false) => Some("The assistant is still working. Quit now to stop the turn?"),
            (false, true) => Some("A query is still running. Quit now to stop it?"),
            (false, false) => None,
        }
    }

    fn confirm_quit_while_working(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_warning_open {
            return;
        }
        let Some(description) = self.active_work_description() else {
            return;
        };
        self.quit_warning_open = true;
        let keep_working = cx.weak_entity();
        let quit_anyway = cx.weak_entity();
        let dismiss = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let keep_working = keep_working.clone();
            let quit_anyway = quit_anyway.clone();
            let dismiss = dismiss.clone();
            alert
                .title("Work is still running")
                .description(description)
                .width(px(560.))
                .on_close(move |_, _, cx| {
                    let _ = dismiss.update(cx, |this, cx| {
                        this.quit_warning_open = false;
                        cx.notify();
                    });
                })
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("keep-working")
                                .primary()
                                .label("Keep working")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = keep_working.update(cx, |this, cx| {
                                        this.quit_warning_open = false;
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new("quit-anyway")
                                .label("Quit anyway")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = quit_anyway.update(cx, |this, cx| {
                                        this.quit_warning_open = false;
                                        this.quit_work_confirmed = true;
                                        this.request_quit(window, cx);
                                    });
                                }),
                        ),
                )
        });
        cx.notify();
    }

    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_quit.is_some() || self.quit_confirmed || self.quit_warning_open {
            return;
        }
        if self.form.as_ref().is_some_and(|form| form.saving.is_some()) {
            self.message =
                Some("Wait for the connection to finish saving, then quit again.".into());
            cx.notify();
            return;
        }
        if !self.quit_work_confirmed && self.active_work_description().is_some() {
            self.confirm_quit_while_working(window, cx);
            return;
        }
        if self.demo {
            self.quit_confirmed = true;
            cx.quit();
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
            let keep_editing = weak.clone();
            let dismiss = weak.clone();
            alert
                .title("Could not save workspace")
                .description(format!(
                    "{error} Your edits are still available in this window."
                ))
                .width(px(560.))
                .on_close(move |_, _, cx| {
                    let _ = dismiss.update(cx, |this, cx| {
                        this.quit_work_confirmed = false;
                        cx.notify();
                    });
                })
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("quit-without-saving")
                                .label("Quit without saving")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, _, cx| {
                                    let _ = discard.update(cx, |this, cx| {
                                        this.quit_confirmed = true;
                                        cx.quit();
                                    });
                                }),
                        )
                        .child(Button::new("keep-editing").label("Keep editing").on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                // A failed save cancels the earlier quit decision.
                                // Ask again if work is still active on the next quit.
                                let _ = keep_editing.update(cx, |this, cx| {
                                    this.quit_work_confirmed = false;
                                    cx.notify();
                                });
                            },
                        ))
                        .child(
                            Button::new("retry-save-and-quit")
                                .primary()
                                .label("Retry save and quit")
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
        self.external_auth.configure(&self.sign_ins, &self.profiles);
        self.dirty = Some(Instant::now());
        let _ = self.wake.try_send(());
        // The UI scale and the Logs font settings change here.
        self.sync_activity_typography(cx);
        cx.notify();
    }
    fn record_log(tab: &mut Tab, event: LogEvent) {
        let at_bottom =
            tab.output_scroll.offset().y <= -tab.output_scroll.max_offset().y + px(8. * 1.);
        tab.output.record(event);
        if at_bottom {
            tab.output_scroll.scroll_to_bottom();
        }
    }
    fn record_local_log(tab: &mut Tab, severity: Severity, kind: LogKind, text: impl Into<String>) {
        Self::record_log(
            tab,
            LogEvent::new(tab.current_execution, severity, kind, text),
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
    /// Applies what the workers, the assistant, the workspace saver, and a
    /// connection form save sent since the last tick. Returns whether a timed
    /// step still waits: an autosave, a quit, a connection form save, or a
    /// catalog tool call.
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut changed = self.drain_workers(cx);
        changed |= self.drain_catalogs(cx);
        self.sync_dbt();
        changed |= self.drain_dbt(cx);
        self.activity
            .update(cx, |activity, cx| activity.sync_labels(window, cx));
        changed |= self.tick_sign_ins(cx);
        changed |= self.tick_assistant(window, cx);
        // Catalog tool calls, also the calls that the assistant just made,
        // wait for the catalogs and for their deadline.
        let catalog_calls = self.resume_catalog_calls(cx);
        changed |= self.autosave(cx);
        changed |= self.finish_profile_save(window, cx);
        changed |= self.finish_quit(window, cx);
        self.sync_catalog_warmth();
        if changed {
            cx.notify();
        }
        self.dirty.is_some()
            || catalog_calls
            || self.pending_quit.is_some()
            || self.form.as_ref().is_some_and(|f| f.saving.is_some())
    }
    /// Applies the Logs entries and the events that the tab workers sent.
    /// Each entry also goes to the Activity of the connection of the tab.
    fn drain_workers(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let mut activity = Vec::new();
        let workspace_visible = !self.activity.read(cx).is_open();
        let mut waits = Vec::new();
        let mut sign_in_errors = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let events: Vec<_> = tab
                .worker
                .as_ref()
                .map(|w| w.events.try_iter().collect())
                .unwrap_or_default();
            // A query that needs a new sign-in never reached the server. Its
            // first such error opens the browser, and the query waits.
            let wait = events
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        Event::Error {
                            sign_in_required: true,
                            ..
                        }
                    )
                })
                .then(|| sign_in_view::automatic_sign_in(tab, &self.profiles))
                .flatten();
            let logs: Vec<_> = tab
                .worker
                .as_ref()
                .map(|worker| worker.logs.try_iter().collect())
                .unwrap_or_default();
            changed |= !logs.is_empty();
            let connection = tab.worker_profile.or(tab.saved.profile);
            for event in logs {
                if let Some(connection) = connection
                    && let Some(entry) =
                        crate::activity::from_tab(&event, tab.saved.id, &tab.saved.title)
                {
                    activity.push((connection, entry));
                }
                let Some(event) = crate::activity::tab_event(event) else {
                    continue;
                };
                // The sign-in error explains why the browser opens; the
                // query has not failed yet.
                let error = event.severity == Severity::Error && wait.is_none();
                Self::record_log(tab, event);
                if error {
                    Self::record_failure(tab, workspace_visible && index == self.active);
                }
            }
            changed |= !events.is_empty();
            // A query that waits for a sign-in keeps its tab busy and its
            // status while the session of the tab sends keep-alives or
            // closes.
            let waiting = tab
                .sign_in_wait
                .is_some()
                .then(|| (tab.status.clone(), tab.status_detail.clone()));
            for event in events {
                if let Event::Error { message, .. } = &event
                    && let Some(profile) = self
                        .profiles
                        .iter()
                        .find(|profile| Some(profile.id) == connection)
                    && let Authentication::TrinoExternal { sign_in } = profile.authentication
                    && (!tab.connected || self.external_auth.failed(profile))
                {
                    sign_in_errors.push((sign_in, profile.id, message.clone()));
                }
                if wait.is_some() && matches!(event, Event::Error { .. }) {
                    continue;
                }
                Self::apply_worker_event(
                    tab,
                    event,
                    workspace_visible && index == self.active,
                    &self.profiles,
                    cx,
                );
                if let Some((status, detail)) = &waiting {
                    tab.busy = true;
                    tab.status = status.clone();
                    tab.status_detail = detail.clone();
                }
            }
            if let Some(wait) = wait {
                waits.push((index, wait));
            }
        }
        for (connection, entry) in activity {
            self.record_activity(connection, entry, cx);
        }
        for (sign_in, connection, message) in sign_in_errors {
            self.record_sign_in_error(sign_in, connection, message, cx);
        }
        for (index, (sign_in, sql)) in waits {
            self.tabs[index].busy = false;
            self.wait_for_sign_in(index, sign_in, sql, cx);
        }
        changed
    }
    /// Applies one worker event to its tab. `active` is true for the shown tab.
    fn apply_worker_event(
        tab: &mut Tab,
        event: Event,
        active: bool,
        profiles: &[Profile],
        cx: &mut Context<Self>,
    ) {
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
                tab.set_status("Connecting…");
            }
            Event::Authenticating(waiting) => {
                if waiting {
                    tab.set_status("Waiting for browser sign-in…");
                } else if !tab.cancelling {
                    tab.set_status(if tab.connected {
                        "Executing…"
                    } else {
                        "Connecting…"
                    });
                }
            }
            Event::Connected => tab.connected = true,
            Event::Running => {
                tab.busy = true;
                tab.set_status("Executing…");
            }
            Event::KeepAliveStarted => {
                tab.busy = true;
                tab.set_status("Sending keep-alive…");
            }
            Event::KeepAliveFinished => {
                tab.busy = false;
                tab.cancelling = false;
                if profiles
                    .iter()
                    .find(|profile| Some(profile.id) == tab.worker_profile)
                    .is_some_and(|profile| profile.lifecycle.keep_alive_seconds > 0)
                {
                    tab.set_status_detail("Connected", "Keep-alive enabled");
                } else {
                    tab.set_status("Connected");
                }
            }
            Event::Columns(columns) => {
                tab.table.update(cx, |t, cx| {
                    t.delegate_mut().schema(columns, cx);
                    t.refresh(cx);
                });
                tab.set_status("Fetching preview…");
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
                if limited {
                    tab.set_status_detail("Preview", "Limit reached");
                } else if more {
                    tab.set_status_detail("Preview", "More rows available");
                } else {
                    tab.set_status("Complete");
                }
                if !was_cancelling {
                    tab.panel.success(active);
                }
            }
            Event::Cancelled => {
                tab.busy = false;
                tab.cancelling = false;
                tab.more = false;
                tab.pending_page = None;
                tab.elapsed = tab.started.take().map(|t| t.elapsed());
                tab.set_status_detail("Cancelled", "Partial preview retained");
            }
            Event::Error {
                message: _,
                disconnected,
                sign_in_required,
            } => {
                tab.busy = false;
                tab.cancelling = false;
                tab.more = false;
                tab.pending_page = None;
                // A session that never opened failed to connect; it was not lost.
                let was_connected = tab.connected;
                if disconnected {
                    tab.connected = false;
                }
                tab.elapsed = tab.started.take().map(|t| t.elapsed());
                if sign_in_required {
                    tab.set_status_detail("Error", "Sign-in required");
                } else if disconnected && !was_connected {
                    tab.set_status_detail("Error", "Connection failed");
                } else if disconnected {
                    tab.set_status_detail("Error", "Connection lost");
                } else {
                    tab.set_status_detail("Error", "Query failed");
                }
                Self::record_failure(tab, active);
            }
            Event::CancelError(message) => {
                Self::record_local_log(tab, Severity::Error, LogKind::Error, message);
                Self::record_failure(tab, active);
                tab.cancelling = false;
            }
            Event::Disconnected | Event::IdleDisconnected => {
                tab.connected = false;
                tab.more = false;
                tab.pending_page = None;
                tab.busy = false;
                tab.cancelling = false;
                if matches!(event, Event::IdleDisconnected) {
                    tab.set_status_detail("Disconnected", "Idle timeout");
                } else {
                    tab.set_status("Disconnected");
                }
            }
        }
    }
    /// Saves the workspace 400 ms after the last change, and shows the errors
    /// of earlier saves.
    fn autosave(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
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
        changed
    }
    /// Applies the result of a connection form save.
    fn finish_profile_save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let result = self
            .form
            .as_ref()
            .and_then(|f| f.saving.as_ref())
            .and_then(|r| r.try_recv().ok());
        let Some(result) = result else {
            return false;
        };
        match result {
            Ok(saved) => {
                let ProfileSave {
                    profile,
                    shared,
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
                if let Some(shared) = shared {
                    match self
                        .shared_catalogs
                        .iter_mut()
                        .find(|catalog| catalog.id == shared.id)
                    {
                        Some(existing) => *existing = shared,
                        None => self.shared_catalogs.push(shared),
                    }
                }
                crate::model::prune_shared_catalogs(&mut self.profiles, &mut self.shared_catalogs);
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
                        tab.set_status("Not connected");
                    }
                } else if action == ProfileSaveAction::Update {
                    for tab in &mut self.tabs {
                        if tab.worker_profile == Some(id) {
                            if let Some(worker) = &tab.worker {
                                let _ = worker.update_profile(profile.clone());
                            }
                            if profile.lifecycle.keep_alive_seconds == 0
                                && tab.status == "Connected"
                                && tab.status_detail.as_deref() == Some("Keep-alive enabled")
                            {
                                tab.set_status("Connected");
                            }
                        }
                    }
                }
                self.external_auth.configure(&self.sign_ins, &self.profiles);
                self.sync_catalogs(previous.as_ref(), cx);
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
        true
    }
    /// Quits after the final save, or reports a failed save.
    fn finish_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
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
        let Some(result) = quit_result else {
            return false;
        };
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
        true
    }
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(index, true, window, cx);
    }
    /// Shows the tab at `index`. With `focus_editor`, its editor takes the focus.
    fn show_tab(
        &mut self,
        index: usize,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        if let Some(profile) = self.tabs[index].saved.profile {
            self.active_tabs.insert(profile, self.tabs[index].saved.id);
        }
        // A new or selected tab can be outside the visible part of a full tab
        // strip. The scroll applies at the next layout, after the tab renders.
        if let Some(position) = self
            .visible_tab_indices()
            .iter()
            .position(|visible| *visible == index)
        {
            self.tab_scroll.scroll_to_item(position);
        }
        if !self.activity.read(cx).is_open() {
            self.tabs[index].panel.content_visible();
            if focus_editor {
                self.tabs[index]
                    .input
                    .update(cx, |s, cx| s.focus(window, cx));
            }
        } else {
            let focus = self.tabs[index].input.read(cx).focus_handle(cx);
            self.activity
                .update(cx, |view, _| view.return_focus_to(focus));
        }
        self.show_tab_conversation(window, cx);
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
        self.form.is_some()
            || self.settings_open
            || self.about_open
            || self.tab_form.is_some()
            || self.assistant_state.rename_form.is_some()
            || self.sign_in_ui.editor.is_some()
    }
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        let profile = self.active_profile();
        if profile.is_none() {
            return;
        }
        let index = self.add_tab(profile, window, cx);
        self.activate(index, window, cx);
    }
    /// Adds a blank tab with the next free number of the connection.
    fn add_tab(
        &mut self,
        profile: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
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
        self.tabs.len() - 1
    }
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open()
            || index >= self.tabs.len()
            || self.tabs[index].busy
            || self.assistant_tab_busy(self.tabs[index].saved.id)
        {
            return;
        }
        // The menu names a tab that is about to disappear.
        self.menu = None;
        if let Some(w) = &self.tabs[index].worker {
            w.shutdown();
        }
        let profile = self.tabs[index].saved.profile;
        let tab_id = self.tabs[index].saved.id;
        self.tabs.remove(index);
        self.sync_catalog_warmth();
        self.assistant_tab_removed(tab_id, profile);
        self.activity
            .update(cx, |activity, cx| activity.tab_closed(tab_id, cx));
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
            // A connection selection changes the visible tab group. It does
            // not change the owning connection of either tab, so it is not a
            // query Logs event. The tree keeps the focus.
            self.show_tab(index, false, window, cx);
            if let Some(profile) = self
                .profiles
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
            {
                self.authenticate_connection(profile, cx);
            }
        }
        self.select_catalog_connection(id, window, cx);
    }
    fn run(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.run_selected_query(window, cx);
    }
    fn run_selected_query(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.form.is_some() || self.settings_open {
            return false;
        }
        self.run_tab_query(self.active, window, cx)
    }
    /// Runs the selected SQL of a tab, or the statement at its cursor when nothing is
    /// selected.
    fn run_tab_query(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.tabs.get(index).is_none_or(|tab| tab.busy) {
            return false;
        }
        if self.demo {
            return self.run_tab_sql(index, String::new(), false, cx);
        }
        let tab = &mut self.tabs[index];
        let query = tab.input.update(cx, |s, cx| {
            let selected = s
                .selected_text_range(false, window, cx)
                .filter(|s| !s.range.is_empty());
            selected
                .and_then(|r| s.text_for_range(r.range, &mut None, window, cx))
                .map(Ok)
                .unwrap_or_else(|| {
                    let text = s.value();
                    sql::statement_range_at(&text, s.selected_range().start)
                        .map(|range| text[range].to_string())
                        .or_else(|| sql::statement_ranges(&text).is_empty().then(String::new))
                        .ok_or("Move the cursor into a SQL statement or select SQL to run.")
                })
        });
        let query = match query {
            Ok(query) => query,
            Err(message) => {
                let active = index == self.active && !self.activity.read(cx).is_open();
                Self::record_log(
                    tab,
                    LogEvent::new(None, Severity::Error, LogKind::Error, message),
                );
                Self::record_failure(tab, active);
                tab.set_status_detail("Rejected", message);
                cx.notify();
                return false;
            }
        };
        self.run_tab_sql(index, query, false, cx)
    }
    fn tab_database_type(&self, profile: Option<Uuid>) -> crate::model::DatabaseType {
        self.profiles
            .iter()
            .find(|candidate| Some(candidate.id) == profile)
            .map(|profile| profile.database_type)
            .unwrap_or_default()
    }

    /// Runs `query` in the tab `index`. A connection whose sign-in needs the
    /// browser waits for it first, unless the query runs `after_sign_in`.
    fn run_tab_sql(
        &mut self,
        index: usize,
        query: String,
        after_sign_in: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tabs.get(index).is_none_or(|tab| tab.busy) {
            return false;
        }
        if self.demo {
            self.seed_demo(index, cx);
            cx.notify();
            return true;
        }
        let active = index == self.active && !self.activity.read(cx).is_open();
        let database_type = self.tab_database_type(self.tabs[index].saved.profile);
        let tab = &mut self.tabs[index];
        if let Err(e) = sql::validate_single_for(&query, database_type) {
            let message = e.to_string();
            Self::record_log(
                tab,
                LogEvent::new(None, Severity::Error, LogKind::Error, message.clone()),
            );
            Self::record_failure(tab, active);
            tab.set_status_detail("Rejected", message);
            cx.notify();
            return false;
        }
        let Some(profile) = self
            .profiles
            .iter()
            .find(|p| Some(p.id) == tab.saved.profile)
            .cloned()
        else {
            let message = "Choose a connection before running SQL.";
            Self::record_log(
                tab,
                LogEvent::new(None, Severity::Error, LogKind::Error, message),
            );
            Self::record_failure(tab, active);
            tab.set_status_detail("Rejected", message);
            cx.notify();
            return false;
        };
        if let Err(error) = profile.validate() {
            let message = error.to_string();
            Self::record_log(
                tab,
                LogEvent::new(None, Severity::Error, LogKind::Error, message.clone()),
            );
            Self::record_failure(tab, active);
            tab.set_status_detail("Rejected", message);
            cx.notify();
            return false;
        }
        // An open session of the connection continues without a new token,
        // but not while a browser sign-in or a sign-out can change the
        // account of the sign-in.
        let live = tab.connected && tab.worker_profile == Some(profile.id) && !tab.release_pending;
        if !after_sign_in
            && matches!(profile.authentication, Authentication::Oidc { .. })
            && let Some(sign_in) = profile.authentication.sign_in()
            && (self.sign_in_changing(sign_in) || !live && self.sign_in_needs_browser(sign_in))
        {
            self.wait_for_sign_in(index, sign_in, query, cx);
            return true;
        }
        if let Authentication::TrinoExternal { sign_in } = profile.authentication {
            self.sign_in_ui.clear_connection_error(sign_in, profile.id);
        }
        let tab = &mut self.tabs[index];
        tab.signed_in_for_run = after_sign_in;
        tab.submitted_sql = Some(query.clone());
        // A session that a sign-out or another account released while the
        // tab was busy closes now, so the query opens a new one.
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
            let credentials = self.credential_provider();
            let tab = &mut self.tabs[index];
            tab.worker = Some(Worker::with_connector(
                Arc::new(move || {
                    let _ = wake.try_send(());
                }),
                self.connector.clone(),
                credentials,
            ));
        }
        let tab = &mut self.tabs[index];
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
        tab.set_status("Preparing query…");
        let execution_id = Self::allocate_execution_id(tab);
        tab.worker_profile = Some(profile.id);
        tab.worker
            .as_ref()
            .unwrap()
            .run_with_id(profile.clone(), query.clone(), execution_id);
        tab.current_execution = Some(execution_id);
        let submission = LogEvent::new(
            Some(execution_id),
            Severity::Info,
            LogKind::Submitted,
            format!("Submitted query:\n{query}"),
        )
        .with_connection(profile.name)
        .with_sql(query);
        let entry = crate::activity::from_tab(&submission, tab.saved.id, &tab.saved.title);
        Self::record_log(tab, submission);
        if let Some(entry) = entry {
            self.record_activity(profile.id, entry, cx);
        }
        cx.notify();
        true
    }
    fn next_page(&mut self, cx: &mut Context<Self>) {
        self.next_tab_page(self.active, cx);
    }
    fn next_tab_page(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
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
            worker.more();
            tab.set_status("Fetching next page…");
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
        self.cancel_tab(self.active, cx);
    }
    fn cancel_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.tabs[index].sign_in_wait.is_some() {
            self.cancel_sign_in_wait(index, cx);
            return;
        }
        let t = &mut self.tabs[index];
        if t.busy && !t.cancelling && t.worker.is_some() {
            Self::record_local_log(
                t,
                Severity::Info,
                LogKind::CancelRequested,
                "Cancellation requested by user",
            );
            t.worker.as_ref().unwrap().cancel();
            t.cancelling = true;
            t.set_status("Cancelling…");
        }
        cx.notify();
    }
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.disconnect_tab(self.active, cx);
    }
    fn disconnect_profile(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.profile_busy(id) || self.dialog_open() {
            return;
        }
        for index in 0..self.tabs.len() {
            if self.tabs[index].saved.profile == Some(id) {
                self.disconnect_tab(index, cx);
            }
        }
    }
    fn disconnect_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
        if !tab.can_disconnect() || self.form.is_some() || self.settings_open {
            return;
        }
        if tab.worker.is_some() {
            let event = LogEvent::new(
                None,
                Severity::Info,
                LogKind::Disconnected,
                "Disconnect requested",
            );
            let entry = crate::activity::from_tab(&event, tab.saved.id, &tab.saved.title);
            let connection = tab.worker_profile;
            Self::record_log(tab, event);
            tab.worker.as_ref().unwrap().disconnect();
            tab.busy = true;
            tab.set_status("Disconnecting…");
            if let (Some(connection), Some(entry)) = (connection, entry) {
                self.record_activity(connection, entry, cx);
            }
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
    fn set_theme(&mut self, theme: String, window: &mut Window, cx: &mut Context<Self>) {
        if theme == self.settings.theme || !themes::is_available(&theme, cx) {
            return;
        }
        self.settings.theme = theme;
        themes::apply(&self.settings.theme, Some(window), cx);
        apply_ui_theme(&self.settings, window, cx);
        self.changed(cx);
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
        self.resize_result_columns(cx);
        self.changed(cx);
    }
    /// Size result columns again for the current interface scale and font.
    fn resize_result_columns(&self, cx: &mut Context<Self>) {
        let scale = self.settings.ui_scale;
        for tab in &self.tabs {
            tab.table.update(cx, |table, cx| {
                table.delegate_mut().set_ui_scale(scale, cx);
                table.refresh(cx);
            });
        }
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
    fn set_editor_tab_size(&mut self, size: u8, cx: &mut Context<Self>) {
        if self.settings.editor_tab_size == size {
            return;
        }
        self.settings.editor_tab_size = size;
        let tab = editor_tab_size(&self.settings);
        for editor in self.tabs.iter().map(|tab| tab.input.clone()) {
            editor.update(cx, |editor, cx| editor.set_tab_size(tab, cx));
        }
        self.changed(cx);
    }
    fn set_editor_font(&mut self, font: String, cx: &mut Context<Self>) {
        if font_available(&font, &self.fonts) && self.settings.editor_font_family != font {
            self.settings.editor_font_family = font;
            self.changed(cx);
        }
    }
    fn set_logs_font(&mut self, font: String, cx: &mut Context<Self>) {
        if font_available(&font, &self.fonts) && self.settings.logs_font_family != font {
            self.settings.logs_font_family = font;
            self.changed(cx);
        }
    }
    fn set_assistant_font(&mut self, font: String, cx: &mut Context<Self>) {
        if font_available(&font, &self.fonts) && self.settings.assistant_font_family != font {
            self.settings.assistant_font_family = font;
            self.changed(cx);
        }
    }
    fn set_ui_font(&mut self, font: String, window: &mut Window, cx: &mut Context<Self>) {
        if font_available(&font, &self.fonts) && self.settings.ui_font_family != font {
            self.settings.ui_font_family = font;
            apply_ui_theme(&self.settings, window, cx);
            self.resize_result_columns(cx);
            self.changed(cx);
        }
    }
    fn reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::default();
        self.settings.theme = settings.theme;
        let font_changed = self.settings.ui_font_family != settings.ui_font_family;
        self.settings.ui_font_family = settings.ui_font_family;
        self.settings.editor_font_family = settings.editor_font_family;
        self.settings.editor_font_size = settings.editor_font_size;
        self.settings.editor_line_height = settings.editor_line_height;
        self.set_editor_tab_size(settings.editor_tab_size, cx);
        self.settings.assistant.sql_keyword_case = settings.assistant.sql_keyword_case;
        self.settings.logs_font_family = settings.logs_font_family;
        self.settings.logs_font_size = settings.logs_font_size;
        self.settings.logs_line_height = settings.logs_line_height;
        self.settings.assistant_font_family = settings.assistant_font_family;
        self.settings.assistant_font_size = settings.assistant_font_size;
        self.settings.assistant_line_height = settings.assistant_line_height;
        self.apply_ui_scale(settings.ui_scale, window, cx);
        themes::apply(&self.settings.theme, Some(window), cx);
        apply_ui_theme(&self.settings, window, cx);
        if font_changed {
            self.resize_result_columns(cx);
        }
        self.changed(cx);
    }
    /// Open a context menu at the pointer. Callers defer this from their right
    /// mouse down so an already open menu dismisses itself first.
    pub(crate) fn open_context_menu(
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
        let source_busy = source.busy || self.assistant_tab_busy(tab);
        // A tab has at most one conversation.
        let conversation = self
            .settings
            .assistant
            .enabled
            .then(|| self.tab_assistant_status(tab).is_none());
        let start_conversation = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.start_tab_conversation(tab, window, cx)
        });
        let rename = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_tab_rename(tab, window, cx)
        });
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
        let destinations: Vec<_> = self
            .profiles
            .iter()
            .filter(|profile| Some(profile.id) != source_profile)
            .map(|profile| (profile.id, profile.name.clone()))
            .collect();
        let weak = cx.weak_entity();
        self.open_context_menu(
            position,
            move |menu, window, cx| {
                let menu = menu
                    .item(PopupMenuItem::new("Rename…").on_click(rename))
                    .item(PopupMenuItem::new("Duplicate").on_click(duplicate));
                let menu = if destinations.is_empty() {
                    menu.item(PopupMenuItem::new("Copy to connection…").disabled(true))
                } else {
                    let destinations = destinations.clone();
                    let weak = weak.clone();
                    menu.submenu("Copy to connection…", window, cx, move |menu, _, _| {
                        destinations.iter().cloned().fold(
                            menu.scrollable(true),
                            |menu, (id, name)| {
                                let weak = weak.clone();
                                menu.item(PopupMenuItem::new(name).on_click(
                                    move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.copy_tab(tab, id, false, window, cx)
                                        });
                                    },
                                ))
                            },
                        )
                    })
                };
                let menu = if destinations.is_empty() || source_busy {
                    menu.item(PopupMenuItem::new("Move to connection…").disabled(true))
                } else {
                    let weak = weak.clone();
                    menu.submenu("Move to connection…", window, cx, move |menu, _, _| {
                        destinations.iter().cloned().fold(
                            menu.scrollable(true),
                            |menu, (id, name)| {
                                let weak = weak.clone();
                                menu.item(PopupMenuItem::new(name).on_click(
                                    move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.copy_tab(tab, id, true, window, cx)
                                        });
                                    },
                                ))
                            },
                        )
                    })
                };
                match conversation {
                    Some(available) => menu.separator().item(
                        PopupMenuItem::new("Start conversation")
                            .on_click(start_conversation)
                            .disabled(!available),
                    ),
                    None => menu,
                }
            },
            window,
            cx,
        );
    }
    /// The menu of a connection row. Qrow owns it like the tab menu; GPUI Kit's
    /// `context_menu` keeps each dismissed menu alive through a reference cycle.
    pub(crate) fn open_profile_menu(
        &mut self,
        id: Uuid,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
        else {
            return;
        };
        let busy = self.profile_busy(id);
        let in_use = self.profile_in_use(id);
        let disconnect_enabled = !busy
            && self
                .tabs
                .iter()
                .any(|tab| tab.saved.profile == Some(id) && tab.can_disconnect());
        let disconnect =
            cx.listener(move |this, _: &ClickEvent, _, cx| this.disconnect_profile(id, cx));
        let has_dbt = profile.dbt.is_some();
        let dbt_parsing = self.dbt_parsing(id);
        let edited = profile.clone();
        let edit = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.edit_profile(edited.clone(), false, window, cx)
        });
        let duplicate = cx.listener(move |this, _: &ClickEvent, window, cx| {
            let mut profile = profile.clone();
            profile.id = Uuid::new_v4();
            profile.name = crate::model::copied_profile_name(&profile.name, |name| {
                this.profiles.iter().any(|existing| existing.name == name)
            });
            this.edit_profile(profile, true, window, cx);
        });
        let delete = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.confirm_delete_profile(id, window, cx)
        });
        let show_activity = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_activity(Some(id), window, cx)
        });
        let browses = self
            .profiles
            .iter()
            .any(|profile| profile.id == id && profile.catalog.browses());
        let has_expanded = self.has_expanded_descendants(id, None, cx);
        let collapse =
            cx.listener(move |this, _: &ClickEvent, _, cx| this.collapse_catalog(id, None, cx));

        let refresh_dbt = cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.refresh_dbt(id);
            cx.notify();
        });
        let refreshing = self.catalog.is_refreshing(id);
        let refresh = cx.listener(move |this, _: &ClickEvent, _, cx| {
            if refreshing {
                this.cancel_catalog_refresh(id);
            } else {
                this.refresh_catalog(id, crate::catalog::Scope::Connection, cx);
            }
        });
        self.open_context_menu(
            position,
            move |menu, _, _| {
                // Labeled sections: what the row is, then what it contains.
                menu.item(menu_section("Connection"))
                    .item(
                        PopupMenuItem::new("Disconnect")
                            .on_click(disconnect)
                            .disabled(!disconnect_enabled),
                    )
                    .item(PopupMenuItem::new("Edit").on_click(edit).disabled(busy))
                    .item(PopupMenuItem::new("Duplicate").on_click(duplicate))
                    .item(PopupMenuItem::new("Show activity").on_click(show_activity))
                    .item(
                        PopupMenuItem::new("Delete")
                            .on_click(delete)
                            .disabled(in_use),
                    )
                    .when(browses, |menu| {
                        menu.separator()
                            .item(menu_section("Schemas"))
                            .item(
                                PopupMenuItem::new(if refreshing {
                                    "Stop refresh"
                                } else {
                                    "Refresh"
                                })
                                .on_click(refresh),
                            )
                            .item(
                                PopupMenuItem::new("Collapse")
                                    .on_click(collapse)
                                    .disabled(!has_expanded),
                            )
                    })
                    .when(has_dbt, |menu| {
                        menu.separator().item(menu_section("dbt")).item(
                            PopupMenuItem::new("Refresh manifest")
                                .on_click(refresh_dbt)
                                .disabled(dbt_parsing),
                        )
                    })
            },
            window,
            cx,
        );
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
        if move_tab && (self.tabs[source_index].busy || self.assistant_tab_busy(tab_id)) {
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
    fn open_tab_rename(&mut self, tab: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        let Some(current_title) = self
            .tabs
            .iter()
            .find(|current| current.saved.id == tab)
            .map(|current| current.saved.title.clone())
        else {
            return;
        };
        let title = cx.new(|cx| InputState::new(window, cx).default_value(current_title));
        self.tab_form = Some(TabEditor {
            tab,
            title,
            error: None,
        });
        self.open_rename_dialog(tab_view::TAB_RENAME, window, cx);
        cx.notify();
    }
    pub(super) fn rename_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            if let Some(conversation) = self
                .assistant
                .conversations
                .iter_mut()
                .find(|conversation| conversation.tab_id == Some(tab))
            {
                conversation.title_follows_conversation = false;
            }
            self.assistant_state.new_conversation_tabs.remove(&tab);
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
        let shared = profile.shared_catalog.and_then(|id| {
            self.shared_catalogs
                .iter()
                .find(|catalog| catalog.id == id)
                .cloned()
        });
        // A member shows the settings of its shared catalog, also while it
        // does not browse schemas.
        let settings = match &shared {
            Some(shared) => CatalogSettings {
                refresh: if profile.catalog.browses() {
                    shared.settings.refresh
                } else {
                    CatalogRefresh::Disabled
                },
                ..shared.settings.clone()
            },
            None => profile.catalog.clone(),
        };
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
            settings.include.join(", "),
            settings.exclude.join(", "),
            settings.refresh_minutes.to_string(),
            settings.timeout_minutes.to_string(),
            profile.lifecycle.response_timeout_seconds.to_string(),
            profile.trino_schema.clone(),
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
            // The field is as tall as its text, so a short object has no
            // empty lines below its closing brace.
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .default_value(serde_json::to_string_pretty(&profile.parameters).unwrap())
        });
        let assistant_notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 12)
                .default_value(profile.assistant_notes.clone())
        });
        let assistant_notes_subscription = cx.subscribe_in(
            &assistant_notes,
            window,
            |this, notes, event: &InputEvent, _, cx| {
                // The form renders again only while it shows the byte count.
                let counted = connection_form::counts_notes(notes.read(cx).value().len());
                if let (InputEvent::Change, Some(form)) = (event, &mut this.form)
                    && (counted || form.notes_counted)
                {
                    form.notes_counted = counted;
                    cx.notify();
                }
            },
        );
        let keep_connected = profile.lifecycle.keep_alive_seconds > 0;
        let idle_behavior = connection_form::idle_behavior_select(keep_connected, window, cx);
        let schema_refresh = connection_form::schema_refresh_select(
            connection_form::RefreshMode::of(settings.refresh),
            window,
            cx,
        );
        let column_reads = connection_form::choice_select(
            &connection_form::column_read_choices(),
            &profile.catalog_column_reads,
            window,
            cx,
        );
        let catalog_choices = connection_form::catalog_choices(&self.shared_catalogs);
        let catalog = match &shared {
            Some(shared) => connection_form::CatalogChoice::Shared(shared.id),
            None => connection_form::CatalogChoice::Private,
        };
        let catalog_select =
            connection_form::catalog_combobox(&catalog_choices, &catalog, window, cx);
        let shared_name = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                shared
                    .as_ref()
                    .map(|shared| shared.name.clone())
                    .unwrap_or_default(),
            )
        });
        let preferred_choices =
            connection_form::preferred_choices(&profile, catalog, &self.profiles);
        let preferred = shared.as_ref().and_then(|shared| shared.preferred);
        let preferred_select =
            connection_form::choice_select(&preferred_choices, &preferred, window, cx);
        // The choice decides which Schemas fields show.
        let schema_refresh_subscription =
            cx.subscribe_in(&schema_refresh, window, |_this, _, event, _, cx| {
                if matches!(event, SelectEvent::Confirm(Some(_))) {
                    cx.notify();
                }
            });
        let idle_behavior_subscription =
            cx.subscribe_in(&idle_behavior, window, |_this, _, event, _, cx| {
                if connection_form::keep_connected_from_event(event).is_some() {
                    cx.notify();
                }
            });
        let sign_in_choices =
            connection_form::sign_in_choices(&self.sign_ins, profile.database_type);
        let authentication = connection_form::authentication_select(
            profile.authentication.sign_in().is_some(),
            window,
            cx,
        );
        let sign_in = connection_form::sign_in_combobox(
            &sign_in_choices,
            profile.authentication.sign_in(),
            window,
            cx,
        );
        let authentication_subscription =
            cx.subscribe_in(&authentication, window, |_, _, event, _, cx| {
                if connection_form::uses_sign_in_from_event(event).is_some() {
                    cx.notify();
                }
            });
        let sign_in_subscription = Self::subscribe_sign_in_list(&sign_in, window, cx);
        let dbt = dbt::DbtForm::new(profile.dbt.as_ref(), window, cx);
        // The match summary needs the saved schemas of the connection.
        if !is_new && profile.dbt.is_some() {
            self.ensure_catalog(profile.id);
        }
        let database_type = connection_form::choice_select(
            &connection_form::database_type_choices(),
            &profile.database_type,
            window,
            cx,
        );
        let database_type_subscription =
            cx.subscribe_in(&database_type, window, |this, _, event, window, cx| {
                if matches!(event, SelectEvent::Confirm(Some(_))) {
                    if let Some(form) = &mut this.form {
                        let selected = connection_form::chosen(
                            &form.database_type,
                            &connection_form::database_type_choices(),
                            cx,
                        );
                        let previous = form.profile.database_type;
                        if selected != previous {
                            let default_name = |kind| match kind {
                                crate::model::DatabaseType::Kyuubi => "Spark",
                                crate::model::DatabaseType::Postgres => "Postgres",
                                crate::model::DatabaseType::Trino => "Trino",
                            };
                            if form.is_new
                                && form.fields[0].read(cx).value().as_ref()
                                    == default_name(previous)
                            {
                                form.fields[0].update(cx, |field, cx| {
                                    field.set_value(default_name(selected), window, cx)
                                });
                            }
                            if form.fields[2].read(cx).value().as_ref()
                                == previous.default_port().to_string()
                            {
                                form.fields[2].update(cx, |field, cx| {
                                    field.set_value(selected.default_port().to_string(), window, cx)
                                });
                            }
                            if form.fields[5].read(cx).value().as_ref()
                                == match previous {
                                    crate::model::DatabaseType::Kyuubi => "avia",
                                    crate::model::DatabaseType::Postgres => "postgres",
                                    crate::model::DatabaseType::Trino => "tpch",
                                }
                            {
                                form.fields[5].update(cx, |field, cx| {
                                    field.set_value(
                                        match selected {
                                            crate::model::DatabaseType::Kyuubi => "avia",
                                            crate::model::DatabaseType::Postgres => "postgres",
                                            crate::model::DatabaseType::Trino => "tpch",
                                        },
                                        window,
                                        cx,
                                    )
                                });
                            }
                            form.profile.database_type = selected;
                        }
                    }
                    let selected = this.form.as_ref().and_then(|form| {
                        connection_form::chosen_sign_in(&form.sign_in, &form.sign_in_choices, cx)
                    });
                    this.replace_sign_in_list(selected, window, cx);
                    cx.notify();
                }
            });
        let postgres_ssl_mode = connection_form::choice_select(
            &connection_form::postgres_ssl_mode_choices(),
            &if is_new {
                crate::model::PostgresSslMode::Require
            } else {
                profile.postgres_ssl_mode()
            },
            window,
            cx,
        );
        let postgres_ssl_mode_subscription =
            cx.subscribe_in(&postgres_ssl_mode, window, |_, _, event, _, cx| {
                if matches!(event, SelectEvent::Confirm(Some(_))) {
                    cx.notify();
                }
            });
        self.form = Some(ProfileEditor {
            dbt,
            database_type,
            _database_type_subscription: database_type_subscription,
            postgres_ssl_mode,
            _postgres_ssl_mode_subscription: postgres_ssl_mode_subscription,
            parameters,
            notes_counted: connection_form::counts_notes(profile.assistant_notes.len()),
            assistant_notes,
            _assistant_notes_subscription: assistant_notes_subscription,
            idle_behavior,
            _idle_behavior_subscription: idle_behavior_subscription,
            schema_refresh,
            _schema_refresh_subscription: schema_refresh_subscription,
            column_reads,
            catalog_select,
            catalog_choices,
            catalog_choice: catalog,
            new_catalog: Uuid::new_v4(),
            shared_name,
            preferred_select,
            preferred_choices,
            authentication,
            sign_in,
            sign_in_choices,
            tls: profile.tls,
            _authentication_subscription: authentication_subscription,
            _sign_in_subscription: sign_in_subscription,
            profile,
            fields,
            is_new,
            error: None,
            saving: None,
        });
        self.open_profile_dialog(window, cx);
        cx.notify();
    }
    /// Choose a new shared catalog, and move to its name. A new list
    /// replaces the open one, because GPUI Kit 0.6.6 cannot close it.
    fn new_shared_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else {
            return;
        };
        connection_form::add_new_catalog(&mut form.catalog_choices);
        form.catalog_select = connection_form::catalog_combobox(
            &form.catalog_choices,
            &connection_form::CatalogChoice::New,
            window,
            cx,
        );
        let name = form.shared_name.clone();
        self.catalog_choice_changed(connection_form::CatalogChoice::New, window, cx);
        name.update(cx, |input, cx| input.focus(window, cx));
    }
    /// Load the settings of the chosen catalog into the Schemas fields. A new
    /// shared catalog starts with the settings in the fields.
    fn catalog_choice_changed(
        &mut self,
        choice: connection_form::CatalogChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.catalog_choice = choice;
        // Another choice drops the new shared catalog, so the list shows the
        // New shared catalog button again.
        if choice != connection_form::CatalogChoice::New
            && connection_form::remove_new_catalog(&mut form.catalog_choices)
        {
            connection_form::set_catalog_choices(
                &form.catalog_select,
                &form.catalog_choices,
                &choice,
                window,
                cx,
            );
        }
        let shared = match choice {
            connection_form::CatalogChoice::Shared(id) => {
                self.shared_catalogs.iter().find(|catalog| catalog.id == id)
            }
            connection_form::CatalogChoice::Private | connection_form::CatalogChoice::New => None,
        };
        let settings = match (choice, shared) {
            (connection_form::CatalogChoice::Private, _) => Some(&form.profile.catalog),
            (_, Some(shared)) => Some(&shared.settings),
            _ => None,
        };
        if let Some(settings) = settings {
            let values = [
                (10, settings.include.join(", ")),
                (11, settings.exclude.join(", ")),
                (12, settings.refresh_minutes.to_string()),
                (13, settings.timeout_minutes.to_string()),
            ];
            for (index, value) in values {
                form.fields[index].update(cx, |input, cx| input.set_value(value, window, cx));
            }
            // A private catalog that is off still shows schemas once chosen.
            let mode = match connection_form::RefreshMode::of(settings.refresh) {
                connection_form::RefreshMode::Disabled => connection_form::RefreshMode::Manual,
                mode => mode,
            };
            form.schema_refresh = connection_form::schema_refresh_select(mode, window, cx);
            form._schema_refresh_subscription =
                cx.subscribe_in(&form.schema_refresh, window, |_this, _, event, _, cx| {
                    if matches!(event, SelectEvent::Confirm(Some(_))) {
                        cx.notify();
                    }
                });
        }
        let name = shared.map(|shared| shared.name.clone()).unwrap_or_default();
        form.shared_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        form.preferred_choices =
            connection_form::preferred_choices(&form.profile, choice, &self.profiles);
        let preferred = shared.and_then(|shared| shared.preferred);
        form.preferred_select =
            connection_form::choice_select(&form.preferred_choices, &preferred, window, cx);
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
        let mut shared = None;
        profile.database_type = connection_form::chosen(
            &form.database_type,
            &connection_form::database_type_choices(),
            cx,
        );
        profile.name = values[0].trim().into();
        profile.host = values[1].trim().into();
        profile.username = values[3].trim().into();
        profile.database = values[5].trim().into();
        profile.trino_schema = values[15].trim().into();
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
            let mode = connection_form::refresh_mode(&form.schema_refresh, cx);
            // Disabled hides the other Schemas fields and keeps their values,
            // also the shared catalog of the connection.
            if mode == connection_form::RefreshMode::Disabled {
                profile.catalog.refresh = CatalogRefresh::Disabled;
                shared = profile.shared_catalog.and_then(|id| {
                    self.shared_catalogs
                        .iter()
                        .find(|catalog| catalog.id == id)
                        .cloned()
                });
            } else {
                profile.catalog_column_reads = connection_form::chosen(
                    &form.column_reads,
                    &connection_form::column_read_choices(),
                    cx,
                );
                let choice = connection_form::chosen_catalog(
                    &form.catalog_select,
                    &form.catalog_choices,
                    cx,
                );
                let preferred =
                    connection_form::chosen(&form.preferred_select, &form.preferred_choices, cx);
                // A member keeps its own settings for a later private catalog.
                // A shared catalog keeps the values of the fields that the
                // mode hides, like the period of a manual refresh.
                let mut settings = match choice {
                    connection_form::CatalogChoice::Private => profile.catalog.clone(),
                    connection_form::CatalogChoice::Shared(id) => self
                        .shared_catalogs
                        .iter()
                        .find(|catalog| catalog.id == id)
                        .map_or_else(
                            || profile.catalog.clone(),
                            |catalog| catalog.settings.clone(),
                        ),
                    connection_form::CatalogChoice::New => profile.catalog.clone(),
                };
                settings.include = connection_form::parse_patterns(&values[10]);
                settings.exclude = connection_form::parse_patterns(&values[11]);
                connection_form::parse_refresh_policy(
                    &values[12],
                    &values[13],
                    mode,
                    &mut settings,
                )?;
                let id = match choice {
                    connection_form::CatalogChoice::Private => None,
                    connection_form::CatalogChoice::Shared(id) => Some(id),
                    connection_form::CatalogChoice::New => Some(form.new_catalog),
                };
                match id {
                    None => {
                        profile.catalog = settings;
                        profile.shared_catalog = None;
                    }
                    Some(id) => {
                        profile.catalog.refresh = settings.refresh;
                        profile.shared_catalog = Some(id);
                        let catalog = SharedCatalog {
                            id,
                            name: form.shared_name.read(cx).value().trim().to_owned(),
                            settings,
                            preferred,
                        };
                        catalog.validate()?;
                        anyhow::ensure!(
                            !connection_form::shared_name_is_taken(&self.shared_catalogs, &catalog),
                            "A shared catalog or a catalog choice with this name already exists."
                        );
                        shared = Some(catalog);
                    }
                }
            }
            profile.lifecycle.response_timeout_seconds =
                values[14].trim().parse().map_err(|_| {
                    anyhow::anyhow!("Response timeout must be a whole number of seconds.")
                })?;
            profile.lifecycle = connection_form::parse_lifecycle(
                &values[7..10],
                connection_form::keeps_connected(&form.idle_behavior, cx),
                &profile.lifecycle,
            )?;
            if profile.database_type == crate::model::DatabaseType::Postgres {
                let mode = connection_form::chosen(
                    &form.postgres_ssl_mode,
                    &connection_form::postgres_ssl_mode_choices(),
                    cx,
                );
                profile.postgres_ssl_mode = Some(mode);
                profile.tls = mode != crate::model::PostgresSslMode::Disable;
            } else {
                profile.tls = form.tls;
            }
            profile.assistant_notes = form.assistant_notes.read(cx).value().trim().to_owned();
            profile.dbt = form.dbt.project(cx)?;
            profile.authentication = if profile.database_type
                != crate::model::DatabaseType::Postgres
                && connection_form::uses_sign_in(&form.authentication, cx)
            {
                let sign_in =
                    connection_form::chosen_sign_in(&form.sign_in, &form.sign_in_choices, cx)
                        .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id))
                        .ok_or_else(|| anyhow::anyhow!("Choose a sign-in for this connection."))?;
                anyhow::ensure!(
                    sign_in.allows_host(&profile.host),
                    "The sign-in \"{}\" does not send tokens to {}. Add the host to the database hosts of the sign-in.",
                    sign_in.name,
                    profile.host
                );
                match sign_in.provider {
                    crate::model::SignInProvider::Oidc => Authentication::Oidc {
                        sign_in: sign_in.id,
                    },
                    crate::model::SignInProvider::TrinoExternal => Authentication::TrinoExternal {
                        sign_in: sign_in.id,
                    },
                }
            } else {
                Authentication::Password
            };
            profile.validate()?;
            anyhow::ensure!(
                !connection_form::profile_name_is_taken(&self.profiles, &profile),
                "A connection with this name already exists."
            );
            Ok(())
        })();
        if let Err(e) = parse {
            form.error = Some(e.to_string());
            cx.notify();
            return;
        }
        let password = if profile.authentication == Authentication::Password {
            Zeroizing::new(form.fields[4].read(cx).unmask_value().to_string())
        } else {
            Zeroizing::new(String::new())
        };
        // A profile that used a sign-in may have no stored password.
        let needs_password = form.is_new || form.profile.authentication != Authentication::Password;
        if profile.authentication == Authentication::Password
            && needs_password
            && password.is_empty()
            && profile.database_type != crate::model::DatabaseType::Trino
            && !self.demo
        {
            form.error = Some("Enter the password for this connection.".into());
            cx.notify();
            return;
        }
        let (send, recv) = mpsc::channel();
        form.saving = Some(recv);
        let _ = self.wake.try_send(());
        form.error = None;
        let demo = self.demo;
        let credentials = self.credentials.clone();
        let password_changed = !password.is_empty();
        std::thread::spawn(move || {
            let result = if !demo
                && (!password.is_empty()
                    || (needs_password
                        && profile.database_type == crate::model::DatabaseType::Trino
                        && profile.authentication == Authentication::Password))
            {
                credentials
                    .set_password(profile.id, &password)
                    .map_err(|e| e.to_string())
            } else {
                Ok(())
            };
            let _ = send.send(result.map(|()| ProfileSave {
                profile,
                shared,
                password_changed,
            }));
        });
        cx.notify();
    }
    fn confirm_delete_profile(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_in_use(id) {
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
        if self.profile_in_use(id) || !self.profiles.iter().any(|p| p.id == id) {
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
        let removed: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| tab.saved.profile == Some(id))
            .map(|tab| tab.saved.id)
            .collect();
        self.tabs.retain(|tab| tab.saved.profile != Some(id));
        for tab in removed {
            self.assistant_tab_removed(tab, None);
            self.activity
                .update(cx, |activity, cx| activity.tab_closed(tab, cx));
        }
        self.activity
            .update(cx, |activity, cx| activity.remove(id, cx));
        self.profiles.retain(|p| p.id != id);
        self.active_tabs.remove(&id);
        crate::model::prune_shared_catalogs(&mut self.profiles, &mut self.shared_catalogs);
        self.sync_catalogs(None, cx);
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
            let credentials = self.credentials.clone();
            std::thread::spawn(move || {
                let _ = credentials.delete_password(id);
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
    /// A connection cannot be deleted while a query runs or an assistant
    /// conversation works in one of its tabs.
    fn profile_in_use(&self, id: Uuid) -> bool {
        self.profile_busy(id)
            || self
                .tabs
                .iter()
                .any(|tab| tab.saved.profile == Some(id) && self.assistant_tab_busy(tab.saved.id))
    }
    fn seed_demo(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
        let execution_id = Self::allocate_execution_id(tab);
        let sql = tab.input.read(cx).value().to_string();
        tab.current_execution = Some(execution_id);
        let events = [
            LogEvent::new(
                Some(execution_id),
                Severity::Info,
                LogKind::Connected,
                "Connected to rivendell-s (demo)",
            )
            .with_connection("rivendell-s"),
            LogEvent::new(
                Some(execution_id),
                Severity::Info,
                LogKind::Submitted,
                format!("Submitted query:\n{sql}"),
            )
            .with_connection("rivendell-s")
            .with_sql(sql),
            LogEvent::new(
                Some(execution_id),
                Severity::Info,
                LogKind::ExecutionCompleted,
                "Execution completed on the server, result set: true (demo)",
            ),
            LogEvent::new(
                Some(execution_id),
                Severity::Info,
                LogKind::FetchCompleted,
                "Fetched preview page 1: rows 1–2250, 2250 rows, 2250 retained, more rows: false (demo)",
            ),
        ];
        let mut activity = Vec::new();
        for event in events {
            activity.extend(crate::activity::from_tab(
                &event,
                tab.saved.id,
                &tab.saved.title,
            ));
            Self::record_log(tab, event);
        }
        if let Some(connection) = tab.saved.profile {
            for entry in activity {
                self.record_activity(connection, entry, cx);
            }
        }
        let tab = &mut self.tabs[index];
        tab.table.update(cx, |t, cx| {
            let data = t.delegate_mut();
            data.clear();
            let mut columns: Vec<_> = [
                ("route", "STRING"),
                ("carrier", "STRING"),
                ("departures", "BIGINT"),
                ("avg_fare", "DOUBLE"),
                ("currency", "STRING"),
                ("updated_at", "TIMESTAMP"),
                ("load_factor", "DOUBLE"),
                ("cancelled", "BOOLEAN"),
            ]
            .into_iter()
            .map(|(name, data_type)| crate::model::Column {
                name: name.into(),
                data_type: data_type.into(),
            })
            .collect();
            columns.extend((9..=141).map(|n| crate::model::Column {
                name: format!("metric_{n}"),
                data_type: "DOUBLE".into(),
            }));
            data.schema(columns, cx);
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
                        if i % 7 == 0 {
                            None
                        } else {
                            Some(["Finnair", "British Airways", "Singapore Airlines"][i % 3].into())
                        },
                        Some((3821 - i).to_string()),
                        Some(format!("{:.2}", 142.5 + (i % 93) as f64 * 3.2)),
                        Some("USD".into()),
                        Some("2026-09-11 09:41:03".into()),
                        Some(format!("0.{:02}", 70 + i % 29)),
                        Some("false".into()),
                    ];
                    row.extend((9..=141).map(|c| Some(format!("{:.2}", (i * c) as f64 / 10.))));
                    row
                })
                .collect();
            t.refresh(cx);
        });
        tab.set_status_detail("Complete", "Demo data");
        tab.elapsed = Some(Duration::from_millis(842));
        tab.panel.success(true);
    }
}
/// The title of a section of a context menu. GPUI Kit draws its menu labels
/// like disabled items, so the title uses the style of a native menu section
/// header instead: smaller, semibold, and muted.
fn menu_section(title: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, cx| {
        div()
            .id(SharedString::from(format!("menu-section-{title}")))
            .test_support()
            .aria_label(title)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx.theme().muted_foreground)
            .child(title)
    })
    .disabled(true)
}

fn demo_workspace(manifest: Option<&tempfile::NamedTempFile>) -> Workspace {
    let catalog = CatalogSettings {
        refresh: CatalogRefresh::Manual,
        ..CatalogSettings::default()
    };
    // The two Rivendell clusters read the same metastore.
    let shared_catalogs = vec![SharedCatalog {
        id: Uuid::new_v4(),
        name: "Rivendell".into(),
        settings: catalog.clone(),
        preferred: None,
    }];
    let profiles: Vec<_> = ["rivendell-s", "rivendell-xl", "analytics-s"]
        .into_iter()
        .map(|name| Profile {
            name: name.into(),
            host: "demo.local".into(),
            username: format!("kyuubi-{name}"),
            catalog: catalog.clone(),
            shared_catalog: name
                .starts_with("rivendell")
                .then_some(shared_catalogs[0].id),
            dbt: manifest
                .filter(|_| name.starts_with("rivendell"))
                .map(|manifest| crate::model::DbtProject {
                    manifest: manifest.path().to_string_lossy().into_owned(),
                    refresh: crate::model::DbtRefresh::Manual,
                    schema_mapping: vec![],
                }),
            ..Default::default()
        })
        .collect();
    let mut tab = SavedTab::new(1, Some(profiles[0].id));
    tab.title = "Route overview".into();
    tab.sql = "-- A quick look at route performance\nSELECT\n    route,\n    COUNT(*) AS departures,\n    ROUND(AVG(fare), 2) AS avg_fare,\n    currency,\n    MAX(updated_at) AS updated_at\nFROM avia.flight_events\nWHERE departure_date >= '2026-09-01'\nGROUP BY route, currency\nORDER BY departures DESC;".into();
    Workspace {
        version: WORKSPACE_VERSION,
        settings: Settings::default(),
        assistant: AssistantWorkspace::default(),
        profiles: profiles.clone(),
        tabs: vec![tab, SavedTab::new(2, Some(profiles[1].id))],
        active_tab: 0,
        active_tabs: BTreeMap::new(),
        shared_catalogs,
        sign_ins: vec![],
    }
}

#[cfg(test)]
mod font_tests {
    use super::{SYSTEM_FONT_FAMILY, font_available};

    #[test]
    fn system_font_is_valid_even_when_not_enumerated() {
        let installed = vec!["Menlo".into()];
        assert!(font_available(SYSTEM_FONT_FAMILY, &installed));
        assert!(font_available("Menlo", &installed));
        assert!(!font_available("Missing font", &installed));
    }
}

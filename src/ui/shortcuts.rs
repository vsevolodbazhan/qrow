//! The commands that can have a keyboard shortcut, their default shortcuts,
//! and the keymap that the defaults and the user's choices make.
//!
//! Each command has at most one shortcut. Settings keeps only the commands
//! that the user changed, so a new default reaches the other commands. A
//! change clears the keymap and builds it again: first the bindings of GPUI
//! Kit, without those of the commands here, then the shortcut of each
//! command. The macOS menu bar copies shortcuts when it is set, so the menus
//! are set again after each change.

use super::*;
use gpui_kit::component::input::{Indent, Outdent, Redo, Replace, Search, Undo};
use std::collections::HashMap;

/// The settings section of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Group {
    Query,
    Tabs,
    Results,
    ResultGrid,
    Editor,
    Connections,
    Assistant,
    Dialogs,
    Application,
}

impl Group {
    pub(crate) const ALL: [Self; 9] = [
        Self::Query,
        Self::Tabs,
        Self::Results,
        Self::ResultGrid,
        Self::Editor,
        Self::Connections,
        Self::Assistant,
        Self::Dialogs,
        Self::Application,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Query => "Query",
            Self::Tabs => "Tabs",
            Self::Results => "Results",
            Self::ResultGrid => "Result Grid",
            Self::Editor => "Editor",
            Self::Connections => "Connections",
            Self::Assistant => "Assistant",
            Self::Dialogs => "Dialogs",
            Self::Application => "Application",
        }
    }
}

/// Where a shortcut operates. Two commands in the same place cannot share a
/// shortcut. Where one place is inside another, like the SQL editor in the
/// window, the inner command wins there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Everywhere in every Qrow window, before any other binding: Quit,
    /// Settings, and the scale keys.
    App,
    /// The workspace of the main window.
    Window,
    /// The SQL editor.
    Editor,
    /// Every text field, the SQL editor and the message field included.
    Text,
    Grid,
    Tree,
    /// The assistant's message field.
    Composer,
    /// Dialogs and the Settings window, outside the workspace.
    Dialog,
}

/// How two shortcuts with the same keys get along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Relation {
    /// Both cannot work: one hides the other.
    Conflict,
    /// Both work; in the inner place, its command wins.
    Overlap {
        inner: Scope,
    },
    Independent,
}

impl Scope {
    /// The start of a message about the place, like "In the SQL editor".
    pub(crate) fn where_phrase(self) -> &'static str {
        match self {
            Self::App | Self::Window => "In the window",
            Self::Editor => "In the SQL editor",
            Self::Text => "In text fields",
            Self::Grid => "In the result grid",
            Self::Tree => "In the connection tree",
            Self::Composer => "In the message field",
            Self::Dialog => "In dialogs",
        }
    }

    /// The relation of a shortcut in `self` and one in `other` with the same
    /// keys.
    pub(crate) fn relation(self, other: Self) -> Relation {
        match (self, other) {
            _ if self == other => Relation::Conflict,
            // An app-wide binding wins everywhere.
            (Self::App, _) | (_, Self::App) => Relation::Conflict,
            // The SQL editor is a text field, and text fields win over it.
            (Self::Editor, Self::Text) | (Self::Text, Self::Editor) => Relation::Conflict,
            (Self::Window, Self::Dialog) | (Self::Dialog, Self::Window) => Relation::Independent,
            // Dialogs have text fields, which take their keys first.
            (Self::Dialog, Self::Text) | (Self::Text, Self::Dialog) => {
                Relation::Overlap { inner: Self::Text }
            }
            (Self::Window, inner) | (inner, Self::Window) => Relation::Overlap { inner },
            (Self::Text, Self::Composer) | (Self::Composer, Self::Text) => Relation::Overlap {
                inner: Self::Composer,
            },
            _ => Relation::Independent,
        }
    }
}

/// The key context of GPUI Kit's text fields.
const INPUT: &str = "Input";
/// The key context of the main window's workspace. Commands of the window
/// bind in it, so a binding of a nested place, like the editor, wins over
/// them. A binding without a context would match at the deepest level and
/// win over every nested binding.
pub(crate) const WORKSPACE: &str = "Qrow";

/// The default shortcut of a command.
#[derive(Clone, Copy, Debug)]
enum DefaultKeys {
    None,
    Keys(&'static str),
    /// The shortcut that GPUI Kit binds to the first target in its context.
    /// GPUI Kit chooses the keys of each platform.
    Kit,
}

type Targets = fn() -> Vec<(Box<dyn Action>, Option<&'static str>)>;

/// A command that can have a shortcut.
pub(crate) struct Command {
    /// The stable name in the saved settings.
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) group: Group,
    pub(crate) scope: Scope,
    /// A second place of the command: Run Query also works in the SQL editor,
    /// where it wins over the editor's own ⌘↩.
    also: Option<Scope>,
    /// The actions that the shortcut sends, each in its key context. Some
    /// commands send one action of several, for example in different dialogs.
    targets: Targets,
    default: DefaultKeys,
}

impl Command {
    /// The places where the shortcut of the command works.
    pub(crate) fn places(&self) -> impl Iterator<Item = Scope> {
        std::iter::once(self.scope).chain(self.also)
    }

    /// The relation of this command's shortcut and `other`'s with the same
    /// keys, over all of their places.
    pub(crate) fn relation(&self, other: &Command) -> Relation {
        let relations: Vec<Relation> = self
            .places()
            .flat_map(|mine| other.places().map(move |theirs| mine.relation(theirs)))
            .collect();
        relations
            .iter()
            .find(|relation| **relation == Relation::Conflict)
            .or_else(|| {
                relations
                    .iter()
                    .find(|relation| matches!(relation, Relation::Overlap { .. }))
            })
            .copied()
            .unwrap_or(Relation::Independent)
    }

    pub(crate) fn targets(&self) -> Vec<(Box<dyn Action>, Option<&'static str>)> {
        (self.targets)()
    }
}

macro_rules! command {
    ($id:literal, $label:literal, $group:ident, $scope:ident, $default:expr, [$(($action:expr, $context:expr)),+ $(,)?]) => {
        Command {
            id: $id,
            label: $label,
            group: Group::$group,
            scope: Scope::$scope,
            also: None,
            targets: || vec![$((Box::new($action) as Box<dyn Action>, $context)),+],
            default: $default,
        }
    };
}

macro_rules! select_tab {
    ($($number:literal),+) => {
        [$(Command {
            id: concat!("select_tab_", $number),
            label: concat!("Select Tab ", $number),
            group: Group::Tabs,
            scope: Scope::Window,
            also: None,
            targets: || vec![(Box::new(SelectTab($number - 1)) as Box<dyn Action>, Some("Qrow"))],
            default: DefaultKeys::Keys(concat!("secondary-", $number)),
        }),+]
    };
}

const RESULTS_TABLE: Option<&str> = Some("Results > DataTable");
const TREE: Option<&str> = Some("Tree");

fn base_commands() -> Vec<Command> {
    use DefaultKeys::{Keys, Kit, None as Unset};
    let mut commands = vec![
        Command {
            also: Some(Scope::Editor),
            ..command!(
                "run_query",
                "Run Query",
                Query,
                Window,
                Keys("secondary-enter"),
                [(RunQuery, None), (RunQuery, Some("SqlEditor > Input"))]
            )
        },
        command!(
            "cancel_query",
            "Cancel Query",
            Query,
            Window,
            Unset,
            [(CancelQuery, None)]
        ),
        command!(
            "export_results",
            "Export Results",
            Query,
            Window,
            Unset,
            [(ExportResults, None)]
        ),
        command!(
            "run_and_export",
            "Run and Export",
            Query,
            Window,
            Unset,
            [(RunAndExport, None)]
        ),
        command!(
            "format_sql",
            "Format SQL",
            Query,
            Window,
            Keys("alt-secondary-l"),
            [(FormatSql, None)]
        ),
        command!(
            "toggle_comment",
            "Toggle Comment",
            Query,
            Editor,
            Keys("secondary-/"),
            [(ToggleComment, Some("SqlEditor"))]
        ),
        command!(
            "new_tab",
            "New Tab",
            Tabs,
            Window,
            Keys("secondary-t"),
            [(NewTab, None)]
        ),
        command!(
            "close_tab",
            "Close Tab",
            Tabs,
            Window,
            Keys("secondary-w"),
            [(CloseTab, None)]
        ),
        command!(
            "rename_tab",
            "Rename Tab",
            Tabs,
            Window,
            Unset,
            [(RenameTab, None)]
        ),
        command!(
            "next_tab",
            "Next Tab",
            Tabs,
            Window,
            Unset,
            [(NextTab, None)]
        ),
        command!(
            "previous_tab",
            "Previous Tab",
            Tabs,
            Window,
            Unset,
            [(PreviousTab, None)]
        ),
    ];
    commands.extend(select_tab!(1, 2, 3, 4, 5, 6, 7, 8, 9));
    commands.extend([
        command!(
            "import_sql_file",
            "Import SQL File",
            Tabs,
            Window,
            Keys("secondary-o"),
            [(ImportSqlFile, None)]
        ),
        command!(
            "save_as_sql_file",
            "Save as SQL File",
            Tabs,
            Window,
            Keys("secondary-shift-s"),
            [(SaveAsSqlFile, None)]
        ),
        command!(
            "toggle_results",
            "Toggle Results",
            Results,
            Window,
            Keys("secondary-j"),
            [(ToggleResults, None)]
        ),
        command!(
            "transpose_result",
            "Transpose Result",
            Results,
            Window,
            Keys("alt-secondary-t"),
            [(TransposeResult, None)]
        ),
        command!(
            "pin_result",
            "Pin Result",
            Results,
            Window,
            Unset,
            [(PinResult, None)]
        ),
        command!(
            "close_result",
            "Close Result",
            Results,
            Window,
            Unset,
            [(CloseResult, None)]
        ),
        command!(
            "next_result_tab",
            "Next Result Tab",
            Results,
            Window,
            Unset,
            [(NextResultTab, None)]
        ),
        command!(
            "previous_result_tab",
            "Previous Result Tab",
            Results,
            Window,
            Unset,
            [(PreviousResultTab, None)]
        ),
        command!(
            "copy_query",
            "Copy Query",
            Results,
            Window,
            Unset,
            [(CopyQuery, None)]
        ),
        command!(
            "copy_cells",
            "Copy Cells",
            ResultGrid,
            Grid,
            Keys("secondary-c"),
            [(gpui_kit::component::input::Copy, RESULTS_TABLE)]
        ),
        command!(
            "select_all_cells",
            "Select All Cells",
            ResultGrid,
            Grid,
            Keys("secondary-a"),
            [(gpui_kit::component::input::SelectAll, RESULTS_TABLE)]
        ),
        command!(
            "first_row",
            "First Row",
            ResultGrid,
            Grid,
            Keys("secondary-up"),
            [(results::SelectFirstRow, RESULTS_TABLE)]
        ),
        command!(
            "last_row",
            "Last Row",
            ResultGrid,
            Grid,
            Keys("secondary-down"),
            [(results::SelectLastRow, RESULTS_TABLE)]
        ),
        command!(
            "extend_selection_up",
            "Extend Selection Up",
            ResultGrid,
            Grid,
            Keys("shift-up"),
            [(results::ExtendSelectionUp, RESULTS_TABLE)]
        ),
        command!(
            "extend_selection_down",
            "Extend Selection Down",
            ResultGrid,
            Grid,
            Keys("shift-down"),
            [(results::ExtendSelectionDown, RESULTS_TABLE)]
        ),
        command!(
            "extend_selection_left",
            "Extend Selection Left",
            ResultGrid,
            Grid,
            Keys("shift-left"),
            [(results::ExtendSelectionLeft, RESULTS_TABLE)]
        ),
        command!(
            "extend_selection_right",
            "Extend Selection Right",
            ResultGrid,
            Grid,
            Keys("shift-right"),
            [(results::ExtendSelectionRight, RESULTS_TABLE)]
        ),
        command!("find", "Find", Editor, Text, Kit, [(Search, Some(INPUT))]),
        command!(
            "replace",
            "Replace",
            Editor,
            Text,
            Kit,
            [(Replace, Some(INPUT))]
        ),
        command!(
            "indent",
            "Indent",
            Editor,
            Text,
            Kit,
            [(Indent, Some(INPUT))]
        ),
        command!(
            "outdent",
            "Outdent",
            Editor,
            Text,
            Kit,
            [(Outdent, Some(INPUT))]
        ),
        command!("undo", "Undo", Editor, Text, Kit, [(Undo, Some(INPUT))]),
        command!("redo", "Redo", Editor, Text, Kit, [(Redo, Some(INPUT))]),
        command!(
            "select_all",
            "Select All",
            Editor,
            Text,
            Kit,
            [(gpui_kit::component::input::SelectAll, Some(INPUT))]
        ),
        command!(
            "show_connections",
            "Show Connections",
            Connections,
            Window,
            Keys("secondary-b"),
            [(ShowConnections, None)]
        ),
        command!(
            "show_sign_ins",
            "Show Sign-Ins",
            Connections,
            Window,
            Unset,
            [(ShowSignIns, None)]
        ),
        command!(
            "toggle_sidebar",
            "Toggle Sidebar",
            Connections,
            Window,
            Unset,
            [(ToggleSidebar, None)]
        ),
        command!(
            "paste_connection",
            "Paste Connection",
            Connections,
            Window,
            Unset,
            [(PasteConnection, None)]
        ),
        command!(
            "refresh_metadata",
            "Refresh Metadata",
            Connections,
            Window,
            Unset,
            [(RefreshMetadata, None)]
        ),
        command!(
            "copy_name",
            "Copy Name",
            Connections,
            Tree,
            Keys("secondary-c"),
            [(CopyCatalogName, TREE)]
        ),
        command!(
            "insert_name",
            "Insert Name",
            Connections,
            Tree,
            Keys("shift-enter"),
            [(InsertCatalogName, TREE)]
        ),
        command!(
            "move_connection_up",
            "Move Connection Up",
            Connections,
            Tree,
            Keys("alt-up"),
            [(MoveConnectionUp, Some("Connections > Tree"))]
        ),
        command!(
            "move_connection_down",
            "Move Connection Down",
            Connections,
            Tree,
            Keys("alt-down"),
            [(MoveConnectionDown, Some("Connections > Tree"))]
        ),
        command!(
            "toggle_assistant",
            "Toggle Assistant",
            Assistant,
            Window,
            Keys("secondary-r"),
            [(ToggleAssistant, None)]
        ),
        command!(
            "new_conversation",
            "New Conversation",
            Assistant,
            Window,
            Unset,
            [(NewConversation, None)]
        ),
        command!(
            "maximize_assistant",
            "Maximize or Restore Assistant",
            Assistant,
            Window,
            Keys("secondary-shift-r"),
            [
                (MaximizeAssistant, Some("Qrow && !AssistantMaximized")),
                (MinimizeAssistant, Some("Qrow && AssistantMaximized")),
            ]
        ),
        command!(
            "send_message",
            "Send Message",
            Assistant,
            Composer,
            Keys("secondary-enter"),
            [(SendAssistantMessage, Some("AssistantComposer > Input"))]
        ),
        command!(
            "confirm_dialog",
            "Save or Confirm",
            Dialogs,
            Dialog,
            Keys("secondary-enter"),
            [
                (SaveConnection, Some("ConnectionSettings")),
                (SaveSignIn, Some("SignInSettings")),
                (SubmitRename, Some("RenameDialog")),
            ]
        ),
        command!(
            "close_dialog",
            "Close Dialog",
            Dialogs,
            Dialog,
            Keys("secondary-w"),
            [
                (gpui_kit::base::actions::Cancel, Some("Dialog")),
                (
                    settings_window::CloseSettings,
                    Some(settings_window::CONTEXT)
                ),
            ]
        ),
        command!(
            "activity",
            "Activity",
            Application,
            Window,
            Keys("secondary-shift-u"),
            [(ToggleActivity, None)]
        ),
        command!(
            "settings",
            "Settings",
            Application,
            App,
            Keys("secondary-,"),
            [(OpenSettings, None)]
        ),
        command!(
            "increase_scale",
            "Increase Scale",
            Application,
            App,
            Keys("secondary-="),
            [(IncreaseUiScale, None)]
        ),
        command!(
            "decrease_scale",
            "Decrease Scale",
            Application,
            App,
            Keys("secondary--"),
            [(DecreaseUiScale, None)]
        ),
        command!(
            "focus_editor",
            "Focus Editor",
            Application,
            Window,
            Unset,
            [(FocusEditor, None)]
        ),
        command!(
            "focus_results",
            "Focus Results",
            Application,
            Window,
            Unset,
            [(FocusResults, None)]
        ),
        command!(
            "focus_catalog",
            "Focus Connection Tree",
            Application,
            Window,
            Unset,
            [(FocusCatalog, None)]
        ),
        command!(
            "check_for_updates",
            "Check for Updates",
            Application,
            Window,
            Unset,
            [(CheckForUpdates, None)]
        ),
        command!(
            "about",
            "About Qrow",
            Application,
            Window,
            Unset,
            [(OpenAbout, None)]
        ),
        command!(
            "quit",
            "Quit Qrow",
            Application,
            App,
            Keys("secondary-q"),
            [(Quit, None)]
        ),
    ]);
    commands
}

/// Bindings that are not commands: Escape closes Activity and Settings.
fn fixed_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new(
            "escape",
            activity_view::CloseActivity,
            Some(activity_view::CONTEXT),
        ),
        KeyBinding::new(
            "escape",
            settings_window::CloseSettings,
            Some(settings_window::CONTEXT),
        ),
    ]
}

/// The commands, in their settings order.
pub(crate) fn commands() -> &'static [Command] {
    static COMMANDS: std::sync::OnceLock<Vec<Command>> = std::sync::OnceLock::new();
    COMMANDS.get_or_init(base_commands)
}

pub(crate) fn command(id: &str) -> Option<&'static Command> {
    commands().iter().find(|command| command.id == id)
}

/// The keymap of GPUI Kit and the defaults that come from it.
struct KitBindings {
    bindings: Vec<KeyBinding>,
    defaults: HashMap<&'static str, String>,
}

impl Global for KitBindings {}

/// The state that the menu bar needs when the shortcuts change.
struct MenuState {
    assistant_enabled: bool,
}

impl Global for MenuState {}

/// Store the keymap that GPUI Kit made. Call once after `gpui_kit::init` and
/// before Qrow binds keys.
pub(super) fn init(cx: &mut App) {
    let bindings: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();
    let mut defaults = HashMap::new();
    for command in commands() {
        if !matches!(command.default, DefaultKeys::Kit) {
            continue;
        }
        let Some((action, context)) = command.targets().into_iter().next() else {
            continue;
        };
        // The last binding wins in GPUI, so it is the default.
        let keys = bindings
            .iter()
            .rev()
            .find(|binding| is_binding_of(binding, action.as_ref(), context))
            .and_then(|binding| binding.keystrokes().first())
            .map(|keystroke| stored(keystroke.inner()));
        if let Some(keys) = keys {
            defaults.insert(command.id, keys);
        }
    }
    cx.set_global(KitBindings { bindings, defaults });
    cx.set_global(MenuState {
        assistant_enabled: false,
    });
}

fn is_binding_of(binding: &KeyBinding, action: &dyn Action, context: Option<&str>) -> bool {
    binding.action().name() == action.name()
        && binding
            .predicate()
            .map(|predicate| predicate.to_string())
            .as_deref()
            == context
}

/// The default shortcut of `command`, in its stored form.
pub(crate) fn default_keys(command: &Command, cx: &App) -> Option<String> {
    match command.default {
        DefaultKeys::None => None,
        DefaultKeys::Keys(keys) => normalize(keys),
        DefaultKeys::Kit => cx
            .try_global::<KitBindings>()
            .and_then(|kit| kit.defaults.get(command.id).cloned()),
    }
}

/// The shortcut of `command` with the user's choices.
pub(crate) fn effective(
    command: &Command,
    overrides: &BTreeMap<String, Option<String>>,
    cx: &App,
) -> Option<String> {
    match overrides.get(command.id) {
        Some(keys) => keys.as_deref().and_then(normalize),
        None => default_keys(command, cx),
    }
}

/// True when the user changed the shortcut of `command`.
pub(crate) fn is_customized(
    command: &Command,
    overrides: &BTreeMap<String, Option<String>>,
    cx: &App,
) -> bool {
    overrides.contains_key(command.id)
        && effective(command, overrides, cx) != default_keys(command, cx)
}

/// Set the shortcut of `command`. A shortcut equal to the default removes
/// the override, so a later default reaches the command.
pub(crate) fn assign(
    command: &Command,
    keys: Option<String>,
    overrides: &mut BTreeMap<String, Option<String>>,
    cx: &App,
) {
    let keys = keys.as_deref().and_then(normalize);
    if keys == default_keys(command, cx) {
        overrides.remove(command.id);
    } else {
        overrides.insert(command.id.to_owned(), keys);
    }
}

/// The stored form of a keystroke: the platform's primary modifier is
/// `secondary`, so a workspace keeps its shortcuts on another platform.
pub(crate) fn stored(keystroke: &Keystroke) -> String {
    let modifiers = &keystroke.modifiers;
    let (secondary, ctrl, cmd) = if cfg!(target_os = "macos") {
        (modifiers.platform, modifiers.control, false)
    } else {
        (modifiers.control, false, modifiers.platform)
    };
    let mut parts = Vec::new();
    if secondary {
        parts.push("secondary");
    }
    if ctrl {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if cmd {
        parts.push("cmd");
    }
    if modifiers.function {
        parts.push("fn");
    }
    let key = keystroke.key.to_ascii_lowercase();
    parts.push(&key);
    parts.join("-")
}

/// The stored form of `keys`, or None when `keys` is not one valid keystroke.
pub(crate) fn normalize(keys: &str) -> Option<String> {
    let keys = keys.trim();
    if keys.is_empty() || keys.contains(char::is_whitespace) {
        return None;
    }
    let keystroke = Keystroke::parse(keys).ok()?;
    if keystroke.key.is_empty() || is_modifier(&keystroke.key) {
        return None;
    }
    Some(stored(&keystroke))
}

fn is_modifier(key: &str) -> bool {
    matches!(key, "shift" | "control" | "alt" | "platform" | "function")
}

/// The keystroke of stored `keys`.
pub(crate) fn keystroke(keys: &str) -> Option<Keystroke> {
    Keystroke::parse(&normalize(keys)?).ok()
}

/// The keys as the platform shows them, like "⇧⌘S" or "Ctrl+Shift+S".
pub(crate) fn display(keys: &str) -> String {
    keystroke(keys)
        .map(|keystroke| gpui_kit::component::kbd::Kbd::format(&keystroke))
        .unwrap_or_else(|| keys.to_owned())
}

/// The fixed bindings of GPUI Kit with `keys`: their place and the name of
/// their action, like (Text, "Move Home"). Text fields, the result grid, and
/// the connection tree keep these bindings.
pub(crate) fn fixed_bindings_with(keys: &str, cx: &App) -> Vec<(Scope, String)> {
    let Some(kit) = cx.try_global::<KitBindings>() else {
        return Vec::new();
    };
    let Some(pressed) = keystroke(keys) else {
        return Vec::new();
    };
    let customizable: Vec<(Box<dyn Action>, Option<&'static str>)> = commands()
        .iter()
        .filter(|command| matches!(command.default, DefaultKeys::Kit))
        .flat_map(Command::targets)
        .collect();
    kit.bindings
        .iter()
        .filter_map(|binding| {
            let place = match binding.predicate()?.to_string().as_str() {
                INPUT => Scope::Text,
                "DataTable" => Scope::Grid,
                "Tree" => Scope::Tree,
                _ => return None,
            };
            let [keystroke] = binding.keystrokes() else {
                return None;
            };
            let keystroke = keystroke.inner();
            if keystroke.modifiers != pressed.modifiers || keystroke.key != pressed.key {
                return None;
            }
            if customizable
                .iter()
                .any(|(action, context)| is_binding_of(binding, action.as_ref(), *context))
            {
                return None;
            }
            // Run Query takes ⌘↩ from the editor's own secondary Enter.
            if binding.action().name().ends_with("::Enter") && pressed.modifiers.secondary() {
                return None;
            }
            Some((place, humanize(binding.action().name())))
        })
        .collect()
}

/// "input::MoveToStart" becomes "Move To Start".
fn humanize(name: &str) -> String {
    let name = name.rsplit("::").next().unwrap_or(name);
    let mut words = String::new();
    for (index, character) in name.char_indices() {
        if index > 0 && character.is_uppercase() {
            words.push(' ');
        }
        words.push(character);
    }
    words
}

/// Build the keymap from GPUI Kit's bindings and the shortcuts of the
/// commands, then set the menus again so they show the new shortcuts.
pub(crate) fn apply(overrides: &BTreeMap<String, Option<String>>, cx: &mut App) {
    let Some(kit) = cx.try_global::<KitBindings>() else {
        return;
    };
    let kit_targets: Vec<(Box<dyn Action>, Option<&'static str>)> = commands()
        .iter()
        .filter(|command| matches!(command.default, DefaultKeys::Kit))
        .flat_map(Command::targets)
        .collect();
    let kit_bindings: Vec<KeyBinding> = kit
        .bindings
        .iter()
        .filter(|binding| {
            !kit_targets
                .iter()
                .any(|(action, context)| is_binding_of(binding, action.as_ref(), *context))
        })
        .cloned()
        .collect();
    // Bindings of GPUI Kit's actions come first: the macOS menu bar shows the
    // first binding of an action, for example Select All in the Edit menu.
    let mut kit_command_bindings = Vec::new();
    let mut bindings = Vec::new();
    for command in commands() {
        let Some(keys) = effective(command, overrides, cx) else {
            continue;
        };
        let target = if matches!(command.default, DefaultKeys::Kit) {
            &mut kit_command_bindings
        } else {
            &mut bindings
        };
        for (action, context) in command.targets() {
            let context = context.or((command.scope == Scope::Window).then_some(WORKSPACE));
            let predicate = context
                .map(|context| KeyBindingContextPredicate::parse(context).map(std::rc::Rc::new))
                .transpose();
            let binding = predicate
                .map_err(|error| error.to_string())
                .and_then(|predicate| {
                    KeyBinding::load(&keys, action, predicate, false, None, &DummyKeyboardMapper)
                        .map_err(|error| error.to_string())
                });
            match binding {
                Ok(binding) => target.push(binding),
                Err(error) => eprintln!("Could not bind {keys} to {}: {error}", command.id),
            }
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(kit_command_bindings);
    cx.bind_keys(kit_bindings);
    cx.bind_keys(fixed_bindings());
    cx.bind_keys(bindings);
    let assistant_enabled = cx
        .try_global::<MenuState>()
        .is_some_and(|state| state.assistant_enabled);
    set_menus(cx, assistant_enabled);
}

/// Remember whether the View menu lists the assistant, for later rebuilds.
pub(super) fn set_assistant_menu(cx: &mut App, assistant_enabled: bool) {
    if cx.has_global::<MenuState>() {
        cx.global_mut::<MenuState>().assistant_enabled = assistant_enabled;
    }
}

/// The UI scale change that `pressed` asks for. The window handles the
/// scale keys before text fields and dialogs do, so it matches them here.
/// For keys with `=` or `+`, Shift does not matter: some layouts need Shift
/// for `+`, and ⌘+ is the usual name of the shortcut.
pub(crate) fn scale_change(
    pressed: &Keystroke,
    overrides: &BTreeMap<String, Option<String>>,
    cx: &App,
) -> Option<f32> {
    let keys = [
        ("increase_scale", UI_SCALE_STEP),
        ("decrease_scale", -UI_SCALE_STEP),
    ]
    .into_iter()
    .filter_map(|(id, change)| Some((keystroke(&effective(command(id)?, overrides, cx)?)?, change)))
    .collect::<Vec<_>>();
    // An exact match wins, so ⇧⌘= can decrease while ⌘= increases.
    keys.iter()
        .find(|(bound, _)| bound.modifiers == pressed.modifiers && bound.key == pressed.key)
        .or_else(|| {
            keys.iter()
                .find(|(bound, _)| scale_key_matches(bound, pressed))
        })
        .map(|(_, change)| *change)
}

fn scale_key_matches(bound: &Keystroke, pressed: &Keystroke) -> bool {
    if bound.modifiers == pressed.modifiers && bound.key == pressed.key {
        return true;
    }
    let plus = |key: &str| key == "=" || key == "+";
    let without_shift = |keystroke: &Keystroke| Modifiers {
        shift: false,
        ..keystroke.modifiers
    };
    plus(&bound.key) && plus(&pressed.key) && without_shift(bound) == without_shift(pressed)
}

/// The shortcut of `action` in the keymap, as the platform shows it. Text
/// that names a shortcut uses this, so it follows the user's choice.
pub(crate) fn action_display(action: &dyn Action, cx: &App) -> Option<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let binding = keymap.bindings_for_action(action).last()?;
    let keystroke = binding.keystrokes().first()?;
    Some(gpui_kit::component::kbd::Kbd::format(keystroke.inner()))
}

/// The placeholder of an empty SQL editor.
pub(crate) fn run_hint(cx: &App) -> SharedString {
    match action_display(&RunQuery, cx) {
        Some(keys) => format!("Write SQL, then press {keys} to run").into(),
        None => "Write SQL, then click Run".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob above brings GPUI's `test`; its expansion needs the built-in one.
    use ::core::prelude::v1::test;

    fn binding_keys(action: &dyn Action, cx: &App) -> Vec<String> {
        cx.key_bindings()
            .borrow()
            .bindings_for_action(action)
            .map(|binding| stored(binding.keystrokes()[0].inner()))
            .collect()
    }

    #[test]
    fn stored_keys_use_the_portable_primary_modifier() {
        assert_eq!(
            normalize("secondary-shift-S").as_deref(),
            Some("secondary-shift-s")
        );
        let primary = if cfg!(target_os = "macos") {
            "cmd-k"
        } else {
            "ctrl-k"
        };
        assert_eq!(normalize(primary).as_deref(), Some("secondary-k"));
        assert_eq!(normalize("alt-up").as_deref(), Some("alt-up"));
        assert_eq!(normalize("secondary-k secondary-s"), None, "no sequences");
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("secondary--").as_deref(), Some("secondary--"));
        assert_eq!(
            normalize("shift"),
            None,
            "a modifier alone is not a shortcut"
        );
    }

    #[test]
    fn command_ids_are_unique_and_every_default_parses() {
        let mut ids = std::collections::HashSet::new();
        for command in commands() {
            assert!(ids.insert(command.id), "duplicate id {}", command.id);
            assert!(
                !command.targets().is_empty(),
                "{} sends no action",
                command.id
            );
            if let DefaultKeys::Keys(keys) = command.default {
                assert!(
                    normalize(keys).is_some(),
                    "{} has an invalid default",
                    command.id
                );
            }
            for (_, context) in command.targets() {
                if let Some(context) = context {
                    assert!(
                        KeyBindingContextPredicate::parse(context).is_ok(),
                        "{} has an invalid context",
                        command.id
                    );
                }
            }
        }
    }

    #[test]
    fn scale_keys_ignore_shift_only_for_plus_and_equals() {
        let parse = |keys: &str| Keystroke::parse(keys).unwrap();
        assert!(scale_key_matches(
            &parse("secondary-="),
            &parse("secondary-+")
        ));
        assert!(scale_key_matches(
            &parse("secondary-="),
            &parse("secondary-shift-=")
        ));
        assert!(!scale_key_matches(
            &parse("secondary-="),
            &parse("alt-secondary-=")
        ));
        assert!(scale_key_matches(
            &parse("secondary--"),
            &parse("secondary--")
        ));
        assert!(!scale_key_matches(
            &parse("secondary--"),
            &parse("secondary-shift--")
        ));
        assert!(!scale_key_matches(
            &parse("secondary-k"),
            &parse("secondary-shift-k")
        ));
    }

    #[gpui_kit::test]
    fn overrides_replace_defaults_and_kit_bindings(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            super::super::init(cx);
            let find = command("find").unwrap();
            let kit_find = default_keys(find, cx).expect("GPUI Kit binds Find");
            assert_eq!(binding_keys(&Search, cx), vec![kit_find.clone()]);
            assert_eq!(
                binding_keys(&ToggleResults, cx),
                vec!["secondary-j".to_owned()]
            );
            assert!(binding_keys(&PasteConnection, cx).is_empty());

            let mut overrides = BTreeMap::new();
            assign(
                command("toggle_results").unwrap(),
                Some("alt-secondary-j".into()),
                &mut overrides,
                cx,
            );
            assign(find, Some("secondary-g".into()), &mut overrides, cx);
            assign(
                command("paste_connection").unwrap(),
                Some("secondary-shift-v".into()),
                &mut overrides,
                cx,
            );
            assign(command("quit").unwrap(), None, &mut overrides, cx);
            apply(&overrides, cx);

            assert_eq!(
                binding_keys(&ToggleResults, cx),
                vec!["secondary-alt-j".to_owned()]
            );
            assert_eq!(binding_keys(&Search, cx), vec!["secondary-g".to_owned()]);
            assert_eq!(
                binding_keys(&PasteConnection, cx),
                vec!["secondary-shift-v".to_owned()]
            );
            assert!(binding_keys(&Quit, cx).is_empty());
            assert!(is_customized(find, &overrides, cx));
            // GPUI Kit's other text bindings stay.
            assert!(!binding_keys(&gpui_kit::component::input::Paste, cx).is_empty());
            // Escape is not a command and stays.
            assert!(!binding_keys(&activity_view::CloseActivity, cx).is_empty());

            // A shortcut equal to the default is not an override.
            assign(find, Some(kit_find), &mut overrides, cx);
            assert!(!overrides.contains_key("find"));
            apply(&BTreeMap::new(), cx);
            assert_eq!(
                binding_keys(&ToggleResults, cx),
                vec!["secondary-j".to_owned()]
            );
            assert_eq!(binding_keys(&Quit, cx), vec!["secondary-q".to_owned()]);
        });
    }

    #[gpui_kit::test]
    fn the_first_binding_of_an_editor_action_is_the_users(cx: &mut gpui_kit::TestAppContext) {
        // The macOS menu bar shows the first binding of an action.
        cx.update(|cx| {
            super::super::init(cx);
            let mut overrides = BTreeMap::new();
            assign(
                command("select_all").unwrap(),
                Some("secondary-shift-a".into()),
                &mut overrides,
                cx,
            );
            apply(&overrides, cx);
            let first = cx
                .key_bindings()
                .borrow()
                .bindings_for_action(&gpui_kit::component::input::SelectAll)
                .next()
                .map(|binding| stored(binding.keystrokes()[0].inner()));
            assert_eq!(first.as_deref(), Some("secondary-shift-a"));
            // Window commands bind in the workspace context.
            let run = cx
                .key_bindings()
                .borrow()
                .bindings_for_action(&RunQuery)
                .next()
                .and_then(|binding| binding.predicate())
                .map(|predicate| predicate.to_string());
            assert_eq!(run.as_deref(), Some(WORKSPACE));
        });
    }

    #[gpui_kit::test]
    fn one_command_binds_each_of_its_targets(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            super::super::init(cx);
            let mut overrides = BTreeMap::new();
            assign(
                command("confirm_dialog").unwrap(),
                Some("secondary-s".into()),
                &mut overrides,
                cx,
            );
            apply(&overrides, cx);
            for action in [&SaveConnection as &dyn Action, &SaveSignIn, &SubmitRename] {
                assert_eq!(binding_keys(action, cx), vec!["secondary-s".to_owned()]);
            }
            // Run Query binds in the window and in the SQL editor.
            assert_eq!(
                binding_keys(&RunQuery, cx),
                vec!["secondary-enter".to_owned(), "secondary-enter".to_owned()]
            );
        });
    }
}

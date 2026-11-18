//! The Shortcuts page of Settings. Each command shows its shortcut in a
//! field. A click on the field records the next key press. Escape cancels,
//! Delete removes the shortcut. Problems show below the field in one
//! pattern: a tinted border, then an icon and one line in the same color.
//! An error (red) keeps recording, a conflict (amber) offers Replace, and a
//! note (blue) tells which command wins where two shortcuts overlap.

use super::shortcuts::{self, Command, Group, Scope};
use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Icon, Selectable, Size, StyleSized as _,
    button::ButtonGroup,
    h_flex,
    kbd::Kbd,
    label::Label,
    setting::{SettingGroup, SettingItem, SettingPage},
    v_flex,
};

/// The width of the shortcut field, as the other Settings controls.
const CONTROL_REMS: f32 = 16.;
const LINE_REMS: f32 = 1.25;

/// Which commands the page lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    Customized,
    Unassigned,
}

/// The state of the page while the Settings window is open.
#[derive(Default)]
pub(super) struct ShortcutsForm {
    filter: Filter,
    recording: Option<Recording>,
    /// Find by keys waits for a key press.
    finding: bool,
    /// The keys that Find by keys looks for.
    found: Option<String>,
    /// A note below the field of one command after a change.
    notice: Option<(&'static str, SharedString)>,
    _intercept: Option<Subscription>,
}

#[derive(Clone)]
struct Recording {
    id: &'static str,
    error: Option<SharedString>,
    /// Keys that another command in the same place uses, and that command.
    conflict: Option<(String, &'static str)>,
}

/// What a new shortcut means for the other shortcuts.
enum Check {
    Free,
    /// Another command in the same place has the keys. Replace moves them.
    Conflict(&'static Command),
    /// A fixed binding in the same place has the keys.
    Reserved(String),
    /// Both apply, and the inner place wins.
    Overlap(String),
}

#[derive(Clone, Copy)]
enum Tone {
    Error,
    Warning,
    Info,
}

impl Tone {
    fn color(self, cx: &App) -> Hsla {
        match self {
            Self::Error => cx.theme().danger,
            Self::Warning => cx.theme().warning,
            Self::Info => cx.theme().info,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Error => Icon::new(IconName::CircleX),
            Self::Warning => Icon::new(AssetIconName::TriangleAlert),
            Self::Info => Icon::new(IconName::Info),
        }
    }
}

/// The message below a field.
const MODIFIER_HINT: &str = if cfg!(target_os = "macos") {
    "Add ⌘, ⌃, or ⌥."
} else {
    "Add Ctrl, Alt, or Super."
};

/// Keys that can be a shortcut without a modifier other than Shift.
fn bare_allowed(key: &str) -> bool {
    matches!(
        key,
        "up" | "down" | "left" | "right" | "enter" | "tab" | "home" | "end" | "pageup" | "pagedown"
    ) || key
        .strip_prefix('f')
        .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

impl Qrow {
    pub(super) fn init_shortcuts_form(&mut self, cx: &mut Context<Self>) -> ShortcutsForm {
        let qrow = cx.weak_entity();
        // While a field records, key presses go to it and not to bindings,
        // so ⌘W and Escape do not close the window.
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            let handled = qrow
                .update(cx, |qrow, cx| {
                    qrow.record_keystroke(&event.keystroke, window, cx)
                })
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });
        ShortcutsForm {
            _intercept: Some(intercept),
            ..ShortcutsForm::default()
        }
    }

    fn shortcuts_form(&mut self) -> Option<&mut ShortcutsForm> {
        self.settings_form.as_mut().map(|form| &mut form.shortcuts)
    }

    /// Handle a key press while a field records or Find by keys waits.
    /// Returns true when the key press is used.
    fn record_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let in_settings = self
            .settings_window
            .is_some_and(|handle| handle == window.window_handle());
        let Some(form) = self.shortcuts_form() else {
            return false;
        };
        if form.recording.is_none() && !form.finding {
            return false;
        }
        if !in_settings {
            form.recording = None;
            form.finding = false;
            cx.notify();
            return false;
        }
        let modifiers = keystroke.modifiers;
        let plain =
            !(modifiers.platform || modifiers.control || modifiers.alt || modifiers.function);
        let key = keystroke.key.as_str();
        if form.finding {
            form.finding = false;
            if !(plain && !modifiers.shift && key == "escape") {
                form.found = Some(shortcuts::stored(keystroke));
            }
            cx.notify();
            return true;
        }
        let Some(command) = form
            .recording
            .as_ref()
            .and_then(|recording| shortcuts::command(recording.id))
        else {
            return false;
        };
        if plain && !modifiers.shift && key == "escape" {
            form.recording = None;
            cx.notify();
            return true;
        }
        if plain && !modifiers.shift && matches!(key, "backspace" | "delete") {
            form.recording = None;
            form.notice = None;
            self.set_shortcut(command, None, window, cx);
            return true;
        }
        if plain && !bare_allowed(key) {
            if let Some(recording) = &mut form.recording {
                recording.error = Some(MODIFIER_HINT.into());
                recording.conflict = None;
            }
            cx.notify();
            return true;
        }
        let keys = shortcuts::stored(keystroke);
        let check = self.check_shortcut(command, &keys, cx);
        let Some(form) = self.shortcuts_form() else {
            return true;
        };
        match check {
            Check::Conflict(other) => {
                if let Some(recording) = &mut form.recording {
                    recording.error = None;
                    recording.conflict = Some((keys, other.id));
                }
                cx.notify();
            }
            Check::Reserved(name) => {
                if let Some(recording) = &mut form.recording {
                    recording.error = Some(format!("Used by {name}.").into());
                    recording.conflict = None;
                }
                cx.notify();
            }
            Check::Free | Check::Overlap(_) => {
                form.recording = None;
                form.notice = match check {
                    Check::Overlap(text) => Some((command.id, text.into())),
                    _ => None,
                };
                self.set_shortcut(command, Some(keys), window, cx);
            }
        }
        true
    }

    /// How `keys` for `command` relate to the other shortcuts.
    fn check_shortcut(&self, command: &Command, keys: &str, cx: &App) -> Check {
        let overrides = &self.settings.shortcuts;
        let users: Vec<&'static Command> = shortcuts::commands()
            .iter()
            .filter(|other| {
                other.id != command.id
                    && shortcuts::effective(other, overrides, cx).as_deref() == Some(keys)
            })
            .collect();
        if let Some(other) = users.iter().find(|other| other.scope == command.scope) {
            return Check::Conflict(other);
        }
        let shown = shortcuts::display(keys);
        if let Some(name) = shortcuts::fixed_text_binding(keys, cx) {
            match command.scope {
                Scope::Editor => return Check::Reserved(name),
                Scope::Window => {
                    return Check::Overlap(format!(
                        "{}, {shown} runs {name}.",
                        Scope::Editor.where_phrase()
                    ));
                }
                _ => {}
            }
        }
        users
            .iter()
            .find(|other| command.scope.overlaps(other.scope))
            .map(|other| {
                // The inner place wins: a dialog over the window, the
                // message field over the editor commands.
                let inner = match (command.scope, other.scope) {
                    (Scope::Window, _) => *other,
                    (_, Scope::Window) => command,
                    (Scope::Composer, _) => command,
                    _ => *other,
                };
                Check::Overlap(format!(
                    "{}, {shown} runs {}.",
                    inner.scope.where_phrase(),
                    inner.label
                ))
            })
            .unwrap_or(Check::Free)
    }

    fn set_shortcut(
        &mut self,
        command: &Command,
        keys: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        shortcuts::assign(command, keys, &mut self.settings.shortcuts, cx);
        self.apply_shortcuts(window, cx);
    }

    /// Build the keymap again from the settings, and update text that shows
    /// a shortcut.
    pub(super) fn apply_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        shortcuts::apply(&self.settings.shortcuts, cx);
        let hint = shortcuts::run_hint(cx);
        for tab in &self.tabs {
            tab.input.update(cx, |input, cx| {
                input.set_placeholder(hint.clone(), window, cx)
            });
        }
        self.changed(cx);
    }

    fn start_recording(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.recording = Some(Recording {
                id,
                error: None,
                conflict: None,
            });
            form.finding = false;
            form.notice = None;
            cx.notify();
        }
    }

    fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.recording = None;
            cx.notify();
        }
    }

    /// Give the keys of a conflict to the recording command, and remove them
    /// from the other one.
    fn replace_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.shortcuts_form() else {
            return;
        };
        let Some((id, (keys, other))) = form
            .recording
            .take()
            .and_then(|recording| Some((recording.id, recording.conflict?)))
        else {
            return;
        };
        let (Some(command), Some(other)) = (shortcuts::command(id), shortcuts::command(other))
        else {
            return;
        };
        form.notice = Some((id, format!("{} has no shortcut now.", other.label).into()));
        shortcuts::assign(other, None, &mut self.settings.shortcuts, cx);
        self.set_shortcut(command, Some(keys), window, cx);
    }

    fn remove_shortcut(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = shortcuts::command(id) else {
            return;
        };
        if let Some(form) = self.shortcuts_form() {
            form.notice = None;
        }
        self.set_shortcut(command, None, window, cx);
    }

    fn reset_shortcut(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.notice = None;
        }
        self.settings.shortcuts.remove(id);
        self.apply_shortcuts(window, cx);
    }

    fn set_shortcut_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.filter = filter;
            cx.notify();
        }
    }

    fn find_by_keys(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.finding = !form.finding;
            form.recording = None;
            cx.notify();
        }
    }

    fn clear_found_keys(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.found = None;
            cx.notify();
        }
    }

    /// The commands whose shortcut the user changed.
    fn customized_shortcuts(&self, cx: &App) -> usize {
        shortcuts::commands()
            .iter()
            .filter(|command| shortcuts::is_customized(command, &self.settings.shortcuts, cx))
            .count()
    }

    fn reset_all_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.shortcuts.clear();
        if let Some(form) = self.shortcuts_form() {
            form.recording = None;
            form.notice = None;
        }
        self.apply_shortcuts(window, cx);
    }

    /// The Shortcuts page of Settings.
    pub(super) fn shortcuts_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let qrow = cx.weak_entity();
        let Some(form) = self.settings_form.as_ref().map(|form| &form.shortcuts) else {
            return SettingPage::new("Shortcuts");
        };
        let overrides = &self.settings.shortcuts;
        let customized = self.customized_shortcuts(cx);
        let mut groups = Vec::new();
        for group in Group::ALL {
            let items: Vec<SettingItem> = shortcuts::commands()
                .iter()
                .filter(|command| command.group == group)
                .filter_map(|command| {
                    let row = ShortcutRow::new(command, form, overrides, cx);
                    row.listed(form).then(|| row.item(qrow.clone()))
                })
                .collect();
            if !items.is_empty() {
                groups.push(SettingGroup::new().title(group.title()).items(items));
            }
        }
        if groups.is_empty() {
            let detail = match (&form.found, form.filter) {
                (Some(keys), _) => format!("No command uses {}.", shortcuts::display(keys)),
                (None, Filter::Customized) => "Every command uses its default shortcut.".into(),
                _ => "Every command has a shortcut.".into(),
            };
            groups.push(SettingGroup::new().item(SettingItem::render(
                move |_, _, cx: &mut App| {
                    v_flex()
                        .id("shortcuts-empty")
                        .test_support()
                        .py_6()
                        .gap_1()
                        .child(Label::new("No matching shortcuts").text_sm())
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(detail.clone()),
                        )
                },
            )));
        }
        let reset = qrow.clone();
        SettingPage::new("Shortcuts")
            .resettable(false)
            .on_reset_page(
                "Reset shortcuts",
                move |_| customized > 0,
                move |window, cx| {
                    let _ = reset.update(cx, |qrow, cx| {
                        qrow.confirm_page_reset(ResetPage::Shortcuts, window, cx)
                    });
                },
            )
            .header_content(filters(qrow, form.filter, form.finding, form.found.clone()))
            .groups(groups)
    }
}

/// The filters below the page title.
fn filters(
    qrow: WeakEntity<Qrow>,
    filter: Filter,
    finding: bool,
    found: Option<String>,
) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
    move |_, cx| {
        let choices = [
            (Filter::All, "All"),
            (Filter::Customized, "Customized"),
            (Filter::Unassigned, "No shortcut"),
        ];
        let group = ButtonGroup::new("shortcut-filter")
            .outline()
            .small()
            .children(choices.iter().map(|(choice, label)| {
                Button::new(SharedString::from(format!("shortcut-filter-{label}")))
                    .label(*label)
                    .selected(*choice == filter)
            }))
            .on_click({
                let qrow = qrow.clone();
                move |clicked: &Vec<usize>, _, cx| {
                    if let Some((choice, _)) = clicked.first().and_then(|index| choices.get(*index))
                    {
                        let choice = *choice;
                        let _ = qrow.update(cx, |qrow, cx| qrow.set_shortcut_filter(choice, cx));
                    }
                }
            });
        let find = match &found {
            Some(keys) => h_flex()
                .gap_1()
                .items_center()
                .children(shortcuts::keystroke(keys).map(Kbd::new))
                .child({
                    let qrow = qrow.clone();
                    Button::new("clear-found-keys")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("Clear key filter")
                        .accessibility_label("Clear key filter")
                        .on_click(move |_, _, cx| {
                            let _ = qrow.update(cx, |qrow, cx| qrow.clear_found_keys(cx));
                        })
                })
                .into_any_element(),
            None => {
                let qrow = qrow.clone();
                Button::new("find-by-keys")
                    .outline()
                    .small()
                    .icon(Icon::new(AssetIconName::Keyboard))
                    .label(if finding {
                        "Press keys…"
                    } else {
                        "Find by keys"
                    })
                    .selected(finding)
                    .on_click(move |_, _, cx| {
                        let _ = qrow.update(cx, |qrow, cx| qrow.find_by_keys(cx));
                    })
                    .into_any_element()
            }
        };
        h_flex()
            .gap_2()
            .items_center()
            .text_color(cx.theme().foreground)
            .child(group)
            .child(find)
            .into_any_element()
    }
}

/// A snapshot of one command for its row.
#[derive(Clone)]
struct ShortcutRow {
    id: &'static str,
    label: &'static str,
    keys: Option<String>,
    customized: bool,
    default_keys: Option<String>,
    recording: Option<Recording>,
    notice: Option<SharedString>,
}

impl ShortcutRow {
    fn new(
        command: &'static Command,
        form: &ShortcutsForm,
        overrides: &BTreeMap<String, Option<String>>,
        cx: &App,
    ) -> Self {
        Self {
            id: command.id,
            label: command.label,
            keys: shortcuts::effective(command, overrides, cx),
            customized: shortcuts::is_customized(command, overrides, cx),
            default_keys: shortcuts::default_keys(command, cx),
            recording: form
                .recording
                .clone()
                .filter(|recording| recording.id == command.id),
            notice: form
                .notice
                .as_ref()
                .filter(|(id, _)| *id == command.id)
                .map(|(_, text)| text.clone()),
        }
    }

    /// Whether the filters list the row. The recording row stays listed.
    fn listed(&self, form: &ShortcutsForm) -> bool {
        if self.recording.is_some() {
            return true;
        }
        let filtered = match form.filter {
            Filter::All => true,
            Filter::Customized => self.customized,
            Filter::Unassigned => self.keys.is_none(),
        };
        filtered
            && form
                .found
                .as_ref()
                .is_none_or(|found| self.keys.as_ref() == Some(found))
    }

    fn message(&self) -> Option<(Tone, SharedString)> {
        if let Some(recording) = &self.recording {
            if let Some(error) = &recording.error {
                return Some((Tone::Error, error.clone()));
            }
            if let Some((_, other)) = &recording.conflict {
                let other =
                    shortcuts::command(other).map_or("another command", |other| other.label);
                return Some((Tone::Warning, format!("Used by {other}.").into()));
            }
            return None;
        }
        self.notice.clone().map(|notice| (Tone::Info, notice))
    }

    fn item(self, qrow: WeakEntity<Qrow>) -> SettingItem {
        let mut keywords: Vec<SharedString> =
            vec![self.label.into(), "shortcut".into(), "key".into()];
        if let Some(keys) = &self.keys {
            keywords.push(shortcuts::display(keys).into());
        }
        SettingItem::render(move |_, window: &mut Window, cx: &mut App| {
            let rem = window.rem_size();
            let message = self.message();
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .input_h(Size::Medium)
                        .flex()
                        .items_center()
                        .child(Label::new(self.label).text_sm()),
                )
                .child(
                    v_flex()
                        .w(rem * CONTROL_REMS)
                        .flex_shrink_0()
                        .px_1()
                        .gap_1()
                        .child(self.field(
                            qrow.clone(),
                            message.as_ref().map(|(tone, _)| *tone),
                            cx,
                        ))
                        .children(message.map(|(tone, text)| {
                            h_flex()
                                .id(SharedString::from(format!("shortcut-message-{}", self.id)))
                                .test_support()
                                .role(Role::Alert)
                                .aria_label(text.clone())
                                .w_full()
                                .items_start()
                                .gap_1()
                                .text_sm()
                                .line_height(rems(LINE_REMS))
                                .text_color(tone.color(cx))
                                .child(
                                    div()
                                        .h(rems(LINE_REMS))
                                        .flex()
                                        .items_center()
                                        .child(tone.icon().xsmall()),
                                )
                                .child(div().flex_1().min_w_0().child(text))
                        })),
                )
        })
        .keywords(keywords)
    }

    fn field(&self, qrow: WeakEntity<Qrow>, tone: Option<Tone>, cx: &App) -> impl IntoElement {
        let id = self.id;
        let field_id = SharedString::from(format!("shortcut-{id}"));
        let recording = self.recording.is_some();
        let border = match (tone, recording) {
            (Some(tone), _) => tone.color(cx),
            (None, true) => cx.theme().ring,
            (None, false) => cx.theme().input,
        };
        let button = |name: &str, icon: Icon, label: SharedString| {
            Button::new(SharedString::from(format!("{name}-{id}")))
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(label.clone())
                .accessibility_label(label)
        };
        let muted = |text: &'static str| {
            div()
                .flex_1()
                .min_w_0()
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        let chip = |keys: &str| shortcuts::keystroke(keys).map(Kbd::new);
        let mut content = h_flex().w_full().gap_1().items_center();
        if let Some(recording) = &self.recording {
            content = match &recording.conflict {
                Some((keys, _)) => content.children(chip(keys)).child(div().flex_1()).child({
                    let qrow = qrow.clone();
                    Button::new(SharedString::from(format!("replace-shortcut-{id}")))
                        .outline()
                        .xsmall()
                        .label("Replace")
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = qrow.update(cx, |qrow, cx| qrow.replace_shortcut(window, cx));
                        })
                }),
                None => content.child(muted("Press keys…")),
            };
            let qrow = qrow.clone();
            content = content.child(
                button(
                    "cancel-shortcut",
                    Icon::new(IconName::Close),
                    "Cancel".into(),
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = qrow.update(cx, |qrow, cx| qrow.cancel_recording(cx));
                }),
            );
        } else {
            content = match &self.keys {
                Some(keys) => content.children(chip(keys)).child(div().flex_1()).child({
                    let qrow = qrow.clone();
                    // Shows on hover and keyboard focus only.
                    div()
                        .invisible()
                        .group_hover(field_id.clone(), |style| style.visible())
                        .child(
                            button(
                                "remove-shortcut",
                                Icon::new(IconName::Close),
                                "Remove shortcut".into(),
                            )
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                let _ = qrow
                                    .update(cx, |qrow, cx| qrow.remove_shortcut(id, window, cx));
                            }),
                        )
                }),
                None => content.child(muted("None")),
            };
            if self.customized {
                let label = match &self.default_keys {
                    Some(keys) => format!("Reset to {}", shortcuts::display(keys)),
                    None => "Reset to no shortcut".to_owned(),
                };
                let qrow = qrow.clone();
                content = content.child(
                    button(
                        "reset-shortcut",
                        Icon::new(AssetIconName::Undo2),
                        label.into(),
                    )
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = qrow.update(cx, |qrow, cx| qrow.reset_shortcut(id, window, cx));
                    }),
                );
            }
        }
        let label = self.label;
        h_flex()
            .id(field_id.clone())
            .test_support()
            .group(field_id)
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{label} shortcut")))
            .w_full()
            .input_h(Size::Medium)
            .pl_1p5()
            .pr_1()
            .items_center()
            .text_sm()
            .border_1()
            .border_color(border)
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .when(!recording, |field| field.cursor_pointer())
            .on_click(move |_, _, cx| {
                let _ = qrow.update(cx, |qrow, cx| {
                    let current = qrow
                        .settings_form
                        .as_ref()
                        .and_then(|form| form.shortcuts.recording.as_ref())
                        .map(|recording| recording.id);
                    if current != Some(id) {
                        qrow.start_recording(id, cx);
                    }
                });
            })
            .child(content)
    }
}

/// A Settings page that its reset button returns to the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResetPage {
    Appearance,
    Export,
    Shortcuts,
}

impl Qrow {
    /// The changed settings of `page`.
    pub(super) fn changed_on_page(&self, page: ResetPage, cx: &App) -> usize {
        let defaults = Settings::default();
        let settings = &self.settings;
        match page {
            ResetPage::Shortcuts => self.customized_shortcuts(cx),
            ResetPage::Export => [
                settings.copy_format != defaults.copy_format,
                settings.export_replay_limit_mib != defaults.export_replay_limit_mib,
            ]
            .into_iter()
            .filter(|changed| *changed)
            .count(),
            ResetPage::Appearance => [
                settings.theme != defaults.theme,
                settings.ui_scale != defaults.ui_scale,
                settings.ui_font_family != defaults.ui_font_family,
                settings.smooth_transitions != defaults.smooth_transitions,
                settings.assistant_font_family != defaults.assistant_font_family,
                settings.assistant_font_size != defaults.assistant_font_size,
                settings.assistant_line_height != defaults.assistant_line_height,
                settings.assistant_smooth_caret != defaults.assistant_smooth_caret,
                settings.assistant_smooth_resize != defaults.assistant_smooth_resize,
                settings.assistant_buffer_replies != defaults.assistant_buffer_replies,
                settings.editor_font_family != defaults.editor_font_family,
                settings.editor_font_size != defaults.editor_font_size,
                settings.editor_line_height != defaults.editor_line_height,
                settings.editor_tab_size != defaults.editor_tab_size,
                settings.editor_smooth_caret != defaults.editor_smooth_caret,
                settings.editor_smooth_scrolling != defaults.editor_smooth_scrolling,
                settings.editor_autocomplete != defaults.editor_autocomplete,
                settings.assistant.sql_keyword_case != defaults.assistant.sql_keyword_case,
                settings.logs_font_family != defaults.logs_font_family,
                settings.logs_font_size != defaults.logs_font_size,
                settings.logs_line_height != defaults.logs_line_height,
            ]
            .into_iter()
            .filter(|changed| *changed)
            .count(),
        }
    }

    /// Ask before `page` returns to its defaults: the reset cannot be undone.
    pub(super) fn confirm_page_reset(
        &mut self,
        page: ResetPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.changed_on_page(page, cx);
        if count == 0 {
            return;
        }
        let (name, things) = match page {
            ResetPage::Appearance => ("appearance", ("setting", "settings")),
            ResetPage::Export => ("export", ("setting", "settings")),
            ResetPage::Shortcuts => ("shortcuts", ("shortcut", "shortcuts")),
        };
        let description = if count == 1 {
            format!("1 changed {} returns to its default.", things.0)
        } else {
            format!("{count} changed {} return to their defaults.", things.1)
        };
        let qrow = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = qrow.clone();
            alert
                .title(format!("Reset {name}?"))
                .description(description.clone())
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-reset-page")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-reset-page")
                                .danger()
                                .label("Reset")
                                .on_click(move |_, window, cx| {
                                    let _ = confirm
                                        .update(cx, |qrow, cx| qrow.reset_page(page, window, cx));
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn reset_page(&mut self, page: ResetPage, window: &mut Window, cx: &mut Context<Self>) {
        match page {
            ResetPage::Appearance => self.reset_settings(window, cx),
            ResetPage::Export => self.reset_export_settings(cx),
            ResetPage::Shortcuts => self.reset_all_shortcuts(window, cx),
        }
    }
}

//! The Shortcuts page of Settings. Each command shows its shortcut in a
//! field. A click on the field records the next key press. Escape cancels,
//! Delete removes the shortcut. Problems show in one pattern: a tinted
//! border on the field, and an icon and one line in the same color under
//! the command name.
//! An error (red) keeps recording, a conflict (amber) offers Replace, and a
//! note (blue) tells which command wins where two shortcuts overlap.

use super::setting_row::{Tone, row_message};
use super::shortcuts::{self, Command, Group, Relation};
use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    ElementExt as _, Icon, Selectable, Size, StyleSized as _,
    button::ButtonGroup,
    h_flex,
    kbd::Kbd,
    label::Label,
    setting::{SettingGroup, SettingItem, SettingPage},
    v_flex,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// The width of the shortcut field, as the other Settings controls.
const CONTROL_REMS: f32 = 16.;

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
    /// The command changed last. Its row stays listed, so the focus stays
    /// on a field that shows.
    last: Option<&'static str>,
    /// The focus of each field. A field records only while it has the focus.
    focus: HashMap<&'static str, FocusHandle>,
    /// The focus of Find by keys while it waits.
    find_focus: Option<FocusHandle>,
    /// Whether the recording field or Find by keys showed in the last frame.
    /// The Settings window clears it before each frame, so a field on
    /// another page cannot record.
    pub(super) shown: Rc<Cell<bool>>,
    /// Whether the Shortcuts page showed in the last frame. Its header sets
    /// it, and the Settings window clears it before each frame.
    pub(super) page_shown: Rc<Cell<bool>>,
    /// Set in a frame without the Shortcuts page. A recording or Find by
    /// keys from before then is over, also before its state is cleared.
    pub(super) stale: Rc<Cell<bool>>,
    /// The press of Enter or Space on a field or Find by keys, and the
    /// control. Its release on the same control starts it, as a button does.
    /// Each Settings frame drops it when its control lost the focus.
    armed: RefCell<Option<(String, Control)>>,
    /// A key that the recorder used and that is not released yet. Its
    /// repeats do nothing.
    /// Each Settings frame drops it when no shortcut control has the focus,
    /// so a release that Settings did not see does not stay pending.
    held: RefCell<Option<String>>,
    _intercept: Option<Subscription>,
}

/// A control that Enter or Space starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Field(&'static str),
    Find,
}

#[derive(Clone)]
struct Recording {
    id: &'static str,
    error: Option<SharedString>,
    /// Keys that other commands in the same place use, and those commands.
    conflict: Option<(String, Vec<&'static str>)>,
}

/// What a new shortcut means for the other shortcuts.
enum Check {
    Free,
    /// Other commands in the same place have the keys. Replace moves them.
    Conflict(Vec<&'static Command>),
    /// A fixed binding in the same place has the keys.
    Reserved(String),
    /// Both apply, and the inner place wins.
    Overlap(String),
}

/// The message under the command name.
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
            focus: shortcuts::commands()
                .iter()
                .map(|command| (command.id, cx.focus_handle().tab_stop(true)))
                .collect(),
            find_focus: Some(cx.focus_handle()),
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
            // A field or Find by keys starts on the release of Enter or Space,
            // as a button does. Its press, and the repeats of a held key, do
            // nothing else, so they cannot reach a binding of the window.
            let key = keystroke.key.as_str();
            let activation = matches!(key, "enter" | "space") && !keystroke.modifiers.modified();
            let control = form
                .focus
                .iter()
                .find(|(_, focus)| focus.is_focused(window))
                .map(|(id, _)| Control::Field(id))
                .or_else(|| {
                    form.find_focus
                        .as_ref()
                        .is_some_and(|focus| focus.is_focused(window))
                        .then_some(Control::Find)
                });
            // A repeat of a key that the recorder just used.
            if in_settings && control.is_some() && form.held.borrow().as_deref() == Some(key) {
                return true;
            }
            if let (true, true, Some(control)) = (in_settings, activation, control) {
                *form.armed.borrow_mut() = Some((key.to_owned(), control));
                return true;
            }
            return false;
        }
        if form.stale.get() {
            form.recording = None;
            form.finding = false;
            cx.notify();
            return false;
        }
        // Only the shown control with the focus takes keys. Focus elsewhere,
        // like the search field or a dialog, ends the recording.
        let target = if form.finding {
            form.find_focus.as_ref()
        } else {
            form.recording
                .as_ref()
                .and_then(|recording| form.focus.get(recording.id))
        };
        let modifiers = keystroke.modifiers;
        let plain =
            !(modifiers.platform || modifiers.control || modifiers.alt || modifiers.function);
        let key = keystroke.key.as_str();
        let shown = form.shown.get();
        let target_focused = target.is_some_and(|focus| focus.is_focused(window));
        let on_control = shown && target_focused;
        // A button in the field, like Replace, has the focus. Escape still
        // cancels, and Tab moves between the buttons: neither reaches the
        // bindings of the window. Other keys go to the button.
        let in_control = shown
            && !target_focused
            && target.is_some_and(|focus| focus.contains_focused(window, cx));
        if in_settings && in_control {
            if plain && key == "escape" {
                let target = target.cloned();
                form.recording = None;
                form.finding = false;
                if let Some(target) = target {
                    target.focus(window, cx);
                }
                cx.notify();
                return true;
            }
            if plain && key == "tab" {
                let target = target.cloned();
                move_focus(modifiers.shift, window, cx);
                self.end_recording_outside(target, window, cx);
                return true;
            }
            return false;
        }
        if !in_settings || !on_control {
            form.recording = None;
            form.finding = false;
            cx.notify();
            return false;
        }
        // The recorder uses this key; its repeats and release do nothing more.
        *form.held.borrow_mut() = Some(key.to_owned());
        form.armed.take();
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
        // During a conflict, Tab moves to Replace and Cancel.
        let pending = form
            .recording
            .as_ref()
            .is_some_and(|recording| recording.conflict.is_some());
        if pending && plain && key == "tab" {
            let target = form
                .recording
                .as_ref()
                .and_then(|recording| form.focus.get(recording.id))
                .cloned();
            move_focus(modifiers.shift, window, cx);
            self.end_recording_outside(target, window, cx);
            return true;
        }
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
            Check::Conflict(others) => {
                if let Some(recording) = &mut form.recording {
                    recording.error = None;
                    recording.conflict =
                        Some((keys, others.iter().map(|other| other.id).collect()));
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

    /// How `keys` for `command` relate to the other shortcuts and to the
    /// fixed bindings of text fields, the grid, and the tree.
    fn check_shortcut(&self, command: &Command, keys: &str, cx: &App) -> Check {
        let overrides = &self.settings.shortcuts;
        let users: Vec<&'static Command> = shortcuts::commands()
            .iter()
            .filter(|other| {
                other.id != command.id
                    && shortcuts::effective(other, overrides, cx).as_deref() == Some(keys)
            })
            .collect();
        let shown = shortcuts::display(keys);
        // A fixed binding cannot share keys with a command: either the fixed
        // binding hides the command in its place, or the command hides it.
        for (place, name) in shortcuts::fixed_bindings_with(keys, cx) {
            if command
                .places()
                .any(|mine| mine.relation(place) != Relation::Independent)
            {
                return Check::Reserved(name);
            }
        }
        let conflicts: Vec<&'static Command> = users
            .iter()
            .copied()
            .filter(|other| command.relation(other) == Relation::Conflict)
            .collect();
        if !conflicts.is_empty() {
            return Check::Conflict(conflicts);
        }
        users
            .iter()
            .find_map(|other| match command.relation(other) {
                Relation::Overlap { inner } => {
                    let winner = if other.places().any(|place| place == inner) {
                        *other
                    } else {
                        command
                    };
                    Some(format!(
                        "{}, {shown} runs {}.",
                        inner.where_phrase(),
                        winner.label
                    ))
                }
                _ => None,
            })
            .map(Check::Overlap)
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
        if let Some(form) = self.shortcuts_form() {
            form.last = Some(command.id);
        }
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

    fn start_recording(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.recording = Some(Recording {
                id,
                error: None,
                conflict: None,
            });
            form.finding = false;
            form.notice = None;
            // It shows: the user clicked it or moved to it.
            form.shown.set(true);
            form.page_shown.set(true);
            form.stale.set(false);
            if let Some(focus) = form.focus.get(id) {
                focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    /// Cancel recording, and give the focus back to the field.
    fn cancel_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            if let Some(focus) = form
                .recording
                .take()
                .and_then(|recording| form.focus.get(recording.id))
            {
                focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    /// End a recording or Find by keys whose control did not show in the
    /// last frame, for example after a switch to another page.
    pub(super) fn drop_hidden_recording(&mut self, cx: &mut Context<Self>) {
        // A recording that started after the frame without the page stays.
        if let Some(form) = self.shortcuts_form()
            && form.stale.get()
            && (form.recording.is_some() || form.finding)
        {
            form.recording = None;
            form.finding = false;
            cx.notify();
        }
    }

    /// A key release in the Settings window. The release of an armed Enter
    /// or Space on the control that took its press starts that control.
    pub(super) fn key_released(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.shortcuts_form() else {
            return;
        };
        let key = keystroke.key.as_str();
        if form.held.borrow().as_deref() == Some(key) {
            form.held.take();
        }
        if form
            .armed
            .borrow()
            .as_ref()
            .is_none_or(|(armed, _)| armed != key)
        {
            return;
        }
        let Some((_, control)) = form.armed.take() else {
            return;
        };
        // A press with a modifier added before the release is not a click.
        if keystroke.modifiers.modified() || form.recording.is_some() || form.finding {
            return;
        }
        match control {
            Control::Field(id) => {
                if form
                    .focus
                    .get(id)
                    .is_some_and(|focus| focus.is_focused(window))
                {
                    self.start_recording(id, window, cx);
                }
            }
            Control::Find => {
                if form
                    .find_focus
                    .as_ref()
                    .is_some_and(|focus| focus.is_focused(window))
                {
                    self.find_by_keys(window, cx);
                }
            }
        }
    }

    /// End the recording when Tab moved the focus out of the field.
    fn end_recording_outside(
        &mut self,
        field: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inside = field.is_some_and(|field| field.contains_focused(window, cx));
        if !inside && let Some(form) = self.shortcuts_form() {
            form.recording = None;
            form.finding = false;
            cx.notify();
        }
    }

    /// Give the keys of a conflict to the recording command, and remove them
    /// from the commands that still use them.
    fn replace_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.shortcuts_form() else {
            return;
        };
        let Some((id, (keys, others))) = form
            .recording
            .take()
            .and_then(|recording| Some((recording.id, recording.conflict?)))
        else {
            return;
        };
        form.notice = None;
        if let Some(focus) = form.focus.get(id) {
            focus.focus(window, cx);
        }
        let Some(command) = shortcuts::command(id) else {
            return;
        };
        // The other commands can have changed since the conflict showed.
        let owners: Vec<&'static Command> = others
            .iter()
            .filter_map(|other| shortcuts::command(other))
            .filter(|other| {
                shortcuts::effective(other, &self.settings.shortcuts, cx).as_deref() == Some(&keys)
            })
            .collect();
        for other in &owners {
            shortcuts::assign(other, None, &mut self.settings.shortcuts, cx);
        }
        if !owners.is_empty()
            && let Some(form) = self.shortcuts_form()
        {
            let names = list(owners.iter().map(|other| other.label));
            let verb = if owners.len() == 1 { "has" } else { "have" };
            form.notice = Some((id, format!("{names} {verb} no shortcut now.").into()));
        }
        self.set_shortcut(command, Some(keys), window, cx);
    }

    fn remove_shortcut(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = shortcuts::command(id) else {
            return;
        };
        if let Some(form) = self.shortcuts_form() {
            form.notice = None;
            form.recording = None;
        }
        self.set_shortcut(command, None, window, cx);
    }

    /// Return a command to its default shortcut. When another command now
    /// uses the default keys, the field shows the conflict and offers Replace.
    fn reset_shortcut(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = shortcuts::command(id) else {
            return;
        };
        if let Some(form) = self.shortcuts_form() {
            form.notice = None;
            form.recording = None;
        }
        let Some(keys) = shortcuts::default_keys(command, cx) else {
            self.set_shortcut(command, None, window, cx);
            return;
        };
        match self.check_shortcut(command, &keys, cx) {
            Check::Conflict(others) => {
                self.start_recording(id, window, cx);
                if let Some(recording) = self
                    .shortcuts_form()
                    .and_then(|form| form.recording.as_mut())
                {
                    recording.conflict =
                        Some((keys, others.iter().map(|other| other.id).collect()));
                }
            }
            check => {
                if let (Check::Overlap(text), Some(form)) = (check, self.shortcuts_form()) {
                    form.notice = Some((id, text.into()));
                }
                self.set_shortcut(command, Some(keys), window, cx);
            }
        }
    }

    fn set_shortcut_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.filter = filter;
            form.recording = None;
            form.last = None;
            cx.notify();
        }
    }

    fn find_by_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.finding = !form.finding;
            form.recording = None;
            form.last = None;
            if form.finding {
                form.shown.set(true);
                form.stale.set(false);
                if let Some(focus) = &form.find_focus {
                    focus.focus(window, cx);
                }
            }
            cx.notify();
        }
    }

    fn clear_found_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.shortcuts_form() {
            form.found = None;
            form.last = None;
            if let Some(focus) = &form.find_focus {
                focus.focus(window, cx);
            }
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
            form.last = None;
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
                    row.listed(form).then(|| row.item(qrow.clone(), form))
                })
                .collect();
            if !items.is_empty() {
                groups.push(SettingGroup::new().title(group.title()).items(items));
            }
        }
        if groups.is_empty() {
            let detail = match (&form.found, form.filter) {
                (Some(keys), Filter::All) => {
                    format!("No command uses {}.", shortcuts::display(keys))
                }
                (None, Filter::Customized) => "Every command uses its default shortcut.".into(),
                (None, Filter::Unassigned) => "Every command has a shortcut.".into(),
                _ => "No command matches these filters.".into(),
            };
            // The names of all commands keep the page, and so its filters, in
            // the results of a Settings search that the filters emptied.
            let keywords: Vec<SharedString> = shortcuts::commands()
                .iter()
                .map(|command| command.label.into())
                .collect();
            groups.push(
                SettingGroup::new().item(
                    SettingItem::render(move |_, _, cx: &mut App| {
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
                    })
                    .keywords(keywords),
                ),
            );
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
            .header_content(filters(
                qrow,
                Header {
                    filter: form.filter,
                    finding: form.finding,
                    found: form.found.clone(),
                    find_focus: form.find_focus.clone(),
                    shown: form.shown.clone(),
                    page_shown: form.page_shown.clone(),
                },
            ))
            .groups(groups)
    }
}

/// The state that the filters below the page title show and share.
struct Header {
    filter: Filter,
    finding: bool,
    found: Option<String>,
    find_focus: Option<FocusHandle>,
    shown: Rc<Cell<bool>>,
    page_shown: Rc<Cell<bool>>,
}

/// The filters below the page title.
fn filters(
    qrow: WeakEntity<Qrow>,
    header: Header,
) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
    let Header {
        filter,
        finding,
        found,
        find_focus,
        shown,
        page_shown,
    } = header;
    move |_, cx| {
        page_shown.set(true);
        if finding {
            shown.set(true);
        }
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
        let find: AnyElement = match &found {
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
                        .on_click(move |_, window, cx| {
                            let _ = qrow.update(cx, |qrow, cx| qrow.clear_found_keys(window, cx));
                        })
                })
                .into_any_element(),
            None => {
                let qrow = qrow.clone();
                let button = Button::new("find-by-keys")
                    .outline()
                    .small()
                    .icon(Icon::new(AssetIconName::Keyboard))
                    .label(if finding {
                        "Press keys…"
                    } else {
                        "Find by keys"
                    })
                    .selected(finding)
                    .on_click(move |_, window, cx| {
                        let _ = qrow.update(cx, |qrow, cx| qrow.find_by_keys(window, cx));
                    });
                button.into_any_element()
            }
        };
        // One focus for both states, so the focus stays on a control that
        // shows when Find by keys ends.
        let find = match &find_focus {
            Some(focus) => {
                // Clear key filter gives the focus to this wrapper; Enter or
                // Space on it starts Find by keys, like a press of its button.
                div().track_focus(focus).child(find).into_any_element()
            }
            None => find,
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

impl ShortcutsForm {
    fn control_focused(&self, control: Control, window: &Window) -> bool {
        match control {
            Control::Field(id) => self
                .focus
                .get(id)
                .is_some_and(|focus| focus.is_focused(window)),
            Control::Find => self
                .find_focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window)),
        }
    }

    /// Drop a pending press or a held key whose control lost the focus, or
    /// when the window is not active: Settings may not see their release.
    pub(super) fn drop_stale_keys(&self, window: &Window) {
        if !window.is_window_active() {
            self.armed.take();
            self.held.take();
            return;
        }
        let armed_lost = self
            .armed
            .borrow()
            .as_ref()
            .is_some_and(|(_, control)| !self.control_focused(*control, window));
        if armed_lost {
            self.armed.take();
        }
        let on_control = self.focus.values().any(|focus| focus.is_focused(window))
            || self
                .find_focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window));
        if !on_control {
            self.held.take();
        }
    }

    /// The control that records or waits for Find by keys.
    pub(super) fn active_focus(&self) -> Option<&FocusHandle> {
        if self.finding {
            return self.find_focus.as_ref();
        }
        self.recording
            .as_ref()
            .and_then(|recording| self.focus.get(recording.id))
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
    focus: Option<FocusHandle>,
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
                .filter(|recording| recording.id == command.id && !form.stale.get()),
            notice: form
                .notice
                .as_ref()
                .filter(|(id, _)| *id == command.id)
                .map(|(_, text)| text.clone()),
            focus: form.focus.get(command.id).cloned(),
        }
    }

    /// Whether the filters list the row. The recording row stays listed.
    fn listed(&self, form: &ShortcutsForm) -> bool {
        if self.recording.is_some() || form.last == Some(self.id) {
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
            if let Some((_, others)) = &recording.conflict {
                let names = list(
                    others
                        .iter()
                        .filter_map(|other| shortcuts::command(other))
                        .map(|other| other.label),
                );
                return Some((Tone::Warning, format!("Used by {names}.").into()));
            }
            return None;
        }
        self.notice.clone().map(|notice| (Tone::Info, notice))
    }

    fn item(self, qrow: WeakEntity<Qrow>, form: &ShortcutsForm) -> SettingItem {
        let shown = form.shown.clone();
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
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(Label::new(self.label).text_sm())
                        .children(message.as_ref().map(|(tone, text)| {
                            div().mt_0p5().child(row_message(
                                format!("shortcut-message-{}", self.id),
                                *tone,
                                text.clone(),
                                cx,
                            ))
                        })),
                )
                .child(
                    div()
                        .w(rem * CONTROL_REMS)
                        .flex_shrink_0()
                        .px_1()
                        .child(self.field(
                            qrow.clone(),
                            message.as_ref().map(|(tone, _)| *tone),
                            shown.clone(),
                            window,
                            cx,
                        )),
                )
        })
        .keywords(keywords)
    }

    fn field(
        &self,
        qrow: WeakEntity<Qrow>,
        tone: Option<Tone>,
        shown: Rc<Cell<bool>>,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement {
        let id = self.id;
        let field_id = SharedString::from(format!("shortcut-{id}"));
        let recording = self.recording.is_some();
        let focused = self
            .focus
            .as_ref()
            .is_some_and(|focus| focus.contains_focused(window, cx));
        // At rest the field is only its keys. It draws its box on hover, on
        // focus, and while it records; a problem tints the box.
        let boxed = recording || focused;
        let border = match (tone.filter(|_| recording), boxed) {
            (Some(tone), _) => tone.color(cx),
            (None, true) => cx.theme().ring,
            (None, false) => transparent_black(),
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
                .min_w_0()
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        let chip = |keys: &str| shortcuts::keystroke(keys).map(Kbd::new);
        // The keys stay at the start of the field in every state, and the
        // buttons follow them.
        let mut content = h_flex().w_full().gap_1().items_center();
        if let Some(recording) = &self.recording {
            content = match &recording.conflict {
                Some((keys, _)) => content.children(chip(keys)).child({
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
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = qrow.update(cx, |qrow, cx| qrow.cancel_recording(window, cx));
                }),
            );
        } else {
            content = match &self.keys {
                Some(keys) => content.children(chip(keys)).child({
                    let qrow = qrow.clone();
                    // Shows on hover and keyboard focus only.
                    div()
                        .when(!focused, |remove| remove.invisible())
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
        let value: SharedString = self
            .keys
            .as_deref()
            .map_or("None".into(), |keys| shortcuts::display(keys).into());
        let start = {
            move |window: &mut Window, cx: &mut App| {
                let _ = qrow.update(cx, |qrow, cx| {
                    let current = qrow
                        .settings_form
                        .as_ref()
                        .and_then(|form| form.shortcuts.recording.as_ref())
                        .map(|recording| recording.id);
                    if current != Some(id) {
                        qrow.start_recording(id, window, cx);
                    }
                });
            }
        };
        h_flex()
            .id(field_id.clone())
            .test_support()
            .group(field_id)
            .when_some(self.focus.as_ref(), |field, focus| field.track_focus(focus))
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{label} shortcut")))
            .aria_value(value)
            // Enter or Space records on its release, like a click; see
            // `key_released`.
            .w_full()
            .input_h(Size::Medium)
            .pl_1p5()
            .pr_1()
            .items_center()
            .text_sm()
            .border_1()
            .border_color(border)
            .rounded(cx.theme().radius)
            .when(boxed, |field| field.bg(cx.theme().background))
            .when(!boxed, |field| {
                field.hover(|style| style.border_color(cx.theme().input))
            })
            .when(!recording, |field| field.cursor_pointer())
            .on_click(move |_, window, cx| start(window, cx))
            // A recording field takes keys only while it is in view.
            .relative()
            .when(recording, |field| {
                field.on_prepaint(move |bounds, window, _| {
                    if window.content_mask().bounds.intersects(&bounds) {
                        shown.set(true);
                    }
                })
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
        // The dialog covers the page: a recording on it ends. The field gets
        // the focus first, so the dialog returns the focus to a control that
        // stays, not to a button of the recording.
        if let Some(form) = self.shortcuts_form() {
            if let Some(recording) = form.recording.take() {
                if let Some(focus) = form.focus.get(recording.id) {
                    focus.focus(window, cx);
                }
                form.last = Some(recording.id);
            }
            form.finding = false;
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
                                    // The dialog returns the focus when it
                                    // closes; the reset can remove that
                                    // control, so the window takes the focus.
                                    window.close_dialog(cx);
                                    let _ = confirm.update(cx, |qrow, cx| {
                                        // A filtered Shortcuts list can drop
                                        // the row with the focus.
                                        let filtered = page == ResetPage::Shortcuts
                                            && qrow.settings_form.as_ref().is_some_and(|form| {
                                                form.shortcuts.filter != Filter::All
                                                    || form.shortcuts.found.is_some()
                                            });
                                        qrow.reset_page(page, window, cx);
                                        if filtered
                                            && let Some(focus) = qrow
                                                .settings_form
                                                .as_ref()
                                                .and_then(|form| form.root_focus.clone())
                                        {
                                            focus.focus(window, cx);
                                        }
                                    });
                                }),
                        ),
                )
        });
        cx.notify();
    }

    fn reset_page(&mut self, page: ResetPage, window: &mut Window, cx: &mut Context<Self>) {
        match page {
            ResetPage::Appearance => self.reset_settings(window, cx),
            ResetPage::Export => self.reset_export_settings(window, cx),
            ResetPage::Shortcuts => self.reset_all_shortcuts(window, cx),
        }
    }
}

/// "A", "A and B", or "A, B, and C".
fn list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<&str> = names.collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Move the focus to the next or the previous control, as Tab does.
fn move_focus(back: bool, window: &mut Window, cx: &mut App) {
    if back {
        window.focus_prev(cx);
    } else {
        window.focus_next(cx);
    }
}

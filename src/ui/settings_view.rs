use super::setting_row::Rows;
use super::*;
use crate::model::{
    ASSISTANT_DATA_SHARING_NOTICE_VERSION, AssistantExecutionMode, MAX_TAB_SIZE, MIN_TAB_SIZE,
};
use crate::sql::KeywordCase;
use crate::themes;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    IndexPath, h_flex,
    input::{NumberInputEvent, StepAction},
    label::Label,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    setting::{RenderOptions, SettingGroup, SettingItem, SettingPage, Settings as SettingsPanel},
    switch::Switch,
    v_flex,
};
use std::cell::{Cell, RefCell};

const DIALOG_REMS: f32 = 56.;
const DIALOG_HEIGHT_REMS: f32 = 44.;
const SIDEBAR_REMS: f32 = 11.;
const SIDEBAR_MIN_REMS: f32 = 8.;
const SIDEBAR_MAX_REMS: f32 = 18.;
/// Width of the control column while a page keeps label and control side by
/// side. A stacked page gives the control the full width instead.
const CONTROL_REMS: f32 = 16.;

type SettingSelect = Entity<SelectState<SearchableVec<String>>>;
const SYSTEM_FONT_LABEL: &str = "System Font";

/// A setting the dialog edits with a stepper. The variants carry their own
/// units, bounds, and formatting so that one control and one commit path serve
/// all of them.
#[derive(Clone, Copy, PartialEq)]
enum NumberSetting {
    Scale,
    EditorFontSize,
    EditorLineHeight,
    EditorTabSize,
    LogsFontSize,
    LogsLineHeight,
    AssistantFontSize,
    AssistantLineHeight,
}

impl NumberSetting {
    const ALL: [Self; 8] = [
        Self::Scale,
        Self::EditorFontSize,
        Self::EditorLineHeight,
        Self::EditorTabSize,
        Self::LogsFontSize,
        Self::LogsLineHeight,
        Self::AssistantFontSize,
        Self::AssistantLineHeight,
    ];

    /// The value as the dialog shows it. The scale is a percentage; the others
    /// keep the unit they are stored in.
    fn value(self, settings: &Settings) -> f32 {
        match self {
            Self::Scale => settings.ui_scale * 100.,
            Self::EditorFontSize => settings.editor_font_size,
            Self::EditorLineHeight => settings.editor_line_height,
            Self::EditorTabSize => f32::from(settings.editor_tab_size),
            Self::LogsFontSize => settings.logs_font_size,
            Self::LogsLineHeight => settings.logs_line_height,
            Self::AssistantFontSize => settings.assistant_font_size,
            Self::AssistantLineHeight => settings.assistant_line_height,
        }
    }

    /// Minimum, maximum, and step, in the displayed unit.
    fn range(self) -> (f32, f32, f32) {
        match self {
            Self::Scale => (
                MIN_UI_SCALE * 100.,
                MAX_UI_SCALE * 100.,
                UI_SCALE_STEP * 100.,
            ),
            Self::EditorFontSize | Self::LogsFontSize | Self::AssistantFontSize => {
                (MIN_EDITOR_FONT_SIZE, MAX_EDITOR_FONT_SIZE, 1.)
            }
            Self::EditorLineHeight | Self::LogsLineHeight | Self::AssistantLineHeight => {
                (MIN_LINE_HEIGHT, MAX_LINE_HEIGHT, LINE_HEIGHT_STEP)
            }
            Self::EditorTabSize => (f32::from(MIN_TAB_SIZE), f32::from(MAX_TAB_SIZE), 1.),
        }
    }

    /// Line heights carry one decimal. The others are whole numbers.
    fn fractional(self) -> bool {
        matches!(
            self,
            Self::EditorLineHeight | Self::LogsLineHeight | Self::AssistantLineHeight
        )
    }

    fn format(self, value: f32) -> String {
        if self.fractional() {
            format!("{value:.1}")
        } else {
            format!("{value:.0}")
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::Scale => "%",
            Self::EditorFontSize | Self::LogsFontSize | Self::AssistantFontSize => "px",
            Self::EditorLineHeight | Self::LogsLineHeight | Self::AssistantLineHeight => "x",
            Self::EditorTabSize => "",
        }
    }

    /// Accessibility label. A group title is not part of the accessible name,
    /// so each control names its own scope.
    fn label(self) -> &'static str {
        match self {
            Self::Scale => "UI Scale",
            Self::EditorFontSize => "Editor Font Size",
            Self::EditorLineHeight => "Editor Line Height",
            Self::EditorTabSize => "Editor Tab Size",
            Self::LogsFontSize => "Logs Font Size",
            Self::LogsLineHeight => "Logs Line Height",
            Self::AssistantFontSize => "Assistant Font Size",
            Self::AssistantLineHeight => "Assistant Line Height",
        }
    }
}

/// A font family the dialog edits with a searchable select.
#[derive(Clone, Copy, PartialEq)]
enum FontSetting {
    Ui,
    Editor,
    Logs,
    Assistant,
}

impl FontSetting {
    const ALL: [Self; 4] = [Self::Ui, Self::Editor, Self::Logs, Self::Assistant];

    fn family(self, settings: &Settings) -> &str {
        match self {
            Self::Ui => &settings.ui_font_family,
            Self::Editor => &settings.editor_font_family,
            Self::Logs => &settings.logs_font_family,
            Self::Assistant => &settings.assistant_font_family,
        }
    }

    /// The option the select shows. The system font identifier needs a readable
    /// entry in every font selector.
    fn selected(self, settings: &Settings) -> String {
        let family = self.family(settings);
        if family == SYSTEM_FONT_FAMILY {
            SYSTEM_FONT_LABEL.to_owned()
        } else {
            family.to_owned()
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ui => "UI Font Family",
            Self::Editor => "Editor Font Family",
            Self::Logs => "Logs Font Family",
            Self::Assistant => "Assistant Font Family",
        }
    }
}

fn font_options(fonts: &[String]) -> Vec<String> {
    let mut options: Vec<String> = fonts
        .iter()
        .filter(|font| font.as_str() != SYSTEM_FONT_FAMILY && font.as_str() != SYSTEM_FONT_LABEL)
        .cloned()
        .collect();
    options.insert(0, SYSTEM_FONT_LABEL.into());
    options
}

fn font_family_for_selection(value: String) -> String {
    if value == SYSTEM_FONT_LABEL {
        SYSTEM_FONT_FAMILY.to_owned()
    } else {
        value
    }
}

pub(super) struct SettingsForm {
    theme: SettingSelect,
    /// Each stepper with the value it last displayed, so that a change made
    /// outside the dialog can be pushed back into the input.
    numbers: Vec<(NumberSetting, Entity<InputState>, Cell<f32>)>,
    /// The last value sent from Qrow to each picker. Search and keyboard
    /// navigation can change a picker's transient state before confirmation.
    fonts: Vec<(FontSetting, SettingSelect, RefCell<String>)>,
    assistant_mode: SettingSelect,
    assistant_keyword_case: SettingSelect,
    assistant_executable: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsForm {
    fn number(&self, setting: NumberSetting) -> &Entity<InputState> {
        &self
            .numbers
            .iter()
            .find(|(candidate, ..)| *candidate == setting)
            .expect("the form holds every number setting")
            .1
    }

    fn font(&self, setting: FontSetting) -> &SettingSelect {
        &self
            .fonts
            .iter()
            .find(|(candidate, ..)| *candidate == setting)
            .expect("the form holds every font setting")
            .1
    }
}

impl Qrow {
    pub(super) fn init_settings_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut subscriptions = Vec::new();
        let theme = cx.new(|cx| {
            SelectState::new(SearchableVec::new(themes::options(cx)), None, window, cx)
                .searchable(true)
        });
        subscriptions.push(cx.subscribe_in(
            &theme,
            window,
            move |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.set_theme(value.clone(), window, cx);
                }
            },
        ));
        let mut numbers = Vec::new();
        for setting in NumberSetting::ALL {
            let value = setting.value(&self.settings);
            let input =
                cx.new(|cx| InputState::new(window, cx).default_value(setting.format(value)));
            // Handle steps here instead of InputState's default one-unit step.
            input.update(cx, |input, cx| input.set_step(None, window, cx));
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    let direction = if *action == StepAction::Increment {
                        1.
                    } else {
                        -1.
                    };
                    this.commit_number_setting(input, setting, direction, window, cx);
                },
            ));
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_number_setting(input, setting, 0., window, cx);
                    } else if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                },
            ));
            numbers.push((setting, input, Cell::new(value)));
        }
        let options = font_options(&self.fonts);
        let mut fonts = Vec::new();
        for setting in FontSetting::ALL {
            let selected = setting.selected(&self.settings);
            let selected_index = options.iter().position(|option| option == &selected);
            let select = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(options.clone()),
                    selected_index.map(|index| IndexPath::default().row(index)),
                    window,
                    cx,
                )
                .searchable(true)
            });
            subscriptions.push(cx.subscribe_in(
                &select,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.set_font(setting, value.clone(), window, cx);
                    }
                },
            ));
            fonts.push((setting, select, RefCell::new(selected)));
        }
        let mode = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![
                    "Ask before running".into(),
                    "Run automatically".into(),
                ]),
                Some(IndexPath::default().row(usize::from(
                    self.settings.assistant.default_execution_mode
                        == AssistantExecutionMode::RunAutomatically,
                ))),
                window,
                cx,
            )
        });
        subscriptions.push(cx.subscribe_in(
            &mode,
            window,
            |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.set_default_assistant_mode(value == "Run automatically", window, cx);
                }
            },
        ));
        let keyword_case = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(
                    [KeywordCase::Uppercase, KeywordCase::Lowercase]
                        .map(|case| keyword_case_label(case).to_owned())
                        .to_vec(),
                ),
                Some(IndexPath::default().row(usize::from(
                    self.settings.assistant.sql_keyword_case == KeywordCase::Lowercase,
                ))),
                window,
                cx,
            )
        });
        subscriptions.push(cx.subscribe_in(
            &keyword_case,
            window,
            |this, _, event: &SelectEvent<SearchableVec<String>>, _, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    let case = if value == keyword_case_label(KeywordCase::Lowercase) {
                        KeywordCase::Lowercase
                    } else {
                        KeywordCase::Uppercase
                    };
                    if this.settings.assistant.sql_keyword_case != case {
                        this.settings.assistant.sql_keyword_case = case;
                        this.changed(cx);
                    }
                }
            },
        ));
        let executable = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Automatic")
                .default_value(
                    self.settings
                        .assistant
                        .codex_executable
                        .clone()
                        .unwrap_or_default(),
                )
        });
        subscriptions.push(cx.subscribe_in(
            &executable,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().trim().to_owned();
                    let value = (!value.is_empty()).then_some(value);
                    if this.settings.assistant.codex_executable != value {
                        this.settings.assistant.codex_executable = value;
                        this.changed(cx);
                    }
                }
            },
        ));
        self.settings_form = Some(SettingsForm {
            theme,
            numbers,
            fonts,
            assistant_mode: mode,
            assistant_keyword_case: keyword_case,
            assistant_executable: executable,
            _subscriptions: subscriptions,
        });
    }

    fn commit_number_setting(
        &mut self,
        input: &Entity<InputState>,
        setting: NumberSetting,
        direction: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (min, max, step) = setting.range();
        let current = setting.value(&self.settings);
        let value = input
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .unwrap_or(current);
        // Step on the tenth a line height displays, so that the value stays on
        // the grid the field shows.
        let value = if setting.fractional() {
            ((value * 10.).round() + direction * step * 10.).round() / 10.
        } else {
            value.round() + direction * step
        }
        .clamp(min, max);
        let displayed = setting.format(value);
        input.update(cx, |input, cx| input.set_value(displayed, window, cx));
        self.apply_number_setting(setting, value, window, cx);
    }

    fn apply_number_setting(
        &mut self,
        setting: NumberSetting,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match setting {
            NumberSetting::Scale => self.apply_ui_scale(value / 100., window, cx),
            NumberSetting::EditorFontSize if self.settings.editor_font_size != value => {
                self.settings.editor_font_size = value;
                self.changed(cx);
            }
            NumberSetting::EditorLineHeight if self.settings.editor_line_height != value => {
                self.settings.editor_line_height = value;
                self.changed(cx);
            }
            NumberSetting::EditorTabSize => self.set_editor_tab_size(value as u8, cx),
            NumberSetting::LogsFontSize if self.settings.logs_font_size != value => {
                self.settings.logs_font_size = value;
                self.changed(cx);
            }
            NumberSetting::LogsLineHeight if self.settings.logs_line_height != value => {
                self.settings.logs_line_height = value;
                self.changed(cx);
            }
            NumberSetting::AssistantFontSize if self.settings.assistant_font_size != value => {
                self.settings.assistant_font_size = value;
                self.changed(cx);
            }
            NumberSetting::AssistantLineHeight if self.settings.assistant_line_height != value => {
                self.settings.assistant_line_height = value;
                self.changed(cx);
            }
            _ => {}
        }
    }

    fn set_font(
        &mut self,
        setting: FontSetting,
        font: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let font = font_family_for_selection(font);
        match setting {
            FontSetting::Ui => self.set_ui_font(font, window, cx),
            FontSetting::Editor => self.set_editor_font(font, cx),
            FontSetting::Logs => self.set_logs_font(font, cx),
            FontSetting::Assistant => self.set_assistant_font(font, cx),
        }
    }

    fn set_assistant_enabled(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !enabled {
            self.settings.assistant.enabled = false;
            set_menus(cx, false);
            self.assistant_state.open = false;
            self.reset_assistant_runs(window, cx);
            self.assistant_state.runs.clear();
            self.assistant_state.snapshot = None;
            self.assistant_state.transcripts.clear();
            self.assistant_state.older_cursors.clear();
            self.assistant_state.loaded_cursors.clear();
            self.assistant_state.notice = None;
            self.assistant_state.status = super::assistant_view::Status::Idle;
            self.assistant_state.stop();
            self.changed(cx);
            return;
        }
        if self.settings.assistant.data_sharing_notice_version
            == ASSISTANT_DATA_SHARING_NOTICE_VERSION
        {
            self.settings.assistant.enabled = true;
            set_menus(cx, true);
            self.changed(cx);
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            alert.title("Enable the assistant?")
                .description("Qrow sends selected SQL, allowed connection and tab details, the assistant notes of connections, the cached names and comments of schemas, tables, views, and columns, and the descriptions, tests, and lineage of dbt projects to Codex. It sends them only when you send a message or when the assistant reads them with a tool. Codex is a separate installation and keeps conversation history locally. Demo threads can remain in Codex after a crash.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-enable-assistant").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-enable-assistant").primary().label("Enable")
                        .on_click(move |_, window, cx| {
                            let _ = confirm.update(cx, |this, cx| {
                                this.settings.assistant.data_sharing_notice_version = ASSISTANT_DATA_SHARING_NOTICE_VERSION;
                                this.settings.assistant.enabled = true;
                                set_menus(cx, true);
                                this.changed(cx);
                            });
                            window.close_dialog(cx);
                        })))
        });
        cx.notify();
    }

    fn set_default_assistant_mode(
        &mut self,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !automatic {
            self.settings.assistant.default_execution_mode =
                AssistantExecutionMode::AskBeforeRunning;
            self.changed(cx);
            return;
        }
        if self.settings.assistant.default_execution_mode
            == AssistantExecutionMode::RunAutomatically
        {
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            alert.title("Run assistant queries automatically?")
                .description("The assistant can run SQL that changes or deletes data and schema. Qrow cannot confirm that a statement is read-only.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-default-auto-run").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-default-auto-run").with_variant(ButtonVariant::Danger).label("Run automatically")
                        .on_click(move |_, window, cx| {
                            let _ = confirm.update(cx, |this, cx| {
                                this.settings.assistant.default_execution_mode = AssistantExecutionMode::RunAutomatically;
                                this.changed(cx);
                            });
                            window.close_dialog(cx);
                        })))
        });
        cx.notify();
    }

    pub(super) fn open_settings_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let body = weak.clone();
            let footer = weak.update(cx, |this, cx| this.settings_footer(cx)).ok();
            let (width, height) = settings_dialog_size(window);
            let viewport = window.viewport_size();
            dialog
                .title("Settings")
                .w(width)
                .h(height)
                .margin_top((viewport.height - height) / 2.)
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .content(move |content, window, cx| {
                    let panel = body
                        .update(cx, |this, cx| this.settings_content(window, cx))
                        .ok();
                    content.children(panel)
                })
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.settings_open = false;
                        this.settings_form = None;
                        cx.notify();
                    });
                })
        });
    }

    fn settings_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.settings_form else {
            return div().into_any_element();
        };
        // Keep the controls in sync with the scale shortcuts and Restore
        // defaults.
        for (setting, input, displayed) in &form.numbers {
            let value = setting.value(&self.settings);
            if displayed.get() != value {
                displayed.set(value);
                let text = setting.format(value);
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
        let theme = self.settings.theme.clone();
        if form.theme.read(cx).selected_value() != Some(&theme) {
            form.theme
                .update(cx, |state, cx| state.set_selected_value(&theme, window, cx));
        }
        for (setting, select, displayed) in &form.fonts {
            let selected = setting.selected(&self.settings);
            if *displayed.borrow() != selected {
                *displayed.borrow_mut() = selected.clone();
                select.update(cx, |state, cx| {
                    state.set_selected_value(&selected, window, cx)
                });
            }
        }
        let selected_mode = if self.settings.assistant.default_execution_mode
            == AssistantExecutionMode::RunAutomatically
        {
            "Run automatically"
        } else {
            "Ask before running"
        };
        if form
            .assistant_mode
            .read(cx)
            .selected_value()
            .map(String::as_str)
            != Some(selected_mode)
        {
            form.assistant_mode.update(cx, |state, cx| {
                state.set_selected_value(&selected_mode.to_owned(), window, cx)
            });
        }
        let keyword_case = keyword_case_label(self.settings.assistant.sql_keyword_case);
        if form
            .assistant_keyword_case
            .read(cx)
            .selected_value()
            .map(String::as_str)
            != Some(keyword_case)
        {
            form.assistant_keyword_case.update(cx, |state, cx| {
                state.set_selected_value(&keyword_case.to_owned(), window, cx)
            });
        }
        // Match the gap the dialog leaves above the footer, which the dialog
        // adds to the smaller gap below the title.
        div()
            .size_full()
            .mt_2()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                settings_panel("settings", window, cx)
                    .page(appearance_page(form))
                    .page(assistant_page(
                        form,
                        cx.weak_entity(),
                        self.settings.assistant.enabled,
                    )),
            )
            .into_any_element()
    }

    fn settings_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .w_full()
            .gap_2()
            .child(
                Button::new("reset-settings")
                    .label("Restore defaults")
                    .on_click(cx.listener(|this, _, window, cx| this.reset_settings(window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("save-settings")
                    .primary()
                    .label("Save")
                    .on_click(cx.listener(|this, _, window, cx| {
                        // Commit pending numeric edits before disposing their subscriptions.
                        let pending: Vec<_> = this
                            .settings_form
                            .iter()
                            .flat_map(|form| form.numbers.iter())
                            .map(|(setting, input, _)| (*setting, input.clone()))
                            .collect();
                        for (setting, input) in pending {
                            this.commit_number_setting(&input, setting, 0., window, cx);
                        }
                        if let Some(form) = &this.settings_form {
                            let path = form.assistant_executable.read(cx).value().trim().to_owned();
                            let path = (!path.is_empty()).then_some(path);
                            if this.settings.assistant.codex_executable != path {
                                this.settings.assistant.codex_executable = path;
                                this.changed(cx);
                            }
                        }
                        // Programmatic close_dialog does not invoke Dialog::on_close.
                        this.settings_open = false;
                        this.settings_form = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

/// Appearance settings for the interface, assistant messages, editor, and logs.
fn appearance_page(form: &SettingsForm) -> SettingPage {
    SettingPage::new("Appearance")
        .default_open(true)
        // The footer resets all appearance settings, so this page does not
        // need a second reset action.
        .resettable(false)
        .group(
            SettingGroup::new()
                .title("Interface")
                .item(setting_item(
                    "Theme",
                    "Colors of the interface. System follows macOS.",
                    &["appearance", "colors", "dark", "light"],
                    theme_field(form),
                ))
                .item(setting_item(
                    "Scale",
                    "Size of all text and controls. Also ⌘+ and ⌘−.",
                    &["zoom", "interface", "ui"],
                    number_field(form, NumberSetting::Scale),
                ))
                .item(setting_item(
                    "Font Family",
                    "Font of controls, labels, and result tables.",
                    &["interface", "ui", "typeface"],
                    font_field(form, FontSetting::Ui),
                )),
        )
        .group(
            SettingGroup::new()
                .title("Assistant")
                .item(setting_item(
                    "Font Family",
                    "Font of messages in assistant conversations.",
                    &["assistant", "messages", "typeface"],
                    font_field(form, FontSetting::Assistant),
                ))
                .item(setting_item(
                    "Font Size",
                    "Size of message text, in pixels.",
                    &["assistant", "messages"],
                    number_field(form, NumberSetting::AssistantFontSize),
                ))
                .item(setting_item(
                    "Line Height",
                    "Space between message lines, times the font size.",
                    &["assistant", "messages", "spacing"],
                    number_field(form, NumberSetting::AssistantLineHeight),
                )),
        )
        .group(
            SettingGroup::new()
                .title("Editor")
                .item(setting_item(
                    "Font Family",
                    "Font of SQL in the editor. Use a monospace font.",
                    &["editor", "sql", "typeface"],
                    font_field(form, FontSetting::Editor),
                ))
                .item(setting_item(
                    "Font Size",
                    "Size of SQL text, in pixels.",
                    &["editor", "sql"],
                    number_field(form, NumberSetting::EditorFontSize),
                ))
                .item(setting_item(
                    "Line Height",
                    "Space between SQL lines, times the font size.",
                    &["editor", "sql", "spacing"],
                    number_field(form, NumberSetting::EditorLineHeight),
                ))
                .item(setting_item(
                    "Tab Size",
                    "Spaces for each indent level.",
                    &["editor", "sql", "indent", "spaces", "format"],
                    number_field(form, NumberSetting::EditorTabSize),
                ))
                .item(setting_item(
                    "SQL Keyword Case",
                    "Keyword case of the SQL that the assistant writes.",
                    &[
                        "editor",
                        "sql",
                        "style",
                        "format",
                        "uppercase",
                        "lowercase",
                        "assistant",
                    ],
                    select_field(&form.assistant_keyword_case, "SQL Keyword Case"),
                )),
        )
        .group(
            SettingGroup::new()
                .title("Logs")
                .item(setting_item(
                    "Font Family",
                    "Font of log entries.",
                    &["logs", "typeface"],
                    font_field(form, FontSetting::Logs),
                ))
                .item(setting_item(
                    "Font Size",
                    "Size of log text, in pixels.",
                    &["logs"],
                    number_field(form, NumberSetting::LogsFontSize),
                ))
                .item(setting_item(
                    "Line Height",
                    "Space between log lines, times the font size.",
                    &["logs", "spacing"],
                    number_field(form, NumberSetting::LogsLineHeight),
                )),
        )
}

fn assistant_page(form: &SettingsForm, owner: WeakEntity<Qrow>, enabled: bool) -> SettingPage {
    let executable = form.assistant_executable.clone();
    SettingPage::new("Assistant")
        .resettable(false)
        .group(
            SettingGroup::new()
                .title("General")
                .item(setting_item(
                    "Enabled",
                    "Shows the assistant button and turns on ⌘J.",
                    &["assistant", "enable", "codex", "ai"],
                    move |_: &mut Window, _: &mut App| {
                        let owner = owner.clone();
                        Switch::new("enable-assistant")
                            .checked(enabled)
                            .accessibility_label("Enable assistant")
                            .on_click(move |next, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.set_assistant_enabled(*next, window, cx)
                                });
                            })
                    },
                ))
                .item(setting_item(
                    "Query Execution",
                    "How new conversations run assistant queries.",
                    &["assistant", "run", "approval", "mode", "automatically"],
                    select_field(&form.assistant_mode, "Assistant Query Execution"),
                )),
        )
        .group(SettingGroup::new().title("Codex").item(setting_item(
            "Executable",
            "Empty searches your PATH and Homebrew folders.",
            &["assistant", "codex", "path", "binary"],
            move |_: &mut Window, _: &mut App| {
                Input::new(&executable)
                    .focus_ring(false)
                    .id(setting_id("Codex Executable"))
                    .w_full()
                    .aria_label("Codex Executable")
            },
        )))
}

fn keyword_case_label(case: KeywordCase) -> &'static str {
    match case {
        KeywordCase::Uppercase => "Uppercase",
        KeywordCase::Lowercase => "Lowercase",
    }
}

/// One setting: the title and description at the left and the control at the
/// right, or the control below them on a stacked page. A page that keeps the
/// label beside the control aligns every control on one width.
///
/// Kit's own item row does not let the label column shrink. GPUI measures the
/// minimum width of text as one unwrapped line, so a long description holds
/// the column wide and pushes the control past the page edge. Here the column
/// shrinks, and the description wraps instead. Search matches the title and
/// description through the keywords, as it does for Kit's row.
fn setting_item<E: IntoElement>(
    title: &'static str,
    description: &'static str,
    keywords: &[&'static str],
    field: impl Fn(&mut Window, &mut App) -> E + 'static,
) -> SettingItem {
    setting_row(title.into(), description.into(), keywords, false, field)
}

/// A setting row like [`setting_item`], with a description that can change.
/// A `stacked` row puts its control below the label at the full width, for
/// controls that need more room than the control column, like a text area.
pub(super) fn setting_row<E: IntoElement>(
    title: SharedString,
    description: SharedString,
    keywords: &[&'static str],
    stacked: bool,
    field: impl Fn(&mut Window, &mut App) -> E + 'static,
) -> SettingItem {
    let searchable: Vec<SharedString> = [title.clone(), description.clone()]
        .into_iter()
        .chain(keywords.iter().map(|keyword| SharedString::from(*keyword)))
        .collect();
    SettingItem::render(
        move |options: &RenderOptions, window: &mut Window, cx: &mut App| {
            let rem = window.rem_size();
            let label = v_flex().child(Label::new(title.clone()).text_sm()).when(
                !description.is_empty(),
                |label| {
                    label.child(
                        div()
                            .id(SharedString::from(format!("setting-help-{title}")))
                            .test_support()
                            .role(Role::Label)
                            .aria_label(description.clone())
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(description.clone()),
                    )
                },
            );
            let control = div().child(field(window, cx));
            if stacked || options.layout() == Axis::Vertical {
                v_flex()
                    .w_full()
                    .gap_3()
                    .child(label.w_full())
                    .child(control.w_full())
            } else {
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .child(label.flex_1().min_w_0().max_w_3_5())
                    .child(control.w(rem * CONTROL_REMS).flex_shrink_0())
            }
        },
    )
    .keywords(searchable)
}

/// The Settings panel at the size and look of the Settings dialog.
pub(super) fn settings_panel(id: &'static str, window: &Window, cx: &App) -> SettingsPanel {
    let rem = window.rem_size();
    SettingsPanel::new(id)
        // Kit tints the sidebar. The dialog is one surface, so the divider
        // alone separates the section list from the page.
        .sidebar_style(&StyleRefinement::default().bg(cx.theme().background))
        .sidebar_width(rem * SIDEBAR_REMS)
        .sidebar_size_range((rem * SIDEBAR_MIN_REMS)..(rem * SIDEBAR_MAX_REMS))
}

/// The width and height of a settings dialog in `window`.
pub(super) fn settings_dialog_size(window: &Window) -> (Pixels, Pixels) {
    let rem = window.rem_size();
    let height = (rem * DIALOG_HEIGHT_REMS).min(window.viewport_size().height - rem * 4.);
    (Rows::dialog_width(window, DIALOG_REMS), height)
}

fn number_field(
    form: &SettingsForm,
    setting: NumberSetting,
) -> impl Fn(&mut Window, &mut App) -> AnyElement + 'static {
    let input = form.number(setting).clone();
    move |window: &mut Window, cx: &mut App| {
        setting_stepper(&input, setting.unit(), setting.label(), window, cx).into_any_element()
    }
}

fn font_field(
    form: &SettingsForm,
    setting: FontSetting,
) -> impl Fn(&mut Window, &mut App) -> Select<SearchableVec<String>> + 'static {
    select_field(form.font(setting), setting.label())
}

fn theme_field(
    form: &SettingsForm,
) -> impl Fn(&mut Window, &mut App) -> Select<SearchableVec<String>> + 'static {
    select_field(&form.theme, "Theme")
}

/// The element ID of the control of a setting, for example `setting-ui-scale`.
pub(crate) fn setting_id(label: &str) -> SharedString {
    let slug: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("setting-{}", slug.trim_matches('-')).into()
}

fn select_field(
    select: &SettingSelect,
    label: &'static str,
) -> impl Fn(&mut Window, &mut App) -> Select<SearchableVec<String>> + 'static {
    let select = select.clone();
    move |_: &mut Window, _: &mut App| {
        Select::new(&select)
            .focus_ring(false)
            .id(setting_id(label))
            .w_full()
            .accessibility_label(label)
    }
}

/// Retain Kit's spinbutton behavior while grouping the value and unit in one
/// centered segment. The default NumberInput suffix sits at the field edge.
fn setting_stepper(
    input: &Entity<InputState>,
    unit: &'static str,
    label: &'static str,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let value = input.read(cx).value();
    let font_size = window.rem_size() * 0.875;
    let run = TextRun {
        len: value.len(),
        font: window.text_style().font(),
        color: cx.theme().foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let value_width = window
        .text_system()
        .shape_line(value, font_size, &[run], None)
        .width;
    let border = cx.theme().input;
    let hover = cx.theme().secondary;
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    div()
        .id(setting_id(label))
        .w_full()
        .h(rems(2.))
        .rounded(cx.theme().radius)
        .bg(cx.theme().secondary)
        .border_1()
        .border_color(if focused { cx.theme().ring } else { border })
        .child(
            gpui_kit::base::NumberInput::new(input)
                .size_full()
                .decrement_button(move |button| {
                    button
                        .accessibility_label(format!("Decrease {label}"))
                        .w(rems(2.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_r_1()
                        .border_color(border)
                        .hover(move |el| el.bg(hover))
                        .child(gpui_kit::component::Icon::new(IconName::Minus).small())
                })
                .input(
                    h_flex()
                        .h_full()
                        .justify_center()
                        .gap_0()
                        .child(
                            div().w(value_width + px(2.)).min_w(px(4.)).h_full().child(
                                Input::new(input)
                                    .id("value")
                                    .appearance(false)
                                    .px_0()
                                    .h_full()
                                    .text_align(TextAlign::Right)
                                    .aria_label(label),
                            ),
                        )
                        .child(div().text_size(font_size).child(unit)),
                )
                .increment_button(move |button| {
                    button
                        .accessibility_label(format!("Increase {label}"))
                        .w(rems(2.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_l_1()
                        .border_color(border)
                        .hover(move |el| el.bg(hover))
                        .child(gpui_kit::component::Icon::new(IconName::Plus).small())
                }),
        )
}

#[cfg(test)]
mod font_tests {
    use super::{
        FontSetting, SYSTEM_FONT_FAMILY, SYSTEM_FONT_LABEL, Settings, font_family_for_selection,
        font_options,
    };

    #[test]
    fn every_font_picker_can_select_and_display_the_system_font() {
        let options = font_options(&[
            "Menlo".into(),
            SYSTEM_FONT_FAMILY.into(),
            "Apple Symbols".into(),
        ]);
        assert_eq!(options, [SYSTEM_FONT_LABEL, "Menlo", "Apple Symbols"]);
        assert_eq!(
            font_family_for_selection(SYSTEM_FONT_LABEL.into()),
            SYSTEM_FONT_FAMILY
        );

        let settings = Settings {
            ui_font_family: SYSTEM_FONT_FAMILY.into(),
            editor_font_family: SYSTEM_FONT_FAMILY.into(),
            logs_font_family: SYSTEM_FONT_FAMILY.into(),
            assistant_font_family: SYSTEM_FONT_FAMILY.into(),
            ..Settings::default()
        };
        for setting in FontSetting::ALL {
            assert_eq!(setting.selected(&settings), SYSTEM_FONT_LABEL);
        }
    }
}

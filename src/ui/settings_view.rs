use super::setting_row::Rows;
use super::*;
use gpui_kit::component::{
    IndexPath, h_flex,
    input::{NumberInputEvent, StepAction},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    setting::{
        RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings as SettingsPanel,
    },
    switch::Switch,
};
use qrow::model::{ASSISTANT_DATA_SHARING_NOTICE_VERSION, AssistantExecutionMode};
use std::cell::Cell;

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
    LogsFontSize,
    LogsLineHeight,
    AssistantFontSize,
    AssistantLineHeight,
}

impl NumberSetting {
    const ALL: [Self; 7] = [
        Self::Scale,
        Self::EditorFontSize,
        Self::EditorLineHeight,
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
        }
    }

    /// Accessibility label. A group title is not part of the accessible name,
    /// so each control names its own scope.
    fn label(self) -> &'static str {
        match self {
            Self::Scale => "UI Scale",
            Self::EditorFontSize => "Editor Font Size",
            Self::EditorLineHeight => "Editor Line Height",
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

    /// The option the select shows. System font identifiers need a readable
    /// entry in the UI and assistant font selectors.
    fn selected(self, settings: &Settings) -> String {
        let family = self.family(settings);
        let system_font = match self {
            Self::Ui => Some(Settings::default().ui_font_family),
            Self::Assistant => Some(Settings::default().assistant_font_family),
            _ => None,
        };
        if system_font.as_deref() == Some(family) {
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

pub(super) struct SettingsForm {
    /// Each stepper with the value it last displayed, so that a change made
    /// outside the dialog can be pushed back into the input.
    numbers: Vec<(NumberSetting, Entity<InputState>, Cell<f32>)>,
    fonts: Vec<(FontSetting, SettingSelect)>,
    assistant_mode: SettingSelect,
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
            .find(|(candidate, _)| *candidate == setting)
            .expect("the form holds every font setting")
            .1
    }
}

impl Qrow {
    pub(super) fn init_settings_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut subscriptions = Vec::new();
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
        let mut interface_fonts = self.fonts.clone();
        interface_fonts.retain(|font| font != &Settings::default().ui_font_family);
        interface_fonts.insert(0, SYSTEM_FONT_LABEL.into());
        let mut fonts = Vec::new();
        for setting in FontSetting::ALL {
            let options = match setting {
                FontSetting::Ui | FontSetting::Assistant => interface_fonts.clone(),
                _ => self.fonts.clone(),
            };
            let select = cx.new(|cx| {
                SelectState::new(SearchableVec::new(options), None, window, cx).searchable(true)
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
            fonts.push((setting, select));
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
            numbers,
            fonts,
            assistant_mode: mode,
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
        match setting {
            FontSetting::Ui => {
                let font = if font == SYSTEM_FONT_LABEL {
                    Settings::default().ui_font_family
                } else {
                    font
                };
                self.set_ui_font(font, window, cx);
            }
            FontSetting::Editor => self.set_editor_font(font, cx),
            FontSetting::Logs => self.set_logs_font(font, cx),
            FontSetting::Assistant => {
                let font = if font == SYSTEM_FONT_LABEL {
                    Settings::default().assistant_font_family
                } else {
                    font
                };
                self.set_assistant_font(font, cx);
            }
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
            self.assistant_panel.open = false;
            if self.assistant_panel.auto_hidden_sidebar {
                self.sidebar = true;
                self.assistant_panel.auto_hidden_sidebar = false;
            }
            self.assistant_panel.unread = false;
            self.assistant_panel.active_turn = None;
            self.assistant_panel.target = None;
            self.assistant_panel.pending_query = None;
            self.assistant_panel.snapshot = None;
            self.assistant_panel.transcripts.clear();
            self.assistant_panel.older_cursors.clear();
            self.assistant_panel.loaded_cursors.clear();
            self.assistant_panel.loading_older = false;
            self.assistant_panel.creating_conversation = false;
            self.assistant_panel.notice = None;
            self.assistant_panel.status = super::assistant_view::Status::Idle;
            self.assistant_panel.shutdown();
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
            alert.title("Enable AI assistant?")
                .description("Qrow sends selected SQL and allowed connection and tab details to Codex only when you send a message. Codex is a separate installation and keeps conversation history locally. Demo threads can remain in Codex after a crash.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-enable-assistant").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-enable-assistant").primary().label("Enable assistant")
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
            let rem = window.rem_size();
            let viewport = window.viewport_size();
            let height = (rem * DIALOG_HEIGHT_REMS).min(viewport.height - rem * 4.);
            dialog
                .title("Settings")
                .w(Rows::dialog_width(window, DIALOG_REMS))
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
        for (setting, select) in &form.fonts {
            let selected = setting.selected(&self.settings);
            if select.read(cx).selected_value() != Some(&selected) {
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
        let rem = window.rem_size();
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
                SettingsPanel::new("settings")
                    // Kit tints the sidebar. The dialog is one surface, so the
                    // divider alone separates the section list from the page.
                    .sidebar_style(&StyleRefinement::default().bg(cx.theme().background))
                    .sidebar_width(rem * SIDEBAR_REMS)
                    .sidebar_size_range((rem * SIDEBAR_MIN_REMS)..(rem * SIDEBAR_MAX_REMS))
                    .page(settings_page(form))
                    .page(assistant_settings_page(
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

/// The settings the dialog shows. One page holds every group, so each control
/// stays rendered and reachable from the keyboard: the sidebar entries are a
/// pointer-only shortcut that scrolls to a group.
fn settings_page(form: &SettingsForm) -> SettingPage {
    SettingPage::new("Appearance")
        .default_open(true)
        // Restore defaults in the footer already resets this page, and it is
        // the only page.
        .resettable(false)
        .group(
            SettingGroup::new()
                .title("Interface")
                .item(
                    SettingItem::new("Scale", number_field(form, NumberSetting::Scale))
                        .description("Resizes the whole interface.")
                        .keywords(["zoom", "interface", "ui"]),
                )
                .item(
                    SettingItem::new("Font Family", font_field(form, FontSetting::Ui))
                        .description(
                            "Used everywhere except the Editor, Logs, and assistant messages.",
                        )
                        .keywords(["interface", "ui", "typeface"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Editor")
                .item(
                    SettingItem::new("Font Family", font_field(form, FontSetting::Editor))
                        .description("Font for the SQL text.")
                        .keywords(["editor", "sql", "typeface"]),
                )
                .item(
                    SettingItem::new(
                        "Font Size",
                        number_field(form, NumberSetting::EditorFontSize),
                    )
                    .description("Base size, before Scale.")
                    .keywords(["editor", "sql"]),
                )
                .item(
                    SettingItem::new(
                        "Line Height",
                        number_field(form, NumberSetting::EditorLineHeight),
                    )
                    .description("Line spacing, relative to the font size.")
                    .keywords(["editor", "sql", "spacing"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Logs")
                .item(
                    SettingItem::new("Font Family", font_field(form, FontSetting::Logs))
                        .description("Font for the log entries.")
                        .keywords(["logs", "typeface"]),
                )
                .item(
                    SettingItem::new("Font Size", number_field(form, NumberSetting::LogsFontSize))
                        .description("Base size, before Scale.")
                        .keywords(["logs"]),
                )
                .item(
                    SettingItem::new(
                        "Line Height",
                        number_field(form, NumberSetting::LogsLineHeight),
                    )
                    .description("Line spacing, relative to the font size.")
                    .keywords(["logs", "spacing"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Assistant")
                .item(
                    SettingItem::new("Font Family", font_field(form, FontSetting::Assistant))
                        .description("Font for conversation messages.")
                        .keywords(["assistant", "messages", "typeface"]),
                )
                .item(
                    SettingItem::new(
                        "Font Size",
                        number_field(form, NumberSetting::AssistantFontSize),
                    )
                    .description("Base size, before Scale.")
                    .keywords(["assistant", "messages"]),
                )
                .item(
                    SettingItem::new(
                        "Line Height",
                        number_field(form, NumberSetting::AssistantLineHeight),
                    )
                    .description("Line spacing, relative to the font size.")
                    .keywords(["assistant", "messages", "spacing"]),
                ),
        )
}

fn assistant_settings_page(
    form: &SettingsForm,
    owner: WeakEntity<Qrow>,
    enabled: bool,
) -> SettingPage {
    let mode = form.assistant_mode.clone();
    let executable = form.assistant_executable.clone();
    SettingPage::new("AI Assistant")
        .resettable(false)
        .group(SettingGroup::new().title("Codex")
            .item(SettingItem::new("Enable assistant", SettingField::render({
                move |options: &RenderOptions, window: &mut Window, _: &mut App| {
                    let owner = owner.clone();
                    control(options, window.rem_size(), Switch::new("enable-assistant")
                        .checked(enabled).accessibility_label("Enable AI assistant")
                        .on_click(move |next, window, cx| {
                            let _ = owner.update(cx, |this, cx| this.set_assistant_enabled(*next, window, cx));
                        }))
                }
            })).description("Optional. Qrow starts Codex only when you open the assistant pane."))
            .item(SettingItem::new("Codex executable", SettingField::render(move |options: &RenderOptions, window: &mut Window, _: &mut App| {
                control(options, window.rem_size(), Input::new(&executable).w_full().aria_label("Codex executable"))
            })).description("Leave blank for automatic discovery. Install Codex separately.")))
        .group(SettingGroup::new().title("Query execution")
            .item(SettingItem::new("Default mode", SettingField::render(move |options: &RenderOptions, window: &mut Window, _: &mut App| {
                control(options, window.rem_size(), Select::new(&mode).w_full().accessibility_label("Default assistant query execution mode"))
            })).description("New conversations copy this mode. Run automatically can change or delete data and schema.")))
}

/// Size the control column. A page that keeps the label beside the control
/// aligns every control on one width; a stacked page fills the row.
fn control(options: &RenderOptions, rem: Pixels, element: impl IntoElement) -> Div {
    div()
        .map(|this| match options.layout() {
            Axis::Horizontal => this.w(rem * CONTROL_REMS).flex_shrink_0(),
            Axis::Vertical => this.w_full(),
        })
        .child(element)
}

fn number_field(form: &SettingsForm, setting: NumberSetting) -> SettingField<SharedString> {
    let input = form.number(setting).clone();
    SettingField::render(
        move |options: &RenderOptions, window: &mut Window, cx: &mut App| {
            control(
                options,
                window.rem_size(),
                setting_stepper(&input, setting.unit(), setting.label(), window, cx),
            )
        },
    )
}

fn font_field(form: &SettingsForm, setting: FontSetting) -> SettingField<SharedString> {
    let select = form.font(setting).clone();
    SettingField::render(
        move |options: &RenderOptions, window: &mut Window, _: &mut App| {
            control(
                options,
                window.rem_size(),
                Select::new(&select)
                    .w_full()
                    .accessibility_label(setting.label()),
            )
        },
    )
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

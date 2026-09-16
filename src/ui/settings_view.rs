use super::setting_row::Rows;
use super::*;
use gpui_kit::component::{
    h_flex,
    input::{NumberInputEvent, StepAction},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    v_flex,
};

const DIALOG_REMS: f32 = 56.;
const DIALOG_HEIGHT_REMS: f32 = 64.;
const CONTROL_REMS: f32 = 10.;

type SettingSelect = Entity<SelectState<SearchableVec<String>>>;
const SYSTEM_FONT_LABEL: &str = "System Font";

#[derive(Clone, Copy)]
enum NumberSetting {
    Scale,
    EditorFontSize,
    EditorLineHeight,
    LogsFontSize,
    LogsLineHeight,
}

pub(super) struct SettingsForm {
    scale: Entity<InputState>,
    editor_font: SettingSelect,
    editor_font_size: Entity<InputState>,
    editor_line_height: Entity<InputState>,
    logs_font: SettingSelect,
    logs_font_size: Entity<InputState>,
    logs_line_height: Entity<InputState>,
    ui_font: SettingSelect,
    displayed_scale: std::cell::Cell<f32>,
    displayed_editor_font_size: std::cell::Cell<f32>,
    displayed_editor_line_height: std::cell::Cell<f32>,
    displayed_logs_font_size: std::cell::Cell<f32>,
    displayed_logs_line_height: std::cell::Cell<f32>,
    _subscriptions: Vec<Subscription>,
}

impl Qrow {
    pub(super) fn init_settings_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scale = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.0}", self.settings.ui_scale * 100.))
        });
        let editor_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(self.fonts.clone()), None, window, cx)
                .searchable(true)
        });
        let editor_font_size = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.0}", self.settings.editor_font_size))
        });
        let editor_line_height = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.1}", self.settings.editor_line_height))
        });
        let logs_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(self.fonts.clone()), None, window, cx)
                .searchable(true)
        });
        let logs_font_size = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.0}", self.settings.logs_font_size))
        });
        let logs_line_height = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.1}", self.settings.logs_line_height))
        });
        let ui_font = cx.new(|cx| {
            let mut fonts = self.fonts.clone();
            fonts.retain(|font| font != &Settings::default().ui_font_family);
            fonts.insert(0, SYSTEM_FONT_LABEL.into());
            SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true)
        });
        let mut subscriptions = vec![cx.subscribe(&editor_font, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                this.set_editor_font(value.clone(), cx);
            }
        })];
        subscriptions.push(cx.subscribe(&logs_font, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                this.set_logs_font(value.clone(), cx);
            }
        }));
        subscriptions.push(
            cx.subscribe_in(&ui_font, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    let font = if value == SYSTEM_FONT_LABEL {
                        Settings::default().ui_font_family
                    } else {
                        value.clone()
                    };
                    this.set_ui_font(font, window, cx);
                }
            }),
        );
        for (input, setting) in [
            (&scale, NumberSetting::Scale),
            (&editor_font_size, NumberSetting::EditorFontSize),
            (&editor_line_height, NumberSetting::EditorLineHeight),
            (&logs_font_size, NumberSetting::LogsFontSize),
            (&logs_line_height, NumberSetting::LogsLineHeight),
        ] {
            // Handle steps here instead of InputState's default one-unit step.
            input.update(cx, |input, cx| input.set_step(None, window, cx));
            subscriptions.push(cx.subscribe_in(
                input,
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
                input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_number_setting(input, setting, 0., window, cx);
                    } else if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                },
            ));
        }
        self.settings_form = Some(SettingsForm {
            displayed_scale: std::cell::Cell::new(self.settings.ui_scale * 100.),
            displayed_editor_font_size: std::cell::Cell::new(self.settings.editor_font_size),
            displayed_editor_line_height: std::cell::Cell::new(self.settings.editor_line_height),
            displayed_logs_font_size: std::cell::Cell::new(self.settings.logs_font_size),
            displayed_logs_line_height: std::cell::Cell::new(self.settings.logs_line_height),
            scale,
            editor_font,
            editor_font_size,
            editor_line_height,
            logs_font,
            logs_font_size,
            logs_line_height,
            ui_font,
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
        let (current, min, max, step) = match setting {
            NumberSetting::Scale => (
                self.settings.ui_scale * 100.,
                MIN_UI_SCALE * 100.,
                MAX_UI_SCALE * 100.,
                UI_SCALE_STEP * 100.,
            ),
            NumberSetting::EditorFontSize => (
                self.settings.editor_font_size,
                MIN_EDITOR_FONT_SIZE,
                MAX_EDITOR_FONT_SIZE,
                1.,
            ),
            NumberSetting::EditorLineHeight => (
                self.settings.editor_line_height,
                MIN_LINE_HEIGHT,
                MAX_LINE_HEIGHT,
                LINE_HEIGHT_STEP,
            ),
            NumberSetting::LogsFontSize => (
                self.settings.logs_font_size,
                MIN_EDITOR_FONT_SIZE,
                MAX_EDITOR_FONT_SIZE,
                1.,
            ),
            NumberSetting::LogsLineHeight => (
                self.settings.logs_line_height,
                MIN_LINE_HEIGHT,
                MAX_LINE_HEIGHT,
                LINE_HEIGHT_STEP,
            ),
        };
        let value = input
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .unwrap_or(current);
        let value = match setting {
            NumberSetting::EditorLineHeight | NumberSetting::LogsLineHeight => {
                ((value * 10.).round() + direction * step * 10.).round() / 10.
            }
            _ => value.round() + direction * step,
        }
        .clamp(min, max);
        let displayed = if matches!(
            setting,
            NumberSetting::EditorLineHeight | NumberSetting::LogsLineHeight
        ) {
            format!("{value:.1}")
        } else {
            format!("{value:.0}")
        };
        input.update(cx, |input, cx| input.set_value(displayed, window, cx));
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
            _ => {}
        }
    }

    pub(super) fn open_settings_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.settings_content(window, cx))
                .ok();
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
                .children(content)
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
        // Keep the controls in sync with shortcuts and Restore defaults.
        for (control, displayed, value) in [
            (
                &form.scale,
                &form.displayed_scale,
                self.settings.ui_scale * 100.,
            ),
            (
                &form.editor_font_size,
                &form.displayed_editor_font_size,
                self.settings.editor_font_size,
            ),
            (
                &form.logs_font_size,
                &form.displayed_logs_font_size,
                self.settings.logs_font_size,
            ),
        ] {
            if displayed.get() != value {
                displayed.set(value);
                control.update(cx, |state, cx| {
                    state.set_value(format!("{value:.0}"), window, cx)
                });
            }
        }
        for (control, displayed, value) in [
            (
                &form.editor_line_height,
                &form.displayed_editor_line_height,
                self.settings.editor_line_height,
            ),
            (
                &form.logs_line_height,
                &form.displayed_logs_line_height,
                self.settings.logs_line_height,
            ),
        ] {
            if displayed.get() != value {
                displayed.set(value);
                control.update(cx, |state, cx| {
                    state.set_value(format!("{value:.1}"), window, cx)
                });
            }
        }
        if form.editor_font.read(cx).selected_value() != Some(&self.settings.editor_font_family) {
            form.editor_font.update(cx, |state, cx| {
                state.set_selected_value(&self.settings.editor_font_family, window, cx)
            });
        }
        if form.logs_font.read(cx).selected_value() != Some(&self.settings.logs_font_family) {
            form.logs_font.update(cx, |state, cx| {
                state.set_selected_value(&self.settings.logs_font_family, window, cx)
            });
        }
        let ui_font = if self.settings.ui_font_family == Settings::default().ui_font_family {
            SYSTEM_FONT_LABEL.to_owned()
        } else {
            self.settings.ui_font_family.clone()
        };
        if form.ui_font.read(cx).selected_value() != Some(&ui_font) {
            form.ui_font.update(cx, |state, cx| {
                state.set_selected_value(&ui_font, window, cx)
            });
        }
        let rows = Rows::new(
            Rows::dialog_width(window, DIALOG_REMS),
            window.rem_size(),
            CONTROL_REMS,
        );
        let row = |label: &'static str, description: &'static str, control: AnyElement| {
            rows.row(label, description, control, cx)
        };
        let section = |label: &'static str| {
            div()
                .w_full()
                .pt_4()
                .pb_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
        };
        v_flex()
            .w_full()
            .flex_shrink_0()
            .pt_3()
            .pb_4()
            .child(section("UI"))
            .child(row(
                "Scale",
                "Text and controls size.",
                setting_stepper(&form.scale, "%", "UI Scale", window, cx).into_any_element(),
            ))
            .child(
                rows.last_row(
                    "Font Family",
                    "Font used for text and controls.",
                    Select::new(&form.ui_font)
                        .w_full()
                        .accessibility_label("UI Font Family")
                        .into_any_element(),
                    cx,
                ),
            )
            .child(section("Editor"))
            .child(row(
                "Font Family",
                "A monospace font is recommended.",
                Select::new(&form.editor_font)
                    .w_full()
                    .accessibility_label("Editor Font Family")
                    .into_any_element(),
            ))
            .child(row(
                "Font Size",
                "Font size before scaling.",
                setting_stepper(&form.editor_font_size, "px", "Editor Font Size", window, cx)
                    .into_any_element(),
            ))
            .child(
                rows.last_row(
                    "Line Height",
                    "Multiplier for the vertical spacing between lines.",
                    setting_stepper(
                        &form.editor_line_height,
                        "x",
                        "Editor Line Height",
                        window,
                        cx,
                    )
                    .into_any_element(),
                    cx,
                ),
            )
            .child(section("Logs"))
            .child(row(
                "Font Family",
                "A monospace font is recommended.",
                Select::new(&form.logs_font)
                    .w_full()
                    .accessibility_label("Logs Font Family")
                    .into_any_element(),
            ))
            .child(row(
                "Font Size",
                "Font size before scaling.",
                setting_stepper(&form.logs_font_size, "px", "Logs Font Size", window, cx)
                    .into_any_element(),
            ))
            .child(
                rows.last_row(
                    "Line Height",
                    "Multiplier for the vertical spacing between lines.",
                    setting_stepper(&form.logs_line_height, "x", "Logs Line Height", window, cx)
                        .into_any_element(),
                    cx,
                ),
            )
            .into_any_element()
    }

    fn settings_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .w_full()
            .gap_2()
            .pt_3()
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
                        if let Some(form) = &this.settings_form {
                            let scale = form.scale.clone();
                            let editor_font_size = form.editor_font_size.clone();
                            let editor_line_height = form.editor_line_height.clone();
                            let logs_font_size = form.logs_font_size.clone();
                            let logs_line_height = form.logs_line_height.clone();
                            this.commit_number_setting(
                                &scale,
                                NumberSetting::Scale,
                                0.,
                                window,
                                cx,
                            );
                            this.commit_number_setting(
                                &editor_font_size,
                                NumberSetting::EditorFontSize,
                                0.,
                                window,
                                cx,
                            );
                            this.commit_number_setting(
                                &editor_line_height,
                                NumberSetting::EditorLineHeight,
                                0.,
                                window,
                                cx,
                            );
                            this.commit_number_setting(
                                &logs_font_size,
                                NumberSetting::LogsFontSize,
                                0.,
                                window,
                                cx,
                            );
                            this.commit_number_setting(
                                &logs_line_height,
                                NumberSetting::LogsLineHeight,
                                0.,
                                window,
                                cx,
                            );
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

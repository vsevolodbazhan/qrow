use super::*;
use gpui_kit::component::{
    form::{Field, Form},
    h_flex,
    input::{NumberInputEvent, StepAction},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    v_flex,
};

type SettingSelect = Entity<SelectState<SearchableVec<String>>>;

pub(super) struct SettingsForm {
    scale: Entity<InputState>,
    font: SettingSelect,
    size: Entity<InputState>,
    displayed_scale: std::cell::Cell<f32>,
    displayed_size: std::cell::Cell<f32>,
    _subscriptions: Vec<Subscription>,
}

impl Qrow {
    pub(super) fn init_settings_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scale = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.0}", self.settings.ui_scale * 100.))
        });
        let font = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(self.coding_fonts.clone()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let size = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.0}", self.settings.editor_font_size))
        });
        let mut subscriptions = vec![cx.subscribe(&font, |this, _, event, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                this.set_editor_font(value.clone(), cx);
            }
        })];
        for (input, is_scale) in [(&scale, true), (&size, false)] {
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
                    this.commit_number_setting(input, is_scale, direction, window, cx);
                },
            ));
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_number_setting(input, is_scale, 0., window, cx);
                    } else if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                },
            ));
        }
        self.settings_form = Some(SettingsForm {
            displayed_scale: std::cell::Cell::new(self.settings.ui_scale * 100.),
            displayed_size: std::cell::Cell::new(self.settings.editor_font_size),
            scale,
            font,
            size,
            _subscriptions: subscriptions,
        });
    }

    fn commit_number_setting(
        &mut self,
        input: &Entity<InputState>,
        is_scale: bool,
        direction: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (current, min, max, step) = if is_scale {
            (
                self.settings.ui_scale * 100.,
                MIN_UI_SCALE * 100.,
                MAX_UI_SCALE * 100.,
                UI_SCALE_STEP * 100.,
            )
        } else {
            (
                self.settings.editor_font_size,
                MIN_EDITOR_FONT_SIZE,
                MAX_EDITOR_FONT_SIZE,
                1.,
            )
        };
        let value = input
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .unwrap_or(current);
        let value = (value.round() + direction * step).clamp(min, max);
        input.update(cx, |input, cx| {
            input.set_value(format!("{value:.0}"), window, cx)
        });
        if is_scale {
            self.apply_ui_scale(value / 100., window, cx);
        } else if self.settings.editor_font_size != value {
            self.settings.editor_font_size = value;
            self.changed(cx);
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
            let height = (rem * 28.).min(viewport.height - rem * 4.);
            dialog
                .title("Settings")
                .w((rem * 56.).min(viewport.width - rem * 4.))
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
                &form.size,
                &form.displayed_size,
                self.settings.editor_font_size,
            ),
        ] {
            if displayed.get() != value {
                displayed.set(value);
                control.update(cx, |state, cx| {
                    state.set_value(format!("{value:.0}"), window, cx)
                });
            }
        }
        if form.font.read(cx).selected_value() != Some(&self.settings.editor_font_family) {
            form.font.update(cx, |state, cx| {
                state.set_selected_value(&self.settings.editor_font_family, window, cx)
            });
        }
        let width =
            (window.rem_size() * 56.).min(window.viewport_size().width - window.rem_size() * 4.);
        let label_width = (width - window.rem_size() * 6.) * 0.60;
        let row = |label: &'static str, description: &'static str, control: AnyElement| {
            Form::horizontal().label_width(label_width).child(
                Field::new()
                    .py_5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .label_fn(move |_, cx| {
                        v_flex()
                            .gap_1()
                            .pr_6()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::NORMAL)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(description),
                            )
                    })
                    .child(h_flex().w_full().child(div().flex_1()).child(control)),
            )
        };
        v_flex()
            .w_full()
            .pt_3()
            .child(row(
                "Interface scale",
                "Resize text and controls.",
                div()
                    .w(rems(10.))
                    .flex_shrink_0()
                    .child(setting_stepper(
                        &form.scale,
                        "%",
                        "Interface scale",
                        window,
                        cx,
                    ))
                    .into_any_element(),
            ))
            .child(row(
                "Coding font",
                "Editor font; monospace recommended.",
                div()
                    .w(rems(10.))
                    .flex_shrink_0()
                    .child(
                        Select::new(&form.font)
                            .w_full()
                            .accessibility_label("Coding font"),
                    )
                    .into_any_element(),
            ))
            .child(row(
                "Editor font size",
                "Editor text size before scaling.",
                div()
                    .w(rems(10.))
                    .flex_shrink_0()
                    .child(setting_stepper(
                        &form.size,
                        "px",
                        "Editor font size",
                        window,
                        cx,
                    ))
                    .into_any_element(),
            ))
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
                            let size = form.size.clone();
                            this.commit_number_setting(&scale, true, 0., window, cx);
                            this.commit_number_setting(&size, false, 0., window, cx);
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

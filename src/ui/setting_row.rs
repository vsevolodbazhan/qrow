use super::*;
use gpui_kit::component::{
    form::{Field, Form},
    h_flex, v_flex,
};

/// Row geometry for a settings-style dialog: label and description on the
/// left, control on the right, separated by a rule. Narrow windows stack the
/// two instead, because the label column would otherwise crush the control.
#[derive(Clone, Copy)]
pub(super) struct Rows {
    label_width: Pixels,
    /// `None` lets the control fill its column instead of hugging the right
    /// edge. A stepper or a select looks deliberate right-aligned; a text
    /// field just leaves a gap.
    control_width: Option<Pixels>,
    compact: bool,
}

impl Rows {
    /// `control` is the width of the right-hand column, in rems.
    pub(super) fn new(width: Pixels, rem: Pixels, control: f32) -> Self {
        Self {
            control_width: Some(rem * control),
            ..Self::filling(width, rem)
        }
    }

    /// Rows whose control spans its whole column.
    pub(super) fn filling(width: Pixels, rem: Pixels) -> Self {
        Self {
            label_width: (width - rem * 6.) * 0.60,
            control_width: None,
            compact: width < rem * 42.,
        }
    }

    /// Width a dialog needs for its rows to stay side by side.
    pub(super) fn dialog_width(window: &Window, rems: f32) -> Pixels {
        let rem = window.rem_size();
        (rem * rems).min(window.viewport_size().width - rem * 4.)
    }

    pub(super) fn row(
        &self,
        label: &'static str,
        description: impl Into<SharedString>,
        control: AnyElement,
        cx: &App,
    ) -> impl IntoElement {
        let description = description.into();
        let form = if self.compact {
            Form::vertical()
        } else {
            Form::horizontal().label_width(self.label_width)
        };
        form.child(
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
                                .child(description.clone()),
                        )
                })
                .child(
                    h_flex()
                        .w_full()
                        .when(self.control_width.is_some(), |el| el.child(div().flex_1()))
                        .child(
                            div()
                                .when_some(self.control_width, |el, width| {
                                    el.w(width).flex_shrink_0()
                                })
                                .when(self.control_width.is_none(), |el| el.flex_1().min_w_0())
                                .child(control),
                        ),
                ),
        )
    }
}

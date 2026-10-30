use super::*;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::input::Input;

/// A text field with its unit right after the value, like `16 MiB`.
pub(super) fn unit_input(
    input: &Entity<InputState>,
    id: impl Into<ElementId>,
    label: &'static str,
    unit: (&'static str, impl Into<ElementId>),
    disabled: bool,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let (unit, unit_id) = (unit.0, unit.1.into());
    let value = input.read(cx).value();
    // Bound the measurement for invalid paste data. Long input uses the
    // regular suffix so that the unit cannot cover the text or caret.
    let measured: String = value.chars().take(32).collect();
    let complete = measured.len() == value.len();
    let font_size = window.rem_size() * 0.875;
    let run = TextRun {
        len: measured.len(),
        font: window.text_style().font(),
        color: cx.theme().foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let width = window
        .text_system()
        .shape_line(measured.into(), font_size, &[run], None)
        .width;
    let adjacent = complete && width <= window.rem_size() * 7.;
    div()
        .relative()
        .w_full()
        .child(
            Input::new(input)
                .focus_ring(false)
                .id(id)
                .w_full()
                .disabled(disabled)
                .aria_label(label)
                .when(!adjacent, |view| {
                    view.suffix(div().id(unit_id.clone()).test_support().child(unit))
                }),
        )
        .when(adjacent, |view| {
            view.child(
                div()
                    .id(unit_id)
                    .test_support()
                    .absolute()
                    .left(px(14.) + width)
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(font_size)
                    .when(disabled, |view| view.opacity(0.5))
                    .child(unit),
            )
        })
        .into_any_element()
}

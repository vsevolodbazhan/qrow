use super::*;
use gpui_kit::component::{Selectable as _, button::ButtonCustomVariant, h_flex};

/// The opacity of the foreground color that fills a selected segment. The
/// fill follows the theme: light gray on a light theme, mid gray on a dark one.
const SELECTED_SEGMENT_FILL: f32 = 0.2;

/// `button` as one segment of a segmented control. The selected segment is
/// filled and uses the full text color; the other one is quiet and muted.
pub(super) fn segment(button: Button, selected: bool, cx: &App) -> Button {
    let theme = cx.theme();
    let variant = if selected {
        let fill = theme.foreground.opacity(SELECTED_SEGMENT_FILL);
        ButtonCustomVariant::new(cx)
            .color(fill)
            .hover(fill)
            .active(fill)
            .foreground(theme.foreground)
    } else {
        ButtonCustomVariant::new(cx)
            .color(theme.background.opacity(0.))
            .hover(theme.accent)
            .active(theme.accent)
            .foreground(theme.muted_foreground)
    };
    button
        .custom(variant.shadow(false))
        .selected(selected)
        .toggled(selected)
}

pub(super) fn button_pair(
    id: &'static str,
    left: Button,
    right: Button,
    cx: &App,
) -> impl IntoElement {
    let radius = cx.theme().font_size * 0.375;
    let inner_radius = (radius - px(1.)).max(px(0.));

    // The end buttons are rounded themselves, so their selected, hover, and
    // pressed fills stay inside the rounded outline without a clip. Without
    // one, the keyboard focus ring of a segment shows in full.
    h_flex()
        .id(id)
        .flex_shrink_0()
        .items_stretch()
        .border_1()
        .border_color(cx.theme().border)
        .rounded(radius)
        .child(left.rounded_none().rounded_l(inner_radius))
        .child(div().w_px().bg(cx.theme().border))
        .child(right.rounded_none().rounded_r(inner_radius))
}

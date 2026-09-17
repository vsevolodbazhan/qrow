use super::*;
use gpui_kit::component::h_flex;

pub(super) fn button_pair(
    id: &'static str,
    left: Button,
    right: Button,
    cx: &App,
) -> impl IntoElement {
    let radius = cx.theme().font_size * 0.375;
    let inner_radius = (radius - px(1.)).max(px(0.));

    // GPUI overflow masks are rectangular. Round the end buttons themselves
    // so selected, hover, and pressed fills stay inside the rounded outline.
    h_flex()
        .id(id)
        .flex_shrink_0()
        .items_stretch()
        .border_1()
        .border_color(cx.theme().border)
        .rounded(radius)
        .overflow_hidden()
        .child(left.rounded_none().rounded_l(inner_radius))
        .child(div().w_px().bg(cx.theme().border))
        .child(right.rounded_none().rounded_r(inner_radius))
}

use gpui::App;
use rust_i18n::t;

use crate::{
    Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
};

#[inline]
pub(crate) fn clear_button(_: &App) -> Button {
    Button::new("clean")
        .icon(Icon::new(IconName::Close))
        .text()
        .xsmall()
        .accessibility_label(t!("Input.Clear"))
        .tooltip(t!("Input.Clear"))
        .tab_stop(false)
}

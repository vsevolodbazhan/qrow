use super::setting_row::Rows;
use super::*;
use crate::assets::APP_ICON;
use gpui_kit::base::StyledExt;
use gpui_kit::component::v_flex;

/// Wide enough for the longest line, the copyright, without wrapping it.
const DIALOG_REMS: f32 = 22.;
/// Tall enough for the icon and the three lines below it.
const DIALOG_HEIGHT_REMS: f32 = 15.;
/// Points, at the standard interface scale.
const ICON_SIZE: f32 = 64.;

impl Qrow {
    pub(super) fn open_about_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let content = weak.update(cx, |this, cx| this.about_content(cx)).ok();
            // A short card reads as a panel about the application, so it sits
            // in the middle of the window instead of near its top.
            let viewport = window.viewport_size();
            let height = (window.rem_size() * DIALOG_HEIGHT_REMS).min(viewport.height);
            dialog
                .w(Rows::dialog_width(window, DIALOG_REMS))
                .h(height)
                .margin_top((viewport.height - height) / 2.)
                .children(content)
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.about_open = false;
                        cx.notify();
                    });
                })
        });
    }

    fn about_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let version = qrow::build_info::version_label();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            // The bundled artwork carries its own rounded corners.
            .child(
                img(APP_ICON)
                    .size(self.ui_px(ICON_SIZE))
                    .mb_3()
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .id("about-name")
                    .role(Role::Label)
                    .text_size(rems(16. / 14.))
                    .font_semibold()
                    .aria_label("Qrow")
                    .child("Qrow"),
            )
            .child(
                div()
                    .id("about-version")
                    .role(Role::Label)
                    .text_size(rems(12. / 14.))
                    .text_color(cx.theme().muted_foreground)
                    // Bug reports name this build.
                    .aria_label(version.clone())
                    .child(version),
            )
            .child(
                div()
                    .id("about-copyright")
                    .role(Role::Label)
                    .mt_2()
                    .text_size(rems(11. / 14.))
                    .text_color(cx.theme().muted_foreground)
                    .aria_label(qrow::build_info::COPYRIGHT)
                    .child(qrow::build_info::COPYRIGHT),
            )
            .into_any_element()
    }
}

use super::setting_row::Rows;
use super::*;
use crate::assets::APP_ICON;
use gpui_kit::base::StyledExt;
use gpui_kit::component::v_flex;

/// Wide enough for the longest line, the copyright, without wrapping it.
const DIALOG_REMS: f32 = 22.;
/// Tall enough for the icon and the three lines below it.
const DIALOG_HEIGHT_REMS: f32 = 15.;
/// The extra height of the update status and its button.
const UPDATES_HEIGHT_REMS: f32 = 6.;
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
            let updates = weak
                .read_with(cx, |this, _| this.updates.enabled())
                .unwrap_or(false);
            let rems = if updates {
                DIALOG_HEIGHT_REMS + UPDATES_HEIGHT_REMS
            } else {
                DIALOG_HEIGHT_REMS
            };
            let height = (window.rem_size() * rems).min(viewport.height);
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
        let version = crate::build_info::version_label();
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
                    .test_support()
                    .role(Role::Label)
                    .text_size(rems(16. / 14.))
                    .font_semibold()
                    .aria_label("Qrow")
                    .child("Qrow"),
            )
            .child(
                div()
                    .id("about-version")
                    .test_support()
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
                    .test_support()
                    .role(Role::Label)
                    .mt_2()
                    .text_size(rems(11. / 14.))
                    .text_color(cx.theme().muted_foreground)
                    .aria_label(crate::build_info::COPYRIGHT)
                    .child(crate::build_info::COPYRIGHT),
            )
            .when(self.updates.enabled(), |content| {
                content.child(self.about_updates(cx))
            })
            .into_any_element()
    }

    /// The update state and the action that fits it.
    fn about_updates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let line = |id: &'static str, text: String| {
            div()
                .id(id)
                .test_support()
                .role(Role::Label)
                .text_size(rems(12. / 14.))
                .text_color(cx.theme().muted_foreground)
                .text_center()
                .whitespace_normal()
                .aria_label(text.clone())
                .child(text)
        };
        let button = if self.updates.can_restart() {
            Button::new("about-restart-update")
                .primary()
                .label("Restart to update")
                .on_click(cx.listener(|this, _, window, cx| this.restart_to_update(window, cx)))
        } else {
            Button::new("about-check-updates")
                .label("Check for updates")
                .loading(self.updates.checking())
                .disabled(self.updates.checking())
                .on_click(
                    cx.listener(|this, _, window, cx| this.check_for_updates(true, window, cx)),
                )
        };
        v_flex()
            .w_full()
            .mt_3()
            .gap_1()
            .items_center()
            .when_some(self.updates.summary(), |column, summary| {
                column.child(line("about-update-status", summary))
            })
            .when_some(self.updates.instruction(), |column, instruction| {
                column.child(line("about-update-instruction", instruction))
            })
            .child(div().mt_1().child(button))
    }
}

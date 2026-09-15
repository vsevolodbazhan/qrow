use super::setting_row::Rows;
use super::*;
use gpui_kit::component::{h_flex, input::Textarea, v_flex};

/// Wide enough that a label, its description, and the control sit on one line.
const DIALOG_REMS: f32 = 56.;

impl Qrow {
    pub(super) fn open_profile_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        if window.has_active_dialog(cx) {
            if let Some(form) = &self.form {
                form.fields[0].update(cx, |field, cx| field.focus(window, cx));
            }
            return;
        }
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let save = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.profile_content(window, cx))
                .ok();
            let footer = weak.update(cx, |this, cx| this.profile_footer(cx)).ok();
            // Resolve the popup against the current window so its footer stays visible.
            let rem = window.rem_size();
            let viewport = window.viewport_size();
            let height = (rem * 48.).min(viewport.height - rem * 4.);
            dialog
                .title("Connection settings")
                .w(Rows::dialog_width(window, DIALOG_REMS))
                .h(height)
                .margin_top((viewport.height - height) / 2.)
                .overlay_closable(false)
                .on_ok(move |_, _, cx| {
                    let _ = save.update(cx, |this, cx| this.save_profile(cx));
                    // The worker result closes the popup only after a successful save.
                    false
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        // A submitted save must finish even if its popup is dismissed.
                        if !this.form.as_ref().is_some_and(|f| f.saving.is_some()) {
                            this.form = None;
                        }
                        cx.notify();
                    });
                })
        });
    }

    fn profile_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let saving = form.saving.is_some();
        let rows = Rows::filling(Rows::dialog_width(window, DIALOG_REMS), window.rem_size());
        let field = |index: usize, label: &'static str| {
            Input::new(&form.fields[index])
                .disabled(saving)
                .aria_label(label)
                .into_any_element()
        };
        v_flex()
            .key_context("ConnectionSettings")
            .on_action(cx.listener(|this, _: &SaveConnection, _, cx| this.save_profile(cx)))
            .w_full()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Spark (HiveServer2) · LDAP authentication"),
            )
            .child(
                v_flex()
                    .id("connection-fields")
                    .pt_3()
                    .child(rows.row(
                        "Name",
                        "Shown in the connections sidebar.",
                        field(0, "Name"),
                        cx,
                    ))
                    .child(rows.row(
                        "Host",
                        "Hostname of the Kyuubi or HiveServer2 endpoint.",
                        field(1, "Host"),
                        cx,
                    ))
                    .child(rows.row("Port", "Thrift port on that host.", field(2, "Port"), cx))
                    .child(rows.row(
                        "LDAP username",
                        "User name for LDAP authentication.",
                        field(3, "LDAP username"),
                        cx,
                    ))
                    .child(rows.row(
                        "Password",
                        if self.demo {
                            "Demo credentials are never stored."
                        } else {
                            "Leave blank to keep the stored password."
                        },
                        field(4, "Password"),
                        cx,
                    ))
                    .child(rows.row(
                        "Initial database",
                        "Selected when the session opens.",
                        field(5, "Initial database"),
                        cx,
                    ))
                    .child(
                        rows.row(
                            "Session parameters",
                            "JSON object with string values.",
                            // A one-entry pretty-printed object uses four lines.
                            // Keep the control to that height so it does not show
                            // an empty fifth line below the closing brace.
                            Textarea::new(&form.parameters)
                                .h(rems(6.))
                                .disabled(saving)
                                .font_family("Menlo")
                                .aria_label("Session parameters")
                                .into_any_element(),
                            cx,
                        ),
                    )
                    .child(connection_form::render_lifecycle(form, &rows, cx)),
            )
            .into_any_element()
    }

    fn profile_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let saving = form.saving.is_some();
        v_flex()
            .w_full()
            .gap_2()
            .key_context("ConnectionSettings")
            .on_action(cx.listener(|this, _: &SaveConnection, _, cx| this.save_profile(cx)))
            .when_some(form.error.clone(), |el, error| {
                el.child(div().text_color(cx.theme().danger).child(error))
            })
            .child(
                // Delete and Duplicate act on the connection as a list item, so
                // they live in its context menu rather than in its settings.
                // The last row's rule already separates the body from the footer.
                h_flex()
                    .gap_2()
                    .pt_3()
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-profile")
                            .label("Cancel")
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.form = None;
                                window.close_dialog(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save-profile")
                            .primary()
                            .label(if saving { "Saving" } else { "Save" })
                            .tooltip("Save connection · ⌘Enter")
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                    ),
            )
            .into_any_element()
    }
}

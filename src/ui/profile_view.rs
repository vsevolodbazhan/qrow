use super::*;
use gpui_kit::component::{
    form::{Field, Form},
    h_flex,
    input::Textarea,
    v_flex,
};

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
            let content = weak.update(cx, |this, cx| this.profile_content(cx)).ok();
            let footer = weak.update(cx, |this, cx| this.profile_footer(cx)).ok();
            // Resolve the popup against the current window so its footer stays visible.
            let rem = window.rem_size();
            let viewport = window.viewport_size();
            let height = (rem * 48.).min(viewport.height - rem * 4.);
            dialog
                .title("Connection settings")
                .w((rem * 36.).min(viewport.width - rem * 4.))
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

    fn profile_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let saving = form.saving.is_some();
        let field = |index, label: &'static str| {
            Field::new().label(label).child(
                Input::new(&form.fields[index])
                    .disabled(saving)
                    .aria_label(label),
            )
        };
        v_flex().key_context("ConnectionSettings")
            .on_action(cx.listener(|this, _: &SaveConnection, _, cx| this.save_profile(cx)))
            .w_full().gap_4().pb_4()
            .child(v_flex().gap_1()
                .child(div().font_weight(FontWeight::MEDIUM).child(if form.is_new { "New connection".to_owned() } else { form.profile.name.clone() }))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Spark (HiveServer2) · LDAP authentication")))
            .child(v_flex().id("connection-fields").gap_4()
                .child(Form::new().child(field(0, "Name")))
                .child(Form::new().columns(2).child(field(1, "Host")).child(field(2, "Port")))
                .child(Form::new().child(field(3, "LDAP username")).child(field(4, "Password"))
                    .child(field(5, "Initial database")))
                .child(div().text_sm().text_color(cx.theme().muted_foreground)
                    .child(if self.demo { "Demo credentials are never stored." } else { "Passwords are stored in macOS Keychain. Leave an existing password blank to keep it." }))
                .child(Form::new().child(Field::new().label("Session parameters")
                    .child(Textarea::new(&form.parameters).disabled(saving).font_family("Menlo").aria_label("Session parameters"))))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("JSON object with string values"))
                .child(connection_form::render_lifecycle(form, &[
                    "Name", "Host", "Port", "LDAP username", "Password", "Initial database", "Session parameters",
                    "Idle timeout in seconds", "Heartbeat interval in seconds", "Heartbeat SQL",
                ], cx))
                .child(div().text_sm().text_color(cx.theme().muted_foreground)
                    .child("Disconnecting keeps SQL and downloaded results. It releases temporary views, session settings, and unfetched rows.")))
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
            .when(form.confirm_delete, |el| {
                el.child(
                    v_flex()
                        .gap_2()
                        .child(format!("Delete \"{}\"?", form.profile.name))
                        .child("Its tabs will keep their SQL and downloaded results.")
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("keep-profile").label("Cancel").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(form) = &mut this.form {
                                            form.confirm_delete = false;
                                        }
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    Button::new("confirm-delete-profile")
                                        .danger()
                                        .label("Delete")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.delete_profile(cx);
                                            window.close_dialog(cx);
                                        })),
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_3()
                    .when(!form.is_new, |el| {
                        el.child(
                            Button::new("delete-profile")
                                .ghost()
                                .label("Delete")
                                .disabled(saving || form.confirm_delete)
                                .on_click(cx.listener(|this, _, _, cx| this.delete_profile(cx))),
                        )
                        .child(
                            Button::new("duplicate-profile")
                                .ghost()
                                .label("Duplicate")
                                .disabled(saving)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let mut profile = this.form.as_ref().unwrap().profile.clone();
                                    profile.id = Uuid::new_v4();
                                    profile.name.push_str(" copy");
                                    this.edit_profile(profile, true, window, cx);
                                })),
                        )
                    })
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

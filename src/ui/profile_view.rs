use super::*;
use gpui_kit::component::{
    alert::Alert,
    combobox::Combobox,
    form::{Field, Form},
    h_flex,
    input::Textarea,
    select::Select,
    switch::Switch,
    v_flex,
};

const DIALOG_REMS: f32 = 40.;

pub(super) fn dialog_width(window: &Window) -> Pixels {
    let rem = window.rem_size();
    (rem * DIALOG_REMS).min(window.viewport_size().width - rem * 4.)
}

fn field(label: &'static str, description: Option<&'static str>, control: AnyElement) -> Field {
    Field::new()
        .label(label)
        .child(control)
        .when_some(description, |field, description| {
            field.description(description)
        })
}

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
                .title("Connection Settings")
                .w(dialog_width(window))
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
        // A new choice in the Schema catalog list loads the settings of that
        // catalog into the fields.
        let catalog =
            connection_form::chosen_catalog(&form.catalog_select, &form.catalog_choices, cx);
        if catalog != form.catalog_choice {
            cx.defer_in(window, move |this, window, cx| {
                this.catalog_choice_changed(catalog, window, cx)
            });
        }
        let saving = form.saving.is_some();
        let uses_sign_in = connection_form::uses_sign_in(&form.authentication, cx);
        let chosen_sign_in =
            connection_form::chosen_sign_in(&form.sign_in, &form.sign_in_choices, cx);
        let sign_in_label = chosen_sign_in
            .and_then(|id| {
                form.sign_in_choices
                    .iter()
                    .find(|(choice, _)| *choice == id)
            })
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let sign_in_description = chosen_sign_in
            .map(|id| match self.oidc.identity(id) {
                Some(identity) => format!("Signed in as {}.", identity.display()),
                None => "Not signed in. Sign in from the Sign-ins sidebar.".to_owned(),
            })
            .unwrap_or_else(|| {
                if form.sign_in_choices.is_empty() {
                    "Add a sign-in with New Sign-in… in the list.".to_owned()
                } else {
                    "Choose the sign-in that this connection uses.".to_owned()
                }
            });
        let qrow = cx.weak_entity();
        let input = |index: usize, label: &'static str| {
            Input::new(&form.fields[index])
                .id(connection_form::FIELD_IDS[index])
                .w_full()
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
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("Spark (HiveServer2)"),
            )
            .child(
                v_flex()
                    .pt_3()
                    .w_full()
                    .gap_2()
                    .child(
                        Form::vertical()
                            .w_full()
                            .child(field(
                                "Name",
                                Some("Shown in the connections sidebar."),
                                input(0, "Name"),
                            ))
                            .child(field(
                                "Host",
                                Some("Hostname of the Kyuubi or HiveServer2 endpoint."),
                                input(1, "Host"),
                            ))
                            .child(field(
                                "Port",
                                Some("Thrift port on that host."),
                                input(2, "Port"),
                            ))
                            .child(field(
                                "TLS",
                                Some(if uses_sign_in && !form.tls {
                                    "Without TLS, anyone on the network path can read the access token and use it until it expires. Use only a trusted network or VPN."
                                } else {
                                    "Encrypts the connection. The server must accept TLS on this port."
                                }),
                                Switch::new("connection-tls")
                                    .checked(form.tls)
                                    .disabled(saving)
                                    .accessibility_label("TLS")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if let Some(form) = &mut this.form {
                                            form.tls = *checked;
                                        }
                                        cx.notify();
                                    }))
                                    .into_any_element(),
                            ))
                            .child(field(
                                "Authentication",
                                Some("A sign-in can serve several connections. Each connection keeps its own username."),
                                Select::new(&form.authentication)
                                    .id("connection-authentication")
                                    .w_full()
                                    .disabled(saving)
                                    .accessibility_label("Authentication")
                                    .into_any_element(),
                            ))
                            .when(uses_sign_in, |el| {
                                el.child(
                                    Field::new()
                                        .label("Sign-in")
                                        .child(
                                            // The combobox has no accessibility of
                                            // its own in GPUI Kit 0.6.6, so this
                                            // element names it and gives its value.
                                            div()
                                                .id("connection-sign-in")
                                                .test_support()
                                                .role(Role::ComboBox)
                                                .aria_label("Sign-in")
                                                .aria_value(sign_in_label)
                                                .w_full()
                                                .child(
                                                    Combobox::new(&form.sign_in)
                                                        .w_full()
                                                        .disabled(saving)
                                                        .placeholder("Choose a sign-in")
                                                        .search_placeholder("Search sign-ins…")
                                                        .footer(move |_, _| {
                                                            let qrow = qrow.clone();
                                                            Button::new("connection-new-sign-in")
                                                                .ghost()
                                                                .small()
                                                                .w_full()
                                                                .justify_start()
                                                                .icon(IconName::Plus)
                                                                .label("New Sign-in…")
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = qrow.update(cx, |this, cx| {
                                                                        this.open_sign_in_editor(None, true, window, cx)
                                                                    });
                                                                })
                                                        }),
                                                )
                                                .into_any_element(),
                                        )
                                        .description(sign_in_description),
                                )
                            })
                            .child(field(
                                "Username",
                                Some(if uses_sign_in {
                                    "The database account. Kyuubi checks that the signed-in identity can use it."
                                } else {
                                    "The database account for LDAP authentication."
                                }),
                                input(3, "Username"),
                            ))
                            .when(!uses_sign_in, |el| {
                                el.child(field(
                                    "Password",
                                    Some("Used for LDAP authentication."),
                                    input(4, "Password"),
                                ))
                            })
                            .child(field(
                                "Initial database",
                                Some("Selected when the session opens."),
                                input(5, "Initial database"),
                            ))
                            .child(field(
                                "Session parameters",
                                Some("JSON object with string values."),
                                Textarea::new(&form.parameters)
                                    .w_full()
                                    .disabled(saving)
                                    .font_family("Menlo")
                                    .aria_label("Session parameters")
                                    .into_any_element(),
                            ))
                            .child(field(
                                "Response timeout",
                                Some("Seconds to wait for one answer from Kyuubi, from 10 to 3600. Applies to new sessions."),
                                input(14, "Response timeout in seconds"),
                            )),
                    )
                    .child(connection_form::render_lifecycle(form, cx))
                    .child(connection_form::render_schemas(form, cx.weak_entity(), cx)),
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
                el.child(
                    div()
                        .id("connection-form-error-accessibility")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .child(Alert::error("connection-form-error", error)),
                )
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
                            .map(|mut button| {
                                button.interactivity().tooltip(
                                    StatusTooltip::new("Save Connection", "")
                                        .for_action(&SaveConnection, Some("ConnectionSettings")),
                                );
                                button
                            })
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                    ),
            )
            .into_any_element()
    }
}

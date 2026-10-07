use super::settings_view::{setting_row, settings_dialog_size, settings_panel};
use super::*;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    alert::Alert,
    combobox::Combobox,
    h_flex,
    input::Textarea,
    select::Select,
    setting::{SettingGroup, SettingItem, SettingPage},
    switch::Switch,
    v_flex,
};

const DIALOG_REMS: f32 = 40.;

/// The width of a form dialog, like the sign-in editor, in `window`.
pub(super) fn dialog_width(window: &Window) -> Pixels {
    let rem = window.rem_size();
    (rem * DIALOG_REMS).min(window.viewport_size().width - rem * 4.)
}

/// A row of Connection Settings. Qrow renders its control from the open form
/// on each frame, so the control follows the other fields. A `stacked` row
/// puts the control below its label at the full width.
pub(super) fn connection_row(
    qrow: &WeakEntity<Qrow>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    keywords: &[&'static str],
    stacked: bool,
    control: impl Fn(&Qrow, &ProfileEditor, &mut Window, &mut Context<Qrow>) -> AnyElement + 'static,
) -> SettingItem {
    let qrow = qrow.clone();
    setting_row(
        title.into(),
        description.into(),
        keywords,
        stacked,
        move |window: &mut Window, cx: &mut App| {
            qrow.update(cx, |this, cx| match &this.form {
                Some(form) => control(this, form, window, cx),
                None => div().into_any_element(),
            })
            .unwrap_or_else(|_| div().into_any_element())
        },
    )
}

/// A text input of the connection form, by field index.
pub(super) fn form_input(form: &ProfileEditor, index: usize, label: &'static str) -> AnyElement {
    Input::new(&form.fields[index])
        .focus_ring(false)
        .id(connection_form::FIELD_IDS[index])
        .w_full()
        .disabled(form.saving.is_some())
        .aria_label(label)
        .into_any_element()
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
            let body = weak.clone();
            let footer = weak.update(cx, |this, cx| this.profile_footer(cx)).ok();
            // Resolve the popup against the current window so its footer stays visible.
            let (width, height) = settings_dialog_size(window);
            let viewport = window.viewport_size();
            dialog
                .title("Connection Settings")
                .w(width)
                .h(height)
                .margin_top((viewport.height - height) / 2.)
                .overlay_closable(false)
                .on_ok(move |_, _, cx| {
                    let _ = save.update(cx, |this, cx| this.save_profile(cx));
                    // The worker result closes the popup only after a successful save.
                    false
                })
                .content(move |content, window, cx| {
                    let panel = body
                        .update(cx, |this, cx| this.profile_content(window, cx))
                        .ok();
                    content.children(panel)
                })
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
        let qrow = cx.weak_entity();
        let panel = settings_panel("connection-settings", window, cx)
            .page(self.general_page(form, &qrow, cx))
            .page(connection_form::catalog_page(form, &qrow, cx))
            .page(self.dbt_page(form, &qrow, cx))
            .pages(
                self.settings
                    .assistant
                    .enabled
                    .then(|| connection_form::assistant_page(&qrow)),
            );
        // Match the gap the dialog leaves above the footer, as Settings does.
        div()
            .key_context("ConnectionSettings")
            .on_action(cx.listener(|this, _: &SaveConnection, _, cx| this.save_profile(cx)))
            .size_full()
            .mt_2()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(panel)
            .into_any_element()
    }

    /// The server, the sign-in, and the session of the connection.
    fn general_page(
        &self,
        form: &ProfileEditor,
        qrow: &WeakEntity<Qrow>,
        cx: &mut Context<Self>,
    ) -> SettingPage {
        let postgres = connection_form::chosen(
            &form.database_type,
            &connection_form::database_type_choices(),
            cx,
        ) == crate::model::DatabaseType::Postgres;
        let uses_sign_in = !postgres && connection_form::uses_sign_in(&form.authentication, cx);
        let chosen_sign_in =
            connection_form::chosen_sign_in(&form.sign_in, &form.sign_in_choices, cx);
        let sign_in_description = chosen_sign_in
            .map(|id| match self.oidc.identity(id) {
                Some(identity) => format!("Signed in as {}.", identity.display()),
                None => "Not signed in. Sign in from the Sign-ins sidebar.".to_owned(),
            })
            .unwrap_or_else(|| {
                if form.sign_in_choices.is_empty() {
                    "Add a sign-in with New sign-in… in the list.".to_owned()
                } else {
                    "Choose the sign-in that this connection uses.".to_owned()
                }
            });
        let keep = connection_form::keeps_connected(&form.idle_behavior, cx);
        let fields = SettingGroup::new()
            .item(connection_row(
                qrow,
                "Connection Type",
                "",
                &["connector", "type"],
                false,
                |_, form, _, _| {
                    Select::new(&form.database_type)
                        .id("connection-database-type")
                        .w_full()
                        .disabled(form.saving.is_some())
                        .accessibility_label("Connection Type")
                        .into_any_element()
                },
            ))
            .item(connection_row(
                qrow,
                "Name",
                "Shown in the connections sidebar.",
                &["connection"],
                false,
                |_, form, _, _| form_input(form, 0, "Name"),
            ))
            .item(connection_row(
                qrow,
                "Host",
                "Hostname or IP address of the server.",
                &["server", "address", "kyuubi"],
                false,
                |_, form, _, _| form_input(form, 1, "Host"),
            ))
            .item(connection_row(
                qrow,
                "Port",
                "Port on that host.",
                &["server"],
                false,
                |_, form, _, _| form_input(form, 2, "Port"),
            ))
            .items(postgres.then(|| {
                connection_row(
                    qrow,
                    "TLS Mode",
                    match connection_form::chosen(
                        &form.postgres_ssl_mode,
                        &connection_form::postgres_ssl_mode_choices(),
                        cx,
                    ) {
                        crate::model::PostgresSslMode::Disable => "Connects without encryption.",
                        crate::model::PostgresSslMode::Require => {
                            "Encrypts without checking the server certificate."
                        }
                        crate::model::PostgresSslMode::VerifyFull => {
                            "Checks the server certificate and hostname."
                        }
                    },
                    &["ssl", "encryption", "security"],
                    false,
                    |_, form, _, _| {
                        Select::new(&form.postgres_ssl_mode)
                            .id("connection-postgres-ssl-mode")
                            .w_full()
                            .disabled(form.saving.is_some())
                            .accessibility_label("TLS Mode")
                            .into_any_element()
                    },
                )
            }))
            .items((!postgres).then(|| {
                connection_row(
                    qrow,
                    "TLS",
                    if uses_sign_in && !form.tls {
                        "Without TLS, others on the network can read and use the access token."
                    } else {
                        "Encrypts the connection. The server must accept TLS on this port."
                    },
                    &["ssl", "encryption", "security"],
                    false,
                    |_, form, _, cx| {
                        Switch::new("connection-tls")
                            .checked(form.tls)
                            .disabled(form.saving.is_some())
                            .accessibility_label("TLS")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                if let Some(form) = &mut this.form {
                                    form.tls = *checked;
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                    },
                )
            }))
            .items((!postgres).then(|| {
                connection_row(
                    qrow,
                    "Authentication",
                    "Several connections can share one sign-in, each with its own username.",
                    &["sign-in", "ldap", "password", "oidc"],
                    false,
                    |_, form, _, _| {
                        Select::new(&form.authentication)
                            .focus_ring(false)
                            .id("connection-authentication")
                            .w_full()
                            .disabled(form.saving.is_some())
                            .accessibility_label("Authentication")
                            .into_any_element()
                    },
                )
            }))
            .items(uses_sign_in.then(|| {
                connection_row(
                    qrow,
                    "Sign-In",
                    sign_in_description,
                    &["oidc", "identity", "token"],
                    false,
                    |this, form, _, cx| this.sign_in_field(form, cx),
                )
            }))
            .item(connection_row(
                qrow,
                "Username",
                "The database account used to connect.",
                &["user", "account", "login"],
                false,
                |_, form, _, _| form_input(form, 3, "Username"),
            ))
            .items((!uses_sign_in).then(|| {
                connection_row(
                    qrow,
                    "Password",
                    "The password for this account.",
                    &["ldap", "secret"],
                    false,
                    |_, form, _, _| form_input(form, 4, "Password"),
                )
            }))
            .item(connection_row(
                qrow,
                "Initial Database",
                "Selected when the session opens.",
                &["schema", "database", "use"],
                false,
                |_, form, _, _| form_input(form, 5, "Initial Database"),
            ))
            .item(connection_row(
                qrow,
                "Session Parameters",
                "JSON object with setting names and string values.",
                &["spark", "conf", "configuration", "json"],
                true,
                |_, form, _, _| {
                    Textarea::new(&form.parameters)
                        .focus_ring(false)
                        .w_full()
                        .disabled(form.saving.is_some())
                        .font_family("Menlo")
                        .aria_label("Session Parameters")
                        .into_any_element()
                },
            ))
            .item(connection_row(
                qrow,
                "Response Timeout",
                if postgres {
                    "Seconds to wait for setup and cancellation, from 10 to 3600."
                } else {
                    "Seconds to wait for a server response, from 10 to 3600."
                },
                &["timeout", "seconds"],
                false,
                |_, form, _, _| form_input(form, 14, "Response Timeout in Seconds"),
            ))
            .item(connection_row(
                qrow,
                "Idle Behavior",
                "Releasing keeps SQL and results, but drops temporary views and unfetched rows.",
                &["idle", "disconnect", "keep-alive", "release"],
                false,
                |_, form, _, _| {
                    Select::new(&form.idle_behavior)
                        .focus_ring(false)
                        .id("connection-idle-behavior")
                        .w_full()
                        .disabled(form.saving.is_some())
                        .accessibility_label("Idle Behavior")
                        .into_any_element()
                },
            ))
            .items((!keep).then(|| {
                connection_row(
                    qrow,
                    "Idle Timeout",
                    "Seconds of inactivity before the session is released.",
                    &["idle", "seconds"],
                    false,
                    |_, form, _, _| form_input(form, 7, "Idle Timeout in Seconds"),
                )
            }))
            .items(if keep {
                vec![
                    connection_row(
                        qrow,
                        "Keep-Alive Interval",
                        "Seconds between keep-alive queries.",
                        &["idle", "seconds"],
                        false,
                        |_, form, _, _| form_input(form, 8, "Keep-Alive Interval in Seconds"),
                    ),
                    connection_row(
                        qrow,
                        "Keep-Alive Query",
                        "A light, read-only query that keeps the engine active while idle.",
                        &["idle", "sql"],
                        false,
                        |_, form, _, _| form_input(form, 9, "Keep-Alive Query"),
                    ),
                ]
            } else {
                vec![]
            });
        SettingPage::new("General")
            .description(if postgres {
                "Postgres"
            } else {
                "Spark (HiveServer2)"
            })
            .default_open(true)
            .resettable(false)
            .group(fields)
    }

    /// The sign-in list, with a button that adds a sign-in.
    fn sign_in_field(&self, form: &ProfileEditor, cx: &mut Context<Self>) -> AnyElement {
        let chosen = connection_form::chosen_sign_in(&form.sign_in, &form.sign_in_choices, cx);
        let label = chosen
            .and_then(|id| {
                form.sign_in_choices
                    .iter()
                    .find(|(choice, _)| *choice == id)
            })
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let qrow = cx.weak_entity();
        // The combobox has no accessibility of its own in GPUI Kit 0.6.6, so
        // this element names it and gives its value.
        div()
            .id("connection-sign-in")
            .test_support()
            .role(Role::ComboBox)
            .aria_label("Sign-In")
            .aria_value(label)
            .w_full()
            .child(
                Combobox::new(&form.sign_in)
                    .focus_ring(false)
                    .w_full()
                    .disabled(form.saving.is_some())
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
                            .label("New sign-in…")
                            .on_click(move |_, window, cx| {
                                let _ = qrow.update(cx, |this, cx| {
                                    this.open_sign_in_editor(None, true, window, cx)
                                });
                            })
                    }),
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
                                    StatusTooltip::new("Save connection", "")
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

//! Sign-in to Codex with a ChatGPT account.
use super::*;

impl Qrow {
    pub(super) fn begin_assistant_sign_in(&mut self, cx: &mut Context<Self>) {
        if self.assistant_command(AssistantCommand::Login, cx) {
            self.assistant_state.sign_in = SignIn::Starting;
            cx.notify();
        }
    }

    pub(super) fn cancel_assistant_sign_in(&mut self, cx: &mut Context<Self>) {
        if let SignIn::Waiting { login_id, .. } = std::mem::take(&mut self.assistant_state.sign_in)
        {
            self.assistant_command(AssistantCommand::CancelLogin(login_id), cx);
        }
        cx.notify();
    }
}

impl AssistantPane {
    /// Replaces the transcript while Codex has no account.
    pub(super) fn sign_in(&self, qrow: &Qrow, cx: &Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let sign_in = &qrow.assistant_state.sign_in;
        Empty::new()
            .size_full()
            .border_0()
            .header(
                EmptyHeader::new()
                    .media(
                        EmptyMedia::new()
                            .with_variant(EmptyMediaVariant::Icon)
                            .child(Icon::new(AssetIconName::Bot)),
                    )
                    .title(EmptyTitle::new().child("Sign in to Codex"))
                    .description(EmptyDescription::new().child(
                        "The assistant uses your Codex account. Sign in with ChatGPT to use your plan.",
                    )),
            )
            .content(match sign_in {
                SignIn::Waiting { url, .. } => {
                    let url = url.clone();
                    EmptyContent::new()
                        .child(
                            h_flex()
                                .id("assistant-sign-in-waiting")
                                .test_support()
                                .role(Role::Status)
                                .aria_label("Continue sign-in in your browser")
                                .gap_2()
                                .text_color(muted)
                                .child(Spinner::new().small().color(muted))
                                .child("Continue sign-in in your browser"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("assistant-sign-in-reopen")
                                        .label("Reopen page")
                                        .tooltip("Reopen sign-in page")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                                .child(
                                    Button::new("assistant-sign-in-cancel")
                                        .label("Cancel")
                                        .accessibility_label("Cancel sign-in")
                                        .on_click(on_qrow(&self.qrow, |this, _, _, cx| {
                                            this.cancel_assistant_sign_in(cx)
                                        })),
                                ),
                        )
                }
                SignIn::Idle | SignIn::Starting | SignIn::Failed(_) => EmptyContent::new()
                    .child(
                        Button::new("assistant-sign-in")
                            .label("Sign in with ChatGPT…")
                            .loading(*sign_in == SignIn::Starting)
                            .disabled(*sign_in == SignIn::Starting)
                            .on_click(on_qrow(&self.qrow, |this, _, _, cx| {
                                this.begin_assistant_sign_in(cx)
                            })),
                    )
                    .when_some(
                        match sign_in {
                            SignIn::Failed(error) => Some(error.clone()),
                            _ => None,
                        },
                        |content, error| {
                            content.child(
                                v_flex()
                                    .id("assistant-sign-in-error")
                                    .test_support()
                                    .role(Role::Alert)
                                    .aria_label(format!("Couldn't sign in. {error}"))
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_color(
                                                cx.theme().semantic_tokens().colors.destructive,
                                            )
                                            .child("Couldn't sign in. Try again."),
                                    )
                                    .child(div().text_xs().text_color(muted).child(error)),
                            )
                        },
                    ),
            })
    }
}

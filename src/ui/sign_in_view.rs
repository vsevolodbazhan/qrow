//! Settings > Sign-ins: reusable OpenID Connect sign-ins, their browser
//! sign-in, sign-out, and the sessions that use them.
use super::environment::Browser;
use super::settings_view::setting_id;
use super::*;
use crate::{
    model::{Authentication, Identity, SignIn},
    oidc::{self, Failure, Status},
};
use gpui_kit::component::{
    h_flex,
    label::Label,
    setting::{RenderOptions, SettingGroup, SettingItem, SettingPage},
    v_flex,
};
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
};

/// The result of background sign-in work.
enum Outcome {
    SignedIn(Uuid, Result<Identity, (Failure, String)>),
    SignedOut(Uuid, Result<(), String>),
    Removed(Uuid, Result<(), String>),
    Retried(Uuid, Result<(), String>),
}

/// The inline form that adds or edits a sign-in.
pub(super) struct SignInEditor {
    /// `None` adds a new sign-in.
    id: Option<Uuid>,
    /// Name, issuer, client ID, scopes, resource, database hosts, callback port.
    fields: Vec<Entity<InputState>>,
    error: Option<String>,
    /// Enter in a field saves the form.
    _subscriptions: Vec<Subscription>,
}

const EDITOR_FIELDS: [(&str, &str, &str); 7] = [
    ("Name", "Shown in Connection Settings.", "Company"),
    (
        "Issuer",
        "The issuer URL of the provider, for example the URL of a Keycloak realm.",
        "https://id.example.com/realms/data",
    ),
    (
        "Client ID",
        "The public client that the provider registered for Qrow. Qrow uses no client secret.",
        "qrow-desktop",
    ),
    (
        "Scopes",
        "Separate scopes with spaces. Qrow always requests openid. Add offline_access if the provider requires it for refresh tokens.",
        "profile email",
    ),
    (
        "Resource",
        "Optional. An RFC 8707 resource indicator for the access tokens.",
        "",
    ),
    (
        "Database hosts",
        "The Kyuubi hosts that can receive the access tokens. Separate hosts with spaces or commas.",
        "kyuubi.example.com",
    ),
    (
        "Callback port",
        "Optional. The loopback port of the browser callback. Leave empty to use an available port.",
        "",
    ),
];

pub(super) struct SignInState {
    /// Browser sign-ins that run, with their cancellation flags.
    pending: HashMap<Uuid, Arc<AtomicBool>>,
    signing_out: HashSet<Uuid>,
    retrying: HashSet<Uuid>,
    /// The last failure of an action, shown beside the sign-in.
    errors: HashMap<Uuid, String>,
    outcomes: (mpsc::Sender<Outcome>, mpsc::Receiver<Outcome>),
    /// Authorization URLs to open on the main thread.
    urls: (mpsc::Sender<String>, mpsc::Receiver<String>),
    browser: Option<Browser>,
    pub(super) editor: Option<SignInEditor>,
}

impl SignInState {
    pub(super) fn new(browser: Option<Browser>) -> Self {
        Self {
            pending: HashMap::new(),
            signing_out: HashSet::new(),
            retrying: HashSet::new(),
            errors: HashMap::new(),
            outcomes: mpsc::channel(),
            urls: mpsc::channel(),
            browser,
            editor: None,
        }
    }

    /// Stops browser sign-ins, for example when the window closes.
    pub(super) fn cancel_all(&self) {
        for cancel in self.pending.values() {
            cancel.store(true, Ordering::SeqCst);
        }
    }
}

fn split_list(value: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for item in value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|item| !item.is_empty())
    {
        if !items.iter().any(|existing| existing == item) {
            items.push(item.to_owned());
        }
    }
    items
}

/// Reads the editor fields into a sign-in based on `base`.
fn parse_editor(values: &[String], base: &SignIn) -> anyhow::Result<SignIn> {
    let mut sign_in = base.clone();
    sign_in.name = values[0].trim().to_owned();
    sign_in.issuer = values[1].trim().to_owned();
    sign_in.client_id = values[2].trim().to_owned();
    sign_in.scopes = split_list(&values[3])
        .into_iter()
        .filter(|scope| scope != "openid")
        .collect();
    let resource = values[4].trim();
    sign_in.resource = (!resource.is_empty()).then(|| resource.to_owned());
    sign_in.allowed_hosts = split_list(&values[5]);
    let port = values[6].trim();
    sign_in.callback_port = if port.is_empty() {
        0
    } else {
        port.parse()
            .map_err(|_| anyhow::anyhow!("Callback port must be a number from 1 to 65535."))?
    };
    sign_in.validate()?;
    Ok(sign_in)
}

impl Qrow {
    /// The credentials of a connection: its password from Keychain, or the
    /// access tokens of its sign-in.
    pub(super) fn credential_provider(&self) -> crate::worker::CredentialProvider {
        let credentials = self.credentials.clone();
        let service = self.oidc.clone();
        Arc::new(move |profile: &Profile| match profile.authentication {
            Authentication::Password => credentials
                .password(profile.id)
                .map(|password| password.into()),
            Authentication::Oidc { .. } => service.secret(profile),
        })
    }

    /// The names of the connections that use a sign-in.
    pub(super) fn connections_using(&self, id: Uuid) -> Vec<String> {
        self.profiles
            .iter()
            .filter(|profile| profile.authentication.sign_in() == Some(id))
            .map(|profile| profile.name.clone())
            .collect()
    }

    fn uses_sign_in(&self, profile: Option<Uuid>, id: Uuid) -> bool {
        profile.is_some_and(|profile| {
            self.profiles.iter().any(|candidate| {
                candidate.id == profile && candidate.authentication.sign_in() == Some(id)
            })
        })
    }

    /// Whether a session that uses the sign-in runs work now.
    pub(super) fn sign_in_busy(&self, id: Uuid) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.busy && self.uses_sign_in(tab.worker_profile, id))
    }

    /// Releases the sessions of a sign-in after a sign-out or a change of
    /// identity. SQL and downloaded results stay.
    fn release_sign_in_sessions(&mut self, id: Uuid) {
        let affected: Vec<usize> = (0..self.tabs.len())
            .filter(|&index| self.uses_sign_in(self.tabs[index].worker_profile, id))
            .collect();
        for index in affected {
            release_session(&mut self.tabs[index]);
        }
    }

    fn sync_sign_ins(&mut self, cx: &mut Context<Self>) {
        self.oidc.configure(&self.sign_ins);
        self.changed(cx);
    }

    pub(super) fn start_sign_in(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.demo
            || self.sign_in_ui.pending.contains_key(&id)
            || self.sign_in_ui.signing_out.contains(&id)
            || (self.oidc.identity(id).is_some() && self.sign_in_busy(id))
        {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.sign_in_ui.pending.insert(id, cancel.clone());
        self.sign_in_ui.errors.remove(&id);
        let service = self.oidc.clone();
        let outcomes = self.sign_in_ui.outcomes.0.clone();
        let urls = self.sign_in_ui.urls.0.clone();
        let browser = self.sign_in_ui.browser.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let open = |url: &str| -> anyhow::Result<()> {
                match &browser {
                    Some(browser) => browser(url),
                    None => {
                        urls.send(url.to_owned())?;
                        let _ = wake.try_send(());
                        Ok(())
                    }
                }
            };
            let result = service
                .sign_in(id, &cancel, &open)
                .map_err(|error| (oidc::failure(&error), format!("{error:#}")));
            let _ = outcomes.send(Outcome::SignedIn(id, result));
            let _ = wake.try_send(());
        });
        cx.notify();
    }

    pub(super) fn cancel_sign_in(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(cancel) = self.sign_in_ui.pending.get(&id) {
            cancel.store(true, Ordering::SeqCst);
        }
        cx.notify();
    }

    pub(super) fn sign_out(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.sign_in_busy(id)
            || self.sign_in_ui.pending.contains_key(&id)
            || !self.sign_in_ui.signing_out.insert(id)
        {
            return;
        }
        self.sign_in_ui.errors.remove(&id);
        let service = self.oidc.clone();
        let outcomes = self.sign_in_ui.outcomes.0.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let result = service.sign_out(id).map_err(|error| format!("{error:#}"));
            let _ = outcomes.send(Outcome::SignedOut(id, result));
            let _ = wake.try_send(());
        });
        cx.notify();
    }

    fn retry_sign_in(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if !self.sign_in_ui.retrying.insert(id) {
            return;
        }
        let service = self.oidc.clone();
        let outcomes = self.sign_in_ui.outcomes.0.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let result = service.retry(id).map_err(|error| format!("{error:#}"));
            let _ = outcomes.send(Outcome::Retried(id, result));
            let _ = wake.try_send(());
        });
        cx.notify();
    }

    /// Removes a sign-in that no connection uses, and its tokens.
    fn remove_sign_in(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if !self.connections_using(id).is_empty()
            || self.sign_in_ui.pending.contains_key(&id)
            || self.sign_in_ui.signing_out.contains(&id)
        {
            return;
        }
        // Keychain work stays off the GPUI thread. Removal completes after
        // the tokens are deleted, so a failure keeps the sign-in.
        self.sign_in_ui.signing_out.insert(id);
        let service = self.oidc.clone();
        let outcomes = self.sign_in_ui.outcomes.0.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let result = service.sign_out(id).map_err(|error| format!("{error:#}"));
            let _ = outcomes.send(Outcome::Removed(id, result));
            let _ = wake.try_send(());
        });
        cx.notify();
    }

    /// Applies the results of background sign-in work. Returns whether
    /// something changed.
    pub(super) fn tick_sign_ins(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let urls: Vec<String> = self.sign_in_ui.urls.1.try_iter().collect();
        for url in urls {
            cx.open_url(&url);
        }
        let outcomes: Vec<Outcome> = self.sign_in_ui.outcomes.1.try_iter().collect();
        for outcome in outcomes {
            changed = true;
            match outcome {
                Outcome::SignedIn(id, result) => {
                    self.sign_in_ui.pending.remove(&id);
                    match result {
                        Ok(identity) => {
                            let previous = self
                                .sign_ins
                                .iter()
                                .find(|sign_in| sign_in.id == id)
                                .and_then(|sign_in| sign_in.identity.clone());
                            if previous.is_some_and(|previous| previous.subject != identity.subject)
                            {
                                self.release_sign_in_sessions(id);
                            }
                            if let Some(sign_in) =
                                self.sign_ins.iter_mut().find(|sign_in| sign_in.id == id)
                            {
                                sign_in.identity = Some(identity);
                            }
                            self.sync_sign_ins(cx);
                        }
                        Err((Failure::Cancelled, _)) => {}
                        Err((_, message)) => {
                            self.sign_in_ui.errors.insert(id, message);
                        }
                    }
                }
                Outcome::SignedOut(id, result) => {
                    self.sign_in_ui.signing_out.remove(&id);
                    match result {
                        Ok(()) => {
                            self.release_sign_in_sessions(id);
                            if let Some(sign_in) =
                                self.sign_ins.iter_mut().find(|sign_in| sign_in.id == id)
                            {
                                sign_in.identity = None;
                            }
                            self.sync_sign_ins(cx);
                        }
                        Err(message) => {
                            self.sign_in_ui.errors.insert(id, message);
                        }
                    }
                }
                Outcome::Removed(id, result) => {
                    self.sign_in_ui.signing_out.remove(&id);
                    match result {
                        Ok(()) => {
                            self.sign_ins.retain(|sign_in| sign_in.id != id);
                            self.sign_in_ui.errors.remove(&id);
                            self.sync_sign_ins(cx);
                        }
                        Err(message) => {
                            self.sign_in_ui.errors.insert(id, message);
                        }
                    }
                }
                Outcome::Retried(id, result) => {
                    self.sign_in_ui.retrying.remove(&id);
                    if let Err(message) = result {
                        self.sign_in_ui.errors.insert(id, message);
                    } else {
                        self.sign_in_ui.errors.remove(&id);
                    }
                }
            }
        }
        changed
    }

    pub(super) fn open_sign_in_editor(
        &mut self,
        id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = id
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id))
            .cloned()
            .unwrap_or_default();
        let values = [
            base.name.clone(),
            base.issuer.clone(),
            base.client_id.clone(),
            base.scopes.join(" "),
            base.resource.clone().unwrap_or_default(),
            base.allowed_hosts.join(" "),
            if base.callback_port == 0 {
                String::new()
            } else {
                base.callback_port.to_string()
            },
        ];
        let fields: Vec<_> = values
            .into_iter()
            .zip(EDITOR_FIELDS)
            .map(|(value, (_, _, placeholder))| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            })
            .collect();
        fields[0].update(cx, |field, cx| field.focus(window, cx));
        let subscriptions = fields
            .iter()
            .map(|field| {
                cx.subscribe_in(field, window, |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_sign_in_editor(cx);
                    }
                })
            })
            .collect();
        self.sign_in_ui.editor = Some(SignInEditor {
            id,
            fields,
            error: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn close_sign_in_editor(&mut self, cx: &mut Context<Self>) {
        self.sign_in_ui.editor = None;
        cx.notify();
    }

    fn save_sign_in_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.sign_in_ui.editor else {
            return;
        };
        let values: Vec<String> = editor
            .fields
            .iter()
            .map(|field| field.read(cx).value().to_string())
            .collect();
        let base = editor
            .id
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id))
            .cloned()
            .unwrap_or_default();
        let result = parse_editor(&values, &base).and_then(|sign_in| {
            anyhow::ensure!(
                !self
                    .sign_ins
                    .iter()
                    .any(|other| other.id != sign_in.id && other.name == sign_in.name),
                "A sign-in with this name already exists."
            );
            anyhow::ensure!(
                base.identity.is_none() || base.token_requirements_eq(&sign_in),
                "Sign out before you change the issuer, client ID, scopes, or resource."
            );
            Ok(sign_in)
        });
        match result {
            Ok(sign_in) => {
                match self
                    .sign_ins
                    .iter_mut()
                    .find(|other| other.id == sign_in.id)
                {
                    Some(existing) => *existing = sign_in,
                    None => self.sign_ins.push(sign_in),
                }
                self.sign_in_ui.editor = None;
                self.sync_sign_ins(cx);
            }
            Err(error) => {
                if let Some(editor) = &mut self.sign_in_ui.editor {
                    editor.error = Some(error.to_string());
                }
                cx.notify();
            }
        }
    }

    /// The text and the recovery actions of a sign-in.
    fn sign_in_status(&self, id: Uuid) -> (String, Vec<SignInAction>) {
        use SignInAction::*;
        if self.demo {
            return ("The demo does not sign in.".into(), vec![]);
        }
        if self.sign_in_ui.pending.contains_key(&id) {
            return (
                "Waiting for the browser. Finish the sign-in there.".into(),
                vec![Cancel],
            );
        }
        if self.sign_in_ui.signing_out.contains(&id) {
            return ("Signing out…".into(), vec![]);
        }
        match self.oidc.status(id) {
            Status::SignedOut => ("Not signed in".into(), vec![SignIn]),
            Status::SignedIn(identity) => (
                format!("Signed in as {}", identity.display()),
                vec![SignOut],
            ),
            Status::SignInRequired(identity, _) => (
                format!(
                    "The sign-in of {} has expired. Sign in again.",
                    identity.display()
                ),
                vec![SignIn, SignOut],
            ),
            Status::NetworkFailure(identity, message) => (
                format!(
                    "Signed in as {}. Qrow could not reach the provider: {message}",
                    identity.display()
                ),
                vec![Retry, SignOut],
            ),
        }
    }

    pub(super) fn sign_ins_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let owner = cx.weak_entity();
        let page = SettingPage::new("Sign-ins").resettable(false);
        if let Some(editor) = &self.sign_in_ui.editor {
            return page.group(self.sign_in_editor_group(editor, owner));
        }
        let mut page = page;
        for sign_in in &self.sign_ins {
            page = page.group(self.sign_in_group(sign_in, owner.clone()));
        }
        let empty = self.sign_ins.is_empty();
        page.group(
            SettingGroup::new().item(row(
                "sign-ins-add".into(),
                if empty { "No sign-ins" } else { "Add another sign-in" }.into(),
                if empty {
                    "A sign-in lets connections use OpenID Connect. Several connections can use one sign-in, and each keeps its own database username."
                } else {
                    "Use a separate sign-in for each provider account."
                }
                .into(),
                None,
                move |_, _| {
                    let owner = owner.clone();
                    Button::new("add-sign-in")
                        .label("Add sign-in…")
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.open_sign_in_editor(None, window, cx)
                            });
                        })
                        .into_any_element()
                },
            )),
        )
    }

    fn sign_in_group(&self, sign_in: &SignIn, owner: WeakEntity<Qrow>) -> SettingGroup {
        let id = sign_in.id;
        let (status, actions) = self.sign_in_status(id);
        let busy = self.sign_in_busy(id);
        let retrying = self.sign_in_ui.retrying.contains(&id);
        let error = self.sign_in_ui.errors.get(&id).cloned();
        let users = self.connections_using(id);
        let working =
            self.sign_in_ui.pending.contains_key(&id) || self.sign_in_ui.signing_out.contains(&id);
        let account_owner = owner.clone();
        let account = row(
            format!("sign-in-{id}-account").into(),
            "Account".into(),
            status.into(),
            error.map(SharedString::from),
            move |_, _| {
                let buttons = actions.iter().map(|action| {
                    let owner = account_owner.clone();
                    let action = *action;
                    // Signing out or in as another account releases sessions,
                    // so it waits until their work ends.
                    let blocked =
                        busy && matches!(action, SignInAction::SignOut | SignInAction::SignIn);
                    Button::new(SharedString::from(format!(
                        "sign-in-{id}-{}",
                        action.slug()
                    )))
                    .label(action.label())
                    .disabled(blocked || (retrying && action == SignInAction::Retry))
                    .when(blocked, |button| {
                        button.tooltip("Wait for queries that use this sign-in to finish.")
                    })
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| match action {
                            SignInAction::SignIn => this.start_sign_in(id, cx),
                            SignInAction::Cancel => this.cancel_sign_in(id, cx),
                            SignInAction::SignOut => this.sign_out(id, cx),
                            SignInAction::Retry => this.retry_sign_in(id, cx),
                        });
                    })
                });
                h_flex()
                    .gap_2()
                    .justify_end()
                    .children(buttons)
                    .into_any_element()
            },
        );
        let edit = owner.clone();
        let provider = row(
            format!("sign-in-{id}-provider").into(),
            "Provider".into(),
            format!(
                "{} · client {} · database hosts {}",
                sign_in.issuer,
                sign_in.client_id,
                sign_in.allowed_hosts.join(", ")
            )
            .into(),
            None,
            move |_, _| {
                let edit = edit.clone();
                Button::new(SharedString::from(format!("sign-in-{id}-edit")))
                    .label("Edit…")
                    .disabled(working)
                    .on_click(move |_, window, cx| {
                        let _ = edit.update(cx, |this, cx| {
                            this.open_sign_in_editor(Some(id), window, cx)
                        });
                    })
                    .into_any_element()
            },
        );
        let in_use = !users.is_empty();
        let connections = row(
            format!("sign-in-{id}-connections").into(),
            "Connections".into(),
            if in_use {
                format!(
                    "Used by {}. To remove the sign-in, choose another authentication for these connections first.",
                    users.join(", ")
                )
            } else {
                "No connection uses this sign-in.".to_owned()
            }
            .into(),
            None,
            move |_, _| {
                let remove = owner.clone();
                Button::new(SharedString::from(format!("sign-in-{id}-remove")))
                    .label("Remove")
                    .with_variant(ButtonVariant::Danger)
                    .disabled(working || in_use)
                    .on_click(move |_, _, cx| {
                        let _ = remove.update(cx, |this, cx| this.remove_sign_in(id, cx));
                    })
                    .into_any_element()
            },
        );
        SettingGroup::new()
            .title(sign_in.name.clone())
            .item(account)
            .item(provider)
            .item(connections)
    }

    fn sign_in_editor_group(&self, editor: &SignInEditor, owner: WeakEntity<Qrow>) -> SettingGroup {
        let editing = editor
            .id
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id));
        let signed_in = editing.is_some_and(|sign_in| sign_in.identity.is_some());
        let title = match editing {
            Some(sign_in) => format!("Edit \u{201c}{}\u{201d}", sign_in.name),
            None => "New sign-in".to_owned(),
        };
        let mut group = SettingGroup::new().title(title);
        if signed_in {
            group =
                group.description("Sign out to change the issuer, client ID, scopes, or resource.");
        }
        for (index, (label, description, _)) in EDITOR_FIELDS.into_iter().enumerate() {
            let input = editor.fields[index].clone();
            let locked = signed_in && (1..=4).contains(&index);
            let row_id = format!("{}-row", setting_id(&format!("sign-in {label}")));
            group = group.item(stacked_row(
                row_id.into(),
                label.into(),
                description.into(),
                move |_, _| {
                    Input::new(&input)
                        .id(setting_id(&format!("sign-in {label}")))
                        .w_full()
                        .disabled(locked)
                        .aria_label(label)
                        .into_any_element()
                },
            ));
        }
        let cancel = owner.clone();
        let error = editor.error.clone();
        group.item(SettingItem::render(
            move |_: &RenderOptions, _: &mut Window, cx: &mut App| {
                let cancel = cancel.clone();
                let save = owner.clone();
                let error = error.clone();
                v_flex()
                    .w_full()
                    .gap_2()
                    .when_some(error, |el, error| {
                        el.child(
                            div()
                                .id("sign-in-form-error")
                                .test_support()
                                .role(Role::Alert)
                                .aria_label(error.clone())
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("cancel-sign-in-editor")
                                    .label("Cancel")
                                    .on_click(move |_, _, cx| {
                                        let _ = cancel
                                            .update(cx, |this, cx| this.close_sign_in_editor(cx));
                                    }),
                            )
                            .child(
                                Button::new("save-sign-in-editor")
                                    .primary()
                                    .label("Save")
                                    .on_click(move |_, _, cx| {
                                        let _ = save
                                            .update(cx, |this, cx| this.save_sign_in_editor(cx));
                                    }),
                            ),
                    )
            },
        ))
    }
}

/// Stops the worker of a tab. Its SQL, results, and Logs history stay.
pub(super) fn release_session(tab: &mut Tab) {
    if tab.busy {
        return;
    }
    if let Some(worker) = tab.worker.take() {
        worker.shutdown();
    }
    tab.worker_profile = None;
    tab.connected = false;
    tab.cancelling = false;
    tab.more = false;
    tab.pending_page = None;
    tab.status = "Not connected".into();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignInAction {
    SignIn,
    Cancel,
    SignOut,
    Retry,
}

impl SignInAction {
    fn label(self) -> &'static str {
        match self {
            Self::SignIn => "Sign in…",
            Self::Cancel => "Cancel",
            Self::SignOut => "Sign out",
            Self::Retry => "Retry",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::SignIn => "sign-in",
            Self::Cancel => "cancel",
            Self::SignOut => "sign-out",
            Self::Retry => "retry",
        }
    }
}

/// A row with a title, a description, an optional error, and controls at
/// the trailing edge. The description wraps instead of pushing the controls
/// out of the page.
///
/// The description has the element ID `{id}-description`, and the error
/// `{id}-error`, so that assistive technology and tests can read them.
fn row(
    id: SharedString,
    title: SharedString,
    description: SharedString,
    error: Option<SharedString>,
    control: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> SettingItem {
    build_row(id, title, description, error, false, control)
}

/// A [`row`] with the control below the label at the full width, for text
/// fields with long values such as URLs.
fn stacked_row(
    id: SharedString,
    title: SharedString,
    description: SharedString,
    control: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> SettingItem {
    build_row(id, title, description, None, true, control)
}

fn build_row(
    id: SharedString,
    title: SharedString,
    description: SharedString,
    error: Option<SharedString>,
    stacked: bool,
    control: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> SettingItem {
    let keywords = [title.clone(), description.clone()];
    SettingItem::render(
        move |options: &RenderOptions, window: &mut Window, cx: &mut App| {
            let label = v_flex()
                .gap_1()
                .child(Label::new(title.clone()).text_sm())
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-description")))
                        .test_support()
                        .aria_label(description.clone())
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.clone()),
                )
                .when_some(error.clone(), |el, error| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("{id}-error")))
                            .test_support()
                            .role(Role::Alert)
                            .aria_label(error.clone())
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                });
            let control = div().child(control(window, cx));
            let layout = if stacked {
                Axis::Vertical
            } else {
                options.layout()
            };
            match layout {
                Axis::Horizontal => h_flex()
                    .w_full()
                    .justify_between()
                    .gap_3()
                    .child(label.flex_1().min_w_0())
                    .child(control.flex_shrink_0()),
                Axis::Vertical => v_flex()
                    .w_full()
                    .gap_3()
                    .child(label.w_full())
                    .child(control.w_full()),
            }
        },
    )
    .keywords(keywords)
}

#[cfg(test)]
mod tests {
    use super::parse_editor;
    use crate::model::{Identity, SignIn};

    fn values() -> Vec<String> {
        [
            "Company",
            " https://id.example.test/realms/data ",
            "qrow-desktop",
            "openid profile, kyuubi kyuubi",
            "",
            "kyuubi-a.example.test, kyuubi-b.example.test",
            "",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn the_editor_reads_lists_and_optional_fields() {
        let sign_in = parse_editor(&values(), &SignIn::default()).unwrap();
        assert_eq!(sign_in.issuer, "https://id.example.test/realms/data");
        assert_eq!(sign_in.scopes, vec!["profile", "kyuubi"]);
        assert_eq!(
            sign_in.allowed_hosts,
            vec!["kyuubi-a.example.test", "kyuubi-b.example.test"]
        );
        assert_eq!(sign_in.resource, None);
        assert_eq!(sign_in.callback_port, 0);
        let mut values = values();
        values[6] = "8765".into();
        values[4] = "https://kyuubi.example.test".into();
        let sign_in = parse_editor(&values, &SignIn::default()).unwrap();
        assert_eq!(sign_in.callback_port, 8765);
        assert_eq!(
            sign_in.resource.as_deref(),
            Some("https://kyuubi.example.test")
        );
        values[6] = "port".into();
        assert!(parse_editor(&values, &SignIn::default()).is_err());
    }

    #[test]
    fn the_editor_keeps_the_identifier_and_identity_of_the_sign_in() {
        let base = SignIn {
            identity: Some(Identity {
                issuer: "https://id.example.test/realms/data".into(),
                subject: "s".into(),
                name: None,
                email: None,
            }),
            ..SignIn::default()
        };
        let sign_in = parse_editor(&values(), &base).unwrap();
        assert_eq!(sign_in.id, base.id);
        assert_eq!(sign_in.identity, base.identity);
    }
}

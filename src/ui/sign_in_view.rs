//! Sign-ins: reusable authentication providers in the Sign-ins sidebar and the
//! Sign-in Settings dialog, their browser sign-in and sign-out, and the
//! sessions that use them.
use super::environment::Browser;
use super::*;
use crate::{
    model::{
        Authentication, BASE_SCOPES, Identity, SignIn, SignInProvider, extra_scopes,
        unique_sign_in_name,
    },
    oidc::{self, Failure, Status},
};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{
    ColorName, Icon,
    alert::Alert,
    button::ButtonCustomVariant,
    combobox::ComboboxEvent,
    form::{Field, Form},
    h_flex,
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// The result of background sign-in work.
enum Outcome {
    SignedIn(Uuid, Result<Identity, (Failure, String)>),
    External(Uuid, Result<(), String>),
    SignedOut(Uuid, Result<(), String>),
    Removed(Uuid, Result<(), String>),
    Retried(Uuid, Result<(), String>),
    BrowserFailed(Uuid, Arc<AtomicBool>),
}

/// One browser sign-in shared by the queries that wait for it.
struct PendingSignIn {
    cancel: Arc<AtomicBool>,
    url: Arc<Mutex<Option<String>>>,
}

/// The form of the Sign-in Settings dialog, which adds or edits a sign-in.
pub(super) struct SignInEditor {
    /// `None` adds a new sign-in.
    id: Option<Uuid>,
    /// Connection Settings opened the dialog. A new sign-in becomes the
    /// choice of its Sign-in list.
    for_connection: bool,
    /// Name, issuer, client ID, scopes, resource, callback ports.
    fields: Vec<Entity<InputState>>,
    account: Entity<InputState>,
    provider: connection_form::RowSelect,
    _provider_subscription: Subscription,
    /// The fields came from settings that another user shared.
    pasted: bool,
    error: Option<String>,
}

/// The fields of the Sign-in Settings dialog: element ID, label in title
/// case, description, and placeholder.
const EDITOR_FIELDS: [(&str, &str, &str, &str); 6] = [
    (
        "sign-in-name",
        "Name",
        "Shown in the Sign-Ins sidebar and in Connection Settings.",
        "Company",
    ),
    (
        "sign-in-issuer",
        "Issuer",
        "The issuer URL of the provider, for example a Keycloak realm URL.",
        "https://id.example.com/realms/data",
    ),
    (
        "sign-in-client-id",
        "Client ID",
        "The ID of the public client at the provider. No client secret is necessary.",
        "desktop-app",
    ),
    (
        "sign-in-scopes",
        "Scopes",
        "Optional. Other scopes that the provider requires, separated by spaces.",
        "offline_access",
    ),
    (
        "sign-in-resource",
        "Resource",
        "Optional. The resource URI, if the provider requires one.",
        "https://db.example.com",
    ),
    (
        "sign-in-callback-ports",
        "Callback Ports",
        "Optional. Comma-separated ports. Empty uses any free port.",
        "8765, 8766",
    ),
];

/// The fields that a signed-in account keeps: issuer, client ID, scopes,
/// and resource.
const TOKEN_FIELDS: std::ops::RangeInclusive<usize> = 1..=4;

pub(super) struct SignInState {
    /// Browser sign-ins that run, with their cancellation flags.
    pending: HashMap<Uuid, PendingSignIn>,
    external_pending: HashMap<Uuid, Arc<AtomicBool>>,
    signing_out: HashSet<Uuid>,
    retrying: HashSet<Uuid>,
    /// The last failure of an action, shown with the sign-in.
    errors: HashMap<Uuid, String>,
    /// The connection whose attempt produced the displayed failure.
    error_connections: HashMap<Uuid, Uuid>,
    /// Connections waiting for the shared OIDC browser attempt.
    attempt_connections: HashMap<Uuid, HashSet<Uuid>>,
    /// The statuses that the window shows. A refresh on a worker thread can
    /// change a status.
    statuses: Vec<(Uuid, Status)>,
    external_statuses: Vec<(Uuid, crate::external_auth::Status)>,
    outcomes: (mpsc::Sender<Outcome>, mpsc::Receiver<Outcome>),
    /// Authorization URLs to open on the main thread.
    urls: (mpsc::Sender<BrowserPage>, mpsc::Receiver<BrowserPage>),
    browser: Option<Browser>,
    pub(super) editor: Option<SignInEditor>,
}

type BrowserPage = (Uuid, Arc<AtomicBool>, String);

impl SignInState {
    pub(super) fn new(browser: Option<Browser>) -> Self {
        Self {
            pending: HashMap::new(),
            external_pending: HashMap::new(),
            signing_out: HashSet::new(),
            retrying: HashSet::new(),
            errors: HashMap::new(),
            error_connections: HashMap::new(),
            attempt_connections: HashMap::new(),
            statuses: Vec::new(),
            external_statuses: Vec::new(),
            outcomes: mpsc::channel(),
            urls: mpsc::channel(),
            browser,
            editor: None,
        }
    }

    pub(super) fn set_connection_error(&mut self, id: Uuid, connection: Uuid, message: String) {
        self.errors.insert(id, message);
        self.error_connections.insert(id, connection);
    }

    pub(super) fn clear_error(&mut self, id: Uuid) {
        self.errors.remove(&id);
        self.error_connections.remove(&id);
    }

    /// Stops browser sign-ins, for example when the window closes.
    pub(super) fn cancel_all(&self) {
        for cancel in self.external_pending.values() {
            cancel.store(true, Ordering::SeqCst);
        }
        for pending in self.pending.values() {
            pending.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// A query that waits for a browser sign-in. It has not reached the server.
pub(super) struct SignInWait {
    sign_in: Uuid,
    sql: String,
}

/// The sign-in to open in the browser after a sign-in error of `tab`, and the
/// SQL to run after it. A query that already ran after an automatic sign-in
/// gets none, so a sign-in that does not help ends in an error. A query that
/// the user cancelled gets none, so it does not run after the sign-in.
pub(super) fn automatic_sign_in(tab: &Tab, profiles: &[Profile]) -> Option<(Uuid, String)> {
    if tab.signed_in_for_run || tab.cancelling {
        return None;
    }
    let authentication = profiles
        .iter()
        .find(|profile| Some(profile.id) == tab.worker_profile)?
        .authentication;
    let Authentication::Oidc { sign_in } = authentication else {
        return None;
    };
    Some((sign_in, tab.submitted_sql.clone()?))
}

/// What the Sign-ins sidebar and the Sign-in Settings dialog show about the
/// account of a sign-in.
struct Account {
    /// The short account status for accessibility and status indicators.
    summary: String,
    /// The full status, for the dialog.
    detail: String,
    actions: Vec<SignInAction>,
    /// A browser sign-in or a sign-out runs.
    working: bool,
    /// The sign-in needs the user before its connections can work.
    attention: bool,
}

fn provider_choices() -> [(SignInProvider, String); 1] {
    [(SignInProvider::Oidc, "OpenID Connect".into())]
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
        .filter(|scope| !BASE_SCOPES.contains(&scope.as_str()))
        .collect();
    let resource = values[4].trim();
    sign_in.resource = (!resource.is_empty()).then(|| resource.to_owned());
    sign_in.callback_ports = split_list(if sign_in.provider == SignInProvider::Oidc {
        &values[5]
    } else {
        ""
    })
    .iter()
    .map(|port| match port.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(anyhow::anyhow!(
            "Callback ports must be numbers from 1 to 65535."
        )),
    })
    .collect::<anyhow::Result<_>>()?;
    if sign_in.provider == SignInProvider::TrinoExternal {
        sign_in.issuer.clear();
        sign_in.client_id.clear();
        sign_in.scopes.clear();
        sign_in.resource = None;
        sign_in.callback_ports.clear();
        sign_in.identity = None;
    }
    sign_in.validate()?;
    Ok(sign_in)
}

impl Qrow {
    /// The credentials of a connection: its password from Keychain, or the
    /// access tokens of its sign-in.
    pub(super) fn credential_provider(&self) -> crate::worker::CredentialProvider {
        let credentials = self.credentials.clone();
        let service = self.oidc.clone();
        let external = self.external_auth.clone();
        Arc::new(move |profile: &Profile| match profile.authentication {
            Authentication::Password => credentials
                .password(profile.id)
                .map(|password| password.into()),
            Authentication::Oidc { .. } => service.secret(profile),
            Authentication::TrinoExternal => external.secret(profile),
        })
    }

    /// The connections that use a sign-in.
    pub(super) fn connections_using(&self, id: Uuid) -> Vec<&Profile> {
        self.profiles
            .iter()
            .filter(|profile| profile.authentication.sign_in() == Some(id))
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
        self.tabs.iter().any(|tab| {
            tab.busy && tab.sign_in_wait.is_none() && self.uses_sign_in(tab.worker_profile, id)
        })
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
        self.oidc.configure(&self.sign_ins, &self.profiles);
        self.changed(cx);
    }

    fn start_external_sign_in(&mut self, profile: Profile, cx: &mut Context<Self>) {
        use crate::connector::Connector;
        if self.demo
            || self.sign_in_ui.external_pending.contains_key(&profile.id)
            || self.external_auth.is_authenticated(&profile)
        {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.sign_in_ui
            .external_pending
            .insert(profile.id, cancel.clone());
        let connection = profile.id;
        let service = self.external_auth.clone();
        let connector = self.connector.clone();
        let outcomes = self.sign_in_ui.outcomes.0.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let secret =
                    service
                        .secret(&profile)?
                        .with_control(crate::external_auth::Control::new(
                            Arc::new(move || cancel.load(Ordering::SeqCst)),
                            Arc::new(|_| {}),
                        ));
                let mut session = connector.connect(&profile, secret)?;
                session.close()
            })()
            .map_err(|error: anyhow::Error| {
                if error.is::<crate::external_auth::Cancelled>() {
                    String::new()
                } else {
                    crate::connector::error_message(&error)
                }
            });
            let _ = outcomes.send(Outcome::External(connection, result));
            let _ = wake.try_send(());
        });
        cx.notify();
    }
    /// Explicit connection selection authenticates without submitting editor SQL.
    pub(super) fn authenticate_connection(&mut self, profile: Profile, cx: &mut Context<Self>) {
        match profile.authentication {
            Authentication::TrinoExternal => self.start_external_sign_in(profile, cx),
            Authentication::Oidc { sign_in: id } if self.sign_in_needs_browser(id) => {
                self.sign_in_ui
                    .attempt_connections
                    .entry(id)
                    .or_default()
                    .insert(profile.id);
                self.start_sign_in(id, cx);
            }
            _ => {}
        }
    }

    pub(super) fn external_authentication_pending(&self, connection: Uuid) -> bool {
        self.sign_in_ui.external_pending.contains_key(&connection)
            || self.external_auth.status(connection) == crate::external_auth::Status::Waiting
    }

    pub(super) fn cancel_external_authentication(
        &mut self,
        connection: Uuid,
        cx: &mut Context<Self>,
    ) {
        if let Some(cancel) = self.sign_in_ui.external_pending.get(&connection) {
            cancel.store(true, Ordering::SeqCst);
        }
        cx.notify();
    }

    /// Failures retain their originating connection even when another tab is selected.
    pub(super) fn record_sign_in_error(
        &mut self,
        id: Uuid,
        connection: Uuid,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.sign_in_ui
            .set_connection_error(id, connection, message.clone());
        self.record_activity(
            connection,
            crate::activity::ActivityEntry::new(Severity::Error, message),
            cx,
        );
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
        let authorization_url = Arc::new(Mutex::new(None));
        self.sign_in_ui.pending.insert(
            id,
            PendingSignIn {
                cancel: cancel.clone(),
                url: authorization_url.clone(),
            },
        );
        self.sign_in_ui.clear_error(id);
        let service = self.oidc.clone();
        let outcomes = self.sign_in_ui.outcomes.0.clone();
        let urls = self.sign_in_ui.urls.0.clone();
        let browser = self.sign_in_ui.browser.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let open = |url: &str| -> anyhow::Result<()> {
                if cancel.load(Ordering::SeqCst) {
                    return Err(oidc::SignInError::new(
                        Failure::Cancelled,
                        "Sign-in was cancelled",
                    )
                    .into());
                }
                *authorization_url.lock().unwrap() = Some(url.to_owned());
                match &browser {
                    Some(browser) => browser(url),
                    None => {
                        urls.send((id, cancel.clone(), url.to_owned()))?;
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
        if let Some(pending) = self.sign_in_ui.pending.get(&id) {
            pending.cancel.store(true, Ordering::SeqCst);
        }
        cx.notify();
    }

    /// Opens the page of an existing attempt after a cancelled query is run again.
    fn reopen_sign_in(&self, id: Uuid, cx: &mut Context<Self>) {
        let Some(pending) = self.sign_in_ui.pending.get(&id) else {
            return;
        };
        if pending.cancel.load(Ordering::SeqCst) {
            return;
        }
        let Some(url) = pending.url.lock().unwrap().clone() else {
            return;
        };
        match &self.sign_in_ui.browser {
            Some(browser) => {
                let browser = browser.clone();
                let cancel = pending.cancel.clone();
                let outcomes = self.sign_in_ui.outcomes.0.clone();
                let wake = self.wake.clone();
                std::thread::spawn(move || {
                    if !cancel.load(Ordering::SeqCst) && browser(&url).is_err() {
                        let _ = outcomes.send(Outcome::BrowserFailed(id, cancel));
                        let _ = wake.try_send(());
                    }
                });
            }
            None => cx.open_url(&url),
        }
    }

    pub(super) fn sign_out(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.sign_in_busy(id)
            || self.sign_in_ui.pending.contains_key(&id)
            || !self.sign_in_ui.signing_out.insert(id)
        {
            return;
        }
        self.sign_in_ui.clear_error(id);
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

    /// Applies the results of background sign-in work, and notes a change of
    /// status from a refresh. Returns whether something changed.
    pub(super) fn tick_sign_ins(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let urls: Vec<BrowserPage> = self.sign_in_ui.urls.1.try_iter().collect();
        for (id, cancel, url) in urls {
            if self
                .sign_in_ui
                .pending
                .get(&id)
                .is_some_and(|pending| Arc::ptr_eq(&pending.cancel, &cancel))
                && !cancel.load(Ordering::SeqCst)
            {
                cx.open_url(&url);
            }
        }
        let outcomes: Vec<Outcome> = self.sign_in_ui.outcomes.1.try_iter().collect();
        for outcome in outcomes {
            changed = true;
            match outcome {
                Outcome::SignedIn(id, result) => {
                    self.sign_in_ui.pending.remove(&id);
                    for tab in &mut self.tabs {
                        if tab.cancelled_sign_in == Some(id) {
                            tab.cancelled_sign_in = None;
                        }
                    }
                    let ended = match &result {
                        Ok(_) => Ok(()),
                        Err((Failure::Cancelled, _)) => Err(None),
                        Err((_, message)) => Err(Some(message.clone())),
                    };
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
                            let connections = self
                                .sign_in_ui
                                .attempt_connections
                                .get(&id)
                                .cloned()
                                .unwrap_or_default();
                            for connection in connections {
                                self.record_sign_in_error(id, connection, message.clone(), cx);
                            }
                            self.sign_in_ui.errors.insert(id, message);
                        }
                    }
                    self.sign_in_ui.attempt_connections.remove(&id);
                    self.end_sign_in_waits(id, ended, cx);
                }
                Outcome::External(connection, result) => {
                    self.sign_in_ui.external_pending.remove(&connection);
                    if let Err(message) = result
                        && !message.is_empty()
                    {
                        self.record_activity(
                            connection,
                            crate::activity::ActivityEntry::new(Severity::Error, message),
                            cx,
                        );
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
                            self.sign_in_ui.clear_error(id);
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
                        self.sign_in_ui.clear_error(id);
                    }
                }
                Outcome::BrowserFailed(id, cancel) => {
                    if self
                        .sign_in_ui
                        .pending
                        .get(&id)
                        .is_some_and(|pending| Arc::ptr_eq(&pending.cancel, &cancel))
                        && !cancel.load(Ordering::SeqCst)
                    {
                        let message = "Could not open the sign-in page. Cancel the sign-in in the Sign-ins sidebar, then try again.";
                        self.message = Some(message.into());
                        for tab in &mut self.tabs {
                            if tab
                                .sign_in_wait
                                .as_ref()
                                .is_some_and(|wait| wait.sign_in == id)
                            {
                                Self::record_local_log(
                                    tab,
                                    Severity::Error,
                                    LogKind::SignIn,
                                    message,
                                );
                            }
                        }
                    }
                }
            }
        }
        let external_statuses = self
            .profiles
            .iter()
            .filter(|profile| profile.authentication == Authentication::TrinoExternal)
            .map(|profile| (profile.id, self.external_auth.status(profile.id)))
            .collect::<Vec<_>>();
        if external_statuses != self.sign_in_ui.external_statuses {
            self.sign_in_ui.external_statuses = external_statuses;
            changed = true;
        }
        // A query of another tab that blocked a browser sign-in can end.
        self.start_waiting_sign_ins(cx);
        let statuses: Vec<(Uuid, Status)> = self
            .sign_ins
            .iter()
            .map(|sign_in| (sign_in.id, self.oidc.status(sign_in.id)))
            .collect();
        if statuses != self.sign_in_ui.statuses {
            self.sign_in_ui.statuses = statuses;
            changed = true;
        }
        if let Some(editor) = &self.sign_in_ui.editor {
            let value = editor
                .id
                .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id))
                .and_then(|sign_in| sign_in.identity.as_ref())
                .map(|identity| identity.display())
                .unwrap_or_default();
            if editor.account.read(cx).value().as_ref() != value {
                editor
                    .account
                    .update(cx, |field, cx| field.set_value(value, window, cx));
            }
        }
        changed
    }

    /// The account of a sign-in, with the actions that apply to it.
    fn account(&self, id: Uuid) -> Account {
        use SignInAction::*;
        let failed = self.sign_in_ui.errors.contains_key(&id);
        if self.demo {
            return Account {
                summary: "Not available in the demo".into(),
                detail: "The demo does not sign in.".into(),
                actions: vec![],
                working: false,
                attention: false,
            };
        }
        if self.sign_in_ui.pending.contains_key(&id) {
            return Account {
                summary: "Waiting for the browser…".into(),
                detail: "Waiting for the browser. Finish the sign-in there.".into(),
                actions: vec![Cancel],
                working: true,
                attention: false,
            };
        }
        if self.sign_in_ui.signing_out.contains(&id) {
            return Account {
                summary: "Signing out…".into(),
                detail: "Signing out…".into(),
                actions: vec![],
                working: true,
                attention: false,
            };
        }
        match self.oidc.status(id) {
            Status::SignedOut => Account {
                summary: "Not signed in".into(),
                detail: "Not signed in.".into(),
                actions: vec![SignIn],
                working: false,
                attention: failed || !self.connections_using(id).is_empty(),
            },
            Status::SignedIn(identity) => Account {
                summary: identity.display().to_owned(),
                detail: format!("Signed in as {}.", identity.display()),
                actions: vec![SignOut],
                working: false,
                attention: failed,
            },
            Status::SignInRequired(identity, _) => Account {
                summary: "Expired · Sign in again".into(),
                detail: format!(
                    "The sign-in of {} has expired. Sign in again.",
                    identity.display()
                ),
                actions: vec![SignIn, SignOut],
                working: false,
                attention: true,
            },
            Status::NetworkFailure(identity, message) => Account {
                summary: "Cannot reach the provider".into(),
                detail: format!(
                    "Signed in as {}. Qrow could not reach the provider: {message}",
                    identity.display()
                ),
                actions: vec![Retry, SignOut],
                working: false,
                attention: true,
            },
        }
    }

    /// Whether a browser sign-in or a sign-out of the sign-in `id` runs,
    /// which can change or remove its account.
    pub(super) fn sign_in_changing(&self, id: Uuid) -> bool {
        self.account(id).working
    }

    /// Whether the sign-in `id` needs the browser before a new session can
    /// open: it is not signed in, it has expired, or a browser sign-in runs.
    pub(super) fn sign_in_needs_browser(&self, id: Uuid) -> bool {
        !self.demo
            && (self.sign_in_ui.pending.contains_key(&id)
                || matches!(
                    self.oidc.status(id),
                    Status::SignedOut | Status::SignInRequired(..)
                ))
    }

    /// Holds the query `sql` of the tab `index` until the browser sign-in
    /// `sign_in` ends, and starts that sign-in if it does not run.
    pub(super) fn wait_for_sign_in(
        &mut self,
        index: usize,
        sign_in: Uuid,
        sql: String,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .sign_ins
            .iter()
            .find(|candidate| candidate.id == sign_in)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_default();
        if let Some(connection) = self.tabs[index].saved.profile {
            self.sign_in_ui
                .attempt_connections
                .entry(sign_in)
                .or_default()
                .insert(connection);
        }
        let tab = &mut self.tabs[index];
        let reopen = tab.cancelled_sign_in.take() == Some(sign_in);
        tab.sign_in_wait = Some(SignInWait { sign_in, sql });
        tab.busy = true;
        tab.cancelling = false;
        tab.more = false;
        tab.pending_page = None;
        tab.elapsed = None;
        tab.started = Some(Instant::now());
        Self::record_local_log(
            tab,
            Severity::Info,
            LogKind::SignIn,
            format!("Sign in to \"{name}\" in the browser. The query runs after the sign-in."),
        );
        if reopen {
            self.reopen_sign_in(sign_in, cx);
        }
        self.start_waiting_sign_ins(cx);
    }

    /// Starts the browser sign-ins that waiting queries need, and shows in
    /// each waiting tab whether its browser sign-in runs. A sign-in waits
    /// while a query of another tab uses its account, because a sign-in as
    /// another account releases sessions.
    pub(super) fn start_waiting_sign_ins(&mut self, cx: &mut Context<Self>) {
        let mut needed: Vec<Uuid> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.sign_in_wait.as_ref().map(|wait| wait.sign_in))
            .collect();
        needed.extend(self.sign_in_ui.attempt_connections.keys().copied());
        needed.sort();
        needed.dedup();
        for id in needed {
            if !self.sign_in_ui.pending.contains_key(&id) {
                self.start_sign_in(id, cx);
            }
            let detail = if self.sign_in_ui.pending.contains_key(&id) {
                "Finish it in the browser"
            } else if self.sign_in_ui.signing_out.contains(&id) {
                "The sign-in is signing out"
            } else {
                "Another query uses this sign-in"
            };
            for tab in &mut self.tabs {
                if tab
                    .sign_in_wait
                    .as_ref()
                    .is_some_and(|wait| wait.sign_in == id)
                    && tab.status_detail.as_deref() != Some(detail)
                {
                    tab.set_status_detail("Waiting for sign-in", detail);
                    cx.notify();
                }
            }
        }
    }

    /// Runs the queries that waited for the sign-in `id`, or ends them when
    /// the sign-in failed (`Some` message) or was cancelled (`None`).
    fn end_sign_in_waits(
        &mut self,
        id: Uuid,
        ended: Result<(), Option<String>>,
        cx: &mut Context<Self>,
    ) {
        let waiting: Vec<usize> = (0..self.tabs.len())
            .filter(|&index| {
                self.tabs[index]
                    .sign_in_wait
                    .as_ref()
                    .is_some_and(|wait| wait.sign_in == id)
            })
            .collect();
        for index in waiting {
            let active = index == self.active && !self.activity.read(cx).is_open();
            let tab = &mut self.tabs[index];
            let Some(wait) = tab.sign_in_wait.take() else {
                continue;
            };
            tab.busy = false;
            match &ended {
                Ok(()) => {
                    self.run_tab_sql(index, wait.sql, true, cx);
                }
                Err(message) => {
                    tab.started = None;
                    let (text, status, detail) = match message {
                        Some(message) => (
                            format!("The sign-in failed, so the query did not run: {message}"),
                            "Error",
                            "Sign-in required",
                        ),
                        None => (
                            "The sign-in was cancelled, so the query did not run.".to_owned(),
                            "Cancelled",
                            "Sign-in not finished",
                        ),
                    };
                    Self::record_local_log(tab, Severity::Error, LogKind::SignIn, text);
                    Self::record_failure(tab, active);
                    tab.set_status_detail(status, detail);
                }
            }
        }
        cx.notify();
    }

    /// Cancels the query of the tab `index`, which waits for a sign-in. The
    /// browser sign-in continues, and the Sign-ins sidebar can cancel it.
    pub(super) fn cancel_sign_in_wait(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
        let Some(wait) = tab.sign_in_wait.take() else {
            return;
        };
        tab.cancelled_sign_in = Some(wait.sign_in);
        tab.busy = false;
        tab.started = None;
        tab.set_status_detail("Cancelled", "Sign-in not finished");
        Self::record_local_log(
            tab,
            Severity::Info,
            LogKind::SignIn,
            "Cancelled the query before the sign-in finished. The query did not run.",
        );
        cx.notify();
    }

    /// The number of sign-ins that need the user, for the status bar.
    pub(super) fn sign_ins_needing_attention(&self) -> usize {
        self.sign_ins
            .iter()
            .filter(|sign_in| self.account(sign_in.id).attention)
            .count()
    }

    /// Whether a browser sign-in or a sign-out runs.
    pub(super) fn sign_ins_working(&self) -> bool {
        !self.sign_in_ui.pending.is_empty() || !self.sign_in_ui.signing_out.is_empty()
    }

    /// Runs `action` on the sign-in `id`.
    fn run_sign_in_action(&mut self, id: Uuid, action: SignInAction, cx: &mut Context<Self>) {
        match action {
            SignInAction::SignIn => self.start_sign_in(id, cx),
            SignInAction::Cancel => self.cancel_sign_in(id, cx),
            SignInAction::SignOut => self.sign_out(id, cx),
            SignInAction::Retry => self.retry_sign_in(id, cx),
        }
    }

    /// Whether `action` must wait. Signing out or in as another account
    /// releases sessions, so it waits until their work ends.
    fn sign_in_action_blocked(&self, id: Uuid, action: SignInAction) -> bool {
        match action {
            SignInAction::SignIn | SignInAction::SignOut => self.sign_in_busy(id),
            SignInAction::Retry => self.sign_in_ui.retrying.contains(&id),
            SignInAction::Cancel => false,
        }
    }

    /// The Sign-ins sidebar: its header and one row for each sign-in.
    pub(super) fn sign_ins_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let group_action_size = self.ui_px(24.);
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                self.sidebar_header("Sign-Ins", cx)
                    // The header actions are compact and touch, so they read
                    // as one group.
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .child(
                                Button::new("paste-sign-in")
                                    .ghost()
                                    .small()
                                    .w(group_action_size)
                                    .h(group_action_size)
                                    .icon(AssetIconName::ClipboardPaste)
                                    .disabled(self.demo)
                                    .accessibility_label("Paste sign-in")
                                    .tooltip("Paste sign-in…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.paste_sign_in(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("add-sign-in")
                                    .ghost()
                                    .small()
                                    .w(group_action_size)
                                    .h(group_action_size)
                                    .icon(IconName::Plus)
                                    .disabled(self.demo)
                                    .accessibility_label("New sign-in")
                                    .tooltip("New sign-in…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_sign_in_editor(None, false, window, cx)
                                    })),
                            ),
                    ),
            )
            .when(!self.sign_ins.is_empty(), |el| {
                el.child(
                    v_flex()
                        .id("sign-ins-list")
                        .test_support()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_2()
                        .gap_0p5()
                        .children(
                            self.sign_ins
                                .iter()
                                .map(|sign_in| self.sign_in_row(sign_in, cx)),
                        ),
                )
            })
    }

    /// The row of a sign-in: its name and provider. Click opens Sign-in
    /// Settings; right-click opens the menu of the sign-in.
    fn sign_in_row(&self, sign_in: &SignIn, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let id = sign_in.id;
        let account = self.account(id);
        let error = self.sign_in_ui.errors.get(&id).cloned();
        let slot_width = self.ui_px(28.);
        let mut tooltip = sign_in.issuer.clone();
        if let Some(error) = &error {
            tooltip.push('\n');
            tooltip.push_str(error);
        }
        let summary = match sign_in.provider {
            SignInProvider::Oidc => "OIDC",
            SignInProvider::TrinoExternal => "Trino",
        };
        let status = if error.is_some() {
            "The last action failed"
        } else {
            &account.summary
        };
        let accessibility_label = format!("{}, {summary}, {status}", sign_in.name);
        let menu_open = self.menu.is_some();
        let primary = account
            .actions
            .iter()
            .copied()
            .find(|action| *action == SignInAction::Cancel);
        let button = Button::new(SharedString::from(format!("sign-in-{id}")))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(transparent_black())
                    .hover(transparent_black())
                    .active(transparent_black())
                    .foreground(cx.theme().sidebar_foreground)
                    .shadow(false),
            )
            .small()
            .h_full()
            .flex_1()
            .min_w_0()
            .px_2()
            .accessibility_label(accessibility_label)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(
                        Icon::new(AssetIconName::KeyRound)
                            .size_4()
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_base()
                                    .line_height(relative(1.25))
                                    .child(sign_in.name.clone()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("sign-in-{id}-account")))
                                    .test_support()
                                    .role(Role::Label)
                                    .aria_label(summary)
                                    .w_full()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(summary),
                            ),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_sign_in_editor(Some(id), false, window, cx)
            }));
        let slot = div()
            .flex_shrink_0()
            .min_w(slot_width)
            .flex()
            .items_center()
            .justify_center()
            .pr_1();
        let trailing = match primary {
            Some(action) => slot.child(
                Button::new(SharedString::from(format!(
                    "sign-in-{id}-{}",
                    action.slug()
                )))
                .xsmall()
                .label(action.button_label())
                .disabled(self.sign_in_action_blocked(id, action))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.run_sign_in_action(id, action, cx)),
                ),
            ),
            None if account.working => {
                slot.child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
            }
            None if account.attention => slot.child(
                Button::new(SharedString::from(format!("sign-in-status-{id}")))
                    .ghost()
                    .small()
                    .w(slot_width)
                    .h_full()
                    .accessibility_label(status.to_owned())
                    .tooltip(if error.is_some() {
                        "Show sign-in error"
                    } else {
                        "Show sign-in status"
                    })
                    .child(
                        if error.is_some() {
                            DotStatus::Error
                        } else {
                            DotStatus::Attention
                        }
                        .dot(cx),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(connection) =
                            this.sign_in_ui.error_connections.get(&id).copied()
                        {
                            this.open_activity(Some(connection), window, cx);
                        } else {
                            this.open_sign_in_editor(Some(id), false, window, cx);
                        }
                    })),
            ),
            None => slot,
        };
        h_flex()
            .id(SharedString::from(format!("sign-in-row-{id}")))
            .when(!menu_open, |el| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
            })
            .w_full()
            .h(self.ui_px(44.))
            .rounded(cx.theme().radius)
            .hover(|el| el.bg(cx.theme().tokens.list_hover))
            .child(button)
            .child(trailing)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, event: &MouseDownEvent, window, cx| {
                    let position = event.position;
                    cx.defer_in(window, move |this, window, cx| {
                        this.open_sign_in_menu(id, position, window, cx)
                    });
                }),
            )
    }

    /// The menu of a sign-in row: the account actions, Edit, Copy Settings,
    /// and Delete.
    fn open_sign_in_menu(
        &mut self,
        id: Uuid,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sign_ins.iter().any(|sign_in| sign_in.id == id) {
            return;
        }
        let account = self.account(id);
        let in_use = !self.connections_using(id).is_empty();
        let actions: Vec<_> = account
            .actions
            .iter()
            .map(|&action| {
                (
                    action.menu_label(),
                    self.sign_in_action_blocked(id, action),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.run_sign_in_action(id, action, cx)
                    }),
                )
            })
            .collect();
        let edit = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_sign_in_editor(Some(id), false, window, cx)
        });
        let copy =
            cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_sign_in_settings(id, cx));
        let delete = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.confirm_delete_sign_in(id, window, cx)
        });
        let working = account.working;
        self.open_context_menu(
            position,
            move |menu, _, _| {
                let mut menu = menu.item(menu_section("Sign-In"));
                for (label, blocked, listener) in actions {
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .on_click(listener)
                            .disabled(blocked),
                    );
                }
                menu.item(PopupMenuItem::new("Edit").on_click(edit).disabled(working))
                    .item(PopupMenuItem::new("Copy settings").on_click(copy))
                    .item(
                        PopupMenuItem::new("Delete")
                            .on_click(delete)
                            .when(in_use, |item| {
                                item.tooltip("Change authentication in the connections first.")
                            })
                            .disabled(working || in_use),
                    )
            },
            window,
            cx,
        );
    }

    /// Puts the settings of a sign-in on the clipboard, without its account
    /// or tokens, so that another user can paste them.
    fn copy_sign_in_settings(&self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(sign_in) = self.sign_ins.iter().find(|sign_in| sign_in.id == id) {
            cx.write_to_clipboard(ClipboardItem::new_string(sign_in.to_shared_text()));
        }
    }

    /// Opens Sign-in Settings with the sign-in settings on the clipboard, so
    /// that the user can examine them before Save adds the sign-in.
    pub(super) fn paste_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.demo || self.dialog_open() || window.has_active_dialog(cx) {
            return;
        }
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let pasted = SignIn::from_shared_text(&text).and_then(|sign_in| {
            anyhow::ensure!(
                sign_in.provider == SignInProvider::Oidc,
                "Choose External in the Trino connection's Authentication field."
            );
            Ok(sign_in)
        });
        match pasted {
            Ok(mut sign_in) => {
                sign_in.name = unique_sign_in_name(&sign_in.name, |name| {
                    self.sign_ins.iter().any(|other| other.name == name)
                });
                self.show_sign_in_editor(sign_in, None, false, true, window, cx);
            }
            Err(error) => {
                let message = error.to_string();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    alert
                        .width(px(360.))
                        .title("Cannot paste the sign-in")
                        .description(
                            div()
                                .id("paste-sign-in-error")
                                .test_support()
                                .role(Role::Label)
                                .aria_label(message.clone())
                                .child(message.clone()),
                        )
                        .footer(
                            DialogFooter::new().justify_end().child(
                                Button::new("close-paste-sign-in-error")
                                    .primary()
                                    .label("OK")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                        )
                });
                cx.notify();
            }
        }
    }

    /// Asks before Delete removes a sign-in and its tokens.
    fn confirm_delete_sign_in(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connections_using(id).is_empty() || self.account(id).working {
            return;
        }
        let Some(sign_in) = self.sign_ins.iter().find(|sign_in| sign_in.id == id) else {
            return;
        };
        let display_name = truncate_display_name(&sign_in.name);
        let description = if sign_in.provider == SignInProvider::TrinoExternal {
            "This signs out, deletes the sign-in, and clears its tokens from memory. This cannot be undone."
        } else {
            "This signs out, deletes the sign-in, and removes its tokens from Keychain. This cannot be undone."
        };
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            alert
                .width(px(360.))
                .title(format!("Delete sign-in \"{display_name}\"?"))
                .description(description)
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-delete-sign-in")
                                .label("Cancel")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("confirm-delete-sign-in")
                                .label("Delete sign-in")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, window, cx| {
                                    let _ =
                                        confirm.update(cx, |this, cx| this.remove_sign_in(id, cx));
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
        cx.notify();
    }

    /// Renders Connection Settings again when its Sign-in list changes.
    pub(super) fn subscribe_sign_in_list(
        list: &connection_form::RowCombobox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            list,
            window,
            |_, _, _: &ComboboxEvent<SearchableVec<connection_form::Row>>, _, cx| cx.notify(),
        )
    }

    /// Gives Connection Settings a new, closed Sign-in list with the current
    /// sign-ins and `selected` chosen.
    pub(super) fn replace_sign_in_list(
        &mut self,
        selected: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let database_type = self
            .form
            .as_ref()
            .map(|form| {
                connection_form::chosen(
                    &form.database_type,
                    &connection_form::database_type_choices(),
                    cx,
                )
            })
            .unwrap_or_default();
        let choices = connection_form::sign_in_choices(&self.sign_ins, database_type);
        let list =
            connection_form::sign_in_combobox(&choices, &self.sign_ins, selected, window, cx);
        let subscription = Self::subscribe_sign_in_list(&list, window, cx);
        if let Some(form) = &mut self.form {
            form.sign_in_choices = choices;
            form.sign_in = list;
            form._sign_in_subscription = subscription;
        }
    }

    /// Opens Sign-in Settings for the sign-in `id`, or for a new sign-in.
    /// `for_connection` opens it on top of Connection Settings.
    pub(super) fn open_sign_in_editor(
        &mut self,
        id: Option<Uuid>,
        for_connection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blocked = if for_connection {
            self.form.is_none() || self.sign_in_ui.editor.is_some()
        } else {
            self.dialog_open() || window.has_active_dialog(cx)
        };
        if blocked || self.demo || id.is_some_and(|id| self.account(id).working) {
            return;
        }
        let base = id
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id))
            .cloned()
            .unwrap_or_default();
        self.show_sign_in_editor(base, id, for_connection, false, window, cx);
    }

    /// Opens Sign-in Settings with the fields of `base`. `id` is the sign-in
    /// to change, or `None` to add one.
    fn show_sign_in_editor(
        &mut self,
        base: SignIn,
        id: Option<Uuid>,
        for_connection: bool,
        pasted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let values = [
            base.name.clone(),
            base.issuer.clone(),
            base.client_id.clone(),
            extra_scopes(&base.scopes).join(" "),
            base.resource.clone().unwrap_or_default(),
            base.callback_ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ];
        let fields: Vec<_> = values
            .into_iter()
            .zip(EDITOR_FIELDS)
            .map(|(value, (_, _, _, placeholder))| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            })
            .collect();
        let first = fields[0].clone();
        let account = cx.new(|cx| {
            InputState::new(window, cx).default_value(
                base.identity
                    .as_ref()
                    .map(|identity| identity.display())
                    .unwrap_or_default(),
            )
        });
        // The New Sign-in… button is in the open Sign-in list. A new list is
        // closed, so it does not take the keys of the dialog.
        if let Some(form) = self.form.as_ref().filter(|_| for_connection) {
            let chosen = connection_form::chosen_sign_in(&form.sign_in, &form.sign_in_choices, cx);
            self.replace_sign_in_list(chosen, window, cx);
        }
        let choices: Vec<_> = provider_choices()
            .into_iter()
            .filter(|(provider, _)| {
                *provider == SignInProvider::Oidc
                    || !for_connection
                    || self.form.as_ref().is_some_and(|form| {
                        form.profile.database_type == crate::model::DatabaseType::Trino
                    })
            })
            .collect();
        let provider = connection_form::choice_select(&choices, &base.provider, window, cx);
        let provider_subscription = cx.subscribe_in(
            &provider,
            window,
            |_,
             _,
             _: &gpui_kit::component::select::SelectEvent<
                gpui_kit::component::select::SearchableVec<connection_form::Row>,
            >,
             _,
             cx| cx.notify(),
        );
        self.sign_in_ui.editor = Some(SignInEditor {
            id: id.filter(|id| self.sign_ins.iter().any(|sign_in| sign_in.id == *id)),
            for_connection,
            fields,
            account,
            provider,
            _provider_subscription: provider_subscription,
            pasted,
            error: None,
        });
        self.open_sign_in_dialog(window, cx);
        first.update(cx, |field, cx| field.focus(window, cx));
        cx.notify();
    }

    fn open_sign_in_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let save = weak.clone();
            let content = weak.update(cx, |this, cx| this.sign_in_content(cx)).ok();
            let footer = weak.update(cx, |this, cx| this.sign_in_footer(cx)).ok();
            let rem = window.rem_size();
            let viewport = window.viewport_size();
            let height = (rem * 44.).min(viewport.height - rem * 4.);
            dialog
                .title("Sign-In Settings")
                .w(profile_view::dialog_width(window))
                .h(height)
                .margin_top((viewport.height - height) / 2.)
                .overlay_closable(false)
                // Enter saves. A successful save closes the dialog.
                .on_ok(move |_, window, cx| {
                    let _ = save.update(cx, |this, cx| this.save_sign_in_editor(window, cx));
                    false
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.sign_in_ui.editor = None;
                        cx.notify();
                    });
                })
        });
    }

    fn close_sign_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sign_in_ui.editor = None;
        window.close_dialog(cx);
        cx.notify();
    }

    fn save_sign_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.sign_in_ui.editor else {
            return;
        };
        let values: Vec<String> = editor
            .fields
            .iter()
            .map(|field| field.read(cx).value().to_string())
            .collect();
        let for_connection = editor.for_connection;
        let mut base = editor
            .id
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id))
            .cloned()
            .unwrap_or_default();
        let provider = connection_form::chosen(&editor.provider, &provider_choices(), cx);
        if base.provider != provider && !self.connections_using(base.id).is_empty() {
            if let Some(editor) = &mut self.sign_in_ui.editor {
                editor.error = Some(
                    "Change authentication in its connections before changing the provider.".into(),
                );
            }
            cx.notify();
            return;
        }
        base.provider = provider;
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
        let sign_in = match result {
            Ok(sign_in) => sign_in,
            Err(error) => {
                if let Some(editor) = &mut self.sign_in_ui.editor {
                    editor.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };
        let id = sign_in.id;
        match self
            .sign_ins
            .iter_mut()
            .find(|other| other.id == sign_in.id)
        {
            Some(existing) => *existing = sign_in,
            None => self.sign_ins.push(sign_in),
        }
        self.sync_sign_ins(cx);
        // A sign-in that Connection Settings added becomes its choice.
        if for_connection && self.form.is_some() {
            self.replace_sign_in_list(Some(id), window, cx);
        }
        self.close_sign_in_editor(window, cx);
    }

    /// The Account field of Sign-in Settings: the status, the last error, and
    /// the account actions.
    fn account_field(
        &self,
        id: Uuid,
        signed_in: bool,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> Field {
        let account = self.account(id);
        let error = self.sign_in_ui.errors.get(&id).cloned();
        let buttons = account.actions.iter().map(|&action| {
            let blocked = self.sign_in_action_blocked(id, action);
            Button::new(SharedString::from(format!(
                "sign-in-account-{}",
                action.slug()
            )))
            .label(action.button_label())
            .disabled(blocked)
            .when(
                blocked && matches!(action, SignInAction::SignIn | SignInAction::SignOut),
                |button| button.tooltip("Wait for queries that use this sign-in to finish."),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.run_sign_in_action(id, action, cx)))
        });
        let field = Field::new().label("Account").child(
            v_flex()
                .w_full()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .child(if signed_in {
                            Input::new(input)
                                .id("sign-in-account")
                                .aria_label("Account")
                                .focus_ring(false)
                                .readonly(true)
                                .flex_1()
                                .min_w_0()
                                .into_any_element()
                        } else {
                            div()
                                .id("sign-in-account-status")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .role(Role::Status)
                                .aria_label(account.detail.clone())
                                .child(account.detail.clone())
                                .into_any_element()
                        })
                        .when(!account.actions.is_empty(), |el| {
                            el.child(h_flex().gap_2().flex_shrink_0().children(buttons))
                        }),
                )
                .when(
                    signed_in
                        && (account.working
                            || matches!(
                                self.oidc.status(id),
                                Status::SignInRequired(..) | Status::NetworkFailure(..)
                            )),
                    |el| {
                        el.child(
                            div()
                                .id("sign-in-account-status")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(account.detail.clone())
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(account.detail),
                        )
                    },
                )
                .when_some(error, |el, error| {
                    el.child(
                        div()
                            .id("sign-in-account-error")
                            .test_support()
                            .role(Role::Alert)
                            .aria_label(error.clone())
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                }),
        );
        if signed_in {
            field.description("Sign out to change authentication settings.")
        } else {
            field
        }
    }

    fn sign_in_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = &self.sign_in_ui.editor else {
            return div().into_any_element();
        };
        let existing = editor
            .id
            .and_then(|id| self.sign_ins.iter().find(|sign_in| sign_in.id == id));
        let signed_in = existing.is_some_and(|sign_in| sign_in.identity.is_some());
        let pasted = editor.pasted;
        let account =
            existing.map(|sign_in| self.account_field(sign_in.id, signed_in, &editor.account, cx));
        let users = existing
            .map(|sign_in| self.connections_using(sign_in.id))
            .unwrap_or_default();
        let fields = EDITOR_FIELDS.into_iter().enumerate().map(|(index, field)| {
            let (id, label, description, _) = field;
            Field::new().label(label).description(description).child(
                Input::new(&editor.fields[index])
                    .focus_ring(false)
                    .id(id)
                    .w_full()
                    .disabled(signed_in && TOKEN_FIELDS.contains(&index))
                    .aria_label(label),
            )
        });
        let count = users.len();
        let provider_locked = signed_in || count > 0;
        let connections = Field::new()
            .label_fn(move |_, cx| {
                h_flex().gap_1().child("Connections").child(
                    div()
                        .id("sign-in-connections-count")
                        .test_support()
                        .role(Role::Label)
                        .aria_label(format!(
                            "{count} {}",
                            if count == 1 {
                                "connection"
                            } else {
                                "connections"
                            }
                        ))
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("({count})")),
                )
            })
            .child(
                h_flex()
                    .id("sign-in-connections")
                    .test_support()
                    .w_full()
                    .flex_wrap()
                    .items_start()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .children(users.into_iter().map(|profile| {
                        div()
                            .id(SharedString::from(format!(
                                "sign-in-connection-{}",
                                profile.id
                            )))
                            .test_support()
                            .flex_shrink_0()
                            .max_w_full()
                            .role(Role::Label)
                            .aria_label(profile.name.clone())
                            .child(
                                Tag::color(ColorName::Neutral)
                                    .outline()
                                    .border_color(cx.theme().border)
                                    .max_w_full()
                                    .child(profile.name.clone()),
                            )
                    }))
                    .when(count == 0, |el| {
                        let message = "No connections use this sign-in.";
                        el.child(
                            div()
                                .id("sign-in-connections-empty")
                                .test_support()
                                .role(Role::Label)
                                .aria_label(message)
                                .text_color(cx.theme().muted_foreground)
                                .child(message),
                        )
                    }),
            );
        v_flex()
            .key_context("SignInSettings")
            .on_action(
                cx.listener(|this, _: &SaveSignIn, window, cx| {
                    this.save_sign_in_editor(window, cx)
                }),
            )
            .w_full()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("Authentication"),
            )
            .when(pasted, |el| {
                let note = "Pasted from the clipboard. Make sure that you trust the issuer before you save.";
                el.child(
                    div()
                        .id("sign-in-pasted-note")
                        .test_support()
                        .role(Role::Label)
                        .aria_label(note)
                        .pt_1()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
            })
            .child(
                v_flex().pt_3().w_full().gap_2().child(
                    Form::vertical()
                        .w_full()
                        .child(Field::new().label("Provider").description("Choose the authentication protocol of the database.").child(
                            gpui_kit::component::select::Select::new(&editor.provider).id("sign-in-provider").w_full().disabled(provider_locked)
                        ))
                        .when_some(account, |form, account| form.child(account))
                        .children(fields)
                        .when(existing.is_some(), |form| form.child(connections)),
                ),
            )
            .into_any_element()
    }

    fn sign_in_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let error = self
            .sign_in_ui
            .editor
            .as_ref()
            .and_then(|editor| editor.error.clone());
        v_flex()
            .w_full()
            .gap_2()
            .key_context("SignInSettings")
            .on_action(
                cx.listener(|this, _: &SaveSignIn, window, cx| {
                    this.save_sign_in_editor(window, cx)
                }),
            )
            .when_some(error, |el, error| {
                el.child(
                    div()
                        .id("sign-in-form-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .child(Alert::error("sign-in-form-error-alert", error)),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .pt_3()
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-sign-in-editor")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_sign_in_editor(window, cx)
                            })),
                    )
                    .child(
                        Button::new("save-sign-in-editor")
                            .primary()
                            .label("Save")
                            .tooltip("Save sign-in · ⌘Enter")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_sign_in_editor(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// Stops the worker of a tab. Its SQL, results, and Logs history stay.
pub(super) fn release_session(tab: &mut Tab) {
    if tab.busy {
        // The work of the tab can end later. Its next query releases the
        // session, so it never runs as the old account.
        tab.release_pending = true;
        return;
    }
    tab.release_pending = false;
    if let Some(worker) = tab.worker.take() {
        worker.shutdown();
    }
    tab.worker_profile = None;
    tab.connected = false;
    tab.cancelling = false;
    tab.more = false;
    tab.pending_page = None;
    tab.set_status("Not connected");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignInAction {
    SignIn,
    Cancel,
    SignOut,
    Retry,
}

impl SignInAction {
    /// The label of a button.
    fn button_label(self) -> &'static str {
        match self {
            Self::SignIn => "Sign in…",
            Self::Cancel => "Cancel",
            Self::SignOut => "Sign out",
            Self::Retry => "Retry",
        }
    }

    /// The label of a menu item, which names its object.
    fn menu_label(self) -> &'static str {
        match self {
            Self::SignIn => "Sign in…",
            Self::Cancel => "Cancel sign-in",
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
            "",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn the_editor_reads_lists_and_optional_fields() {
        let sign_in = parse_editor(&values(), &SignIn::default()).unwrap();
        assert_eq!(sign_in.issuer, "https://id.example.test/realms/data");
        assert_eq!(sign_in.scopes, vec!["kyuubi"]);
        assert_eq!(sign_in.resource, None);
        assert!(sign_in.callback_ports.is_empty());
        let mut values = values();
        values[5] = "8765, 8766 8765".into();
        values[4] = "https://kyuubi.example.test".into();
        let sign_in = parse_editor(&values, &SignIn::default()).unwrap();
        assert_eq!(sign_in.callback_ports, vec![8765, 8766]);
        assert_eq!(
            sign_in.resource.as_deref(),
            Some("https://kyuubi.example.test")
        );
        for ports in ["port", "8765 0", "70000"] {
            values[5] = ports.into();
            assert!(
                parse_editor(&values, &SignIn::default()).is_err(),
                "{ports}"
            );
        }
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

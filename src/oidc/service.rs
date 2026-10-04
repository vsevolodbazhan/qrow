//! One credential service for all tabs of a process. It keeps the sign-in
//! configurations, gives access tokens to connections, refreshes them one at
//! a time for each sign-in, and saves replacement tokens in Keychain before
//! it uses them.
use super::{
    Failure, SignInError,
    flow::{self, Attempt, Discovery, SIGN_IN_TIMEOUT},
    jwt::{self, Expected, Jwks},
};
use crate::{
    connector::{Secret, TokenSource},
    model::{Authentication, Identity, Profile, SignIn},
    storage::TokenStore,
    tls::Trust,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Qrow refreshes an access token that expires within this time, so that the
/// token stays valid while a transport opens.
pub const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The state of a sign-in, for the interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    SignedOut,
    SignedIn(Identity),
    /// The tokens cannot be used or refreshed. A browser sign-in can help.
    SignInRequired(Identity, String),
    /// The last refresh could not reach the provider.
    NetworkFailure(Identity, String),
}

type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

pub struct Service {
    store: Arc<dyn TokenStore>,
    trust: Trust,
    clock: Clock,
    records: Mutex<HashMap<Uuid, Arc<Record>>>,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct Record {
    config: Mutex<SignIn>,
    /// Sign-out, removal, and requirement changes increment it. Work that
    /// started before discards its result.
    generation: AtomicU64,
    /// Held during token work, so that one refresh runs at a time.
    work: Mutex<Work>,
    problem: Mutex<Option<(Failure, String)>>,
}

#[derive(Default)]
struct Work {
    loaded: bool,
    tokens: Option<Stored>,
    discovery: Option<Discovery>,
    keys: Option<Jwks>,
}

/// The Keychain record of a sign-in.
#[derive(Deserialize, Serialize)]
struct Stored {
    version: u32,
    issuer: String,
    client_id: String,
    subject: String,
    scopes: Vec<String>,
    resource: Option<String>,
    access_token: String,
    /// Seconds since the Unix epoch.
    expires_at: u64,
    refresh_token: Option<String>,
}

impl Drop for Stored {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

impl Stored {
    fn matches(&self, config: &SignIn, subject: &str) -> bool {
        self.version == 1
            && self.issuer == config.issuer
            && self.client_id == config.client_id
            && crate::model::extra_scopes(&self.scopes)
                == crate::model::extra_scopes(&config.scopes)
            && self.resource == config.resource
            && self.subject == subject
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn required(message: impl Into<String>) -> anyhow::Error {
    SignInError::new(Failure::SignInRequired, message).into()
}

impl Service {
    pub fn new(store: Arc<dyn TokenStore>, trust: Trust) -> Arc<Self> {
        Self::with_clock(store, trust, Arc::new(SystemTime::now))
    }

    /// A service with a test clock.
    pub fn with_clock(store: Arc<dyn TokenStore>, trust: Trust, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            store,
            trust,
            clock,
            records: Mutex::default(),
            on_change: Mutex::default(),
        })
    }

    pub fn trust(&self) -> &Trust {
        &self.trust
    }

    /// Calls `notify` after each change of a status.
    pub fn set_on_change(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.on_change) = Some(notify);
    }

    fn changed(&self) {
        let notify = lock(&self.on_change).clone();
        if let Some(notify) = notify {
            notify();
        }
    }

    /// Applies the configurations of the workspace. The service owns the
    /// identity of a known sign-in, so a configuration does not replace it.
    /// A change of the token requirements discards the results of running
    /// work. Stored tokens for other requirements do not match any more.
    ///
    /// Lock order: `work`, then `config`. This method does not wait for
    /// `work`, which a refresh holds during network requests, because the
    /// interface thread calls it.
    pub fn configure(&self, sign_ins: &[SignIn]) {
        let mut records = lock(&self.records);
        records.retain(|id, record| {
            let keep = sign_ins.iter().any(|sign_in| sign_in.id == *id);
            if !keep {
                record.generation.fetch_add(1, Ordering::SeqCst);
            }
            keep
        });
        for sign_in in sign_ins {
            match records.get(&sign_in.id) {
                Some(record) => {
                    let mut config = lock(&record.config);
                    let identity = config.identity.take();
                    if !config.token_requirements_eq(sign_in) {
                        record.generation.fetch_add(1, Ordering::SeqCst);
                    }
                    *config = SignIn {
                        identity,
                        ..sign_in.clone()
                    };
                }
                None => {
                    records.insert(
                        sign_in.id,
                        Arc::new(Record {
                            config: Mutex::new(sign_in.clone()),
                            generation: AtomicU64::new(0),
                            work: Mutex::default(),
                            problem: Mutex::default(),
                        }),
                    );
                }
            }
        }
    }

    fn record(&self, id: Uuid) -> Result<Arc<Record>> {
        lock(&self.records)
            .get(&id)
            .cloned()
            .ok_or_else(|| required("The sign-in of this connection no longer exists"))
    }

    pub fn status(&self, id: Uuid) -> Status {
        let Ok(record) = self.record(id) else {
            return Status::SignedOut;
        };
        let Some(identity) = lock(&record.config).identity.clone() else {
            return Status::SignedOut;
        };
        match lock(&record.problem).clone() {
            Some((Failure::SignInRequired, message)) => Status::SignInRequired(identity, message),
            Some((Failure::Network, message)) => Status::NetworkFailure(identity, message),
            _ => Status::SignedIn(identity),
        }
    }

    pub fn identity(&self, id: Uuid) -> Option<Identity> {
        self.record(id)
            .ok()
            .and_then(|record| lock(&record.config).identity.clone())
    }

    fn set_problem(&self, record: &Record, problem: Option<(Failure, String)>) {
        let mut current = lock(&record.problem);
        if *current != problem {
            *current = problem;
            drop(current);
            self.changed();
        }
    }

    /// Runs a browser sign-in. `open` shows the authorization URL. The call
    /// returns after the callback, a failure, `cancel`, or the timeout. A
    /// result that arrives after a sign-out or removal is discarded.
    pub fn sign_in(
        &self,
        id: Uuid,
        cancel: &AtomicBool,
        open: &dyn Fn(&str) -> Result<()>,
    ) -> Result<Identity> {
        let record = self.record(id)?;
        let config = lock(&record.config).clone();
        config.validate()?;
        let generation = record.generation.load(Ordering::SeqCst);
        let discovery = flow::discover(&self.trust, &config.issuer)?;
        let attempt = Attempt::prepare(&config, &discovery)?;
        open(attempt.url().as_str())?;
        let code = attempt.wait(cancel, SIGN_IN_TIMEOUT)?;
        let tokens = flow::exchange_code(&self.trust, &config, &discovery, &attempt, &code)?;
        let keys = flow::fetch_keys(&self.trust, &discovery)?;
        let id_token = tokens.id_token.as_deref().map_or("", String::as_str);
        let claims = jwt::validate_id_token(
            id_token,
            &keys,
            &Expected {
                issuer: &discovery.issuer,
                client_id: &config.client_id,
                nonce: Some(attempt.nonce()),
                subject: None,
                now: (self.clock)(),
            },
        )?;
        drop(attempt);
        let missing = flow::missing_scopes(&config, tokens.scope.as_deref());
        anyhow::ensure!(
            missing.is_empty(),
            "The provider did not grant the scopes {}",
            missing.join(", ")
        );
        let identity = Identity {
            issuer: config.issuer.clone(),
            subject: claims.subject,
            name: claims.name,
            email: claims.email,
        };
        let stored = Stored {
            version: 1,
            issuer: config.issuer.clone(),
            client_id: config.client_id.clone(),
            subject: identity.subject.clone(),
            scopes: config.scopes.clone(),
            resource: config.resource,
            access_token: tokens.access_token.to_string(),
            expires_at: seconds((self.clock)()) + tokens.expires_in,
            refresh_token: tokens
                .refresh_token
                .as_deref()
                .map(|token| token.to_string()),
        };
        let mut work = lock(&record.work);
        if cancel.load(Ordering::SeqCst) {
            return Err(SignInError::new(Failure::Cancelled, "Sign-in was cancelled").into());
        }
        anyhow::ensure!(
            record.generation.load(Ordering::SeqCst) == generation
                && lock(&self.records).contains_key(&id),
            "The sign-in changed while the browser was open. Sign in again"
        );
        self.save(id, &stored)?;
        *work = Work {
            loaded: true,
            tokens: Some(stored),
            discovery: Some(discovery),
            keys: Some(keys),
        };
        lock(&record.config).identity = Some(identity.clone());
        drop(work);
        *lock(&record.problem) = None;
        self.changed();
        Ok(identity)
    }

    fn save(&self, id: Uuid, stored: &Stored) -> Result<()> {
        let record = Zeroizing::new(serde_json::to_string(stored)?);
        self.store.save_tokens(id, &record)
    }

    /// Removes the tokens from Keychain. On failure, the sign-in stays
    /// signed in, because its tokens remain available.
    pub fn sign_out(&self, id: Uuid) -> Result<()> {
        let record = self.record(id)?;
        record.generation.fetch_add(1, Ordering::SeqCst);
        // Wait for a running refresh. It sees the new generation and does
        // not save its result.
        let mut work = lock(&record.work);
        self.store.delete_tokens(id)?;
        *work = Work {
            loaded: true,
            ..Work::default()
        };
        lock(&record.config).identity = None;
        drop(work);
        *lock(&record.problem) = None;
        self.changed();
        Ok(())
    }

    /// Returns an access token of sign-in `id` for `host`, for the identity
    /// `subject` that opened a session. Refreshes it when it expires within
    /// [`EXPIRY_MARGIN`].
    pub fn access_token(&self, id: Uuid, subject: &str, host: &str) -> Result<Zeroizing<String>> {
        let record = self.record(id)?;
        let (config, current) = {
            let config = lock(&record.config);
            let current = config
                .identity
                .as_ref()
                .map(|identity| identity.subject.clone());
            (config.clone(), current)
        };
        anyhow::ensure!(
            config.allows_host(host),
            "The sign-in \"{}\" does not allow sending tokens to {host}. Add the host to the database hosts of the sign-in in the Sign-ins sidebar",
            config.name
        );
        match current {
            None => {
                return Err(required(format!(
                    "Sign in to \"{}\" in the Sign-ins sidebar, then run the query again",
                    config.name
                )));
            }
            Some(current) if current != subject => {
                return Err(required(format!(
                    "The sign-in \"{}\" now uses another account. Run the query again to open a session with it",
                    config.name
                )));
            }
            Some(_) => {}
        }
        self.valid_token(&record, &config, subject)
    }

    fn valid_token(
        &self,
        record: &Record,
        config: &SignIn,
        subject: &str,
    ) -> Result<Zeroizing<String>> {
        let id = config.id;
        let mut work = lock(&record.work);
        let generation = record.generation.load(Ordering::SeqCst);
        if !work.loaded {
            let loaded = self.store.load_tokens(id)?;
            work.tokens = match loaded {
                Some(text) => serde_json::from_str(&text).ok(),
                None => None,
            };
            work.loaded = true;
        }
        let expired_message = || {
            format!(
                "The sign-in \"{}\" has expired. Sign in again in the Sign-ins sidebar",
                config.name
            )
        };
        let Some(tokens) = work
            .tokens
            .as_ref()
            .filter(|tokens| tokens.matches(config, subject))
        else {
            let message = expired_message();
            self.set_problem(record, Some((Failure::SignInRequired, message.clone())));
            return Err(required(message));
        };
        let now = seconds((self.clock)());
        if now + EXPIRY_MARGIN.as_secs() < tokens.expires_at {
            return Ok(Zeroizing::new(tokens.access_token.clone()));
        }
        let Some(refresh_token) = tokens.refresh_token.clone().map(Zeroizing::new) else {
            let message = expired_message();
            self.set_problem(record, Some((Failure::SignInRequired, message.clone())));
            return Err(required(message));
        };
        let result = self.refresh(&mut work, config, &refresh_token, subject);
        let tokens = match result {
            Ok(tokens) => tokens,
            Err(error) => {
                let failure = super::failure(&error);
                let message = format!("{error:#}");
                match failure {
                    Failure::SignInRequired => {
                        // The provider rejected the refresh token. Keep it out
                        // of later attempts. The identity stays for display.
                        work.tokens = None;
                        let _ = self.store.delete_tokens(id);
                        let message = expired_message();
                        self.set_problem(record, Some((failure, message.clone())));
                        return Err(required(message));
                    }
                    Failure::Network => self.set_problem(record, Some((failure, message))),
                    _ => {}
                }
                return Err(error);
            }
        };
        if record.generation.load(Ordering::SeqCst) != generation {
            return Err(required(format!(
                "The sign-in \"{}\" was signed out",
                config.name
            )));
        }
        let missing = flow::missing_scopes(config, tokens.scope.as_deref());
        anyhow::ensure!(
            missing.is_empty(),
            "The provider did not grant the scopes {}",
            missing.join(", ")
        );
        let stored = Stored {
            version: 1,
            issuer: config.issuer.clone(),
            client_id: config.client_id.clone(),
            subject: subject.to_owned(),
            scopes: config.scopes.clone(),
            resource: config.resource.clone(),
            access_token: tokens.access_token.to_string(),
            expires_at: now + tokens.expires_in,
            refresh_token: tokens
                .refresh_token
                .as_deref()
                .map(|token| token.to_string())
                .or_else(|| Some(refresh_token.to_string())),
        };
        // Keychain first: a rotated refresh token replaces the old one at
        // the provider, so an unsaved one would be lost.
        self.save(id, &stored)
            .context("Could not save the refreshed sign-in")?;
        let token = Zeroizing::new(stored.access_token.clone());
        work.tokens = Some(stored);
        drop(work);
        self.set_problem(record, None);
        Ok(token)
    }

    fn refresh(
        &self,
        work: &mut Work,
        config: &SignIn,
        refresh_token: &str,
        subject: &str,
    ) -> Result<flow::Tokens> {
        // A configuration change can replace the issuer of the cache.
        let discovery = match &work.discovery {
            Some(discovery) if discovery.issuer == config.issuer => discovery.clone(),
            _ => {
                let discovery = flow::discover(&self.trust, &config.issuer)?;
                work.discovery = Some(discovery.clone());
                work.keys = None;
                discovery
            }
        };
        let tokens = flow::refresh(&self.trust, config, &discovery, refresh_token)?;
        // The provider replaced the refresh token, so the old one is gone.
        // Keychain gets the new one before the ID token check, which can fail
        // on the network. Its access token is not used until the check passes.
        if let Some(rotated) = tokens
            .refresh_token
            .as_deref()
            .filter(|rotated| *rotated != refresh_token)
            && let Some(current) = &work.tokens
        {
            let pending = Stored {
                version: current.version,
                issuer: current.issuer.clone(),
                client_id: current.client_id.clone(),
                subject: current.subject.clone(),
                scopes: current.scopes.clone(),
                resource: current.resource.clone(),
                access_token: String::new(),
                expires_at: 0,
                refresh_token: Some(rotated.to_string()),
            };
            self.save(config.id, &pending)
                .context("Could not save the refreshed sign-in")?;
            work.tokens = Some(pending);
        }
        if let Some(id_token) = &tokens.id_token {
            let known = jwt::key_id(id_token).is_none_or(|kid| {
                work.keys.as_ref().is_some_and(|keys| {
                    keys.keys
                        .iter()
                        .any(|key| key.kid.as_deref() == Some(kid.as_str()))
                })
            });
            if work.keys.is_none() || !known {
                work.keys = Some(flow::fetch_keys(&self.trust, &discovery)?);
            }
            jwt::validate_id_token(
                id_token,
                work.keys.as_ref().unwrap(),
                &Expected {
                    issuer: &discovery.issuer,
                    client_id: &config.client_id,
                    nonce: None,
                    subject: Some(subject),
                    now: (self.clock)(),
                },
            )?;
        }
        Ok(tokens)
    }

    /// Tries the refresh again after a network failure.
    pub fn retry(&self, id: Uuid) -> Result<()> {
        let record = self.record(id)?;
        let config = lock(&record.config).clone();
        let subject = config
            .identity
            .as_ref()
            .map(|identity| identity.subject.clone())
            .ok_or_else(|| required("Sign in first"))?;
        self.valid_token(&record, &config, &subject).map(drop)
    }

    /// The credentials of a connection that uses a sign-in. This gets an
    /// access token now, so that a missing sign-in fails before the
    /// connection opens. Later calls, for example for cancellation, use the
    /// identity of this call.
    pub fn secret(self: &Arc<Self>, profile: &Profile) -> Result<Secret> {
        let Authentication::Oidc { sign_in } = profile.authentication else {
            anyhow::bail!("The connection does not use a sign-in");
        };
        let subject = self
            .identity(sign_in)
            .map(|identity| identity.subject)
            .unwrap_or_default();
        let source = Bound {
            service: self.clone(),
            sign_in,
            subject,
            host: profile.host.clone(),
        };
        source.access_token()?;
        Ok(Secret::Token(Arc::new(source)))
    }
}

/// The tokens of one sign-in, for one identity and one database host.
struct Bound {
    service: Arc<Service>,
    sign_in: Uuid,
    subject: String,
    host: String,
}

impl TokenSource for Bound {
    fn access_token(&self) -> Result<Zeroizing<String>> {
        self.service
            .access_token(self.sign_in, &self.subject, &self.host)
    }
}

//! The sign-in service against a mock HTTPS provider in this process.
use crate::oidc_provider::{Provider, other_trust, subject, trust};
use anyhow::Result;
use qrow::{
    connector::Secret,
    model::{Authentication, Profile, SignIn},
    oidc::{EXPIRY_MARGIN, Failure, Service, Status, failure},
    storage::TokenStore,
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Sign-in tokens in memory, with failures on request.
#[derive(Default)]
pub struct MemoryTokens {
    records: Mutex<HashMap<Uuid, String>>,
    pub fail_save: AtomicBool,
    pub fail_delete: AtomicBool,
}

impl MemoryTokens {
    fn record(&self, id: Uuid) -> Option<serde_json::Value> {
        let records = self.records.lock().unwrap();
        records
            .get(&id)
            .map(|record| serde_json::from_str(record).unwrap())
    }
}

impl TokenStore for MemoryTokens {
    fn load_tokens(&self, id: Uuid) -> Result<Option<Zeroizing<String>>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .map(Zeroizing::new))
    }
    fn save_tokens(&self, id: Uuid, record: &str) -> Result<()> {
        anyhow::ensure!(
            !self.fail_save.load(Ordering::SeqCst),
            "synthetic Keychain failure"
        );
        self.records.lock().unwrap().insert(id, record.into());
        Ok(())
    }
    fn delete_tokens(&self, id: Uuid) -> Result<()> {
        anyhow::ensure!(
            !self.fail_delete.load(Ordering::SeqCst),
            "synthetic Keychain failure"
        );
        self.records.lock().unwrap().remove(&id);
        Ok(())
    }
}

/// A clock that tests move forward.
struct Clock(AtomicU64);
impl Clock {
    fn new() -> Arc<Self> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Arc::new(Self(AtomicU64::new(now)))
    }
    fn advance(&self, seconds: u64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

struct Setup {
    provider: Provider,
    store: Arc<MemoryTokens>,
    clock: Arc<Clock>,
    service: Arc<Service>,
}

fn setup() -> Setup {
    let provider = Provider::start();
    let store = Arc::new(MemoryTokens::default());
    let clock = Clock::new();
    let time = clock.clone();
    let service = Service::with_clock(
        store.clone(),
        trust(),
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(time.0.load(Ordering::SeqCst))),
    );
    Setup {
        provider,
        store,
        clock,
        service,
    }
}

impl Setup {
    fn add(&self, name: &str) -> SignIn {
        let sign_in = self.provider.sign_in(name);
        self.service.configure(std::slice::from_ref(&sign_in));
        sign_in
    }
    fn sign_in(&self, sign_in: &SignIn, user: &str) -> Result<qrow::model::Identity> {
        let browser = self.provider.browser(user);
        self.service
            .sign_in(sign_in.id, &AtomicBool::new(false), &browser)
    }
}

fn signed_in(setup: &Setup, user: &str) -> SignIn {
    let sign_in = setup.add("Provider");
    setup.sign_in(&sign_in, user).unwrap();
    sign_in
}

#[test]
fn sign_in_saves_the_identity_and_tokens_and_reuses_a_valid_token() {
    let setup = setup();
    let sign_in = setup.add("Provider");
    assert_eq!(setup.service.status(sign_in.id), Status::SignedOut);
    let identity = setup.sign_in(&sign_in, "alice").unwrap();
    assert_eq!(identity.subject, subject("alice"));
    assert_eq!(identity.issuer, setup.provider.issuer);
    assert_eq!(identity.email.as_deref(), Some("alice@qrow.test"));
    assert_eq!(
        setup.service.status(sign_in.id),
        Status::SignedIn(identity.clone())
    );
    let record = setup.store.record(sign_in.id).unwrap();
    assert_eq!(record["subject"], subject("alice"));
    assert!(record.get("id_token").is_none());
    let first = setup
        .service
        .access_token(sign_in.id, &identity.subject, "127.0.0.1")
        .unwrap();
    let second = setup
        .service
        .access_token(sign_in.id, &identity.subject, "127.0.0.1")
        .unwrap();
    assert_eq!(*first, *second);
    assert_eq!(setup.provider.refresh_grants(), 0);
}

#[test]
fn a_new_service_reads_the_tokens_from_keychain() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    let token = setup
        .service
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap();
    let restarted = Service::new(setup.store.clone(), trust());
    let mut restored = sign_in.clone();
    restored.identity = setup.service.identity(sign_in.id);
    restarted.configure(&[restored]);
    assert_eq!(
        *restarted
            .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
            .unwrap(),
        *token
    );
}

#[test]
fn a_rotated_refresh_token_survives_a_failed_key_request() {
    let setup = setup();
    // Each access token needs a refresh.
    setup.provider.set_access_ttl(10);
    let sign_in = signed_in(&setup, "alice");
    // A new service has no keys yet, so the refresh must fetch them.
    let restarted = Service::new(setup.store.clone(), trust());
    let mut restored = sign_in.clone();
    restored.identity = setup.service.identity(sign_in.id);
    restarted.configure(&[restored]);
    setup.provider.set_keys_down(true);
    let error = restarted
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap_err();
    assert_ne!(failure(&error), Failure::SignInRequired, "{error:#}");
    assert_eq!(setup.provider.refresh_grants(), 1);
    // The provider rotated the refresh token. The next attempt uses the new
    // one; the old one would revoke the sign-in.
    setup.provider.set_keys_down(false);
    restarted
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap();
    assert_eq!(setup.provider.refresh_grants(), 2);
}

#[test]
fn one_provider_can_have_several_identities_without_mixing_tokens() {
    let setup = setup();
    let alice = setup.provider.sign_in("Alice");
    let bob = setup.provider.sign_in("Bob");
    setup.service.configure(&[alice.clone(), bob.clone()]);
    setup.sign_in(&alice, "alice").unwrap();
    setup.sign_in(&bob, "bob").unwrap();
    let alice_token = setup
        .service
        .access_token(alice.id, &subject("alice"), "127.0.0.1")
        .unwrap();
    let bob_token = setup
        .service
        .access_token(bob.id, &subject("bob"), "127.0.0.1")
        .unwrap();
    assert!(alice_token.starts_with("access-alice-"));
    assert!(bob_token.starts_with("access-bob-"));
    assert_eq!(
        setup.store.record(alice.id).unwrap()["subject"],
        subject("alice")
    );
    assert_eq!(
        setup.store.record(bob.id).unwrap()["subject"],
        subject("bob")
    );
    let error = setup
        .service
        .access_token(alice.id, &subject("bob"), "127.0.0.1")
        .unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
}

#[test]
fn tokens_go_only_to_allowed_hosts() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    let error = setup
        .service
        .access_token(sign_in.id, &subject("alice"), "evil.example.test")
        .unwrap_err();
    assert!(error.to_string().contains("does not allow"), "{error}");
    assert!(
        setup
            .service
            .access_token(sign_in.id, &subject("alice"), "127.0.0.1.")
            .is_ok()
    );
}

#[test]
fn a_token_refreshes_at_the_expiry_margin_and_the_rotated_token_is_saved_first() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    let old_refresh = setup.store.record(sign_in.id).unwrap()["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let get = || {
        setup
            .service
            .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
            .unwrap()
    };
    let first = get();
    // The token lives 300 seconds. One second before the margin it is valid.
    setup.clock.advance(300 - EXPIRY_MARGIN.as_secs() - 1);
    assert_eq!(*get(), *first);
    assert_eq!(setup.provider.refresh_grants(), 0);
    setup.clock.advance(1);
    let refreshed = get();
    assert_ne!(*refreshed, *first);
    assert_eq!(setup.provider.refresh_grants(), 1);
    let record = setup.store.record(sign_in.id).unwrap();
    let new_refresh = record["refresh_token"].as_str().unwrap();
    assert_ne!(new_refresh, old_refresh);
    assert!(setup.provider.refresh_token_is_active(new_refresh));
    assert!(!setup.provider.refresh_token_is_active(&old_refresh));
}

#[test]
fn a_refresh_that_cannot_be_saved_is_not_used() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.clock.advance(300);
    setup.store.fail_save.store(true, Ordering::SeqCst);
    let error = setup
        .service
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap_err();
    assert!(format!("{error:#}").contains("synthetic Keychain failure"));
    assert!(!format!("{error:#}").contains("access-"));
}

#[test]
fn concurrent_requests_from_tabs_refresh_once() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.clock.advance(300);
    setup.provider.set_refresh_delay(Duration::from_millis(300));
    let tokens: Vec<_> = (0..6)
        .map(|_| {
            let service = setup.service.clone();
            let id = sign_in.id;
            thread::spawn(move || {
                service
                    .access_token(id, &subject("alice"), "127.0.0.1")
                    .unwrap()
                    .to_string()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(setup.provider.refresh_grants(), 1);
    assert!(tokens.iter().all(|token| *token == tokens[0]));
}

#[test]
fn a_rejected_refresh_requires_a_new_sign_in() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.provider.revoke("alice");
    setup.clock.advance(300);
    let error = setup
        .service
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
    assert!(matches!(
        setup.service.status(sign_in.id),
        Status::SignInRequired(..)
    ));
    assert!(setup.store.record(sign_in.id).is_none());
    // A new browser sign-in recovers.
    setup.sign_in(&sign_in, "alice").unwrap();
    assert!(
        setup
            .service
            .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
            .is_ok()
    );
    assert!(matches!(
        setup.service.status(sign_in.id),
        Status::SignedIn(..)
    ));
}

#[test]
fn without_a_refresh_token_an_expired_sign_in_needs_the_browser() {
    let setup = setup();
    setup.provider.set_refresh_tokens(false);
    let sign_in = signed_in(&setup, "alice");
    setup.clock.advance(300);
    let error = setup
        .service
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
    assert_eq!(setup.provider.refresh_grants(), 0);
}

#[test]
fn a_network_failure_keeps_the_identity_and_a_retry_recovers() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.clock.advance(300);
    setup.provider.set_down(true);
    let error = setup
        .service
        .access_token(sign_in.id, &subject("alice"), "127.0.0.1")
        .unwrap_err();
    assert_eq!(failure(&error), Failure::Network);
    assert!(matches!(
        setup.service.status(sign_in.id),
        Status::NetworkFailure(..)
    ));
    setup.provider.set_down(false);
    setup.service.retry(sign_in.id).unwrap();
    assert!(matches!(
        setup.service.status(sign_in.id),
        Status::SignedIn(..)
    ));
}

#[test]
fn sign_out_during_a_refresh_discards_the_refreshed_tokens() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.clock.advance(300);
    setup.provider.set_refresh_delay(Duration::from_millis(500));
    let refresh = {
        let service = setup.service.clone();
        let id = sign_in.id;
        thread::spawn(move || service.access_token(id, &subject("alice"), "127.0.0.1"))
    };
    thread::sleep(Duration::from_millis(150));
    setup.service.sign_out(sign_in.id).unwrap();
    let error = refresh.join().unwrap().unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
    assert!(setup.store.record(sign_in.id).is_none());
    assert_eq!(setup.service.status(sign_in.id), Status::SignedOut);
}

#[test]
fn a_failed_keychain_deletion_keeps_the_sign_in() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.store.fail_delete.store(true, Ordering::SeqCst);
    assert!(setup.service.sign_out(sign_in.id).is_err());
    assert!(matches!(
        setup.service.status(sign_in.id),
        Status::SignedIn(..)
    ));
    assert!(setup.store.record(sign_in.id).is_some());
}

#[test]
fn a_sign_in_that_finishes_after_removal_is_discarded() {
    let setup = setup();
    let sign_in = setup.add("Provider");
    let service = setup.service.clone();
    let location: Arc<Mutex<Option<String>>> = Arc::default();
    let opened = location.clone();
    let state = Provider::authorize;
    let provider = &setup.provider;
    thread::scope(|scope| {
        let running = scope.spawn(|| {
            service.sign_in(sign_in.id, &AtomicBool::new(false), &|url| {
                *opened.lock().unwrap() = Some(state(provider, url, "alice")?);
                Ok(())
            })
        });
        while location.lock().unwrap().is_none() {
            thread::sleep(Duration::from_millis(10));
        }
        service.configure(&[]);
        crate::oidc_provider::callback(location.lock().unwrap().as_deref().unwrap());
        assert!(running.join().unwrap().is_err());
    });
    assert!(setup.store.record(sign_in.id).is_none());
}

#[test]
fn cancellation_and_denial_end_the_sign_in_without_tokens() {
    let setup = setup();
    let sign_in = setup.add("Provider");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let error = setup
        .service
        .sign_in(sign_in.id, &cancel, &move |_| {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(failure(&error), Failure::Cancelled);
    let error = setup
        .service
        .sign_in(
            sign_in.id,
            &AtomicBool::new(false),
            &setup.provider.denying_browser(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("denied"), "{error}");
    assert!(setup.store.record(sign_in.id).is_none());
    assert_eq!(setup.service.status(sign_in.id), Status::SignedOut);
}

#[test]
fn an_invalid_id_token_or_missing_scope_rejects_the_sign_in() {
    let setup = setup();
    let sign_in = setup.add("Provider");
    setup.provider.set_wrong_nonce(true);
    let error = setup.sign_in(&sign_in, "alice").unwrap_err();
    assert!(error.to_string().contains("sign-in attempt"), "{error}");
    setup.provider.set_wrong_nonce(false);
    setup.provider.set_granted_scope(Some("openid"));
    let error = setup.sign_in(&sign_in, "alice").unwrap_err();
    assert!(error.to_string().contains("kyuubi"), "{error}");
    assert!(setup.store.record(sign_in.id).is_none());
}

#[test]
fn the_provider_must_present_a_trusted_certificate_and_the_configured_issuer() {
    let provider = Provider::start();
    let store = Arc::new(MemoryTokens::default());
    let untrusted = Service::new(store.clone(), other_trust());
    let sign_in = provider.sign_in("Provider");
    untrusted.configure(std::slice::from_ref(&sign_in));
    let error = untrusted
        .sign_in(
            sign_in.id,
            &AtomicBool::new(false),
            &provider.browser("alice"),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("certificate"), "{error:#}");

    let service = Service::new(store, trust());
    let mut other_issuer = provider.sign_in("Provider");
    other_issuer.issuer = format!("{}/", provider.issuer);
    service.configure(std::slice::from_ref(&other_issuer));
    let error = service
        .sign_in(
            other_issuer.id,
            &AtomicBool::new(false),
            &provider.browser("alice"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("uses the issuer"), "{error}");
}

#[test]
fn a_connection_secret_refreshes_for_cancellation_after_expiry() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    let profile = Profile {
        host: "127.0.0.1".into(),
        tls: true,
        authentication: Authentication::Oidc {
            sign_in: sign_in.id,
        },
        ..Profile::default()
    };
    let secret = setup.service.secret(&profile).unwrap();
    let Secret::Token(source) = secret.clone() else {
        panic!("a sign-in gives a token source")
    };
    let first = secret.value().unwrap();
    setup.clock.advance(300);
    let later = source.access_token().unwrap();
    assert_ne!(*first, *later);
    assert_eq!(format!("{secret:?}"), "Secret::Token(..)");
    // A sign-in as another identity does not serve the old session.
    setup.service.sign_out(sign_in.id).unwrap();
    setup.sign_in(&sign_in, "bob").unwrap();
    let error = source.access_token().unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
}

#[test]
fn a_connection_without_a_sign_in_fails_before_connecting() {
    let setup = setup();
    let sign_in = setup.add("Provider");
    let profile = Profile {
        host: "127.0.0.1".into(),
        tls: true,
        authentication: Authentication::Oidc {
            sign_in: sign_in.id,
        },
        ..Profile::default()
    };
    let error = setup.service.secret(&profile).unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
    assert!(error.to_string().contains("in the Sign-ins sidebar"));
    let missing = Profile {
        authentication: Authentication::Oidc {
            sign_in: Uuid::new_v4(),
        },
        ..profile
    };
    assert_eq!(
        failure(&setup.service.secret(&missing).unwrap_err()),
        Failure::SignInRequired
    );
}

#[test]
fn configuration_does_not_wait_for_a_running_refresh() {
    let setup = setup();
    let sign_in = signed_in(&setup, "alice");
    setup.clock.advance(300);
    setup.provider.set_refresh_delay(Duration::from_millis(800));
    let refresh = {
        let service = setup.service.clone();
        let id = sign_in.id;
        thread::spawn(move || service.access_token(id, &subject("alice"), "127.0.0.1"))
    };
    thread::sleep(Duration::from_millis(150));
    let mut changed = sign_in.clone();
    changed.scopes.push("groups".into());
    let started = std::time::Instant::now();
    setup.service.configure(std::slice::from_ref(&changed));
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "configure waited for the refresh"
    );
    // The refresh that started before the change does not publish tokens.
    let error = refresh.join().unwrap().unwrap_err();
    assert_eq!(failure(&error), Failure::SignInRequired);
    // Sign-out still completes after the change.
    setup.service.sign_out(sign_in.id).unwrap();
}

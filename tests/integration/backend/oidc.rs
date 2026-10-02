//! Sign-in authentication and TLS against the real fixture: the mock OpenID
//! Connect provider, the TLS proxy, and the token authenticator of Kyuubi.
use super::*;
use crate::oidc_provider::FixtureProvider;
use qrow::{
    model::{Authentication, SignIn},
    oidc::Service,
    storage::MemoryTokenStore,
    tls::Trust,
};
use std::sync::atomic::AtomicBool;

/// A signed-in service and a connection that uses the sign-in.
struct Signed {
    service: Arc<Service>,
    sign_in: SignIn,
    fixture: FixtureProvider,
}

fn sign_in(user: &str, options: &[(&str, &str)]) -> Result<Signed> {
    let fixture = FixtureProvider::get();
    let service = Service::new(Arc::new(MemoryTokenStore::default()), fixture.trust.clone());
    let sign_in = fixture.sign_in("Fixture");
    service.configure(std::slice::from_ref(&sign_in));
    let identity = service.sign_in(
        sign_in.id,
        &AtomicBool::new(false),
        &fixture.browser(user, options),
    )?;
    ensure!(identity.email == Some(format!("{user}@qrow.test")));
    Ok(Signed {
        service,
        sign_in,
        fixture,
    })
}

impl Signed {
    fn profile(&self, username: &str) -> Result<Profile> {
        Ok(Profile {
            port: self.fixture.tls_port,
            username: username.into(),
            tls: true,
            authentication: Authentication::Oidc {
                sign_in: self.sign_in.id,
            },
            ..profile()?
        })
    }
    fn client(&self, username: &str) -> Result<Client> {
        let service = self.service.clone();
        Ok(Client::with(
            self.profile(username)?,
            HiveConnector::new(self.fixture.trust.clone()),
            Arc::new(move |profile: &Profile| service.secret(profile)),
        ))
    }
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn an_access_token_opens_a_session_over_tls_as_the_connection_user() -> Result<()> {
    let signed = sign_in("alice", &[])?;
    let client = signed.client("qrow")?;
    scalar(&client.query("SELECT current_user()")?, "qrow");
    // One sign-in serves several connections with their own usernames. The
    // fixture lets alice use only "qrow", so the server sees each username.
    let other = signed.client("qrow-analytics")?;
    other.run("SELECT 1");
    ensure!(other.failure()?, "a rejected connection is discarded");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn a_valid_token_without_access_to_the_account_is_rejected() -> Result<()> {
    let signed = sign_in("mallory", &[])?;
    let error = HiveConnector::new(signed.fixture.trust.clone())
        .connect(
            &signed.profile("qrow")?,
            signed.service.secret(&signed.profile("qrow")?)?,
        )
        .err()
        .context("mallory has no database account")?;
    let message = format!("{error:#}");
    ensure!(
        message.contains("did not accept the access token for the database account \"qrow\""),
        "{message}"
    );
    ensure!(!message.contains("eyJ"), "the error contains a token");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn cancellation_refreshes_an_expired_token_for_its_own_transport() -> Result<()> {
    // A token that lives 2 seconds is always inside the expiry margin, so
    // each new transport refreshes it.
    let signed = sign_in("alice", &[("fixture_access_ttl", "2")])?;
    let client = signed.client("qrow")?;
    client.query(REGISTER)?;
    let token = format!("oidc-cancel-{}", uuid::Uuid::new_v4());
    client.run(&blocking(&token));
    wait_evidence(&token, "started", Instant::now() + TIMEOUT)?;
    // The token of the session has expired at the server.
    thread::sleep(Duration::from_secs(4));
    let (_, refreshes) = signed.fixture.grants();
    client.worker.cancel();
    let deadline = Instant::now() + Duration::from_secs(10);
    wait_evidence(&token, "interrupted", deadline)?;
    loop {
        match client.event(deadline)? {
            Event::Cancelled => break,
            Event::Error { message, .. } | Event::CancelError(message) => {
                anyhow::bail!("{message}")
            }
            Event::Ready { .. } => anyhow::bail!("Cancelled operation reported success"),
            _ => {}
        }
    }
    let (_, after) = signed.fixture.grants();
    ensure!(after > refreshes, "cancellation refreshed the token");
    // The session that opened with the old token stays usable.
    scalar(&client.query("SELECT 45")?, "45");
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn a_revoked_refresh_token_requires_a_new_sign_in_without_resubmitting_sql() -> Result<()> {
    let signed = sign_in("bob", &[("fixture_access_ttl", "2")])?;
    signed.fixture.revoke("bob");
    let client = signed.client("qrow")?;
    client.run("SELECT 1");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match client.event(deadline)? {
            Event::Error {
                message,
                sign_in_required,
                ..
            } => {
                ensure!(sign_in_required, "{message}");
                ensure!(message.contains("Sign in again"), "{message}");
                break;
            }
            Event::Running | Event::Ready { .. } => anyhow::bail!("SQL ran without a sign-in"),
            _ => {}
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires disposable LDAP/Kyuubi/Spark fixture"]
fn a_password_also_works_over_tls_and_tls_verifies_the_server() -> Result<()> {
    let fixture = FixtureProvider::get();
    let tls = Profile {
        port: fixture.tls_port,
        tls: true,
        ..profile()?
    };
    let client = Client::with(
        tls.clone(),
        HiveConnector::new(fixture.trust),
        Arc::new(|_| Ok(Secret::password("qrow-test-password"))),
    );
    scalar(&client.query("SELECT 42")?, "42");
    // Another authority did not sign the certificate of the fixture.
    let untrusted = Trust::from_pem(include_bytes!("../testdata/tls/other-ca.pem"))?;
    let error = HiveConnector::new(untrusted)
        .connect(&tls, Secret::password("qrow-test-password"))
        .err()
        .context("an untrusted server")?;
    ensure!(format!("{error:#}").contains("TLS handshake"), "{error:#}");
    Ok(())
}

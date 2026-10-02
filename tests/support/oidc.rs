//! A mock OpenID Connect provider over HTTPS, in the test process. It signs
//! ID tokens with a new P-256 key and issues opaque access tokens. A fake
//! browser answers authorization requests for synthetic users.
#![allow(dead_code)]
use anyhow::Result;
use base64::Engine;
use qrow::{model::SignIn, oidc::jwt::BASE64URL, tls::Trust};
use ring::{
    digest,
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, pem::PemObject};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

pub const CLIENT_ID: &str = "qrow-desktop";
const CA: &[u8] = include_bytes!("../integration/testdata/tls/ca.pem");
const OTHER_CA: &[u8] = include_bytes!("../integration/testdata/tls/other-ca.pem");
const CERTIFICATE: &[u8] = include_bytes!("../integration/testdata/tls/server.pem");
const KEY: &[u8] = include_bytes!("../integration/testdata/tls/server-key.der");

/// The synthetic authority that signed the certificate of the provider.
pub fn trust() -> Trust {
    Trust::from_pem(CA).unwrap()
}

/// An authority that did not sign the certificate of the provider.
pub fn other_trust() -> Trust {
    Trust::from_pem(OTHER_CA).unwrap()
}

pub fn server_config() -> Arc<ServerConfig> {
    let chain = CertificateDer::pem_slice_iter(CERTIFICATE)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap(),
    )
}

/// The subject of a synthetic user.
pub fn subject(user: &str) -> String {
    format!("subject-{user}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct Grant {
    user: String,
    challenge: String,
    redirect_uri: String,
    nonce: String,
    scope: String,
}

#[derive(Default)]
struct Settings {
    access_ttl: u64,
    refresh: bool,
    down: bool,
    refresh_delay: Duration,
    /// Replaces the nonce of issued ID tokens.
    wrong_nonce: bool,
    /// Grants only these scopes, when set.
    granted_scope: Option<String>,
}

struct State {
    issuer: String,
    key: EcdsaKeyPair,
    settings: Mutex<Settings>,
    codes: Mutex<HashMap<String, Grant>>,
    /// Refresh token → (user, scope). Rotation removes the old token.
    refresh_tokens: Mutex<HashMap<String, (String, String)>>,
    /// Refresh tokens that a rotation replaced. A reuse revokes the user.
    rotated: Mutex<HashMap<String, String>>,
    counter: AtomicU64,
    authorization_grants: AtomicUsize,
    refresh_grants: AtomicUsize,
    stop: AtomicBool,
}

pub struct Provider {
    pub issuer: String,
    state: Arc<State>,
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
    }
}

impl Provider {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let issuer = format!(
            "https://127.0.0.1:{}",
            listener.local_addr().unwrap().port()
        );
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .unwrap();
        let state = Arc::new(State {
            issuer: issuer.clone(),
            key,
            settings: Mutex::new(Settings {
                access_ttl: 300,
                refresh: true,
                ..Settings::default()
            }),
            codes: Mutex::default(),
            refresh_tokens: Mutex::default(),
            rotated: Mutex::default(),
            counter: AtomicU64::new(0),
            authorization_grants: AtomicUsize::new(0),
            refresh_grants: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let config = server_config();
        let shared = state.clone();
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
            while !shared.stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = shared.clone();
                        let config = config.clone();
                        thread::spawn(move || {
                            let _ = serve(&state, config, stream);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Self { issuer, state }
    }

    /// A sign-in configuration for this provider. Tokens can go to 127.0.0.1.
    pub fn sign_in(&self, name: &str) -> SignIn {
        SignIn {
            name: name.into(),
            issuer: self.issuer.clone(),
            client_id: CLIENT_ID.into(),
            scopes: vec!["kyuubi".into()],
            allowed_hosts: vec!["127.0.0.1".into()],
            ..SignIn::default()
        }
    }

    pub fn set_access_ttl(&self, seconds: u64) {
        self.state.settings.lock().unwrap().access_ttl = seconds;
    }
    pub fn set_refresh_tokens(&self, issue: bool) {
        self.state.settings.lock().unwrap().refresh = issue;
    }
    /// Makes the token endpoint answer 503.
    pub fn set_down(&self, down: bool) {
        self.state.settings.lock().unwrap().down = down;
    }
    pub fn set_refresh_delay(&self, delay: Duration) {
        self.state.settings.lock().unwrap().refresh_delay = delay;
    }
    pub fn set_wrong_nonce(&self, wrong: bool) {
        self.state.settings.lock().unwrap().wrong_nonce = wrong;
    }
    pub fn set_granted_scope(&self, scope: Option<&str>) {
        self.state.settings.lock().unwrap().granted_scope = scope.map(str::to_owned);
    }
    /// Revokes all refresh tokens of `user`.
    pub fn revoke(&self, user: &str) {
        self.state
            .refresh_tokens
            .lock()
            .unwrap()
            .retain(|_, (owner, _)| owner != user);
    }
    pub fn authorization_grants(&self) -> usize {
        self.state.authorization_grants.load(Ordering::SeqCst)
    }
    pub fn refresh_grants(&self) -> usize {
        self.state.refresh_grants.load(Ordering::SeqCst)
    }
    /// Whether the provider would accept this refresh token.
    pub fn refresh_token_is_active(&self, token: &str) -> bool {
        self.state
            .refresh_tokens
            .lock()
            .unwrap()
            .contains_key(token)
    }

    /// A browser that signs in as `user` and returns to the callback.
    pub fn browser(&self, user: &str) -> impl Fn(&str) -> Result<()> + Send + Sync + 'static {
        let state = self.state.clone();
        let user = user.to_owned();
        move |url: &str| {
            let location = authorize(&state, url, &user, false)?;
            thread::spawn(move || callback(&location));
            Ok(())
        }
    }

    /// A browser in which the user denies access.
    pub fn denying_browser(&self) -> impl Fn(&str) -> Result<()> + Send + Sync + 'static {
        let state = self.state.clone();
        move |url: &str| {
            let location = authorize(&state, url, "alice", true)?;
            thread::spawn(move || callback(&location));
            Ok(())
        }
    }

    /// The callback URL that a browser would open for `user`, without
    /// opening it.
    pub fn authorize(&self, url: &str, user: &str) -> Result<String> {
        authorize(&self.state, url, user, false)
    }
}

/// Opens a callback URL like a browser and returns the response status line.
pub fn callback(location: &str) -> String {
    let url = Url::parse(location).unwrap();
    let mut stream = TcpStream::connect((
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap(),
    ))
    .unwrap();
    let target = format!("{}?{}", url.path(), url.query().unwrap_or_default());
    write!(stream, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.lines().next().unwrap_or_default().to_owned()
}

fn authorize(state: &State, url: &str, user: &str, deny: bool) -> Result<String> {
    let url = Url::parse(url)?;
    anyhow::ensure!(
        url.as_str()
            .starts_with(&format!("{}/authorize", state.issuer))
    );
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    anyhow::ensure!(query.get("client_id").map(String::as_str) == Some(CLIENT_ID));
    anyhow::ensure!(query.get("response_type").map(String::as_str) == Some("code"));
    anyhow::ensure!(query.get("code_challenge_method").map(String::as_str) == Some("S256"));
    let redirect_uri = query["redirect_uri"].clone();
    anyhow::ensure!(redirect_uri.starts_with("http://127.0.0.1:"));
    anyhow::ensure!(query["scope"].split(' ').any(|scope| scope == "openid"));
    let mut location = Url::parse(&redirect_uri)?;
    location
        .query_pairs_mut()
        .append_pair("state", &query["state"])
        .append_pair("iss", &state.issuer);
    if deny {
        location
            .query_pairs_mut()
            .append_pair("error", "access_denied");
        return Ok(location.into());
    }
    let code = format!("code-{}", state.counter.fetch_add(1, Ordering::SeqCst));
    state.codes.lock().unwrap().insert(
        code.clone(),
        Grant {
            user: user.into(),
            challenge: query["code_challenge"].clone(),
            redirect_uri,
            nonce: query["nonce"].clone(),
            scope: query["scope"].clone(),
        },
    );
    location.query_pairs_mut().append_pair("code", &code);
    Ok(location.into())
}

fn sign(key: &EcdsaKeyPair, claims: Value) -> String {
    let message = format!(
        "{}.{}",
        BASE64URL.encode(json!({"alg": "ES256", "kid": "mock"}).to_string()),
        BASE64URL.encode(claims.to_string())
    );
    let signature = key.sign(&SystemRandom::new(), message.as_bytes()).unwrap();
    format!("{message}.{}", BASE64URL.encode(signature.as_ref()))
}

fn serve(state: &State, config: Arc<ServerConfig>, tcp: TcpStream) -> Result<()> {
    tcp.set_nonblocking(false)?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut stream = StreamOwned::new(ServerConnection::new(config)?, tcp);
    let mut data = Vec::new();
    let mut buffer = [0; 4096];
    let (head, length) = loop {
        let read = stream.read(&mut buffer)?;
        anyhow::ensure!(read > 0);
        data.extend_from_slice(&buffer[..read]);
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        if let httparse::Status::Complete(head) = request.parse(&data)? {
            let length = request
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("content-length"))
                .map(|header| std::str::from_utf8(header.value).unwrap().parse().unwrap())
                .unwrap_or(0usize);
            break (head, length);
        }
    };
    while data.len() < head + length {
        let read = stream.read(&mut buffer)?;
        anyhow::ensure!(read > 0);
        data.extend_from_slice(&buffer[..read]);
    }
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut request = httparse::Request::new(&mut headers);
    request.parse(&data)?;
    let path = request.path.unwrap_or_default().to_owned();
    let body = &data[head..head + length];
    let (status, response) = route(state, &path, body);
    let response = response.to_string();
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    )?;
    stream.conn.send_close_notify();
    stream.flush()?;
    Ok(())
}

fn error(code: &str) -> (u16, Value) {
    (400, json!({"error": code, "error_description": "mock"}))
}

fn route(state: &State, path: &str, body: &[u8]) -> (u16, Value) {
    match path {
        "/.well-known/openid-configuration" => (
            200,
            json!({
                "issuer": state.issuer,
                "authorization_endpoint": format!("{}/authorize", state.issuer),
                "token_endpoint": format!("{}/token", state.issuer),
                "jwks_uri": format!("{}/jwks", state.issuer),
                "code_challenge_methods_supported": ["S256"],
            }),
        ),
        "/jwks" => {
            let point = state.key.public_key().as_ref();
            (
                200,
                json!({"keys": [{"kty": "EC", "crv": "P-256", "kid": "mock", "use": "sig",
                    "x": BASE64URL.encode(&point[1..33]), "y": BASE64URL.encode(&point[33..])}]}),
            )
        }
        "/token" => token(state, body),
        _ => (404, json!({})),
    }
}

fn token(state: &State, body: &[u8]) -> (u16, Value) {
    let form: HashMap<String, String> = form_urlencoded::parse(body).into_owned().collect();
    let (ttl, refresh, down, delay, wrong_nonce, granted) = {
        let settings = state.settings.lock().unwrap();
        (
            settings.access_ttl,
            settings.refresh,
            settings.down,
            settings.refresh_delay,
            settings.wrong_nonce,
            settings.granted_scope.clone(),
        )
    };
    if down {
        return (503, json!({"error": "temporarily_unavailable"}));
    }
    if form.get("client_id").map(String::as_str) != Some(CLIENT_ID) {
        return error("invalid_client");
    }
    let number = state.counter.fetch_add(1, Ordering::SeqCst);
    let (user, scope, nonce) = match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some(grant) = state.codes.lock().unwrap().remove(&form["code"]) else {
                return error("invalid_grant");
            };
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            let challenge = BASE64URL.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
            if challenge != grant.challenge || form.get("redirect_uri") != Some(&grant.redirect_uri)
            {
                return error("invalid_grant");
            }
            state.authorization_grants.fetch_add(1, Ordering::SeqCst);
            (grant.user, grant.scope, Some(grant.nonce))
        }
        Some("refresh_token") => {
            thread::sleep(delay);
            let token = form.get("refresh_token").cloned().unwrap_or_default();
            let Some((user, scope)) = state.refresh_tokens.lock().unwrap().remove(&token) else {
                if let Some(user) = state.rotated.lock().unwrap().get(&token).cloned() {
                    // Reuse of a rotated token revokes its family.
                    state
                        .refresh_tokens
                        .lock()
                        .unwrap()
                        .retain(|_, (owner, _)| *owner != user);
                }
                return error("invalid_grant");
            };
            state.rotated.lock().unwrap().insert(token, user.clone());
            state.refresh_grants.fetch_add(1, Ordering::SeqCst);
            (user, scope, None)
        }
        _ => return error("unsupported_grant_type"),
    };
    let scope = granted.unwrap_or(scope);
    let mut response = json!({
        "access_token": format!("access-{user}-{number}"),
        "token_type": "Bearer",
        "expires_in": ttl,
        "scope": scope,
    });
    if refresh {
        let token = format!("refresh-{user}-{number}");
        state
            .refresh_tokens
            .lock()
            .unwrap()
            .insert(token.clone(), (user.clone(), scope));
        response["refresh_token"] = json!(token);
    }
    let mut claims = json!({
        "iss": state.issuer, "aud": CLIENT_ID, "azp": CLIENT_ID, "sub": subject(&user),
        "exp": now() + 300, "iat": now(), "email": format!("{user}@qrow.test"),
        "name": format!("{user} fixture"),
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = json!(if wrong_nonce { "wrong".into() } else { nonce });
    }
    response["id_token"] = json!(sign(&state.key, claims));
    (200, response)
}

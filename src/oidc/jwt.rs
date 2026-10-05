//! Signature and claim validation of OpenID Connect ID tokens.
use anyhow::{Context, Result};
use base64::{
    Engine,
    alphabet::URL_SAFE,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The allowed difference between the clocks of Qrow and the provider.
pub const CLOCK_SKEW: Duration = Duration::from_secs(60);

pub const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// The public signing keys of a provider.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Jwks {
    #[serde(default)]
    pub keys: Vec<Jwk>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default, rename = "use")]
    pub usage: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Algorithm {
    Rs256,
    Ps256,
    Es256,
}

impl Algorithm {
    fn parse(name: &str) -> Result<Self> {
        match name {
            "RS256" => Ok(Self::Rs256),
            "PS256" => Ok(Self::Ps256),
            "ES256" => Ok(Self::Es256),
            other => anyhow::bail!("The ID token uses the unsupported algorithm {other}"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Ps256 => "PS256",
            Self::Es256 => "ES256",
        }
    }

    fn key_type(self) -> &'static str {
        match self {
            Self::Rs256 | Self::Ps256 => "RSA",
            Self::Es256 => "EC",
        }
    }
}

impl Jwk {
    fn accepts(&self, algorithm: Algorithm, kid: Option<&str>) -> bool {
        self.kty == algorithm.key_type()
            && self.usage.as_deref().is_none_or(|usage| usage == "sig")
            && self
                .alg
                .as_deref()
                .is_none_or(|alg| alg == algorithm.name())
            && kid.is_none_or(|kid| self.kid.as_deref() == Some(kid))
    }

    fn verify(&self, algorithm: Algorithm, message: &[u8], signature: &[u8]) -> Result<()> {
        let decode = |value: &Option<String>, name: &str| {
            value
                .as_deref()
                .and_then(|value| BASE64URL.decode(value).ok())
                .with_context(|| format!("The signing key has no valid {name}"))
        };
        let verified = match algorithm {
            Algorithm::Rs256 | Algorithm::Ps256 => {
                let key = RsaPublicKeyComponents {
                    n: decode(&self.n, "modulus")?,
                    e: decode(&self.e, "exponent")?,
                };
                let parameters = if algorithm == Algorithm::Rs256 {
                    &signature::RSA_PKCS1_2048_8192_SHA256
                } else {
                    &signature::RSA_PSS_2048_8192_SHA256
                };
                key.verify(parameters, message, signature)
            }
            Algorithm::Es256 => {
                anyhow::ensure!(
                    self.crv.as_deref() == Some("P-256"),
                    "The signing key does not use the P-256 curve"
                );
                let mut point = vec![4];
                point.extend(decode(&self.x, "x coordinate")?);
                point.extend(decode(&self.y, "y coordinate")?);
                UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point)
                    .verify(message, signature)
            }
        };
        verified.map_err(|_| anyhow::anyhow!("The signature of the ID token is not valid"))
    }
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    crit: Option<Value>,
}

/// What an ID token must match.
pub struct Expected<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    /// The nonce of the authorization request. A refreshed ID token can
    /// omit it.
    pub nonce: Option<&'a str>,
    /// The subject of the current identity, for a refreshed ID token.
    pub subject: Option<&'a str>,
    pub now: SystemTime,
}

/// The claims of a valid ID token that Qrow uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdClaims {
    pub subject: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// The key ID of a token header, to select keys before validation.
pub fn key_id(token: &str) -> Option<String> {
    let header = token.split('.').next()?;
    let header: Header = serde_json::from_slice(&BASE64URL.decode(header).ok()?).ok()?;
    header.kid
}

/// Validates the signature and the claims of an ID token. Errors do not
/// contain the token.
pub fn validate_id_token(token: &str, keys: &Jwks, expected: &Expected<'_>) -> Result<IdClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    anyhow::ensure!(parts.len() == 3, "The ID token is not a signed JWT");
    let header: Header = BASE64URL
        .decode(parts[0])
        .ok()
        .and_then(|header| serde_json::from_slice(&header).ok())
        .context("The ID token has an invalid header")?;
    anyhow::ensure!(
        header.crit.is_none(),
        "The ID token requires extensions that Qrow does not support"
    );
    let algorithm = Algorithm::parse(&header.alg)?;
    let signature = BASE64URL
        .decode(parts[2])
        .context("The ID token has an invalid signature encoding")?;
    let message = format!("{}.{}", parts[0], parts[1]);
    let candidates: Vec<&Jwk> = keys
        .keys
        .iter()
        .filter(|key| key.accepts(algorithm, header.kid.as_deref()))
        .collect();
    anyhow::ensure!(
        !candidates.is_empty(),
        "The provider has no key that can verify the ID token"
    );
    let mut verified = Err(anyhow::anyhow!(
        "The signature of the ID token is not valid"
    ));
    for key in candidates {
        verified = key.verify(algorithm, message.as_bytes(), &signature);
        if verified.is_ok() {
            break;
        }
    }
    verified?;
    let claims: serde_json::Map<String, Value> = BASE64URL
        .decode(parts[1])
        .ok()
        .and_then(|claims| serde_json::from_slice(&claims).ok())
        .context("The ID token has invalid claims")?;
    validate_claims(&claims, expected)
}

fn validate_claims(
    claims: &serde_json::Map<String, Value>,
    expected: &Expected<'_>,
) -> Result<IdClaims> {
    let text = |name: &str| claims.get(name).and_then(Value::as_str);
    anyhow::ensure!(
        text("iss") == Some(expected.issuer),
        "The ID token comes from a different issuer"
    );
    let audiences: Vec<&str> = match claims.get("aud") {
        Some(Value::String(audience)) => vec![audience.as_str()],
        Some(Value::Array(audiences)) => audiences.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    anyhow::ensure!(
        audiences.contains(&expected.client_id),
        "The ID token is not for this client"
    );
    if audiences.len() > 1 || claims.contains_key("azp") {
        anyhow::ensure!(
            text("azp") == Some(expected.client_id),
            "The ID token was issued to another party"
        );
    }
    let now = expected
        .now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let skew = CLOCK_SKEW.as_secs();
    let expiry = claims
        .get("exp")
        .and_then(Value::as_u64)
        .context("The ID token has no expiry")?;
    anyhow::ensure!(expiry + skew > now, "The ID token has expired");
    let issued = claims
        .get("iat")
        .and_then(Value::as_u64)
        .context("The ID token has no issue time")?;
    anyhow::ensure!(
        issued <= now + skew,
        "The ID token was issued in the future. Check the clock of this Mac"
    );
    if let Some(nonce) = expected.nonce {
        anyhow::ensure!(
            text("nonce") == Some(nonce),
            "The ID token does not belong to this sign-in attempt"
        );
    }
    let subject = text("sub")
        .filter(|subject| !subject.is_empty() && subject.len() <= 255)
        .context("The ID token has no valid subject")?;
    if let Some(current) = expected.subject {
        anyhow::ensure!(
            subject == current,
            "The provider returned a token for a different account"
        );
    }
    let optional = |name: &str| {
        text(name)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(200).collect())
    };
    Ok(IdClaims {
        subject: subject.to_owned(),
        name: optional("name").or_else(|| optional("preferred_username")),
        email: optional("email"),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::{
        rand::SystemRandom,
        signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
    };
    use serde_json::json;

    /// A P-256 signing key and its JWKS, for tests.
    pub(crate) struct TestKey {
        pair: EcdsaKeyPair,
        pub kid: String,
    }

    impl TestKey {
        pub(crate) fn new(kid: &str) -> Self {
            let random = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                    .unwrap();
            Self {
                pair,
                kid: kid.into(),
            }
        }

        pub(crate) fn jwks(&self) -> Jwks {
            let point = self.pair.public_key().as_ref();
            serde_json::from_value(json!({"keys": [{
                "kty": "EC", "crv": "P-256", "kid": self.kid, "use": "sig", "alg": "ES256",
                "x": BASE64URL.encode(&point[1..33]), "y": BASE64URL.encode(&point[33..]),
            }]}))
            .unwrap()
        }

        pub(crate) fn sign(&self, header: Value, claims: Value) -> String {
            let message = format!(
                "{}.{}",
                BASE64URL.encode(header.to_string()),
                BASE64URL.encode(claims.to_string())
            );
            let signature = self
                .pair
                .sign(&SystemRandom::new(), message.as_bytes())
                .unwrap();
            format!("{message}.{}", BASE64URL.encode(signature.as_ref()))
        }

        pub(crate) fn id_token(&self, claims: Value) -> String {
            self.sign(json!({"alg": "ES256", "kid": self.kid}), claims)
        }
    }

    pub(crate) fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn claims() -> Value {
        json!({
            "iss": "https://id.example.test/realms/qrow", "aud": "qrow-desktop",
            "azp": "qrow-desktop", "sub": "subject-1", "nonce": "n-1",
            "exp": now() + 300, "iat": now(), "name": "Ada", "email": "ada@example.test",
        })
    }

    fn expected() -> Expected<'static> {
        Expected {
            issuer: "https://id.example.test/realms/qrow",
            client_id: "qrow-desktop",
            nonce: Some("n-1"),
            subject: None,
            now: SystemTime::now(),
        }
    }

    #[test]
    fn accepts_a_valid_token_and_reads_the_identity() {
        let key = TestKey::new("k1");
        let claims = validate_id_token(&key.id_token(claims()), &key.jwks(), &expected()).unwrap();
        assert_eq!(
            claims,
            IdClaims {
                subject: "subject-1".into(),
                name: Some("Ada".into()),
                email: Some("ada@example.test".into()),
            }
        );
    }

    #[test]
    fn rejects_wrong_claims() {
        let key = TestKey::new("k1");
        let reject = |change: &dyn Fn(&mut Value), reason: &str| {
            let mut value = claims();
            change(&mut value);
            let error = validate_id_token(&key.id_token(value), &key.jwks(), &expected())
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{error} does not contain {reason}");
        };
        reject(
            &|c| c["iss"] = json!("https://evil.test"),
            "different issuer",
        );
        reject(&|c| c["aud"] = json!("other"), "not for this client");
        reject(
            &|c| {
                c["aud"] = json!(["qrow-desktop", "other"]);
                c.as_object_mut().unwrap().remove("azp");
            },
            "another party",
        );
        reject(&|c| c["azp"] = json!("other"), "another party");
        reject(&|c| c["exp"] = json!(now() - 120), "expired");
        reject(&|c| c["iat"] = json!(now() + 600), "in the future");
        reject(&|c| c["nonce"] = json!("n-2"), "this sign-in attempt");
        reject(
            &|c| {
                c.as_object_mut().unwrap().remove("nonce");
            },
            "this sign-in attempt",
        );
        reject(&|c| c["sub"] = json!(""), "no valid subject");
        reject(
            &|c| {
                c.as_object_mut().unwrap().remove("exp");
            },
            "no expiry",
        );
    }

    #[test]
    fn a_refreshed_token_must_keep_the_subject() {
        let key = TestKey::new("k1");
        let mut value = claims();
        value.as_object_mut().unwrap().remove("nonce");
        let token = key.id_token(value);
        let refreshed = Expected {
            nonce: None,
            subject: Some("subject-1"),
            ..expected()
        };
        assert!(validate_id_token(&token, &key.jwks(), &refreshed).is_ok());
        let other = Expected {
            subject: Some("subject-2"),
            ..refreshed
        };
        assert!(validate_id_token(&token, &key.jwks(), &other).is_err());
    }

    #[test]
    fn rejects_bad_signatures_algorithms_and_keys() {
        let key = TestKey::new("k1");
        let other = TestKey::new("k1");
        let token = key.id_token(claims());
        assert!(validate_id_token(&token, &other.jwks(), &expected()).is_err());
        let mut tampered: Vec<&str> = token.split('.').collect();
        let changed = BASE64URL.encode(
            claims()
                .to_string()
                .replace("subject-1", "subject-2")
                .as_bytes(),
        );
        tampered[1] = &changed;
        assert!(validate_id_token(&tampered.join("."), &key.jwks(), &expected()).is_err());
        let unsigned = format!(
            "{}.{}.",
            BASE64URL.encode(json!({"alg": "none"}).to_string()),
            BASE64URL.encode(claims().to_string())
        );
        assert!(validate_id_token(&unsigned, &key.jwks(), &expected()).is_err());
        let hmac = key.sign(json!({"alg": "HS256", "kid": "k1"}), claims());
        assert!(validate_id_token(&hmac, &key.jwks(), &expected()).is_err());
        let unknown_kid = key.sign(json!({"alg": "ES256", "kid": "k2"}), claims());
        assert!(validate_id_token(&unknown_kid, &key.jwks(), &expected()).is_err());
        let critical = key.sign(
            json!({"alg": "ES256", "kid": "k1", "crit": ["x"]}),
            claims(),
        );
        assert!(validate_id_token(&critical, &key.jwks(), &expected()).is_err());
        assert!(validate_id_token("a.b", &key.jwks(), &expected()).is_err());
    }

    #[test]
    fn errors_do_not_contain_the_token() {
        let key = TestKey::new("k1");
        let mut value = claims();
        value["iss"] = json!("https://evil.test");
        let token = key.id_token(value);
        let error = format!(
            "{:#}",
            validate_id_token(&token, &key.jwks(), &expected()).unwrap_err()
        );
        for part in token.split('.') {
            assert!(!error.contains(part));
        }
    }

    #[test]
    fn accepts_rsa_signatures_with_pkcs1_and_pss_padding() {
        use ring::signature::{RSA_PKCS1_SHA256, RSA_PSS_SHA256, RsaKeyPair};
        // A synthetic key for this test only.
        let pair = RsaKeyPair::from_der(include_bytes!("testdata/rsa-test-key.der")).unwrap();
        let public = RsaPublicKeyComponents::<Vec<u8>>::from(pair.public());
        let keys: Jwks = serde_json::from_value(json!({"keys": [
            {"kty": "EC", "kid": "other", "crv": "P-256", "x": "AA", "y": "AA"},
            {"kty": "RSA", "kid": "rsa", "n": BASE64URL.encode(&public.n), "e": BASE64URL.encode(&public.e)},
        ]}))
        .unwrap();
        for (alg, padding) in [
            (
                "RS256",
                &RSA_PKCS1_SHA256 as &dyn ring::signature::RsaEncoding,
            ),
            ("PS256", &RSA_PSS_SHA256),
        ] {
            let message = format!(
                "{}.{}",
                BASE64URL.encode(json!({"alg": alg, "kid": "rsa"}).to_string()),
                BASE64URL.encode(claims().to_string())
            );
            let mut signature = vec![0; pair.public().modulus_len()];
            pair.sign(
                padding,
                &SystemRandom::new(),
                message.as_bytes(),
                &mut signature,
            )
            .unwrap();
            let token = format!("{message}.{}", BASE64URL.encode(&signature));
            assert!(
                validate_id_token(&token, &keys, &expected()).is_ok(),
                "{alg}"
            );
            // The same signature does not verify with the other padding.
            let other = if alg == "RS256" { "PS256" } else { "RS256" };
            let swapped = token.replacen(
                &BASE64URL.encode(json!({"alg": alg, "kid": "rsa"}).to_string()),
                &BASE64URL.encode(json!({"alg": other, "kid": "rsa"}).to_string()),
                1,
            );
            assert!(validate_id_token(&swapped, &keys, &expected()).is_err());
        }
    }

    #[test]
    fn reads_the_key_id_of_a_header() {
        let key = TestKey::new("k9");
        assert_eq!(key_id(&key.id_token(claims())).as_deref(), Some("k9"));
        assert_eq!(key_id("not-a-token"), None);
    }
}

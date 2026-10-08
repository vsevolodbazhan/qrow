//! Browser navigation for the disposable Trino/confidential OIDC fixture.
#![allow(dead_code)]
use qrow::{
    model::{Authentication, DatabaseType, Profile},
    tls::Trust,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use url::Url;

pub fn configuration() -> (Profile, Trust) {
    assert_eq!(std::env::var("QROW_TRINO_OAUTH").unwrap(), "1");
    assert!(
        std::env::var("QROW_TRINO_FIXTURE")
            .unwrap()
            .starts_with("qrow-e2e-trino-")
    );
    let profile = Profile {
        name: "Trino browser".into(),
        database_type: DatabaseType::Trino,
        host: "localhost".into(),
        port: std::env::var("QROW_TRINO_PORT").unwrap().parse().unwrap(),
        username: "alice".into(),
        database: "tpch".into(),
        trino_schema: "tiny".into(),
        tls: true,
        authentication: Authentication::TrinoExternal,
        ..Profile::default()
    };
    let trust =
        Trust::from_pem(&std::fs::read(std::env::var("QROW_TRINO_CA").unwrap()).unwrap()).unwrap();
    (profile, trust)
}

pub fn refresh_grants() -> anyhow::Result<u64> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(
            std::env::var("QROW_TRINO_CA")?,
        )?)?)
        .build()?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let response = client
                .get(format!(
                    "https://localhost:{}/fixture/stats",
                    std::env::var("QROW_TRINO_OIDC_PORT")?
                ))
                .send()
                .await
                .map_err(|error| error.without_url())?
                .error_for_status()
                .map_err(|error| error.without_url())?;
            let stats: serde_json::Value = serde_json::from_slice(&response.bytes().await?)?;
            stats["refresh_token"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Missing fixture refresh grant count"))
        })
}
/// A synthetic browser with a cookie jar and verified TLS. Only the fixture
/// coordinator and its provider are reachable. The application HTTP client
/// retains its separate, disabled-redirect policy.
pub fn browser(_trust: Trust, opens: Arc<AtomicUsize>) -> qrow::external_auth::Browser {
    let coordinator = Url::parse(&format!(
        "https://localhost:{}",
        std::env::var("QROW_TRINO_PORT").unwrap()
    ))
    .unwrap();
    let provider = Url::parse(&format!(
        "https://localhost:{}",
        std::env::var("QROW_TRINO_OIDC_PORT").unwrap()
    ))
    .unwrap();
    Arc::new(move |text| {
        opens.fetch_add(1, Ordering::SeqCst);
        let mut url = Url::parse(text)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(
                std::env::var("QROW_TRINO_CA").unwrap(),
            )?)?)
            .build()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let mut cookies = Vec::new();
                for _ in 0..8 {
                    anyhow::ensure!(
                        url.origin() == coordinator.origin() || url.origin() == provider.origin(),
                        "Unexpected fixture browser origin"
                    );
                    if url.origin() == provider.origin() {
                        url.query_pairs_mut()
                            .append_pair("fixture_user", "alice")
                            .append_pair("fixture_access_ttl", "3");
                    }
                    let mut request = client.get(url.clone());
                    if url.origin() == coordinator.origin() && !cookies.is_empty() {
                        request = request.header("Cookie", cookies.join("; "));
                    }
                    let response = request.send().await.map_err(|error| error.without_url())?;
                    if url.origin() == coordinator.origin() {
                        for cookie in response.headers().get_all("set-cookie") {
                            cookies.push(cookie.to_str()?.split(';').next().unwrap().to_owned());
                        }
                    }
                    if response.status().is_redirection() {
                        url = url.join(
                            response
                                .headers()
                                .get("location")
                                .ok_or_else(|| anyhow::anyhow!("Fixture redirect has no location"))?
                                .to_str()?,
                        )?;
                    } else {
                        anyhow::ensure!(
                            response.status().is_success(),
                            "Fixture browser returned HTTP {}",
                            response.status().as_u16()
                        );
                        return Ok::<_, anyhow::Error>(());
                    }
                }
                anyhow::bail!("Too many fixture browser redirects")
            })
    })
}

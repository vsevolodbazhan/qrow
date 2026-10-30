//! One bounded, credential-free attempt. Retry and ACK policy belong to the reader.
use super::spooling::{Encoding, MAX_SEGMENT, Source, Spec};
use super::{
    raw,
    transport::{Runtime, Transport},
};
use crate::{export::budget, tls::Trust};
use anyhow::{Result, ensure};
use base64::Engine;
use reqwest::{
    Client,
    header::{HeaderMap, LOCATION},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Control {
    pub scope: Arc<Transport>,
    pub requested: Arc<AtomicBool>,
    pub external: Option<Arc<AtomicBool>>,
    pub allowance: Arc<budget::Allowance>,
    pub trust: Trust,
    pub timeout: Duration,
    pub threads: Arc<super::segment_threads::Threads>,
}
impl Control {
    pub fn check(&self) -> Result<()> {
        ensure!(
            !self.scope.is_closed()
                && !self.requested.load(Ordering::SeqCst)
                && !self
                    .external
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst)),
            crate::export::Cancelled
        );
        Ok(())
    }
    pub fn delay(&self, duration: Duration, deadline: Option<Instant>) -> Result<()> {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            self.check()?;
            ensure!(
                deadline.is_none_or(|d| Instant::now() < d),
                "Trino segment deadline expired"
            );
            std::thread::sleep(
                end.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            );
        }
        self.check()
    }
    fn client(&self) -> Result<Client> {
        let scope = self.scope.clone();
        let allowance = self.allowance.clone();
        Ok(Client::builder()
            .tcp_connection_control(move |socket| {
                let lease = scope.register(socket)?;
                Ok(Arc::new((lease, allowance.clone())) as Arc<dyn Send + Sync>)
            })
            .http1_max_buf_size(1024 * 1024)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .use_preconfigured_tls((*self.trust.client_config()?).clone())
            .timeout(self.timeout)
            .connect_timeout(self.timeout)
            .build()?)
    }
    pub fn attempt(&self, spec: &Spec) -> std::result::Result<Decoded, Failure> {
        self.check().map_err(Failure::terminal)?;
        spec.check_expiry().map_err(Failure::terminal)?;
        let wire = match &spec.source {
            Source::Inline(text) => {
                let mut bytes = vec![0; spec.wire_size];
                let size = base64::engine::general_purpose::STANDARD
                    .decode_slice(text, &mut bytes)
                    .map_err(|_| {
                        Failure::terminal(anyhow::anyhow!("Invalid inline Trino segment"))
                    })?;
                if size != spec.wire_size {
                    return Err(Failure::terminal(anyhow::anyhow!(
                        "Trino segment size mismatch"
                    )));
                }
                bytes
            }
            Source::Remote(url) => self.get(
                url.clone(),
                spec.headers.clone(),
                Some(spec.wire_size),
                MAX_SEGMENT,
                spec,
                Instant::now() + self.timeout,
            )?,
        };
        self.check().map_err(Failure::terminal)?;
        let bytes = if spec.encoding == Encoding::Lz4
            && let Some(size) = spec.decoded_size
        {
            let mut output = vec![0; size];
            let written = lz4_flex::block::decompress_into(&wire, &mut output)
                .map_err(|_| Failure::terminal(anyhow::anyhow!("Invalid Trino LZ4 segment")))?;
            if written != size {
                return Err(Failure::terminal(anyhow::anyhow!(
                    "Trino decompressed size mismatch"
                )));
            }
            output
        } else {
            wire
        };
        self.check().map_err(Failure::terminal)?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| Failure::terminal(anyhow::anyhow!("Invalid Trino segment JSON")))?;
        let rows = raw::Rows::new(value).map_err(Failure::terminal)?;
        Ok(Decoded {
            rows,
            _allowance: self.allowance.clone(),
        })
    }
    fn get(
        &self,
        mut url: url::Url,
        mut headers: HeaderMap,
        exact: Option<usize>,
        max: usize,
        spec: &Spec,
        deadline: Instant,
    ) -> std::result::Result<Vec<u8>, Failure> {
        let runtime = Runtime::new().map_err(Failure::terminal)?;
        let client = self.client().map_err(Failure::terminal)?;
        runtime
            .block_on(self.scope.run(async {
                let result = async {
                    for redirect in 0..=2 {
                        self.check().map_err(Failure::terminal)?;
                        spec.check_expiry().map_err(Failure::terminal)?;
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Err(Failure::retry("Trino segment request timed out"));
                        }
                        let mut response = client
                            .get(url.clone())
                            .headers(headers.clone())
                            .header("Accept-Encoding", "identity")
                            .timeout(remaining)
                            .send()
                            .await
                            .map_err(|_| Failure::retry("Trino segment request failed"))?;
                        let status = response.status();
                        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                            if redirect == 2 {
                                return Err(Failure::terminal(anyhow::anyhow!(
                                    "Trino segment exceeded its redirect limit"
                                )));
                            }
                            let location = response
                                .headers()
                                .get(LOCATION)
                                .and_then(|v| v.to_str().ok())
                                .ok_or_else(|| {
                                    Failure::terminal(anyhow::anyhow!(
                                        "Invalid Trino segment redirect"
                                    ))
                                })?;
                            if location.len() > 16384 {
                                return Err(Failure::terminal(anyhow::anyhow!(
                                    "Trino segment redirect exceeds its size limit"
                                )));
                            }
                            let next = url.join(location).map_err(|_| {
                                Failure::terminal(anyhow::anyhow!("Invalid Trino segment redirect"))
                            })?;
                            let next = super::spooling::resource(next.as_str())
                                .map_err(Failure::terminal)?;
                            if url.origin() != next.origin() {
                                headers.clear();
                            }
                            url = next;
                            continue;
                        }
                        if !status.is_success() {
                            return Err(Failure {
                                retryable: matches!(
                                    status.as_u16(),
                                    408 | 429 | 500 | 502 | 503 | 504
                                ),
                                error: anyhow::anyhow!(
                                    "Trino segment returned HTTP {}",
                                    status.as_u16()
                                ),
                            });
                        }
                        if let Some(length) = response.content_length()
                            && (length > max as u64 || exact.is_some_and(|n| n as u64 != length))
                        {
                            return Err(Failure::terminal(anyhow::anyhow!(
                                "Trino segment size mismatch"
                            )));
                        }
                        if response
                            .headers()
                            .get("content-encoding")
                            .is_some_and(|h| h.as_bytes() != b"identity")
                        {
                            return Err(Failure::terminal(anyhow::anyhow!(
                                "Unexpected Trino segment content encoding"
                            )));
                        }
                        let mut bytes = Vec::with_capacity(exact.unwrap_or(max));
                        loop {
                            self.check().map_err(Failure::terminal)?;
                            let chunk = tokio::time::timeout_at(
                                tokio::time::Instant::from_std(deadline),
                                response.chunk(),
                            )
                            .await
                            .map_err(|_| Failure::retry("Trino segment body timed out"))?
                            .map_err(|_| Failure::retry("Trino segment body failed"))?;
                            let Some(chunk) = chunk else { break };
                            if chunk.len() > max.saturating_sub(bytes.len())
                                || exact
                                    .is_some_and(|n| chunk.len() > n.saturating_sub(bytes.len()))
                            {
                                return Err(Failure::terminal(anyhow::anyhow!(
                                    "Trino segment exceeds its declared size"
                                )));
                            }
                            bytes.extend_from_slice(&chunk);
                        }
                        if exact.is_some_and(|n| n != bytes.len()) {
                            return Err(Failure::retry("Trino segment body ended early"));
                        }
                        return Ok(bytes);
                    }
                    unreachable!()
                }
                .await;
                // Transport::run carries only cancellation errors. Preserve attempt classification separately.
                Ok(result)
            }))
            .map_err(Failure::terminal)?
    }
    pub fn ack(&self, spec: &Spec) -> Result<bool> {
        self.check()?;
        let Some(url) = &spec.ack else {
            return Ok(true);
        };
        let _activity = self.scope.activity()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        for attempt in 0..3 {
            self.check()?;
            if attempt > 0
                && self
                    .delay(
                        Duration::from_millis(if attempt == 1 { 100 } else { 250 }),
                        Some(deadline),
                    )
                    .is_err()
            {
                self.check()?;
                return Ok(false);
            }
            match self.get(
                url.clone(),
                spec.headers.clone(),
                None,
                8192,
                spec,
                deadline,
            ) {
                Ok(_) => {
                    self.check()?;
                    return Ok(true);
                }
                Err(failure) => {
                    self.check()?;
                    if !failure.retryable || Instant::now() >= deadline {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(false)
    }
}
pub(super) struct Decoded {
    pub rows: raw::Rows,
    _allowance: Arc<budget::Allowance>,
}
pub(super) struct Failure {
    pub error: anyhow::Error,
    pub retryable: bool,
}
impl Failure {
    fn terminal(error: anyhow::Error) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
    fn retry(message: &str) -> Self {
        Self {
            error: anyhow::anyhow!("{}", message),
            retryable: true,
        }
    }
}

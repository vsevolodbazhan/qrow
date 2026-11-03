//! Bounded Trino 483 segment metadata.
use super::raw;
use anyhow::{Result, ensure};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{
    Deserialize,
    de::{self, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{borrow::Cow, fmt, ops::Range, sync::Arc};
use url::Url;

pub(super) const MAX_SEGMENT: usize = 32 * 1024 * 1024;
const MAX_SEGMENTS: usize = 1024;
const MAX_METADATA: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Encoding {
    Json,
    Lz4,
}
impl Encoding {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "json+lz4" => Ok(Self::Lz4),
            _ => anyhow::bail!("Unsupported Trino segment encoding"),
        }
    }
}
#[derive(Deserialize)]
struct Encoded<'a> {
    #[serde(borrow)]
    encoding: Cow<'a, str>,
    #[serde(borrow)]
    segments: &'a RawValue,
}

pub(super) struct Page {
    raw: Arc<RawValue>,
    ranges: std::collections::VecDeque<Range<usize>>,
    pub encoding: Encoding,
}
impl Page {
    pub fn new(raw: Box<RawValue>) -> Result<Self> {
        ensure!(
            raw.get().len() <= 16 * 1024 * 1024,
            "Trino encoded page exceeds 16 MiB"
        );
        let parsed: Encoded<'_> = serde_json::from_str(raw.get())
            .map_err(|_| anyhow::anyhow!("Invalid Trino encoded page"))?;
        let encoding = Encoding::parse(&parsed.encoding)?;
        let ranges = ranges(parsed.segments, raw.get().as_ptr() as usize)?;
        let mut metadata = 0usize;
        for range in &ranges {
            let spec = Spec::parse(&raw.get()[range.clone()], encoding)?;
            metadata = metadata
                .checked_add(spec.metadata_bytes())
                .ok_or_else(|| anyhow::anyhow!("Trino segment metadata exceeds 2 MiB"))?;
            ensure!(
                metadata <= MAX_METADATA,
                "Trino segment metadata exceeds 2 MiB"
            );
        }
        Ok(Self {
            raw: Arc::from(raw),
            ranges: ranges.into(),
            encoding,
        })
    }
    pub fn next(&mut self) -> Result<Option<Spec>> {
        self.ranges
            .pop_front()
            .map(|range| Spec::parse(&self.raw.get()[range], self.encoding))
            .transpose()
    }
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}
fn ranges(raw: &RawValue, base: usize) -> Result<Vec<Range<usize>>> {
    struct Ranges(usize);
    impl<'de> Visitor<'de> for Ranges {
        type Value = Vec<Range<usize>>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("bounded Trino segments")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut ranges = Vec::with_capacity(MAX_SEGMENTS);
            while let Some(raw) = seq.next_element::<&RawValue>()? {
                if ranges.len() == MAX_SEGMENTS {
                    return Err(de::Error::custom("Trino page exceeds 1024 segments"));
                }
                let offset = (raw.get().as_ptr() as usize)
                    .checked_sub(self.0)
                    .ok_or_else(|| de::Error::custom("Invalid Trino segment range"))?;
                ranges.push(offset..offset + raw.get().len());
            }
            Ok(ranges)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let ranges = de::Deserializer::deserialize_seq(&mut deserializer, Ranges(base))?;
    deserializer.end()?;
    Ok(ranges)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attributes {
    row_offset: Integer,
    rows_count: Integer,
    segment_size: Integer,
    uncompressed_size: Option<Integer>,
    #[serde(default, deserialize_with = "raw::optional_text::<_, 128>")]
    expires_at: Option<String>,
}
struct Integer(u64);
impl<'de> Deserialize<'de> for Integer {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Number;
        impl Visitor<'_> for Number {
            type Value = Integer;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("nonnegative integer or integer string")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Integer, E> {
                Ok(Integer(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Integer, E> {
                u64::try_from(v)
                    .map(Integer)
                    .map_err(|_| E::custom("Invalid Trino segment number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Integer, E> {
                if v.is_empty() || v.len() > 20 || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(E::custom("Invalid Trino segment number"));
                }
                v.parse()
                    .map(Integer)
                    .map_err(|_| E::custom("Invalid Trino segment number"))
            }
        }
        d.deserialize_any(Number)
    }
}
#[derive(Deserialize)]
struct Descriptor<'a> {
    #[serde(rename = "type", deserialize_with = "raw::text::<_, 16>")]
    kind: String,
    metadata: Attributes,
    #[serde(borrow)]
    data: Option<Cow<'a, str>>,
    #[serde(default, deserialize_with = "raw::optional_text::<_, 16384>")]
    uri: Option<String>,
    #[serde(
        rename = "ackUri",
        default,
        deserialize_with = "raw::optional_text::<_, 16384>"
    )]
    ack_uri: Option<String>,
    #[serde(borrow)]
    headers: Option<&'a RawValue>,
}
pub(super) enum Source {
    Inline(String),
    Remote(Url),
}
pub(super) struct Spec {
    pub offset: u64,
    pub rows: u64,
    pub wire_size: usize,
    pub decoded_size: Option<usize>,
    pub expiry: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub source: Source,
    pub ack: Option<Url>,
    pub headers: HeaderMap,
    pub encoding: Encoding,
}
impl Spec {
    fn parse(raw: &str, encoding: Encoding) -> Result<Self> {
        let d: Descriptor<'_> = serde_json::from_str(raw)
            .map_err(|_| anyhow::anyhow!("Invalid Trino segment metadata"))?;
        let bounded_size = |n: u64| -> Result<usize> {
            ensure!(n <= MAX_SEGMENT as u64, "Trino segment exceeds 32 MiB");
            Ok(n as usize)
        };
        let wire_size = bounded_size(d.metadata.segment_size.0)?;
        let decoded_size = d
            .metadata
            .uncompressed_size
            .map(|n| bounded_size(n.0))
            .transpose()?;
        ensure!(
            encoding == Encoding::Lz4 || decoded_size.is_none(),
            "Unexpected compressed Trino JSON segment"
        );
        let expiry = d
            .metadata
            .expires_at
            .as_deref()
            .map(parse_expiry)
            .transpose()
            .map_err(|_| anyhow::anyhow!("Invalid Trino segment expiry"))?
            .flatten();
        let headers = d
            .headers
            .map(parse_headers)
            .transpose()?
            .unwrap_or_default();
        let (source, ack) = match d.kind.as_str() {
            "inline" => {
                ensure!(
                    d.uri.is_none() && d.ack_uri.is_none() && headers.is_empty(),
                    "Invalid inline Trino segment"
                );
                (
                    Source::Inline(
                        d.data
                            .ok_or_else(|| anyhow::anyhow!("Missing inline Trino data"))?
                            .into_owned(),
                    ),
                    None,
                )
            }
            "spooled" => {
                ensure!(
                    d.data.is_none(),
                    "Unexpected inline data on spooled Trino segment"
                );
                (
                    Source::Remote(resource(
                        d.uri
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("Missing Trino segment URI"))?,
                    )?),
                    Some(resource(
                        d.ack_uri
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("Missing Trino ACK URI"))?,
                    )?),
                )
            }
            _ => anyhow::bail!("Unknown Trino segment type"),
        };
        Ok(Self {
            offset: d.metadata.row_offset.0,
            rows: d.metadata.rows_count.0,
            wire_size,
            decoded_size,
            expiry,
            source,
            ack,
            headers,
            encoding,
        })
    }
    fn metadata_bytes(&self) -> usize {
        let uri = match &self.source {
            Source::Remote(url) => 2 * url.as_str().len(),
            Source::Inline(_) => 0,
        };
        uri + self.ack.as_ref().map_or(0, |url| 2 * url.as_str().len())
            + self.headers.capacity() * 256
            + self
                .headers
                .iter()
                .map(|(k, v)| 2 * (k.as_str().len() + v.as_bytes().len()) + 256)
                .sum::<usize>()
            + 512
    }
    pub fn check_expiry(&self) -> Result<()> {
        ensure!(
            self.expiry.is_none_or(|expiry| chrono::Utc::now() < expiry),
            "Trino segment URI expired"
        );
        Ok(())
    }
}

fn parse_expiry(value: &str) -> Result<Option<chrono::DateTime<chrono::FixedOffset>>> {
    if let Ok(absolute) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(Some(absolute));
    }
    // Trino 483 sends LocalDateTime in the effective server session zone,
    // which it does not identify here. Validate it without guessing an offset.
    // The coordinator/storage expiration response remains authoritative.
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M"))?;
    Ok(None)
}
pub(super) fn resource(value: &str) -> Result<Url> {
    ensure!(
        value.len() <= 16384,
        "Trino segment URI exceeds its size limit"
    );
    let url = Url::parse(value).map_err(|_| anyhow::anyhow!("Invalid Trino segment URI"))?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "Trino segments require HTTPS without user information or fragments"
    );
    Ok(url)
}
fn parse_headers(raw: &RawValue) -> Result<HeaderMap> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Text(#[serde(deserialize_with = "raw::text::<_, 32768>")] String);
    struct Values<'a> {
        name: HeaderName,
        key_bytes: usize,
        headers: &'a mut HeaderMap,
        bytes: &'a mut usize,
    }
    impl<'de> de::DeserializeSeed<'de> for Values<'_> {
        type Value = ();
        fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
            d.deserialize_seq(self)
        }
    }
    impl<'de> Visitor<'de> for Values<'_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("bounded header values")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
            loop {
                if self.headers.len() == 32 {
                    if seq.next_element::<de::IgnoredAny>()?.is_some() {
                        return Err(de::Error::custom("Trino segment has too many headers"));
                    }
                    break;
                }
                let Some(Text(value)) = seq.next_element::<Text>()? else {
                    break;
                };
                *self.bytes = self.bytes.saturating_add(self.key_bytes + value.capacity());
                if *self.bytes > 32768 {
                    return Err(de::Error::custom("Trino segment headers exceed 32 KiB"));
                }
                let mut value = HeaderValue::from_str(&value)
                    .map_err(|_| de::Error::custom("Invalid Trino segment header"))?;
                value.set_sensitive(true);
                self.headers.append(self.name.clone(), value);
            }
            Ok(())
        }
    }
    struct Headers;
    impl<'de> Visitor<'de> for Headers {
        type Value = HeaderMap;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("bounded storage headers")
        }
        fn visit_map<A: de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<HeaderMap, A::Error> {
            let mut headers = HeaderMap::with_capacity(32);
            let mut bytes = 0usize;
            let mut entries = 0usize;
            while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                entries += 1;
                if entries > 32 || key.len() > 256 || headers.len() >= 32 {
                    return Err(de::Error::custom(
                        "Trino segment headers exceed their size limit",
                    ));
                }
                let key = key.to_ascii_lowercase();
                if key.starts_with("x-trino-")
                    || matches!(
                        key.as_str(),
                        "authorization"
                            | "proxy-authorization"
                            | "proxy-authenticate"
                            | "cookie"
                            | "cookie2"
                            | "connection"
                            | "keep-alive"
                            | "te"
                            | "trailer"
                            | "transfer-encoding"
                            | "upgrade"
                            | "host"
                            | "content-length"
                            | "accept-encoding"
                            | "x-amz-security-token"
                    )
                {
                    return Err(de::Error::custom(
                        "Trino segment supplied a forbidden header",
                    ));
                }
                let name = HeaderName::from_bytes(key.as_bytes())
                    .map_err(|_| de::Error::custom("Invalid Trino segment header"))?;
                map.next_value_seed(Values {
                    name,
                    key_bytes: key.len(),
                    headers: &mut headers,
                    bytes: &mut bytes,
                })?;
            }
            Ok(headers)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let headers = de::Deserializer::deserialize_map(&mut deserializer, Headers)
        .map_err(|_| anyhow::anyhow!("Invalid Trino segment headers"))?;
    deserializer
        .end()
        .map_err(|_| anyhow::anyhow!("Invalid Trino segment headers"))?;
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn page(segments: serde_json::Value) -> Result<Page> {
        Page::new(serde_json::value::to_raw_value(
            &json!({"encoding":"json","segments":segments}),
        )?)
    }
    fn segment() -> serde_json::Value {
        json!({"type":"spooled","uri":"https://storage.example/data?signature=secret","ackUri":"https://storage.example/ack?signature=secret","metadata":{"rowOffset":"0","rowsCount":"1","segmentSize":"5"}})
    }
    #[test]
    fn expiry_accepts_trino_local_dates_without_assuming_a_server_zone() -> Result<()> {
        for value in [
            "2099-01-01T00:00",
            "2099-01-01T00:00:01",
            "2099-01-01T00:00:01.123456789",
        ] {
            assert!(parse_expiry(value)?.is_none());
        }
        let absolute = parse_expiry("2099-01-01T07:00:00+07:00")?.unwrap();
        assert_eq!(absolute, parse_expiry("2099-01-01T00:00:00Z")?.unwrap());
        for value in [
            "secret",
            "2099-01-01",
            "2099-02-30T00:00",
            "2099-01-01T00:00garbage",
        ] {
            assert!(parse_expiry(value).is_err());
        }
        Ok(())
    }
    #[test]
    fn metadata_bounds_reject_credentials_unsafe_urls_and_unbounded_collections() {
        for name in [
            "Authorization",
            "Proxy-Authorization",
            "Cookie",
            "X-Trino-Session",
            "Connection",
            "Content-Length",
            "X-Amz-Security-Token",
        ] {
            let mut s = segment();
            s["headers"] = json!({name:["synthetic-secret"]});
            let error = page(json!([s])).err().unwrap().to_string();
            assert!(!error.contains("synthetic-secret") && !error.contains("signature"));
        }
        for uri in [
            "http://storage.example/data",
            "https://user:password@storage.example/data",
            "https://storage.example/data#fragment",
        ] {
            let mut s = segment();
            s["uri"] = json!(uri);
            assert!(page(json!([s])).is_err());
        }
        let mut s = segment();
        s["headers"] = json!(
            (0..33)
                .map(|n| (format!("x-empty-{n}"), json!([])))
                .collect::<serde_json::Map<_, _>>()
        );
        assert!(page(json!([s])).is_err());
        let mut s = segment();
        s["headers"] = json!({"x-empty":[]});
        let error = page(json!(vec![s; MAX_SEGMENTS])).err().unwrap();
        assert!(error.to_string().contains("metadata exceeds"));
        assert!(page(json!(vec![segment(); 1025])).is_err());
        for field in ["rowOffset", "rowsCount", "segmentSize"] {
            for invalid in [
                json!(-1),
                json!(1.5),
                json!("-1"),
                json!("18446744073709551616"),
            ] {
                let mut s = segment();
                s["metadata"][field] = invalid;
                assert!(page(json!([s])).is_err());
            }
        }
    }
    #[test]
    fn escaped_storage_header_is_bounded_before_transport_and_redacted() -> Result<()> {
        let escaped = "\\u0078".repeat(64 * 1024);
        let raw =
            serde_json::value::RawValue::from_string(format!("{{\"x-storage\":[\"{escaped}\"]}}"))?;
        let error = parse_headers(&raw).unwrap_err().to_string();
        assert_eq!(error, "Invalid Trino segment headers");
        Ok(())
    }
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use tokio::sync::Mutex;

use super::http_client::{FetchFailure, ProviderHttpClient};
use super::model::ProviderId;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;

/// At most one JWKS refetch per provider inside this window on a key miss.
pub const JWKS_REFRESH_INTERVAL_SECS: i64 = 60;

/// Defensive bounds so the in-memory key cache used by RK-22 stays bounded
/// even if an administrator configures an unusual number of providers.
const MAX_CACHED_PROVIDERS: usize = 64;
const MAX_KEYS_PER_PROVIDER: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwksError {
    /// The configured `jwks_uri` could not be fetched over verified TLS.
    Fetch(FetchFailure),
    /// The document was not a JSON Web Key Set.
    Malformed,
    /// The document parsed but published no keys.
    Empty,
    /// No usable key carries the token's `kid`, including after one refresh.
    KeyNotFound,
}

impl JwksError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Fetch(failure) => failure.code(),
            Self::Malformed => "malformed_jwks",
            Self::Empty => "empty_jwks",
            Self::KeyNotFound => "key_not_found",
        }
    }

    /// The canonical external error code. A key miss is a token failure; a
    /// fetch, malformed or empty document is a discovery failure.
    pub const fn api_code(&self) -> ErrorCode {
        match self {
            Self::KeyNotFound => ErrorCode::ProviderIdTokenInvalid,
            Self::Fetch(_) | Self::Malformed | Self::Empty => ErrorCode::ProviderDiscoveryFailed,
        }
    }
}

impl std::fmt::Display for JwksError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fetch(failure) => write!(f, "JWKS fetch failed: {}", failure.code()),
            Self::Malformed => f.write_str("the JWKS document is malformed"),
            Self::Empty => f.write_str("the JWKS document published no keys"),
            Self::KeyNotFound => f.write_str("no signing key matches the token key id"),
        }
    }
}

impl std::error::Error for JwksError {}

/// A cloned handle to a process-wide, per-provider JWKS cache.
///
/// The cache is in memory only, never persisted, and scoped by provider. A
/// miss for the token's `kid` may trigger at most one refetch per provider per
/// [`JWKS_REFRESH_INTERVAL_SECS`]; the per-provider asynchronous lock is the
/// single-flight mechanism, so concurrent misses collapse into one network
/// call without holding any global lock across I/O.
#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<Inner>,
}

struct Inner {
    clock: Arc<dyn Clock>,
    client: ProviderHttpClient,
    entries: RwLock<HashMap<ProviderId, Arc<Slot>>>,
}

struct Slot {
    keys: Mutex<ProviderKeys>,
    last_used: AtomicI64,
}

struct ProviderKeys {
    uri: Option<String>,
    set: Option<Arc<JwkSet>>,
    last_attempt: Option<i64>,
}

impl JwksCache {
    pub fn new(clock: Arc<dyn Clock>, client: ProviderHttpClient) -> Self {
        Self {
            inner: Arc::new(Inner {
                clock,
                client,
                entries: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Returns the key identified by `kid`, refetching the provider's JWKS at
    /// most once per refresh window when the key is absent.
    pub async fn signing_key(
        &self,
        provider: ProviderId,
        jwks_uri: &str,
        kid: &str,
    ) -> Result<Jwk, JwksError> {
        let slot = self.slot(provider);
        let now = self.inner.clock.now().unix_timestamp();
        let mut keys = slot.keys.lock().await;

        if keys.uri.as_deref() != Some(jwks_uri) {
            keys.uri = Some(jwks_uri.to_owned());
            keys.set = None;
            keys.last_attempt = None;
        }
        if let Some(found) = keys.set.as_ref().and_then(|set| set.find(kid)) {
            return Ok(found.clone());
        }

        let refresh_allowed = keys
            .last_attempt
            .is_none_or(|attempt| now - attempt >= JWKS_REFRESH_INTERVAL_SECS);
        if !refresh_allowed {
            return Err(JwksError::KeyNotFound);
        }

        // Record the attempt before dialling, so a failed refresh cannot be
        // retried in a loop and become an outbound-request amplifier.
        keys.last_attempt = Some(now);
        let fetched = self.fetch(jwks_uri).await?;
        keys.set = Some(Arc::new(fetched));
        keys.set
            .as_ref()
            .and_then(|set| set.find(kid))
            .cloned()
            .ok_or(JwksError::KeyNotFound)
    }

    async fn fetch(&self, jwks_uri: &str) -> Result<JwkSet, JwksError> {
        let document = self
            .inner
            .client
            .document(jwks_uri)
            .await
            .map_err(JwksError::Fetch)?;
        parse(document.body.as_ref())
    }

    fn slot(&self, provider: ProviderId) -> Arc<Slot> {
        let now = self.inner.clock.now().unix_timestamp();
        let mut entries = self
            .inner
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = entries.get(&provider) {
            existing.last_used.store(now, Ordering::Relaxed);
            return Arc::clone(existing);
        }
        if entries.len() >= MAX_CACHED_PROVIDERS {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, slot)| slot.last_used.load(Ordering::Relaxed))
                .map(|(id, _)| *id)
            {
                entries.remove(&oldest);
            }
        }
        let slot = Arc::new(Slot {
            keys: Mutex::new(ProviderKeys {
                uri: None,
                set: None,
                last_attempt: None,
            }),
            last_used: AtomicI64::new(now),
        });
        entries.insert(provider, Arc::clone(&slot));
        slot
    }
}

pub fn parse(body: &[u8]) -> Result<JwkSet, JwksError> {
    let mut set: JwkSet = serde_json::from_slice(body).map_err(|_| JwksError::Malformed)?;
    if set.keys.is_empty() {
        return Err(JwksError::Empty);
    }
    set.keys.truncate(MAX_KEYS_PER_PROVIDER);
    Ok(set)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn unit_jwks_parse_rejects_malformed_and_empty_documents() {
        for body in [
            &b""[..],
            b"null",
            b"[]",
            b"{",
            b"{\"keys\":\"nope\"}",
            b"\"text\"",
        ] {
            assert_eq!(parse(body), Err(JwksError::Malformed), "{body:?}");
        }
        assert_eq!(parse(br#"{"keys":[]}"#), Err(JwksError::Empty));
        let one = serde_json::to_vec(&json!({"keys":[{
            "kty":"RSA","kid":"a","n":"AQAB","e":"AQAB"
        }]}))
        .unwrap();
        assert_eq!(parse(&one).unwrap().keys.len(), 1);
    }

    #[test]
    fn unit_jwks_parse_caps_the_key_set() {
        let keys: Vec<serde_json::Value> = (0..(MAX_KEYS_PER_PROVIDER + 10))
            .map(|index| json!({"kty":"RSA","kid":format!("k{index}"),"n":"AQAB","e":"AQAB"}))
            .collect();
        let body = serde_json::to_vec(&json!({ "keys": keys })).unwrap();
        assert_eq!(parse(&body).unwrap().keys.len(), MAX_KEYS_PER_PROVIDER);
    }

    #[test]
    fn unit_jwks_error_classification_is_stable() {
        assert_eq!(
            JwksError::KeyNotFound.api_code(),
            ErrorCode::ProviderIdTokenInvalid
        );
        assert_eq!(
            JwksError::Malformed.api_code(),
            ErrorCode::ProviderDiscoveryFailed
        );
        assert_eq!(
            JwksError::Fetch(FetchFailure::Timeout).api_code(),
            ErrorCode::ProviderDiscoveryFailed
        );
        assert_eq!(JwksError::Empty.code(), "empty_jwks");
    }
}

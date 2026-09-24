use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRequestParts, MatchedPath};
use axum::response::{IntoResponse, Response};
use http::header::{CONTENT_TYPE, LOCATION, RETRY_AFTER, SET_COOKIE};
use http::request::Parts;
use http::{HeaderName, HeaderValue, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use super::error::{ApiError, JSON_CONTENT_TYPE};
use super::pagination::invalid_param;
use super::request_id::{tag_error, RequestId};
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::id::Id;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hash::{mac_hex, sha256_hex, TokenDigest};
use crate::infra::crypto::hkdf::{KeyRing, MacPurpose, SealPurpose};
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, DbPools, WriteTx};

pub const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");
pub const IDEMPOTENCY_REPLAYED: HeaderName = HeaderName::from_static("idempotency-replayed");
pub const MIN_KEY_CHARS: usize = 16;
pub const MAX_KEY_CHARS: usize = 128;
pub const REPLAY_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
pub const MAX_ENVELOPE_BYTES: usize = 16 * 1024;

const KEY_FIELD: &str = "Idempotency-Key";
const REPLAYED: HeaderValue = HeaderValue::from_static("true");
const IN_PROGRESS_RETRY_AFTER: HeaderValue = HeaderValue::from_static("1");
const AAD_LABEL: &str = "idempotency";

const SELECT_RECORD: &str = "SELECT id, request_hash, state, lease_expires_at, response_status,
        response_json, response_ciphertext, response_nonce, key_version, expires_at
   FROM idempotency_records
  WHERE scope_kind = ?1 AND scope_id = ?2 AND http_method = ?3
    AND route_template = ?4 AND key_hash = ?5";
const INSERT_RECORD: &str = "INSERT INTO idempotency_records
        (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash,
         state, lease_expires_at, created_at, expires_at)
 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'in_progress', ?8, ?9, ?10)";
const DELETE_EXPIRED_RECORD: &str = "DELETE FROM idempotency_records WHERE id = ?1";
const TAKE_OVER_RECORD: &str = "UPDATE idempotency_records SET lease_expires_at = ?1
  WHERE id = ?2 AND state = 'in_progress' AND lease_expires_at = ?3";
const COMPLETE_RECORD: &str = "UPDATE idempotency_records
    SET state = 'completed', lease_expires_at = NULL, completed_at = ?1, response_status = ?2,
        response_json = ?3, response_ciphertext = ?4, response_nonce = ?5, key_version = ?6
  WHERE id = ?7 AND state = 'in_progress' AND lease_expires_at = ?8";
const RELEASE_RECORD: &str = "DELETE FROM idempotency_records
  WHERE id = ?1 AND state = 'in_progress' AND lease_expires_at = ?2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyMode {
    None,
    Plaintext,
    Sealed,
}

impl IdempotencyMode {
    pub const fn is_declared(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Plaintext => "plaintext",
            Self::Sealed => "sealed",
        }
    }

    const fn storage(self) -> Option<ReplayStorage> {
        match self {
            Self::None => None,
            Self::Plaintext => Some(ReplayStorage::Plaintext),
            Self::Sealed => Some(ReplayStorage::Sealed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedRoute {
    pub method: Method,
    pub path: &'static str,
    pub mode: IdempotencyMode,
}

const fn supported(path: &'static str, mode: IdempotencyMode) -> SupportedRoute {
    SupportedRoute {
        method: Method::POST,
        path,
        mode,
    }
}

pub const SUPPORTED_ROUTES: [SupportedRoute; 12] = [
    supported("/api/v1/transfers/sessions", IdempotencyMode::Plaintext),
    supported(
        "/api/v1/public/reverse-shares/{alias}/sessions",
        IdempotencyMode::Sealed,
    ),
    supported("/api/v1/folders/ensure-path", IdempotencyMode::Plaintext),
    supported("/api/v1/shares", IdempotencyMode::Plaintext),
    supported("/api/v1/shares/{id}/notify", IdempotencyMode::Plaintext),
    supported("/api/v1/reverse-shares", IdempotencyMode::Plaintext),
    supported("/api/v1/received/{id}/copy", IdempotencyMode::Plaintext),
    supported("/api/v1/received/batch/copy", IdempotencyMode::Plaintext),
    supported("/api/v1/received/{id}/move", IdempotencyMode::Plaintext),
    supported("/api/v1/received/batch/move", IdempotencyMode::Plaintext),
    supported("/api/v1/admin/users", IdempotencyMode::Plaintext),
    supported("/api/v1/admin/invites", IdempotencyMode::Sealed),
];

pub fn supported_mode(method: &Method, path: &str) -> IdempotencyMode {
    SUPPORTED_ROUTES
        .iter()
        .find(|route| route.method == method && route.path == path)
        .map_or(IdempotencyMode::None, |route| route.mode)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayStorage {
    Plaintext,
    Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdempotencyRoute {
    storage: ReplayStorage,
    lease: Duration,
}

impl IdempotencyRoute {
    pub const fn new(mode: IdempotencyMode, lease: Duration) -> Option<Self> {
        match mode.storage() {
            Some(storage) => Some(Self { storage, lease }),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    User,
    ReverseShareGrant,
    ReverseShareLink,
}

impl ScopeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::ReverseShareGrant => "reverse_share_grant",
            Self::ReverseShareLink => "reverse_share_link",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyScope {
    kind: ScopeKind,
    id: String,
}

impl IdempotencyScope {
    pub fn user<E>(user_id: Id<E>) -> Self {
        Self::new(ScopeKind::User, user_id)
    }

    pub fn reverse_share_grant<E>(upload_session_id: Id<E>) -> Self {
        Self::new(ScopeKind::ReverseShareGrant, upload_session_id)
    }

    pub fn reverse_share_link<E>(reverse_share_id: Id<E>) -> Self {
        Self::new(ScopeKind::ReverseShareLink, reverse_share_id)
    }

    fn new<E>(kind: ScopeKind, id: Id<E>) -> Self {
        Self {
            kind,
            id: id.to_string(),
        }
    }
}

pub struct RequestIdentity(TokenDigest);

impl RequestIdentity {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn matches(&self, stored: &str) -> Result<bool, IdempotencyError> {
        let stored = TokenDigest::parse(stored)
            .map_err(|_| IdempotencyError::CorruptRecord("request_hash"))?;
        Ok(self.0.verify(&stored))
    }
}

impl fmt::Debug for RequestIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RequestIdentity(<redacted>)")
    }
}

pub fn request_identity(
    keys: &KeyRing,
    method: &Method,
    path: &str,
    body: &Value,
) -> RequestIdentity {
    let body = canonical_json(body);
    let mut message = Zeroizing::new(Vec::with_capacity(
        method.as_str().len() + path.len() + body.len() + 2,
    ));
    message.extend_from_slice(method.as_str().as_bytes());
    message.push(0);
    message.extend_from_slice(path.as_bytes());
    message.push(0);
    message.extend_from_slice(body.as_bytes());
    RequestIdentity(mac_hex(keys, MacPurpose::IdempotencyRequest, &message))
}

pub fn canonical_json(value: &Value) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::new());
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{}", Value::from(key.as_str()));
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
        scalar => {
            let _ = write!(out, "{scalar}");
        }
    }
}

pub(crate) fn replay_aad(id: &str, scope: &IdempotencyScope, route_template: &str) -> Vec<u8> {
    let parts = [
        AAD_LABEL,
        id,
        scope.kind.as_str(),
        scope.id.as_str(),
        route_template,
    ];
    let mut aad = Vec::with_capacity(parts.iter().map(|part| part.len() + 1).sum());
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            aad.push(0);
        }
        aad.extend_from_slice(part.as_bytes());
    }
    aad
}

struct Keyed {
    storage: ReplayStorage,
    lease: Duration,
    template: Arc<str>,
    method: Method,
    path: String,
    key_hash: TokenDigest,
    scope: IdempotencyScope,
}

pub struct IdempotencyRequest {
    keyed: Option<Keyed>,
    request_id: Option<RequestId>,
}

impl fmt::Debug for IdempotencyRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdempotencyRequest")
            .field("keyed", &self.keyed.is_some())
            .finish_non_exhaustive()
    }
}

impl<S> FromRequestParts<S> for IdempotencyRequest
where
    S: Send + Sync,
{
    type Rejection = IdempotencyRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let request_id = parts.extensions.get::<RequestId>().cloned();
        let Some(route) = parts.extensions.get::<IdempotencyRoute>().copied() else {
            return Ok(Self {
                keyed: None,
                request_id,
            });
        };
        let reject = |rejected| IdempotencyRejection {
            rejected,
            request_id: request_id.clone(),
        };

        let mut values = parts.headers.get_all(IDEMPOTENCY_KEY).iter();
        let Some(value) = values.next() else {
            return Ok(Self {
                keyed: None,
                request_id,
            });
        };
        let key = value
            .to_str()
            .ok()
            .filter(|key| (MIN_KEY_CHARS..=MAX_KEY_CHARS).contains(&key.len()));
        let (Some(key), None) = (key, values.next()) else {
            return Err(reject(Rejected::InvalidKey));
        };
        let key_hash = sha256_hex(key.as_bytes());

        let Some(template) = parts
            .extensions
            .get::<MatchedPath>()
            .map(MatchedPath::as_str)
        else {
            tracing::error!("an idempotent route was reached without a matched route template");
            return Err(reject(Rejected::Internal(ErrorCode::InternalError)));
        };
        let Some(scope) = parts.extensions.get::<IdempotencyScope>().cloned() else {
            tracing::error!(
                route = template,
                "an idempotent route was reached without a resolved scope"
            );
            return Err(reject(Rejected::Internal(ErrorCode::InternalError)));
        };

        Ok(Self {
            keyed: Some(Keyed {
                storage: route.storage,
                lease: route.lease,
                template: Arc::from(template),
                method: parts.method.clone(),
                path: parts.uri.path().to_owned(),
                key_hash,
                scope,
            }),
            request_id,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rejected {
    InvalidKey,
    KeyConflict,
    InProgress,
    Internal(ErrorCode),
}

#[derive(Debug)]
pub struct IdempotencyRejection {
    rejected: Rejected,
    request_id: Option<RequestId>,
}

impl IdempotencyRejection {
    fn api_error(&self) -> ApiError {
        match self.rejected {
            Rejected::InvalidKey => invalid_param(KEY_FIELD),
            Rejected::KeyConflict => ApiError::new(ErrorCode::IdempotencyKeyConflict),
            Rejected::InProgress => ApiError::new(ErrorCode::IdempotencyRequestInProgress),
            Rejected::Internal(code) => ApiError::new(code),
        }
    }
}

impl IntoResponse for IdempotencyRejection {
    fn into_response(self) -> Response {
        let mut response = tag_error(self.api_error(), self.request_id.as_ref()).into_response();
        if self.rejected == Rejected::InProgress {
            response
                .headers_mut()
                .insert(RETRY_AFTER, IN_PROGRESS_RETRY_AFTER);
        }
        response
    }
}

pub struct ReplayEnvelope {
    status: StatusCode,
    body: Value,
    location: Option<HeaderValue>,
    grant_cookie: Option<HeaderValue>,
}

impl ReplayEnvelope {
    pub const fn new(status: StatusCode, body: Value) -> Self {
        Self {
            status,
            body,
            location: None,
            grant_cookie: None,
        }
    }

    #[must_use]
    pub fn with_location(mut self, location: HeaderValue) -> Self {
        self.location = Some(location);
        self
    }

    #[must_use]
    pub fn with_grant_cookie(mut self, mut cookie: HeaderValue) -> Self {
        cookie.set_sensitive(true);
        self.grant_cookie = Some(cookie);
        self
    }

    fn encode(&self, storage: ReplayStorage) -> Result<Zeroizing<Vec<u8>>, IdempotencyError> {
        if !(200..=599).contains(&self.status.as_u16()) {
            return Err(IdempotencyError::InvalidStatus);
        }
        if storage == ReplayStorage::Plaintext && self.grant_cookie.is_some() {
            return Err(IdempotencyError::GrantCookieRequiresSealed);
        }
        let envelope = EnvelopeRef {
            body: &self.body,
            headers: HeadersRef {
                location: header_text(self.location.as_ref())?,
                set_cookie: header_text(self.grant_cookie.as_ref())?,
            },
        };
        let encoded =
            Zeroizing::new(serde_json::to_vec(&envelope).map_err(|_| IdempotencyError::Encoding)?);
        if encoded.len() > MAX_ENVELOPE_BYTES {
            return Err(IdempotencyError::EnvelopeTooLarge);
        }
        Ok(encoded)
    }

    fn decode(
        status: StatusCode,
        bytes: &[u8],
        storage: ReplayStorage,
    ) -> Result<Self, IdempotencyError> {
        let stored: StoredEnvelope = serde_json::from_slice(bytes).map_err(|_| CORRUPT_ENVELOPE)?;
        if storage == ReplayStorage::Plaintext && stored.headers.set_cookie.is_some() {
            return Err(CORRUPT_ENVELOPE);
        }
        let mut envelope = Self::new(status, stored.body);
        envelope.location = header_value(stored.headers.location)?;
        if let Some(cookie) = header_value(stored.headers.set_cookie)? {
            envelope = envelope.with_grant_cookie(cookie);
        }
        Ok(envelope)
    }
}

const CORRUPT_ENVELOPE: IdempotencyError = IdempotencyError::CorruptRecord("replay envelope");

fn header_text(value: Option<&HeaderValue>) -> Result<Option<&str>, IdempotencyError> {
    value
        .map(|value| {
            value
                .to_str()
                .map_err(|_| IdempotencyError::UnrepresentableHeader)
        })
        .transpose()
}

fn header_value(value: Option<String>) -> Result<Option<HeaderValue>, IdempotencyError> {
    value
        .map(|value| HeaderValue::try_from(value).map_err(|_| CORRUPT_ENVELOPE))
        .transpose()
}

impl fmt::Debug for ReplayEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplayEnvelope")
            .field("status", &self.status)
            .field("location", &self.location.is_some())
            .field("grant_cookie", &self.grant_cookie.is_some())
            .finish_non_exhaustive()
    }
}

impl IntoResponse for ReplayEnvelope {
    fn into_response(self) -> Response {
        let mut response = if self.body.is_null() {
            self.status.into_response()
        } else {
            match serde_json::to_vec(&self.body) {
                Ok(body) => (
                    self.status,
                    [(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))],
                    body,
                )
                    .into_response(),
                Err(_) => return ApiError::internal().into_response(),
            }
        };
        if let Some(location) = self.location {
            response.headers_mut().insert(LOCATION, location);
        }
        if let Some(cookie) = self.grant_cookie {
            response.headers_mut().append(SET_COOKIE, cookie);
        }
        response
    }
}

#[derive(Serialize)]
struct EnvelopeRef<'a> {
    body: &'a Value,
    headers: HeadersRef<'a>,
}

#[derive(Serialize)]
struct HeadersRef<'a> {
    #[serde(rename = "Location", skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
    #[serde(rename = "Set-Cookie", skip_serializing_if = "Option::is_none")]
    set_cookie: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEnvelope {
    body: Value,
    headers: StoredHeaders,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHeaders {
    #[serde(rename = "Location", default)]
    location: Option<String>,
    #[serde(rename = "Set-Cookie", default)]
    set_cookie: Option<String>,
}

#[derive(Debug)]
struct ClaimedRecord {
    id: String,
    storage: ReplayStorage,
    scope: IdempotencyScope,
    template: Arc<str>,
    lease: Timestamp,
}

#[derive(Debug)]
pub struct Claim {
    record: Option<ClaimedRecord>,
}

#[derive(Debug)]
pub enum Admission {
    Execute(Claim),
    Replay(Response),
}

pub enum IdempotencyRecord {}

#[derive(sqlx::FromRow)]
struct StoredRow {
    id: String,
    request_hash: String,
    state: String,
    lease_expires_at: Option<String>,
    response_status: Option<i64>,
    response_json: Option<String>,
    response_ciphertext: Option<Vec<u8>>,
    response_nonce: Option<Vec<u8>>,
    key_version: Option<i64>,
    expires_at: String,
}

enum ClaimOutcome {
    Claimed(ClaimedRecord),
    Conflict,
    InProgress,
    Completed(StoredRow),
}

struct ClaimTimes {
    now: Timestamp,
    lease: Timestamp,
    expires: Timestamp,
}

#[derive(Clone)]
pub struct IdempotencyService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
}

impl fmt::Debug for IdempotencyService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdempotencyService").finish_non_exhaustive()
    }
}

impl IdempotencyService {
    pub fn new(pools: DbPools, clock: Arc<dyn Clock>, keys: Arc<KeyRing>) -> Self {
        Self { pools, clock, keys }
    }

    pub async fn claim(
        &self,
        request: IdempotencyRequest,
        body: &Value,
    ) -> Result<Admission, IdempotencyRejection> {
        let IdempotencyRequest { keyed, request_id } = request;
        let Some(keyed) = keyed else {
            return Ok(Admission::Execute(Claim { record: None }));
        };
        let reject = |rejected| IdempotencyRejection {
            rejected,
            request_id: request_id.clone(),
        };
        let identity = request_identity(&self.keys, &keyed.method, &keyed.path, body);

        let outcome = match self.claim_record(&keyed, &identity).await {
            Ok(outcome) => outcome,
            Err(error) => return Err(reject(failed(&keyed, &error))),
        };
        match outcome {
            ClaimOutcome::Claimed(record) => Ok(Admission::Execute(Claim {
                record: Some(record),
            })),
            ClaimOutcome::Conflict => Err(reject(Rejected::KeyConflict)),
            ClaimOutcome::InProgress => Err(reject(Rejected::InProgress)),
            ClaimOutcome::Completed(row) => match self.replay(&keyed, row) {
                Ok(response) => Ok(Admission::Replay(response)),
                Err(error) => Err(reject(failed(&keyed, &error))),
            },
        }
    }

    pub async fn complete(
        &self,
        tx: &mut WriteTx<'_>,
        claim: &Claim,
        envelope: &ReplayEnvelope,
    ) -> Result<(), IdempotencyError> {
        let Some(record) = &claim.record else {
            return Ok(());
        };
        let encoded = envelope.encode(record.storage)?;
        let completed_at = Timestamp::try_from(self.clock.now())?;
        let (json, sealed) = match record.storage {
            ReplayStorage::Plaintext => {
                let json = std::str::from_utf8(&encoded)
                    .map_err(|_| IdempotencyError::Encoding)?
                    .to_owned();
                (Some(json), None)
            }
            ReplayStorage::Sealed => {
                let aad = replay_aad(&record.id, &record.scope, &record.template);
                let sealed = self
                    .keys
                    .seal(SealPurpose::IdempotencyReplay, &aad, &encoded)?;
                (None, Some(sealed))
            }
        };
        let completed = sqlx::query(COMPLETE_RECORD)
            .bind(completed_at.to_string())
            .bind(i64::from(envelope.status.as_u16()))
            .bind(json)
            .bind(sealed.as_ref().map(|sealed| sealed.ciphertext().to_vec()))
            .bind(sealed.as_ref().map(|sealed| sealed.nonce().to_vec()))
            .bind(sealed.as_ref().map(SealedSecret::key_version))
            .bind(&record.id)
            .bind(record.lease.to_string())
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
        if completed.rows_affected() == 1 {
            Ok(())
        } else {
            Err(IdempotencyError::ClaimLost)
        }
    }

    pub async fn release(&self, claim: Claim) -> Result<(), IdempotencyError> {
        let Some(record) = claim.record else {
            return Ok(());
        };
        self.pools
            .write_tx(self.clock.as_ref(), "idempotency.release", async |tx| {
                sqlx::query(RELEASE_RECORD)
                    .bind(&record.id)
                    .bind(record.lease.to_string())
                    .execute(tx.executor())
                    .await
                    .map_err(DbError::from)?;
                Ok::<(), IdempotencyError>(())
            })
            .await
    }

    async fn claim_record(
        &self,
        keyed: &Keyed,
        identity: &RequestIdentity,
    ) -> Result<ClaimOutcome, IdempotencyError> {
        let now = self.clock.now();
        let times = ClaimTimes {
            now: Timestamp::try_from(now)?,
            lease: Timestamp::try_from(now + keyed.lease)?,
            expires: Timestamp::try_from(now + REPLAY_WINDOW)?,
        };
        let fresh_id = Id::<IdempotencyRecord>::generate(self.clock.as_ref()).to_string();
        self.pools
            .write_tx(self.clock.as_ref(), "idempotency.claim", async |tx| {
                claim_in_tx(tx, keyed, identity, fresh_id, &times).await
            })
            .await
    }

    fn replay(&self, keyed: &Keyed, row: StoredRow) -> Result<Response, IdempotencyError> {
        let status = row
            .response_status
            .and_then(|status| u16::try_from(status).ok())
            .and_then(|status| StatusCode::from_u16(status).ok())
            .ok_or(IdempotencyError::CorruptRecord("response_status"))?;
        let envelope = match (keyed.storage, row.response_json, row.response_ciphertext) {
            (ReplayStorage::Plaintext, Some(json), None) => {
                ReplayEnvelope::decode(status, json.as_bytes(), ReplayStorage::Plaintext)?
            }
            (ReplayStorage::Sealed, None, Some(ciphertext)) => {
                let nonce = row
                    .response_nonce
                    .ok_or(IdempotencyError::CorruptRecord("response_nonce"))?;
                let key_version = row
                    .key_version
                    .ok_or(IdempotencyError::CorruptRecord("key_version"))?;
                let sealed = SealedSecret::from_parts(ciphertext, &nonce, key_version)?;
                let aad = replay_aad(&row.id, &keyed.scope, &keyed.template);
                let opened = self
                    .keys
                    .open(SealPurpose::IdempotencyReplay, &aad, &sealed)?;
                ReplayEnvelope::decode(status, opened.expose_secret(), ReplayStorage::Sealed)?
            }
            _ => return Err(IdempotencyError::CorruptRecord("replay storage")),
        };
        let mut response = envelope.into_response();
        response
            .headers_mut()
            .insert(IDEMPOTENCY_REPLAYED, REPLAYED);
        Ok(response)
    }
}

fn failed(keyed: &Keyed, error: &IdempotencyError) -> Rejected {
    tracing::error!(
        route = &*keyed.template,
        error_kind = error.kind(),
        "idempotency record could not be claimed or replayed"
    );
    Rejected::Internal(error.api_code())
}

async fn claim_in_tx(
    tx: &mut WriteTx<'_>,
    keyed: &Keyed,
    identity: &RequestIdentity,
    fresh_id: String,
    times: &ClaimTimes,
) -> Result<ClaimOutcome, IdempotencyError> {
    let existing: Option<StoredRow> = sqlx::query_as(SELECT_RECORD)
        .bind(keyed.scope.kind.as_str())
        .bind(&keyed.scope.id)
        .bind(keyed.method.as_str())
        .bind(&*keyed.template)
        .bind(keyed.key_hash.as_str())
        .fetch_optional(tx.executor())
        .await
        .map_err(DbError::from)?;

    if let Some(row) = existing {
        if parse_time(&row.expires_at, "expires_at")? > times.now {
            return decide(tx, keyed, identity, row, times).await;
        }
        sqlx::query(DELETE_EXPIRED_RECORD)
            .bind(&row.id)
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
    }

    sqlx::query(INSERT_RECORD)
        .bind(&fresh_id)
        .bind(keyed.scope.kind.as_str())
        .bind(&keyed.scope.id)
        .bind(keyed.method.as_str())
        .bind(&*keyed.template)
        .bind(keyed.key_hash.as_str())
        .bind(identity.as_str())
        .bind(times.lease.to_string())
        .bind(times.now.to_string())
        .bind(times.expires.to_string())
        .execute(tx.executor())
        .await
        .map_err(DbError::from)?;
    Ok(ClaimOutcome::Claimed(claimed(keyed, fresh_id, times)))
}

async fn decide(
    tx: &mut WriteTx<'_>,
    keyed: &Keyed,
    identity: &RequestIdentity,
    row: StoredRow,
    times: &ClaimTimes,
) -> Result<ClaimOutcome, IdempotencyError> {
    if !identity.matches(&row.request_hash)? {
        return Ok(ClaimOutcome::Conflict);
    }
    match row.state.as_str() {
        "completed" => Ok(ClaimOutcome::Completed(row)),
        "in_progress" => {
            let held = row
                .lease_expires_at
                .as_deref()
                .ok_or(IdempotencyError::CorruptRecord("lease_expires_at"))?;
            if parse_time(held, "lease_expires_at")? > times.now {
                return Ok(ClaimOutcome::InProgress);
            }
            let taken = sqlx::query(TAKE_OVER_RECORD)
                .bind(times.lease.to_string())
                .bind(&row.id)
                .bind(held)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)?;
            if taken.rows_affected() != 1 {
                return Err(IdempotencyError::ClaimLost);
            }
            Ok(ClaimOutcome::Claimed(claimed(keyed, row.id, times)))
        }
        _ => Err(IdempotencyError::CorruptRecord("state")),
    }
}

fn claimed(keyed: &Keyed, id: String, times: &ClaimTimes) -> ClaimedRecord {
    ClaimedRecord {
        id,
        storage: keyed.storage,
        scope: keyed.scope.clone(),
        template: Arc::clone(&keyed.template),
        lease: times.lease,
    }
}

fn parse_time(text: &str, column: &'static str) -> Result<Timestamp, IdempotencyError> {
    text.parse::<Timestamp>()
        .map_err(|_| IdempotencyError::CorruptRecord(column))
}

#[derive(Debug)]
pub enum IdempotencyError {
    Db(DbError),
    Time(InvalidTimestamp),
    Crypto(CryptoError),
    GrantCookieRequiresSealed,
    InvalidStatus,
    UnrepresentableHeader,
    Encoding,
    EnvelopeTooLarge,
    CorruptRecord(&'static str),
    ClaimLost,
}

impl IdempotencyError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "idempotency_time_out_of_range",
            Self::Crypto(_) => "idempotency_crypto_failed",
            Self::GrantCookieRequiresSealed => "idempotency_grant_cookie_requires_sealed",
            Self::InvalidStatus => "idempotency_invalid_status",
            Self::UnrepresentableHeader => "idempotency_unrepresentable_header",
            Self::Encoding => "idempotency_encoding_failed",
            Self::EnvelopeTooLarge => "idempotency_envelope_too_large",
            Self::CorruptRecord(_) => "idempotency_corrupt_record",
            Self::ClaimLost => "idempotency_claim_lost",
        }
    }

    pub const fn api_code(&self) -> ErrorCode {
        match self {
            Self::Db(error) => error.api_code(),
            _ => ErrorCode::InternalError,
        }
    }
}

impl fmt::Display for IdempotencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(error) => write!(f, "idempotency database operation failed: {error}"),
            Self::Time(error) => write!(f, "idempotency timestamp is out of range: {error}"),
            Self::Crypto(error) => write!(f, "idempotency replay envelope crypto failed: {error}"),
            Self::GrantCookieRequiresSealed => {
                f.write_str("a grant Set-Cookie can only be stored by a sealed route")
            }
            Self::InvalidStatus => f.write_str("replay status must be between 200 and 599"),
            Self::UnrepresentableHeader => f.write_str("replay header is not visible ASCII"),
            Self::Encoding => f.write_str("replay envelope could not be encoded"),
            Self::EnvelopeTooLarge => {
                write!(f, "replay envelope exceeds {MAX_ENVELOPE_BYTES} bytes")
            }
            Self::CorruptRecord(column) => {
                write!(f, "idempotency record has an invalid {column}")
            }
            Self::ClaimLost => f.write_str("idempotency claim is no longer held by this request"),
        }
    }
}

impl std::error::Error for IdempotencyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::Time(error) => Some(error),
            Self::Crypto(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbError> for IdempotencyError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<InvalidTimestamp> for IdempotencyError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<CryptoError> for IdempotencyError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<IdempotencyError> for ApiError {
    fn from(error: IdempotencyError) -> Self {
        Self::new(error.api_code())
    }
}

#[cfg(test)]
mod tests {
    use http::Method;
    use serde_json::{json, Value};
    use tempfile::TempDir;

    use super::{canonical_json, request_identity, supported_mode, IdempotencyMode};
    use crate::infra::crypto::hash::{sha256_hex, DIGEST_HEX_LEN};
    use crate::infra::crypto::hkdf::{KeyRing, MacPurpose};
    use crate::infra::crypto::instance_key::InstanceKey;

    const PASSWORD: &str = "initial-password-sentinel-51f0";

    fn fresh_ring() -> (TempDir, KeyRing) {
        let dir = TempDir::new().unwrap();
        let (key, _) = InstanceKey::load_or_create(dir.path()).unwrap();
        let ring = KeyRing::new(&key);
        (dir, ring)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn unit_idempotency_canonical_request_hash() {
        let (_dir, ring) = fresh_ring();
        let (_other_dir, other_ring) = fresh_ring();
        let path = "/api/v1/shares/019300aa-0000-7000-8000-00000000000a/notify";
        let body = parse(&format!(
            r#"{{ "recipients": ["b@example.test", "a@example.test"],
                 "options": {{ "z": 1.5, "a": null, "m": [true, {{ "y": 2, "x": 1 }}] }},
                 "password": "{PASSWORD}", "count": 10 }}"#
        ));
        let reordered = parse(&format!(
            r#"{{"count":10,"password":"{PASSWORD}","options":{{"m":[true,{{"x":1,"y":2}}],"a":null,"z":1.5}},"recipients":["b@example.test","a@example.test"]}}"#
        ));

        let canonical = canonical_json(&body);
        assert_eq!(
            canonical.as_str(),
            format!(
                r#"{{"count":10,"options":{{"a":null,"m":[true,{{"x":1,"y":2}}],"z":1.5}},"password":"{PASSWORD}","recipients":["b@example.test","a@example.test"]}}"#
            )
        );
        assert_eq!(canonical.as_str(), canonical_json(&reordered).as_str());
        assert_eq!(
            canonical_json(&parse(
                r#"{"s":"line\nbreak \"quoted\" é","n":-0.25e2,"u":1e2}"#
            ))
            .as_str(),
            r#"{"n":-25.0,"s":"line\nbreak \"quoted\" é","u":100.0}"#
        );
        assert_eq!(canonical_json(&Value::Null).as_str(), "null");

        let identity = request_identity(&ring, &Method::POST, path, &body);
        assert_eq!(
            identity.as_str(),
            request_identity(&ring, &Method::POST, path, &reordered).as_str()
        );
        assert_eq!(identity.as_str().len(), DIGEST_HEX_LEN);
        assert!(identity
            .as_str()
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));

        let mut changed = body.clone();
        changed["count"] = json!(11);
        let other_path = "/api/v1/shares/019300aa-0000-7000-8000-00000000000b/notify";
        for different in [
            request_identity(&ring, &Method::POST, path, &changed),
            request_identity(&ring, &Method::POST, other_path, &body),
            request_identity(&ring, &Method::PUT, path, &body),
            request_identity(&ring, &Method::POST, path, &Value::Null),
            request_identity(&other_ring, &Method::POST, path, &body),
        ] {
            assert_ne!(different.as_str(), identity.as_str());
        }

        let message = format!("POST\0{path}\0{}", canonical.as_str());
        assert_eq!(
            MacPurpose::IdempotencyRequest.label(),
            "palmr:v1:idempotency-request"
        );
        assert_eq!(
            identity.as_str(),
            hex(&ring.mac(MacPurpose::IdempotencyRequest, message.as_bytes()))
        );
        for purpose in MacPurpose::ALL {
            if purpose != MacPurpose::IdempotencyRequest {
                assert_ne!(
                    identity.as_str(),
                    hex(&ring.mac(purpose, message.as_bytes()))
                );
            }
        }
        assert_ne!(identity.as_str(), sha256_hex(message.as_bytes()).as_str());
        assert_ne!(identity.as_str(), sha256_hex(canonical.as_bytes()).as_str());
        assert!(!identity.as_str().contains(PASSWORD));
        assert!(!format!("{identity:?}").contains(identity.as_str()));
    }

    #[test]
    fn unit_idempotency_catalogue_is_the_documented_route_set() {
        let declared: Vec<(Method, &str, IdempotencyMode)> = super::SUPPORTED_ROUTES
            .iter()
            .map(|route| (route.method.clone(), route.path, route.mode))
            .collect();
        assert_eq!(
            declared,
            [
                (
                    Method::POST,
                    "/api/v1/transfers/sessions",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/public/reverse-shares/{alias}/sessions",
                    IdempotencyMode::Sealed
                ),
                (
                    Method::POST,
                    "/api/v1/folders/ensure-path",
                    IdempotencyMode::Plaintext
                ),
                (Method::POST, "/api/v1/shares", IdempotencyMode::Plaintext),
                (
                    Method::POST,
                    "/api/v1/shares/{id}/notify",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/reverse-shares",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/received/{id}/copy",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/received/batch/copy",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/received/{id}/move",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/received/batch/move",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/admin/users",
                    IdempotencyMode::Plaintext
                ),
                (
                    Method::POST,
                    "/api/v1/admin/invites",
                    IdempotencyMode::Sealed
                ),
            ]
        );
        assert_eq!(
            supported_mode(&Method::POST, "/api/v1/admin/invites"),
            IdempotencyMode::Sealed
        );
        for (method, path) in [
            (Method::PUT, "/api/v1/shares/{id}/items"),
            (Method::GET, "/api/v1/shares"),
            (Method::DELETE, "/api/v1/shares/{id}"),
            (Method::POST, "/api/v1/files/batch/move"),
            (Method::POST, "/api/v1/admin/invites/{id}/resend"),
        ] {
            assert_eq!(supported_mode(&method, path), IdempotencyMode::None);
        }
    }
}

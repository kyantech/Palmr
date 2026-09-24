pub mod backoff;
pub mod claim;
pub mod cli;
pub mod kinds;
pub mod prune_tokens;
pub mod recurring;
pub mod runtime;

use std::fmt;

use serde::Serialize;
use serde_json::Value;

pub use self::backoff::Jitter;
pub use self::kinds::{JobKind, Priority};
pub use self::runtime::{
    BackgroundDrain, Dispatcher, FailureClass, Idempotency, JobAudit, JobAuditEvent, JobAuditSink,
    JobRuntime, JobsDrain, Registry, RuntimeTiming,
};
use crate::domain::id::Id;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::infra::db::{DbError, InstanceId};

pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_DEDUP_KEY_CHARS: usize = 256;
pub const MAX_LAST_ERROR_BYTES: usize = 2 * 1024;

pub enum Job {}

pub type JobId = Id<Job>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Claimant(String);

impl Claimant {
    pub fn worker(instance: InstanceId, worker: u16) -> Self {
        Self(format!("{instance}#{worker}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Claimant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    NotAnObject,
    TooLarge,
    Unserializable,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAnObject => "job payload must serialize to a JSON object",
            Self::TooLarge => "job payload exceeds 16 KiB",
            Self::Unserializable => "job payload could not be serialized",
        })
    }
}

impl std::error::Error for PayloadError {}

#[derive(Clone, PartialEq, Eq)]
pub struct JobPayload(String);

impl JobPayload {
    pub fn new(payload: &impl Serialize) -> Result<Self, PayloadError> {
        let value = serde_json::to_value(payload).map_err(|_| PayloadError::Unserializable)?;
        if !value.is_object() {
            return Err(PayloadError::NotAnObject);
        }
        let text = value.to_string();
        if text.len() > MAX_PAYLOAD_BYTES {
            return Err(PayloadError::TooLarge);
        }
        Ok(Self(text))
    }

    pub fn empty() -> Self {
        Self(String::from("{}"))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for JobPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobPayload")
            .field("bytes", &self.0.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDedupKey;

impl fmt::Display for InvalidDedupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dedup key must be 1 to 256 characters")
    }
}

impl std::error::Error for InvalidDedupKey {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DedupKey(String);

impl DedupKey {
    pub fn new(key: impl Into<String>) -> Result<Self, InvalidDedupKey> {
        let key = key.into();
        if (1..=MAX_DEDUP_KEY_CHARS).contains(&key.chars().count()) {
            Ok(Self(key))
        } else {
            Err(InvalidDedupKey)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct NewJob {
    kind: JobKind,
    payload: JobPayload,
    priority: Priority,
    run_at: Option<Timestamp>,
    dedup_key: Option<DedupKey>,
}

impl NewJob {
    pub const fn new(kind: JobKind, payload: JobPayload) -> Self {
        Self {
            kind,
            payload,
            priority: kind.policy().priority,
            run_at: None,
            dedup_key: None,
        }
    }

    #[must_use]
    pub const fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub const fn run_at(mut self, run_at: Timestamp) -> Self {
        self.run_at = Some(run_at);
        self
    }

    #[must_use]
    pub fn dedup_key(mut self, key: DedupKey) -> Self {
        self.dedup_key = Some(key);
        self
    }
}

#[derive(Clone)]
pub struct ClaimedJob {
    id: JobId,
    kind: JobKind,
    payload: Value,
    attempts: u32,
    max_attempts: u32,
    lease_expires_at: Timestamp,
    claimant: Claimant,
}

impl ClaimedJob {
    pub const fn id(&self) -> JobId {
        self.id
    }

    pub const fn kind(&self) -> JobKind {
        self.kind
    }

    pub const fn payload(&self) -> &Value {
        &self.payload
    }

    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub const fn claimant(&self) -> &Claimant {
        &self.claimant
    }
}

impl fmt::Debug for ClaimedJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimedJob")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("attempts", &self.attempts)
            .field("max_attempts", &self.max_attempts)
            .field("lease_expires_at", &self.lease_expires_at)
            .field("claimant", &self.claimant)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum JobsError {
    Db(DbError),
    Time(InvalidTimestamp),
    Dedup(InvalidDedupKey),
    CorruptRow(&'static str),
}

impl JobsError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "time_out_of_range",
            Self::Dedup(_) => "invalid_dedup_key",
            Self::CorruptRow(_) => "corrupt_row",
        }
    }
}

impl fmt::Display for JobsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(error) => write!(f, "job queue database operation failed: {error}"),
            Self::Time(error) => write!(f, "job queue time is out of range: {error}"),
            Self::Dedup(error) => write!(f, "job dedup key is invalid: {error}"),
            Self::CorruptRow(column) => write!(f, "jobs row has an invalid {column}"),
        }
    }
}

impl std::error::Error for JobsError {}

impl From<DbError> for JobsError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for JobsError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for JobsError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

impl From<InvalidDedupKey> for JobsError {
    fn from(error: InvalidDedupKey) -> Self {
        Self::Dedup(error)
    }
}

#[cfg(test)]
mod tests;

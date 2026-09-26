use serde_json::{Map, Value};

use crate::infra::jobs::{FailureClass, JobKind};

use super::model::AuditAction;

pub const MAX_METADATA_BYTES: usize = 4096;

const MAX_SETTING_KEY_CHARS: usize = 64;

// `Metadata` has no public constructor and no `From`/`Serialize` impl: the
// only builders are the per-action functions in this module, so a domain
// struct can never be serialized into an audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata(String);

impl Metadata {
    fn json(fields: &[(&str, Value)]) -> Self {
        let mut object = Map::with_capacity(fields.len());
        for (key, value) in fields {
            object.insert((*key).to_owned(), value.clone());
        }
        let text = Value::Object(object).to_string();
        debug_assert!(
            text.len() <= MAX_METADATA_BYTES,
            "an audit metadata builder produced more than {MAX_METADATA_BYTES} bytes"
        );
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn bytes(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionSpec {
    action: AuditAction,
    metadata: Metadata,
}

impl ActionSpec {
    fn new(action: AuditAction, metadata: Metadata) -> Self {
        Self { action, metadata }
    }

    pub const fn action(&self) -> AuditAction {
        self.action
    }

    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingKey(String);

impl SettingKey {
    pub const SMTP_PASSWORD: &'static str = "smtp_password";

    pub fn new(key: impl Into<String>) -> Option<Self> {
        let key = key.into();
        let valid = (1..=MAX_SETTING_KEY_CHARS).contains(&key.chars().count())
            && key
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        valid.then_some(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Set,
    Unset,
}

impl Presence {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Unset => "unset",
        }
    }
}

// Secret settings record presence transitions only. The builder has no
// parameter for a value, so `smtp_password` can be recorded as
// `{"key":"smtp_password","from":"set","to":"set"}` and never as a secret.
pub fn setting_changed(key: &SettingKey, from: Presence, to: Presence) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("key", Value::from(key.as_str())),
        ("from", Value::from(from.as_str())),
        ("to", Value::from(to.as_str())),
    ]);
    ActionSpec::new(AuditAction::SettingChanged, metadata)
}

// The job payload is deliberately absent: it can carry secret-bearing
// material, and the dead-letter record needs only the kind and outcome.
pub fn job_dead_lettered(
    kind: JobKind,
    attempts: u32,
    max_attempts: u32,
    failure: FailureClass,
) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("kind", Value::from(kind.as_str())),
        ("attempts", Value::from(attempts)),
        ("max_attempts", Value::from(max_attempts)),
        ("failure", Value::from(failure.code())),
    ]);
    ActionSpec::new(AuditAction::JobDeadLettered, metadata)
}

pub fn session_revoked(reason: &'static str, current: bool) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("reason", Value::from(reason)),
        ("current", Value::from(current)),
    ]);
    ActionSpec::new(AuditAction::SessionRevoked, metadata)
}

pub fn all_sessions_revoked(
    reason: &'static str,
    include_current: bool,
    revoked: u64,
) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("reason", Value::from(reason)),
        ("include_current", Value::from(include_current)),
        ("revoked", Value::from(revoked)),
    ]);
    ActionSpec::new(AuditAction::AllSessionsRevoked, metadata)
}

pub fn login_succeeded(method: &'static str) -> ActionSpec {
    let metadata = Metadata::json(&[("method", Value::from(method))]);
    ActionSpec::new(AuditAction::LoginSucceeded, metadata)
}

pub fn login_failed(method: &'static str, reason: &'static str) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("method", Value::from(method)),
        ("reason", Value::from(reason)),
    ]);
    ActionSpec::new(AuditAction::LoginFailed, metadata)
}

pub fn login_locked_out(failed_count: u32, lock_count: u32, lockout_minutes: u32) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("failed_count", Value::from(failed_count)),
        ("lock_count", Value::from(lock_count)),
        ("lockout_minutes", Value::from(lockout_minutes)),
    ]);
    ActionSpec::new(AuditAction::LoginLockedOut, metadata)
}

pub fn logout() -> ActionSpec {
    ActionSpec::new(AuditAction::Logout, Metadata::json(&[]))
}

pub fn password_changed(
    forced: bool,
    sessions_revoked: u64,
    trusted_devices_revoked: u64,
) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("forced", Value::from(forced)),
        ("sessions_revoked", Value::from(sessions_revoked)),
        (
            "trusted_devices_revoked",
            Value::from(trusted_devices_revoked),
        ),
    ]);
    ActionSpec::new(AuditAction::PasswordChanged, metadata)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdminRecoverFacts {
    pub role_changed: bool,
    pub activated: bool,
    pub lockout_cleared: bool,
    pub password_login_reenabled: bool,
    pub sessions_revoked: u64,
}

pub fn operator_cli_admin_recover(facts: AdminRecoverFacts) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("role_changed", Value::from(facts.role_changed)),
        ("activated", Value::from(facts.activated)),
        ("lockout_cleared", Value::from(facts.lockout_cleared)),
        (
            "password_login_reenabled",
            Value::from(facts.password_login_reenabled),
        ),
        ("sessions_revoked", Value::from(facts.sessions_revoked)),
    ]);
    ActionSpec::new(AuditAction::OperatorCliAdminRecover, metadata)
}

// The temporary password and its hash have no parameter here: the row can
// record that a local credential was established, never what it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PasswordResetFacts {
    pub had_local_password: bool,
    pub sessions_revoked: u64,
    pub trusted_devices_revoked: u64,
    pub lockout_cleared: bool,
}

pub fn operator_cli_password_reset(facts: PasswordResetFacts) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("had_local_password", Value::from(facts.had_local_password)),
        ("sessions_revoked", Value::from(facts.sessions_revoked)),
        (
            "trusted_devices_revoked",
            Value::from(facts.trusted_devices_revoked),
        ),
        ("lockout_cleared", Value::from(facts.lockout_cleared)),
    ]);
    ActionSpec::new(AuditAction::OperatorCliPasswordReset, metadata)
}

pub fn setup_completed() -> ActionSpec {
    ActionSpec::new(AuditAction::SetupCompleted, Metadata::json(&[]))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrphanSweepCounts {
    pub outcome: &'static str,
    pub reap_enabled: bool,
    pub listed: u64,
    pub unparseable: u64,
    pub unparseable_bytes: u64,
    pub candidates: u64,
    pub candidate_bytes: u64,
    pub too_young: u64,
    pub live_transfer: u64,
    pub reaped: u64,
    pub reaped_bytes: u64,
    pub stale_tombstones: u64,
}

pub fn storage_orphan_detected(counts: &OrphanSweepCounts) -> ActionSpec {
    let metadata = Metadata::json(&[
        ("outcome", Value::from(counts.outcome)),
        ("reap_enabled", Value::from(counts.reap_enabled)),
        ("listed", Value::from(counts.listed)),
        ("unparseable", Value::from(counts.unparseable)),
        ("unparseable_bytes", Value::from(counts.unparseable_bytes)),
        ("candidates", Value::from(counts.candidates)),
        ("candidate_bytes", Value::from(counts.candidate_bytes)),
        ("too_young", Value::from(counts.too_young)),
        ("live_transfer", Value::from(counts.live_transfer)),
        ("reaped", Value::from(counts.reaped)),
        ("reaped_bytes", Value::from(counts.reaped_bytes)),
        ("stale_tombstones", Value::from(counts.stale_tombstones)),
    ]);
    ActionSpec::new(AuditAction::StorageOrphanDetected, metadata)
}

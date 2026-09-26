use sqlx::sqlite::SqliteRow;
use sqlx::Row;
use time::Duration;

use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::settings::model::AppSettings;
use crate::features::users::model::{NormalizedIdentifier, UserId};
use crate::infra::db::WriteTx;

use super::error::LoginError;

pub enum LoginAttemptRow {}
pub type LoginAttemptId = Id<LoginAttemptRow>;

const MAX_IP_CHARS: usize = 45;
const MAX_USER_AGENT_CHARS: usize = 512;
const MAX_REQUEST_ID_CHARS: usize = 64;

const INSERT_ATTEMPT: &str = "INSERT INTO login_attempts (
        id, at, identifier_normalized, user_id, method, result, ip, user_agent, request_id
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

const SELECT_STATE: &str = "SELECT failed_count, locked_until, lock_count
    FROM account_lockouts WHERE user_id = ?1";

const RECORD_FAILURE: &str = "INSERT INTO account_lockouts (
        user_id, failed_count, first_failed_at, last_failed_at, locked_until, lock_count, updated_at
    ) VALUES (
        ?1, 1, ?2, ?2,
        CASE WHEN 1 >= ?3 THEN ?4 ELSE NULL END,
        CASE WHEN 1 >= ?3 THEN 1 ELSE 0 END,
        ?2
    )
    ON CONFLICT (user_id) DO UPDATE SET
        failed_count = CASE
            WHEN locked_until > ?2 THEN failed_count
            WHEN locked_until IS NOT NULL THEN 1
            ELSE failed_count + 1 END,
        first_failed_at = CASE
            WHEN locked_until > ?2 THEN first_failed_at
            WHEN locked_until IS NOT NULL OR failed_count = 0 OR first_failed_at IS NULL THEN ?2
            ELSE first_failed_at END,
        last_failed_at = CASE WHEN locked_until > ?2 THEN last_failed_at ELSE ?2 END,
        lock_count = CASE
            WHEN locked_until > ?2 THEN lock_count
            WHEN (CASE WHEN locked_until IS NOT NULL THEN 1 ELSE failed_count + 1 END) >= ?3
                THEN lock_count + 1
            ELSE lock_count END,
        locked_until = CASE
            WHEN locked_until > ?2 THEN locked_until
            WHEN (CASE WHEN locked_until IS NOT NULL THEN 1 ELSE failed_count + 1 END) >= ?3
                THEN ?4
            ELSE NULL END,
        updated_at = ?2
    RETURNING failed_count, locked_until, lock_count";

const RESET: &str = "UPDATE account_lockouts
    SET failed_count = 0, first_failed_at = NULL, last_failed_at = NULL, locked_until = NULL,
        updated_at = ?2
    WHERE user_id = ?1";

const CLEAR: &str = "UPDATE account_lockouts
    SET failed_count = 0, first_failed_at = NULL, last_failed_at = NULL, locked_until = NULL,
        cleared_at = ?2, cleared_by = ?3, updated_at = ?2
    WHERE user_id = ?1 AND (failed_count > 0 OR locked_until IS NOT NULL)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockoutPolicy {
    max_attempts: u32,
    minutes: u32,
}

impl LockoutPolicy {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            max_attempts: settings.security.max_login_attempts.max(1),
            minutes: settings.security.login_lockout_minutes.max(1),
        }
    }

    pub const fn max_attempts(self) -> u32 {
        self.max_attempts
    }

    pub const fn minutes(self) -> u32 {
        self.minutes
    }

    fn locked_until(self, now: Timestamp) -> Result<Timestamp, LoginError> {
        Ok(Timestamp::try_from(
            now.get() + Duration::minutes(i64::from(self.minutes)),
        )?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptMethod {
    Password,
}

impl AttemptMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptResult {
    Success,
    BadCredentials,
    UnknownIdentifier,
    Inactive,
    LockedOut,
    PasswordAuthDisabled,
}

impl AttemptResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::BadCredentials => "bad_credentials",
            Self::UnknownIdentifier => "unknown_identifier",
            Self::Inactive => "inactive",
            Self::LockedOut => "locked_out",
            Self::PasswordAuthDisabled => "password_auth_disabled",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AttemptClient {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
}

pub struct LoginAttempt<'a> {
    pub identifier: &'a NormalizedIdentifier,
    pub user_id: Option<UserId>,
    pub method: AttemptMethod,
    pub result: AttemptResult,
    pub client: &'a AttemptClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockState {
    pub failed_count: u32,
    pub locked_until: Option<Timestamp>,
    pub lock_count: u32,
}

impl LockState {
    pub fn active_until(&self, now: Timestamp) -> Option<Timestamp> {
        self.locked_until.filter(|until| *until > now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureOutcome {
    Counted(LockState),
    LockedNow(LockState),
}

pub async fn record_attempt(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    attempt: &LoginAttempt<'_>,
) -> Result<(), LoginError> {
    let now = Timestamp::try_from(clock.now())?;
    sqlx::query(INSERT_ATTEMPT)
        .bind(LoginAttemptId::generate(clock).to_string())
        .bind(now.to_string())
        .bind(attempt.identifier.as_str())
        .bind(attempt.user_id.map(|id| id.to_string()))
        .bind(attempt.method.as_str())
        .bind(attempt.result.as_str())
        .bind(bounded(attempt.client.ip.as_deref(), MAX_IP_CHARS))
        .bind(bounded(
            attempt.client.user_agent.as_deref(),
            MAX_USER_AGENT_CHARS,
        ))
        .bind(bounded(
            attempt.client.request_id.as_deref(),
            MAX_REQUEST_ID_CHARS,
        ))
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn state_in_tx(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
) -> Result<Option<LockState>, LoginError> {
    let row = sqlx::query(SELECT_STATE)
        .bind(user_id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(state_from).transpose()
}

pub async fn record_failure(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    now: Timestamp,
    policy: LockoutPolicy,
) -> Result<FailureOutcome, LoginError> {
    let prior = state_in_tx(tx, user_id).await?;
    let row = sqlx::query(RECORD_FAILURE)
        .bind(user_id.to_string())
        .bind(now.to_string())
        .bind(i64::from(policy.max_attempts))
        .bind(policy.locked_until(now)?.to_string())
        .fetch_one(tx.executor())
        .await?;
    let state = state_from(&row)?;
    let was_locked = prior.is_some_and(|prior| prior.active_until(now).is_some());
    if !was_locked && state.active_until(now).is_some() {
        Ok(FailureOutcome::LockedNow(state))
    } else {
        Ok(FailureOutcome::Counted(state))
    }
}

pub async fn reset(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    now: Timestamp,
) -> Result<(), LoginError> {
    sqlx::query(RESET)
        .bind(user_id.to_string())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn clear(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    cleared_by: Option<UserId>,
    now: Timestamp,
) -> Result<bool, LoginError> {
    let updated = sqlx::query(CLEAR)
        .bind(user_id.to_string())
        .bind(now.to_string())
        .bind(cleared_by.map(|id| id.to_string()))
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

fn state_from(row: &SqliteRow) -> Result<LockState, LoginError> {
    let failed_count: i64 = column(row, "failed_count")?;
    let lock_count: i64 = column(row, "lock_count")?;
    let locked_until: Option<String> = column(row, "locked_until")?;
    Ok(LockState {
        failed_count: u32::try_from(failed_count).map_err(|_| invariant("failed_count"))?,
        locked_until: locked_until
            .map(|text| text.parse().map_err(|_| invariant("locked_until")))
            .transpose()?,
        lock_count: u32::try_from(lock_count).map_err(|_| invariant("lock_count"))?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, LoginError>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> LoginError {
    LoginError::RepositoryInvariant { column }
}

fn bounded(value: Option<&str>, max_chars: usize) -> Option<String> {
    value.map(|value| value.chars().take(max_chars).collect())
}

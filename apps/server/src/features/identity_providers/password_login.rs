use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use sqlx::{Row, SqliteConnection};
use time::Duration;
use utoipa::ToSchema;

use super::model::{IdentityLinkId, ProviderId};
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions::{self, ActionSpec};
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::sessions::AuthenticatedPrincipal;
use crate::features::settings::groups::security::{
    AUTH_PROVIDERS_ENABLED_KEY, PASSWORD_LOGIN_ENABLED_KEY,
};
use crate::features::settings::service::SettingValueInput;
use crate::features::settings::{SettingsError, SettingsService};
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, DbPools, ReadPool, WriteTx};
use crate::infra::http::error::{ApiError, BlockerDetail};

pub const DISABLE_TRANSACTION: &str = "identity_providers.password_login.disable";
pub const ENABLE_TRANSACTION: &str = "identity_providers.password_login.enable";
pub const VALIDATION_MAX_AGE: Duration = Duration::hours(24);

const MAX_LISTED_PATHS: i64 = 100;
const TARGET_LABEL: &str = "passwordLoginEnabled";

const BLOCKER_PROVIDERS_DISABLED: BlockerDetail = BlockerDetail {
    code: ErrorCode::PasswordLoginDisableUnsafe.as_str(),
    detail: "External identity providers are disabled for this instance",
};
const BLOCKER_NO_ENABLED_PROVIDER: BlockerDetail = BlockerDetail {
    code: ErrorCode::NoValidatedProvider.as_str(),
    detail: "No identity provider is enabled",
};
const BLOCKER_NO_VALIDATED_PROVIDER: BlockerDetail = BlockerDetail {
    code: ErrorCode::NoValidatedProvider.as_str(),
    detail: "No enabled provider has been successfully tested",
};
const BLOCKER_NO_LINKED_ADMIN: BlockerDetail = BlockerDetail {
    code: ErrorCode::PasswordLoginDisableUnsafe.as_str(),
    detail: "No active Administrator is linked to a validated provider",
};
const BLOCKER_ACTOR_NOT_LINKED: BlockerDetail = BlockerDetail {
    code: ErrorCode::PasswordLoginDisableUnsafe.as_str(),
    detail: "The acting Administrator is not linked to a validated provider",
};
const BLOCKER_LAST_PATH: BlockerDetail = BlockerDetail {
    code: ErrorCode::PasswordLoginDisableUnsafe.as_str(),
    detail: "The change would remove the last usable Administrator external login path",
};

macro_rules! admin_paths {
    () => {
        "FROM identity_links l
         JOIN identity_providers p ON p.id = l.provider_id
         JOIN users u ON u.id = l.user_id
         WHERE l.state = 'active'
           AND p.is_enabled = 1
           AND u.role = 'admin'
           AND u.is_active = 1
           AND (?1 IS NULL OR l.id <> ?1)
           AND (?2 IS NULL OR u.id <> ?2)
           AND (?3 IS NULL OR p.id <> ?3)"
    };
}

const LIST_PATHS: &str = concat!(
    "SELECT u.id AS user_id, u.username, p.key AS provider_slug, p.validated_at,
         p.validation_error ",
    admin_paths!(),
    " ORDER BY u.username_normalized, p.sort_order, p.key LIMIT ?4"
);

const PATH_EXISTS: &str = concat!("SELECT EXISTS (SELECT 1 ", admin_paths!(), ")");

const ENABLED_PROVIDERS: &str =
    "SELECT validated_at, validation_error FROM identity_providers WHERE is_enabled = 1";

const READ_FLAGS: &str = "SELECT key, value_json FROM app_settings WHERE key IN (?1, ?2)";

const REENABLE: &str =
    "UPDATE app_settings SET value_json = 'true', updated_at = ?2, updated_by = NULL
    WHERE key = ?1 AND value_json <> 'true'";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Projection {
    link: Option<IdentityLinkId>,
    user: Option<UserId>,
    provider: Option<ProviderId>,
    providers_disabled: bool,
}

impl Projection {
    pub fn removing_link(link: IdentityLinkId) -> Self {
        Self {
            link: Some(link),
            ..Self::default()
        }
    }

    pub fn removing_admin(user: UserId) -> Self {
        Self {
            user: Some(user),
            ..Self::default()
        }
    }

    pub fn removing_provider(provider: ProviderId) -> Self {
        Self {
            provider: Some(provider),
            ..Self::default()
        }
    }

    pub fn disabling_providers() -> Self {
        Self {
            providers_disabled: true,
            ..Self::default()
        }
    }
}

#[derive(Debug)]
pub enum SsoGuardError {
    Unsafe(Vec<BlockerDetail>),
    Invariant(&'static str),
    Db(DbError),
}

impl SsoGuardError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Unsafe(_) => "password_login_disable_unsafe",
            Self::Invariant(key) => key,
            Self::Db(error) => error.kind().as_str(),
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Unsafe(blockers) => unsafe_error(blockers),
            Self::Invariant(_) => ApiError::internal(),
            Self::Db(error) => ApiError::new(error.api_code()),
        }
    }
}

impl fmt::Display for SsoGuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsafe(_) => f.write_str("the change would leave no safe administrator login"),
            Self::Invariant(key) => write!(f, "the setting {key} holds an invalid value"),
            Self::Db(error) => write!(f, "password login guard failed: {error}"),
        }
    }
}

impl std::error::Error for SsoGuardError {}

impl From<DbError> for SsoGuardError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for SsoGuardError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

pub fn unsafe_error(blockers: &[BlockerDetail]) -> ApiError {
    ApiError::new(ErrorCode::PasswordLoginDisableUnsafe).with_detail("blockers", blockers.to_vec())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flags {
    password_login_enabled: bool,
    auth_providers_enabled: bool,
}

async fn read_flags(conn: &mut SqliteConnection) -> Result<Flags, SsoGuardError> {
    let rows = sqlx::query(READ_FLAGS)
        .bind(PASSWORD_LOGIN_ENABLED_KEY)
        .bind(AUTH_PROVIDERS_ENABLED_KEY)
        .fetch_all(&mut *conn)
        .await?;
    let mut flags = Flags {
        password_login_enabled: true,
        auth_providers_enabled: true,
    };
    for row in rows {
        let key: String = row.try_get("key")?;
        let json: Option<String> = row.try_get("value_json")?;
        let value = json
            .and_then(|json| serde_json::from_str::<bool>(&json).ok())
            .ok_or(SsoGuardError::Invariant("password_login_setting_invalid"))?;
        if key == PASSWORD_LOGIN_ENABLED_KEY {
            flags.password_login_enabled = value;
        } else {
            flags.auth_providers_enabled = value;
        }
    }
    Ok(flags)
}

struct PathRow {
    user_id: String,
    username: String,
    provider_slug: String,
    validated_at: Option<Timestamp>,
    validation_error: Option<String>,
}

impl PathRow {
    fn qualifies_for_admission(&self, now: Timestamp) -> bool {
        self.validation_error.is_none()
            && self
                .validated_at
                .is_some_and(|validated_at| is_recent(validated_at, now))
    }
}

async fn list_paths(
    conn: &mut SqliteConnection,
    projection: Projection,
) -> Result<Vec<PathRow>, SsoGuardError> {
    let rows = sqlx::query(LIST_PATHS)
        .bind(projection.link.map(|id| id.to_string()))
        .bind(projection.user.map(|id| id.to_string()))
        .bind(projection.provider.map(|id| id.to_string()))
        .bind(MAX_LISTED_PATHS)
        .fetch_all(&mut *conn)
        .await?;
    rows.iter()
        .map(|row| {
            let validated_at: Option<String> = row.try_get("validated_at")?;
            Ok(PathRow {
                user_id: row.try_get("user_id")?,
                username: row.try_get("username")?,
                provider_slug: row.try_get("provider_slug")?,
                validated_at: validated_at
                    .map(|at| {
                        at.parse()
                            .map_err(|_| SsoGuardError::Invariant("provider_validated_at_invalid"))
                    })
                    .transpose()?,
                validation_error: row.try_get("validation_error")?,
            })
        })
        .collect()
}

async fn path_remains(
    conn: &mut SqliteConnection,
    projection: Projection,
) -> Result<bool, SsoGuardError> {
    let remains: bool = sqlx::query_scalar(PATH_EXISTS)
        .bind(projection.link.map(|id| id.to_string()))
        .bind(projection.user.map(|id| id.to_string()))
        .bind(projection.provider.map(|id| id.to_string()))
        .fetch_one(&mut *conn)
        .await?;
    Ok(remains)
}

pub async fn assert_safe_sso_after_change(
    tx: &mut WriteTx<'_>,
    projection: Projection,
) -> Result<(), SsoGuardError> {
    let conn = tx.executor();
    let flags = read_flags(conn).await?;
    if flags.password_login_enabled {
        return Ok(());
    }
    if !flags.auth_providers_enabled || projection.providers_disabled {
        return Err(SsoGuardError::Unsafe(vec![BLOCKER_PROVIDERS_DISABLED]));
    }
    if path_remains(conn, projection).await? {
        Ok(())
    } else {
        Err(SsoGuardError::Unsafe(vec![BLOCKER_LAST_PATH]))
    }
}

pub async fn reenable_in_tx(tx: &mut WriteTx<'_>, at: Timestamp) -> Result<bool, DbError> {
    let updated = sqlx::query(REENABLE)
        .bind(PASSWORD_LOGIN_ENABLED_KEY)
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() > 0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SafeAdminPath {
    #[schema(example = "0192f3a1-0000-7000-8000-000000000001")]
    pub user_id: String,
    pub username: String,
    pub provider_slug: String,
    pub provider_validated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasswordLoginState {
    pub password_login_enabled: bool,
    pub can_disable: bool,
    pub blockers: Vec<BlockerDetail>,
    pub safe_admin_login_paths: Vec<SafeAdminPath>,
}

fn is_recent(validated_at: Timestamp, now: Timestamp) -> bool {
    now.get() - validated_at.get() <= VALIDATION_MAX_AGE
}

async fn evaluate(
    conn: &mut SqliteConnection,
    now: Timestamp,
    actor: UserId,
) -> Result<PasswordLoginState, SsoGuardError> {
    let flags = read_flags(conn).await?;
    let paths = if flags.auth_providers_enabled {
        list_paths(conn, Projection::default()).await?
    } else {
        Vec::new()
    };
    let mut blockers = Vec::new();
    if flags.password_login_enabled {
        blockers = admission_blockers(conn, flags, &paths, now, actor).await?;
    }
    let safe_admin_login_paths = paths
        .iter()
        .map(|path| SafeAdminPath {
            user_id: path.user_id.clone(),
            username: path.username.clone(),
            provider_slug: path.provider_slug.clone(),
            provider_validated: path.qualifies_for_admission(now),
        })
        .collect();
    Ok(PasswordLoginState {
        password_login_enabled: flags.password_login_enabled,
        can_disable: flags.password_login_enabled && blockers.is_empty(),
        blockers,
        safe_admin_login_paths,
    })
}

async fn admission_blockers(
    conn: &mut SqliteConnection,
    flags: Flags,
    paths: &[PathRow],
    now: Timestamp,
    actor: UserId,
) -> Result<Vec<BlockerDetail>, SsoGuardError> {
    if !flags.auth_providers_enabled {
        return Ok(vec![BLOCKER_PROVIDERS_DISABLED]);
    }
    let providers = sqlx::query(ENABLED_PROVIDERS).fetch_all(&mut *conn).await?;
    if providers.is_empty() {
        return Ok(vec![BLOCKER_NO_ENABLED_PROVIDER]);
    }
    let mut validated = false;
    for provider in &providers {
        let validated_at: Option<String> = provider.try_get("validated_at")?;
        let error: Option<String> = provider.try_get("validation_error")?;
        if let (Some(at), None) = (validated_at, error) {
            let at: Timestamp = at
                .parse()
                .map_err(|_| SsoGuardError::Invariant("provider_validated_at_invalid"))?;
            validated |= is_recent(at, now);
        }
    }
    if !validated {
        return Ok(vec![BLOCKER_NO_VALIDATED_PROVIDER]);
    }
    let actor = actor.to_string();
    let mut recent = paths
        .iter()
        .filter(|path| path.qualifies_for_admission(now));
    if recent.clone().next().is_none() {
        return Ok(vec![BLOCKER_NO_LINKED_ADMIN]);
    }
    if !recent.any(|path| path.user_id == actor) {
        return Ok(vec![BLOCKER_ACTOR_NOT_LINKED]);
    }
    Ok(Vec::new())
}

#[derive(Debug)]
pub enum PasswordLoginError {
    Guard(SsoGuardError),
    Settings(SettingsError),
    Audit(AuditError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl PasswordLoginError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Guard(error) => error.kind(),
            Self::Settings(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "password_login_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Guard(error) => error.api_error(),
            Self::Db(error)
            | Self::Settings(SettingsError::Db(error))
            | Self::Audit(AuditError::Db(error)) => ApiError::new(error.api_code()),
            Self::Settings(_) | Self::Audit(_) | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for PasswordLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Guard(error) => write!(f, "{error}"),
            Self::Settings(error) => write!(f, "password login setting failed: {error}"),
            Self::Audit(error) => write!(f, "password login audit failed: {error}"),
            Self::Db(error) => write!(f, "password login database operation failed: {error}"),
            Self::Time(error) => write!(f, "password login timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for PasswordLoginError {}

impl From<SsoGuardError> for PasswordLoginError {
    fn from(error: SsoGuardError) -> Self {
        Self::Guard(error)
    }
}

impl From<SettingsError> for PasswordLoginError {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

impl From<AuditError> for PasswordLoginError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<DbError> for PasswordLoginError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for PasswordLoginError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<InvalidTimestamp> for PasswordLoginError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

#[derive(Clone)]
pub struct PasswordLoginService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsService,
    audit: AuditService,
}

impl fmt::Debug for PasswordLoginService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordLoginService")
            .finish_non_exhaustive()
    }
}

impl PasswordLoginService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsService,
        audit: AuditService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            audit,
        }
    }

    pub async fn state(&self, actor: UserId) -> Result<PasswordLoginState, PasswordLoginError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let reader: &ReadPool = self.pools.reader();
        let mut connection = reader.executor().acquire().await?;
        Ok(evaluate(&mut connection, now, actor).await?)
    }

    pub async fn set(
        &self,
        admin: &AuthenticatedPrincipal,
        enabled: bool,
        client: &ClientMetadata,
    ) -> Result<PasswordLoginState, PasswordLoginError> {
        let name = if enabled {
            ENABLE_TRANSACTION
        } else {
            DISABLE_TRANSACTION
        };
        self.settings
            .update_group(name, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                let current = evaluate(tx.executor(), now, admin.user_id).await?;
                if current.password_login_enabled == enabled {
                    return Ok(current);
                }
                if !enabled && !current.can_disable {
                    return Err(SsoGuardError::Unsafe(current.blockers).into());
                }
                let admin_id = admin.user_id.to_string();
                self.settings
                    .write_setting(
                        tx,
                        PASSWORD_LOGIN_ENABLED_KEY,
                        SettingValueInput::Boolean(enabled),
                        Some(&admin_id),
                    )
                    .await?;
                let count = current
                    .safe_admin_login_paths
                    .iter()
                    .filter(|path| path.provider_validated)
                    .count();
                let spec: ActionSpec = if enabled {
                    actions::password_login_enabled(count)
                } else {
                    actions::password_login_disabled(count)
                };
                let event = AuditEvent::new(
                    spec,
                    Actor::user(&admin_id, &admin.username),
                    Outcome::Success,
                    now,
                )
                .with_target(
                    Target::new(TargetType::Setting)
                        .id(PASSWORD_LOGIN_ENABLED_KEY)
                        .label(TARGET_LABEL),
                )
                .with_client(client.clone());
                self.audit.record_in_tx(tx, &event).await?;
                Ok::<_, PasswordLoginError>(evaluate(tx.executor(), now, admin.user_id).await?)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn stamp(at: time::OffsetDateTime) -> Timestamp {
        Timestamp::try_from(at).unwrap()
    }

    #[test]
    fn unit_validation_window_is_inclusive_at_twenty_four_hours() {
        let now = stamp(datetime!(2026-10-05 12:00:00 UTC));
        let ago = |seconds: i64| stamp(now.get() - Duration::seconds(seconds));

        assert!(is_recent(ago(0), now));
        assert!(is_recent(ago(24 * 3600 - 1), now));
        assert!(is_recent(ago(24 * 3600), now));
        assert!(!is_recent(ago(24 * 3600 + 1), now));
        assert!(!is_recent(ago(7 * 24 * 3600), now));
        assert!(is_recent(stamp(now.get() + Duration::seconds(30)), now));
    }

    #[test]
    fn unit_blockers_carry_only_accepted_codes_and_fixed_text() {
        for blocker in [
            BLOCKER_PROVIDERS_DISABLED,
            BLOCKER_NO_ENABLED_PROVIDER,
            BLOCKER_NO_VALIDATED_PROVIDER,
            BLOCKER_NO_LINKED_ADMIN,
            BLOCKER_ACTOR_NOT_LINKED,
            BLOCKER_LAST_PATH,
        ] {
            assert!(matches!(
                blocker.code,
                "NO_VALIDATED_PROVIDER" | "PASSWORD_LOGIN_DISABLE_UNSAFE"
            ));
            assert!(!blocker.detail.is_empty() && blocker.detail.len() <= 120);
        }
        let error = unsafe_error(&[BLOCKER_LAST_PATH]);
        assert_eq!(error.code(), ErrorCode::PasswordLoginDisableUnsafe);
        assert_eq!(error.status(), http::StatusCode::CONFLICT);
        assert!(!error.retryable());
    }

    #[test]
    fn unit_guard_errors_map_to_canonical_codes() {
        assert_eq!(
            SsoGuardError::Unsafe(vec![BLOCKER_LAST_PATH])
                .api_error()
                .code(),
            ErrorCode::PasswordLoginDisableUnsafe
        );
        assert_eq!(
            SsoGuardError::Invariant("password_login_setting_invalid")
                .api_error()
                .code(),
            ErrorCode::InternalError
        );
    }
}

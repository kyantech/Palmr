use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::secret::Secret;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions;
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::error::LoginError;
use crate::features::auth::login::{password_login_enabled, CredentialProof};
use crate::features::auth::model::{AccountView, MeUser};
use crate::features::auth::repo as accounts;
use crate::features::auth::sessions::{
    AuthenticatedPrincipal, MintedSession, PreparedSessionCredentials, RevokedReason, SessionError,
    SessionRestriction,
};
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::auth::AuthService;
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::password::hash_password;
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};
use crate::storage::caps::StorageCapabilities;

use super::error::UserError;
use super::model::{display_text, User, UserId};
use super::preferences::{self, Preferences, PreferencesChange, PreferencesRequest};
use super::repo as users;
use super::service::{effective_quota, AccountPasswordPolicy};

pub const PROFILE_UPDATE_TRANSACTION: &str = "users.profile_update";
pub const PREFERENCES_UPDATE_TRANSACTION: &str = "users.preferences_update";
pub const PASSWORD_CHANGE_TRANSACTION: &str = "users.password_change";

pub const MAX_SAFE_JSON_INTEGER: u64 = (1 << 53) - 1;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileRequest {
    #[schema(example = "Ada")]
    pub first_name: Option<String>,
    #[schema(example = "Lovelace")]
    pub last_name: Option<String>,
}

impl JsonRequest for ProfileRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::optional("firstName", JsonKind::String),
        JsonField::optional("lastName", JsonKind::String),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChange {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

impl ProfileChange {
    pub fn parse(request: ProfileRequest) -> Result<Self, ProfileError> {
        let mut invalid = Vec::new();
        let mut name = |value: Option<String>, field: &'static str| {
            let value = value?;
            let parsed = display_text(&value);
            if parsed.is_none() {
                invalid.push(field);
            }
            parsed
        };
        let first_name = name(request.first_name, "firstName");
        let last_name = name(request.last_name, "lastName");
        if invalid.is_empty() {
            Ok(Self {
                first_name,
                last_name,
            })
        } else {
            Err(ProfileError::Invalid { fields: invalid })
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordChangeRequest {
    #[schema(format = Password)]
    pub current_password: String,
    #[schema(format = Password)]
    pub new_password: String,
}

impl JsonRequest for PasswordChangeRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("currentPassword", JsonKind::String),
        JsonField::required("newPassword", JsonKind::String),
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    #[schema(maximum = 9_007_199_254_740_991_u64, example = 4_831_838_208_u64)]
    pub used_bytes: u64,
    #[schema(maximum = 9_007_199_254_740_991_u64, example = 3_221_225_472_u64)]
    pub my_files_bytes: u64,
    #[schema(maximum = 9_007_199_254_740_991_u64, example = 1_610_612_736_u64)]
    pub received_bytes: u64,
    #[schema(maximum = 9_007_199_254_740_991_u64, example = 268_435_456_u64)]
    pub reserved_bytes: u64,
    #[schema(required = true, maximum = 9_007_199_254_740_991_u64)]
    pub quota_bytes: Option<u64>,
    #[schema(required = true, maximum = 9_007_199_254_740_991_u64)]
    pub effective_max_file_size_bytes: Option<u64>,
    pub used_bytes_exact: bool,
}

#[derive(Debug, Default)]
struct Clamp {
    clamped: bool,
}

impl Clamp {
    fn bytes(&mut self, value: ByteSize) -> u64 {
        let value = value.get();
        if value > MAX_SAFE_JSON_INTEGER {
            self.clamped = true;
            MAX_SAFE_JSON_INTEGER
        } else {
            value
        }
    }
}

fn safe_limit(limit: ByteSize) -> u64 {
    limit.get().min(MAX_SAFE_JSON_INTEGER)
}

#[derive(Debug)]
pub enum ProfileError {
    Invalid { fields: Vec<&'static str> },
    CurrentPasswordInvalid,
    PasswordLoginDisabled,
    HashTask,
    Account(LoginError),
    User(UserError),
    Session(SessionError),
    Audit(AuditError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl ProfileError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "profile_invalid",
            Self::CurrentPasswordInvalid => "profile_current_password_invalid",
            Self::PasswordLoginDisabled => "profile_password_login_disabled",
            Self::HashTask => "profile_hash_task_failed",
            Self::Account(error) => error.kind(),
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Crypto(_) => "profile_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "profile_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::CurrentPasswordInvalid => ApiError::new(ErrorCode::PasswordCurrentInvalid),
            Self::PasswordLoginDisabled => ApiError::new(ErrorCode::AuthPasswordLoginDisabled),
            Self::User(UserError::PasswordPolicyViolation { min_length }) => {
                ApiError::new(ErrorCode::PasswordPolicyViolation)
                    .with_detail("minLength", i64::from(*min_length))
            }
            Self::Account(error) => error.api_error(),
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Audit(AuditError::Db(error))
            | Self::Session(SessionError::Db(error)) => ApiError::new(error.api_code()),
            Self::User(UserError::NotFound) => ApiError::new(ErrorCode::AuthRequired),
            Self::Session(error) => error.api_error(),
            Self::HashTask | Self::User(_) | Self::Audit(_) | Self::Crypto(_) | Self::Time(_) => {
                ApiError::internal()
            }
        }
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { fields } => {
                write!(f, "the profile fields {} are invalid", fields.join(", "))
            }
            Self::CurrentPasswordInvalid => f.write_str("the current password did not verify"),
            Self::PasswordLoginDisabled => f.write_str("password login is disabled"),
            Self::HashTask => f.write_str("the password hashing task did not complete"),
            Self::Account(error) => write!(f, "profile account read failed: {error}"),
            Self::User(error) => write!(f, "profile user operation failed: {error}"),
            Self::Session(error) => write!(f, "profile session operation failed: {error}"),
            Self::Audit(error) => write!(f, "profile audit record failed: {error}"),
            Self::Crypto(error) => write!(f, "profile credential operation failed: {error}"),
            Self::Db(error) => write!(f, "profile database operation failed: {error}"),
            Self::Time(error) => write!(f, "profile timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl From<LoginError> for ProfileError {
    fn from(error: LoginError) -> Self {
        Self::Account(error)
    }
}

impl From<UserError> for ProfileError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SessionError> for ProfileError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<AuditError> for ProfileError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<CryptoError> for ProfileError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for ProfileError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<InvalidTimestamp> for ProfileError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

struct PasswordChange {
    current: Secret<String>,
    replacement: Secret<String>,
}

pub(crate) struct VerifiedChange<'a> {
    pub(crate) principal: &'a AuthenticatedPrincipal,
    pub(crate) verified: &'a Secret<String>,
    pub(crate) replacement: &'a Secret<String>,
    pub(crate) credentials: &'a PreparedSessionCredentials,
    pub(crate) client: &'a ClientMetadata,
}

#[derive(Clone)]
pub struct ProfileService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    auth: AuthService,
    audit: AuditService,
}

impl ProfileService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        auth: AuthService,
        audit: AuditService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            auth,
            audit,
        }
    }

    pub const fn auth(&self) -> &AuthService {
        &self.auth
    }

    pub async fn profile(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<MeUser, ProfileError> {
        Ok(MeUser::from(self.account(principal.user_id).await?))
    }

    pub async fn update_profile(
        &self,
        principal: &AuthenticatedPrincipal,
        change: ProfileChange,
    ) -> Result<MeUser, ProfileError> {
        if change.first_name.is_some() || change.last_name.is_some() {
            self.pools
                .write_tx(
                    self.clock.as_ref(),
                    PROFILE_UPDATE_TRANSACTION,
                    async |tx| {
                        users::update_names(
                            tx,
                            self.clock.as_ref(),
                            principal.user_id,
                            change.first_name.as_deref(),
                            change.last_name.as_deref(),
                        )
                        .await
                        .map_err(ProfileError::from)
                    },
                )
                .await?;
        }
        self.profile(principal).await
    }

    pub async fn preferences(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<Preferences, ProfileError> {
        Ok(Preferences::from(self.account(principal.user_id).await?))
    }

    pub async fn update_preferences(
        &self,
        principal: &AuthenticatedPrincipal,
        request: PreferencesRequest,
    ) -> Result<Preferences, ProfileError> {
        let change =
            PreferencesChange::parse(request).map_err(|fields| ProfileError::Invalid { fields })?;
        if !change.is_empty() {
            self.pools
                .write_tx(
                    self.clock.as_ref(),
                    PREFERENCES_UPDATE_TRANSACTION,
                    async |tx| {
                        preferences::apply(tx, self.clock.as_ref(), principal.user_id, change)
                            .await
                            .map_err(ProfileError::from)
                    },
                )
                .await?;
        }
        self.preferences(principal).await
    }

    pub async fn usage(
        &self,
        principal: &AuthenticatedPrincipal,
        storage: &StorageCapabilities,
    ) -> Result<UsageResponse, ProfileError> {
        let row = users::usage(self.pools.reader(), principal.user_id)
            .await?
            .ok_or(ProfileError::Session(SessionError::AuthRequired))?;
        let settings = self.settings.load();
        let quota = effective_quota(row.quota, settings.quotas.default_user_quota_bytes);
        let max_file_size = storage.effective_max_file_size(settings.quotas.max_file_size_bytes);
        drop(settings);
        let mut clamp = Clamp::default();
        Ok(UsageResponse {
            used_bytes: clamp.bytes(row.used_bytes),
            my_files_bytes: clamp.bytes(row.my_files_bytes),
            received_bytes: clamp.bytes(row.received_bytes),
            reserved_bytes: clamp.bytes(row.reserved_bytes),
            quota_bytes: quota.map(safe_limit),
            effective_max_file_size_bytes: max_file_size.map(safe_limit),
            used_bytes_exact: !clamp.clamped,
        })
    }

    pub async fn change_password(
        &self,
        principal: &AuthenticatedPrincipal,
        request: PasswordChangeRequest,
        client: &ClientMetadata,
    ) -> Result<MintedSession, ProfileError> {
        if !authorized_for_password_change(principal, principal.restriction) {
            return Err(recent_auth_required(principal));
        }
        let change = PasswordChange {
            current: Secret::new(request.current_password),
            replacement: Secret::new(request.new_password),
        };
        if change.current.expose_secret().is_empty() {
            return Err(ProfileError::Invalid {
                fields: vec!["currentPassword"],
            });
        }
        let settings = self.settings.load();
        let enabled = password_login_enabled(&settings);
        let policy = AccountPasswordPolicy::from_settings(&settings);
        drop(settings);
        if !enabled {
            return Err(ProfileError::PasswordLoginDisabled);
        }
        policy.check(change.replacement.expose_secret())?;

        let user = self.active_user(principal.user_id).await?;
        let stored = user.password_hash.clone();
        let proof = self
            .auth
            .verify_off_runtime(change.current, stored.clone())
            .await?;
        let verified = match (stored, proof) {
            (Some(stored), CredentialProof::Verified { .. }) => stored,
            _ => return Err(ProfileError::CurrentPasswordInvalid),
        };
        let replacement = hash_off_runtime(change.replacement).await?;
        let credentials = self.auth.sessions().prepare_credentials()?;
        self.pools
            .write_tx(
                self.clock.as_ref(),
                PASSWORD_CHANGE_TRANSACTION,
                async |tx| {
                    self.commit_password_change(
                        tx,
                        VerifiedChange {
                            principal,
                            verified: &verified,
                            replacement: &replacement,
                            credentials: &credentials,
                            client,
                        },
                    )
                    .await
                },
            )
            .await
    }

    pub(crate) async fn commit_password_change(
        &self,
        tx: &mut WriteTx<'_>,
        change: VerifiedChange<'_>,
    ) -> Result<MintedSession, ProfileError> {
        let VerifiedChange {
            principal,
            verified,
            replacement,
            credentials,
            client,
        } = change;
        let now = Timestamp::try_from(self.clock.now())?;
        let current = users::find_by_id_in_tx(tx, principal.user_id)
            .await?
            .filter(|user| user.is_active)
            .ok_or(ProfileError::Session(SessionError::AuthRequired))?;
        let restriction = if current.must_change_password {
            SessionRestriction::MustChangePassword
        } else {
            SessionRestriction::None
        };
        if !authorized_for_password_change(principal, restriction) {
            return Err(recent_auth_required(principal));
        }
        if !users::change_password(tx, current.id, verified, replacement, now).await? {
            return Err(ProfileError::CurrentPasswordInvalid);
        }
        let sessions = self.auth.sessions();
        let sessions_revoked = sessions
            .revoke_all_others_in_tx(
                tx,
                current.id,
                principal.session_id,
                RevokedReason::PasswordChanged,
            )
            .await?;
        let devices_revoked = trusted_devices::revoke_all_in_tx(tx, current.id, now).await?;
        let minted = sessions
            .rotate_in_tx(tx, principal.session_id, credentials)
            .await?;
        let event = AuditEvent::new(
            actions::password_changed(
                current.must_change_password,
                sessions_revoked,
                devices_revoked,
            ),
            Actor::user(&current.id.to_string(), &current.username),
            Outcome::Success,
            now,
        )
        .with_target(
            Target::new(TargetType::User)
                .id(&current.id.to_string())
                .label(&current.username),
        )
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(minted)
    }

    async fn account(&self, user_id: UserId) -> Result<AccountView, ProfileError> {
        accounts::account(self.pools.reader().executor(), user_id)
            .await?
            .filter(|account| account.is_active)
            .ok_or(ProfileError::Session(SessionError::AuthRequired))
    }

    async fn active_user(&self, user_id: UserId) -> Result<User, ProfileError> {
        users::find_by_id(self.pools.reader(), user_id)
            .await?
            .filter(|user| user.is_active)
            .ok_or(ProfileError::Session(SessionError::AuthRequired))
    }
}

fn authorized_for_password_change(
    principal: &AuthenticatedPrincipal,
    restriction: SessionRestriction,
) -> bool {
    principal.recent_auth
        || (principal.restriction == SessionRestriction::MustChangePassword
            && restriction == SessionRestriction::MustChangePassword)
}

fn recent_auth_required(principal: &AuthenticatedPrincipal) -> ProfileError {
    ProfileError::Session(SessionError::RecentAuthRequired {
        method: principal.auth_method.recent_auth_hint(),
    })
}

async fn hash_off_runtime(password: Secret<String>) -> Result<Secret<String>, ProfileError> {
    tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
        .map_err(|_| ProfileError::HashTask)?
        .map_err(ProfileError::from)
}

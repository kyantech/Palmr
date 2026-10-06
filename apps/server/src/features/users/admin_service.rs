use std::fmt;
use std::sync::Arc;

use http::StatusCode;

use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::audit::actions::{
    self, QuotaOverrideChangedFacts, UserCreatedFacts, UserDeactivatedFacts,
    UserPasswordResetByAdminFacts, UserRoleChangedFacts,
};
use crate::features::audit::error::AuditError;
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::error::LoginError;
use crate::features::auth::lockout;
use crate::features::auth::login::password_login_enabled;
use crate::features::auth::password_reset::error::PasswordResetError;
use crate::features::auth::password_reset::repo as reset_links;
use crate::features::auth::sessions::{
    AuthenticatedPrincipal, RevokedReason, SessionError, SessionItem, SessionService,
};
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::auth::trusted_devices::TrustedDeviceError;
use crate::features::identity_providers::password_login::{
    assert_safe_sso_after_change, Projection, SsoGuardError,
};
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::crypto::password::hash_password;
use crate::infra::crypto::CryptoError;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::idempotency::{
    Claim, IdempotencyError, IdempotencyService, ReplayEnvelope,
};
use crate::infra::http::pagination::{
    invalid_param, CursorKey, Page, PageRequest, QueryParams, SearchQuery, SortAllowlist,
    SortDirection, SortField, SortKeyKind, SortValue, TotalCount,
};

use super::admin_input::{CreateInput, InitialCredential, UpdateInput};
use super::admin_model::{
    AdminUserDetail, AdminUserItem, AdminUserQuota, AdminUserRecord, DetailParts, ResourceCounts,
    UserStatus,
};
use super::admin_repo::{self as repo, SearchPrefix, UserFilter};
use super::error::UserError;
use super::lifecycle;
use super::model::{NewUser, QuotaOverride, User, UserId};
use super::repo as users;
use super::service::{assert_active_admin_remains, AccountPasswordPolicy};

pub const CREATE_TRANSACTION: &str = "users.admin_create";
pub const UPDATE_TRANSACTION: &str = "users.admin_update";
pub const ROLE_TRANSACTION: &str = "users.admin_role";
pub const DEACTIVATE_TRANSACTION: &str = "users.admin_deactivate";
pub const ACTIVATE_TRANSACTION: &str = "users.admin_activate";
pub const PASSWORD_RESET_TRANSACTION: &str = "users.admin_password_reset";
pub const UNLOCK_TRANSACTION: &str = "users.admin_unlock";
pub const REVOKE_SESSIONS_TRANSACTION: &str = "users.admin_revoke_sessions";
pub const QUOTA_TRANSACTION: &str = "users.admin_quota";

pub const ROLE_PARAM: &str = "role";
pub const STATUS_PARAM: &str = "status";

pub(super) static USER_SORT_FIELDS: [SortField; 4] = [
    SortField::new("createdAt", "u.created_at", SortKeyKind::Text),
    SortField::new("usedBytes", "u.used_bytes", SortKeyKind::Integer),
    SortField::new("username", "u.username_normalized", SortKeyKind::Text),
    SortField::new("email", "u.email_normalized", SortKeyKind::Text),
];
pub static USER_SORT: SortAllowlist =
    SortAllowlist::new(&USER_SORT_FIELDS, 0, SortDirection::Desc).with_id_column("u.id");

#[derive(Debug)]
pub enum AdminUserError {
    NotFound,
    Invalid { fields: Vec<&'static str> },
    HashTask,
    NoLocalAuth,
    PasswordLoginDisabled,
    User(UserError),
    SsoGuard(SsoGuardError),
    Login(LoginError),
    ResetLinks(PasswordResetError),
    Session(SessionError),
    TrustedDevice(TrustedDeviceError),
    Audit(AuditError),
    Idempotency(IdempotencyError),
    Crypto(CryptoError),
    Db(DbError),
    Time(InvalidTimestamp),
}

impl AdminUserError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "admin_user_not_found",
            Self::Invalid { .. } => "admin_user_invalid",
            Self::HashTask => "admin_user_hash_task_failed",
            Self::NoLocalAuth => "admin_user_no_local_auth",
            Self::PasswordLoginDisabled => "admin_user_password_login_disabled",
            Self::User(error) => error.kind(),
            Self::SsoGuard(error) => error.kind(),
            Self::Login(error) => error.kind(),
            Self::ResetLinks(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::TrustedDevice(error) => error.kind(),
            Self::Audit(error) => error.kind(),
            Self::Idempotency(error) => error.kind(),
            Self::Crypto(_) => "admin_user_crypto",
            Self::Db(error) => error.kind().as_str(),
            Self::Time(_) => "admin_user_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound | Self::User(UserError::NotFound) => {
                ApiError::new(ErrorCode::UserNotFound)
            }
            Self::User(UserError::LastAdminProtected) => {
                ApiError::new(ErrorCode::LastAdminProtected)
            }
            Self::NoLocalAuth => ApiError::new(ErrorCode::UserHasNoLocalAuth),
            Self::PasswordLoginDisabled => ApiError::new(ErrorCode::AuthPasswordLoginDisabled),
            Self::SsoGuard(error) => error.api_error(),
            Self::Login(error) => error.api_error(),
            Self::ResetLinks(error) => error.api_error(),
            Self::Invalid { fields } => ApiError::validation(fields.iter().copied()),
            Self::User(UserError::EmailTaken) => ApiError::new(ErrorCode::UserEmailTaken),
            Self::User(UserError::UsernameTaken) => ApiError::new(ErrorCode::UserUsernameTaken),
            Self::User(UserError::PasswordPolicyViolation { min_length }) => {
                ApiError::new(ErrorCode::PasswordPolicyViolation)
                    .with_detail("minLength", i64::from(*min_length))
            }
            Self::Idempotency(error) => ApiError::new(error.api_code()),
            Self::User(UserError::Db(error))
            | Self::Db(error)
            | Self::Audit(AuditError::Db(error)) => ApiError::new(error.api_code()),
            Self::Session(error) => error.api_error(),
            Self::TrustedDevice(error) => error.api_error(),
            Self::HashTask | Self::User(_) | Self::Audit(_) | Self::Crypto(_) | Self::Time(_) => {
                ApiError::internal()
            }
        }
    }
}

impl fmt::Display for AdminUserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the user does not exist"),
            Self::Invalid { fields } => {
                write!(f, "the user fields {} are invalid", fields.join(", "))
            }
            Self::HashTask => f.write_str("the password hashing task did not complete"),
            Self::NoLocalAuth => f.write_str("the user has no local password"),
            Self::PasswordLoginDisabled => f.write_str("password login is disabled"),
            Self::User(error) => write!(f, "admin user operation failed: {error}"),
            Self::SsoGuard(error) => write!(f, "{error}"),
            Self::Login(error) => write!(f, "admin user lockout operation failed: {error}"),
            Self::ResetLinks(error) => {
                write!(f, "admin user reset-link operation failed: {error}")
            }
            Self::Session(error) => write!(f, "admin user session read failed: {error}"),
            Self::TrustedDevice(error) => {
                write!(f, "admin user trusted-device read failed: {error}")
            }
            Self::Audit(error) => write!(f, "admin user audit record failed: {error}"),
            Self::Idempotency(error) => write!(f, "admin user replay record failed: {error}"),
            Self::Crypto(error) => write!(f, "admin user credential operation failed: {error}"),
            Self::Db(error) => write!(f, "admin user database operation failed: {error}"),
            Self::Time(error) => write!(f, "admin user timestamp is out of range: {error}"),
        }
    }
}

impl std::error::Error for AdminUserError {}

impl From<UserError> for AdminUserError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<SsoGuardError> for AdminUserError {
    fn from(error: SsoGuardError) -> Self {
        Self::SsoGuard(error)
    }
}

impl From<LoginError> for AdminUserError {
    fn from(error: LoginError) -> Self {
        Self::Login(error)
    }
}

impl From<PasswordResetError> for AdminUserError {
    fn from(error: PasswordResetError) -> Self {
        Self::ResetLinks(error)
    }
}

impl From<SessionError> for AdminUserError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<TrustedDeviceError> for AdminUserError {
    fn from(error: TrustedDeviceError) -> Self {
        Self::TrustedDevice(error)
    }
}

impl From<AuditError> for AdminUserError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error)
    }
}

impl From<IdempotencyError> for AdminUserError {
    fn from(error: IdempotencyError) -> Self {
        Self::Idempotency(error)
    }
}

impl From<CryptoError> for AdminUserError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<DbError> for AdminUserError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<InvalidTimestamp> for AdminUserError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

pub struct UserListQuery {
    pub page: PageRequest,
    pub filter: UserFilter,
}

#[derive(Clone)]
pub struct AdminUserService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    keys: Arc<KeyRing>,
    sessions: SessionService,
    audit: AuditService,
    idempotency: IdempotencyService,
}

impl AdminUserService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        keys: Arc<KeyRing>,
        sessions: SessionService,
        audit: AuditService,
    ) -> Self {
        let idempotency =
            IdempotencyService::new(pools.clone(), Arc::clone(&clock), Arc::clone(&keys));
        Self {
            pools,
            clock,
            settings,
            keys,
            sessions,
            audit,
            idempotency,
        }
    }

    pub const fn idempotency(&self) -> &IdempotencyService {
        &self.idempotency
    }

    pub fn query(&self, raw_query: Option<&str>) -> Result<UserListQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        let role = params
            .single(ROLE_PARAM)?
            .map(|raw| raw.parse::<Role>().map_err(|_| invalid_param(ROLE_PARAM)))
            .transpose()?;
        let status = params
            .single(STATUS_PARAM)?
            .map(|raw| UserStatus::parse(raw).ok_or_else(|| invalid_param(STATUS_PARAM)))
            .transpose()?;
        let search = SearchPrefix::parse(SearchQuery::parse(params.single("q")?)?.as_ref())?;
        let page = PageRequest::from_query(&params, &USER_SORT, self.keys.as_ref())?;
        Ok(UserListQuery {
            page,
            filter: UserFilter {
                role,
                status,
                search,
            },
        })
    }

    pub fn session_page_request(&self, raw_query: Option<&str>) -> Result<PageRequest, ApiError> {
        self.sessions.page_request(raw_query)
    }

    pub async fn list(&self, query: UserListQuery) -> Result<Page<AdminUserItem>, AdminUserError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let reader = self.pools.reader();
        let (records, total) = repo::list(reader, &query.filter, &query.page).await?;
        let page = query.page.into_page(
            records,
            self.keys.as_ref(),
            cursor_key,
            TotalCount::Exact(total),
        );
        let ids: Vec<UserId> = page.items.iter().map(|record| record.id).collect();
        let counts = repo::resource_counts(reader, &ids).await?;
        let instance_default = self.settings.load().quotas.default_user_quota_bytes;
        let items = page
            .items
            .iter()
            .map(|record| {
                let counts = counts.get(&record.id).copied().unwrap_or_default();
                AdminUserItem::new(record, counts, instance_default, now)
            })
            .collect();
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn detail(&self, id: UserId) -> Result<AdminUserDetail, AdminUserError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let reader = self.pools.reader();
        let record = repo::find(reader, id)
            .await?
            .ok_or(AdminUserError::NotFound)?;
        let counts = repo::resource_counts(reader, &[id])
            .await?
            .remove(&id)
            .unwrap_or_default();
        let session_count = self.sessions.count_active(id).await?;
        let trusted_device_count = trusted_devices::count_usable(reader, id, now).await?;
        let identity_links = repo::identity_links(reader, id).await?;
        let instance_default = self.settings.load().quotas.default_user_quota_bytes;
        Ok(AdminUserDetail::new(
            DetailParts {
                record,
                counts,
                session_count,
                trusted_device_count,
                identity_links,
            },
            instance_default,
            now,
        ))
    }

    pub async fn create(
        &self,
        admin: &AuthenticatedPrincipal,
        input: CreateInput,
        claim: &Claim,
        client: &ClientMetadata,
    ) -> Result<AdminUserItem, AdminUserError> {
        let CreateInput {
            first_name,
            last_name,
            username,
            email,
            role,
            credential,
            quota,
            locale,
            is_active,
        } = input;
        let (password_hash, must_change_password) = match credential {
            InitialCredential::Local {
                password,
                must_change_password,
            } => {
                AccountPasswordPolicy::from_settings(&self.settings.load())
                    .check(password.expose_secret())?;
                (
                    Some(hash_off_runtime(password).await?),
                    must_change_password,
                )
            }
            InitialCredential::SsoOnly => (None, false),
        };
        let (locale, instance_default) = {
            let settings = self.settings.load();
            (
                locale.unwrap_or_else(|| settings.default_locale()),
                settings.quotas.default_user_quota_bytes,
            )
        };
        let new = NewUser {
            email,
            username,
            first_name,
            last_name,
            password_hash,
            must_change_password,
            role,
            is_active,
            quota,
            created_by: Some(admin.user_id),
        };
        let id = UserId::generate(self.clock.as_ref());
        self.pools
            .write_tx(self.clock.as_ref(), CREATE_TRANSACTION, async |tx| {
                let clock = self.clock.as_ref();
                let user = users::insert_with_id(tx, clock, id, &new).await?;
                users::insert_preferences(tx, clock, id, locale).await?;
                let event = AuditEvent::new(
                    actions::user_created(UserCreatedFacts {
                        role: user.role,
                        is_active: user.is_active,
                        local_password: user.password_hash.is_some(),
                        must_change_password: user.must_change_password,
                        quota_mode: user.quota.mode(),
                    }),
                    Actor::user(&admin.user_id.to_string(), &admin.username),
                    Outcome::Success,
                    user.created_at,
                )
                .with_target(
                    Target::new(TargetType::User)
                        .id(&user.id.to_string())
                        .label(&user.username),
                )
                .with_client(client.clone());
                self.audit.record_in_tx(tx, &event).await?;
                let item = AdminUserItem::new(
                    &AdminUserRecord::from_created(&user),
                    ResourceCounts::default(),
                    instance_default,
                    user.created_at,
                );
                let body = serde_json::to_value(&item)
                    .map_err(|_| UserError::RepositoryInvariant { column: "response" })?;
                self.idempotency
                    .complete(tx, claim, &ReplayEnvelope::new(StatusCode::CREATED, body))
                    .await?;
                Ok::<_, AdminUserError>(item)
            })
            .await
    }

    pub async fn update(
        &self,
        id: UserId,
        input: UpdateInput,
    ) -> Result<AdminUserItem, AdminUserError> {
        self.pools
            .write_tx(self.clock.as_ref(), UPDATE_TRANSACTION, async |tx| {
                users::update_identity(tx, self.clock.as_ref(), id, &input).await?;
                Ok::<_, AdminUserError>(())
            })
            .await?;
        self.item(id).await
    }

    pub async fn change_role(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        role: Role,
        client: &ClientMetadata,
    ) -> Result<AdminUserItem, AdminUserError> {
        self.pools
            .write_tx(self.clock.as_ref(), ROLE_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let user = users::find_by_id_in_tx(tx, id)
                    .await?
                    .ok_or(AdminUserError::NotFound)?;
                if user.role == role {
                    return Ok::<_, AdminUserError>(());
                }
                if role == Role::User {
                    assert_active_admin_remains(tx, id).await?;
                    assert_safe_sso_after_change(tx, Projection::removing_admin(id)).await?;
                }
                lifecycle::set_role(tx, id, role, at).await?;
                let sessions_revoked = self
                    .sessions
                    .revoke_all_in_tx(tx, id, RevokedReason::RoleChanged)
                    .await?;
                let spec = actions::user_role_changed(UserRoleChangedFacts {
                    from: user.role,
                    to: role,
                    sessions_revoked,
                });
                self.record_lifecycle(tx, spec, admin, &user, at, client)
                    .await
            })
            .await?;
        self.item(id).await
    }

    pub async fn deactivate(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        client: &ClientMetadata,
    ) -> Result<AdminUserItem, AdminUserError> {
        self.pools
            .write_tx(self.clock.as_ref(), DEACTIVATE_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let user = users::find_by_id_in_tx(tx, id)
                    .await?
                    .ok_or(AdminUserError::NotFound)?;
                if !user.is_active {
                    return Ok::<_, AdminUserError>(());
                }
                assert_active_admin_remains(tx, id).await?;
                assert_safe_sso_after_change(tx, Projection::removing_admin(id)).await?;
                lifecycle::deactivate(tx, id, admin.user_id, at).await?;
                let sessions_revoked = self
                    .sessions
                    .revoke_all_in_tx(tx, id, RevokedReason::Deactivated)
                    .await?;
                let trusted_devices_revoked = trusted_devices::revoke_all_in_tx(tx, id, at).await?;
                let identity_links_suspended =
                    lifecycle::suspend_identity_links(tx, id, at).await?;
                let spec = actions::user_deactivated(UserDeactivatedFacts {
                    sessions_revoked,
                    trusted_devices_revoked,
                    identity_links_suspended,
                });
                self.record_lifecycle(tx, spec, admin, &user, at, client)
                    .await
            })
            .await?;
        self.item(id).await
    }

    pub async fn activate(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        client: &ClientMetadata,
    ) -> Result<AdminUserItem, AdminUserError> {
        self.pools
            .write_tx(self.clock.as_ref(), ACTIVATE_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let user = users::find_by_id_in_tx(tx, id)
                    .await?
                    .ok_or(AdminUserError::NotFound)?;
                if user.is_active {
                    return Ok::<_, AdminUserError>(());
                }
                lifecycle::activate(tx, id, at).await?;
                let identity_links_restored = lifecycle::restore_identity_links(tx, id).await?;
                let spec = actions::user_activated(identity_links_restored);
                self.record_lifecycle(tx, spec, admin, &user, at, client)
                    .await
            })
            .await?;
        self.item(id).await
    }

    pub async fn reset_password(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        client: &ClientMetadata,
    ) -> Result<Secret<String>, AdminUserError> {
        let target = users::find_by_id(self.pools.reader(), id)
            .await?
            .ok_or(AdminUserError::NotFound)?;
        let policy = {
            let settings = self.settings.load();
            check_reset_target(&target, password_login_enabled(&settings))?;
            AccountPasswordPolicy::from_settings(&settings)
        };
        let temporary = policy.temporary_password()?;
        policy.check(temporary.expose_secret())?;
        let hash = hash_off_runtime(temporary.clone()).await?;
        self.pools
            .write_tx(
                self.clock.as_ref(),
                PASSWORD_RESET_TRANSACTION,
                async |tx| {
                    let at = Timestamp::try_from(self.clock.now())?;
                    let user = users::find_by_id_in_tx(tx, id)
                        .await?
                        .ok_or(AdminUserError::NotFound)?;
                    check_reset_target(&user, password_login_enabled(&self.settings.load()))?;
                    if !lifecycle::set_temporary_password(tx, id, &hash, at).await? {
                        return Err(AdminUserError::NoLocalAuth);
                    }
                    let reset_links_invalidated =
                        reset_links::invalidate_outstanding(tx, id, at).await?;
                    let sessions_revoked = self
                        .sessions
                        .revoke_all_in_tx(tx, id, RevokedReason::PasswordReset)
                        .await?;
                    let trusted_devices_revoked =
                        trusted_devices::revoke_all_in_tx(tx, id, at).await?;
                    let lockout_cleared = lockout::clear(tx, id, Some(admin.user_id), at).await?;
                    let spec =
                        actions::user_password_reset_by_admin(UserPasswordResetByAdminFacts {
                            sessions_revoked,
                            trusted_devices_revoked,
                            reset_links_invalidated,
                            lockout_cleared,
                        });
                    self.record_lifecycle(tx, spec, admin, &user, at, client)
                        .await
                },
            )
            .await?;
        Ok(temporary)
    }

    pub async fn unlock(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
    ) -> Result<(), AdminUserError> {
        self.pools
            .write_tx(self.clock.as_ref(), UNLOCK_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                users::find_by_id_in_tx(tx, id)
                    .await?
                    .ok_or(AdminUserError::NotFound)?;
                lockout::clear(tx, id, Some(admin.user_id), at).await?;
                Ok::<_, AdminUserError>(())
            })
            .await
    }

    pub async fn revoke_sessions(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        client: &ClientMetadata,
    ) -> Result<(), AdminUserError> {
        self.pools
            .write_tx(
                self.clock.as_ref(),
                REVOKE_SESSIONS_TRANSACTION,
                async |tx| {
                    let at = Timestamp::try_from(self.clock.now())?;
                    let user = users::find_by_id_in_tx(tx, id)
                        .await?
                        .ok_or(AdminUserError::NotFound)?;
                    let reason = RevokedReason::AdminRequest;
                    let revoked = self.sessions.revoke_all_in_tx(tx, id, reason).await?;
                    if revoked == 0 {
                        return Ok::<_, AdminUserError>(());
                    }
                    let spec = actions::all_sessions_revoked(
                        reason.as_str(),
                        id == admin.user_id,
                        revoked,
                    );
                    self.record_lifecycle(tx, spec, admin, &user, at, client)
                        .await
                },
            )
            .await
    }

    pub async fn set_quota(
        &self,
        admin: &AuthenticatedPrincipal,
        id: UserId,
        quota: QuotaOverride,
        client: &ClientMetadata,
    ) -> Result<AdminUserQuota, AdminUserError> {
        self.pools
            .write_tx(self.clock.as_ref(), QUOTA_TRANSACTION, async |tx| {
                let at = Timestamp::try_from(self.clock.now())?;
                let user = users::find_by_id_in_tx(tx, id)
                    .await?
                    .ok_or(AdminUserError::NotFound)?;
                if user.quota != quota {
                    lifecycle::set_quota_override(tx, id, quota, at).await?;
                    let spec = actions::quota_override_changed(QuotaOverrideChangedFacts {
                        from_mode: user.quota.mode(),
                        from_quota_bytes: user.quota.quota_bytes().map(u64::from),
                        to_mode: quota.mode(),
                        to_quota_bytes: quota.quota_bytes().map(u64::from),
                    });
                    self.record_lifecycle(tx, spec, admin, &user, at, client)
                        .await?;
                }
                let instance_default = self.settings.load().quotas.default_user_quota_bytes;
                Ok::<_, AdminUserError>(AdminUserQuota::new(
                    quota,
                    instance_default,
                    user.used_bytes,
                ))
            })
            .await
    }

    async fn record_lifecycle(
        &self,
        tx: &mut WriteTx<'_>,
        spec: actions::ActionSpec,
        admin: &AuthenticatedPrincipal,
        target: &User,
        at: Timestamp,
        client: &ClientMetadata,
    ) -> Result<(), AdminUserError> {
        let event = AuditEvent::new(
            spec,
            Actor::user(&admin.user_id.to_string(), &admin.username),
            Outcome::Success,
            at,
        )
        .with_target(
            Target::new(TargetType::User)
                .id(&target.id.to_string())
                .label(&target.username),
        )
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }

    async fn item(&self, id: UserId) -> Result<AdminUserItem, AdminUserError> {
        let now = Timestamp::try_from(self.clock.now())?;
        let reader = self.pools.reader();
        let record = repo::find(reader, id)
            .await?
            .ok_or(AdminUserError::NotFound)?;
        let counts = repo::resource_counts(reader, &[id])
            .await?
            .remove(&id)
            .unwrap_or_default();
        let instance_default = self.settings.load().quotas.default_user_quota_bytes;
        Ok(AdminUserItem::new(&record, counts, instance_default, now))
    }

    pub async fn sessions(
        &self,
        admin: &AuthenticatedPrincipal,
        target: UserId,
        page: PageRequest,
    ) -> Result<Page<SessionItem>, AdminUserError> {
        if !repo::exists(self.pools.reader(), target).await? {
            return Err(AdminUserError::NotFound);
        }
        Ok(self
            .sessions
            .list_for_user(target, admin.session_id, page)
            .await?)
    }
}

pub(super) fn check_reset_target(
    target: &User,
    password_login: bool,
) -> Result<(), AdminUserError> {
    if target.password_hash.is_none() {
        Err(AdminUserError::NoLocalAuth)
    } else if !password_login {
        Err(AdminUserError::PasswordLoginDisabled)
    } else {
        Ok(())
    }
}

async fn hash_off_runtime(password: Secret<String>) -> Result<Secret<String>, AdminUserError> {
    tokio::task::spawn_blocking(move || hash_password(password.expose_secret().as_bytes()))
        .await
        .map_err(|_| AdminUserError::HashTask)?
        .map_err(AdminUserError::from)
}

fn cursor_key(record: &AdminUserRecord, field: &'static SortField) -> CursorKey {
    let value = match field.name() {
        "usedBytes" => SortValue::Integer(record.used_bytes.to_i64()),
        "username" => SortValue::Text(record.username_normalized.clone()),
        "email" => SortValue::Text(record.email_normalized.clone()),
        _ => SortValue::Text(record.created_at.to_string()),
    };
    CursorKey::new(value, record.id)
}

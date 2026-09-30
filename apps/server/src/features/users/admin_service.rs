use std::fmt;
use std::sync::Arc;

use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::role::Role;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::features::auth::sessions::{
    AuthenticatedPrincipal, SessionError, SessionItem, SessionService,
};
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::auth::trusted_devices::TrustedDeviceError;
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::db::DbPools;
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    invalid_param, CursorKey, Page, PageRequest, QueryParams, SearchQuery, SortAllowlist,
    SortDirection, SortField, SortKeyKind, SortValue, TotalCount,
};

use super::admin_model::{
    AdminUserDetail, AdminUserItem, AdminUserRecord, DetailParts, UserStatus,
};
use super::admin_repo::{self as repo, SearchPrefix, UserFilter};
use super::error::UserError;
use super::model::UserId;

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
    User(UserError),
    Session(SessionError),
    TrustedDevice(TrustedDeviceError),
    Time(InvalidTimestamp),
}

impl AdminUserError {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "admin_user_not_found",
            Self::User(error) => error.kind(),
            Self::Session(error) => error.kind(),
            Self::TrustedDevice(error) => error.kind(),
            Self::Time(_) => "admin_user_time_out_of_range",
        }
    }

    pub fn api_error(&self) -> ApiError {
        match self {
            Self::NotFound | Self::User(UserError::NotFound) => {
                ApiError::new(ErrorCode::UserNotFound)
            }
            Self::User(UserError::Db(error)) => ApiError::new(error.api_code()),
            Self::Session(error) => error.api_error(),
            Self::TrustedDevice(error) => error.api_error(),
            Self::User(_) | Self::Time(_) => ApiError::internal(),
        }
    }
}

impl fmt::Display for AdminUserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the user does not exist"),
            Self::User(error) => write!(f, "admin user read failed: {error}"),
            Self::Session(error) => write!(f, "admin user session read failed: {error}"),
            Self::TrustedDevice(error) => {
                write!(f, "admin user trusted-device read failed: {error}")
            }
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
}

impl AdminUserService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        keys: Arc<KeyRing>,
        sessions: SessionService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            keys,
            sessions,
        }
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

fn cursor_key(record: &AdminUserRecord, field: &'static SortField) -> CursorKey {
    let value = match field.name() {
        "usedBytes" => SortValue::Integer(record.used_bytes.to_i64()),
        "username" => SortValue::Text(record.username_normalized.clone()),
        "email" => SortValue::Text(record.email_normalized.clone()),
        _ => SortValue::Text(record.created_at.to_string()),
    };
    CursorKey::new(value, record.id)
}

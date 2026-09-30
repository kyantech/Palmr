use std::collections::HashMap;

use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::domain::bytes::ByteSize;
use crate::domain::normalize::normalize;
use crate::domain::role::Role;
use crate::features::auth::lockout::LockState;
use crate::infra::db::ReadPool;
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{invalid_param, Conjunction, PageRequest, SearchQuery};

use super::admin_model::{
    AdminUserRecord, IdentityLinkMethod, IdentityLinkRecord, IdentityLinkState, ResourceCounts,
    UserStatus,
};
use super::error::UserError;
use super::model::{QuotaOverride, UserId};

macro_rules! select_admin_user {
    ($tail:literal) => {
        concat!(
            "SELECT u.id, u.first_name, u.last_name, u.username,
                u.username_normalized, u.email, u.email_normalized, u.pending_email, u.role,
                u.is_active, u.must_change_password, u.totp_enabled,
                u.password_hash IS NOT NULL AS has_local_password,
                u.quota_override_mode, u.quota_bytes, u.used_bytes, u.last_login_at,
                u.created_at,
                l.failed_count AS lock_failed_count, l.lock_count AS lock_lock_count,
                l.locked_until AS lock_locked_until
             FROM users u
             LEFT JOIN account_lockouts l ON l.user_id = u.id",
            $tail
        )
    };
}

const LIST_SELECT: &str = select_admin_user!("");

const SELECT_ONE: &str = select_admin_user!(" WHERE u.id = ?1");

const COUNT_SELECT: &str = "SELECT COUNT(*) FROM users u";

const EXISTS: &str = "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)";

const IDENTITY_LINKS: &str = "SELECT l.id, p.key AS provider_key, p.display_name AS provider_name,
        l.state, l.link_method, l.created_at, l.last_login_at
    FROM identity_links l
    JOIN identity_providers p ON p.id = l.provider_id
    WHERE l.user_id = ?1
    ORDER BY l.created_at ASC, l.id ASC";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPrefix {
    lower: String,
    upper: String,
}

impl SearchPrefix {
    pub fn parse(query: Option<&SearchQuery>) -> Result<Option<Self>, ApiError> {
        query
            .map(|query| {
                let lower = normalize(query.as_str());
                if lower.is_empty() {
                    return Err(invalid_param("q"));
                }
                let upper = format!("{lower}\u{10FFFF}");
                Ok(Self { lower, upper })
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserFilter {
    pub role: Option<Role>,
    pub status: Option<UserStatus>,
    pub search: Option<SearchPrefix>,
}

fn push_filters(query: &mut QueryBuilder<'_, Sqlite>, filter: &UserFilter) -> Conjunction {
    let mut first = true;
    let mut next = |query: &mut QueryBuilder<'_, Sqlite>| {
        query.push(if first { " WHERE " } else { " AND " });
        first = false;
    };
    if let Some(role) = filter.role {
        next(query);
        query.push("u.role = ").push_bind(role.as_str());
    }
    if let Some(status) = filter.status {
        next(query);
        query
            .push("u.is_active = ")
            .push_bind(i64::from(status.is_active()));
    }
    if let Some(search) = &filter.search {
        next(query);
        query
            .push("((u.username_normalized >= ")
            .push_bind(search.lower.clone())
            .push(" AND u.username_normalized < ")
            .push_bind(search.upper.clone())
            .push(") OR (u.email_normalized >= ")
            .push_bind(search.lower.clone())
            .push(" AND u.email_normalized < ")
            .push_bind(search.upper.clone())
            .push("))");
    }
    if first {
        Conjunction::Where
    } else {
        Conjunction::And
    }
}

pub(super) fn push_list(
    query: &mut QueryBuilder<'_, Sqlite>,
    filter: &UserFilter,
    page: &PageRequest,
) {
    query.push(LIST_SELECT);
    let conjunction = push_filters(query, filter);
    page.push_keyset(query, conjunction);
    page.push_order_and_limit(query);
}

pub(super) fn push_count(query: &mut QueryBuilder<'_, Sqlite>, filter: &UserFilter) {
    query.push(COUNT_SELECT);
    push_filters(query, filter);
}

pub async fn list(
    reader: &ReadPool,
    filter: &UserFilter,
    page: &PageRequest,
) -> Result<(Vec<AdminUserRecord>, u64), UserError> {
    let mut query = QueryBuilder::<Sqlite>::new("");
    push_list(&mut query, filter, page);
    let rows = query.build().fetch_all(reader.executor()).await?;
    let records = rows
        .iter()
        .map(record_from)
        .collect::<Result<Vec<_>, _>>()?;

    let mut count = QueryBuilder::<Sqlite>::new("");
    push_count(&mut count, filter);
    let total: i64 = count
        .build_query_scalar()
        .fetch_one(reader.executor())
        .await?;
    let total = u64::try_from(total).map_err(|_| invariant("count"))?;
    Ok((records, total))
}

pub async fn find(reader: &ReadPool, id: UserId) -> Result<Option<AdminUserRecord>, UserError> {
    let row = sqlx::query(SELECT_ONE)
        .bind(id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    row.as_ref().map(record_from).transpose()
}

pub async fn exists(reader: &ReadPool, id: UserId) -> Result<bool, UserError> {
    let exists: bool = sqlx::query_scalar(EXISTS)
        .bind(id.to_string())
        .fetch_one(reader.executor())
        .await?;
    Ok(exists)
}

pub async fn identity_links(
    reader: &ReadPool,
    id: UserId,
) -> Result<Vec<IdentityLinkRecord>, UserError> {
    let rows = sqlx::query(IDENTITY_LINKS)
        .bind(id.to_string())
        .fetch_all(reader.executor())
        .await?;
    rows.iter().map(link_from).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Counted {
    Files,
    Shares,
    ReverseShares,
    ReceivedFiles,
    IdentityLinks,
}

impl Counted {
    pub(super) const ALL: [Self; 5] = [
        Self::Files,
        Self::Shares,
        Self::ReverseShares,
        Self::ReceivedFiles,
        Self::IdentityLinks,
    ];

    const fn prefix(self) -> &'static str {
        match self {
            Self::Files => {
                "SELECT owner_id AS user_id, COUNT(*) AS n FROM files WHERE owner_id IN ("
            }
            Self::Shares => {
                "SELECT owner_id AS user_id, COUNT(*) AS n FROM shares WHERE owner_id IN ("
            }
            Self::ReverseShares => {
                "SELECT owner_id AS user_id, COUNT(*) AS n FROM reverse_shares WHERE owner_id IN ("
            }
            Self::ReceivedFiles => {
                "SELECT owner_id AS user_id, COUNT(*) AS n FROM received_files WHERE owner_id IN ("
            }
            Self::IdentityLinks => {
                "SELECT user_id, COUNT(*) AS n FROM identity_links WHERE user_id IN ("
            }
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::IdentityLinks => ") GROUP BY user_id",
            Self::Files | Self::Shares | Self::ReverseShares | Self::ReceivedFiles => {
                ") GROUP BY owner_id"
            }
        }
    }

    fn assign(self, counts: &mut ResourceCounts, n: u64) {
        match self {
            Self::Files => counts.files = n,
            Self::Shares => counts.shares = n,
            Self::ReverseShares => counts.reverse_shares = n,
            Self::ReceivedFiles => counts.received_files = n,
            Self::IdentityLinks => counts.identity_links = n,
        }
    }
}

pub(super) fn push_grouped_count<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    counted: Counted,
    ids: &[UserId],
) {
    query.push(counted.prefix());
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(id.to_string());
    }
    query.push(counted.suffix());
}

pub async fn resource_counts(
    reader: &ReadPool,
    ids: &[UserId],
) -> Result<HashMap<UserId, ResourceCounts>, UserError> {
    let mut counts: HashMap<UserId, ResourceCounts> = HashMap::new();
    if ids.is_empty() {
        return Ok(counts);
    }
    for counted in Counted::ALL {
        let mut query = QueryBuilder::<Sqlite>::new("");
        push_grouped_count(&mut query, counted, ids);
        let rows = query.build().fetch_all(reader.executor()).await?;
        for row in &rows {
            let id: UserId = parsed(row, "user_id")?;
            let n: i64 = column(row, "n")?;
            let n = u64::try_from(n).map_err(|_| invariant("count"))?;
            counted.assign(counts.entry(id).or_default(), n);
        }
    }
    Ok(counts)
}

fn record_from(row: &SqliteRow) -> Result<AdminUserRecord, UserError> {
    let role: String = column(row, "role")?;
    let mode: String = column(row, "quota_override_mode")?;
    let quota_bytes: Option<i64> = column(row, "quota_bytes")?;
    let used_bytes: i64 = column(row, "used_bytes")?;
    let last_login_at: Option<String> = column(row, "last_login_at")?;
    let lock_failed_count: Option<i64> = column(row, "lock_failed_count")?;
    let lock = lock_failed_count
        .map(|failed_count| lock_from(row, failed_count))
        .transpose()?;
    Ok(AdminUserRecord {
        id: parsed(row, "id")?,
        first_name: column(row, "first_name")?,
        last_name: column(row, "last_name")?,
        username: column(row, "username")?,
        username_normalized: column(row, "username_normalized")?,
        email: column(row, "email")?,
        email_normalized: column(row, "email_normalized")?,
        pending_email: column(row, "pending_email")?,
        role: role.parse::<Role>().map_err(|_| invariant("role"))?,
        is_active: column(row, "is_active")?,
        must_change_password: column(row, "must_change_password")?,
        totp_enabled: column(row, "totp_enabled")?,
        has_local_password: column(row, "has_local_password")?,
        quota: QuotaOverride::from_columns(&mode, quota_bytes)
            .map_err(|_| invariant("quota_override_mode"))?,
        used_bytes: ByteSize::try_from(used_bytes).map_err(|_| invariant("used_bytes"))?,
        lock,
        last_login_at: last_login_at
            .map(|text| text.parse().map_err(|_| invariant("last_login_at")))
            .transpose()?,
        created_at: parsed(row, "created_at")?,
    })
}

fn lock_from(row: &SqliteRow, failed_count: i64) -> Result<LockState, UserError> {
    let lock_count: i64 = column(row, "lock_lock_count")?;
    let locked_until: Option<String> = column(row, "lock_locked_until")?;
    Ok(LockState {
        failed_count: u32::try_from(failed_count).map_err(|_| invariant("failed_count"))?,
        locked_until: locked_until
            .map(|text| text.parse().map_err(|_| invariant("locked_until")))
            .transpose()?,
        lock_count: u32::try_from(lock_count).map_err(|_| invariant("lock_count"))?,
    })
}

fn link_from(row: &SqliteRow) -> Result<IdentityLinkRecord, UserError> {
    let state: String = column(row, "state")?;
    let method: String = column(row, "link_method")?;
    let last_login_at: Option<String> = column(row, "last_login_at")?;
    Ok(IdentityLinkRecord {
        id: column(row, "id")?,
        provider_key: column(row, "provider_key")?,
        provider_name: column(row, "provider_name")?,
        state: IdentityLinkState::parse(&state).ok_or_else(|| invariant("state"))?,
        method: IdentityLinkMethod::parse(&method).ok_or_else(|| invariant("link_method"))?,
        created_at: parsed(row, "created_at")?,
        last_login_at: last_login_at
            .map(|text| text.parse().map_err(|_| invariant("last_login_at")))
            .transpose()?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, UserError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, UserError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> UserError {
    UserError::RepositoryInvariant { column }
}

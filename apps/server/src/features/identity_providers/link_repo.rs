use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use super::error::ExternalLoginError;
use super::model::{IdentityLinkId, ProviderId};
use super::resolve::LinkState;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::db::{ReadPool, WriteTx};
use crate::infra::http::pagination::{Conjunction, PageRequest};

pub const SORT_COLUMN: &str = "l.created_at";
pub const ID_COLUMN: &str = "l.id";

const LINK_COLUMNS: &str =
    "l.id, l.user_id, l.provider_id, l.subject, l.state, p.key AS provider_key";

const LINK_FROM: &str = " FROM identity_links l JOIN identity_providers p ON p.id = l.provider_id";

const LIST_COLUMNS: &str =
    "l.id, p.key AS provider_key, p.display_name AS provider_name, l.subject,
    l.email_at_link, l.created_at, l.last_login_at";

const DELETE_SCOPED: &str = "DELETE FROM identity_links WHERE id = ?1 AND user_id = ?2";

const COUNT_FOR_USER: &str = "SELECT COUNT(*) FROM identity_links WHERE user_id = ?1";

const SELECT_SESSION: &str = "SELECT user_id, state, revoked_at, last_auth_at, idle_expires_at,
        absolute_expires_at, identity_link_id
    FROM sessions WHERE id = ?1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRecord {
    pub id: IdentityLinkId,
    pub user_id: UserId,
    pub provider_id: ProviderId,
    pub provider_slug: String,
    pub subject: String,
    pub state: LinkState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedLink {
    pub id: IdentityLinkId,
    pub provider_slug: String,
    pub provider_display_name: String,
    pub subject: String,
    pub email_at_link: Option<String>,
    pub linked_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    pub user_id: UserId,
    pub live: bool,
    pub last_auth_at: Timestamp,
    pub identity_link_id: Option<IdentityLinkId>,
}

pub async fn find_scoped_in_tx(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    id: IdentityLinkId,
) -> Result<Option<LinkRecord>, ExternalLoginError> {
    let row = sqlx::query(&format!(
        "SELECT {LINK_COLUMNS}{LINK_FROM} WHERE l.id = ?1 AND l.user_id = ?2"
    ))
    .bind(id.to_string())
    .bind(user_id.to_string())
    .fetch_optional(tx.executor())
    .await?;
    row.as_ref().map(link_from).transpose()
}

pub async fn find_in_tx(
    tx: &mut WriteTx<'_>,
    id: IdentityLinkId,
) -> Result<Option<LinkRecord>, ExternalLoginError> {
    let row = sqlx::query(&format!("SELECT {LINK_COLUMNS}{LINK_FROM} WHERE l.id = ?1"))
        .bind(id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(link_from).transpose()
}

pub async fn find_for_session(
    reader: &ReadPool,
    session_id: &str,
) -> Result<Option<LinkRecord>, ExternalLoginError> {
    let row = sqlx::query(&format!(
        "SELECT {LINK_COLUMNS}{LINK_FROM} JOIN sessions s ON s.identity_link_id = l.id
         WHERE s.id = ?1"
    ))
    .bind(session_id)
    .fetch_optional(reader.executor())
    .await?;
    row.as_ref().map(link_from).transpose()
}

pub async fn delete_scoped(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    id: IdentityLinkId,
) -> Result<bool, ExternalLoginError> {
    let deleted = sqlx::query(DELETE_SCOPED)
        .bind(id.to_string())
        .bind(user_id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(deleted.rows_affected() == 1)
}

pub async fn count_for_user_in_tx(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
) -> Result<u64, ExternalLoginError> {
    let count: i64 = sqlx::query_scalar(COUNT_FOR_USER)
        .bind(user_id.to_string())
        .fetch_one(tx.executor())
        .await?;
    u64::try_from(count).map_err(|_| invalid_row(()))
}

pub async fn count_for_user(reader: &ReadPool, user_id: UserId) -> Result<u64, ExternalLoginError> {
    let count: i64 = sqlx::query_scalar(COUNT_FOR_USER)
        .bind(user_id.to_string())
        .fetch_one(reader.executor())
        .await?;
    u64::try_from(count).map_err(|_| invalid_row(()))
}

pub async fn list(
    reader: &ReadPool,
    user_id: UserId,
    page: &PageRequest,
) -> Result<(Vec<ListedLink>, u64), ExternalLoginError> {
    let mut query = QueryBuilder::<Sqlite>::new(format!(
        "SELECT {LIST_COLUMNS}{LINK_FROM} WHERE l.user_id = "
    ));
    query.push_bind(user_id.to_string());
    page.push_keyset(&mut query, Conjunction::And);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(reader.executor()).await?;
    let links = rows
        .iter()
        .map(listed_from)
        .collect::<Result<Vec<_>, _>>()?;
    let total = count_for_user(reader, user_id).await?;
    Ok((links, total))
}

pub async fn session_in_tx(
    tx: &mut WriteTx<'_>,
    session_id: &str,
    now: Timestamp,
) -> Result<Option<SessionFacts>, ExternalLoginError> {
    let row = sqlx::query(SELECT_SESSION)
        .bind(session_id)
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(|row| session_from(row, now)).transpose()
}

fn session_from(row: &SqliteRow, now: Timestamp) -> Result<SessionFacts, ExternalLoginError> {
    let text = |column: &'static str| -> Result<String, ExternalLoginError> {
        row.try_get::<String, _>(column).map_err(invalid_row)
    };
    let state = text("state")?;
    let revoked_at: Option<String> = row.try_get("revoked_at").map_err(invalid_row)?;
    let idle: Timestamp = text("idle_expires_at")?.parse().map_err(invalid_row)?;
    let absolute: Timestamp = text("absolute_expires_at")?.parse().map_err(invalid_row)?;
    let link: Option<String> = row.try_get("identity_link_id").map_err(invalid_row)?;
    Ok(SessionFacts {
        user_id: text("user_id")?.parse().map_err(invalid_row)?,
        live: state == "active" && revoked_at.is_none() && now < idle && now < absolute,
        last_auth_at: text("last_auth_at")?.parse().map_err(invalid_row)?,
        identity_link_id: link.map(|id| id.parse().map_err(invalid_row)).transpose()?,
    })
}

fn link_from(row: &SqliteRow) -> Result<LinkRecord, ExternalLoginError> {
    let text = |column: &'static str| -> Result<String, ExternalLoginError> {
        row.try_get::<String, _>(column).map_err(invalid_row)
    };
    Ok(LinkRecord {
        id: text("id")?.parse().map_err(invalid_row)?,
        user_id: text("user_id")?.parse().map_err(invalid_row)?,
        provider_id: text("provider_id")?.parse().map_err(invalid_row)?,
        provider_slug: text("provider_key")?,
        subject: text("subject")?,
        state: match text("state")?.as_str() {
            "active" => LinkState::Active,
            "suspended" => LinkState::Suspended,
            _ => return Err(invalid_row(())),
        },
    })
}

fn listed_from(row: &SqliteRow) -> Result<ListedLink, ExternalLoginError> {
    let text = |column: &'static str| -> Result<String, ExternalLoginError> {
        row.try_get::<String, _>(column).map_err(invalid_row)
    };
    let last_used: Option<String> = row.try_get("last_login_at").map_err(invalid_row)?;
    Ok(ListedLink {
        id: text("id")?.parse().map_err(invalid_row)?,
        provider_slug: text("provider_key")?,
        provider_display_name: text("provider_name")?,
        subject: text("subject")?,
        email_at_link: row.try_get("email_at_link").map_err(invalid_row)?,
        linked_at: text("created_at")?.parse().map_err(invalid_row)?,
        last_used_at: last_used
            .map(|value| value.parse().map_err(invalid_row))
            .transpose()?,
    })
}

fn invalid_row<E>(_: E) -> ExternalLoginError {
    ExternalLoginError::internal("identity_link_row_invalid")
}

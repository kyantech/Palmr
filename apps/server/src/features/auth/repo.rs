use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite};

use crate::domain::role::Role;
use crate::features::users::model::UserId;

use super::error::LoginError;
use super::model::AccountView;

const SELECT_ACCOUNT: &str = "SELECT
        u.id, u.first_name, u.last_name, u.username, u.email, u.pending_email, u.role,
        u.is_active, u.avatar_storage_object_id IS NOT NULL AS has_avatar,
        u.password_hash IS NOT NULL AS has_local_password,
        u.totp_enabled, u.created_at,
        COALESCE(p.locale, 'en-US') AS locale,
        COALESCE(p.theme, 'system') AS theme,
        COALESCE(p.accent, 'default') AS accent,
        (SELECT COUNT(*) FROM identity_links l WHERE l.user_id = u.id) AS identity_link_count
      FROM users u
      LEFT JOIN user_preferences p ON p.user_id = u.id
      WHERE u.id = ?1";

pub async fn account<'e, E>(executor: E, id: UserId) -> Result<Option<AccountView>, LoginError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(SELECT_ACCOUNT)
        .bind(id.to_string())
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(account_from).transpose()
}

fn account_from(row: &SqliteRow) -> Result<AccountView, LoginError> {
    let role: String = column(row, "role")?;
    let links: i64 = column(row, "identity_link_count")?;
    Ok(AccountView {
        id: parsed(row, "id")?,
        first_name: column(row, "first_name")?,
        last_name: column(row, "last_name")?,
        username: column(row, "username")?,
        email: column(row, "email")?,
        pending_email: column(row, "pending_email")?,
        role: role.parse::<Role>().map_err(|_| invariant("role"))?,
        is_active: column(row, "is_active")?,
        has_avatar: column(row, "has_avatar")?,
        has_local_password: column(row, "has_local_password")?,
        totp_enabled: column(row, "totp_enabled")?,
        locale: column(row, "locale")?,
        theme: column(row, "theme")?,
        accent: column(row, "accent")?,
        created_at: parsed(row, "created_at")?,
        identity_link_count: u32::try_from(links).map_err(|_| invariant("identity_link_count"))?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, LoginError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, LoginError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

const fn invariant(column: &'static str) -> LoginError {
    LoginError::RepositoryInvariant { column }
}

use sqlx::sqlite::SqliteRow;
use sqlx::Row;

use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::locale::LocaleCode;
use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbError, ReadPool, WriteTx};

use super::error::UserError;
use super::model::{
    AdminState, NewUser, NormalizedIdentifier, QuotaOverride, UsageRow, User, UserId,
};

macro_rules! select_user_where {
    ($filter:literal) => {
        concat!(
            "SELECT id, email, email_normalized, username, username_normalized,
                    first_name, last_name, password_hash, password_updated_at,
                    must_change_password, role, is_active, deactivated_at, totp_enabled,
                    quota_override_mode, quota_bytes, used_bytes, created_at, updated_at,
                    created_by
             FROM users WHERE ",
            $filter
        )
    };
}

const SELECT_BY_ID: &str = select_user_where!("id = ?1");
pub(super) const SELECT_BY_EMAIL_NORMALIZED: &str = select_user_where!("email_normalized = ?1");
pub(super) const SELECT_BY_USERNAME_NORMALIZED: &str =
    select_user_where!("username_normalized = ?1");
const SELECT_BY_LOGIN_IDENTIFIER: &str = select_user_where!(
    "email_normalized = ?1 OR username_normalized = ?1
     ORDER BY email_normalized = ?1 DESC LIMIT 1"
);

const UPGRADE_PASSWORD_HASH: &str = "UPDATE users
    SET password_hash = ?3
    WHERE id = ?1 AND password_hash = ?2";

const UPDATE_LAST_LOGIN: &str = "UPDATE users SET last_login_at = ?2 WHERE id = ?1";

const INSERT: &str = "INSERT INTO users (
        id, email, email_normalized, username, username_normalized, first_name, last_name,
        password_hash, password_updated_at, must_change_password, role, is_active,
        deactivated_at, deactivated_by, quota_override_mode, quota_bytes,
        created_at, updated_at, created_by
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17, ?18)";

const EMAIL_NORMALIZED_EXISTS: &str =
    "SELECT EXISTS(SELECT 1 FROM users WHERE email_normalized = ?1)";
const USERNAME_NORMALIZED_EXISTS: &str =
    "SELECT EXISTS(SELECT 1 FROM users WHERE username_normalized = ?1)";

const INSERT_PREFERENCES: &str =
    "INSERT INTO user_preferences (user_id, locale, created_at, updated_at)
    VALUES (?1, ?2, ?3, ?3)";

const SELECT_PASSWORD_HASH: &str = "SELECT password_hash FROM users WHERE id = ?1";

const UPDATE_PASSWORD_HASH: &str = "UPDATE users
    SET password_hash = ?2, password_updated_at = ?3, updated_at = ?3
    WHERE id = ?1";

const UPDATE_MUST_CHANGE_PASSWORD: &str = "UPDATE users
    SET must_change_password = ?2, updated_at = ?3
    WHERE id = ?1";

const CHANGE_PASSWORD: &str = "UPDATE users
    SET password_hash = ?3, password_updated_at = ?4, must_change_password = 0, updated_at = ?4
    WHERE id = ?1 AND is_active = 1 AND password_hash = ?2";

const UPDATE_NAMES: &str = "UPDATE users
    SET first_name = COALESCE(?2, first_name), last_name = COALESCE(?3, last_name), updated_at = ?4
    WHERE id = ?1 AND is_active = 1";

const SELECT_USAGE: &str = "SELECT u.used_bytes, u.quota_override_mode, u.quota_bytes,
        (SELECT COALESCE(SUM(f.size_bytes), 0) FROM files f WHERE f.owner_id = u.id)
            AS my_files_bytes,
        (SELECT COALESCE(SUM(r.size_bytes), 0) FROM received_files r WHERE r.owner_id = u.id)
            AS received_bytes,
        (SELECT COALESCE(SUM(q.reserved_bytes), 0) FROM quota_reservations q
            WHERE q.user_id = u.id AND q.state = 'held')
            AS reserved_bytes
      FROM users u
      WHERE u.id = ?1";

const SELECT_ADMIN_STATE: &str = "SELECT role, is_active FROM users WHERE id = ?1";

pub(super) const COUNT_ACTIVE_ADMINS: &str =
    "SELECT COUNT(*) FROM users WHERE role = 'admin' AND is_active = 1";

pub async fn find_by_id(reader: &ReadPool, id: UserId) -> Result<Option<User>, UserError> {
    let row = sqlx::query(SELECT_BY_ID)
        .bind(id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| user_from(&row)).transpose()
}

pub async fn find_by_email_normalized(
    reader: &ReadPool,
    email: &NormalizedIdentifier,
) -> Result<Option<User>, UserError> {
    let row = sqlx::query(SELECT_BY_EMAIL_NORMALIZED)
        .bind(email.as_str())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| user_from(&row)).transpose()
}

pub async fn find_by_username_normalized(
    reader: &ReadPool,
    username: &NormalizedIdentifier,
) -> Result<Option<User>, UserError> {
    let row = sqlx::query(SELECT_BY_USERNAME_NORMALIZED)
        .bind(username.as_str())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| user_from(&row)).transpose()
}

pub async fn find_by_login_identifier(
    reader: &ReadPool,
    identifier: &NormalizedIdentifier,
) -> Result<Option<User>, UserError> {
    let row = sqlx::query(SELECT_BY_LOGIN_IDENTIFIER)
        .bind(identifier.as_str())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| user_from(&row)).transpose()
}

pub async fn find_by_id_in_tx(tx: &mut WriteTx<'_>, id: UserId) -> Result<Option<User>, UserError> {
    let row = sqlx::query(SELECT_BY_ID)
        .bind(id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.map(|row| user_from(&row)).transpose()
}

pub async fn record_login(
    tx: &mut WriteTx<'_>,
    id: UserId,
    at: Timestamp,
) -> Result<(), UserError> {
    let updated = sqlx::query(UPDATE_LAST_LOGIN)
        .bind(id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    affected_one(updated.rows_affected())
}

pub async fn insert(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    new: &NewUser,
) -> Result<User, UserError> {
    let id = UserId::generate(clock);
    let now = Timestamp::try_from(clock.now())?;
    let password_updated_at = new.password_hash.as_ref().map(|_| now);
    let deactivated_at = (!new.is_active).then_some(now);
    let created_by = new.created_by.map(|creator| creator.to_string());
    let deactivated_by = created_by.as_deref().filter(|_| !new.is_active);

    let inserted = sqlx::query(INSERT)
        .bind(id.to_string())
        .bind(new.email.as_str())
        .bind(new.email.normalized())
        .bind(new.username.as_str())
        .bind(new.username.normalized())
        .bind(&new.first_name)
        .bind(&new.last_name)
        .bind(
            new.password_hash
                .as_ref()
                .map(|hash| hash.expose_secret().as_str()),
        )
        .bind(password_updated_at.map(|at| at.to_string()))
        .bind(new.must_change_password)
        .bind(new.role.as_str())
        .bind(new.is_active)
        .bind(deactivated_at.map(|at| at.to_string()))
        .bind(deactivated_by)
        .bind(new.quota.mode())
        .bind(new.quota.quota_bytes().map(ByteSize::to_i64))
        .bind(now.to_string())
        .bind(created_by.as_deref())
        .execute(tx.executor())
        .await
        .map_err(DbError::from);

    match inserted {
        Ok(_) => Ok(User {
            id,
            email: new.email.as_str().to_owned(),
            email_normalized: new.email.normalized().to_owned(),
            username: new.username.as_str().to_owned(),
            username_normalized: new.username.normalized().to_owned(),
            first_name: new.first_name.clone(),
            last_name: new.last_name.clone(),
            password_hash: new.password_hash.clone(),
            password_updated_at,
            must_change_password: new.must_change_password,
            role: new.role,
            is_active: new.is_active,
            deactivated_at,
            totp_enabled: false,
            quota: new.quota,
            used_bytes: ByteSize::ZERO,
            created_at: now,
            updated_at: now,
            created_by: new.created_by,
        }),
        Err(error @ DbError::UniqueViolation(_)) => Err(identity_conflict(tx, new, error).await),
        Err(error) => Err(error.into()),
    }
}

pub async fn insert_preferences(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    id: UserId,
    locale: LocaleCode,
) -> Result<(), UserError> {
    let now = Timestamp::try_from(clock.now())?;
    sqlx::query(INSERT_PREFERENCES)
        .bind(id.to_string())
        .bind(locale.as_str())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    Ok(())
}

async fn identity_conflict(tx: &mut WriteTx<'_>, new: &NewUser, error: DbError) -> UserError {
    let probes = [
        (
            EMAIL_NORMALIZED_EXISTS,
            new.email.normalized(),
            UserError::EmailTaken,
        ),
        (
            USERNAME_NORMALIZED_EXISTS,
            new.username.normalized(),
            UserError::UsernameTaken,
        ),
    ];
    for (probe, value, conflict) in probes {
        match sqlx::query_scalar::<_, bool>(probe)
            .bind(value)
            .fetch_one(tx.executor())
            .await
        {
            Ok(true) => return conflict,
            Ok(false) => {}
            Err(probe_error) => return probe_error.into(),
        }
    }
    error.into()
}

pub async fn password_hash(
    reader: &ReadPool,
    id: UserId,
) -> Result<Option<Secret<String>>, UserError> {
    let stored: Option<Option<String>> = sqlx::query_scalar(SELECT_PASSWORD_HASH)
        .bind(id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    stored
        .map(|hash| hash.map(Secret::new))
        .ok_or(UserError::NotFound)
}

pub async fn replace_password_hash(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    id: UserId,
    hash: &Secret<String>,
) -> Result<(), UserError> {
    let now = Timestamp::try_from(clock.now())?;
    let updated = sqlx::query(UPDATE_PASSWORD_HASH)
        .bind(id.to_string())
        .bind(hash.expose_secret().as_str())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    affected_one(updated.rows_affected())
}

pub async fn upgrade_password_hash(
    tx: &mut WriteTx<'_>,
    id: UserId,
    verified: &Secret<String>,
    upgraded: &Secret<String>,
) -> Result<bool, UserError> {
    let updated = sqlx::query(UPGRADE_PASSWORD_HASH)
        .bind(id.to_string())
        .bind(verified.expose_secret().as_str())
        .bind(upgraded.expose_secret().as_str())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn set_must_change_password(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    id: UserId,
    required: bool,
) -> Result<(), UserError> {
    let now = Timestamp::try_from(clock.now())?;
    let updated = sqlx::query(UPDATE_MUST_CHANGE_PASSWORD)
        .bind(id.to_string())
        .bind(required)
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    affected_one(updated.rows_affected())
}

pub async fn change_password(
    tx: &mut WriteTx<'_>,
    id: UserId,
    verified: &Secret<String>,
    replacement: &Secret<String>,
    at: Timestamp,
) -> Result<bool, UserError> {
    let updated = sqlx::query(CHANGE_PASSWORD)
        .bind(id.to_string())
        .bind(verified.expose_secret().as_str())
        .bind(replacement.expose_secret().as_str())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn update_names(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    id: UserId,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> Result<(), UserError> {
    let now = Timestamp::try_from(clock.now())?;
    let updated = sqlx::query(UPDATE_NAMES)
        .bind(id.to_string())
        .bind(first_name)
        .bind(last_name)
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
    affected_one(updated.rows_affected())
}

pub async fn usage(reader: &ReadPool, id: UserId) -> Result<Option<UsageRow>, UserError> {
    let row = sqlx::query(SELECT_USAGE)
        .bind(id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| {
        let quota_mode: String = column(&row, "quota_override_mode")?;
        let quota_bytes: Option<i64> = column(&row, "quota_bytes")?;
        Ok(UsageRow {
            used_bytes: bytes(&row, "used_bytes")?,
            my_files_bytes: bytes(&row, "my_files_bytes")?,
            received_bytes: bytes(&row, "received_bytes")?,
            reserved_bytes: bytes(&row, "reserved_bytes")?,
            quota: QuotaOverride::from_columns(&quota_mode, quota_bytes)
                .map_err(|_| invariant("quota_override_mode"))?,
        })
    })
    .transpose()
}

fn bytes(row: &SqliteRow, name: &'static str) -> Result<ByteSize, UserError> {
    ByteSize::try_from(column::<i64>(row, name)?).map_err(|_| invariant(name))
}

pub(super) async fn admin_state(
    tx: &mut WriteTx<'_>,
    id: UserId,
) -> Result<Option<AdminState>, UserError> {
    let row: Option<(String, bool)> = sqlx::query_as(SELECT_ADMIN_STATE)
        .bind(id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.map(|(role, is_active)| {
        let role = role.parse::<Role>().map_err(|_| invariant("role"))?;
        Ok(AdminState { role, is_active })
    })
    .transpose()
}

pub(super) async fn count_active_admins(tx: &mut WriteTx<'_>) -> Result<i64, UserError> {
    Ok(sqlx::query_scalar(COUNT_ACTIVE_ADMINS)
        .fetch_one(tx.executor())
        .await?)
}

const fn affected_one(rows: u64) -> Result<(), UserError> {
    if rows == 0 {
        Err(UserError::NotFound)
    } else {
        Ok(())
    }
}

fn user_from(row: &SqliteRow) -> Result<User, UserError> {
    let quota_mode: String = column(row, "quota_override_mode")?;
    let quota_bytes: Option<i64> = column(row, "quota_bytes")?;
    let used_bytes: i64 = column(row, "used_bytes")?;
    let role: String = column(row, "role")?;
    Ok(User {
        id: parsed(row, "id")?,
        email: column(row, "email")?,
        email_normalized: column(row, "email_normalized")?,
        username: column(row, "username")?,
        username_normalized: column(row, "username_normalized")?,
        first_name: column(row, "first_name")?,
        last_name: column(row, "last_name")?,
        password_hash: column::<Option<String>>(row, "password_hash")?.map(Secret::new),
        password_updated_at: parsed_optional(row, "password_updated_at")?,
        must_change_password: column(row, "must_change_password")?,
        role: role.parse().map_err(|_| invariant("role"))?,
        is_active: column(row, "is_active")?,
        deactivated_at: parsed_optional(row, "deactivated_at")?,
        totp_enabled: column(row, "totp_enabled")?,
        quota: QuotaOverride::from_columns(&quota_mode, quota_bytes)
            .map_err(|_| invariant("quota_override_mode"))?,
        used_bytes: ByteSize::try_from(used_bytes).map_err(|_| invariant("used_bytes"))?,
        created_at: parsed(row, "created_at")?,
        updated_at: parsed(row, "updated_at")?,
        created_by: parsed_optional(row, "created_by")?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, UserError>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: std::str::FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, UserError> {
    column::<String>(row, name)?
        .parse()
        .map_err(|_| invariant(name))
}

fn parsed_optional<T: std::str::FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<T>, UserError> {
    column::<Option<String>>(row, name)?
        .map(|text| text.parse().map_err(|_| invariant(name)))
        .transpose()
}

const fn invariant(column: &'static str) -> UserError {
    UserError::RepositoryInvariant { column }
}

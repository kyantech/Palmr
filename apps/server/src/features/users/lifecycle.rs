use crate::domain::role::Role;
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::infra::db::WriteTx;

use super::error::UserError;
use super::model::{QuotaOverride, UserId};

const SET_ROLE: &str = "UPDATE users SET role = ?2, updated_at = ?3
    WHERE id = ?1 AND role <> ?2";

const DEACTIVATE: &str = "UPDATE users
    SET is_active = 0, deactivated_at = ?2, deactivated_by = ?3, updated_at = ?2
    WHERE id = ?1 AND is_active = 1";

const ACTIVATE: &str = "UPDATE users
    SET is_active = 1, deactivated_at = NULL, deactivated_by = NULL, updated_at = ?2
    WHERE id = ?1 AND is_active = 0";

const SET_TEMPORARY_PASSWORD: &str = "UPDATE users
    SET password_hash = ?2, password_updated_at = ?3, must_change_password = 1, updated_at = ?3
    WHERE id = ?1 AND password_hash IS NOT NULL";

const SET_QUOTA_OVERRIDE: &str = "UPDATE users
    SET quota_override_mode = ?2, quota_bytes = ?3, updated_at = ?4
    WHERE id = ?1";

const SUSPEND_IDENTITY_LINKS: &str = "UPDATE identity_links
    SET state = 'suspended', suspended_at = ?2
    WHERE user_id = ?1 AND state = 'active'";

const RESTORE_IDENTITY_LINKS: &str = "UPDATE identity_links
    SET state = 'active', suspended_at = NULL
    WHERE user_id = ?1 AND state = 'suspended'";

pub(super) async fn set_role(
    tx: &mut WriteTx<'_>,
    target: UserId,
    role: Role,
    at: Timestamp,
) -> Result<bool, UserError> {
    let updated = sqlx::query(SET_ROLE)
        .bind(target.to_string())
        .bind(role.as_str())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub(super) async fn deactivate(
    tx: &mut WriteTx<'_>,
    target: UserId,
    by: UserId,
    at: Timestamp,
) -> Result<bool, UserError> {
    let updated = sqlx::query(DEACTIVATE)
        .bind(target.to_string())
        .bind(at.to_string())
        .bind(by.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub(super) async fn activate(
    tx: &mut WriteTx<'_>,
    target: UserId,
    at: Timestamp,
) -> Result<bool, UserError> {
    let updated = sqlx::query(ACTIVATE)
        .bind(target.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub(super) async fn suspend_identity_links(
    tx: &mut WriteTx<'_>,
    target: UserId,
    at: Timestamp,
) -> Result<u64, UserError> {
    let updated = sqlx::query(SUSPEND_IDENTITY_LINKS)
        .bind(target.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected())
}

pub(super) async fn restore_identity_links(
    tx: &mut WriteTx<'_>,
    target: UserId,
) -> Result<u64, UserError> {
    let updated = sqlx::query(RESTORE_IDENTITY_LINKS)
        .bind(target.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected())
}

pub(super) async fn set_temporary_password(
    tx: &mut WriteTx<'_>,
    target: UserId,
    hash: &Secret<String>,
    at: Timestamp,
) -> Result<bool, UserError> {
    let updated = sqlx::query(SET_TEMPORARY_PASSWORD)
        .bind(target.to_string())
        .bind(hash.expose_secret().as_str())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

pub(super) async fn set_quota_override(
    tx: &mut WriteTx<'_>,
    target: UserId,
    quota: QuotaOverride,
    at: Timestamp,
) -> Result<bool, UserError> {
    let updated = sqlx::query(SET_QUOTA_OVERRIDE)
        .bind(target.to_string())
        .bind(quota.mode())
        .bind(quota.quota_bytes().map(i64::from))
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(updated.rows_affected() == 1)
}

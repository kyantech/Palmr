use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::infra::db::WriteTx;

use super::error::UserError;
use super::model::UserId;

const SET_ROLE: &str = "UPDATE users SET role = ?2, updated_at = ?3
    WHERE id = ?1 AND role <> ?2";

const DEACTIVATE: &str = "UPDATE users
    SET is_active = 0, deactivated_at = ?2, deactivated_by = ?3, updated_at = ?2
    WHERE id = ?1 AND is_active = 1";

const ACTIVATE: &str = "UPDATE users
    SET is_active = 1, deactivated_at = NULL, deactivated_by = NULL, updated_at = ?2
    WHERE id = ?1 AND is_active = 0";

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

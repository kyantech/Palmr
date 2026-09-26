use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, WriteTx};

const REVOKE_ALL: &str = "UPDATE trusted_devices SET revoked_at = ?2
    WHERE user_id = ?1 AND revoked_at IS NULL";

pub async fn revoke_all_in_tx(
    tx: &mut WriteTx<'_>,
    user_id: UserId,
    at: Timestamp,
) -> Result<u64, DbError> {
    let revoked = sqlx::query(REVOKE_ALL)
        .bind(user_id.to_string())
        .bind(at.to_string())
        .execute(tx.executor())
        .await?;
    Ok(revoked.rows_affected())
}

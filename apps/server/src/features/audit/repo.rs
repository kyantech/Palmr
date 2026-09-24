use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::jobs::claim::MAX_ROWS_PER_TX;

use super::error::AuditError;
use super::model::AuditEvent;

pub const DEFAULT_AUDIT_RETENTION_DAYS: u32 = 90;
pub const MIN_AUDIT_RETENTION_DAYS: u32 = 7;
pub const MAX_AUDIT_RETENTION_DAYS: u32 = 3_650;

const RETENTION_KEY: &str = "audit_retention_days";

const INSERT: &str = "INSERT INTO audit_events (
        id, occurred_at, action, actor_type, actor_user_id, actor_label,
        target_type, target_id, target_label, result, error_code,
        request_id, client_ip, user_agent, metadata_json
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

const DELETE_BATCH: &str = "DELETE FROM audit_events
                             WHERE id IN (SELECT id FROM audit_events
                                           WHERE occurred_at < ?1
                                           ORDER BY occurred_at, id
                                           LIMIT ?2)";

const RETENTION_DAYS: &str = "SELECT value_json FROM app_settings WHERE key = ?1";

pub async fn insert(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    event: &AuditEvent,
) -> Result<(), AuditError> {
    let id = Id::<AuditEvent>::generate(clock).to_string();
    let actor = event.actor();
    let target = event.target();
    let client = event.client();
    sqlx::query(INSERT)
        .bind(&id)
        .bind(event.occurred_at().to_string())
        .bind(event.action().as_str())
        .bind(actor.kind().as_str())
        .bind(actor.user_id())
        .bind(actor.label())
        .bind(target.map(|target| target.kind().as_str()))
        .bind(target.and_then(|target| target.id_value()))
        .bind(target.and_then(|target| target.label_value()))
        .bind(event.outcome().result_str())
        .bind(event.outcome().error_code())
        .bind(client.request_id())
        .bind(client.client_ip())
        .bind(client.user_agent())
        .bind(event.metadata().as_str())
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn delete_expired_batch(
    pools: &DbPools,
    clock: &dyn Clock,
    cutoff: Timestamp,
) -> Result<u64, AuditError> {
    pools
        .write_tx(clock, "audit.retention_delete", async |tx| {
            let deleted = sqlx::query(DELETE_BATCH)
                .bind(cutoff.to_string())
                .bind(i64::from(MAX_ROWS_PER_TX))
                .execute(tx.executor())
                .await?;
            Ok::<u64, AuditError>(deleted.rows_affected())
        })
        .await
}

pub async fn retention_days(pools: &DbPools) -> Result<u32, AuditError> {
    let stored: Option<String> = sqlx::query_scalar(RETENTION_DAYS)
        .bind(RETENTION_KEY)
        .fetch_optional(pools.reader().executor())
        .await?;
    Ok(stored
        .and_then(|text| serde_json::from_str::<i64>(&text).ok())
        .map_or(DEFAULT_AUDIT_RETENTION_DAYS, |days| {
            days.clamp(
                i64::from(MIN_AUDIT_RETENTION_DAYS),
                i64::from(MAX_AUDIT_RETENTION_DAYS),
            ) as u32
        }))
}

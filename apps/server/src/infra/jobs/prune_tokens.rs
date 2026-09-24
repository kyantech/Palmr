use std::sync::Arc;
use std::time::Duration;

use super::claim::MAX_ROWS_PER_TX;
use super::recurring::Recurring;
use super::{ClaimedJob, Idempotency, JobKind, JobPayload, JobsError, Registry};
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::DbPools;

pub const TOKENS_PRUNE_PERIOD: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneStep {
    IdempotencyRecords,
}

impl PruneStep {
    pub const ALL: [Self; 1] = [Self::IdempotencyRecords];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdempotencyRecords => "idempotency_records",
        }
    }

    const fn transaction(self) -> &'static str {
        match self {
            Self::IdempotencyRecords => "tokens.prune.idempotency_records",
        }
    }

    const fn delete_batch(self) -> &'static str {
        match self {
            Self::IdempotencyRecords => {
                "DELETE FROM idempotency_records
                  WHERE id IN (SELECT id FROM idempotency_records
                                WHERE expires_at <= ?1
                                ORDER BY expires_at
                                LIMIT ?2)"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepReport {
    pub deleted: u64,
    pub transactions: u32,
    pub largest_batch: u64,
}

pub async fn prune_step(
    pools: &DbPools,
    clock: &dyn Clock,
    step: PruneStep,
) -> Result<StepReport, JobsError> {
    let cutoff = Timestamp::try_from(clock.now())?.to_string();
    let mut report = StepReport::default();
    loop {
        let batch = pools
            .write_tx(clock, step.transaction(), async |tx| {
                let deleted = sqlx::query(step.delete_batch())
                    .bind(&cutoff)
                    .bind(i64::from(MAX_ROWS_PER_TX))
                    .execute(tx.executor())
                    .await?;
                Ok::<u64, JobsError>(deleted.rows_affected())
            })
            .await?;
        report.deleted = report.deleted.saturating_add(batch);
        report.transactions = report.transactions.saturating_add(1);
        report.largest_batch = report.largest_batch.max(batch);
        if batch < u64::from(MAX_ROWS_PER_TX) {
            return Ok(report);
        }
    }
}

#[derive(Clone)]
struct PruneContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
}

pub fn register_jobs(registry: Registry, pools: DbPools, clock: Arc<dyn Clock>) -> Registry {
    let context = PruneContext { pools, clock };
    registry.register(
        JobKind::TokensPrune,
        Idempotency::key("tokens.prune expiry cutoff"),
        move |job: ClaimedJob| {
            let context = context.clone();
            async move { prune(job, context).await }
        },
    )
}

async fn prune(_job: ClaimedJob, context: PruneContext) -> anyhow::Result<()> {
    for step in PruneStep::ALL {
        let report = prune_step(&context.pools, context.clock.as_ref(), step).await?;
        tracing::info!(
            step = step.as_str(),
            deleted = report.deleted,
            transactions = report.transactions,
            "tokens prune step finished"
        );
    }
    let recurring = Recurring::new(JobKind::TokensPrune, TOKENS_PRUNE_PERIOD)?;
    let payload = JobPayload::empty();
    context
        .pools
        .write_tx(
            context.clock.as_ref(),
            "tokens.prune_successor",
            async |tx| {
                recurring
                    .schedule_successor(tx, context.clock.as_ref(), &payload)
                    .await
            },
        )
        .await?;
    Ok(())
}

pub async fn ensure_scheduled(pools: &DbPools, clock: &dyn Clock) -> anyhow::Result<()> {
    let recurring = Recurring::new(JobKind::TokensPrune, TOKENS_PRUNE_PERIOD)?;
    let payload = JobPayload::empty();
    pools
        .write_tx(clock, "tokens.schedule_prune", async |tx| {
            recurring.schedule(tx, clock, &payload).await
        })
        .await?;
    Ok(())
}

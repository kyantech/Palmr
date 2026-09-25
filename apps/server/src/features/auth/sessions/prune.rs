use std::sync::Arc;
use std::time::Duration;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::DbPools;
use crate::infra::jobs::claim::MAX_ROWS_PER_TX;
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::{ClaimedJob, Idempotency, JobKind, JobPayload, JobsError, Registry};

pub const SESSIONS_PRUNE_PERIOD: Duration = Duration::from_secs(60 * 60);
const FORENSIC_RETENTION: time::Duration = time::Duration::days(7);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneStep {
    StaleMfaPending,
    MarkExpired,
    TerminalSessions,
}

impl PruneStep {
    pub const ALL: [Self; 3] = [
        Self::StaleMfaPending,
        Self::MarkExpired,
        Self::TerminalSessions,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::StaleMfaPending => "stale_mfa_pending",
            Self::MarkExpired => "mark_expired",
            Self::TerminalSessions => "terminal_sessions",
        }
    }

    const fn transaction(self) -> &'static str {
        match self {
            Self::StaleMfaPending => "sessions.prune.stale_mfa_pending",
            Self::MarkExpired => "sessions.prune.mark_expired",
            Self::TerminalSessions => "sessions.prune.terminal",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepReport {
    pub affected: u64,
    pub transactions: u32,
    pub largest_batch: u64,
}

pub async fn prune_step(
    pools: &DbPools,
    clock: &dyn Clock,
    step: PruneStep,
) -> Result<StepReport, JobsError> {
    let now = Timestamp::try_from(clock.now())?.to_string();
    let retention_cutoff = Timestamp::try_from(clock.now() - FORENSIC_RETENTION)?.to_string();
    let mut report = StepReport::default();
    loop {
        let batch = pools
            .write_tx(clock, step.transaction(), async |tx| {
                let query = match step {
                    PruneStep::StaleMfaPending => {
                        "DELETE FROM sessions
                          WHERE id IN (SELECT id FROM sessions
                                        WHERE state = 'mfa_pending' AND mfa_expires_at <= ?1
                                        ORDER BY mfa_expires_at LIMIT ?3)"
                    }
                    PruneStep::MarkExpired => {
                        "UPDATE sessions SET state = 'expired'
                          WHERE id IN (SELECT id FROM sessions
                                        WHERE state = 'active'
                                          AND (idle_expires_at <= ?1 OR absolute_expires_at <= ?1)
                                        ORDER BY absolute_expires_at LIMIT ?3)"
                    }
                    PruneStep::TerminalSessions => {
                        "DELETE FROM sessions
                          WHERE id IN (SELECT id FROM sessions
                                        WHERE state IN ('revoked','expired')
                                          AND absolute_expires_at <= ?2
                                        ORDER BY absolute_expires_at LIMIT ?3)"
                    }
                };
                let affected = sqlx::query(query)
                    .bind(&now)
                    .bind(&retention_cutoff)
                    .bind(i64::from(MAX_ROWS_PER_TX))
                    .execute(tx.executor())
                    .await?;
                Ok::<u64, JobsError>(affected.rows_affected())
            })
            .await?;
        report.affected = report.affected.saturating_add(batch);
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
        JobKind::SessionsPrune,
        Idempotency::key("sessions.prune expiry and forensic cutoffs"),
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
            affected = report.affected,
            transactions = report.transactions,
            "sessions prune step finished"
        );
    }
    let recurring = Recurring::new(JobKind::SessionsPrune, SESSIONS_PRUNE_PERIOD)?;
    let payload = JobPayload::empty();
    context
        .pools
        .write_tx(
            context.clock.as_ref(),
            "sessions.prune_successor",
            async |tx| {
                recurring
                    .schedule_successor(tx, context.clock.as_ref(), &payload)
                    .await
            },
        )
        .await?;
    Ok(())
}

pub async fn ensure_scheduled(pools: &DbPools, clock: &dyn Clock) -> Result<(), JobsError> {
    let recurring = Recurring::new(JobKind::SessionsPrune, SESSIONS_PRUNE_PERIOD)
        .map_err(|_| JobsError::CorruptRow("sessions_prune_period"))?;
    let payload = JobPayload::empty();
    pools
        .write_tx(clock, "sessions.schedule_prune", async |tx| {
            recurring.schedule(tx, clock, &payload).await.map(drop)
        })
        .await
}

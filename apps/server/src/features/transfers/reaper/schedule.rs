use serde_json::{json, Value};

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::DbPools;
use crate::infra::jobs::claim::{enqueue, Enqueued};
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::{
    ClaimedJob, DedupKey, Idempotency, JobKind, JobPayload, JobsError, NewJob, Registry,
};

use super::{run_pass, ReapReport, ReaperContext, EXPIRE_STALE_PERIOD, MAX_CONTINUATIONS};

const SCHEDULE_TRANSACTION: &str = "tus.schedule_expire_stale";
const SUCCESSOR_TRANSACTION: &str = "tus.expire_stale_successor";
const CONTINUATION_TRANSACTION: &str = "tus.expire_stale_continuation";

pub fn register_jobs(registry: Registry, context: ReaperContext) -> Registry {
    registry.register(
        JobKind::TusExpireStale,
        Idempotency::key(
            "tus.expire_stale guards every transition by its expected from-state, removes staging with ENOENT as success and re-selects its candidates from the database",
        ),
        move |job: ClaimedJob| {
            let context = context.clone();
            async move { run(job, context).await }
        },
    )
}

fn recurring() -> Result<Recurring, JobsError> {
    Recurring::new(JobKind::TusExpireStale, EXPIRE_STALE_PERIOD)
        .map_err(|_| JobsError::CorruptRow("tus_expire_stale_period"))
}

pub async fn ensure_scheduled(pools: &DbPools, clock: &dyn Clock) -> Result<Enqueued, JobsError> {
    let recurring = recurring()?;
    let payload = JobPayload::empty();
    pools
        .write_tx(clock, SCHEDULE_TRANSACTION, async |tx| {
            let live: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind = ?1 AND state IN ('pending', 'claimed'))",
            )
            .bind(JobKind::TusExpireStale.as_str())
            .fetch_one(tx.executor())
            .await?;
            if live {
                return Ok(Enqueued::Deduplicated);
            }
            match recurring.schedule(tx, clock, &payload).await? {
                Enqueued::Deduplicated => recurring.schedule_successor(tx, clock, &payload).await,
                inserted @ Enqueued::Inserted(_) => Ok(inserted),
            }
        })
        .await
}

fn continuation(payload: &Value) -> u32 {
    payload
        .get("continuation")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

async fn run(job: ClaimedJob, context: ReaperContext) -> anyhow::Result<()> {
    let sequence = continuation(job.payload());
    let clock = context.clock.as_ref();
    let recurring = recurring()?;
    context
        .pools
        .write_tx(clock, SUCCESSOR_TRANSACTION, async |tx| {
            recurring
                .schedule_successor(tx, clock, &JobPayload::empty())
                .await
        })
        .await?;
    let report = run_pass(&context).await.inspect_err(|error| {
        tracing::warn!(
            error_kind = error.kind(),
            "tus.expire_stale failed; the job runtime retries it"
        );
    })?;
    if report.more && !context.stopping() && sequence < MAX_CONTINUATIONS {
        enqueue_continuation(&context, sequence + 1).await?;
    }
    log(&report, sequence);
    if report.errors > 0 {
        anyhow::bail!(
            "tus.expire_stale left {} candidates unprocessed",
            report.errors
        );
    }
    Ok(())
}

async fn enqueue_continuation(context: &ReaperContext, sequence: u32) -> Result<(), JobsError> {
    let clock = context.clock.as_ref();
    let recurring = recurring()?;
    let bucket = recurring.bucket(Timestamp::try_from(clock.now())?)?;
    let key = DedupKey::new(format!(
        "{}:more:{sequence}",
        recurring.dedup_key(bucket)?.as_str()
    ))?;
    let payload = JobPayload::new(&json!({ "continuation": sequence }))
        .map_err(|_| JobsError::CorruptRow("tus_expire_stale_continuation"))?;
    context
        .pools
        .write_tx(clock, CONTINUATION_TRANSACTION, async |tx| {
            enqueue(
                tx,
                clock,
                &NewJob::new(JobKind::TusExpireStale, payload.clone()).dedup_key(key.clone()),
            )
            .await
        })
        .await?;
    Ok(())
}

fn log(report: &ReapReport, sequence: u32) {
    tracing::info!(
        continuation = sequence,
        sessions_expired = report.sessions_expired,
        sessions_skipped = report.sessions_skipped,
        uploads_expired = report.uploads_expired,
        uploads_skipped = report.uploads_skipped,
        staging_removed = report.staging_removed,
        staging_failed = report.staging_failed,
        uploads_pruned = report.uploads_pruned,
        sessions_pruned = report.sessions_pruned,
        errors = report.errors,
        more = report.more,
        "tus.expire_stale finished"
    );
}

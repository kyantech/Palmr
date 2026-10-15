use std::time::Duration;

use serde::Serialize;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, InstanceId};
use crate::infra::jobs::claim::{enqueue, Enqueued};
use crate::infra::jobs::recurring::Recurring;
use crate::infra::jobs::{DedupKey, JobKind, JobPayload, JobsError, NewJob};

use super::payload::{AbortJob, ReconcileJob};

pub(super) const ABORT_SWEEP_PERIOD: Duration = Duration::from_secs(60 * 60);
pub(super) const RECONCILE_PERIOD: Duration = Duration::from_secs(6 * 60 * 60);

const SCHEDULE_TRANSACTION: &str = "s3.schedule_reapers";
const SUCCESSOR_TRANSACTION: &str = "s3.reaper_successor";
const CONTINUATION_TRANSACTION: &str = "s3.reaper_continuation";

const LIVE_HEAD: &str = "SELECT EXISTS(SELECT 1 FROM jobs
     WHERE kind = ?1 AND state IN ('pending', 'claimed')
       AND json_extract(payload_json, '$.mode') = 'sweep')";

pub(super) fn period(kind: JobKind) -> Duration {
    match kind {
        JobKind::S3ReconcileMultipart => RECONCILE_PERIOD,
        _ => ABORT_SWEEP_PERIOD,
    }
}

fn recurring(kind: JobKind) -> Result<Recurring, JobsError> {
    Recurring::new(kind, period(kind)).map_err(|_| JobsError::CorruptRow("s3_reaper_period"))
}

fn payload(value: &impl Serialize) -> Result<JobPayload, JobsError> {
    JobPayload::new(value).map_err(|_| JobsError::CorruptRow("s3_reaper_payload"))
}

fn head_payload(kind: JobKind) -> Result<JobPayload, JobsError> {
    match kind {
        JobKind::S3ReconcileMultipart => payload(&ReconcileJob::Sweep {}),
        _ => payload(&AbortJob::Sweep {}),
    }
}

pub async fn ensure_chain(
    pools: &DbPools,
    clock: &dyn Clock,
    kind: JobKind,
) -> Result<Enqueued, JobsError> {
    let recurring = recurring(kind)?;
    let head = head_payload(kind)?;
    pools
        .write_tx(clock, SCHEDULE_TRANSACTION, async |tx| {
            let live: bool = sqlx::query_scalar(LIVE_HEAD)
                .bind(kind.as_str())
                .fetch_one(tx.executor())
                .await?;
            if live {
                return Ok(Enqueued::Deduplicated);
            }
            match recurring.schedule(tx, clock, &head).await? {
                Enqueued::Deduplicated => recurring.schedule_successor(tx, clock, &head).await,
                inserted @ Enqueued::Inserted(_) => Ok(inserted),
            }
        })
        .await
}

pub async fn schedule_startup(
    pools: &DbPools,
    clock: &dyn Clock,
    instance: InstanceId,
) -> Result<u64, JobsError> {
    let mut inserted = 0_u64;
    for kind in [
        JobKind::S3AbortAbandonedMultipart,
        JobKind::S3ReconcileMultipart,
    ] {
        if matches!(
            ensure_chain(pools, clock, kind).await?,
            Enqueued::Inserted(_)
        ) {
            inserted += 1;
        }
        let head = head_payload(kind)?;
        let key = DedupKey::new(format!("{}:startup:{instance}", kind.as_str()))?;
        let queued = pools
            .write_tx(clock, SCHEDULE_TRANSACTION, async |tx| {
                enqueue(
                    tx,
                    clock,
                    &NewJob::new(kind, head.clone()).dedup_key(key.clone()),
                )
                .await
            })
            .await?;
        if matches!(queued, Enqueued::Inserted(_)) {
            inserted += 1;
        }
    }
    Ok(inserted)
}

pub(super) async fn schedule_successor(
    pools: &DbPools,
    clock: &dyn Clock,
    kind: JobKind,
) -> Result<(), JobsError> {
    let recurring = recurring(kind)?;
    let head = head_payload(kind)?;
    pools
        .write_tx(clock, SUCCESSOR_TRANSACTION, async |tx| {
            recurring.schedule_successor(tx, clock, &head).await
        })
        .await?;
    Ok(())
}

pub(super) async fn enqueue_continuation(
    pools: &DbPools,
    clock: &dyn Clock,
    kind: JobKind,
    lane: &'static str,
    sequence: u32,
    job: &impl Serialize,
) -> Result<(), JobsError> {
    let recurring = recurring(kind)?;
    let bucket = recurring.bucket(Timestamp::try_from(clock.now())?)?;
    let key = DedupKey::new(format!(
        "{}:{lane}:{sequence}",
        recurring.dedup_key(bucket)?.as_str()
    ))?;
    let payload = payload(job)?;
    pools
        .write_tx(clock, CONTINUATION_TRANSACTION, async |tx| {
            enqueue(
                tx,
                clock,
                &NewJob::new(kind, payload.clone()).dedup_key(key.clone()),
            )
            .await
        })
        .await?;
    Ok(())
}

use std::time::Duration;

use serde_json::{Map, Value};

use super::backoff::retry_delay;
use super::kinds::JobKind;
use super::{Claimant, ClaimedJob, JobId, JobsError, NewJob, MAX_LAST_ERROR_BYTES};
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, WriteTx};

pub const SUCCEEDED_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
pub const MAX_ROWS_PER_TX: u32 = 1_000;

const ENQUEUE: &str = "INSERT INTO jobs (id, kind, payload_json, state, priority, run_at, attempts,
                                         max_attempts, dedup_key, created_at, updated_at)
                       VALUES (?1, ?2, ?3, 'pending', ?4, ?5, 0, ?6, ?7, ?8, ?8)
                       ON CONFLICT (dedup_key) WHERE dedup_key IS NOT NULL DO NOTHING
                       RETURNING id";

const CLAIM: &str = "UPDATE jobs
                        SET state = 'claimed',
                            claimed_by = ?1,
                            lease_expires_at = json_extract(?2, '$.\"' || kind || '\"'),
                            updated_at = ?3
                      WHERE id IN (SELECT id FROM jobs
                                    WHERE state = 'pending' AND run_at <= ?3
                                      AND kind IN (SELECT key FROM json_each(?2))
                                    ORDER BY priority, run_at
                                    LIMIT ?4)
                  RETURNING id, kind, payload_json, attempts, max_attempts, lease_expires_at";

const RUNNABLE: &str = "SELECT EXISTS (SELECT 1 FROM jobs
                                        WHERE state = 'pending' AND run_at <= ?1
                                          AND kind IN (SELECT value FROM json_each(?2)))";

const RENEW: &str = "UPDATE jobs SET lease_expires_at = ?1, updated_at = ?2
                      WHERE id = ?3 AND state = 'claimed' AND claimed_by = ?4";

const SUCCEED: &str = "UPDATE jobs
                          SET state = 'succeeded', claimed_by = NULL, lease_expires_at = NULL,
                              updated_at = ?1
                        WHERE id = ?2 AND state = 'claimed' AND claimed_by = ?3";

const FAIL: &str = "UPDATE jobs
                       SET state = ?1, attempts = ?2, run_at = COALESCE(?3, run_at),
                           last_error = ?4, claimed_by = NULL, lease_expires_at = NULL,
                           updated_at = ?5
                     WHERE id = ?6 AND state = 'claimed' AND claimed_by = ?7 AND attempts = ?8";

const SWEEP: &str = "UPDATE jobs
                        SET state = 'pending', claimed_by = NULL, lease_expires_at = NULL,
                            updated_at = ?1
                      WHERE id IN (SELECT id FROM jobs
                                    WHERE state = 'claimed' AND lease_expires_at <= ?1
                                    LIMIT ?2)";

const PRUNE: &str = "DELETE FROM jobs
                      WHERE id IN (SELECT id FROM jobs
                                    WHERE state = 'succeeded' AND updated_at <= ?1
                                    LIMIT ?2)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    Inserted(JobId),
    Deduplicated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    Succeeded,
    Retrying { run_at: Timestamp },
    Dead { attempts: u32 },
    LeaseLost,
}

fn now(clock: &dyn Clock) -> Result<Timestamp, JobsError> {
    Ok(Timestamp::try_from(clock.now())?)
}

fn after(clock: &dyn Clock, delay: Duration) -> Result<Timestamp, JobsError> {
    Ok(Timestamp::try_from(clock.now() + delay)?)
}

pub async fn enqueue(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    job: &NewJob,
) -> Result<Enqueued, JobsError> {
    let now = now(clock)?;
    let id = JobId::generate(clock);
    let inserted: Option<String> = sqlx::query_scalar(ENQUEUE)
        .bind(id.to_string())
        .bind(job.kind.as_str())
        .bind(job.payload.as_str())
        .bind(i64::from(job.priority.get()))
        .bind(job.run_at.unwrap_or(now).to_string())
        .bind(i64::from(job.kind.policy().max_attempts))
        .bind(job.dedup_key.as_ref().map(|key| key.as_str().to_owned()))
        .bind(now.to_string())
        .fetch_optional(tx.executor())
        .await?;
    Ok(match inserted {
        Some(_) => Enqueued::Inserted(id),
        None => Enqueued::Deduplicated,
    })
}

fn lease_map(clock: &dyn Clock, kinds: &[JobKind]) -> Result<String, JobsError> {
    let mut leases = Map::new();
    for kind in kinds {
        let expires = after(clock, kind.policy().lease)?;
        leases.insert(kind.as_str().to_owned(), Value::from(expires.to_string()));
    }
    Ok(Value::Object(leases).to_string())
}

fn kind_list(kinds: &[JobKind]) -> String {
    Value::from_iter(kinds.iter().map(|kind| kind.as_str())).to_string()
}

pub async fn has_runnable(
    pools: &DbPools,
    clock: &dyn Clock,
    kinds: &[JobKind],
) -> Result<bool, JobsError> {
    if kinds.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(RUNNABLE)
        .bind(now(clock)?.to_string())
        .bind(kind_list(kinds))
        .fetch_one(pools.reader().executor())
        .await?)
}

type ClaimedRow = (String, String, String, i64, i64, String);

fn claimed(row: ClaimedRow, claimant: &Claimant) -> Result<ClaimedJob, JobsError> {
    let (id, kind, payload, attempts, max_attempts, lease_expires_at) = row;
    Ok(ClaimedJob {
        id: id.parse().map_err(|_| JobsError::CorruptRow("id"))?,
        kind: kind.parse().map_err(|_| JobsError::CorruptRow("kind"))?,
        payload: serde_json::from_str(&payload)
            .map_err(|_| JobsError::CorruptRow("payload_json"))?,
        attempts: u32::try_from(attempts).map_err(|_| JobsError::CorruptRow("attempts"))?,
        max_attempts: u32::try_from(max_attempts)
            .map_err(|_| JobsError::CorruptRow("max_attempts"))?,
        lease_expires_at: lease_expires_at
            .parse()
            .map_err(|_| JobsError::CorruptRow("lease_expires_at"))?,
        claimant: claimant.clone(),
    })
}

pub async fn claim(
    pools: &DbPools,
    clock: &dyn Clock,
    claimant: &Claimant,
    kinds: &[JobKind],
    limit: u32,
    proceed: impl Fn() -> bool,
) -> Result<Vec<ClaimedJob>, JobsError> {
    if kinds.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let limit = limit.min(MAX_ROWS_PER_TX);
    pools
        .write_tx(clock, "jobs.claim", async |tx| {
            if !proceed() {
                return Ok(Vec::new());
            }
            let rows: Vec<ClaimedRow> = sqlx::query_as(CLAIM)
                .bind(claimant.as_str())
                .bind(lease_map(clock, kinds)?)
                .bind(now(clock)?.to_string())
                .bind(i64::from(limit))
                .fetch_all(tx.executor())
                .await?;
            rows.into_iter()
                .map(|row| claimed(row, claimant))
                .collect::<Result<Vec<_>, JobsError>>()
        })
        .await
}

pub async fn renew(
    pools: &DbPools,
    clock: &dyn Clock,
    job: &ClaimedJob,
) -> Result<bool, JobsError> {
    pools
        .write_tx(clock, "jobs.renew_lease", async |tx| {
            let renewed = sqlx::query(RENEW)
                .bind(after(clock, job.kind.policy().lease)?.to_string())
                .bind(now(clock)?.to_string())
                .bind(job.id.to_string())
                .bind(job.claimant.as_str())
                .execute(tx.executor())
                .await?;
            Ok(renewed.rows_affected() == 1)
        })
        .await
}

pub async fn settle_success(
    pools: &DbPools,
    clock: &dyn Clock,
    job: &ClaimedJob,
) -> Result<Settled, JobsError> {
    pools
        .write_tx(clock, "jobs.settle_success", async |tx| {
            let settled = sqlx::query(SUCCEED)
                .bind(now(clock)?.to_string())
                .bind(job.id.to_string())
                .bind(job.claimant.as_str())
                .execute(tx.executor())
                .await?;
            Ok(if settled.rows_affected() == 1 {
                Settled::Succeeded
            } else {
                Settled::LeaseLost
            })
        })
        .await
}

pub fn bounded_error(text: &str) -> String {
    if text.len() <= MAX_LAST_ERROR_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_LAST_ERROR_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub async fn settle_failure(
    pools: &DbPools,
    clock: &dyn Clock,
    job: &ClaimedJob,
    last_error: &str,
    jitter_sample: u32,
) -> Result<Settled, JobsError> {
    let attempts = job.attempts.saturating_add(1);
    let (state, run_at, settled) = if attempts >= job.max_attempts {
        ("dead", None, Settled::Dead { attempts })
    } else {
        let run_at = after(clock, retry_delay(job.attempts, jitter_sample))?;
        ("pending", Some(run_at), Settled::Retrying { run_at })
    };
    pools
        .write_tx(clock, "jobs.settle_failure", async |tx| {
            let updated = sqlx::query(FAIL)
                .bind(state)
                .bind(i64::from(attempts))
                .bind(run_at.map(|at| at.to_string()))
                .bind(bounded_error(last_error))
                .bind(now(clock)?.to_string())
                .bind(job.id.to_string())
                .bind(job.claimant.as_str())
                .bind(i64::from(job.attempts))
                .execute(tx.executor())
                .await?;
            Ok(if updated.rows_affected() == 1 {
                settled
            } else {
                Settled::LeaseLost
            })
        })
        .await
}

pub async fn sweep_expired_leases(pools: &DbPools, clock: &dyn Clock) -> Result<u64, JobsError> {
    let mut reclaimed = 0;
    loop {
        let batch = pools
            .write_tx(clock, "jobs.sweep_expired_leases", async |tx| {
                let swept = sqlx::query(SWEEP)
                    .bind(now(clock)?.to_string())
                    .bind(i64::from(MAX_ROWS_PER_TX))
                    .execute(tx.executor())
                    .await?;
                Ok::<_, JobsError>(swept.rows_affected())
            })
            .await?;
        reclaimed += batch;
        if batch < u64::from(MAX_ROWS_PER_TX) {
            return Ok(reclaimed);
        }
    }
}

pub async fn prune_succeeded(pools: &DbPools, clock: &dyn Clock) -> Result<u64, JobsError> {
    let cutoff = Timestamp::try_from(clock.now() - SUCCEEDED_RETENTION)?;
    pools
        .write_tx(clock, "jobs.prune_succeeded", async |tx| {
            let pruned = sqlx::query(PRUNE)
                .bind(cutoff.to_string())
                .bind(i64::from(MAX_ROWS_PER_TX))
                .execute(tx.executor())
                .await?;
            Ok(pruned.rows_affected())
        })
        .await
}

use std::fmt;
use std::time::Duration;

use time::OffsetDateTime;

use super::claim::{enqueue, Enqueued};
use super::{DedupKey, JobKind, JobPayload, JobsError, NewJob};
use crate::domain::clock::Clock;
use crate::domain::time::{InvalidTimestamp, Timestamp};
use crate::infra::db::WriteTx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPeriod;

impl fmt::Display for InvalidPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a recurring period must be at least one second")
    }
}

impl std::error::Error for InvalidPeriod {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodBucket(Timestamp);

impl PeriodBucket {
    pub const fn start(self) -> Timestamp {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Recurring {
    kind: JobKind,
    period: Duration,
}

impl Recurring {
    pub fn new(kind: JobKind, period: Duration) -> Result<Self, InvalidPeriod> {
        if period.as_secs() == 0 {
            return Err(InvalidPeriod);
        }
        Ok(Self { kind, period })
    }

    pub const fn kind(self) -> JobKind {
        self.kind
    }

    pub fn bucket(self, now: Timestamp) -> Result<PeriodBucket, JobsError> {
        Ok(PeriodBucket(timestamp_from_unix(bucket_start(
            now,
            self.period,
        )?)?))
    }

    pub fn successor_bucket(self, now: Timestamp) -> Result<PeriodBucket, JobsError> {
        let successor = bucket_start(now, self.period)?.saturating_add(period_secs(self.period)?);
        Ok(PeriodBucket(timestamp_from_unix(successor)?))
    }

    pub async fn schedule(
        self,
        tx: &mut WriteTx<'_>,
        clock: &dyn Clock,
        payload: &JobPayload,
    ) -> Result<Enqueued, JobsError> {
        let now = Timestamp::try_from(clock.now())?;
        self.enqueue_bucket(tx, clock, payload, self.bucket(now)?)
            .await
    }

    pub async fn schedule_successor(
        self,
        tx: &mut WriteTx<'_>,
        clock: &dyn Clock,
        payload: &JobPayload,
    ) -> Result<Enqueued, JobsError> {
        let now = Timestamp::try_from(clock.now())?;
        self.enqueue_bucket(tx, clock, payload, self.successor_bucket(now)?)
            .await
    }

    pub fn dedup_key(self, bucket: PeriodBucket) -> Result<DedupKey, JobsError> {
        DedupKey::new(format!(
            "{}:{}",
            self.kind.as_str(),
            bucket.start().get().unix_timestamp()
        ))
        .map_err(JobsError::from)
    }

    async fn enqueue_bucket(
        self,
        tx: &mut WriteTx<'_>,
        clock: &dyn Clock,
        payload: &JobPayload,
        bucket: PeriodBucket,
    ) -> Result<Enqueued, JobsError> {
        let job = NewJob::new(self.kind, payload.clone())
            .run_at(bucket.start())
            .dedup_key(self.dedup_key(bucket)?);
        enqueue(tx, clock, &job).await
    }
}

fn period_secs(period: Duration) -> Result<i64, JobsError> {
    i64::try_from(period.as_secs()).map_err(|_| JobsError::Time(InvalidTimestamp))
}

fn bucket_start(now: Timestamp, period: Duration) -> Result<i64, JobsError> {
    let period = period_secs(period)?;
    let now = now.get().unix_timestamp();
    Ok(now - now.rem_euclid(period))
}

fn timestamp_from_unix(seconds: i64) -> Result<Timestamp, JobsError> {
    let at = OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| JobsError::Time(InvalidTimestamp))?;
    Ok(Timestamp::try_from(at)?)
}

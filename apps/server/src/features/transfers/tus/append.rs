use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::quota::error::QuotaError;
use crate::features::quota::service::QuotaService;
use crate::features::users::model::UserId;
use crate::infra::db::DbPools;
use crate::storage::staging::StagingAppend;

use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferSessionId};
use super::error::TusError;
use super::headers::CHECKSUM_DIGEST_LEN;
use super::repo::{self, OffsetWrite, TusState, TusUploadId};

const PERSIST_TRANSACTION: &str = "tus.persist_offset";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TusLimits {
    pub buffer_bytes: usize,
    pub flush_bytes: u64,
    pub flush_interval: Duration,
    pub idle_timeout: Duration,
    pub lease_ttl: Duration,
    pub upload_ttl: Duration,
    pub patch_concurrency: usize,
    pub permit_wait: Duration,
}

impl TusLimits {
    pub const FLUSH_BYTES: u64 = 8 * 1024 * 1024;
    pub const FLUSH_INTERVAL: Duration = Duration::from_secs(2);
    pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
    pub const LEASE_TTL: Duration = Duration::from_secs(60);
    pub const UPLOAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
    pub const PATCH_CONCURRENCY: usize = 64;
    pub const PERMIT_WAIT: Duration = Duration::from_secs(5);

    pub const fn production(buffer_bytes: usize) -> Self {
        Self {
            buffer_bytes,
            flush_bytes: Self::FLUSH_BYTES,
            flush_interval: Self::FLUSH_INTERVAL,
            idle_timeout: Self::IDLE_TIMEOUT,
            lease_ttl: Self::LEASE_TTL,
            upload_ttl: Self::UPLOAD_TTL,
            patch_concurrency: Self::PATCH_CONCURRENCY,
            permit_wait: Self::PERMIT_WAIT,
        }
    }

    pub fn heartbeat_interval(&self) -> Duration {
        self.lease_ttl / 3
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPlan {
    pub upload: TusUploadId,
    pub owner: UserId,
    pub session: TransferSessionId,
    pub item: SessionItemId,
    pub start: u64,
    pub declared: Option<u64>,
    pub max: Option<u64>,
    pub running_quota: bool,
    pub checksum: Option<[u8; CHECKSUM_DIGEST_LEN]>,
}

#[derive(Debug)]
pub enum StreamEnd {
    Finished,
    Overrun,
    TooLarge,
    Quota(QuotaError),
    Disconnected,
    Idle,
    Shutdown,
    WriteFailed,
    Gone,
    LeaseLost,
    ChecksumMismatch,
    Internal(TusError),
}

#[derive(Debug)]
pub struct StreamOutcome {
    pub durable: u64,
    pub expires_at: Option<Timestamp>,
    pub end: StreamEnd,
}

#[derive(Clone)]
pub struct Streamer {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    quota: QuotaService,
    holder: Arc<str>,
    limits: TusLimits,
    shutdown: CancellationToken,
}

#[derive(Clone, Copy)]
enum Stop {
    Overrun,
    TooLarge,
    Quota,
}

struct Ceilings {
    declared: Option<u64>,
    max: Option<u64>,
    quota: Option<u64>,
}

impl Ceilings {
    fn room(&self, accepted: u64) -> (u64, Stop) {
        let candidates = [
            (self.declared, Stop::Overrun),
            (self.max, Stop::TooLarge),
            (self.quota, Stop::Quota),
        ];
        candidates
            .into_iter()
            .filter_map(|(ceiling, stop)| ceiling.map(|ceiling| (ceiling, stop)))
            .min_by_key(|(ceiling, _)| *ceiling)
            .map_or((u64::MAX, Stop::Overrun), |(ceiling, stop)| {
                (ceiling.saturating_sub(accepted), stop)
            })
    }
}

struct Progress {
    buffer: Vec<u8>,
    written: u64,
    durable: u64,
    expires_at: Option<Timestamp>,
    hasher: Option<Sha256>,
    write_failed: bool,
}

impl Progress {
    fn accepted(&self) -> u64 {
        self.written + self.buffer.len() as u64
    }
}

enum Step {
    Shutdown,
    Frame(Option<Result<http_body::Frame<bytes::Bytes>, axum::Error>>),
    Tick,
    Idle,
}

impl Streamer {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        quota: QuotaService,
        holder: Arc<str>,
        limits: TusLimits,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            pools,
            clock,
            quota,
            holder,
            limits,
            shutdown,
        }
    }

    pub async fn run(
        &self,
        plan: StreamPlan,
        mut writer: Box<dyn StagingAppend>,
        mut body: Body,
    ) -> StreamOutcome {
        let mut progress = Progress {
            buffer: Vec::with_capacity(self.limits.buffer_bytes),
            written: plan.start,
            durable: plan.start,
            expires_at: None,
            hasher: plan.checksum.map(|_| Sha256::new()),
            write_failed: false,
        };
        let end = self
            .drive(&plan, writer.as_mut(), &mut body, &mut progress)
            .await;
        self.settle(&plan, writer.as_mut(), &mut progress, end)
            .await
    }

    pub async fn release(&self, id: TusUploadId) -> Result<(), TusError> {
        let now = Timestamp::try_from(self.clock.now()).map_err(TransferError::from)?;
        self.pools
            .write_tx(self.clock.as_ref(), RELEASE_TRANSACTION, async |tx| {
                repo::release_lease(tx.executor(), id, &self.holder, now).await
            })
            .await?;
        Ok(())
    }

    async fn drive(
        &self,
        plan: &StreamPlan,
        writer: &mut dyn StagingAppend,
        body: &mut Body,
        progress: &mut Progress,
    ) -> StreamEnd {
        let mut ceilings = Ceilings {
            declared: plan.declared,
            max: plan.declared.is_none().then_some(plan.max).flatten(),
            quota: None,
        };
        if plan.running_quota {
            match self.quota_ceiling(plan).await {
                Ok(ceiling) => ceilings.quota = ceiling,
                Err(error) => return StreamEnd::Internal(error),
            }
        }
        let heartbeat = self.limits.heartbeat_interval();
        let mut since_flush = 0_u64;
        let mut last_flush = Instant::now();
        let mut last_frame = Instant::now();
        loop {
            let tick_at = last_flush
                + if since_flush > 0 {
                    self.limits.flush_interval
                } else {
                    heartbeat
                };
            let idle_at = last_frame + self.limits.idle_timeout;
            let step = tokio::select! {
                biased;
                () = self.shutdown.cancelled() => Step::Shutdown,
                frame = body.frame() => Step::Frame(frame),
                () = sleep_until(tick_at) => Step::Tick,
                () = sleep_until(idle_at) => Step::Idle,
            };
            let mut due = false;
            match step {
                Step::Shutdown => return StreamEnd::Shutdown,
                Step::Idle => return StreamEnd::Idle,
                Step::Tick => due = true,
                Step::Frame(None) => return StreamEnd::Finished,
                Step::Frame(Some(Err(_))) => return StreamEnd::Disconnected,
                Step::Frame(Some(Ok(frame))) => {
                    last_frame = Instant::now();
                    let Ok(data) = frame.into_data() else {
                        continue;
                    };
                    let mut rest: &[u8] = &data;
                    while !rest.is_empty() {
                        let (room, stop) = ceilings.room(progress.accepted());
                        if room == 0 {
                            return match stop {
                                Stop::Overrun => StreamEnd::Overrun,
                                Stop::TooLarge => StreamEnd::TooLarge,
                                Stop::Quota => {
                                    StreamEnd::Quota(self.quota_exceeded(plan, progress).await)
                                }
                            };
                        }
                        let take =
                            usize::try_from(room).map_or(rest.len(), |room| room.min(rest.len()));
                        let (head, tail) = rest.split_at(take);
                        if !self.buffer(writer, progress, head).await {
                            return StreamEnd::WriteFailed;
                        }
                        since_flush += head.len() as u64;
                        rest = tail;
                    }
                }
            }
            if due
                || since_flush >= self.limits.flush_bytes
                || last_flush.elapsed()
                    >= if since_flush > 0 {
                        self.limits.flush_interval
                    } else {
                        heartbeat
                    }
            {
                if let Some(end) = self
                    .checkpoint(plan, writer, progress, since_flush > 0)
                    .await
                {
                    return end;
                }
                if plan.running_quota {
                    match self.quota_ceiling(plan).await {
                        Ok(ceiling) => ceilings.quota = ceiling,
                        Err(error) => return StreamEnd::Internal(error),
                    }
                }
                since_flush = 0;
                last_flush = Instant::now();
            }
        }
    }

    async fn buffer(
        &self,
        writer: &mut dyn StagingAppend,
        progress: &mut Progress,
        mut part: &[u8],
    ) -> bool {
        while !part.is_empty() {
            let space = self.limits.buffer_bytes - progress.buffer.len();
            let (now, later) = part.split_at(space.min(part.len()));
            if let Some(hasher) = progress.hasher.as_mut() {
                hasher.update(now);
            }
            progress.buffer.extend_from_slice(now);
            part = later;
            if progress.buffer.len() == self.limits.buffer_bytes
                && !self.drain(writer, progress).await
            {
                return false;
            }
        }
        true
    }

    async fn drain(&self, writer: &mut dyn StagingAppend, progress: &mut Progress) -> bool {
        if progress.buffer.is_empty() {
            return true;
        }
        match writer.write_chunk(&progress.buffer).await {
            Ok(()) => {
                progress.written += progress.buffer.len() as u64;
                progress.buffer.clear();
                true
            }
            Err(error) => {
                tracing::warn!(
                    storage_error = error.kind(),
                    "a write to upload staging failed"
                );
                progress.buffer.clear();
                progress.write_failed = true;
                false
            }
        }
    }

    async fn checkpoint(
        &self,
        plan: &StreamPlan,
        writer: &mut dyn StagingAppend,
        progress: &mut Progress,
        dirty: bool,
    ) -> Option<StreamEnd> {
        if dirty {
            if !self.drain(writer, progress).await {
                return Some(StreamEnd::WriteFailed);
            }
            if let Err(error) = writer.flush().await {
                tracing::warn!(
                    storage_error = error.kind(),
                    "upload staging could not be synced"
                );
                progress.write_failed = true;
                return Some(StreamEnd::WriteFailed);
            }
        }
        let acknowledged = if plan.checksum.is_some() {
            progress.durable
        } else {
            progress.written
        };
        match self.persist(plan.upload, acknowledged, false).await {
            Ok(Some(expires)) => {
                progress.durable = acknowledged;
                progress.expires_at = Some(expires);
                None
            }
            Ok(None) => Some(self.lost(plan).await),
            Err(error) => Some(StreamEnd::Internal(error)),
        }
    }

    async fn settle(
        &self,
        plan: &StreamPlan,
        writer: &mut dyn StagingAppend,
        progress: &mut Progress,
        end: StreamEnd,
    ) -> StreamOutcome {
        if matches!(end, StreamEnd::Gone | StreamEnd::LeaseLost) {
            return StreamOutcome {
                durable: progress.durable,
                expires_at: progress.expires_at,
                end,
            };
        }
        let drained = !progress.write_failed && self.drain(writer, progress).await;
        let flushed = writer.flush().await.is_ok();
        let (offset, end) = match plan.checksum {
            None => (
                if flushed {
                    progress.written
                } else {
                    progress.durable
                },
                if drained { end } else { StreamEnd::WriteFailed },
            ),
            Some(expected) => {
                self.verify(plan, writer, progress, end, expected, drained && flushed)
                    .await
            }
        };
        let end = match self.persist(plan.upload, offset, true).await {
            Ok(Some(expires)) => {
                progress.durable = offset;
                progress.expires_at = Some(expires);
                end
            }
            Ok(None) => self.lost(plan).await,
            Err(error) => StreamEnd::Internal(error),
        };
        StreamOutcome {
            durable: progress.durable,
            expires_at: progress.expires_at,
            end,
        }
    }

    async fn verify(
        &self,
        plan: &StreamPlan,
        writer: &mut dyn StagingAppend,
        progress: &mut Progress,
        end: StreamEnd,
        expected: [u8; CHECKSUM_DIGEST_LEN],
        intact: bool,
    ) -> (u64, StreamEnd) {
        let verified = matches!(end, StreamEnd::Finished)
            && intact
            && progress.hasher.take().is_some_and(|hasher| {
                let digest = hasher.finalize();
                bool::from(digest.as_slice().ct_eq(expected.as_slice()))
            });
        if verified {
            return (progress.written, end);
        }
        if let Err(error) = writer.truncate(plan.start).await {
            tracing::warn!(
                storage_error = error.kind(),
                "an unverified upload chunk could not be truncated; the next request reconciles it"
            );
        }
        progress.written = plan.start;
        let end = if !intact {
            StreamEnd::WriteFailed
        } else if matches!(end, StreamEnd::Finished) {
            StreamEnd::ChecksumMismatch
        } else {
            end
        };
        (plan.start, end)
    }

    async fn lost(&self, plan: &StreamPlan) -> StreamEnd {
        let mut connection = match self.pools.reader().executor().acquire().await {
            Ok(connection) => connection,
            Err(error) => {
                return StreamEnd::Internal(TusError::from(crate::infra::db::DbError::from(error)))
            }
        };
        match repo::find_upload(&mut connection, plan.owner, plan.upload).await {
            Ok(Some(row)) if matches!(row.state, TusState::Created | TusState::InProgress) => {
                StreamEnd::LeaseLost
            }
            Ok(_) => StreamEnd::Gone,
            Err(error) => StreamEnd::Internal(error.into()),
        }
    }

    pub async fn persist(
        &self,
        id: TusUploadId,
        offset: u64,
        release: bool,
    ) -> Result<Option<Timestamp>, TusError> {
        let now = Timestamp::try_from(self.clock.now()).map_err(TransferError::from)?;
        let lease_until =
            Timestamp::try_from(now.get() + self.limits.lease_ttl).map_err(TransferError::from)?;
        let extend_to =
            Timestamp::try_from(now.get() + self.limits.upload_ttl).map_err(TransferError::from)?;
        let written = self
            .pools
            .write_tx(self.clock.as_ref(), PERSIST_TRANSACTION, async |tx| {
                repo::persist_offset(
                    tx.executor(),
                    &OffsetWrite {
                        id,
                        offset,
                        holder: &self.holder,
                        lease_until,
                        release,
                        now,
                        extend_to,
                    },
                )
                .await
            })
            .await?;
        Ok(written)
    }

    async fn quota_ceiling(&self, plan: &StreamPlan) -> Result<Option<u64>, TusError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(crate::infra::db::DbError::from)?;
        let headroom = self
            .quota
            .running_headroom(&mut connection, plan.owner, plan.session)
            .await?;
        let others = repo::other_running_bytes(&mut connection, plan.session, plan.upload).await?;
        Ok(headroom.map(|headroom| headroom.get().saturating_sub(others.get())))
    }

    async fn quota_exceeded(&self, plan: &StreamPlan, progress: &Progress) -> QuotaError {
        use crate::domain::bytes::ByteSize;
        use crate::features::quota::error::Exceeded;
        let requested =
            ByteSize::try_from(progress.accepted().saturating_add(1)).unwrap_or(ByteSize::MAX);
        let mut connection = match self.pools.reader().executor().acquire().await {
            Ok(connection) => connection,
            Err(error) => return QuotaError::Db(crate::infra::db::DbError::from(error)),
        };
        match self
            .quota
            .running_check(&mut connection, plan.owner, plan.session, requested)
            .await
        {
            Err(error) => error,
            Ok(admission) => QuotaError::Exceeded(Exceeded {
                used: admission.usage.used,
                held: admission.usage.held,
                requested,
                quota: admission.usage.quota,
            }),
        }
    }
}

const RELEASE_TRANSACTION: &str = "tus.release_lease";

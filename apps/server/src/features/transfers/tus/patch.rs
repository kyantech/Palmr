use std::sync::Arc;

use axum::body::Body;

use crate::domain::bytes::ByteSize;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::storage::staging::StagingStorage;

use super::super::error::TransferError;
use super::append::{StreamEnd, StreamPlan};
use super::error::TusError;
use super::headers::CHECKSUM_DIGEST_LEN;
use super::reconcile;
use super::repo::{self, Acquired, Denial, LeaseGrant, LeasedUpload, TusUploadId};
use super::service::{state_invalid, upload_hex, TusService};

const ACQUIRE_TRANSACTION: &str = "tus.acquire_lease";
const LENGTH_TRANSACTION: &str = "tus.set_length";

pub struct PatchInput {
    pub client_offset: u64,
    pub checksum: Option<[u8; CHECKSUM_DIGEST_LEN]>,
    pub length: Option<ByteSize>,
    pub content_length: Option<u64>,
    pub body: Body,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Patched {
    pub offset: u64,
    pub expires_at: Timestamp,
}

struct Held {
    released: bool,
}

impl TusService {
    pub async fn patch(
        &self,
        owner: UserId,
        id: TusUploadId,
        input: PatchInput,
    ) -> Result<Patched, TusError> {
        let staging = self.local_staging()?;
        if !self.storage.is_available() {
            return Err(TusError::Transfer(TransferError::StorageUnavailable));
        }
        if self.shutdown.is_cancelled() {
            return Err(TusError::ShuttingDown);
        }
        let permit = self.permits.acquire().await?;
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            service.patch_task(staging, owner, id, input).await
        })
        .await
        .map_err(|_| TransferError::Invariant {
            what: "upload_stream",
        })?
    }

    async fn patch_task(
        &self,
        staging: Arc<dyn StagingStorage>,
        owner: UserId,
        id: TusUploadId,
        input: PatchInput,
    ) -> Result<Patched, TusError> {
        let Some(_guard) = self.locks.try_acquire(id) else {
            return Err(TusError::Locked);
        };
        let leased = self.acquire_lease(owner, id).await?;
        let mut held = Held { released: false };
        let outcome = self
            .patch_leased(&staging, owner, id, *leased, input, &mut held)
            .await;
        if !held.released {
            if let Err(error) = self.streamer.release(id).await {
                tracing::warn!(
                    kind = error.kind(),
                    "an upload lease could not be released; it expires on its own"
                );
            }
        }
        outcome
    }

    async fn acquire_lease(
        &self,
        owner: UserId,
        id: TusUploadId,
    ) -> Result<Box<LeasedUpload>, TusError> {
        let now = self.now()?;
        let until =
            Timestamp::try_from(now.get() + self.limits.lease_ttl).map_err(TransferError::from)?;
        let extend_to =
            Timestamp::try_from(now.get() + self.limits.upload_ttl).map_err(TransferError::from)?;
        let acquired = self
            .pools
            .write_tx(self.clock.as_ref(), ACQUIRE_TRANSACTION, async |tx| {
                repo::acquire_lease(
                    tx.executor(),
                    &LeaseGrant {
                        owner,
                        id,
                        holder: &self.holder,
                        now,
                        until,
                        extend_to,
                    },
                )
                .await
            })
            .await?;
        match acquired {
            Acquired::Held(leased) => Ok(leased),
            Acquired::Denied(Denial::NotFound) => Err(TusError::NotFound),
            Acquired::Denied(Denial::Gone) => Err(TusError::Gone),
            Acquired::Denied(Denial::Locked) => Err(TusError::Locked),
            Acquired::Denied(Denial::StateInvalid) => Err(state_invalid()),
        }
    }

    async fn patch_leased(
        &self,
        staging: &Arc<dyn StagingStorage>,
        owner: UserId,
        id: TusUploadId,
        leased: LeasedUpload,
        input: PatchInput,
        held: &mut Held,
    ) -> Result<Patched, TusError> {
        let PatchInput {
            client_offset,
            checksum,
            length,
            content_length,
            body,
            request_id,
        } = input;
        let hex = upload_hex(id)?;
        let mut writer = staging.ensure(&hex).await?;
        self.confirm_live(staging, owner, id, &hex).await?;

        let reconciled =
            reconcile::truncate_to_authoritative(writer.as_mut(), leased.row.offset.get()).await?;
        if reconciled.phantom_offset() || reconciled.torn_tail() {
            tracing::warn!(
                recorded = reconciled.recorded,
                staged = reconciled.staged,
                authoritative = reconciled.authoritative,
                "an upload offset was reconciled against its staging bytes"
            );
        }
        if reconciled.phantom_offset()
            && self
                .streamer
                .persist(id, reconciled.authoritative, false)
                .await?
                .is_none()
        {
            return Err(self.after_lost(owner, id).await);
        }
        if client_offset != reconciled.authoritative {
            return Err(TusError::OffsetMismatch {
                expected: reconciled.authoritative,
            });
        }

        let declared = self
            .settle_length(id, &leased, length, reconciled.authoritative)
            .await?;
        let max = self.max_size().map(ByteSize::get);
        self.precheck_body(declared, max, reconciled.authoritative, content_length)?;

        let plan = StreamPlan {
            upload: id,
            owner,
            session: leased.row.session,
            item: leased.row.item,
            start: reconciled.authoritative,
            declared,
            max,
            running_quota: leased.running_quota,
            checksum,
        };
        let outcome = self.streamer.run(plan, writer, body).await;
        held.released = !matches!(
            outcome.end,
            StreamEnd::Gone | StreamEnd::LeaseLost | StreamEnd::Internal(_)
        );
        self.conclude(owner, &plan, outcome.end, request_id.clone())
            .await?;
        if declared == Some(outcome.durable) {
            self.transfers
                .finalize_when_complete(leased.row.item, request_id);
        }
        let expires_at = outcome
            .expires_at
            .ok_or(TransferError::Invariant { what: "expires_at" })?;
        Ok(Patched {
            offset: outcome.durable,
            expires_at,
        })
    }

    async fn after_lost(&self, owner: UserId, id: TusUploadId) -> TusError {
        let now = match self.now() {
            Ok(now) => now,
            Err(error) => return error,
        };
        match self.live_row(owner, id, now).await {
            Ok(_) => TusError::Locked,
            Err(error) => error,
        }
    }

    async fn settle_length(
        &self,
        id: TusUploadId,
        leased: &LeasedUpload,
        requested: Option<ByteSize>,
        authoritative: u64,
    ) -> Result<Option<u64>, TusError> {
        let fixed = leased.row.length;
        let Some(requested) = requested else {
            return Ok(fixed.map(ByteSize::get));
        };
        if !leased.row.defer {
            return if fixed == Some(requested) {
                Ok(Some(requested.get()))
            } else {
                Err(TusError::LengthMismatch {
                    requested: Some(requested),
                    planned: fixed,
                })
            };
        }
        if let Some(max) = self.max_size() {
            if requested > max {
                return Err(TusError::TooLarge { max: Some(max) });
            }
        }
        if requested.get() < authoritative {
            return Err(TusError::LengthMismatch {
                requested: Some(requested),
                planned: None,
            });
        }
        let now = self.now()?;
        let applied = self
            .pools
            .write_tx(self.clock.as_ref(), LENGTH_TRANSACTION, async |tx| {
                repo::set_length(tx.executor(), id, &self.holder, requested, now).await
            })
            .await?;
        if applied {
            Ok(Some(requested.get()))
        } else {
            Err(TusError::LengthMismatch {
                requested: Some(requested),
                planned: None,
            })
        }
    }

    fn precheck_body(
        &self,
        declared: Option<u64>,
        max: Option<u64>,
        start: u64,
        content_length: Option<u64>,
    ) -> Result<(), TusError> {
        let Some(content_length) = content_length else {
            return Ok(());
        };
        let end = start
            .checked_add(content_length)
            .ok_or(TusError::LengthBeyondRange)?;
        match (declared, max) {
            (Some(declared), _) if end > declared => Err(TusError::Overrun {
                declared: ByteSize::try_from(declared).unwrap_or(ByteSize::ZERO),
            }),
            (None, Some(max)) if end > max => Err(TusError::TooLarge {
                max: ByteSize::try_from(max).ok(),
            }),
            _ => Ok(()),
        }
    }
}

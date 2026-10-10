use std::sync::Arc;

use sqlx::SqliteConnection;

use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::features::users::service::effective_quota;
use crate::infra::db::WriteTx;

use super::arith::{add, evaluate, exceeds_reservation, sub};
use super::error::{Exceeded, QuotaError};
use super::model::{
    Adjustment, Admission, Hold, HoldRequest, ItemSettlement, ReleaseReason, ReservationRow,
    ReservationState, SettledItem, Settlement, SizeCoverage, TransferSessionId, Usage,
};
use super::repo;

#[derive(Clone)]
pub struct QuotaService {
    settings: SettingsHandle,
    clock: Arc<dyn Clock>,
}

impl QuotaService {
    pub fn new(settings: SettingsHandle, clock: Arc<dyn Clock>) -> Self {
        Self { settings, clock }
    }

    fn now(&self) -> Result<Timestamp, QuotaError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }

    fn default_quota(&self) -> Option<ByteSize> {
        self.settings.load().quotas.default_user_quota_bytes
    }

    pub async fn effective_quota(
        &self,
        connection: &mut SqliteConnection,
        owner: UserId,
    ) -> Result<Option<ByteSize>, QuotaError> {
        let row = repo::owner(connection, owner)
            .await?
            .ok_or(QuotaError::OwnerNotFound)?;
        Ok(effective_quota(row.quota, self.default_quota()))
    }

    async fn usage(
        &self,
        connection: &mut SqliteConnection,
        owner: UserId,
        excluding: Option<TransferSessionId>,
        require_active: bool,
    ) -> Result<Usage, QuotaError> {
        let row = repo::owner(connection, owner)
            .await?
            .ok_or(QuotaError::OwnerNotFound)?;
        if require_active && !row.active {
            return Err(QuotaError::OwnerInactive);
        }
        let held = repo::held_total(connection, owner, excluding).await?;
        Ok(Usage {
            used: row.used,
            held,
            quota: effective_quota(row.quota, self.default_quota()),
        })
    }

    pub async fn admit(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        requested: ByteSize,
    ) -> Result<Admission, QuotaError> {
        let usage = self.usage(tx.executor(), owner, None, true).await?;
        evaluate(usage, requested)
    }

    pub async fn running_check(
        &self,
        connection: &mut SqliteConnection,
        owner: UserId,
        session: TransferSessionId,
        in_flight: ByteSize,
    ) -> Result<Admission, QuotaError> {
        let usage = self.usage(connection, owner, Some(session), false).await?;
        evaluate(usage, in_flight)
    }

    pub async fn running_headroom(
        &self,
        connection: &mut SqliteConnection,
        owner: UserId,
        session: TransferSessionId,
    ) -> Result<Option<ByteSize>, QuotaError> {
        let usage = self.usage(connection, owner, Some(session), false).await?;
        let committed = add(usage.used, usage.held, "used_plus_held")?;
        Ok(usage
            .quota
            .map(|quota| quota.checked_sub(committed).unwrap_or(ByteSize::ZERO)))
    }

    pub async fn hold(
        &self,
        tx: &mut WriteTx<'_>,
        request: HoldRequest,
    ) -> Result<Hold, QuotaError> {
        let binding = repo::session_binding(tx.executor(), request.session)
            .await?
            .ok_or(QuotaError::SessionNotFound)?;
        if binding.context != request.context || binding.owner != Some(request.owner) {
            return Err(QuotaError::OwnerContextMismatch);
        }
        if let Some(existing) = repo::find_by_session(tx.executor(), request.session).await? {
            return match existing.state {
                ReservationState::Held if is_equivalent(&existing, &request) => {
                    Ok(Hold::Reused(existing))
                }
                ReservationState::Held => Err(QuotaError::ReservationMismatch),
                ReservationState::Committed | ReservationState::Released => {
                    Err(QuotaError::SessionAlreadySettled)
                }
            };
        }
        let now = self.now()?;
        if request.expires_at <= now {
            return Err(QuotaError::InvalidExpiry);
        }
        self.admit(tx, request.owner, request.reserved).await?;
        let row = ReservationRow {
            id: Id::generate(self.clock.as_ref()),
            user_id: request.owner,
            session_id: request.session,
            context: request.context,
            reserved: request.reserved,
            committed: None,
            state: ReservationState::Held,
            created_at: now,
            expires_at: request.expires_at,
            settled_at: None,
            release_reason: None,
        };
        repo::insert_held(tx.executor(), &row).await?;
        Ok(Hold::Created(row))
    }

    pub async fn adjust(
        &self,
        tx: &mut WriteTx<'_>,
        session: TransferSessionId,
        reserved: ByteSize,
    ) -> Result<Adjustment, QuotaError> {
        let row = require_held(tx.executor(), session).await?;
        if reserved > row.reserved {
            let additional = sub(reserved, row.reserved, "reservation_increase")?;
            self.admit(tx, row.user_id, additional).await?;
        }
        if reserved != row.reserved
            && !repo::update_reserved(tx.executor(), row.id, reserved).await?
        {
            return Err(QuotaError::StateConflict {
                current: ReservationState::Released,
            });
        }
        Ok(Adjustment {
            previous: row.reserved,
            current: reserved,
        })
    }

    pub async fn release_share(
        &self,
        tx: &mut WriteTx<'_>,
        session: TransferSessionId,
        share: ByteSize,
    ) -> Result<ByteSize, QuotaError> {
        let row = require_held(tx.executor(), session).await?;
        let released = row.reserved.min(share);
        let remaining = sub(row.reserved, released, "reservation_decrease")?;
        if released != ByteSize::ZERO
            && !repo::update_reserved(tx.executor(), row.id, remaining).await?
        {
            return Err(QuotaError::StateConflict {
                current: ReservationState::Released,
            });
        }
        Ok(released)
    }

    pub async fn settle_item(
        &self,
        tx: &mut WriteTx<'_>,
        item: ItemSettlement,
    ) -> Result<SettledItem, QuotaError> {
        let row = require_held(tx.executor(), item.session).await?;
        let usage = self.usage(tx.executor(), row.user_id, None, true).await?;
        let rejected = Exceeded {
            used: usage.used,
            held: usage.held,
            requested: item.authoritative,
            quota: usage.quota,
        };
        if item.coverage == SizeCoverage::Reserved
            && exceeds_reservation(item.share, item.authoritative)?
        {
            return Err(QuotaError::Exceeded(rejected));
        }
        let projected = add(usage.used, item.authoritative, "used_plus_authoritative")?;
        if usage.quota.is_some_and(|quota| projected > quota) {
            return Err(QuotaError::Exceeded(rejected));
        }
        let released = row.reserved.min(item.share);
        let remaining = sub(row.reserved, released, "reservation_decrease")?;
        if released != ByteSize::ZERO
            && !repo::update_reserved(tx.executor(), row.id, remaining).await?
        {
            return Err(QuotaError::StateConflict {
                current: ReservationState::Released,
            });
        }
        let used = increment_used(tx, row.user_id, item.authoritative).await?;
        Ok(SettledItem {
            released_share: released,
            used,
        })
    }

    pub async fn commit(
        &self,
        tx: &mut WriteTx<'_>,
        session: TransferSessionId,
        committed: ByteSize,
    ) -> Result<Settlement, QuotaError> {
        let row = repo::find_by_session(tx.executor(), session)
            .await?
            .ok_or(QuotaError::ReservationNotFound)?;
        match row.state {
            ReservationState::Held => {
                let now = self.now()?;
                if !repo::commit_held(tx.executor(), row.id, committed, now).await? {
                    return Err(QuotaError::StateConflict {
                        current: ReservationState::Released,
                    });
                }
                Ok(Settlement::Applied(ReservationRow {
                    state: ReservationState::Committed,
                    committed: Some(committed),
                    settled_at: Some(now),
                    release_reason: None,
                    ..row
                }))
            }
            ReservationState::Committed if row.committed == Some(committed) => {
                Ok(Settlement::AlreadySettled(row))
            }
            ReservationState::Committed | ReservationState::Released => {
                Err(QuotaError::StateConflict { current: row.state })
            }
        }
    }

    pub async fn release(
        &self,
        tx: &mut WriteTx<'_>,
        session: TransferSessionId,
        reason: ReleaseReason,
    ) -> Result<Settlement, QuotaError> {
        let row = repo::find_by_session(tx.executor(), session)
            .await?
            .ok_or(QuotaError::ReservationNotFound)?;
        match row.state {
            ReservationState::Held => {
                let now = self.now()?;
                if !repo::release_held(tx.executor(), row.id, reason, now).await? {
                    return Err(QuotaError::StateConflict {
                        current: ReservationState::Released,
                    });
                }
                Ok(Settlement::Applied(ReservationRow {
                    state: ReservationState::Released,
                    settled_at: Some(now),
                    release_reason: Some(reason),
                    ..row
                }))
            }
            ReservationState::Released => Ok(Settlement::AlreadySettled(row)),
            ReservationState::Committed => Err(QuotaError::StateConflict { current: row.state }),
        }
    }
}

fn is_equivalent(existing: &ReservationRow, request: &HoldRequest) -> bool {
    existing.user_id == request.owner
        && existing.context == request.context
        && existing.reserved == request.reserved
        && existing.expires_at == request.expires_at
}

async fn require_held(
    connection: &mut SqliteConnection,
    session: TransferSessionId,
) -> Result<ReservationRow, QuotaError> {
    let row = repo::find_by_session(connection, session)
        .await?
        .ok_or(QuotaError::ReservationNotFound)?;
    if row.state == ReservationState::Held {
        Ok(row)
    } else {
        Err(QuotaError::StateConflict { current: row.state })
    }
}

pub async fn increment_used(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    delta: ByteSize,
) -> Result<ByteSize, QuotaError> {
    let current = repo::owner(tx.executor(), owner)
        .await?
        .ok_or(QuotaError::OwnerNotFound)?
        .used;
    let next = add(current, delta, "increment_used")?;
    write_used(tx, owner, current, next).await
}

pub async fn decrement_used(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    delta: ByteSize,
) -> Result<ByteSize, QuotaError> {
    let current = repo::owner(tx.executor(), owner)
        .await?
        .ok_or(QuotaError::OwnerNotFound)?
        .used;
    let next = sub(current, delta, "decrement_used")?;
    write_used(tx, owner, current, next).await
}

async fn write_used(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    current: ByteSize,
    next: ByteSize,
) -> Result<ByteSize, QuotaError> {
    if repo::set_used(tx.executor(), owner, current, next).await? {
        Ok(next)
    } else {
        Err(QuotaError::Integrity {
            column: "users.used_bytes",
        })
    }
}

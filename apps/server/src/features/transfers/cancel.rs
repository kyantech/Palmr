use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::actions::{self, TransferEndedFacts, TransferScope};
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::quota::model::ReleaseReason;
use crate::features::quota::service::QuotaService;
use crate::features::users::model::UserId;
use crate::infra::db::WriteTx;
use crate::storage::key::ObjectKey;
use crate::storage::lifecycle::{
    parse_provider, tombstone_uncommitted, DeletionReason, PlacedObject,
};

use super::error::TransferError;
use super::model::{TransferProvider, TransferSessionId};
use super::repo::{self, FinalizeStage, LiveItem, SessionRow};
use super::state::{
    session_transition, ItemState, Outcome as Moved, TransferSessionState, Trigger,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Canceled,
    Expired,
}

impl Ending {
    pub const fn item_state(self) -> ItemState {
        match self {
            Self::Canceled => ItemState::Canceled,
            Self::Expired => ItemState::Expired,
        }
    }

    pub const fn tus_state(self) -> &'static str {
        match self {
            Self::Canceled => "terminated",
            Self::Expired => "expired",
        }
    }
}

pub async fn end_items(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    provider: TransferProvider,
    items: &[LiveItem],
    ending: Ending,
    now: Timestamp,
) -> Result<u64, TransferError> {
    let ids: Vec<_> = items.iter().map(|item| item.id).collect();
    let ended = repo::end_items(tx.executor(), &ids, ending, now).await?;
    tombstone_placed(tx, clock, provider, items).await?;
    Ok(ended)
}

pub async fn tombstone_placed(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    provider: TransferProvider,
    items: &[LiveItem],
) -> Result<(), TransferError> {
    let kind =
        parse_provider(provider.as_str()).ok_or(TransferError::Invariant { what: "provider" })?;
    for item in items
        .iter()
        .filter(|item| item.finalize_stage == FinalizeStage::Placing)
    {
        let key =
            ObjectKey::parse(&item.final_object_key).map_err(|_| TransferError::Invariant {
                what: "final_object_key",
            })?;
        tombstone_uncommitted(
            tx,
            clock,
            &PlacedObject {
                id: item.final_object_id,
                key: &key,
                provider: kind,
                measured_size: None,
            },
            DeletionReason::UploadAbandoned,
        )
        .await?;
    }
    Ok(())
}

pub async fn settle_session(
    tx: &mut WriteTx<'_>,
    quota: &QuotaService,
    session: &SessionRow,
    reason: ReleaseReason,
) -> Result<(), TransferError> {
    if session.completed_file_count > 0 {
        quota
            .commit(tx, session.id, session.completed_bytes)
            .await?;
    } else {
        quota.release(tx, session.id, reason).await?;
    }
    Ok(())
}

pub async fn settle_if_held(
    tx: &mut WriteTx<'_>,
    quota: &QuotaService,
    session: &SessionRow,
    reason: ReleaseReason,
) -> Result<bool, TransferError> {
    if !repo::reservation_held(tx.executor(), session.id).await? {
        return Ok(false);
    }
    settle_session(tx, quota, session, reason).await?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpiredShare {
    pub session_closed: bool,
    pub completed_kept: u64,
}

pub async fn settle_expired_item(
    tx: &mut WriteTx<'_>,
    quota: &QuotaService,
    session: (TransferSessionId, TransferSessionState),
    item: &LiveItem,
    now: Timestamp,
) -> Result<ExpiredShare, TransferError> {
    let (session_id, session_state) = session;
    let mut settled = ExpiredShare::default();
    if session_state.is_terminal() {
        return Ok(settled);
    }
    let row = repo::find_session_unowned(tx.executor(), session_id)
        .await?
        .ok_or(TransferError::SessionNotFound)?;
    settled.completed_kept = u64::from(row.completed_file_count);
    if repo::reservation_held(tx.executor(), session_id).await? {
        if item.state.holds_reservation() {
            quota.release_share(tx, session_id, item.reserved).await?;
        }
        if repo::remaining_items(tx.executor(), session_id).await? == 0 {
            if let Ok(Moved::Moved(_)) = session_transition(row.state, Trigger::Expire) {
                let moved =
                    repo::expire_session(tx.executor(), session_id, row.state, now, false).await?;
                if !moved {
                    return Err(TransferError::Invariant {
                        what: "session_state",
                    });
                }
                settle_session(tx, quota, &row, ReleaseReason::Expired).await?;
                settled.session_closed = true;
                return Ok(settled);
            }
        }
    }
    repo::touch_session(tx.executor(), session_id, now).await?;
    Ok(settled)
}

#[derive(Debug, Clone)]
pub struct TransferCaller {
    pub owner: UserId,
    pub username: String,
    pub client: ClientMetadata,
}

pub struct Ended<'a> {
    pub scope: TransferScope,
    pub session: &'a SessionRow,
    pub item_id: Option<String>,
    pub items_ended: u64,
    pub session_closed: bool,
}

pub async fn record_canceled(
    tx: &mut WriteTx<'_>,
    audit: &AuditService,
    caller: &TransferCaller,
    ended: Ended<'_>,
    at: Timestamp,
) -> Result<(), TransferError> {
    let session_id = ended.session.id.to_string();
    let spec = actions::transfer_canceled(&TransferEndedFacts {
        scope: ended.scope,
        transfer_session_id: &session_id,
        item_id: ended.item_id.as_deref(),
        provider: ended.session.provider.as_str(),
        items_ended: ended.items_ended,
        completed_items_kept: u64::from(ended.session.completed_file_count),
        session_closed: ended.session_closed,
        owner_id: None,
    });
    let event = AuditEvent::new(
        spec,
        Actor::user(&caller.owner.to_string(), &caller.username),
        Outcome::Success,
        at,
    )
    .with_target(Target::new(TargetType::Session).id(&session_id))
    .with_client(caller.client.clone());
    audit.record_in_tx(tx, &event).await.map_err(|error| {
        tracing::error!(kind = %error, "the transfer cancellation audit event could not be recorded");
        TransferError::Invariant { what: "audit_event" }
    })
}

pub struct Expired<'a> {
    pub scope: TransferScope,
    pub session: &'a str,
    pub item_id: Option<&'a str>,
    pub upload: Option<&'a str>,
    pub provider: &'static str,
    pub items_ended: u64,
    pub completed_items_kept: u64,
    pub session_closed: bool,
    pub owner: Option<&'a str>,
}

pub fn record_expired(audit: &AuditService, expired: &Expired<'_>, at: Timestamp) {
    let spec = actions::transfer_expired(&TransferEndedFacts {
        scope: expired.scope,
        transfer_session_id: expired.session,
        item_id: expired.item_id,
        provider: expired.provider,
        items_ended: expired.items_ended,
        completed_items_kept: expired.completed_items_kept,
        session_closed: expired.session_closed,
        owner_id: expired.owner,
    });
    let target = match (expired.upload, expired.provider) {
        (Some(upload), "s3") => Target::new(TargetType::S3MultipartUpload).id(upload),
        (Some(upload), _) => Target::new(TargetType::TusUpload).id(upload),
        (None, _) => Target::new(TargetType::Session).id(expired.session),
    };
    audit.record_async(
        AuditEvent::new(spec, Actor::system(), Outcome::Success, at).with_target(target),
    );
}

use std::collections::HashMap;
use std::sync::Arc;

use http::StatusCode;
use sqlx::Connection;
use utoipa::openapi::path::Parameter;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::folders::MAX_FOLDER_DEPTH;
use crate::features::folders::{
    ensure_child_in_tx, resolve_writable_folder, FolderError, FolderId,
};
use crate::features::quota::model::{HoldRequest, ReleaseReason, ReservationContext};
use crate::features::quota::service::QuotaService;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::idempotency::{Claim, IdempotencyService, ReplayEnvelope};
use crate::infra::http::pagination::{
    cursor_parameter, limit_parameter, repeated_enum_parameter, CursorKey, Page, PageRequest,
    QueryParams, SortAllowlist, SortDirection, SortField, SortKeyKind, SortValue, TotalCount,
};
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::lifecycle::{
    parse_provider, tombstone_uncommitted, DeletionReason, PlacedObject, StorageObjectId,
};

use super::admission::{exceeds_depth, plan_files, reservation, TransferStorage};
use super::error::TransferError;
use super::model::{
    SessionItemId, TransferFileView, TransferProvider, TransferSessionId, TransferSessionSummary,
    TransferSessionView, ValidatedSession, SESSION_TTL,
};
use super::presentation::{file_view, session_view, summary};
use super::repo::{
    self, ErrorParts, FinalizeStage, ItemRow, LiveItem, NewItem, NewSession, SessionMove,
    SessionRow,
};
use super::state::{
    has_expired, item_transition, session_transition, ItemState, Outcome, TransferSessionState,
    Trigger,
};

const CREATE_TRANSACTION: &str = "transfers.create_session";
const CANCEL_TRANSACTION: &str = "transfers.cancel_session";
const COMPLETE_TRANSACTION: &str = "transfers.complete_session";
const RETRY_TRANSACTION: &str = "transfers.retry_item";
const CANCEL_ITEM_TRANSACTION: &str = "transfers.cancel_item";
const FAIL_ITEM_TRANSACTION: &str = "transfers.fail_item";

pub const STATE_PARAM: &str = "state";

static SESSION_SORT_FIELDS: [SortField; 1] = [SortField::new(
    "createdAt",
    "ts.created_at",
    SortKeyKind::Text,
)];
static SESSION_SORT: SortAllowlist =
    SortAllowlist::new(&SESSION_SORT_FIELDS, 0, SortDirection::Desc).with_id_column("ts.id");

#[derive(Debug, Clone)]
pub struct ListQuery {
    states: Vec<TransferSessionState>,
    binding: String,
    page: PageRequest,
}

#[derive(Clone)]
pub struct TransferService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
    settings: SettingsHandle,
    quota: QuotaService,
    storage: TransferStorage,
    idempotency: IdempotencyService,
}

impl TransferService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        keys: Arc<KeyRing>,
        settings: SettingsHandle,
        storage: TransferStorage,
    ) -> Self {
        let quota = QuotaService::new(settings.clone(), Arc::clone(&clock));
        let idempotency =
            IdempotencyService::new(pools.clone(), Arc::clone(&clock), Arc::clone(&keys));
        Self {
            pools,
            clock,
            keys,
            settings,
            quota,
            storage,
            idempotency,
        }
    }

    pub const fn idempotency(&self) -> &IdempotencyService {
        &self.idempotency
    }

    pub fn list_parameters() -> Vec<Parameter> {
        let states: Vec<&'static str> = TransferSessionState::ALL
            .iter()
            .map(|s| s.as_str())
            .collect();
        vec![
            repeated_enum_parameter(STATE_PARAM, &states),
            cursor_parameter(),
            limit_parameter(),
        ]
    }

    pub fn list_query(&self, raw_query: Option<&str>) -> Result<ListQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        let mut states = params.repeated(STATE_PARAM, TransferSessionState::parse)?;
        states.sort_by_key(|state| state.as_str());
        states.dedup();
        let binding = format!(
            "states:{}",
            states
                .iter()
                .map(|state| state.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        let page =
            PageRequest::from_bound_query(&params, &SESSION_SORT, self.keys.as_ref(), 1, &binding)?;
        Ok(ListQuery {
            states,
            binding,
            page,
        })
    }

    fn now(&self) -> Result<Timestamp, TransferError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }

    pub async fn create(
        &self,
        owner: UserId,
        request: ValidatedSession,
        claim: &Claim,
    ) -> Result<ReplayEnvelope, TransferError> {
        let now = self.now()?;
        let expires_at = Timestamp::try_from(now.get() + SESSION_TTL)?;
        let id = TransferSessionId::generate(self.clock.as_ref());
        self.pools
            .write_tx(self.clock.as_ref(), CREATE_TRANSACTION, async |tx| {
                let view = self.admit(tx, owner, &request, id, now, expires_at).await?;
                let body = serde_json::to_value(&view)
                    .map_err(|_| TransferError::Invariant { what: "response" })?;
                let envelope = ReplayEnvelope::new(StatusCode::CREATED, body);
                self.idempotency.complete(tx, claim, &envelope).await?;
                Ok::<_, TransferError>(envelope)
            })
            .await
    }

    async fn admit(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        request: &ValidatedSession,
        id: TransferSessionId,
        now: Timestamp,
        expires_at: Timestamp,
    ) -> Result<TransferSessionView, TransferError> {
        if repo::owner_is_active(tx.executor(), owner).await? != Some(true) {
            return Err(TransferError::AccountInactive);
        }
        let policy_max = self.settings.load().quotas.max_file_size_bytes;
        if !self.storage.is_available() {
            return Err(TransferError::StorageUnavailable);
        }
        let base_depth = match request.target {
            None => 0,
            Some(folder) => {
                let target = resolve_writable_folder(tx.executor(), owner, folder).await?;
                i64::from(target.depth) + 1
            }
        };
        let kinds = plan_files(&request.files, &self.storage, policy_max)?;
        if exceeds_depth(base_depth, &request.files) {
            return Err(TransferError::Folder(FolderError::DepthExceeded));
        }
        let reserved = reservation(
            &request.files,
            self.storage.effective_max_file_size(policy_max),
        )?;
        self.materialize_folders(tx, owner, request, base_depth, now)
            .await?;
        self.quota.admit(tx, owner, reserved.total).await?;

        let file_count = u32::try_from(request.files.len())
            .map_err(|_| TransferError::Invariant { what: "file_count" })?;
        repo::insert_session(
            tx.executor(),
            &NewSession {
                id,
                owner,
                provider: self.storage.provider(),
                target: request.target,
                file_count,
                declared_bytes: reserved.declared_total,
                now,
                expires_at,
            },
        )
        .await?;

        let identities: Vec<(SessionItemId, StorageObjectId, ObjectKey)> = request
            .files
            .iter()
            .map(|_| {
                (
                    SessionItemId::generate(self.clock.as_ref()),
                    StorageObjectId::generate(self.clock.as_ref()),
                    ObjectKey::allocate(KeyNamespace::Objects),
                )
            })
            .collect();
        let items: Vec<NewItem<'_>> = request
            .files
            .iter()
            .zip(&identities)
            .zip(kinds.iter().zip(&reserved.shares))
            .map(
                |((file, (item_id, object_id, key)), (kind, share))| NewItem {
                    id: *item_id,
                    ordinal: file.ordinal,
                    client_key: file.client_key.as_str(),
                    name: file.name.display(),
                    directory: file.relative_path(),
                    declared: file.size,
                    reserved: *share,
                    kind: *kind,
                    final_object_id: *object_id,
                    final_object_key: key.as_str(),
                },
            )
            .collect();
        repo::insert_items(tx.executor(), id, &items, now).await?;

        self.quota
            .hold(
                tx,
                HoldRequest {
                    owner,
                    session: id,
                    context: ReservationContext::MyFiles,
                    reserved: reserved.total,
                    expires_at,
                },
            )
            .await?;
        self.view_in_tx(tx, owner, id).await
    }

    async fn materialize_folders(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        request: &ValidatedSession,
        base_depth: i64,
        now: Timestamp,
    ) -> Result<(), TransferError> {
        let mut known: HashMap<String, (FolderId, i64)> = HashMap::new();
        for file in &request.files {
            let mut key = String::new();
            let mut parent = request.target;
            let mut depth = base_depth;
            for segment in file.directory.segments() {
                if !key.is_empty() {
                    key.push('/');
                }
                key.push_str(segment.normalized());
                if let Some((folder, folder_depth)) = known.get(&key).copied() {
                    parent = Some(folder);
                    depth = folder_depth + 1;
                    continue;
                }
                if depth > MAX_FOLDER_DEPTH {
                    return Err(TransferError::Folder(FolderError::DepthExceeded));
                }
                let step =
                    ensure_child_in_tx(tx, self.clock.as_ref(), owner, parent, depth, segment, now)
                        .await?;
                known.insert(key.clone(), (step.id, depth));
                parent = Some(step.id);
                depth += 1;
            }
        }
        Ok(())
    }

    async fn view_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        id: TransferSessionId,
    ) -> Result<TransferSessionView, TransferError> {
        let session = repo::find_session(tx.executor(), owner, id)
            .await?
            .ok_or(TransferError::SessionNotFound)?;
        let items = repo::session_items(tx.executor(), id).await?;
        session_view(&session, &items, &self.storage)
    }

    pub async fn list(
        &self,
        owner: UserId,
        query: ListQuery,
    ) -> Result<Page<TransferSessionSummary>, TransferError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        let mut snapshot = connection.begin().await?;
        let rows = repo::list_sessions(&mut snapshot, owner, &query.states, &query.page).await?;
        let total = repo::count_sessions(&mut snapshot, owner, &query.states).await?;
        snapshot.commit().await?;
        let binding = query.binding;
        let page = query.page.into_page(
            rows,
            self.keys.as_ref(),
            |row: &SessionRow, _field| {
                CursorKey::in_group(0, SortValue::Text(row.created_at.to_string()), row.id)
                    .bound_to(binding.clone())
            },
            TotalCount::Exact(total),
        );
        Ok(Page {
            items: page.items.iter().map(summary).collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn detail(
        &self,
        owner: UserId,
        id: TransferSessionId,
    ) -> Result<TransferSessionView, TransferError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        let mut snapshot = connection.begin().await?;
        let session = repo::find_session(&mut snapshot, owner, id)
            .await?
            .ok_or(TransferError::SessionNotFound)?;
        let items = repo::session_items(&mut snapshot, id).await?;
        snapshot.commit().await?;
        session_view(&session, &items, &self.storage)
    }

    pub async fn cancel(&self, owner: UserId, id: TransferSessionId) -> Result<(), TransferError> {
        let now = self.now()?;
        self.pools
            .write_tx(self.clock.as_ref(), CANCEL_TRANSACTION, async |tx| {
                let session = repo::find_session(tx.executor(), owner, id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                match session_transition(session.state, Trigger::Cancel) {
                    Err(_) => Err(TransferError::StateInvalid),
                    Ok(Outcome::Unchanged(_)) => Ok(()),
                    Ok(Outcome::Moved(_)) => {
                        self.cancel_session_in_tx(tx, owner, &session, now).await
                    }
                }
            })
            .await
    }

    async fn cancel_session_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        session: &SessionRow,
        now: Timestamp,
    ) -> Result<(), TransferError> {
        let live = repo::live_items(tx.executor(), session.id).await?;
        let ids: Vec<SessionItemId> = live.iter().map(|item| item.id).collect();
        repo::cancel_items(tx.executor(), &ids, now).await?;
        self.tombstone_placed(tx, session.provider, &live).await?;
        self.close_session(tx, owner, session, TransferSessionState::Canceled, now)
            .await?;
        self.settle_reservation(tx, session, ReleaseReason::Canceled)
            .await
    }

    async fn close_session(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        session: &SessionRow,
        to: TransferSessionState,
        now: Timestamp,
    ) -> Result<(), TransferError> {
        let moved = repo::move_session(
            tx.executor(),
            owner,
            session.id,
            &SessionMove {
                from: session.state,
                to,
                now,
                finished: true,
                cancel_requested: to == TransferSessionState::Canceled,
            },
        )
        .await?;
        if moved {
            Ok(())
        } else {
            Err(TransferError::Invariant {
                what: "session_state",
            })
        }
    }

    async fn settle_reservation(
        &self,
        tx: &mut WriteTx<'_>,
        session: &SessionRow,
        reason: ReleaseReason,
    ) -> Result<(), TransferError> {
        if session.completed_file_count > 0 {
            self.quota
                .commit(tx, session.id, session.completed_bytes)
                .await?;
        } else {
            self.quota.release(tx, session.id, reason).await?;
        }
        Ok(())
    }

    async fn tombstone_placed(
        &self,
        tx: &mut WriteTx<'_>,
        provider: TransferProvider,
        items: &[LiveItem],
    ) -> Result<(), TransferError> {
        let kind = parse_provider(provider.as_str())
            .ok_or(TransferError::Invariant { what: "provider" })?;
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
                self.clock.as_ref(),
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

    pub async fn complete(
        &self,
        owner: UserId,
        id: TransferSessionId,
    ) -> Result<TransferSessionView, TransferError> {
        let now = self.now()?;
        self.pools
            .write_tx(self.clock.as_ref(), COMPLETE_TRANSACTION, async |tx| {
                let session = repo::find_session(tx.executor(), owner, id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                match session_transition(session.state, Trigger::Close) {
                    Err(_) => return Err(TransferError::StateInvalid),
                    Ok(Outcome::Unchanged(_)) => return self.view_in_tx(tx, owner, id).await,
                    Ok(Outcome::Moved(_)) => {}
                }
                if has_expired(now, session.expires_at) {
                    return Err(TransferError::Expired);
                }
                if repo::in_flight_items(tx.executor(), id).await? > 0 {
                    return Err(TransferError::StateInvalid);
                }
                let items = repo::session_items(tx.executor(), id).await?;
                verify_completed(&session, &items)?;
                let failed = repo::failed_items(tx.executor(), id).await? > 0;
                self.close_session(tx, owner, &session, TransferSessionState::Completed, now)
                    .await?;
                let reason = if failed {
                    ReleaseReason::Failed
                } else {
                    ReleaseReason::Canceled
                };
                self.settle_reservation(tx, &session, reason).await?;
                self.view_in_tx(tx, owner, id).await
            })
            .await
    }

    pub async fn retry_item(
        &self,
        owner: UserId,
        id: TransferSessionId,
        item_id: SessionItemId,
    ) -> Result<TransferFileView, TransferError> {
        let now = self.now()?;
        self.pools
            .write_tx(self.clock.as_ref(), RETRY_TRANSACTION, async |tx| {
                let session = repo::find_session(tx.executor(), owner, id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                let item = repo::one_item(tx.executor(), id, item_id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                if session.state.is_terminal() {
                    return Err(TransferError::StateInvalid);
                }
                if has_expired(now, session.expires_at) {
                    return Err(TransferError::Expired);
                }
                let outcome = item_transition(item.state, Trigger::Retry)
                    .map_err(|_| TransferError::StateInvalid)?;
                if outcome.moved() {
                    if !repo::protocol_resource_present(tx.executor(), item_id).await? {
                        return Err(TransferError::StateInvalid);
                    }
                    let session_outcome = session_transition(session.state, Trigger::Retry)
                        .map_err(|_| TransferError::StateInvalid)?;
                    if !repo::retry_item(tx.executor(), id, item_id, now).await? {
                        return Err(TransferError::StateInvalid);
                    }
                    if let Outcome::Moved(to) = session_outcome {
                        let moved = repo::move_session(
                            tx.executor(),
                            owner,
                            id,
                            &SessionMove {
                                from: session.state,
                                to,
                                now,
                                finished: false,
                                cancel_requested: false,
                            },
                        )
                        .await?;
                        if !moved {
                            return Err(TransferError::Invariant {
                                what: "session_state",
                            });
                        }
                    } else {
                        repo::touch_session(tx.executor(), id, now).await?;
                    }
                }
                let items = repo::session_items(tx.executor(), id).await?;
                items
                    .iter()
                    .find(|row| row.id == item_id)
                    .map(|row| file_view(row, &self.storage))
                    .ok_or(TransferError::SessionNotFound)
            })
            .await
    }

    pub async fn fail_item(
        &self,
        owner: UserId,
        id: TransferSessionId,
        item_id: SessionItemId,
        failure: ErrorParts,
    ) -> Result<(), TransferError> {
        let now = self.now()?;
        self.pools
            .write_tx(self.clock.as_ref(), FAIL_ITEM_TRANSACTION, async |tx| {
                let session = repo::find_session(tx.executor(), owner, id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                let item = repo::one_item(tx.executor(), id, item_id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                let outcome = item_transition(item.state, Trigger::Failed)
                    .map_err(|_| TransferError::StateInvalid)?;
                if !outcome.moved() {
                    return Ok(());
                }
                let session_outcome = session_transition(session.state, Trigger::Failed)
                    .map_err(|_| TransferError::StateInvalid)?;
                if !repo::fail_item(tx.executor(), id, item_id, &failure, now).await? {
                    return Err(TransferError::StateInvalid);
                }
                match session_outcome {
                    Outcome::Moved(to) => {
                        let moved = repo::move_session(
                            tx.executor(),
                            owner,
                            id,
                            &SessionMove {
                                from: session.state,
                                to,
                                now,
                                finished: false,
                                cancel_requested: false,
                            },
                        )
                        .await?;
                        if !moved {
                            return Err(TransferError::Invariant {
                                what: "session_state",
                            });
                        }
                    }
                    Outcome::Unchanged(_) => repo::touch_session(tx.executor(), id, now).await?,
                }
                repo::record_session_error(tx.executor(), id, &failure).await
            })
            .await
    }

    pub async fn cancel_item(
        &self,
        owner: UserId,
        id: TransferSessionId,
        item_id: SessionItemId,
    ) -> Result<(), TransferError> {
        let now = self.now()?;
        self.pools
            .write_tx(self.clock.as_ref(), CANCEL_ITEM_TRANSACTION, async |tx| {
                let session = repo::find_session(tx.executor(), owner, id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                let item = repo::one_item(tx.executor(), id, item_id)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                let outcome = item_transition(item.state, Trigger::Cancel)
                    .map_err(|_| TransferError::StateInvalid)?;
                if !outcome.moved() {
                    return Ok(());
                }
                repo::cancel_items(tx.executor(), &[item_id], now).await?;
                self.tombstone_placed(tx, session.provider, std::slice::from_ref(&item))
                    .await?;
                if session.state.is_terminal() {
                    repo::touch_session(tx.executor(), id, now).await?;
                    return Ok(());
                }
                if item.state.holds_reservation() {
                    self.quota.release_share(tx, id, item.reserved).await?;
                }
                if repo::remaining_items(tx.executor(), id).await? == 0 {
                    self.close_session(tx, owner, &session, TransferSessionState::Canceled, now)
                        .await?;
                    self.quota.release(tx, id, ReleaseReason::Canceled).await?;
                } else {
                    repo::touch_session(tx.executor(), id, now).await?;
                }
                Ok(())
            })
            .await
    }
}

fn verify_completed(session: &SessionRow, items: &[ItemRow]) -> Result<(), TransferError> {
    let invariant = || TransferError::Invariant {
        what: "completed_items",
    };
    let mut count = 0_u32;
    let mut total = crate::domain::bytes::ByteSize::ZERO;
    for item in items
        .iter()
        .filter(|item| item.state == ItemState::Completed)
    {
        let size = item.result_size.ok_or_else(invariant)?;
        count += 1;
        total = total.checked_add(size).ok_or_else(invariant)?;
    }
    if count == session.completed_file_count && total == session.completed_bytes {
        Ok(())
    } else {
        Err(invariant())
    }
}

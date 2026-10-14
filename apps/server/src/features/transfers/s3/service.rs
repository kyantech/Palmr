use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::domain::bytes::ByteSize;
use crate::domain::client_key::ClientFileKey;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, DbErrorKind, DbPools};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{MultipartHandle, PartPlanEntry, PutHint};

use super::super::admission::{TooLarge, TransferStorage};
use super::super::cancel::TransferCaller;
use super::super::error::{FileTooLarge, TooLargeReason, TransferError};
use super::super::finalize::{destination_of, FinalizeError, FinalizeOutcome, Finalizer};
use super::super::model::{
    wire_bytes, SessionItemId, TransferProvider, TransferSessionId, UploadKind, MAX_PRESIGN_BATCH,
    PRESIGN_TTL_SECONDS,
};
use super::super::repo as transfer_repo;
use super::super::service::TransferService;
use super::super::state::{has_expired, session_transition, ItemState, Outcome, Trigger};
use super::super::tus::repo as tus_repo;
use super::abort::abort_at_provider;
use super::model::{
    part_bytes, CompleteResponse, CompletedPartView, CompletedState, CreateMultipartResponse,
    MultipartId, MultipartState, PartsResponse, PresignRequest, PresignResponse, PresignedPart,
    ReportPartsRequest, S3Ref, MAX_ETAG_CHARS, MAX_REPORTED_PARTS,
};
use super::reconcile::{analyze, Analysis, PartLayout};
use super::repo::{self, NewMultipart};
use crate::features::transfers::finalize::repo::{load_context, ItemContext};

const CREATE_TRANSACTION: &str = "transfers.s3_create";
const PRESIGN_TRANSACTION: &str = "transfers.s3_presign";
const REPORT_TRANSACTION: &str = "transfers.s3_report";
const LIST_TRANSACTION: &str = "transfers.s3_list";

pub struct S3ServiceParts {
    pub pools: DbPools,
    pub clock: Arc<dyn Clock>,
    pub settings: SettingsHandle,
    pub storage: TransferStorage,
    pub transfers: TransferService,
    pub finalizer: Finalizer,
}

#[derive(Clone)]
pub struct S3Service {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    storage: TransferStorage,
    transfers: TransferService,
    finalizer: Finalizer,
}

struct Open {
    s3: S3Ref,
    layout: PartLayout,
    handle: MultipartHandle,
}

fn recorded(code: Option<&str>) -> TransferError {
    code.and_then(|code| ErrorCode::ALL.iter().find(|known| known.as_str() == code))
        .map_or(TransferError::StateInvalid, |known| {
            TransferError::Recorded(*known)
        })
}

fn part_invalid(reason: &'static str) -> TransferError {
    TransferError::MultipartPartInvalid { reason }
}

fn from_finalize(error: FinalizeError) -> TransferError {
    match error {
        FinalizeError::Transfer(error) => error,
        FinalizeError::Storage(error) => TransferError::Storage(error),
        FinalizeError::NotAvailable | FinalizeError::Interrupted | FinalizeError::ShuttingDown => {
            TransferError::StorageUnavailable
        }
    }
}

pub struct Snapshot<'a> {
    pub s3: &'a S3Ref,
    pub layout: &'a PartLayout,
    pub analysis: &'a Analysis,
    pub started: Timestamp,
}

pub async fn persist_snapshot(
    pools: &DbPools,
    clock: &dyn Clock,
    ttl: Duration,
    snapshot: &Snapshot<'_>,
    transaction: &'static str,
) -> Result<bool, TransferError> {
    let Snapshot {
        s3,
        layout,
        analysis,
        started,
    } = *snapshot;
    let now = Timestamp::try_from(clock.now())?;
    let expires_at = Timestamp::try_from(now.get() + ttl)?;
    let present = analysis.present_numbers();
    pools
        .write_tx(clock, transaction, async |tx| {
            let promote = !analysis.held.is_empty();
            if !repo::mark_reconciled(tx.executor(), s3.id, now, expires_at, promote).await? {
                return Ok::<_, TransferError>(false);
            }
            repo::verify_present(tx.executor(), s3.id, &analysis.held, now).await?;
            repo::demote_absent(tx.executor(), s3.id, layout, &present, started).await?;
            Ok(true)
        })
        .await
}

impl S3Service {
    pub fn new(parts: S3ServiceParts) -> Self {
        Self {
            pools: parts.pools,
            clock: parts.clock,
            settings: parts.settings,
            storage: parts.storage,
            transfers: parts.transfers,
            finalizer: parts.finalizer,
        }
    }

    fn now(&self) -> Result<Timestamp, TransferError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }

    fn extended(&self, now: Timestamp) -> Result<Timestamp, TransferError> {
        Ok(Timestamp::try_from(
            now.get() + self.storage.multipart_ttl(),
        )?)
    }

    async fn owned(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
    ) -> Result<Option<ItemContext>, TransferError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        let ctx = load_context(&mut connection, item).await?;
        Ok(ctx.filter(|ctx| ctx.owner == Some(owner) && ctx.session == session))
    }

    fn open_from(&self, ctx: ItemContext) -> Result<Open, TransferError> {
        if ctx.provider != TransferProvider::S3 || ctx.kind != UploadKind::S3Multipart {
            return Err(TransferError::MultipartNotFound);
        }
        let s3 = ctx.s3.clone().ok_or(TransferError::MultipartNotFound)?;
        match s3.state {
            MultipartState::Completed => return Err(TransferError::MultipartAlreadyCompleted),
            MultipartState::Aborted | MultipartState::Abandoned => {
                return Err(TransferError::MultipartNotFound)
            }
            MultipartState::Created | MultipartState::InProgress => {}
            MultipartState::Completing => return Err(TransferError::StateInvalid),
        }
        if ctx.session_state.is_terminal() || ctx.cancel_requested {
            return Err(TransferError::StateInvalid);
        }
        if has_expired(self.now()?, ctx.session_expires_at) {
            return Err(TransferError::Expired);
        }
        if ctx.item_state != ItemState::Uploading {
            return Err(TransferError::StateInvalid);
        }
        if self.storage.bucket() != Some(s3.bucket.as_str()) || s3.object_key != ctx.final_key {
            return Err(TransferError::Invariant {
                what: "multipart_identity",
            });
        }
        let layout = PartLayout::of(&s3).ok_or(TransferError::Invariant { what: "part_plan" })?;
        let key = ObjectKey::parse(&s3.object_key).map_err(|_| TransferError::Invariant {
            what: "multipart_object_key",
        })?;
        let handle = MultipartHandle::new(key, s3.upload_id.clone());
        Ok(Open { s3, layout, handle })
    }

    async fn open(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
    ) -> Result<Open, TransferError> {
        let ctx = self
            .owned(owner, session, item)
            .await?
            .ok_or(TransferError::MultipartNotFound)?;
        self.open_from(ctx)
    }

    pub async fn create(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
    ) -> Result<CreateMultipartResponse, TransferError> {
        let ctx = self
            .owned(owner, session, item)
            .await?
            .ok_or(TransferError::SessionNotFound)?;
        if ctx.provider != TransferProvider::S3 || ctx.kind != UploadKind::S3Multipart {
            return Err(TransferError::StateInvalid);
        }
        if ctx.session_state.is_terminal() || ctx.cancel_requested {
            return Err(TransferError::StateInvalid);
        }
        if ctx.s3.is_some() {
            return Err(TransferError::MultipartAlreadyCreated);
        }
        if !matches!(ctx.item_state, ItemState::Pending | ItemState::Uploading) {
            return Err(TransferError::StateInvalid);
        }
        if has_expired(self.now()?, ctx.session_expires_at) {
            return Err(TransferError::Expired);
        }
        if !self.storage.is_available() {
            return Err(TransferError::StorageUnavailable);
        }
        let declared = ctx
            .declared
            .ok_or_else(|| part_invalid("unknown_length_requires_server_mediated_upload"))?;
        let layout = self.plan(&ctx, declared)?;
        self.preflight(&ctx, owner).await?;

        let multipart = self
            .storage
            .multipart()
            .ok_or(TransferError::StorageUnavailable)?;
        let bucket = self
            .storage
            .bucket()
            .ok_or(TransferError::StorageUnavailable)?
            .to_owned();
        let key = ObjectKey::parse(&ctx.final_key).map_err(|_| TransferError::Invariant {
            what: "final_object_key",
        })?;
        let handle = multipart
            .create_multipart(
                &key,
                PutHint {
                    declared_len: Some(declared.get()),
                    content_type: None,
                },
            )
            .await?;

        let id = MultipartId::generate(self.clock.as_ref());
        let now = self.now()?;
        let expires_at = self.extended(now)?;
        let stored = self
            .pools
            .write_tx(self.clock.as_ref(), CREATE_TRANSACTION, async |tx| {
                self.record_creation(
                    tx,
                    owner,
                    item,
                    &NewMultipart {
                        id,
                        item,
                        owner,
                        upload_id: handle.upload_id(),
                        bucket: &bucket,
                        object_key: key.as_str(),
                        layout,
                        now,
                        expires_at,
                    },
                )
                .await
            })
            .await;
        if let Err(error) = stored {
            self.compensate(&handle).await;
            return Err(match error {
                TransferError::Db(db) if db.kind() == DbErrorKind::UniqueViolation => {
                    TransferError::MultipartAlreadyCreated
                }
                other => other,
            });
        }
        Ok(CreateMultipartResponse {
            multipart_id: id.to_string(),
            part_size_bytes: part_bytes(layout.part_size),
            part_count: layout.part_count,
            max_presign_batch: MAX_PRESIGN_BATCH,
            presign_ttl_seconds: PRESIGN_TTL_SECONDS,
            checksum_headers_required: self.storage.requires_proxied_parts(),
        })
    }

    fn plan(&self, ctx: &ItemContext, declared: ByteSize) -> Result<PartLayout, TransferError> {
        let too_large = |reason: TooLargeReason, capacity: Option<ByteSize>, max| {
            ClientFileKey::parse(&ctx.client_key)
                .map(|client_key| {
                    TransferError::FileTooLarge(FileTooLarge {
                        client_key,
                        declared,
                        max,
                        provider_max: capacity,
                        reason,
                    })
                })
                .map_err(|_| TransferError::Invariant { what: "client_key" })
                .unwrap_or_else(|error| error)
        };
        let policy_max = self.settings.load().quotas.max_file_size_bytes;
        if policy_max.is_some_and(|max| declared > max) {
            return Err(too_large(TooLargeReason::MaxFileSize, None, policy_max));
        }
        match self.storage.plan(Some(declared)) {
            Ok(UploadKind::S3Multipart) => {}
            Ok(_) => return Err(TransferError::StateInvalid),
            Err(TooLarge::Provider { reason, capacity }) => {
                return Err(too_large(
                    reason,
                    ByteSize::try_from(capacity).ok(),
                    policy_max,
                ))
            }
            Err(TooLarge::Profile) => {
                return Err(TransferError::Invariant {
                    what: "provider_profile",
                })
            }
        }
        let (part_size, part_count) = self
            .storage
            .part_layout(declared)
            .ok_or(TransferError::Invariant { what: "part_plan" })?;
        PartLayout::new(part_size, part_count, declared.get())
            .ok_or(TransferError::Invariant { what: "part_plan" })
    }

    async fn preflight(&self, ctx: &ItemContext, owner: UserId) -> Result<(), TransferError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        if transfer_repo::owner_is_active(&mut connection, owner).await? != Some(true) {
            return Err(TransferError::AccountInactive);
        }
        destination_of(&mut connection, ctx, owner).await?;
        Ok(())
    }

    async fn record_creation(
        &self,
        tx: &mut crate::infra::db::WriteTx<'_>,
        owner: UserId,
        item: SessionItemId,
        new: &NewMultipart<'_>,
    ) -> Result<(), TransferError> {
        let fresh = load_context(tx.executor(), item)
            .await?
            .ok_or(TransferError::SessionNotFound)?;
        if fresh.owner != Some(owner) {
            return Err(TransferError::SessionNotFound);
        }
        if fresh.s3.is_some() {
            return Err(TransferError::MultipartAlreadyCreated);
        }
        if fresh.session_state.is_terminal()
            || fresh.cancel_requested
            || !matches!(fresh.item_state, ItemState::Pending | ItemState::Uploading)
        {
            return Err(TransferError::StateInvalid);
        }
        if has_expired(new.now, fresh.session_expires_at) {
            return Err(TransferError::Expired);
        }
        if transfer_repo::owner_is_active(tx.executor(), owner).await? != Some(true) {
            return Err(TransferError::AccountInactive);
        }
        let session_next = session_transition(fresh.session_state, Trigger::ProtocolStarted)
            .map_err(|_| TransferError::StateInvalid)?;
        if repo::held_bytes(tx.executor(), fresh.session).await? < fresh.reserved {
            return Err(TransferError::StateInvalid);
        }
        repo::insert_multipart(tx.executor(), new).await?;
        let started = tus_repo::start_item(tx.executor(), fresh.session, item, new.now).await?;
        if !started && fresh.item_state != ItemState::Uploading {
            return Err(TransferError::StateInvalid);
        }
        match session_next {
            Outcome::Moved(to) => {
                let moved = transfer_repo::move_session(
                    tx.executor(),
                    owner,
                    fresh.session,
                    &transfer_repo::SessionMove {
                        from: fresh.session_state,
                        to,
                        now: new.now,
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
            Outcome::Unchanged(_) => {
                transfer_repo::touch_session(tx.executor(), fresh.session, new.now).await?;
            }
        }
        Ok(())
    }

    async fn compensate(&self, handle: &MultipartHandle) {
        let Some(multipart) = self.storage.multipart() else {
            return;
        };
        if let Err(error) = multipart.abort_multipart(handle).await {
            tracing::warn!(
                storage_error = error.kind(),
                "an unrecorded multipart upload could not be aborted; the orphan sweep reclaims it"
            );
        }
    }

    pub async fn presign(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
        request: PresignRequest,
    ) -> Result<PresignResponse, TransferError> {
        let max = usize::try_from(MAX_PRESIGN_BATCH).unwrap_or(usize::MAX);
        if request.part_numbers.len() > max {
            return Err(TransferError::PresignBatchTooLarge);
        }
        if request.part_numbers.is_empty() {
            return Err(TransferError::Invalid {
                fields: vec!["partNumbers"],
            });
        }
        let open = self.open(owner, session, item).await?;
        if self.storage.requires_proxied_parts() {
            return Err(part_invalid("server_proxied_part_upload"));
        }
        let mut numbers: Vec<u32> = Vec::with_capacity(request.part_numbers.len());
        for value in &request.part_numbers {
            let number = part_number(value, &open.layout)?;
            if numbers.contains(&number) {
                return Err(part_invalid("duplicate_part_number"));
            }
            numbers.push(number);
        }
        let entries: Vec<PartPlanEntry> = numbers
            .iter()
            .map(|number| {
                open.layout
                    .expected(*number)
                    .map(|len| PartPlanEntry {
                        part_number: *number,
                        len,
                    })
                    .ok_or_else(|| part_invalid("part_number"))
            })
            .collect::<Result<_, _>>()?;
        let multipart = self
            .storage
            .multipart()
            .ok_or(TransferError::StorageUnavailable)?;
        let ttl = Duration::from_secs(u64::from(PRESIGN_TTL_SECONDS));
        let signed = multipart
            .sign_part_urls(&open.handle, &entries, ttl)
            .await?;
        if signed.len() != entries.len() {
            return Err(TransferError::Invariant { what: "presign" });
        }

        let now = self.now()?;
        let expires_at = self.extended(now)?;
        let mut signed_until = Timestamp::try_from(now.get() + ttl)?;
        let mut expiries = signed
            .iter()
            .map(|request| Timestamp::try_from(request.expires_at()));
        if let Some(first) = expiries.next() {
            signed_until =
                expiries.try_fold(first?, |earliest, next| next.map(|next| earliest.min(next)))?;
        }
        let alive = self
            .pools
            .write_tx(self.clock.as_ref(), PRESIGN_TRANSACTION, async |tx| {
                if !repo::touch_open(tx.executor(), open.s3.id, now, expires_at, true).await? {
                    return Ok::<_, TransferError>(false);
                }
                repo::mark_signed(tx.executor(), open.s3.id, &numbers, now, signed_until).await?;
                Ok(true)
            })
            .await?;
        if !alive {
            return Err(self.after_race(owner, session, item).await);
        }

        let mut parts = Vec::with_capacity(signed.len());
        for (number, request) in numbers.iter().zip(&signed) {
            parts.push(PresignedPart {
                part_number: *number,
                url: request.url().to_string(),
                expires_at: Timestamp::try_from(request.expires_at())?.to_string(),
            });
        }
        Ok(PresignResponse {
            parts,
            ttl_seconds: PRESIGN_TTL_SECONDS,
            expires_at: signed_until.to_string(),
        })
    }

    async fn after_race(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
    ) -> TransferError {
        match self.open(owner, session, item).await {
            Ok(_) => TransferError::StateInvalid,
            Err(error) => error,
        }
    }

    pub async fn report(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
        request: ReportPartsRequest,
    ) -> Result<(), TransferError> {
        if request.parts.is_empty() || request.parts.len() > MAX_REPORTED_PARTS {
            return Err(TransferError::Invalid {
                fields: vec!["parts"],
            });
        }
        let open = self.open(owner, session, item).await?;
        let mut reports: BTreeMap<u32, &str> = BTreeMap::new();
        for reported in &request.parts {
            let number = u32::try_from(reported.part_number)
                .ok()
                .filter(|number| open.layout.expected(*number).is_some())
                .ok_or_else(|| part_invalid("part_number"))?;
            let etag = reported.etag.as_str();
            let well_formed = !etag.is_empty()
                && etag.chars().count() <= MAX_ETAG_CHARS
                && etag.chars().all(|c| !c.is_control());
            if !well_formed {
                return Err(part_invalid("etag"));
            }
            let size = u64::try_from(reported.size_bytes).ok();
            if size != open.layout.expected(number) {
                return Err(part_invalid("part_size"));
            }
            if reports
                .insert(number, etag)
                .is_some_and(|prior| prior != etag)
            {
                return Err(part_invalid("duplicate_part_number"));
            }
        }
        let reports: Vec<(u32, &str)> = reports.into_iter().collect();
        let now = self.now()?;
        let expires_at = self.extended(now)?;
        let alive = self
            .pools
            .write_tx(self.clock.as_ref(), REPORT_TRANSACTION, async |tx| {
                if !repo::touch_open(tx.executor(), open.s3.id, now, expires_at, true).await? {
                    return Ok::<_, TransferError>(false);
                }
                repo::record_reports(tx.executor(), open.s3.id, &reports, now).await?;
                Ok(true)
            })
            .await?;
        if alive {
            Ok(())
        } else {
            Err(self.after_race(owner, session, item).await)
        }
    }

    pub async fn list(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
    ) -> Result<PartsResponse, TransferError> {
        let open = self.open(owner, session, item).await?;
        let multipart = self
            .storage
            .multipart()
            .ok_or(TransferError::StorageUnavailable)?;
        let started = self.now()?;
        let listed = match multipart.list_parts(&open.handle).await {
            Ok(listed) => listed,
            Err(StorageError::NotFound) => return Err(TransferError::MultipartNotFound),
            Err(error) => return Err(TransferError::Storage(error)),
        };
        let analysis = analyze(&open.layout, &listed);
        let applied = persist_snapshot(
            &self.pools,
            self.clock.as_ref(),
            self.storage.multipart_ttl(),
            &Snapshot {
                s3: &open.s3,
                layout: &open.layout,
                analysis: &analysis,
                started,
            },
            LIST_TRANSACTION,
        )
        .await?;
        if !applied {
            return Err(self.after_race(owner, session, item).await);
        }
        Ok(PartsResponse {
            completed_parts: analysis
                .held
                .iter()
                .map(|part| CompletedPartView {
                    part_number: part.part_number,
                    etag: part.etag.clone(),
                    size_bytes: part_bytes(part.size),
                })
                .collect(),
            missing_part_numbers: analysis.missing.clone(),
            uploaded_bytes: part_bytes(analysis.uploaded_bytes),
            part_size_bytes: part_bytes(open.layout.part_size),
            part_count: open.layout.part_count,
        })
    }

    pub async fn complete(
        &self,
        owner: UserId,
        session: TransferSessionId,
        item: SessionItemId,
        request_id: Option<String>,
    ) -> Result<CompleteResponse, TransferError> {
        let ctx = self
            .owned(owner, session, item)
            .await?
            .ok_or(TransferError::MultipartNotFound)?;
        if ctx.provider != TransferProvider::S3
            || !matches!(ctx.kind, UploadKind::S3Multipart | UploadKind::S3Single)
        {
            return Err(TransferError::MultipartNotFound);
        }
        if ctx.item_state == ItemState::Completed {
            let file_id = ctx.file_id.ok_or(TransferError::Invariant {
                what: "resulting_file_id",
            })?;
            return self.file_response(&file_id).await;
        }
        if ctx.item_state == ItemState::Failed {
            return Err(recorded(ctx.error_code.as_deref()));
        }
        if ctx.kind == UploadKind::S3Multipart {
            let s3 = ctx.s3.as_ref().ok_or(TransferError::MultipartNotFound)?;
            if matches!(
                s3.state,
                MultipartState::Aborted | MultipartState::Abandoned
            ) {
                return Err(TransferError::MultipartNotFound);
            }
        }
        let outcome = self
            .finalizer
            .complete_s3(item, request_id)
            .await
            .map_err(from_finalize)?;
        match outcome {
            FinalizeOutcome::Committed { file_id, .. } => self.file_response(&file_id).await,
            FinalizeOutcome::Failed { code, .. } => Err(recorded(Some(&code))),
            FinalizeOutcome::Busy => Err(TransferError::CompletionBusy),
            FinalizeOutcome::Superseded | FinalizeOutcome::NotReady => {
                Err(TransferError::StateInvalid)
            }
        }
    }

    async fn file_response(&self, file_id: &str) -> Result<CompleteResponse, TransferError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        let file = repo::find_file(&mut connection, file_id)
            .await?
            .ok_or(TransferError::Invariant { what: "file" })?;
        Ok(CompleteResponse {
            file_id: file.id,
            size_bytes: wire_bytes(file.size),
            content_type: file.mime,
            state: CompletedState::Completed,
        })
    }

    pub async fn abort(
        &self,
        caller: &TransferCaller,
        session: TransferSessionId,
        item: SessionItemId,
    ) -> Result<(), TransferError> {
        let ctx = self
            .owned(caller.owner, session, item)
            .await?
            .ok_or(TransferError::MultipartNotFound)?;
        let s3 = ctx.s3.clone().ok_or(TransferError::MultipartNotFound)?;
        if ctx.item_state == ItemState::Completed || s3.state == MultipartState::Completed {
            return Err(TransferError::MultipartAlreadyCompleted);
        }
        match self.transfers.cancel_item(caller, session, item).await {
            Ok(()) => {}
            Err(TransferError::StateInvalid) => {
                let completed = self
                    .owned(caller.owner, session, item)
                    .await?
                    .is_some_and(|fresh| fresh.item_state == ItemState::Completed);
                return Err(if completed {
                    TransferError::MultipartAlreadyCompleted
                } else {
                    TransferError::StateInvalid
                });
            }
            Err(error) => return Err(error),
        }
        abort_at_provider(&self.pools, self.clock.as_ref(), &self.storage, &s3).await
    }
}

fn part_number(value: &Value, layout: &PartLayout) -> Result<u32, TransferError> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| layout.expected(*number).is_some())
        .ok_or_else(|| part_invalid("part_number"))
}

use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::domain::bytes::ByteSize;
use crate::domain::error_code::ErrorCode;
use crate::domain::mime::resolve_mime;
use crate::domain::naming::NameCandidate;
use crate::domain::time::Timestamp;
use crate::features::audit::model::ClientMetadata;
use crate::features::users::model::UserId;
use crate::infra::db::DbError;
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::staging::{StagedObject, StagingStorage, UploadId};

use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferProvider, UploadKind};
use super::super::repo::{self as transfer_repo, FinalizeStage, SessionMove};
use super::super::state::{has_expired, session_transition, ItemState, Outcome as Moved, Trigger};
use super::commit::{
    destination_of, CommitResult, Committed, Disposition, FinalizationProof, Recorded, RejectCause,
    RejectionTarget,
};
use super::hooks::{Flow, Point};
use super::repo::{self, ItemContext};
use super::service::{FinalizeError, FinalizeOutcome, Finalizer};

enum Claim {
    Held(Box<ItemContext>),
    Done(String),
    Superseded,
    NotReady,
    Busy,
}

enum Located {
    Staged(StagedObject),
    Placed(StagedObject),
    Missing,
    Conflict,
}

pub(super) enum Preflight {
    Proceed,
    Reject(RejectCause),
    Superseded,
}

struct LeaseKeeper {
    work: CancellationToken,
    stop: CancellationToken,
    handle: JoinHandle<()>,
}

impl LeaseKeeper {
    fn start(finalizer: &Finalizer, ctx: &ItemContext) -> Option<Self> {
        let upload = ctx.tus.as_ref()?.id;
        let item = ctx.item;
        let work = finalizer.inner.stop.child_token();
        let stop = CancellationToken::new();
        let this = finalizer.clone();
        let (task_work, task_stop) = (work.clone(), stop.clone());
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = task_stop.cancelled() => return,
                    () = tokio::time::sleep(this.inner.limits.heartbeat) => {}
                }
                match this.renew(upload, item).await {
                    Ok(true) => {}
                    Ok(false) => {
                        task_work.cancel();
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(
                            kind = error.kind(),
                            "an upload finalization lease could not be renewed"
                        );
                    }
                }
            }
        });
        Some(Self { work, stop, handle })
    }

    fn token(&self) -> CancellationToken {
        self.work.clone()
    }
}

impl Drop for LeaseKeeper {
    fn drop(&mut self) {
        self.stop.cancel();
        self.handle.abort();
    }
}

fn upload_hex(id: impl std::fmt::Display) -> Result<UploadId, StorageError> {
    UploadId::parse(&id.to_string().replace('-', "")).map_err(|_| StorageError::InvalidKey)
}

fn storage_code(error: &StorageError) -> ErrorCode {
    match error {
        StorageError::QuotaOnDevice => ErrorCode::StorageWriteFailed,
        _ => ErrorCode::StorageUnavailable,
    }
}

impl Finalizer {
    pub(super) async fn run_local(
        &self,
        item: SessionItemId,
        request_id: Option<&str>,
        wait: bool,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let Some(_slot) = self.enter(item) else {
            return self.busy(item, wait).await;
        };
        self.reached(Point::Dispatched).await?;
        match self.claim(item).await? {
            Claim::Done(file_id) => Ok(FinalizeOutcome::Committed {
                file_id,
                created: false,
            }),
            Claim::Superseded => Ok(FinalizeOutcome::Superseded),
            Claim::NotReady => Ok(FinalizeOutcome::NotReady),
            Claim::Busy => self.busy(item, wait).await,
            Claim::Held(ctx) => {
                let upload = ctx.tus.as_ref().map(|tus| tus.id);
                let result = self.finalize_held(*ctx, request_id).await;
                if let (Err(error), Some(upload)) = (&result, upload) {
                    if !matches!(error, FinalizeError::Interrupted) {
                        self.release_lease(upload).await;
                    }
                }
                result
            }
        }
    }

    async fn release_lease(&self, upload: super::super::tus::repo::TusUploadId) {
        let Ok(now) = self.now() else { return };
        let holder = Arc::clone(&self.inner.holder);
        let released = self
            .inner
            .pools
            .write_tx(
                self.inner.clock.as_ref(),
                "transfers.finalize_release",
                async |tx| repo::release_lease(tx.executor(), upload, &holder, now).await,
            )
            .await;
        if let Err(error) = released {
            tracing::warn!(
                kind = error.kind(),
                "an upload finalization lease could not be released; it expires on its own"
            );
        }
    }

    pub(super) async fn busy(
        &self,
        item: SessionItemId,
        wait: bool,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        if !wait {
            return Ok(FinalizeOutcome::Busy);
        }
        let deadline = Instant::now() + self.inner.limits.busy_wait;
        loop {
            let ctx = self.read_context(item).await?;
            match ctx.item_state {
                ItemState::Completed => {
                    let file_id = ctx.file_id.ok_or(TransferError::Invariant {
                        what: "resulting_file_id",
                    })?;
                    return Ok(FinalizeOutcome::Committed {
                        file_id,
                        created: false,
                    });
                }
                ItemState::Failed => {
                    return Ok(FinalizeOutcome::Failed {
                        code: ctx.error_code.unwrap_or_default(),
                        retryable: ctx.tus.is_some_and(|tus| {
                            matches!(
                                tus.state,
                                super::super::tus::repo::TusState::Created
                                    | super::super::tus::repo::TusState::InProgress
                            )
                        }),
                    });
                }
                ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
                    return Ok(FinalizeOutcome::Superseded)
                }
                ItemState::Pending | ItemState::Uploading | ItemState::Finalizing => {}
            }
            if Instant::now() >= deadline {
                return Ok(FinalizeOutcome::Busy);
            }
            tokio::time::sleep(self.inner.limits.busy_poll).await;
        }
    }

    pub(super) async fn read_context(
        &self,
        item: SessionItemId,
    ) -> Result<ItemContext, FinalizeError> {
        let mut connection = self
            .inner
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        Ok(repo::load_context(&mut connection, item)
            .await?
            .ok_or(TransferError::SessionNotFound)?)
    }

    async fn claim(&self, item: SessionItemId) -> Result<Claim, FinalizeError> {
        let now = self.now()?;
        let until = Timestamp::try_from(now.get() + self.inner.limits.lease_ttl)?;
        let holder = Arc::clone(&self.inner.holder);
        let claim = self
            .inner
            .pools
            .write_tx(
                self.inner.clock.as_ref(),
                "transfers.finalize_claim",
                async |tx| {
                    let mut ctx = repo::load_context(tx.executor(), item)
                        .await?
                        .ok_or(TransferError::SessionNotFound)?;
                    match ctx.item_state {
                        ItemState::Completed => {
                            let file_id = ctx.file_id.ok_or(TransferError::Invariant {
                                what: "resulting_file_id",
                            })?;
                            return Ok(Claim::Done(file_id));
                        }
                        ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
                            return Ok(Claim::Superseded)
                        }
                        ItemState::Pending | ItemState::Failed => return Ok(Claim::NotReady),
                        ItemState::Uploading | ItemState::Finalizing => {}
                    }
                    let local = ctx.context == "my_files"
                        && ctx.provider == TransferProvider::Local
                        && ctx.kind == UploadKind::Tus;
                    let Some(tus) = ctx.tus.clone().filter(|_| local) else {
                        return Ok(Claim::NotReady);
                    };
                    if !matches!(
                        tus.state,
                        super::super::tus::repo::TusState::Created
                            | super::super::tus::repo::TusState::InProgress
                    ) {
                        return Ok(Claim::Superseded);
                    }
                    if tus.length != Some(tus.offset) || tus.length.is_none() {
                        return Ok(Claim::NotReady);
                    }
                    if ctx.session_state.is_terminal() || ctx.cancel_requested {
                        return Ok(Claim::Superseded);
                    }
                    if !repo::acquire_lease(tx.executor(), tus.id, &holder, until, now).await? {
                        return Ok(Claim::Busy);
                    }
                    if !repo::enter_finalizing(tx.executor(), item, now).await? {
                        return Err(TransferError::StateInvalid);
                    }
                    let owner = ctx
                        .owner
                        .ok_or(TransferError::Invariant { what: "owner" })?;
                    let starting =
                        repo::other_starting_items(tx.executor(), ctx.session, item).await?;
                    match session_transition(ctx.session_state, Trigger::FinalizeIntent) {
                        Ok(Moved::Moved(to)) if starting == 0 => {
                            let moved = transfer_repo::move_session(
                                tx.executor(),
                                owner,
                                ctx.session,
                                &SessionMove {
                                    from: ctx.session_state,
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
                        _ => transfer_repo::touch_session(tx.executor(), ctx.session, now).await?,
                    }
                    ctx.item_state = ItemState::Finalizing;
                    Ok::<_, TransferError>(Claim::Held(Box::new(ctx)))
                },
            )
            .await?;
        Ok(claim)
    }

    pub(super) async fn reached(&self, point: Point) -> Result<(), FinalizeError> {
        match self.inner.hooks.reached(point).await {
            Flow::Continue => Ok(()),
            Flow::Crash => Err(FinalizeError::Interrupted),
        }
    }

    async fn finalize_held(
        &self,
        ctx: ItemContext,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let staging: Arc<dyn StagingStorage> = self
            .inner
            .storage
            .staging()
            .cloned()
            .ok_or(FinalizeError::NotAvailable)?;
        let tus = ctx
            .tus
            .clone()
            .ok_or(TransferError::Invariant { what: "tus_upload" })?;
        let owner = ctx
            .owner
            .ok_or(TransferError::Invariant { what: "owner" })?;
        let hex = upload_hex(tus.id)?;
        let key = ObjectKey::parse(&ctx.final_key).map_err(|_| TransferError::Invariant {
            what: "final_object_key",
        })?;
        let keeper = LeaseKeeper::start(self, &ctx);
        self.reached(Point::Claimed).await?;

        let located = match self.locate(&staging, &hex, &key, &ctx).await {
            Ok(located) => located,
            Err(FinalizeError::Storage(error)) => {
                return self
                    .fail_retryable(&ctx, owner, storage_code(&error), request_id)
                    .await
            }
            Err(other) => return Err(other),
        };
        self.reached(Point::StagingSynced).await?;
        let object = match &located {
            Located::Staged(object) | Located::Placed(object) => object.clone(),
            Located::Missing => StagedObject {
                size: 0,
                prefix: Vec::new(),
            },
            Located::Conflict => {
                return self
                    .fail_retryable(&ctx, owner, ErrorCode::InternalError, request_id)
                    .await
            }
        };
        self.reached(Point::Measured).await?;

        let size = ByteSize::try_from(object.size).map_err(|_| TransferError::Invariant {
            what: "measured_size",
        })?;
        let measured = Some(object.size);
        if matches!(located, Located::Missing) {
            return self
                .reject(
                    &ctx,
                    RejectCause::LengthMismatch,
                    measured,
                    request_id,
                    &staging,
                    &hex,
                )
                .await;
        }
        if let Some(cause) = self.size_policy(&ctx, size) {
            return self
                .reject(&ctx, cause, measured, request_id, &staging, &hex)
                .await;
        }
        match self.preflight(&ctx, owner).await? {
            Preflight::Proceed => {}
            Preflight::Reject(cause) => {
                return self
                    .reject(&ctx, cause, measured, request_id, &staging, &hex)
                    .await
            }
            Preflight::Superseded => return self.settled_elsewhere(ctx.item).await,
        }
        let extension = NameCandidate::new(ctx.name.as_str())
            .map(|name| name.extension())
            .unwrap_or_default();
        let hint = ctx.filetype_hint();
        let mime = resolve_mime(&object.prefix, Some(extension.as_str()), hint.as_deref());
        self.reached(Point::MimeResolved).await?;

        if !self.record_intent(ctx.item).await? {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::IntentCommitted).await?;
        let mut ctx = ctx;
        ctx.stage = FinalizeStage::Placing;

        if matches!(self.preflight_cancel(&ctx).await?, Preflight::Superseded) {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::BeforePlace).await?;
        if matches!(located, Located::Staged(_)) {
            let token = keeper
                .as_ref()
                .map_or_else(|| self.inner.stop.child_token(), LeaseKeeper::token);
            match staging.place(&hex, &key, &token).await {
                Ok(placed) if placed.size == object.size => {}
                Ok(placed) => {
                    return self
                        .reject(
                            &ctx,
                            RejectCause::LengthMismatch,
                            Some(placed.size),
                            request_id,
                            &staging,
                            &hex,
                        )
                        .await
                }
                Err(StorageError::Canceled) => {
                    return if self.inner.stop.is_cancelled() {
                        Err(FinalizeError::ShuttingDown)
                    } else {
                        self.settled_elsewhere(ctx.item).await
                    }
                }
                Err(StorageError::NotFound) => return self.settled_elsewhere(ctx.item).await,
                Err(StorageError::AlreadyExists) => {
                    let identical = staging.staged_matches(&hex, &key).await;
                    if !matches!(identical, Ok(true)) {
                        return self
                            .fail_retryable(&ctx, owner, ErrorCode::InternalError, request_id)
                            .await;
                    }
                }
                Err(error) => {
                    return self
                        .fail_retryable(&ctx, owner, storage_code(&error), request_id)
                        .await
                }
            }
        }
        self.reached(Point::AfterPlace).await?;

        let proof = FinalizationProof {
            item: ctx.item,
            session: ctx.session,
            owner,
            final_object_id: ctx.final_id,
            final_object_key: key.clone(),
            provider: ctx.provider,
            size,
            mime,
            checksum: None,
            final_etag: None,
        };
        self.reached(Point::BeforeCommit).await?;
        let client = request_id.map_or_else(ClientMetadata::none, ClientMetadata::request_only);
        match self.inner.committer.commit(&proof, &client).await? {
            CommitResult::Committed(committed) => {
                drop(keeper);
                self.reached(Point::AfterCommit).await?;
                self.reached(Point::BeforeCleanup).await?;
                cleanup(&staging, &hex).await;
                Ok(match committed {
                    Committed::Created { file_id, .. } => FinalizeOutcome::Committed {
                        file_id: file_id.to_string(),
                        created: true,
                    },
                    Committed::Existing { file_id } => FinalizeOutcome::Committed {
                        file_id,
                        created: false,
                    },
                })
            }
            CommitResult::Rejected(cause) => {
                self.reject(&ctx, cause, measured, request_id, &staging, &hex)
                    .await
            }
            CommitResult::Superseded => self.settled_elsewhere(ctx.item).await,
        }
    }

    async fn locate(
        &self,
        staging: &Arc<dyn StagingStorage>,
        hex: &UploadId,
        key: &ObjectKey,
        ctx: &ItemContext,
    ) -> Result<Located, FinalizeError> {
        let mut staged = staging.measure(hex).await?;
        if staged.is_none() && ctx.tus.as_ref().and_then(|tus| tus.length) == Some(ByteSize::ZERO) {
            let empty = staging.inspect(key).await?;
            if empty.is_none() {
                drop(staging.ensure(hex).await?);
                staged = staging.measure(hex).await?;
            }
        }
        match staged {
            Some(object) => match staging.inspect(key).await? {
                None => Ok(Located::Staged(object)),
                Some(placed) => {
                    if placed.size == object.size && staging.staged_matches(hex, key).await? {
                        Ok(Located::Placed(placed))
                    } else {
                        Ok(Located::Conflict)
                    }
                }
            },
            None => Ok(match staging.inspect(key).await? {
                Some(placed) => Located::Placed(placed),
                None => Located::Missing,
            }),
        }
    }

    fn size_policy(&self, ctx: &ItemContext, size: ByteSize) -> Option<RejectCause> {
        let length = ctx.tus.as_ref().and_then(|tus| tus.length);
        if length != Some(size) || ctx.declared.is_some_and(|declared| declared != size) {
            return Some(RejectCause::LengthMismatch);
        }
        let max = self
            .inner
            .storage
            .effective_max_file_size(self.inner.settings.load().quotas.max_file_size_bytes);
        max.is_some_and(|max| size > max)
            .then_some(RejectCause::FileTooLarge)
    }

    pub(super) async fn preflight(
        &self,
        ctx: &ItemContext,
        owner: UserId,
    ) -> Result<Preflight, FinalizeError> {
        let fresh = self.read_context(ctx.item).await?;
        if !matches!(fresh.item_state, ItemState::Finalizing) {
            return Ok(Preflight::Superseded);
        }
        if fresh.cancel_requested || fresh.session_state.is_terminal() {
            return Ok(Preflight::Superseded);
        }
        if has_expired(self.now()?, fresh.session_expires_at) {
            return Ok(Preflight::Reject(RejectCause::SessionExpired));
        }
        let mut connection = self
            .inner
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        if transfer_repo::owner_is_active(&mut connection, owner).await? != Some(true) {
            return Ok(Preflight::Reject(RejectCause::AccountInactive));
        }
        match destination_of(&mut connection, &fresh, owner).await {
            Ok(_) => Ok(Preflight::Proceed),
            Err(error) => Ok(Preflight::Reject(match error {
                crate::features::folders::FolderError::NotFound => RejectCause::FolderNotFound,
                crate::features::folders::FolderError::Deleting => RejectCause::FolderDeleting,
                other => return Err(TransferError::from(other).into()),
            })),
        }
    }

    pub(super) async fn preflight_cancel(
        &self,
        ctx: &ItemContext,
    ) -> Result<Preflight, FinalizeError> {
        let fresh = self.read_context(ctx.item).await?;
        if matches!(fresh.item_state, ItemState::Finalizing)
            && !fresh.cancel_requested
            && !fresh.session_state.is_terminal()
        {
            Ok(Preflight::Proceed)
        } else {
            Ok(Preflight::Superseded)
        }
    }

    async fn record_intent(&self, item: SessionItemId) -> Result<bool, FinalizeError> {
        let now = self.now()?;
        let holder = Arc::clone(&self.inner.holder);
        let recorded = self
            .inner
            .pools
            .write_tx(
                self.inner.clock.as_ref(),
                "transfers.finalize_intent",
                async |tx| repo::record_intent(tx.executor(), item, &holder, now).await,
            )
            .await?;
        Ok(recorded)
    }

    pub(super) async fn renew(
        &self,
        upload: super::super::tus::repo::TusUploadId,
        item: SessionItemId,
    ) -> Result<bool, FinalizeError> {
        let now = self.now()?;
        let until = Timestamp::try_from(now.get() + self.inner.limits.lease_ttl)?;
        let holder = Arc::clone(&self.inner.holder);
        let renewed = self
            .inner
            .pools
            .write_tx(
                self.inner.clock.as_ref(),
                "transfers.finalize_lease",
                async |tx| repo::renew_lease(tx.executor(), upload, &holder, until, now).await,
            )
            .await?;
        if !renewed {
            return Ok(false);
        }
        let fresh = self.read_context(item).await?;
        Ok(matches!(fresh.item_state, ItemState::Finalizing) && !fresh.cancel_requested)
    }

    pub(super) async fn settled_elsewhere(
        &self,
        item: SessionItemId,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let ctx = self.read_context(item).await?;
        match ctx.item_state {
            ItemState::Completed => Ok(FinalizeOutcome::Committed {
                file_id: ctx.file_id.ok_or(TransferError::Invariant {
                    what: "resulting_file_id",
                })?,
                created: false,
            }),
            ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
                Ok(FinalizeOutcome::Superseded)
            }
            ItemState::Failed => Ok(FinalizeOutcome::Failed {
                code: ctx.error_code.unwrap_or_default(),
                retryable: false,
            }),
            ItemState::Pending | ItemState::Uploading | ItemState::Finalizing => {
                Err(TransferError::StateInvalid.into())
            }
        }
    }

    pub(super) fn target(
        &self,
        ctx: &ItemContext,
        measured: Option<u64>,
    ) -> Result<RejectionTarget, FinalizeError> {
        Ok(RejectionTarget {
            item: ctx.item,
            final_object_id: ctx.final_id,
            final_object_key: ObjectKey::parse(&ctx.final_key).map_err(|_| {
                TransferError::Invariant {
                    what: "final_object_key",
                }
            })?,
            provider: ctx.provider,
            measured_size: measured,
        })
    }

    async fn reject(
        &self,
        ctx: &ItemContext,
        cause: RejectCause,
        measured: Option<u64>,
        request_id: Option<&str>,
        staging: &Arc<dyn StagingStorage>,
        hex: &UploadId,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let target = self.target(ctx, measured)?;
        let recorded = self
            .inner
            .committer
            .reject(
                &target,
                cause.code(),
                Disposition::Terminal(cause),
                request_id,
            )
            .await?;
        match recorded {
            Recorded::Failed => {
                cleanup(staging, hex).await;
                Ok(FinalizeOutcome::Failed {
                    code: cause.code().as_str().to_owned(),
                    retryable: false,
                })
            }
            Recorded::AlreadyCommitted(file_id) => Ok(FinalizeOutcome::Committed {
                file_id,
                created: false,
            }),
            Recorded::Superseded => Ok(FinalizeOutcome::Superseded),
        }
    }

    async fn fail_retryable(
        &self,
        ctx: &ItemContext,
        _owner: UserId,
        code: ErrorCode,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let target = self.target(ctx, None)?;
        let recorded = self
            .inner
            .committer
            .reject(&target, code, Disposition::Retryable, request_id)
            .await?;
        match recorded {
            Recorded::Failed => Ok(FinalizeOutcome::Failed {
                code: code.as_str().to_owned(),
                retryable: true,
            }),
            Recorded::AlreadyCommitted(file_id) => Ok(FinalizeOutcome::Committed {
                file_id,
                created: false,
            }),
            Recorded::Superseded => Ok(FinalizeOutcome::Superseded),
        }
    }
}

async fn cleanup(staging: &Arc<dyn StagingStorage>, hex: &UploadId) {
    if let Err(error) = staging.remove(hex).await {
        tracing::warn!(
            storage_error = error.kind(),
            "finalized upload staging could not be removed; the cleanup pass retries it"
        );
    }
}

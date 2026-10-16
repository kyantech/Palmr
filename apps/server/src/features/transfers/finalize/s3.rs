use tokio::io::AsyncReadExt as _;

use crate::domain::bytes::ByteSize;
use crate::domain::mime::resolve_mime;
use crate::domain::naming::NameCandidate;
use crate::features::audit::model::ClientMetadata;
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{MultipartHandle, ObjectStat, PutHint, StorageProvider};
use crate::storage::s3::service_code;
use crate::storage::staging::MIME_PREFIX_BYTES;

use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferProvider, UploadKind};
use super::super::repo::{self as transfer_repo, FinalizeStage, SessionMove};
use super::super::s3::abort::abort_at_provider;
use super::super::s3::model::MultipartState;
use super::super::s3::reconcile::{analyze, Analysis, PartLayout};
use super::super::s3::repo as s3_repo;
use super::super::state::{has_expired, session_transition, ItemState, Outcome as Moved, Trigger};
use super::commit::{
    CommitResult, Committed, Disposition, FinalizationProof, Recorded, RejectCause,
};
use super::hooks::Point;
use super::repo::{self, ItemContext};
use super::service::{FinalizeError, FinalizeOutcome, Finalizer};

const CLAIM_TRANSACTION: &str = "transfers.finalize_s3_claim";
const INTENT_TRANSACTION: &str = "transfers.finalize_s3_intent";
const CACHE_TRANSACTION: &str = "transfers.s3_reconcile";
const REVERT_TRANSACTION: &str = "transfers.finalize_s3_revert";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features::transfers) enum S3Origin {
    Complete,
    Redrive,
}

enum Gate {
    Proceed,
    Done(String),
    Superseded,
    NotReady,
}

enum Provider {
    Placed,
    Finished(FinalizeOutcome),
}

fn rejects_part(error: &StorageError) -> bool {
    matches!(
        service_code(error),
        Some("InvalidPart" | "InvalidPartOrder" | "EntityTooSmall")
    )
}

fn part_rejection(error: &StorageError) -> TransferError {
    match service_code(error) {
        Some("EntityTooSmall") => TransferError::MultipartPartInvalid {
            reason: "provider_part_too_small",
        },
        _ => TransferError::MultipartPartRejected,
    }
}

async fn read_prefix(
    provider: &dyn StorageProvider,
    key: &ObjectKey,
    size: u64,
) -> Result<Vec<u8>, StorageError> {
    let wanted = size.min(MIME_PREFIX_BYTES as u64);
    let Ok(wanted) = usize::try_from(wanted) else {
        return Ok(Vec::new());
    };
    if wanted == 0 {
        return Ok(Vec::new());
    }
    let (_, mut body) = provider.open_range(key, 0, wanted as u64).await?;
    let mut prefix = vec![0_u8; wanted];
    let mut filled = 0;
    while filled < prefix.len() {
        let read = body
            .read(&mut prefix[filled..])
            .await
            .map_err(StorageError::from)?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    prefix.truncate(filled);
    Ok(prefix)
}

impl Finalizer {
    pub(super) async fn run_s3(
        &self,
        item: SessionItemId,
        request_id: Option<&str>,
        origin: S3Origin,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let wait = origin == S3Origin::Complete;
        let Some(_slot) = self.enter(item) else {
            return self.busy(item, wait).await;
        };
        self.reached(Point::Dispatched).await?;
        let ctx = self.read_context(item).await?;
        match self.s3_gate(&ctx, origin)? {
            Gate::Done(file_id) => {
                return Ok(FinalizeOutcome::Committed {
                    file_id,
                    created: false,
                })
            }
            Gate::Superseded => return Ok(FinalizeOutcome::Superseded),
            Gate::NotReady => return Ok(FinalizeOutcome::NotReady),
            Gate::Proceed => {}
        }
        match ctx.kind {
            UploadKind::S3Single => self.s3_single(ctx, request_id).await,
            UploadKind::S3Multipart => self.s3_multipart(ctx, request_id).await,
            UploadKind::Tus => Ok(FinalizeOutcome::NotReady),
        }
    }

    fn s3_gate(&self, ctx: &ItemContext, origin: S3Origin) -> Result<Gate, FinalizeError> {
        let s3_item = ctx.context == "my_files"
            && ctx.provider == TransferProvider::S3
            && matches!(ctx.kind, UploadKind::S3Multipart | UploadKind::S3Single)
            && ctx.owner.is_some();
        if !s3_item {
            return Ok(Gate::NotReady);
        }
        let explicit = origin == S3Origin::Complete;
        match ctx.item_state {
            ItemState::Completed => {
                let file_id = ctx.file_id.clone().ok_or(TransferError::Invariant {
                    what: "resulting_file_id",
                })?;
                return Ok(Gate::Done(file_id));
            }
            ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
                return Ok(Gate::Superseded)
            }
            ItemState::Failed => return Ok(Gate::NotReady),
            ItemState::Pending => {
                if !(explicit && ctx.kind == UploadKind::S3Single) {
                    return Ok(Gate::NotReady);
                }
            }
            ItemState::Uploading => {
                if !explicit {
                    return Ok(Gate::NotReady);
                }
            }
            ItemState::Finalizing => {}
        }
        if ctx.session_state.is_terminal() || ctx.cancel_requested {
            return Ok(Gate::Superseded);
        }
        if explicit
            && ctx.item_state != ItemState::Finalizing
            && has_expired(self.now()?, ctx.session_expires_at)
        {
            return Err(TransferError::Expired.into());
        }
        Ok(Gate::Proceed)
    }

    async fn s3_multipart(
        &self,
        ctx: ItemContext,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let storage = &self.inner.storage;
        let s3 = ctx.s3.clone().ok_or(TransferError::MultipartNotFound)?;
        if s3.object_key != ctx.final_key {
            return Err(TransferError::Invariant {
                what: "multipart_object_key",
            }
            .into());
        }
        if storage.bucket() != Some(s3.bucket.as_str()) {
            return Err(TransferError::Invariant {
                what: "multipart_bucket",
            }
            .into());
        }
        let layout = PartLayout::of(&s3).ok_or(TransferError::Invariant { what: "part_plan" })?;
        let key = self.final_key_of(&ctx)?;
        let handle = MultipartHandle::new(key.clone(), s3.upload_id.clone());
        match s3.state {
            MultipartState::Completed => self.settled_elsewhere(ctx.item).await,
            MultipartState::Aborted | MultipartState::Abandoned => {
                Err(TransferError::MultipartNotFound.into())
            }
            MultipartState::Created | MultipartState::InProgress => {
                self.s3_fresh(ctx, &layout, &handle, &key, request_id).await
            }
            MultipartState::Completing => {
                self.s3_resume(ctx, &layout, &handle, &key, request_id)
                    .await
            }
        }
    }

    fn final_key_of(&self, ctx: &ItemContext) -> Result<ObjectKey, FinalizeError> {
        Ok(
            ObjectKey::parse(&ctx.final_key).map_err(|_| TransferError::Invariant {
                what: "final_object_key",
            })?,
        )
    }

    async fn s3_fresh(
        &self,
        ctx: ItemContext,
        layout: &PartLayout,
        handle: &MultipartHandle,
        key: &ObjectKey,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let storage = &self.inner.storage;
        let mp = storage.multipart().ok_or(FinalizeError::NotAvailable)?;
        let owner = ctx
            .owner
            .ok_or(TransferError::Invariant { what: "owner" })?;
        let s3 = ctx.s3.clone().ok_or(TransferError::MultipartNotFound)?;

        let started = self.now()?;
        let listed = match mp.list_parts(handle).await {
            Ok(listed) => listed,
            Err(StorageError::NotFound) => return Err(TransferError::MultipartNotFound.into()),
            Err(error) => return Err(FinalizeError::Storage(error)),
        };
        let analysis = analyze(layout, &listed);
        self.s3_cache(&s3, layout, &analysis, started).await?;
        if !analysis.is_complete() {
            return Err(TransferError::MultipartIncomplete {
                missing: analysis.missing,
            }
            .into());
        }
        if !analysis.invalid.is_empty() {
            return Err(TransferError::MultipartPartInvalid {
                reason: "part_size",
            }
            .into());
        }

        if !self.s3_claim(&ctx).await? {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::Claimed).await?;
        let size = ByteSize::try_from(layout.total).map_err(|_| TransferError::Invariant {
            what: "measured_size",
        })?;
        if let Some(cause) = self.s3_size_policy(&ctx, size) {
            return self
                .s3_reject(&ctx, cause, None, request_id, Some(&s3))
                .await;
        }
        match self.preflight(&ctx, owner).await? {
            super::local::Preflight::Proceed => {}
            super::local::Preflight::Reject(cause) => {
                return self
                    .s3_reject(&ctx, cause, None, request_id, Some(&s3))
                    .await
            }
            super::local::Preflight::Superseded => return self.settled_elsewhere(ctx.item).await,
        }
        if !self.s3_intent(&ctx, false).await? {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::IntentCommitted).await?;
        let mut ctx = ctx;
        ctx.stage = FinalizeStage::Placing;
        if matches!(
            self.preflight_cancel(&ctx).await?,
            super::local::Preflight::Superseded
        ) {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::BeforePlace).await?;
        match self
            .s3_place_multipart(&ctx, handle, key, &analysis, request_id)
            .await?
        {
            Provider::Placed => {}
            Provider::Finished(outcome) => return Ok(outcome),
        }
        self.reached(Point::AfterPlace).await?;
        self.s3_finish(&ctx, key, request_id).await
    }

    async fn s3_resume(
        &self,
        ctx: ItemContext,
        layout: &PartLayout,
        handle: &MultipartHandle,
        key: &ObjectKey,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let storage = &self.inner.storage;
        let provider = storage
            .provider_handle()
            .ok_or(FinalizeError::NotAvailable)?;
        let mp = storage.multipart().ok_or(FinalizeError::NotAvailable)?;
        if ctx.stage != FinalizeStage::Placing || ctx.item_state != ItemState::Finalizing {
            return Err(TransferError::StateInvalid.into());
        }
        if matches!(
            self.preflight_cancel(&ctx).await?,
            super::local::Preflight::Superseded
        ) {
            return self.settled_elsewhere(ctx.item).await;
        }
        match provider.stat(key).await {
            Ok(_) => {}
            Err(StorageError::NotFound) => {
                let listed = match mp.list_parts(handle).await {
                    Ok(listed) => listed,
                    Err(StorageError::NotFound) => {
                        return self
                            .s3_reject(&ctx, RejectCause::MultipartLost, None, request_id, None)
                            .await
                    }
                    Err(error) => return Err(FinalizeError::Storage(error)),
                };
                let analysis = analyze(layout, &listed);
                if !analysis.is_complete() || !analysis.invalid.is_empty() {
                    return self
                        .s3_reject(&ctx, RejectCause::MultipartLost, None, request_id, None)
                        .await;
                }
                self.reached(Point::BeforePlace).await?;
                match self
                    .s3_place_multipart(&ctx, handle, key, &analysis, request_id)
                    .await?
                {
                    Provider::Placed => {}
                    Provider::Finished(outcome) => return Ok(outcome),
                }
            }
            Err(error) => return Err(FinalizeError::Storage(error)),
        }
        self.reached(Point::AfterPlace).await?;
        self.s3_finish(&ctx, key, request_id).await
    }

    async fn s3_place_multipart(
        &self,
        ctx: &ItemContext,
        handle: &MultipartHandle,
        key: &ObjectKey,
        analysis: &Analysis,
        request_id: Option<&str>,
    ) -> Result<Provider, FinalizeError> {
        let storage = &self.inner.storage;
        let mp = storage.multipart().ok_or(FinalizeError::NotAvailable)?;
        let provider = storage
            .provider_handle()
            .ok_or(FinalizeError::NotAvailable)?;
        let error = match mp.complete_multipart(handle, &analysis.manifest()).await {
            Ok(_) => return Ok(Provider::Placed),
            Err(error) => error,
        };
        if matches!(error, StorageError::NotFound) {
            return match provider.stat(key).await {
                Ok(_) => Ok(Provider::Placed),
                Err(StorageError::NotFound) => Ok(Provider::Finished(
                    self.s3_reject(ctx, RejectCause::MultipartLost, None, request_id, None)
                        .await?,
                )),
                Err(other) => Err(FinalizeError::Storage(other)),
            };
        }
        if rejects_part(&error) {
            self.s3_revert(ctx).await?;
            return Err(part_rejection(&error).into());
        }
        Err(FinalizeError::Storage(error))
    }

    async fn s3_single(
        &self,
        ctx: ItemContext,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let storage = &self.inner.storage;
        let provider = storage
            .provider_handle()
            .ok_or(FinalizeError::NotAvailable)?;
        let owner = ctx
            .owner
            .ok_or(TransferError::Invariant { what: "owner" })?;
        if ctx.declared != Some(ByteSize::ZERO) || ctx.s3.is_some() {
            return Err(TransferError::Invariant {
                what: "single_part_item",
            }
            .into());
        }
        let key = self.final_key_of(&ctx)?;
        if !self.s3_claim(&ctx).await? {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::Claimed).await?;
        match self.preflight(&ctx, owner).await? {
            super::local::Preflight::Proceed => {}
            super::local::Preflight::Reject(cause) => {
                return self.s3_reject(&ctx, cause, None, request_id, None).await
            }
            super::local::Preflight::Superseded => return self.settled_elsewhere(ctx.item).await,
        }
        if !self.s3_intent(&ctx, true).await? {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::IntentCommitted).await?;
        let mut ctx = ctx;
        ctx.stage = FinalizeStage::Placing;
        if matches!(
            self.preflight_cancel(&ctx).await?,
            super::local::Preflight::Superseded
        ) {
            return self.settled_elsewhere(ctx.item).await;
        }
        self.reached(Point::BeforePlace).await?;
        match provider.stat(&key).await {
            Ok(_) => {}
            Err(StorageError::NotFound) => {
                provider
                    .put_stream(
                        &key,
                        Box::pin(tokio::io::empty()),
                        PutHint {
                            declared_len: Some(0),
                            content_type: None,
                        },
                    )
                    .await
                    .map_err(FinalizeError::Storage)?;
            }
            Err(error) => return Err(FinalizeError::Storage(error)),
        }
        self.reached(Point::AfterPlace).await?;
        self.s3_finish(&ctx, &key, request_id).await
    }

    async fn s3_finish(
        &self,
        ctx: &ItemContext,
        key: &ObjectKey,
        request_id: Option<&str>,
    ) -> Result<FinalizeOutcome, FinalizeError> {
        let provider = self
            .inner
            .storage
            .provider_handle()
            .ok_or(FinalizeError::NotAvailable)?;
        let owner = ctx
            .owner
            .ok_or(TransferError::Invariant { what: "owner" })?;
        let stat: ObjectStat = match provider.stat(key).await {
            Ok(stat) => stat,
            Err(StorageError::NotFound) => {
                return self
                    .s3_reject(ctx, RejectCause::MultipartLost, None, request_id, None)
                    .await
            }
            Err(error) => return Err(FinalizeError::Storage(error)),
        };
        let size = ByteSize::try_from(stat.size).map_err(|_| TransferError::Invariant {
            what: "measured_size",
        })?;
        self.reached(Point::Measured).await?;
        if let Some(cause) = self.s3_size_policy(ctx, size) {
            return self
                .s3_reject(ctx, cause, Some(stat.size), request_id, None)
                .await;
        }
        match self.preflight(ctx, owner).await? {
            super::local::Preflight::Proceed => {}
            super::local::Preflight::Reject(cause) => {
                return self
                    .s3_reject(ctx, cause, Some(stat.size), request_id, None)
                    .await
            }
            super::local::Preflight::Superseded => return self.settled_elsewhere(ctx.item).await,
        }
        let prefix = read_prefix(provider.as_ref(), key, stat.size)
            .await
            .map_err(FinalizeError::Storage)?;
        let extension = NameCandidate::new(ctx.name.as_str())
            .map(|name| name.extension())
            .unwrap_or_default();
        let mime = resolve_mime(&prefix, Some(extension.as_str()), None);
        self.reached(Point::MimeResolved).await?;

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
            final_etag: stat.etag.map(|etag| etag.as_str().to_owned()),
        };
        self.reached(Point::BeforeCommit).await?;
        let client = request_id.map_or_else(ClientMetadata::none, ClientMetadata::request_only);
        match self.inner.committer.commit(&proof, &client).await? {
            CommitResult::Committed(committed) => {
                self.reached(Point::AfterCommit).await?;
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
                self.s3_reject(ctx, cause, Some(stat.size), request_id, None)
                    .await
            }
            CommitResult::Superseded => self.settled_elsewhere(ctx.item).await,
        }
    }

    fn s3_size_policy(&self, ctx: &ItemContext, size: ByteSize) -> Option<RejectCause> {
        if ctx.declared != Some(size) {
            return Some(RejectCause::LengthMismatch);
        }
        let max = self
            .inner
            .storage
            .effective_max_file_size(self.inner.settings.load().quotas.max_file_size_bytes);
        max.is_some_and(|max| size > max)
            .then_some(RejectCause::FileTooLarge)
    }

    async fn s3_reject(
        &self,
        ctx: &ItemContext,
        cause: RejectCause,
        measured: Option<u64>,
        request_id: Option<&str>,
        open: Option<&super::super::s3::model::S3Ref>,
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
                if let Some(open) = open {
                    if let Err(error) = abort_at_provider(
                        &self.inner.pools,
                        self.inner.clock.as_ref(),
                        &self.inner.storage,
                        open,
                    )
                    .await
                    {
                        tracing::warn!(
                            kind = error.kind(),
                            "a rejected multipart upload could not be aborted at once; the abandoned-upload cleanup retries it"
                        );
                    }
                }
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

    async fn s3_cache(
        &self,
        s3: &super::super::s3::model::S3Ref,
        layout: &PartLayout,
        analysis: &Analysis,
        started: crate::domain::time::Timestamp,
    ) -> Result<(), FinalizeError> {
        super::super::s3::service::persist_snapshot(
            &self.inner.pools,
            self.inner.clock.as_ref(),
            self.inner.storage.multipart_ttl(),
            &super::super::s3::service::Snapshot {
                s3,
                layout,
                analysis,
                started,
            },
            CACHE_TRANSACTION,
        )
        .await?;
        Ok(())
    }

    async fn s3_claim(&self, ctx: &ItemContext) -> Result<bool, FinalizeError> {
        let now = self.now()?;
        let item = ctx.item;
        let claimed = self
            .inner
            .pools
            .write_tx(self.inner.clock.as_ref(), CLAIM_TRANSACTION, async |tx| {
                let fresh = repo::load_context(tx.executor(), item)
                    .await?
                    .ok_or(TransferError::SessionNotFound)?;
                let owner = fresh
                    .owner
                    .ok_or(TransferError::Invariant { what: "owner" })?;
                if matches!(
                    fresh.item_state,
                    ItemState::Completed
                        | ItemState::Canceled
                        | ItemState::Expired
                        | ItemState::Skipped
                        | ItemState::Failed
                ) || fresh.session_state.is_terminal()
                    || fresh.cancel_requested
                {
                    return Ok(false);
                }
                if !s3_repo::claim_item(tx.executor(), item, now).await? {
                    return Ok(false);
                }
                let mut state = fresh.session_state;
                if let Ok(Moved::Moved(to)) = session_transition(state, Trigger::ProtocolStarted) {
                    let moved = transfer_repo::move_session(
                        tx.executor(),
                        owner,
                        fresh.session,
                        &SessionMove {
                            from: state,
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
                    state = to;
                }
                let starting =
                    repo::other_starting_items(tx.executor(), fresh.session, item).await?;
                match session_transition(state, Trigger::FinalizeIntent) {
                    Ok(Moved::Moved(to)) if starting == 0 => {
                        let moved = transfer_repo::move_session(
                            tx.executor(),
                            owner,
                            fresh.session,
                            &SessionMove {
                                from: state,
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
                    _ => {
                        transfer_repo::touch_session(tx.executor(), fresh.session, now).await?;
                    }
                }
                Ok::<_, TransferError>(true)
            })
            .await?;
        Ok(claimed)
    }

    async fn s3_intent(&self, ctx: &ItemContext, zero_byte: bool) -> Result<bool, FinalizeError> {
        let now = self.now()?;
        let item = ctx.item;
        let recorded = self
            .inner
            .pools
            .write_tx(self.inner.clock.as_ref(), INTENT_TRANSACTION, async |tx| {
                s3_repo::record_intent(tx.executor(), item, zero_byte, now).await
            })
            .await?;
        Ok(recorded)
    }

    async fn s3_revert(&self, ctx: &ItemContext) -> Result<(), FinalizeError> {
        let now = self.now()?;
        let item = ctx.item;
        self.inner
            .pools
            .write_tx(self.inner.clock.as_ref(), REVERT_TRANSACTION, async |tx| {
                s3_repo::revert_intent(tx.executor(), item, now).await
            })
            .await?;
        Ok(())
    }
}

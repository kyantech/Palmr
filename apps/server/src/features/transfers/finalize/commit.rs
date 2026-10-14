use std::sync::Arc;

use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::mime::ResolvedMime;
use crate::domain::relative_path::DirectoryPath;
use crate::domain::time::Timestamp;
use crate::features::audit::actions::{self, FileUploadedFacts};
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::files::naming_insert::{Attempt, NameAttempts, NameNamespace};
use crate::features::files::FileId;
use crate::features::folders::{resolve_existing_destination, FolderError, FolderId};
use crate::features::quota::error::QuotaError;
use crate::features::quota::model::{ItemSettlement, SizeCoverage};
use crate::features::quota::service::QuotaService;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::storage::key::ObjectKey;
use crate::storage::lifecycle::{
    parse_provider, tombstone_uncommitted, DeletionReason, PlacedObject, StorageObjectId,
};

use super::super::admission::TransferStorage;
use super::super::error::TransferError;
use super::super::model::{SessionItemId, TransferProvider, TransferSessionId, UploadKind};
use super::super::repo::{self as transfer_repo, ErrorParts, FinalizeStage, SessionMove};
use super::super::state::{has_expired, session_transition, ItemState, Outcome as Moved, Trigger};
use super::repo::{self, ItemContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectCause {
    LengthMismatch,
    FileTooLarge,
    QuotaExceeded,
    SessionExpired,
    AccountInactive,
    FolderNotFound,
    FolderDeleting,
    NameExhausted,
    MultipartLost,
}

impl RejectCause {
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::LengthMismatch => ErrorCode::UploadLengthMismatch,
            Self::FileTooLarge => ErrorCode::FileTooLarge,
            Self::QuotaExceeded => ErrorCode::QuotaExceeded,
            Self::SessionExpired => ErrorCode::TransferSessionExpired,
            Self::AccountInactive => ErrorCode::AuthAccountInactive,
            Self::FolderNotFound => ErrorCode::FolderNotFound,
            Self::FolderDeleting => ErrorCode::FolderDeleting,
            Self::NameExhausted => ErrorCode::FileNameConflict,
            Self::MultipartLost => ErrorCode::MultipartNotFound,
        }
    }

    pub const fn reason(self) -> DeletionReason {
        match self {
            Self::QuotaExceeded => DeletionReason::QuotaRejected,
            Self::SessionExpired => DeletionReason::UploadAbandoned,
            Self::LengthMismatch
            | Self::FileTooLarge
            | Self::AccountInactive
            | Self::FolderNotFound
            | Self::FolderDeleting
            | Self::NameExhausted
            | Self::MultipartLost => DeletionReason::UploadRejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedChecksum {
    pub hex: String,
}

#[derive(Debug, Clone)]
pub struct FinalizationProof {
    pub(super) item: SessionItemId,
    pub(super) session: TransferSessionId,
    pub(super) owner: UserId,
    pub(super) final_object_id: StorageObjectId,
    pub(super) final_object_key: ObjectKey,
    pub(super) provider: TransferProvider,
    pub(super) size: ByteSize,
    pub(super) mime: ResolvedMime,
    pub(super) checksum: Option<VerifiedChecksum>,
    pub(super) final_etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Committed {
    Created { file_id: FileId, size: ByteSize },
    Existing { file_id: String },
}

#[derive(Debug)]
pub enum CommitResult {
    Committed(Committed),
    Rejected(RejectCause),
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Retryable,
    Terminal(RejectCause),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Failed,
    AlreadyCommitted(String),
    Superseded,
}

#[derive(Debug, Clone)]
pub struct RejectionTarget {
    pub item: SessionItemId,
    pub final_object_id: StorageObjectId,
    pub final_object_key: ObjectKey,
    pub provider: TransferProvider,
    pub measured_size: Option<u64>,
}

enum Stop {
    Reject(RejectCause),
    Superseded,
    Error(TransferError),
}

impl From<DbError> for Stop {
    fn from(error: DbError) -> Self {
        Self::Error(error.into())
    }
}

impl From<sqlx::Error> for Stop {
    fn from(error: sqlx::Error) -> Self {
        Self::Error(error.into())
    }
}

impl From<TransferError> for Stop {
    fn from(error: TransferError) -> Self {
        Self::Error(error)
    }
}

impl From<QuotaError> for Stop {
    fn from(error: QuotaError) -> Self {
        match error {
            QuotaError::Exceeded(_) => Self::Reject(RejectCause::QuotaExceeded),
            QuotaError::OwnerInactive => Self::Reject(RejectCause::AccountInactive),
            other => Self::Error(other.into()),
        }
    }
}

impl From<FolderError> for Stop {
    fn from(error: FolderError) -> Self {
        match error {
            FolderError::NotFound => Self::Reject(RejectCause::FolderNotFound),
            FolderError::Deleting => Self::Reject(RejectCause::FolderDeleting),
            other => Self::Error(other.into()),
        }
    }
}

fn invariant(what: &'static str) -> Stop {
    Stop::Error(TransferError::Invariant { what })
}

#[derive(Clone)]
pub struct Committer {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    quota: QuotaService,
    settings: SettingsHandle,
    audit: AuditService,
    storage: TransferStorage,
}

impl Committer {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        audit: AuditService,
        storage: TransferStorage,
    ) -> Self {
        let quota = QuotaService::new(settings.clone(), Arc::clone(&clock));
        Self {
            pools,
            clock,
            quota,
            settings,
            audit,
            storage,
        }
    }

    fn now(&self) -> Result<Timestamp, TransferError> {
        Ok(Timestamp::try_from(self.clock.now())?)
    }

    pub async fn commit(
        &self,
        proof: &FinalizationProof,
        client: &ClientMetadata,
    ) -> Result<CommitResult, TransferError> {
        let now = self.now()?;
        let outcome = self
            .pools
            .write_tx(
                self.clock.as_ref(),
                "transfers.finalize_commit",
                async |tx| self.commit_in_tx(tx, proof, client, now).await,
            )
            .await;
        match outcome {
            Ok(committed) => Ok(CommitResult::Committed(committed)),
            Err(Stop::Reject(cause)) => Ok(CommitResult::Rejected(cause)),
            Err(Stop::Superseded) => Ok(CommitResult::Superseded),
            Err(Stop::Error(error)) => Err(error),
        }
    }

    async fn commit_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        proof: &FinalizationProof,
        client: &ClientMetadata,
        now: Timestamp,
    ) -> Result<Committed, Stop> {
        let ctx = repo::load_context(tx.executor(), proof.item)
            .await?
            .ok_or(Stop::Error(TransferError::SessionNotFound))?;
        let bound = ctx.final_id == proof.final_object_id
            && ctx.final_key == proof.final_object_key.as_str()
            && ctx.owner == Some(proof.owner)
            && ctx.session == proof.session
            && ctx.provider == proof.provider;
        if !bound {
            return Err(invariant("finalization_proof"));
        }
        match ctx.item_state {
            ItemState::Completed => {
                let file_id = ctx.file_id.ok_or_else(|| invariant("resulting_file_id"))?;
                return Ok(Committed::Existing { file_id });
            }
            ItemState::Finalizing => {}
            ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
                return Err(Stop::Superseded)
            }
            ItemState::Pending | ItemState::Uploading | ItemState::Failed => {
                return Err(Stop::Error(TransferError::StateInvalid))
            }
        }
        if ctx.stage != FinalizeStage::Placing {
            return Err(Stop::Error(TransferError::StateInvalid));
        }
        if ctx.session_state.is_terminal() || ctx.cancel_requested {
            return Err(Stop::Superseded);
        }
        if has_expired(now, ctx.session_expires_at) {
            return Err(Stop::Reject(RejectCause::SessionExpired));
        }
        if transfer_repo::owner_is_active(tx.executor(), proof.owner).await? != Some(true) {
            return Err(Stop::Reject(RejectCause::AccountInactive));
        }
        self.check_size(&ctx, proof)?;
        let folder = self.destination(tx, &ctx, proof.owner).await?;

        insert_object(tx, proof, now).await?;
        let (file_id, name) = self.insert_file(tx, &ctx, proof, folder, now).await?;

        let coverage = if ctx.running_quota() {
            SizeCoverage::RunningCheck
        } else {
            SizeCoverage::Reserved
        };
        self.quota
            .settle_item(
                tx,
                ItemSettlement {
                    session: proof.session,
                    share: ctx.reserved,
                    authoritative: proof.size,
                    coverage,
                },
            )
            .await?;

        complete_item(tx, proof.item, file_id, now).await?;
        complete_upload(tx, proof, now).await?;
        self.advance_session(tx, &ctx, proof.size, now).await?;
        self.record_upload(tx, &ctx, proof, file_id, &name, client, now)
            .await?;
        Ok(Committed::Created {
            file_id,
            size: proof.size,
        })
    }

    fn check_size(&self, ctx: &ItemContext, proof: &FinalizationProof) -> Result<(), Stop> {
        let matches_length = match ctx.kind {
            UploadKind::Tus => {
                let tus = ctx.tus.as_ref().ok_or_else(|| invariant("tus_upload"))?;
                tus.length == Some(proof.size)
            }
            UploadKind::S3Multipart | UploadKind::S3Single => ctx.declared.is_some(),
        };
        let matches_declared = ctx.declared.is_none_or(|declared| declared == proof.size);
        if !matches_length || !matches_declared {
            return Err(Stop::Reject(RejectCause::LengthMismatch));
        }
        let max = self
            .storage
            .effective_max_file_size(self.settings.load().quotas.max_file_size_bytes);
        if max.is_some_and(|max| proof.size > max) {
            return Err(Stop::Reject(RejectCause::FileTooLarge));
        }
        Ok(())
    }

    async fn destination(
        &self,
        tx: &mut WriteTx<'_>,
        ctx: &ItemContext,
        owner: UserId,
    ) -> Result<Option<FolderId>, Stop> {
        destination_of(tx.executor(), ctx, owner)
            .await
            .map_err(Stop::from)
    }

    async fn insert_file(
        &self,
        tx: &mut WriteTx<'_>,
        ctx: &ItemContext,
        proof: &FinalizationProof,
        folder: Option<FolderId>,
        now: Timestamp,
    ) -> Result<(FileId, String), Stop> {
        let file_id = FileId::generate(self.clock.as_ref());
        let mut names = NameAttempts::parse(&ctx.name).map_err(|_| invariant("display_name"))?;
        let namespace = if folder.is_some() {
            NameNamespace::FilesInFolder
        } else {
            NameNamespace::FilesAtRoot
        };
        let sql = format!(
            "INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                extension, size_bytes, mime_type, mime_source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11) {}",
            namespace.conflict_clause()
        );
        loop {
            let candidate = names
                .next_candidate()
                .map_err(|_| Stop::Reject(RejectCause::NameExhausted))?;
            let result = sqlx::query(&sql)
                .bind(file_id.to_string())
                .bind(proof.owner.to_string())
                .bind(folder.map(|folder| folder.to_string()))
                .bind(proof.final_object_id.to_string())
                .bind(candidate.display())
                .bind(candidate.normalized())
                .bind(candidate.extension())
                .bind(proof.size.to_i64())
                .bind(proof.mime.essence())
                .bind(proof.mime.source().as_str())
                .bind(now.to_string())
                .execute(tx.executor())
                .await?;
            if let Attempt::Stored(()) = Attempt::from_insert(&result) {
                return Ok((file_id, candidate.display().to_owned()));
            }
        }
    }

    async fn advance_session(
        &self,
        tx: &mut WriteTx<'_>,
        ctx: &ItemContext,
        size: ByteSize,
        now: Timestamp,
    ) -> Result<(), Stop> {
        let files = ctx
            .completed_files
            .checked_add(1)
            .ok_or_else(|| invariant("completed_file_count"))?;
        let bytes = ctx
            .completed_bytes
            .checked_add(size)
            .ok_or_else(|| invariant("completed_bytes"))?;
        sqlx::query(
            "UPDATE transfer_sessions
                SET completed_file_count = ?2, completed_bytes = ?3, updated_at = ?4
              WHERE id = ?1",
        )
        .bind(ctx.session.to_string())
        .bind(i64::from(files))
        .bind(bytes.to_i64())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?;
        let live = transfer_repo::in_flight_items(tx.executor(), ctx.session).await?;
        let failed = transfer_repo::failed_items(tx.executor(), ctx.session).await?;
        if live > 0 || failed > 0 {
            return Ok(());
        }
        let Ok(Moved::Moved(to)) = session_transition(ctx.session_state, Trigger::Close) else {
            return Ok(());
        };
        let owner = ctx.owner.ok_or_else(|| invariant("owner"))?;
        let moved = transfer_repo::move_session(
            tx.executor(),
            owner,
            ctx.session,
            &SessionMove {
                from: ctx.session_state,
                to,
                now,
                finished: true,
                cancel_requested: false,
            },
        )
        .await?;
        if !moved {
            return Err(invariant("session_state"));
        }
        self.quota.commit(tx, ctx.session, bytes).await?;
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the audit event is assembled from the commit's own facts in one place"
    )]
    async fn record_upload(
        &self,
        tx: &mut WriteTx<'_>,
        ctx: &ItemContext,
        proof: &FinalizationProof,
        file_id: FileId,
        name: &str,
        client: &ClientMetadata,
        now: Timestamp,
    ) -> Result<(), Stop> {
        let session = ctx.session.to_string();
        let owner = proof.owner.to_string();
        let label = ctx.owner_label.as_deref().unwrap_or_default();
        let event = AuditEvent::new(
            actions::file_uploaded(&FileUploadedFacts {
                size_bytes: proof.size.get(),
                mime_type: proof.mime.essence(),
                transfer_session_id: &session,
                provider: proof.provider.as_str(),
            }),
            Actor::user(&owner, label),
            Outcome::Success,
            now,
        )
        .with_target(
            Target::new(TargetType::File)
                .id(&file_id.to_string())
                .label(name),
        )
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await.map_err(|error| {
            tracing::error!(kind = %error, "the upload audit event could not be recorded");
            invariant("audit_event")
        })
    }

    pub async fn reject(
        &self,
        target: &RejectionTarget,
        code: ErrorCode,
        disposition: Disposition,
        request_id: Option<&str>,
    ) -> Result<Recorded, TransferError> {
        let now = self.now()?;
        let failure = ErrorParts {
            code: code.as_str().to_owned(),
            request_id: request_id.map(str::to_owned),
        };
        let outcome = self
            .pools
            .write_tx(
                self.clock.as_ref(),
                "transfers.finalize_reject",
                async |tx| {
                    self.reject_in_tx(tx, target, &failure, disposition, now)
                        .await
                },
            )
            .await;
        match outcome {
            Ok(recorded) => Ok(recorded),
            Err(Stop::Error(error)) => Err(error),
            Err(Stop::Reject(_) | Stop::Superseded) => Ok(Recorded::Superseded),
        }
    }

    async fn reject_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        target: &RejectionTarget,
        failure: &ErrorParts,
        disposition: Disposition,
        now: Timestamp,
    ) -> Result<Recorded, Stop> {
        let ctx = repo::load_context(tx.executor(), target.item)
            .await?
            .ok_or(Stop::Error(TransferError::SessionNotFound))?;
        if ctx.final_id != target.final_object_id
            || ctx.final_key != target.final_object_key.as_str()
        {
            return Err(invariant("rejection_target"));
        }
        match ctx.item_state {
            ItemState::Completed => {
                let file_id = ctx.file_id.ok_or_else(|| invariant("resulting_file_id"))?;
                return Ok(Recorded::AlreadyCommitted(file_id));
            }
            ItemState::Failed => return Ok(Recorded::Failed),
            ItemState::Finalizing => {}
            ItemState::Canceled | ItemState::Expired | ItemState::Skipped => {
                return Ok(Recorded::Superseded)
            }
            ItemState::Pending | ItemState::Uploading => {
                return Err(Stop::Error(TransferError::StateInvalid))
            }
        }
        let owner = ctx.owner.ok_or_else(|| invariant("owner"))?;
        let kept_share = match disposition {
            Disposition::Retryable => ctx.reserved,
            Disposition::Terminal(cause) => {
                if ctx.stage == FinalizeStage::Placing {
                    let provider = parse_provider(target.provider.as_str())
                        .ok_or_else(|| invariant("provider"))?;
                    tombstone_uncommitted(
                        tx,
                        self.clock.as_ref(),
                        &PlacedObject {
                            id: target.final_object_id,
                            key: &target.final_object_key,
                            provider,
                            measured_size: target.measured_size,
                        },
                        cause.reason(),
                    )
                    .await
                    .map_err(TransferError::from)?;
                }
                if !ctx.session_state.is_terminal() {
                    self.quota
                        .release_share(tx, ctx.session, ctx.reserved)
                        .await?;
                }
                ByteSize::ZERO
            }
        };
        let failed = sqlx::query(
            "UPDATE transfer_session_files
                SET state = 'failed', error_code = ?2, error_request_id = ?3,
                    reserved_bytes = ?4, updated_at = ?5
              WHERE id = ?1 AND state = 'finalizing'",
        )
        .bind(ctx.item.to_string())
        .bind(&failure.code)
        .bind(failure.request_id.as_deref())
        .bind(kept_share.to_i64())
        .bind(now.to_string())
        .execute(tx.executor())
        .await?
        .rows_affected();
        if failed != 1 {
            return Err(invariant("item_state"));
        }
        match disposition {
            Disposition::Terminal(_) => {
                sqlx::query(
                    "UPDATE tus_uploads
                        SET state = 'terminated', locked_by = NULL, lock_expires_at = NULL,
                            updated_at = ?2
                      WHERE transfer_session_file_id = ?1 AND state IN ('created', 'in_progress')",
                )
                .bind(ctx.item.to_string())
                .bind(now.to_string())
                .execute(tx.executor())
                .await?;
                sqlx::query(
                    "UPDATE s3_multipart_uploads
                        SET state = 'abandoned', updated_at = ?2
                      WHERE transfer_session_file_id = ?1
                        AND state IN ('created', 'in_progress', 'completing')",
                )
                .bind(ctx.item.to_string())
                .bind(now.to_string())
                .execute(tx.executor())
                .await?;
            }
            Disposition::Retryable => {
                sqlx::query(
                    "UPDATE tus_uploads
                        SET locked_by = NULL, lock_expires_at = NULL, updated_at = ?2
                      WHERE transfer_session_file_id = ?1 AND state IN ('created', 'in_progress')",
                )
                .bind(ctx.item.to_string())
                .bind(now.to_string())
                .execute(tx.executor())
                .await?;
            }
        }
        self.fail_session(tx, &ctx, owner, failure, now).await?;
        Ok(Recorded::Failed)
    }

    async fn fail_session(
        &self,
        tx: &mut WriteTx<'_>,
        ctx: &ItemContext,
        owner: UserId,
        failure: &ErrorParts,
        now: Timestamp,
    ) -> Result<(), Stop> {
        match session_transition(ctx.session_state, Trigger::Failed) {
            Ok(Moved::Moved(to)) => {
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
                    return Err(invariant("session_state"));
                }
            }
            Ok(Moved::Unchanged(_)) | Err(_) => {
                transfer_repo::touch_session(tx.executor(), ctx.session, now).await?;
            }
        }
        transfer_repo::record_session_error(tx.executor(), ctx.session, failure).await?;
        Ok(())
    }
}

pub async fn destination_of(
    connection: &mut sqlx::SqliteConnection,
    ctx: &ItemContext,
    owner: UserId,
) -> Result<Option<FolderId>, FolderError> {
    let directory = DirectoryPath::parse_prefix(&ctx.directory).map_err(|_| {
        FolderError::RepositoryInvariant {
            column: "relative_path",
        }
    })?;
    let segments: Vec<&str> = directory
        .segments()
        .iter()
        .map(crate::domain::naming::NameCandidate::normalized)
        .collect();
    resolve_existing_destination(connection, owner, ctx.target_folder, &segments).await
}

async fn insert_object(
    tx: &mut WriteTx<'_>,
    proof: &FinalizationProof,
    now: Timestamp,
) -> Result<(), Stop> {
    let checksum = proof
        .checksum
        .as_ref()
        .map(|checksum| checksum.hex.as_str());
    sqlx::query(
        "INSERT INTO storage_objects (id, object_key, provider, size_bytes, checksum,
            checksum_algo, state, refcount, created_at, updated_at, finalized_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 1, ?7, ?7, ?7)",
    )
    .bind(proof.final_object_id.to_string())
    .bind(proof.final_object_key.as_str())
    .bind(proof.provider.as_str())
    .bind(proof.size.to_i64())
    .bind(checksum)
    .bind(checksum.map(|_| "sha256"))
    .bind(now.to_string())
    .execute(tx.executor())
    .await?;
    Ok(())
}

async fn complete_item(
    tx: &mut WriteTx<'_>,
    item: SessionItemId,
    file_id: FileId,
    now: Timestamp,
) -> Result<(), Stop> {
    let moved = sqlx::query(
        "UPDATE transfer_session_files
            SET state = 'completed', finalize_stage = 'committed', resulting_file_id = ?2,
                completed_at = ?3, updated_at = ?3
          WHERE id = ?1 AND state = 'finalizing' AND finalize_stage = 'placing'",
    )
    .bind(item.to_string())
    .bind(file_id.to_string())
    .bind(now.to_string())
    .execute(tx.executor())
    .await?
    .rows_affected();
    if moved == 1 {
        Ok(())
    } else {
        Err(invariant("item_state"))
    }
}

async fn complete_upload(
    tx: &mut WriteTx<'_>,
    proof: &FinalizationProof,
    now: Timestamp,
) -> Result<(), Stop> {
    sqlx::query(
        "UPDATE tus_uploads
            SET state = 'completed', completed_at = ?2, updated_at = ?2,
                locked_by = NULL, lock_expires_at = NULL
          WHERE transfer_session_file_id = ?1 AND state IN ('created', 'in_progress')",
    )
    .bind(proof.item.to_string())
    .bind(now.to_string())
    .execute(tx.executor())
    .await?;
    let etag = proof
        .final_etag
        .as_deref()
        .filter(|etag| etag.chars().count() <= 128);
    sqlx::query(
        "UPDATE s3_multipart_uploads
            SET state = 'completed', final_etag = ?3, completed_at = ?2, updated_at = ?2
          WHERE transfer_session_file_id = ?1
            AND state IN ('created', 'in_progress', 'completing')",
    )
    .bind(proof.item.to_string())
    .bind(now.to_string())
    .bind(etag)
    .execute(tx.executor())
    .await?;
    Ok(())
}

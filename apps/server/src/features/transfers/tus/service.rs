use std::sync::Arc;

use axum::body::Body;

use crate::config::PublicBaseUrl;
use crate::domain::bytes::ByteSize;
use crate::domain::clock::Clock;
use crate::domain::error_code::ErrorCode;
use crate::domain::time::Timestamp;
use crate::features::folders::resolve_writable_folder;
use crate::features::quota::service::QuotaService;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::UserId;
use crate::infra::db::{DbError, DbPools, WriteTx};
use crate::storage::error::StorageError;
use crate::storage::staging::{StagingStorage, UploadId};

use super::super::admission::TransferStorage;
use super::super::error::TransferError;
use super::super::model::{TransferProvider, UploadKind};
use super::super::repo::{self as transfer_repo, ErrorParts, SessionMove};
use super::super::service::TransferService;
use super::super::state::{
    has_expired, session_transition, ItemState, Outcome, TransferSessionState, Trigger,
};
use super::append::{StreamEnd, StreamPlan, Streamer, TusLimits};
use super::error::TusError;
use super::headers::{upload_location, DeclaredLength};
use super::metadata::UploadMetadata;
use super::repo::{
    self, CreateBinding, ExistingUpload, NewUpload, TusState, TusUploadId, UploadRow,
};

const CREATE_TRANSACTION: &str = "tus.create_upload";

#[derive(Clone)]
pub struct TusService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    storage: TransferStorage,
    transfers: TransferService,
    base_url: PublicBaseUrl,
    streamer: Streamer,
    holder: Arc<str>,
    limits: TusLimits,
}

pub struct TusServiceParts {
    pub pools: DbPools,
    pub clock: Arc<dyn Clock>,
    pub settings: SettingsHandle,
    pub storage: TransferStorage,
    pub transfers: TransferService,
    pub base_url: PublicBaseUrl,
    pub holder: String,
    pub limits: TusLimits,
}

pub struct CreateInput {
    pub metadata: UploadMetadata,
    pub length: DeclaredLength,
    pub body: Option<Body>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub id: TusUploadId,
    pub offset: u64,
    pub expires_at: Timestamp,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub offset: u64,
    pub length: Option<ByteSize>,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub max_size: Option<ByteSize>,
}

#[derive(Debug, Clone, Copy)]
pub struct DecideContext {
    pub now: Timestamp,
    pub max: Option<ByteSize>,
    pub upload_ttl: std::time::Duration,
}

#[derive(Debug)]
pub enum Decision {
    Create {
        length: Option<ByteSize>,
        expires_at: Timestamp,
        session_next: Outcome<TransferSessionState>,
    },
    Existing(ExistingUpload),
}

struct Fresh {
    id: TusUploadId,
    expires_at: Timestamp,
    plan: StreamPlan,
    hint: String,
    streaming: bool,
}

enum Committed {
    New(Fresh),
    Existing(ExistingUpload),
}

fn state_invalid() -> TusError {
    TusError::Transfer(TransferError::StateInvalid)
}

pub fn decide(
    binding: &CreateBinding,
    metadata: &UploadMetadata,
    length: DeclaredLength,
    ctx: &DecideContext,
) -> Result<Decision, TusError> {
    if binding.item.kind != UploadKind::Tus
        || binding.session_state.is_terminal()
        || binding.target_detached
    {
        return Err(state_invalid());
    }
    if has_expired(ctx.now, binding.session_expires_at) {
        return Err(TusError::Transfer(TransferError::Expired));
    }
    check_metadata(binding, metadata)?;
    check_length(binding, length, ctx.max)?;
    match (&binding.existing, binding.item.state) {
        (Some(existing), ItemState::Pending | ItemState::Uploading) => {
            existing_decision(existing, length, ctx.now)
        }
        (None, ItemState::Pending) => {
            let session_next = session_transition(binding.session_state, Trigger::ProtocolStarted)
                .map_err(|_| state_invalid())?;
            let ttl = time::Duration::try_from(ctx.upload_ttl)
                .map_err(|_| TusError::Transfer(TransferError::Invariant { what: "ttl" }))?;
            let own = Timestamp::try_from(ctx.now.get() + ttl).map_err(TransferError::from)?;
            Ok(Decision::Create {
                length: match length {
                    DeclaredLength::Known(size) => Some(size),
                    DeclaredLength::Deferred => None,
                },
                expires_at: own.min(binding.session_expires_at),
                session_next,
            })
        }
        _ => Err(state_invalid()),
    }
}

fn check_metadata(binding: &CreateBinding, metadata: &UploadMetadata) -> Result<(), TusError> {
    use super::metadata::{
        KEY_FILENAME, KEY_FOLDER_ID, KEY_RELATIVE_PATH, KEY_REVERSE_SHARE_SESSION_ID,
    };
    if metadata.filename.display() != binding.item.name {
        return Err(TusError::Header { key: KEY_FILENAME });
    }
    let expected = if binding.item.directory.is_empty() {
        None
    } else {
        Some(format!("{}/{}", binding.item.directory, binding.item.name))
    };
    let provided = metadata.relative_path.as_ref().map(|path| path.joined());
    let consistent = match (&expected, &provided) {
        (Some(expected), Some(provided)) => expected == provided,
        (Some(_), None) => false,
        (None, Some(provided)) => *provided == binding.item.name,
        (None, None) => true,
    };
    if !consistent {
        return Err(TusError::Header {
            key: KEY_RELATIVE_PATH,
        });
    }
    if let Some(folder) = metadata.folder_id {
        if binding.target_folder != Some(folder) {
            return Err(TusError::Header { key: KEY_FOLDER_ID });
        }
    }
    if metadata.reverse_share_session_id.is_some() {
        return Err(TusError::Header {
            key: KEY_REVERSE_SHARE_SESSION_ID,
        });
    }
    Ok(())
}

fn check_length(
    binding: &CreateBinding,
    length: DeclaredLength,
    max: Option<ByteSize>,
) -> Result<(), TusError> {
    let planned = binding.item.declared;
    match length {
        DeclaredLength::Known(requested) => {
            if max.is_some_and(|max| requested > max) {
                return Err(TusError::TooLarge { max });
            }
            if planned.is_some_and(|planned| planned != requested) {
                return Err(TusError::LengthMismatch {
                    requested: Some(requested),
                    planned,
                });
            }
        }
        DeclaredLength::Deferred => {
            if planned.is_some() {
                return Err(TusError::LengthMismatch {
                    requested: None,
                    planned,
                });
            }
        }
    }
    Ok(())
}

fn existing_decision(
    existing: &ExistingUpload,
    length: DeclaredLength,
    now: Timestamp,
) -> Result<Decision, TusError> {
    match existing.state {
        TusState::Terminated | TusState::Expired => return Err(TusError::Gone),
        TusState::Completed => return Err(state_invalid()),
        TusState::Created | TusState::InProgress => {}
    }
    if has_expired(now, existing.expires_at) {
        return Err(TusError::Gone);
    }
    let compatible = match length {
        DeclaredLength::Known(requested) => !existing.defer && existing.length == Some(requested),
        DeclaredLength::Deferred => existing.defer,
    };
    if !compatible {
        return Err(TusError::LengthMismatch {
            requested: match length {
                DeclaredLength::Known(requested) => Some(requested),
                DeclaredLength::Deferred => None,
            },
            planned: existing.length,
        });
    }
    Ok(Decision::Existing(existing.clone()))
}

impl TusService {
    pub fn new(parts: TusServiceParts) -> Self {
        let quota = QuotaService::new(parts.settings.clone(), Arc::clone(&parts.clock));
        let holder: Arc<str> = Arc::from(parts.holder);
        let streamer = Streamer::new(
            parts.pools.clone(),
            Arc::clone(&parts.clock),
            quota,
            Arc::clone(&holder),
            parts.limits,
        );
        Self {
            pools: parts.pools,
            clock: parts.clock,
            settings: parts.settings,
            storage: parts.storage,
            transfers: parts.transfers,
            base_url: parts.base_url,
            streamer,
            holder,
            limits: parts.limits,
        }
    }

    fn now(&self) -> Result<Timestamp, TusError> {
        Ok(Timestamp::try_from(self.clock.now()).map_err(TransferError::from)?)
    }

    fn local_staging(&self) -> Result<Arc<dyn StagingStorage>, TusError> {
        if self.storage.provider() != TransferProvider::Local {
            return Err(TusError::NotAvailable);
        }
        self.storage
            .staging()
            .cloned()
            .ok_or(TusError::NotAvailable)
    }

    fn max_size(&self) -> Option<ByteSize> {
        self.storage
            .effective_max_file_size(self.settings.load().quotas.max_file_size_bytes)
    }

    pub fn capabilities(&self) -> Result<Capabilities, TusError> {
        self.local_staging()?;
        Ok(Capabilities {
            max_size: self.max_size(),
        })
    }

    pub async fn create(&self, owner: UserId, input: CreateInput) -> Result<Created, TusError> {
        let staging = self.local_staging()?;
        if !self.storage.is_available() {
            return Err(TusError::Transfer(TransferError::StorageUnavailable));
        }
        let CreateInput {
            metadata,
            length,
            body,
            request_id,
        } = input;
        let ctx = DecideContext {
            now: self.now()?,
            max: self.max_size(),
            upload_ttl: self.limits.upload_ttl,
        };

        if let Some(existing) = self.precheck(owner, &metadata, length, &ctx).await? {
            return self.resume(&staging, owner, existing).await;
        }
        let streaming = body.is_some();
        let committed = self
            .pools
            .write_tx(self.clock.as_ref(), CREATE_TRANSACTION, async |tx| {
                self.create_in_tx(tx, owner, &metadata, length, streaming, &ctx)
                    .await
            })
            .await?;
        let fresh = match committed {
            Committed::Existing(existing) => return self.resume(&staging, owner, existing).await,
            Committed::New(fresh) => fresh,
        };

        let hex = upload_hex(fresh.id)?;
        let writer = staging.ensure(&hex).await?;
        if let Err(error) = staging.write_hint(&hex, fresh.hint.as_bytes()).await {
            tracing::warn!(
                storage_error = error.kind(),
                "an upload staging hint could not be written"
            );
        }
        self.confirm_live(&staging, owner, fresh.id, &hex).await?;
        let location = upload_location(self.base_url.url(), &fresh.id.to_string());
        let created = |offset: u64| Created {
            id: fresh.id,
            offset,
            expires_at: fresh.expires_at,
            location: location.clone(),
        };
        let Some(body) = body.filter(|_| fresh.streaming) else {
            drop(writer);
            return Ok(created(0));
        };

        let streamer = self.streamer.clone();
        let plan = fresh.plan;
        let outcome = tokio::spawn(async move { streamer.run(plan, writer, body).await })
            .await
            .map_err(|_| TransferError::Invariant {
                what: "upload_stream",
            })?;
        self.conclude(owner, &fresh.plan, outcome.end, request_id)
            .await?;
        Ok(created(outcome.durable))
    }

    async fn precheck(
        &self,
        owner: UserId,
        metadata: &UploadMetadata,
        length: DeclaredLength,
        ctx: &DecideContext,
    ) -> Result<Option<ExistingUpload>, TusError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        let binding = repo::find_binding(
            &mut connection,
            owner,
            metadata.transfer_session_id,
            metadata.item_id,
        )
        .await?
        .ok_or(TransferError::SessionNotFound)?;
        Ok(match decide(&binding, metadata, length, ctx)? {
            Decision::Existing(existing) => Some(existing),
            Decision::Create { .. } => None,
        })
    }

    async fn create_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        owner: UserId,
        metadata: &UploadMetadata,
        length: DeclaredLength,
        streaming: bool,
        ctx: &DecideContext,
    ) -> Result<Committed, TusError> {
        if transfer_repo::owner_is_active(tx.executor(), owner).await? != Some(true) {
            return Err(TusError::Transfer(TransferError::AccountInactive));
        }
        let binding = repo::find_binding(
            tx.executor(),
            owner,
            metadata.transfer_session_id,
            metadata.item_id,
        )
        .await?
        .ok_or(TransferError::SessionNotFound)?;
        let (length, expires_at, session_next) = match decide(&binding, metadata, length, ctx)? {
            Decision::Existing(existing) => return Ok(Committed::Existing(existing)),
            Decision::Create {
                length,
                expires_at,
                session_next,
            } => (length, expires_at, session_next),
        };
        if let Some(folder) = binding.target_folder {
            resolve_writable_folder(tx.executor(), owner, folder).await?;
        }

        let id = TusUploadId::generate(self.clock.as_ref());
        let hex = upload_hex(id)?;
        let staging_path = format!("uploads/{hex}/blob");
        let metadata_json = metadata.stored_json();
        let lease = streaming
            .then(|| {
                time::Duration::try_from(self.limits.lease_ttl)
                    .ok()
                    .and_then(|ttl| Timestamp::try_from(ctx.now.get() + ttl).ok())
                    .map(|until| (&*self.holder, until))
            })
            .flatten();
        repo::insert_upload(
            tx.executor(),
            &NewUpload {
                id,
                item: metadata.item_id,
                owner,
                length,
                staging_path: &staging_path,
                metadata_json: &metadata_json,
                lease,
                now: ctx.now,
                expires_at,
            },
        )
        .await?;
        if !repo::start_item(
            tx.executor(),
            metadata.transfer_session_id,
            metadata.item_id,
            ctx.now,
        )
        .await?
        {
            return Err(state_invalid());
        }
        match session_next {
            Outcome::Moved(to) => {
                let moved = transfer_repo::move_session(
                    tx.executor(),
                    owner,
                    metadata.transfer_session_id,
                    &SessionMove {
                        from: binding.session_state,
                        to,
                        now: ctx.now,
                        finished: false,
                        cancel_requested: false,
                    },
                )
                .await?;
                if !moved {
                    return Err(TusError::Transfer(TransferError::Invariant {
                        what: "session_state",
                    }));
                }
            }
            Outcome::Unchanged(_) => {
                transfer_repo::touch_session(tx.executor(), metadata.transfer_session_id, ctx.now)
                    .await?;
            }
        }

        let running_quota =
            binding.item.declared.is_none() && binding.item.reserved == ByteSize::ZERO;
        Ok(Committed::New(Fresh {
            id,
            expires_at,
            plan: StreamPlan {
                upload: id,
                owner,
                session: metadata.transfer_session_id,
                item: metadata.item_id,
                start: 0,
                declared: length.map(ByteSize::get),
                max: ctx.max.map(ByteSize::get),
                running_quota,
            },
            hint: staging_hint(id, metadata, owner, ctx.now),
            streaming,
        }))
    }

    async fn resume(
        &self,
        staging: &Arc<dyn StagingStorage>,
        owner: UserId,
        existing: ExistingUpload,
    ) -> Result<Created, TusError> {
        let hex = upload_hex(existing.id)?;
        drop(staging.ensure(&hex).await?);
        self.confirm_live(staging, owner, existing.id, &hex).await?;
        let offset = self.reconciled(staging, &hex, existing.offset).await?;
        Ok(Created {
            id: existing.id,
            offset,
            expires_at: existing.expires_at,
            location: upload_location(self.base_url.url(), &existing.id.to_string()),
        })
    }

    async fn confirm_live(
        &self,
        staging: &Arc<dyn StagingStorage>,
        owner: UserId,
        id: TusUploadId,
        hex: &UploadId,
    ) -> Result<(), TusError> {
        match self.live_row(owner, id, self.now()?).await {
            Ok(_) => Ok(()),
            Err(error) => {
                self.discard(staging, hex).await;
                Err(error)
            }
        }
    }

    async fn reconciled(
        &self,
        staging: &Arc<dyn StagingStorage>,
        hex: &UploadId,
        recorded: ByteSize,
    ) -> Result<u64, TusError> {
        let on_disk = staging.staged_len(hex).await?.unwrap_or(0);
        Ok(recorded.get().min(on_disk))
    }

    async fn conclude(
        &self,
        owner: UserId,
        plan: &StreamPlan,
        end: StreamEnd,
        request_id: Option<String>,
    ) -> Result<(), TusError> {
        match end {
            StreamEnd::Finished | StreamEnd::Disconnected => Ok(()),
            StreamEnd::Idle => Err(TusError::IdleTimeout),
            StreamEnd::WriteFailed => Err(TusError::WriteFailed),
            StreamEnd::Gone => Err(TusError::Gone),
            StreamEnd::Internal(error) => Err(error),
            StreamEnd::Overrun => {
                self.record_failure(owner, plan, ErrorCode::FileTooLarge, request_id)
                    .await;
                Err(TusError::Overrun {
                    declared: plan
                        .declared
                        .and_then(|declared| ByteSize::try_from(declared).ok())
                        .unwrap_or(ByteSize::ZERO),
                })
            }
            StreamEnd::TooLarge => {
                self.record_failure(owner, plan, ErrorCode::FileTooLarge, request_id)
                    .await;
                Err(TusError::TooLarge {
                    max: plan.max.and_then(|max| ByteSize::try_from(max).ok()),
                })
            }
            StreamEnd::Quota(error) => {
                self.record_failure(owner, plan, ErrorCode::QuotaExceeded, request_id)
                    .await;
                Err(TusError::from(error))
            }
        }
    }

    async fn record_failure(
        &self,
        owner: UserId,
        plan: &StreamPlan,
        code: ErrorCode,
        request_id: Option<String>,
    ) {
        let failure = ErrorParts {
            code: code.as_str().to_owned(),
            request_id,
        };
        if let Err(error) = self
            .transfers
            .fail_item(owner, plan.session, plan.item, failure)
            .await
        {
            tracing::error!(
                kind = error.kind(),
                "an upload failure could not be recorded on its item"
            );
        }
    }

    pub async fn head(&self, owner: UserId, id: TusUploadId) -> Result<Head, TusError> {
        let staging = self.local_staging()?;
        let now = self.now()?;
        let row = self.live_row(owner, id, now).await?;
        let hex = upload_hex(row.id)?;
        let offset = self.reconciled(&staging, &hex, row.offset).await?;
        Ok(Head {
            offset,
            length: row.length,
            expires_at: row.expires_at,
        })
    }

    async fn read_row(&self, owner: UserId, id: TusUploadId) -> Result<UploadRow, TusError> {
        let mut connection = self
            .pools
            .reader()
            .executor()
            .acquire()
            .await
            .map_err(DbError::from)?;
        repo::find_upload(&mut connection, owner, id)
            .await?
            .ok_or(TusError::NotFound)
    }

    async fn live_row(
        &self,
        owner: UserId,
        id: TusUploadId,
        now: Timestamp,
    ) -> Result<UploadRow, TusError> {
        let row = self.read_row(owner, id).await?;
        match row.state {
            TusState::Terminated | TusState::Expired => Err(TusError::Gone),
            TusState::Completed => Ok(row),
            TusState::Created | TusState::InProgress => {
                if has_expired(now, row.expires_at) {
                    Err(TusError::Gone)
                } else {
                    Ok(row)
                }
            }
        }
    }

    pub async fn terminate(&self, owner: UserId, id: TusUploadId) -> Result<(), TusError> {
        let staging = self.local_staging()?;
        let row = self.read_row(owner, id).await?;
        let hex = upload_hex(row.id)?;
        match row.state {
            TusState::Completed => return Ok(()),
            TusState::Expired => return Err(TusError::Gone),
            TusState::Terminated => {
                self.discard(&staging, &hex).await;
                return Ok(());
            }
            TusState::Created | TusState::InProgress => {}
        }
        match self
            .transfers
            .cancel_item(owner, row.session, row.item)
            .await
        {
            Ok(()) => {}
            Err(TransferError::StateInvalid) => {
                let current = self.read_row(owner, id).await?;
                match current.state {
                    TusState::Terminated | TusState::Completed => {}
                    TusState::Expired => return Err(TusError::Gone),
                    TusState::Created | TusState::InProgress => return Err(state_invalid()),
                }
            }
            Err(error) => return Err(error.into()),
        }
        self.discard(&staging, &hex).await;
        Ok(())
    }

    async fn discard(&self, staging: &Arc<dyn StagingStorage>, hex: &UploadId) {
        if let Err(error) = staging.remove(hex).await {
            tracing::warn!(
                storage_error = error.kind(),
                "terminated upload staging could not be removed; the cleanup pass retries it"
            );
        }
    }
}

fn upload_hex(id: TusUploadId) -> Result<UploadId, TusError> {
    UploadId::parse(&id.to_string().replace('-', ""))
        .map_err(|_| TusError::Storage(StorageError::InvalidKey))
}

fn staging_hint(
    id: TusUploadId,
    metadata: &UploadMetadata,
    owner: UserId,
    now: Timestamp,
) -> String {
    serde_json::json!({
        "version": 1,
        "uploadId": id.to_string(),
        "transferSessionId": metadata.transfer_session_id.to_string(),
        "itemId": metadata.item_id.to_string(),
        "ownerId": owner.to_string(),
        "createdAt": now.to_string(),
    })
    .to_string()
}

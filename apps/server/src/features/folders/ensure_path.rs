use http::StatusCode;

use crate::domain::time::Timestamp;
use crate::features::files::naming_insert::Attempt;
use crate::features::users::model::UserId;
use crate::infra::db::WriteTx;
use crate::infra::http::idempotency::{Claim, ReplayEnvelope};

use super::error::FolderError;
use super::model::{EnsurePath, EnsurePathResponse, FolderId, MAX_FOLDER_DEPTH};
use super::repo::{self, NewRow};
use super::service::{resolve_owned_folder, FolderService};

const TRANSACTION: &str = "folders.ensure_path";

impl FolderService {
    pub async fn ensure_path(
        &self,
        owner: UserId,
        request: &EnsurePath,
        claim: &Claim,
    ) -> Result<EnsurePathResponse, FolderError> {
        let at = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(self.clock.as_ref(), TRANSACTION, async |tx| {
                let response = materialize(self, tx, owner, request, at).await?;
                let body = serde_json::to_value(&response)
                    .map_err(|_| FolderError::RepositoryInvariant { column: "response" })?;
                self.idempotency()
                    .complete(tx, claim, &ReplayEnvelope::new(StatusCode::OK, body))
                    .await?;
                Ok::<_, FolderError>(response)
            })
            .await
    }
}

async fn materialize(
    service: &FolderService,
    tx: &mut WriteTx<'_>,
    owner: UserId,
    request: &EnsurePath,
    at: Timestamp,
) -> Result<EnsurePathResponse, FolderError> {
    let mut parent = request.parent_id;
    let mut depth = match request.parent_id {
        None => 0,
        Some(parent) => {
            i64::from(
                resolve_owned_folder(tx.executor(), owner, parent)
                    .await?
                    .depth,
            ) + 1
        }
    };
    let mut folder_ids = Vec::with_capacity(request.path.len());
    let mut created = Vec::new();
    for candidate in request.path.segments() {
        if depth > MAX_FOLDER_DEPTH {
            return Err(FolderError::DepthExceeded);
        }
        let row = NewRow {
            id: FolderId::generate(service.clock.as_ref()),
            owner,
            parent,
            description: None,
            depth: u8::try_from(depth).map_err(|_| FolderError::DepthExceeded)?,
            at,
        };
        let id = match repo::insert(tx.executor(), &row, candidate).await? {
            Attempt::Stored(()) => {
                created.push(row.id.to_string());
                row.id
            }
            Attempt::NameTaken => {
                let existing = repo::find_child_by_normalized_name(
                    tx.executor(),
                    owner,
                    parent,
                    candidate.normalized(),
                )
                .await?
                .ok_or(FolderError::RepositoryInvariant {
                    column: "name_normalized",
                })?;
                if i64::from(existing.depth) != depth {
                    return Err(FolderError::RepositoryInvariant { column: "depth" });
                }
                existing.id
            }
        };
        folder_ids.push(id.to_string());
        parent = Some(id);
        depth += 1;
    }
    let leaf_folder_id = folder_ids.last().cloned().ok_or(FolderError::Invalid {
        fields: vec!["segments"],
    })?;
    Ok(EnsurePathResponse {
        folder_ids,
        leaf_folder_id,
        created,
    })
}

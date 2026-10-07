use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::db::WriteTx;

use super::error::FolderError;
use super::model::{FolderId, FolderItem, MAX_FOLDER_DEPTH};
use super::repo::{self, Relocation};
use super::service::{resolve_owned_folder, store_unique_name, FolderService, Write};

const TRANSACTION: &str = "folders.move";

impl FolderService {
    pub async fn move_folder(
        &self,
        owner: UserId,
        id: FolderId,
        destination: Option<FolderId>,
    ) -> Result<FolderItem, FolderError> {
        let at = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(self.clock.as_ref(), TRANSACTION, async |tx| {
                move_in_tx(tx, owner, id, destination, at).await
            })
            .await?;
        self.item(owner, id).await
    }
}

async fn move_in_tx(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    id: FolderId,
    destination: Option<FolderId>,
    at: Timestamp,
) -> Result<(), FolderError> {
    let source = repo::find_for_move(tx.executor(), owner, id)
        .await?
        .ok_or(FolderError::NotFound)?;
    let new_root_depth = match destination {
        None => 0,
        Some(parent) => {
            i64::from(
                resolve_owned_folder(tx.executor(), owner, parent)
                    .await?
                    .depth,
            ) + 1
        }
    };
    if destination == source.parent_id {
        return Ok(());
    }

    let profile = repo::profile_subtree(tx.executor(), owner, id, destination).await?;
    if profile.contains_destination {
        return Err(FolderError::Cycle);
    }
    let old_root_depth = i64::from(source.depth);
    let subtree_height = (profile.max_depth - old_root_depth).max(0);
    if new_root_depth + subtree_height > MAX_FOLDER_DEPTH {
        return Err(FolderError::DepthExceeded);
    }

    let relocation = Relocation {
        owner,
        id,
        parent: destination,
        depth: u8::try_from(new_root_depth).map_err(|_| FolderError::DepthExceeded)?,
        at,
    };
    store_unique_name(tx.executor(), &source.name, &Write::Relocate(&relocation)).await?;

    let delta = new_root_depth - old_root_depth;
    if delta != 0 && subtree_height > 0 {
        repo::shift_descendant_depths(tx.executor(), owner, id, delta).await?;
    }
    Ok(())
}

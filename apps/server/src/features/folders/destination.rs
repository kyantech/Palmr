use sqlx::SqliteConnection;

use crate::features::users::model::UserId;

use super::error::FolderError;
use super::model::FolderId;
use super::repo;
use super::service::resolve_writable_folder;

pub async fn resolve_existing_destination(
    connection: &mut SqliteConnection,
    owner: UserId,
    base: Option<FolderId>,
    normalized_segments: &[&str],
) -> Result<Option<FolderId>, FolderError> {
    let mut current = match base {
        Some(base) => Some(
            resolve_writable_folder(&mut *connection, owner, base)
                .await?
                .id,
        ),
        None => None,
    };
    for segment in normalized_segments {
        let child = repo::find_child_by_normalized_name(connection, owner, current, segment)
            .await?
            .ok_or(FolderError::NotFound)?;
        if child.hidden {
            return Err(FolderError::Deleting);
        }
        current = Some(child.id);
    }
    Ok(current)
}

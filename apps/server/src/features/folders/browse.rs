use sqlx::Sqlite;

use crate::features::users::model::UserId;
use crate::infra::http::pagination::{CursorKey, PageRequest};

use super::error::FolderError;
use super::model::{FolderId, FolderRecord, FolderTotals};
use super::repo::{self, Scope};

#[derive(Debug, Clone)]
pub struct ChildFolder {
    pub record: FolderRecord,
    pub totals: FolderTotals,
}

pub async fn list_child_folders<'e, E>(
    executor: E,
    owner: UserId,
    parent: Option<FolderId>,
    page: &PageRequest,
    after: Option<&CursorKey>,
    fetch: i64,
) -> Result<Vec<ChildFolder>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite> + Copy,
{
    let scope = Scope {
        owner,
        parent,
        search: None,
    };
    let records = repo::list_children(executor, &scope, page, after, fetch).await?;
    let limit = usize::try_from(fetch).unwrap_or(usize::MAX);
    let listed: Vec<FolderId> = records.iter().take(limit).map(|record| record.id).collect();
    let mut totals = repo::totals(executor, owner, listed).await?;
    Ok(records
        .into_iter()
        .map(|record| {
            let totals = totals.remove(&record.id).unwrap_or_default();
            ChildFolder { record, totals }
        })
        .collect())
}

pub async fn count_child_folders<'e, E>(
    executor: E,
    owner: UserId,
    parent: Option<FolderId>,
) -> Result<u64, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    repo::count(
        executor,
        &Scope {
            owner,
            parent,
            search: None,
        },
    )
    .await
}

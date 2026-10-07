use std::sync::Arc;

use sqlx::{Sqlite, SqliteConnection};
use utoipa::openapi::path::Parameter;

use crate::domain::clock::Clock;
use crate::domain::naming::NameCandidate;
use crate::domain::normalize::normalize;
use crate::domain::time::Timestamp;
use crate::features::files::naming_insert::{Attempt, NameAttempts};
use crate::features::users::model::UserId;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    cursor_parameter, invalid_param, limit_parameter, search_parameter, CursorKey, Page,
    PageRequest, QueryParams, SearchQuery, SortAllowlist, SortDirection, SortField, SortKeyKind,
    SortValue, TotalCount,
};

use super::error::FolderError;
use super::model::{
    FolderChange, FolderDetail, FolderId, FolderItem, FolderRecord, FolderTotals, FolderTree,
    NewFolder, OwnedFolder, MAX_FOLDER_DEPTH, TREE_DEFAULT_DEPTH, TREE_MAX_DEPTH,
};
use super::repo::{self, NewRow, Scope};

pub const PARENT_PARAM: &str = "parentId";
pub const DEPTH_PARAM: &str = "depth";
pub const ROOT_PARAM: &str = "rootId";

static FOLDER_SORT_FIELDS: [SortField; 3] = [
    SortField::new("name", "name_normalized", SortKeyKind::Text),
    SortField::new("createdAt", "created_at", SortKeyKind::Text),
    SortField::new("updatedAt", "updated_at", SortKeyKind::Text),
];
static FOLDER_SORT: SortAllowlist = SortAllowlist::new(&FOLDER_SORT_FIELDS, 0, SortDirection::Asc);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListQuery {
    parent: Option<FolderId>,
    search: Option<String>,
    page: PageRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeQuery {
    root: Option<FolderId>,
    depth: u8,
}

#[derive(Clone)]
pub struct FolderService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
}

pub async fn resolve_owned_folder<'e, E>(
    executor: E,
    owner: UserId,
    id: FolderId,
) -> Result<OwnedFolder, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    repo::find_owned(executor, owner, id)
        .await?
        .ok_or(FolderError::NotFound)
}

impl FolderService {
    pub fn new(pools: DbPools, clock: Arc<dyn Clock>, keys: Arc<KeyRing>) -> Self {
        Self { pools, clock, keys }
    }

    pub fn list_parameters() -> Vec<Parameter> {
        vec![
            search_parameter(),
            FOLDER_SORT.parameter(),
            cursor_parameter(),
            limit_parameter(),
        ]
    }

    pub fn list_query(&self, raw_query: Option<&str>) -> Result<ListQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        let page = PageRequest::from_query(&params, &FOLDER_SORT, self.keys.as_ref())?;
        let search = SearchQuery::parse(params.single("q")?)?
            .map(|query| normalize(query.as_str()))
            .map(|query| {
                if query.is_empty() {
                    Err(invalid_param("q"))
                } else {
                    Ok(query)
                }
            })
            .transpose()?;
        let parent = params
            .single(PARENT_PARAM)?
            .map(parse_folder_id)
            .transpose()?;
        Ok(ListQuery {
            parent,
            search,
            page,
        })
    }

    pub fn tree_query(raw_query: Option<&str>) -> Result<TreeQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        let depth = match params.single(DEPTH_PARAM)? {
            None => TREE_DEFAULT_DEPTH,
            Some(raw) => raw
                .parse::<u8>()
                .ok()
                .filter(|depth| (1..=TREE_MAX_DEPTH).contains(depth))
                .ok_or_else(|| invalid_param(DEPTH_PARAM))?,
        };
        let root = params
            .single(ROOT_PARAM)?
            .map(parse_folder_id)
            .transpose()?;
        Ok(TreeQuery { root, depth })
    }

    pub async fn list(
        &self,
        owner: UserId,
        query: ListQuery,
    ) -> Result<Page<FolderItem>, FolderError> {
        let reader = self.pools.reader().executor();
        if let Some(parent) = query.parent {
            resolve_owned_folder(reader, owner, parent).await?;
        }
        let scope = Scope {
            owner,
            parent: query.parent,
            search: query.search.as_deref(),
        };
        let records = repo::list(reader, &scope, &query.page).await?;
        let total = repo::count(reader, &scope).await?;
        let limit = usize::from(query.page.limit().get());
        let listed: Vec<FolderId> = records.iter().take(limit).map(|record| record.id).collect();
        let mut totals = repo::totals(reader, owner, listed).await?;
        let page = query.page.into_page(
            records,
            self.keys.as_ref(),
            cursor_key,
            TotalCount::Exact(total),
        );
        Ok(Page {
            items: page
                .items
                .into_iter()
                .map(|record| {
                    let folder_totals = totals.remove(&record.id).unwrap_or_default();
                    FolderItem::new(record, folder_totals)
                })
                .collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn tree(&self, owner: UserId, query: TreeQuery) -> Result<FolderTree, FolderError> {
        let rows = repo::tree(
            self.pools.reader().executor(),
            owner,
            query.root,
            query.depth,
        )
        .await?;
        if query.root.is_some() && rows.is_empty() {
            return Err(FolderError::NotFound);
        }
        Ok(FolderTree::from_rows(rows))
    }

    pub async fn detail(&self, owner: UserId, id: FolderId) -> Result<FolderDetail, FolderError> {
        let reader = self.pools.reader().executor();
        let folder = self.item(owner, id).await?;
        let path = repo::breadcrumbs(reader, owner, id).await?;
        if path.is_empty() {
            return Err(FolderError::NotFound);
        }
        Ok(FolderDetail {
            folder,
            path: path.into_iter().map(Into::into).collect(),
        })
    }

    pub async fn create(&self, owner: UserId, new: NewFolder) -> Result<FolderItem, FolderError> {
        NameCandidate::new(new.name.as_str()).map_err(FolderError::InvalidName)?;
        let creation = Creation {
            id: FolderId::generate(self.clock.as_ref()),
            owner,
            parent: new.parent_id,
            name: new.name,
            description: new.description.clone(),
            at: Timestamp::try_from(self.clock.now())?,
        };
        let (id, at) = (creation.id, creation.at);
        let stored = self
            .pools
            .write_tx(self.clock.as_ref(), "folders.create", async |tx| {
                create_in_tx(tx, creation).await
            })
            .await?;
        Ok(FolderItem::new(
            FolderRecord {
                id,
                parent_id: new.parent_id,
                name: stored.display().to_owned(),
                name_normalized: stored.normalized().to_owned(),
                description: new.description,
                created_at: at,
                updated_at: at,
            },
            FolderTotals::default(),
        ))
    }

    pub async fn update(
        &self,
        owner: UserId,
        id: FolderId,
        change: FolderChange,
    ) -> Result<FolderItem, FolderError> {
        if change.is_empty() {
            return self.item(owner, id).await;
        }
        let requested = change
            .name
            .as_deref()
            .map(NameCandidate::new)
            .transpose()
            .map_err(FolderError::InvalidName)?;
        let edit = Edit {
            owner,
            id,
            requested,
            description: change.description,
            at: Timestamp::try_from(self.clock.now())?,
        };
        self.pools
            .write_tx(self.clock.as_ref(), "folders.update", async |tx| {
                update_in_tx(tx, edit).await
            })
            .await?;
        self.item(owner, id).await
    }

    async fn item(&self, owner: UserId, id: FolderId) -> Result<FolderItem, FolderError> {
        let reader = self.pools.reader().executor();
        let record = repo::get_record(reader, owner, id)
            .await?
            .ok_or(FolderError::NotFound)?;
        let totals = repo::totals(reader, owner, vec![id])
            .await?
            .remove(&id)
            .unwrap_or_default();
        Ok(FolderItem::new(record, totals))
    }
}

struct Creation {
    id: FolderId,
    owner: UserId,
    parent: Option<FolderId>,
    name: String,
    description: Option<String>,
    at: Timestamp,
}

struct Edit {
    owner: UserId,
    id: FolderId,
    requested: Option<NameCandidate>,
    description: Option<Option<String>>,
    at: Timestamp,
}

async fn create_in_tx(
    tx: &mut WriteTx<'_>,
    creation: Creation,
) -> Result<NameCandidate, FolderError> {
    let depth = match creation.parent {
        None => 0,
        Some(parent) => {
            let parent = resolve_owned_folder(tx.executor(), creation.owner, parent).await?;
            if i64::from(parent.depth) >= MAX_FOLDER_DEPTH {
                return Err(FolderError::DepthExceeded);
            }
            parent.depth + 1
        }
    };
    let row = NewRow {
        id: creation.id,
        owner: creation.owner,
        parent: creation.parent,
        description: creation.description,
        depth,
        at: creation.at,
    };
    store_unique_name(tx.executor(), creation.name.as_str(), &Write::Insert(&row)).await
}

async fn update_in_tx(tx: &mut WriteTx<'_>, edit: Edit) -> Result<(), FolderError> {
    let Edit {
        owner,
        id,
        requested,
        description,
        at,
    } = edit;
    let current = repo::get_record(tx.executor(), owner, id)
        .await?
        .ok_or(FolderError::NotFound)?;
    if let Some(description) = description {
        repo::set_description(tx.executor(), owner, id, description.as_deref(), at).await?;
    }
    if let Some(requested) = requested.filter(|requested| requested.display() != current.name) {
        store_unique_name(
            tx.executor(),
            requested.display(),
            &Write::Rename { owner, id, at },
        )
        .await?;
    }
    Ok(())
}

enum Write<'a> {
    Insert(&'a NewRow),
    Rename {
        owner: UserId,
        id: FolderId,
        at: Timestamp,
    },
}

async fn store_unique_name(
    connection: &mut SqliteConnection,
    requested: &str,
    write: &Write<'_>,
) -> Result<NameCandidate, FolderError> {
    let mut names = NameAttempts::parse(requested).map_err(FolderError::InvalidName)?;
    loop {
        let candidate = names.next_candidate().map_err(FolderError::NameConflict)?;
        let attempt = match write {
            Write::Insert(row) => repo::insert(&mut *connection, row, &candidate).await?,
            Write::Rename { owner, id, at } => {
                repo::rename(&mut *connection, *owner, *id, &candidate, *at).await?
            }
        };
        if let Attempt::Stored(()) = attempt {
            return Ok(candidate);
        }
    }
}

fn parse_folder_id(raw: &str) -> Result<FolderId, ApiError> {
    raw.parse::<FolderId>()
        .map_err(|_| FolderError::NotFound.api_error())
}

fn cursor_key(record: &FolderRecord, field: &'static SortField) -> CursorKey {
    let value = match field.name() {
        "createdAt" => record.created_at.to_string(),
        "updatedAt" => record.updated_at.to_string(),
        _ => record.name_normalized.clone(),
    };
    CursorKey::new(SortValue::Text(value), record.id)
}

use std::sync::Arc;

use sqlx::SqliteConnection;
use utoipa::openapi::path::{Parameter, ParameterBuilder, ParameterIn};
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::Required;

use crate::domain::clock::Clock;
use crate::domain::naming::NameCandidate;
use crate::domain::time::Timestamp;
use crate::features::folders::{
    count_child_folders, list_child_folders, move_folder_in_tx, resolve_owned_folder, ChildFolder,
    FolderError, FolderId, FolderItem,
};
use crate::features::users::model::UserId;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    cursor_parameter, invalid_param, limit_parameter, CursorKey, Page, PageRequest, QueryParams,
    SortAllowlist, SortDirection, SortField, SortKeyKind, SortValue, TotalCount, SEARCH_PARAM,
};

use super::error::FileError;
use super::model::{
    BatchMove, BatchMoveResult, BrowseItem, FileChange, FileId, FileItem, FileRecord, FileResult,
    MovedItem, NameCheck,
};
use super::naming_insert::{Attempt, NameAttempts};
use super::repo::{self, Relocation};

pub const FOLDER_PARAM: &str = "folderId";
pub const NAME_PARAM: &str = "name";

const FOLDER_GROUP: u8 = 0;
const FILE_GROUP: u8 = 1;
const GROUPS: u8 = 2;

static BROWSE_SORT_FIELDS: [SortField; 4] = [
    SortField::new("name", "name_normalized", SortKeyKind::Text),
    SortField::new("size", "size_bytes", SortKeyKind::Integer),
    SortField::new("createdAt", "created_at", SortKeyKind::Text),
    SortField::new("updatedAt", "updated_at", SortKeyKind::Text),
];
static BROWSE_SORT: SortAllowlist = SortAllowlist::new(&BROWSE_SORT_FIELDS, 0, SortDirection::Asc);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseQuery {
    folder: Option<FolderId>,
    page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCheckQuery {
    folder: Option<FolderId>,
    name: String,
}

#[derive(Clone)]
pub struct FileService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
}

enum Node {
    Folder(Box<ChildFolder>),
    File(Box<FileRecord>),
}

struct Placed {
    requested: String,
    stored: String,
}

impl Placed {
    fn renamed_to(self) -> Option<String> {
        (self.stored != self.requested).then_some(self.stored)
    }
}

impl FileService {
    pub fn new(pools: DbPools, clock: Arc<dyn Clock>, keys: Arc<KeyRing>) -> Self {
        Self { pools, clock, keys }
    }

    pub fn browse_parameters() -> Vec<Parameter> {
        vec![
            query_parameter(
                FOLDER_PARAM,
                "List the direct children of this folder. Absent lists the My Files root. An unknown or foreign folder id is `FOLDER_NOT_FOUND`.",
            ),
            BROWSE_SORT.parameter(),
            cursor_parameter(),
            limit_parameter(),
        ]
    }

    pub fn name_check_parameters() -> Vec<Parameter> {
        vec![
            query_parameter(
                FOLDER_PARAM,
                "The folder whose file names are checked. Absent checks the My Files root. An unknown or foreign folder id is `FOLDER_NOT_FOUND`.",
            ),
            ParameterBuilder::new()
                .name(NAME_PARAM)
                .parameter_in(ParameterIn::Query)
                .required(Required::True)
                .description(Some("The file name to check: 1 to 255 bytes, no `/`, `\\`, control characters, `.` or `..`."))
                .schema(Some(
                    ObjectBuilder::new()
                        .schema_type(Type::String)
                        .min_length(Some(1))
                        .max_length(Some(255)),
                ))
                .build(),
        ]
    }

    pub fn browse_query(&self, raw_query: Option<&str>) -> Result<BrowseQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        if params.all(SEARCH_PARAM).next().is_some() {
            return Err(invalid_param(SEARCH_PARAM));
        }
        let page =
            PageRequest::from_grouped_query(&params, &BROWSE_SORT, self.keys.as_ref(), GROUPS)?;
        let folder = params
            .single(FOLDER_PARAM)?
            .map(parse_folder_id)
            .transpose()?;
        Ok(BrowseQuery { folder, page })
    }

    pub fn name_check_query(raw_query: Option<&str>) -> Result<NameCheckQuery, ApiError> {
        let params = QueryParams::parse(raw_query);
        let name = params
            .single(NAME_PARAM)?
            .ok_or_else(|| invalid_param(NAME_PARAM))?
            .to_owned();
        let folder = params
            .single(FOLDER_PARAM)?
            .map(parse_folder_id)
            .transpose()?;
        Ok(NameCheckQuery { folder, name })
    }

    pub async fn browse(
        &self,
        owner: UserId,
        query: BrowseQuery,
    ) -> Result<Page<BrowseItem>, FileError> {
        let reader = self.pools.reader().executor();
        if let Some(folder) = query.folder {
            resolve_owned_folder(reader, owner, folder).await?;
        }
        let BrowseQuery { folder, page } = query;
        let after = page.after().cloned();
        let fetch = page.fetch_size();
        let mut nodes: Vec<Node> = Vec::new();
        let folders_remain = after
            .as_ref()
            .is_none_or(|after| after.group() == Some(FOLDER_GROUP));
        if folders_remain {
            let children =
                list_child_folders(reader, owner, folder, &page, after.as_ref(), fetch).await?;
            nodes.extend(
                children
                    .into_iter()
                    .map(|child| Node::Folder(Box::new(child))),
            );
        }
        let wanted = fetch - i64::try_from(nodes.len()).unwrap_or(fetch);
        if wanted > 0 {
            let file_after = after
                .as_ref()
                .filter(|after| after.group() == Some(FILE_GROUP));
            let files =
                repo::list_children(reader, owner, folder, &page, file_after, wanted).await?;
            nodes.extend(files.into_iter().map(|file| Node::File(Box::new(file))));
        }
        let total = count_child_folders(reader, owner, folder).await?
            + repo::count_children(reader, owner, folder).await?;
        let page = page.into_page(
            nodes,
            self.keys.as_ref(),
            node_key,
            TotalCount::Exact(total),
        );
        Ok(Page {
            items: page
                .items
                .into_iter()
                .map(|node| match node {
                    Node::Folder(child) => {
                        BrowseItem::Folder(FolderItem::new(child.record, child.totals))
                    }
                    Node::File(file) => BrowseItem::File(FileItem::from(*file)),
                })
                .collect(),
            next_cursor: page.next_cursor,
            total_count: page.total_count,
        })
    }

    pub async fn get(&self, owner: UserId, id: FileId) -> Result<FileItem, FileError> {
        repo::get_record(self.pools.reader().executor(), owner, id)
            .await?
            .map(FileItem::from)
            .ok_or(FileError::NotFound)
    }

    pub async fn update(
        &self,
        owner: UserId,
        id: FileId,
        change: FileChange,
    ) -> Result<FileResult, FileError> {
        if change.is_empty() {
            return Ok(FileResult {
                file: self.get(owner, id).await?,
                renamed_to: None,
            });
        }
        let requested = change
            .name
            .as_deref()
            .map(NameCandidate::new)
            .transpose()
            .map_err(FileError::InvalidName)?;
        let edit = Edit {
            owner,
            id,
            requested,
            description: change.description,
            at: Timestamp::try_from(self.clock.now())?,
        };
        let placed = self
            .pools
            .write_tx(self.clock.as_ref(), "files.update", async |tx| {
                update_in_tx(tx, edit).await
            })
            .await?;
        Ok(FileResult {
            file: self.get(owner, id).await?,
            renamed_to: placed.and_then(Placed::renamed_to),
        })
    }

    pub async fn move_file(
        &self,
        owner: UserId,
        id: FileId,
        destination: Option<FolderId>,
    ) -> Result<FileResult, FileError> {
        let at = Timestamp::try_from(self.clock.now())?;
        let placed = self
            .pools
            .write_tx(self.clock.as_ref(), "files.move", async |tx| {
                move_one_in_tx(tx, owner, id, destination, at).await
            })
            .await?;
        Ok(FileResult {
            file: self.get(owner, id).await?,
            renamed_to: placed.renamed_to(),
        })
    }

    pub async fn batch_move(
        &self,
        owner: UserId,
        batch: &BatchMove,
    ) -> Result<BatchMoveResult, FileError> {
        let at = Timestamp::try_from(self.clock.now())?;
        self.pools
            .write_tx(self.clock.as_ref(), "files.batch_move", async |tx| {
                batch_move_in_tx(tx, owner, batch, at).await
            })
            .await
    }

    pub async fn name_check(
        &self,
        owner: UserId,
        query: NameCheckQuery,
    ) -> Result<NameCheck, FileError> {
        let reader = self.pools.reader().executor();
        let mut names = NameAttempts::parse(&query.name).map_err(FileError::InvalidName)?;
        if let Some(folder) = query.folder {
            resolve_owned_folder(reader, owner, folder).await?;
        }
        let mut first = true;
        loop {
            let candidate = names.next_candidate().map_err(FileError::NameConflict)?;
            if !repo::name_taken(reader, owner, query.folder, candidate.normalized()).await? {
                return Ok(NameCheck {
                    available: first,
                    suggested_name: (!first).then(|| candidate.into_display()),
                });
            }
            first = false;
        }
    }
}

struct Edit {
    owner: UserId,
    id: FileId,
    requested: Option<NameCandidate>,
    description: Option<Option<String>>,
    at: Timestamp,
}

async fn update_in_tx(tx: &mut WriteTx<'_>, edit: Edit) -> Result<Option<Placed>, FileError> {
    let Edit {
        owner,
        id,
        requested,
        description,
        at,
    } = edit;
    let current = repo::find_source(tx.executor(), owner, id)
        .await?
        .ok_or(FileError::NotFound)?;
    if let Some(description) = description.filter(|new| *new != current.description) {
        repo::set_description(tx.executor(), owner, id, description.as_deref(), at).await?;
    }
    let Some(requested) = requested.filter(|requested| requested.display() != current.name) else {
        return Ok(None);
    };
    let requested_name = requested.display().to_owned();
    let stored = store_unique_name(
        tx.executor(),
        &requested_name,
        &Write::Rename { owner, id, at },
    )
    .await?;
    Ok(Some(Placed {
        requested: requested_name,
        stored: stored.into_display(),
    }))
}

async fn move_one_in_tx(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    id: FileId,
    destination: Option<FolderId>,
    at: Timestamp,
) -> Result<Placed, FileError> {
    let source = repo::find_source(tx.executor(), owner, id)
        .await?
        .ok_or(FileError::NotFound)?;
    if let Some(folder) = destination {
        resolve_owned_folder(tx.executor(), owner, folder).await?;
    }
    place(
        tx,
        owner,
        id,
        source.folder_id,
        source.name,
        destination,
        at,
    )
    .await
}

async fn batch_move_in_tx(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    batch: &BatchMove,
    at: Timestamp,
) -> Result<BatchMoveResult, FileError> {
    if let Some(folder) = batch.target {
        resolve_owned_folder(tx.executor(), owner, folder).await?;
    }
    let mut folders = Vec::with_capacity(batch.folders.len());
    for id in &batch.folders {
        let outcome = move_folder_in_tx(tx, owner, *id, batch.target, at).await?;
        folders.push(MovedItem {
            id: id.to_string(),
            renamed_to: (outcome.stored != outcome.requested).then(|| outcome.stored.clone()),
            name: outcome.stored,
        });
    }
    let mut files = Vec::with_capacity(batch.files.len());
    for id in &batch.files {
        let source = repo::find_source(tx.executor(), owner, *id)
            .await?
            .ok_or(FileError::NotFound)?;
        let placed = place(
            tx,
            owner,
            *id,
            source.folder_id,
            source.name,
            batch.target,
            at,
        )
        .await?;
        files.push(MovedItem {
            id: id.to_string(),
            name: placed.stored.clone(),
            renamed_to: placed.renamed_to(),
        });
    }
    Ok(BatchMoveResult { files, folders })
}

async fn place(
    tx: &mut WriteTx<'_>,
    owner: UserId,
    id: FileId,
    current_folder: Option<FolderId>,
    current_name: String,
    destination: Option<FolderId>,
    at: Timestamp,
) -> Result<Placed, FileError> {
    if destination == current_folder {
        return Ok(Placed {
            stored: current_name.clone(),
            requested: current_name,
        });
    }
    let relocation = Relocation {
        owner,
        id,
        folder: destination,
        at,
    };
    let stored =
        store_unique_name(tx.executor(), &current_name, &Write::Relocate(&relocation)).await?;
    Ok(Placed {
        requested: current_name,
        stored: stored.into_display(),
    })
}

enum Write<'a> {
    Rename {
        owner: UserId,
        id: FileId,
        at: Timestamp,
    },
    Relocate(&'a Relocation),
}

async fn store_unique_name(
    connection: &mut SqliteConnection,
    requested: &str,
    write: &Write<'_>,
) -> Result<NameCandidate, FileError> {
    let mut names = NameAttempts::parse(requested).map_err(FileError::InvalidName)?;
    loop {
        let candidate = names.next_candidate().map_err(FileError::NameConflict)?;
        let attempt = match write {
            Write::Rename { owner, id, at } => {
                repo::rename(&mut *connection, *owner, *id, &candidate, *at).await?
            }
            Write::Relocate(relocation) => {
                repo::relocate(&mut *connection, relocation, &candidate).await?
            }
        };
        if let Attempt::Stored(()) = attempt {
            return Ok(candidate);
        }
    }
}

fn parse_folder_id(raw: &str) -> Result<FolderId, ApiError> {
    raw.parse::<FolderId>()
        .map_err(|_| FileError::Folder(FolderError::NotFound).api_error())
}

fn query_parameter(name: &'static str, description: &'static str) -> Parameter {
    ParameterBuilder::new()
        .name(name)
        .parameter_in(ParameterIn::Query)
        .required(Required::False)
        .description(Some(description))
        .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
        .build()
}

fn node_key(node: &Node, field: &'static SortField) -> CursorKey {
    match node {
        Node::Folder(child) => {
            let value = match field.name() {
                "size" => SortValue::Integer(child.totals.total_bytes),
                "createdAt" => SortValue::Text(child.record.created_at.to_string()),
                "updatedAt" => SortValue::Text(child.record.updated_at.to_string()),
                _ => SortValue::Text(child.record.name_normalized.clone()),
            };
            CursorKey::in_group(FOLDER_GROUP, value, child.record.id)
        }
        Node::File(file) => {
            let value = match field.name() {
                "size" => SortValue::Integer(file.size_bytes),
                "createdAt" => SortValue::Text(file.created_at.to_string()),
                "updatedAt" => SortValue::Text(file.updated_at.to_string()),
                _ => SortValue::Text(file.name_normalized.clone()),
            };
            CursorKey::in_group(FILE_GROUP, value, file.id)
        }
    }
}

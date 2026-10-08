use std::collections::HashMap;
use std::str::FromStr;

use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::domain::naming::NameCandidate;
use crate::domain::time::Timestamp;
use crate::features::files::naming_insert::{Attempt, NameNamespace};
use crate::features::users::model::UserId;
use crate::infra::http::pagination::{Conjunction, CursorKey, PageRequest};

use super::error::FolderError;
use super::model::{
    Crumb, FolderId, FolderRecord, FolderTotals, OwnedFolder, TreeRow, MAX_FOLDER_DEPTH,
    TREE_NODE_CAP,
};

const FIND_OWNED: &str = "SELECT id, parent_id, depth FROM folders WHERE id = ?1 AND owner_id = ?2";

const GET_RECORD: &str = "SELECT id, parent_id, name, name_normalized, description, created_at, \
    updated_at FROM folders WHERE id = ?1 AND owner_id = ?2";

const INSERT_COLUMNS: &str = "INSERT INTO folders \
    (id, owner_id, parent_id, name, name_normalized, description, depth, created_at, updated_at) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)";

const RENAME: &str = "UPDATE folders SET name = ?1, name_normalized = ?2, updated_at = ?3 \
    WHERE id = ?4 AND owner_id = ?5";

const SET_DESCRIPTION: &str =
    "UPDATE folders SET description = ?1, updated_at = ?2 WHERE id = ?3 AND owner_id = ?4";

const FIND_FOR_MOVE: &str =
    "SELECT id, parent_id, name, depth FROM folders WHERE id = ?1 AND owner_id = ?2";

const FIND_CHILD_BY_NAME: &str = "SELECT id, parent_id, depth FROM folders \
    WHERE owner_id = ?1 AND parent_id = ?2 AND name_normalized = ?3";

const FIND_ROOT_BY_NAME: &str = "SELECT id, parent_id, depth FROM folders \
    WHERE owner_id = ?1 AND parent_id IS NULL AND name_normalized = ?2";

const PROFILE_SUBTREE: &str = "WITH RECURSIVE subtree(id, depth, level) AS (
    SELECT id, depth, 0 FROM folders WHERE id = ?1 AND owner_id = ?2
    UNION ALL
    SELECT f.id, f.depth, s.level + 1
      FROM folders f JOIN subtree s ON f.parent_id = s.id
     WHERE f.owner_id = ?2 AND s.level < ?3
)
SELECT COALESCE(MAX(depth), 0) AS max_depth,
       COALESCE(MAX(id = ?4), 0) AS contains_destination
  FROM subtree";

const RELOCATE: &str = "UPDATE folders \
    SET parent_id = ?1, name = ?2, name_normalized = ?3, depth = ?4, updated_at = ?5 \
    WHERE id = ?6 AND owner_id = ?7";

const SHIFT_DESCENDANT_DEPTHS: &str = "WITH RECURSIVE subtree(id, level) AS (
    SELECT id, 1 FROM folders WHERE parent_id = ?1 AND owner_id = ?2
    UNION ALL
    SELECT f.id, s.level + 1
      FROM folders f JOIN subtree s ON f.parent_id = s.id
     WHERE f.owner_id = ?2 AND s.level < ?3
)
UPDATE folders SET depth = depth + ?4
 WHERE owner_id = ?2 AND id IN (SELECT id FROM subtree)";

const TOTALS: &str = "WITH RECURSIVE roots(id) AS (
    SELECT f.id FROM folders f, json_each(?2) j WHERE f.owner_id = ?1 AND f.id = j.value
), walk(root_id, id, level) AS (
    SELECT id, id, 0 FROM roots
    UNION ALL
    SELECT w.root_id, c.id, w.level + 1
      FROM folders c JOIN walk w ON c.parent_id = w.id
     WHERE c.owner_id = ?1 AND w.level < ?3
), folder_totals AS (
    SELECT root_id, COUNT(*) AS subfolders FROM walk WHERE level > 0 GROUP BY root_id
), file_totals AS (
    SELECT w.root_id, COUNT(*) AS files, COALESCE(SUM(fi.size_bytes), 0) AS bytes
      FROM walk w JOIN files fi ON fi.folder_id = w.id AND fi.owner_id = ?1
     GROUP BY w.root_id
)
SELECT r.id AS root_id,
       COALESCE(ft.subfolders, 0) AS subfolders,
       COALESCE(t.files, 0) AS files,
       COALESCE(t.bytes, 0) AS bytes
  FROM roots r
  LEFT JOIN folder_totals ft ON ft.root_id = r.id
  LEFT JOIN file_totals t ON t.root_id = r.id";

const BREADCRUMBS: &str = "WITH RECURSIVE crumbs(id, parent_id, name, level) AS (
    SELECT id, parent_id, name, 0 FROM folders WHERE id = ?1 AND owner_id = ?2
    UNION ALL
    SELECT f.id, f.parent_id, f.name, c.level + 1
      FROM folders f JOIN crumbs c ON f.id = c.parent_id
     WHERE f.owner_id = ?2 AND c.level < ?3
)
SELECT id, name FROM crumbs ORDER BY level DESC";

const TREE_FROM_ROOT_LEVEL: &str = "parent_id IS NULL";
const TREE_FROM_FOLDER: &str = "id = ?4";

fn tree_sql(anchor: &str) -> String {
    format!(
        "WITH RECURSIVE walk(id, parent_id, name, name_normalized, level) AS (
    SELECT id, parent_id, name, name_normalized, 1 FROM folders
     WHERE owner_id = ?1 AND {anchor}
    UNION ALL
    SELECT f.id, f.parent_id, f.name, f.name_normalized, w.level + 1
      FROM folders f JOIN walk w ON f.parent_id = w.id
     WHERE f.owner_id = ?1 AND w.level < ?2
    ORDER BY 5, 4, 1
    LIMIT ?3
)
SELECT w.id, w.parent_id, w.name, w.level,
       EXISTS (SELECT 1 FROM folders c WHERE c.parent_id = w.id AND c.owner_id = ?1) AS has_children
  FROM walk w
 ORDER BY w.level, w.name_normalized, w.id"
    )
}

pub async fn find_owned<'e, E>(
    executor: E,
    owner: UserId,
    id: FolderId,
) -> Result<Option<OwnedFolder>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(FIND_OWNED)
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(executor)
        .await?;
    row.map(|row| {
        let depth: i64 = column(&row, "depth")?;
        Ok(OwnedFolder {
            id: parsed(&row, "id")?,
            parent_id: optional_parsed(&row, "parent_id")?,
            depth: stored_depth(depth)?,
        })
    })
    .transpose()
}

pub async fn get_record<'e, E>(
    executor: E,
    owner: UserId,
    id: FolderId,
) -> Result<Option<FolderRecord>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(GET_RECORD)
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(executor)
        .await?;
    row.as_ref().map(record_from).transpose()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveSource {
    pub parent_id: Option<FolderId>,
    pub name: String,
    pub depth: u8,
}

pub async fn find_for_move(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: FolderId,
) -> Result<Option<MoveSource>, FolderError> {
    let row = sqlx::query(FIND_FOR_MOVE)
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?;
    row.map(|row| {
        let depth: i64 = column(&row, "depth")?;
        Ok(MoveSource {
            parent_id: optional_parsed(&row, "parent_id")?,
            name: column(&row, "name")?,
            depth: stored_depth(depth)?,
        })
    })
    .transpose()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubtreeProfile {
    pub max_depth: i64,
    pub contains_destination: bool,
}

pub async fn profile_subtree(
    connection: &mut SqliteConnection,
    owner: UserId,
    root: FolderId,
    destination: Option<FolderId>,
) -> Result<SubtreeProfile, FolderError> {
    let row = sqlx::query(PROFILE_SUBTREE)
        .bind(root.to_string())
        .bind(owner.to_string())
        .bind(MAX_FOLDER_DEPTH)
        .bind(destination.map(|destination| destination.to_string()))
        .fetch_one(connection)
        .await?;
    let contains: i64 = column(&row, "contains_destination")?;
    Ok(SubtreeProfile {
        max_depth: column(&row, "max_depth")?,
        contains_destination: contains != 0,
    })
}

pub struct Relocation {
    pub owner: UserId,
    pub id: FolderId,
    pub parent: Option<FolderId>,
    pub depth: u8,
    pub at: Timestamp,
}

pub async fn relocate(
    connection: &mut SqliteConnection,
    relocation: &Relocation,
    candidate: &NameCandidate,
) -> Result<Attempt<()>, FolderError> {
    let result = sqlx::query(RELOCATE)
        .bind(relocation.parent.map(|parent| parent.to_string()))
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(i64::from(relocation.depth))
        .bind(relocation.at.to_string())
        .bind(relocation.id.to_string())
        .bind(relocation.owner.to_string())
        .execute(connection)
        .await;
    match result {
        Ok(done) if done.rows_affected() == 0 => Err(FolderError::NotFound),
        other => Ok(Attempt::from_name_update(other)?),
    }
}

pub async fn shift_descendant_depths(
    connection: &mut SqliteConnection,
    owner: UserId,
    root: FolderId,
    delta: i64,
) -> Result<u64, FolderError> {
    let done = sqlx::query(SHIFT_DESCENDANT_DEPTHS)
        .bind(root.to_string())
        .bind(owner.to_string())
        .bind(MAX_FOLDER_DEPTH)
        .bind(delta)
        .execute(connection)
        .await?;
    Ok(done.rows_affected())
}

pub async fn find_child_by_normalized_name(
    connection: &mut SqliteConnection,
    owner: UserId,
    parent: Option<FolderId>,
    normalized: &str,
) -> Result<Option<OwnedFolder>, FolderError> {
    let row = match parent {
        Some(parent) => {
            sqlx::query(FIND_CHILD_BY_NAME)
                .bind(owner.to_string())
                .bind(parent.to_string())
                .bind(normalized)
                .fetch_optional(connection)
                .await?
        }
        None => {
            sqlx::query(FIND_ROOT_BY_NAME)
                .bind(owner.to_string())
                .bind(normalized)
                .fetch_optional(connection)
                .await?
        }
    };
    row.map(|row| {
        let depth: i64 = column(&row, "depth")?;
        Ok(OwnedFolder {
            id: parsed(&row, "id")?,
            parent_id: optional_parsed(&row, "parent_id")?,
            depth: stored_depth(depth)?,
        })
    })
    .transpose()
}

pub struct NewRow {
    pub id: FolderId,
    pub owner: UserId,
    pub parent: Option<FolderId>,
    pub description: Option<String>,
    pub depth: u8,
    pub at: Timestamp,
}

pub async fn insert(
    connection: &mut SqliteConnection,
    row: &NewRow,
    candidate: &NameCandidate,
) -> Result<Attempt<()>, FolderError> {
    let namespace = if row.parent.is_some() {
        NameNamespace::FoldersInFolder
    } else {
        NameNamespace::FoldersAtRoot
    };
    let sql = format!("{INSERT_COLUMNS} {}", namespace.conflict_clause());
    let result = sqlx::query(&sql)
        .bind(row.id.to_string())
        .bind(row.owner.to_string())
        .bind(row.parent.map(|parent| parent.to_string()))
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(row.description.as_deref())
        .bind(i64::from(row.depth))
        .bind(row.at.to_string())
        .execute(connection)
        .await?;
    Ok(Attempt::from_insert(&result))
}

pub async fn rename(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: FolderId,
    candidate: &NameCandidate,
    at: Timestamp,
) -> Result<Attempt<()>, FolderError> {
    let result = sqlx::query(RENAME)
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(at.to_string())
        .bind(id.to_string())
        .bind(owner.to_string())
        .execute(connection)
        .await;
    match result {
        Ok(done) if done.rows_affected() == 0 => Err(FolderError::NotFound),
        other => Ok(Attempt::from_name_update(other)?),
    }
}

pub async fn set_description(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: FolderId,
    description: Option<&str>,
    at: Timestamp,
) -> Result<(), FolderError> {
    let done = sqlx::query(SET_DESCRIPTION)
        .bind(description)
        .bind(at.to_string())
        .bind(id.to_string())
        .bind(owner.to_string())
        .execute(connection)
        .await?;
    if done.rows_affected() == 0 {
        Err(FolderError::NotFound)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub owner: UserId,
    pub parent: Option<FolderId>,
    pub search: Option<&'a str>,
}

fn push_scope(query: &mut QueryBuilder<'_, Sqlite>, scope: &Scope<'_>) {
    query
        .push(" WHERE owner_id = ")
        .push_bind(scope.owner.to_string());
    match scope.parent {
        Some(parent) => query
            .push(" AND parent_id = ")
            .push_bind(parent.to_string()),
        None => query.push(" AND parent_id IS NULL"),
    };
    if let Some(search) = scope.search {
        query
            .push(" AND instr(name_normalized, ")
            .push_bind(search.to_owned())
            .push(") > 0");
    }
}

pub async fn list<'e, E>(
    executor: E,
    scope: &Scope<'_>,
    page: &PageRequest,
) -> Result<Vec<FolderRecord>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT id, parent_id, name, name_normalized, description, created_at, updated_at \
         FROM folders",
    );
    push_scope(&mut query, scope);
    page.push_keyset(&mut query, Conjunction::And);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(executor).await?;
    rows.iter().map(record_from).collect()
}

pub async fn list_children<'e, E>(
    executor: E,
    scope: &Scope<'_>,
    page: &PageRequest,
    after: Option<&CursorKey>,
    fetch: i64,
) -> Result<Vec<FolderRecord>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = if page.sort().field().name() == "size" {
        sized_children_query(scope)
    } else {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, parent_id, name, name_normalized, description, created_at, updated_at \
             FROM folders",
        );
        push_scope(&mut query, scope);
        query
    };
    let conjunction = if page.sort().field().name() == "size" {
        Conjunction::Where
    } else {
        Conjunction::And
    };
    page.push_keyset_after(&mut query, conjunction, after);
    page.push_order_and_fetch(&mut query, fetch);
    let rows = query.build().fetch_all(executor).await?;
    rows.iter().map(record_from).collect()
}

fn sized_children_query<'q>(scope: &Scope<'_>) -> QueryBuilder<'q, Sqlite> {
    let owner = scope.owner.to_string();
    let mut query = QueryBuilder::<Sqlite>::new(
        "WITH RECURSIVE kids(id) AS (SELECT id FROM folders WHERE owner_id = ",
    );
    query.push_bind(owner.clone());
    push_parent(&mut query, "", scope.parent);
    query
        .push(
            "), walk(root_id, id, level) AS (
    SELECT id, id, 0 FROM kids
    UNION ALL
    SELECT w.root_id, c.id, w.level + 1
      FROM folders c JOIN walk w ON c.parent_id = w.id
     WHERE c.owner_id = ",
        )
        .push_bind(owner.clone())
        .push(" AND w.level < ")
        .push_bind(MAX_FOLDER_DEPTH)
        .push(
            "), sizes(root_id, size_bytes) AS (
    SELECT w.root_id, SUM(fi.size_bytes)
      FROM walk w JOIN files fi ON fi.folder_id = w.id AND fi.owner_id = ",
        )
        .push_bind(owner.clone())
        .push(
            " GROUP BY w.root_id
), sized AS (
    SELECT f.id, f.parent_id, f.name, f.name_normalized, f.description, f.created_at,
           f.updated_at, COALESCE(s.size_bytes, 0) AS size_bytes
      FROM folders f LEFT JOIN sizes s ON s.root_id = f.id
     WHERE f.owner_id = ",
        )
        .push_bind(owner);
    push_parent(&mut query, "f.", scope.parent);
    query.push(
        ")
SELECT id, parent_id, name, name_normalized, description, created_at, updated_at, size_bytes
  FROM sized",
    );
    query
}

fn push_parent(query: &mut QueryBuilder<'_, Sqlite>, alias: &str, parent: Option<FolderId>) {
    match parent {
        Some(parent) => query
            .push(format!(" AND {alias}parent_id = "))
            .push_bind(parent.to_string()),
        None => query.push(format!(" AND {alias}parent_id IS NULL")),
    };
}

pub async fn count<'e, E>(executor: E, scope: &Scope<'_>) -> Result<u64, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM folders");
    push_scope(&mut query, scope);
    let count: i64 = query.build_query_scalar().fetch_one(executor).await?;
    u64::try_from(count).map_err(|_| invariant("count"))
}

pub async fn totals<'e, E>(
    executor: E,
    owner: UserId,
    ids: Vec<FolderId>,
) -> Result<HashMap<FolderId, FolderTotals>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let listed: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let listed = serde_json::to_string(&listed).map_err(|_| invariant("ids"))?;
    let rows = sqlx::query(TOTALS)
        .bind(owner.to_string())
        .bind(listed)
        .bind(MAX_FOLDER_DEPTH)
        .fetch_all(executor)
        .await?;
    rows.iter()
        .map(|row| {
            let subfolders: i64 = column(row, "subfolders")?;
            let files: i64 = column(row, "files")?;
            Ok((
                parsed(row, "root_id")?,
                FolderTotals {
                    file_count: u64::try_from(files).map_err(|_| invariant("files"))?,
                    subfolder_count: u64::try_from(subfolders)
                        .map_err(|_| invariant("subfolders"))?,
                    total_bytes: column(row, "bytes")?,
                },
            ))
        })
        .collect()
}

pub async fn breadcrumbs<'e, E>(
    executor: E,
    owner: UserId,
    id: FolderId,
) -> Result<Vec<Crumb>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query(BREADCRUMBS)
        .bind(id.to_string())
        .bind(owner.to_string())
        .bind(MAX_FOLDER_DEPTH)
        .fetch_all(executor)
        .await?;
    rows.iter()
        .map(|row| {
            Ok(Crumb {
                id: parsed(row, "id")?,
                name: column(row, "name")?,
            })
        })
        .collect()
}

pub async fn tree<'e, E>(
    executor: E,
    owner: UserId,
    root: Option<FolderId>,
    depth: u8,
) -> Result<Vec<TreeRow>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let sql = tree_sql(if root.is_some() {
        TREE_FROM_FOLDER
    } else {
        TREE_FROM_ROOT_LEVEL
    });
    let fetch_limit = i64::try_from(TREE_NODE_CAP + 1).map_err(|_| invariant("limit"))?;
    let mut query = sqlx::query(&sql)
        .bind(owner.to_string())
        .bind(i64::from(depth))
        .bind(fetch_limit);
    if let Some(root) = root {
        query = query.bind(root.to_string());
    }
    let rows = query.fetch_all(executor).await?;
    rows.iter()
        .map(|row| {
            let level: i64 = column(row, "level")?;
            Ok(TreeRow {
                id: parsed(row, "id")?,
                parent_id: optional_parsed(row, "parent_id")?,
                name: column(row, "name")?,
                level: u8::try_from(level).map_err(|_| invariant("level"))?,
                has_children: column(row, "has_children")?,
            })
        })
        .collect()
}

fn record_from(row: &SqliteRow) -> Result<FolderRecord, FolderError> {
    Ok(FolderRecord {
        id: parsed(row, "id")?,
        parent_id: optional_parsed(row, "parent_id")?,
        name: column(row, "name")?,
        name_normalized: column(row, "name_normalized")?,
        description: column(row, "description")?,
        created_at: parsed(row, "created_at")?,
        updated_at: parsed(row, "updated_at")?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, FolderError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, FolderError> {
    let text: String = column(row, name)?;
    text.parse().map_err(|_| invariant(name))
}

fn optional_parsed<T: FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<T>, FolderError> {
    let text: Option<String> = column(row, name)?;
    text.map(|text| text.parse().map_err(|_| invariant(name)))
        .transpose()
}

fn stored_depth(depth: i64) -> Result<u8, FolderError> {
    u8::try_from(depth)
        .ok()
        .filter(|depth| i64::from(*depth) <= MAX_FOLDER_DEPTH)
        .ok_or_else(|| invariant("depth"))
}

const fn invariant(column: &'static str) -> FolderError {
    FolderError::RepositoryInvariant { column }
}

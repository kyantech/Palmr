use std::str::FromStr;

use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::domain::naming::NameCandidate;
use crate::domain::time::Timestamp;
use crate::features::folders::FolderId;
use crate::features::users::model::UserId;
use crate::infra::http::pagination::{Conjunction, CursorKey, PageRequest};

use super::error::FileError;
use super::model::{FileId, FileRecord, FileSource};
use super::naming_insert::Attempt;

const COLUMNS: &str = "id, folder_id, name, name_normalized, description, size_bytes, mime_type, \
    created_at, updated_at";

const GET_RECORD: &str = "SELECT id, folder_id, name, name_normalized, description, size_bytes, \
    mime_type, created_at, updated_at FROM files WHERE id = ?1 AND owner_id = ?2";

const FIND_SOURCE: &str =
    "SELECT folder_id, name, description FROM files WHERE id = ?1 AND owner_id = ?2";

const RENAME: &str = "UPDATE files \
    SET name = ?1, name_normalized = ?2, extension = ?3, updated_at = ?4 \
    WHERE id = ?5 AND owner_id = ?6";

const RELOCATE: &str = "UPDATE files \
    SET folder_id = ?1, name = ?2, name_normalized = ?3, extension = ?4, updated_at = ?5 \
    WHERE id = ?6 AND owner_id = ?7";

const SET_DESCRIPTION: &str =
    "UPDATE files SET description = ?1, updated_at = ?2 WHERE id = ?3 AND owner_id = ?4";

const NAME_TAKEN_IN_FOLDER: &str = "SELECT EXISTS (SELECT 1 FROM files \
    WHERE owner_id = ?1 AND folder_id = ?2 AND name_normalized = ?3)";

const NAME_TAKEN_AT_ROOT: &str = "SELECT EXISTS (SELECT 1 FROM files \
    WHERE owner_id = ?1 AND folder_id IS NULL AND name_normalized = ?2)";

pub async fn get_record<'e, E>(
    executor: E,
    owner: UserId,
    id: FileId,
) -> Result<Option<FileRecord>, FileError>
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

pub async fn find_source(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: FileId,
) -> Result<Option<FileSource>, FileError> {
    let row = sqlx::query(FIND_SOURCE)
        .bind(id.to_string())
        .bind(owner.to_string())
        .fetch_optional(connection)
        .await?;
    row.map(|row| {
        Ok(FileSource {
            folder_id: optional_parsed(&row, "folder_id")?,
            name: column(&row, "name")?,
            description: column(&row, "description")?,
        })
    })
    .transpose()
}

pub async fn rename(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: FileId,
    candidate: &NameCandidate,
    at: Timestamp,
) -> Result<Attempt<()>, FileError> {
    let result = sqlx::query(RENAME)
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(candidate.extension())
        .bind(at.to_string())
        .bind(id.to_string())
        .bind(owner.to_string())
        .execute(connection)
        .await;
    stored_or_taken(result)
}

pub struct Relocation {
    pub owner: UserId,
    pub id: FileId,
    pub folder: Option<FolderId>,
    pub at: Timestamp,
}

pub async fn relocate(
    connection: &mut SqliteConnection,
    relocation: &Relocation,
    candidate: &NameCandidate,
) -> Result<Attempt<()>, FileError> {
    let result = sqlx::query(RELOCATE)
        .bind(relocation.folder.map(|folder| folder.to_string()))
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(candidate.extension())
        .bind(relocation.at.to_string())
        .bind(relocation.id.to_string())
        .bind(relocation.owner.to_string())
        .execute(connection)
        .await;
    stored_or_taken(result)
}

fn stored_or_taken(
    result: Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>,
) -> Result<Attempt<()>, FileError> {
    match result {
        Ok(done) if done.rows_affected() == 0 => Err(FileError::NotFound),
        other => Ok(Attempt::from_name_update(other)?),
    }
}

pub async fn set_description(
    connection: &mut SqliteConnection,
    owner: UserId,
    id: FileId,
    description: Option<&str>,
    at: Timestamp,
) -> Result<(), FileError> {
    let done = sqlx::query(SET_DESCRIPTION)
        .bind(description)
        .bind(at.to_string())
        .bind(id.to_string())
        .bind(owner.to_string())
        .execute(connection)
        .await?;
    if done.rows_affected() == 0 {
        Err(FileError::NotFound)
    } else {
        Ok(())
    }
}

pub async fn name_taken<'e, E>(
    executor: E,
    owner: UserId,
    folder: Option<FolderId>,
    normalized: &str,
) -> Result<bool, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let taken: i64 = match folder {
        Some(folder) => {
            sqlx::query_scalar(NAME_TAKEN_IN_FOLDER)
                .bind(owner.to_string())
                .bind(folder.to_string())
                .bind(normalized)
                .fetch_one(executor)
                .await?
        }
        None => {
            sqlx::query_scalar(NAME_TAKEN_AT_ROOT)
                .bind(owner.to_string())
                .bind(normalized)
                .fetch_one(executor)
                .await?
        }
    };
    Ok(taken != 0)
}

fn push_scope(query: &mut QueryBuilder<'_, Sqlite>, owner: UserId, folder: Option<FolderId>) {
    query
        .push(" WHERE owner_id = ")
        .push_bind(owner.to_string());
    match folder {
        Some(folder) => query
            .push(" AND folder_id = ")
            .push_bind(folder.to_string()),
        None => query.push(" AND folder_id IS NULL"),
    };
}

pub async fn list_children<'e, E>(
    executor: E,
    owner: UserId,
    folder: Option<FolderId>,
    page: &PageRequest,
    after: Option<&CursorKey>,
    fetch: i64,
) -> Result<Vec<FileRecord>, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = QueryBuilder::<Sqlite>::new(format!("SELECT {COLUMNS} FROM files"));
    push_scope(&mut query, owner, folder);
    page.push_keyset_after(&mut query, Conjunction::And, after);
    page.push_order_and_fetch(&mut query, fetch);
    let rows = query.build().fetch_all(executor).await?;
    rows.iter().map(record_from).collect()
}

pub async fn count_children<'e, E>(
    executor: E,
    owner: UserId,
    folder: Option<FolderId>,
) -> Result<u64, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM files");
    push_scope(&mut query, owner, folder);
    let count: i64 = query.build_query_scalar().fetch_one(executor).await?;
    u64::try_from(count).map_err(|_| invariant("count"))
}

fn record_from(row: &SqliteRow) -> Result<FileRecord, FileError> {
    Ok(FileRecord {
        id: parsed(row, "id")?,
        folder_id: optional_parsed(row, "folder_id")?,
        name: column(row, "name")?,
        name_normalized: column(row, "name_normalized")?,
        description: column(row, "description")?,
        size_bytes: column(row, "size_bytes")?,
        mime_type: column(row, "mime_type")?,
        created_at: parsed(row, "created_at")?,
        updated_at: parsed(row, "updated_at")?,
    })
}

fn column<'r, T>(row: &'r SqliteRow, name: &'static str) -> Result<T, FileError>
where
    T: sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(name).map_err(|_| invariant(name))
}

fn parsed<T: FromStr>(row: &SqliteRow, name: &'static str) -> Result<T, FileError> {
    let text: String = column(row, name)?;
    text.parse().map_err(|_| invariant(name))
}

fn optional_parsed<T: FromStr>(
    row: &SqliteRow,
    name: &'static str,
) -> Result<Option<T>, FileError> {
    let text: Option<String> = column(row, name)?;
    text.map(|text| text.parse().map_err(|_| invariant(name)))
        .transpose()
}

const fn invariant(column: &'static str) -> FileError {
    FileError::RepositoryInvariant { column }
}

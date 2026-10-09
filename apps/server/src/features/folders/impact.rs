use serde::Serialize;
use sqlx::{Row, Sqlite};
use utoipa::ToSchema;

use crate::domain::time::Timestamp;
use crate::features::files::FileId;
use crate::features::quota::arith::Limbs;
use crate::features::users::model::UserId;
use crate::infra::http::pagination::WireBytes;

use super::error::FolderError;
use super::model::FolderId;
use super::visibility::folder_hidden_sql;

pub const MAX_LISTED_SHARES: i64 = 100;

const COUNT_VISIBLE_FILES: &str = concat!(
    "SELECT COUNT(*) FROM files f, json_each(?2) j WHERE f.id = j.value AND f.owner_id = ?1 AND NOT ",
    folder_hidden_sql!("f.folder_id", "f.owner_id")
);

const COUNT_VISIBLE_FOLDERS: &str = concat!(
    "SELECT COUNT(*) FROM folders f, json_each(?2) j WHERE f.id = j.value AND f.owner_id = ?1 AND NOT ",
    folder_hidden_sql!("f.id", "f.owner_id")
);

const IMPACT: &str = "WITH RECURSIVE
roots(id) AS (
    SELECT f.id FROM folders f, json_each(?3) j
     WHERE f.id = j.value AND f.owner_id = ?1 AND f.deleting = 0
),
walk(id, level) AS (
    SELECT id, 0 FROM roots
    UNION ALL
    SELECT c.id, w.level + 1 FROM folders c JOIN walk w ON c.parent_id = w.id
     WHERE c.owner_id = ?1 AND c.deleting = 0 AND w.level < 64
),
doomed_folders(id) AS MATERIALIZED (SELECT DISTINCT id FROM walk),
doomed_files(id, size_bytes) AS MATERIALIZED (
    SELECT fi.id, fi.size_bytes FROM files fi
     WHERE fi.owner_id = ?1 AND fi.folder_id IN (SELECT id FROM doomed_folders)
    UNION
    SELECT fi.id, fi.size_bytes FROM files fi, json_each(?2) j
     WHERE fi.id = j.value AND fi.owner_id = ?1
),
seeds(id) AS (
    SELECT id FROM roots
    UNION
    SELECT fi.folder_id FROM files fi, json_each(?2) j
     WHERE fi.id = j.value AND fi.owner_id = ?1 AND fi.folder_id IS NOT NULL
),
ancestry(id, level) AS (
    SELECT id, 0 FROM seeds
    UNION ALL
    SELECT c.parent_id, a.level + 1 FROM folders c JOIN ancestry a ON c.id = a.id
     WHERE c.owner_id = ?1 AND c.parent_id IS NOT NULL AND a.level < 64
),
affected(share_id) AS (
    SELECT si.share_id FROM share_items si WHERE si.file_id IN (SELECT id FROM doomed_files)
    UNION
    SELECT si.share_id FROM share_items si WHERE si.folder_id IN (SELECT id FROM doomed_folders)
    UNION
    SELECT si.share_id FROM share_items si WHERE si.folder_id IN (SELECT id FROM ancestry)
),
affected_shares(id, name, alias) AS MATERIALIZED (
    SELECT s.id, s.name, s.alias FROM shares s
     WHERE s.owner_id = ?1 AND s.id IN (SELECT share_id FROM affected)
)
SELECT 'totals' AS kind, NULL AS id, NULL AS name, NULL AS alias,
       (SELECT COUNT(*) FROM doomed_files) AS n1,
       (SELECT COUNT(*) FROM doomed_folders) AS n2,
       (SELECT COALESCE(SUM(size_bytes & 2097151), 0) FROM doomed_files) AS n3,
       (SELECT COALESCE(SUM((size_bytes >> 21) & 2097151), 0) FROM doomed_files) AS n4,
       (SELECT COALESCE(SUM(size_bytes >> 42), 0) FROM doomed_files) AS n5
UNION ALL
SELECT 'embeds', NULL, NULL, NULL,
       (SELECT COUNT(*) FROM embed_grants g
         WHERE g.owner_id = ?1 AND g.file_id IN (SELECT id FROM doomed_files)
           AND g.revoked_at IS NULL AND (g.expires_at IS NULL OR g.expires_at > ?4)),
       0, 0, 0, 0
UNION ALL
SELECT 'share_count', NULL, NULL, NULL, (SELECT COUNT(*) FROM affected_shares), 0, 0, 0, 0
UNION ALL
SELECT * FROM (
    SELECT 'share', s.id, s.name, s.alias,
           (SELECT COUNT(*) FROM share_items x
             WHERE x.share_id = s.id
               AND NOT (COALESCE(x.file_id IN (SELECT id FROM doomed_files), 0)
                     OR COALESCE(x.folder_id IN (SELECT id FROM doomed_folders), 0))),
           0, 0, 0, 0
      FROM affected_shares s
     ORDER BY s.alias, s.id
     LIMIT ?5
)";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AffectedShare {
    pub id: String,
    #[schema(required = true)]
    pub name: Option<String>,
    pub alias: String,
    #[schema(minimum = 0)]
    pub remaining_items: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeletionImpact {
    #[schema(minimum = 0)]
    pub files: u64,
    #[schema(minimum = 0)]
    pub folders: u64,
    pub total_bytes: WireBytes,
    pub affected_shares: Vec<AffectedShare>,
    #[schema(minimum = 0)]
    pub affected_share_count: u64,
    #[schema(minimum = 0)]
    pub affected_embeds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    File,
    Folder,
}

fn id_list<T: ToString>(ids: &[T]) -> Result<String, FolderError> {
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    serde_json::to_string(&ids).map_err(|_| FolderError::RepositoryInvariant { column: "ids" })
}

pub async fn ensure_selectable<'e, E>(
    executor: E,
    owner: UserId,
    files: &[FileId],
    folders: &[FolderId],
) -> Result<Option<Missing>, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite> + Copy,
{
    if !files.is_empty() {
        let found: i64 = sqlx::query_scalar(COUNT_VISIBLE_FILES)
            .bind(owner.to_string())
            .bind(id_list(files)?)
            .fetch_one(executor)
            .await?;
        if usize::try_from(found).ok() != Some(files.len()) {
            return Ok(Some(Missing::File));
        }
    }
    if !folders.is_empty() {
        let found: i64 = sqlx::query_scalar(COUNT_VISIBLE_FOLDERS)
            .bind(owner.to_string())
            .bind(id_list(folders)?)
            .fetch_one(executor)
            .await?;
        if usize::try_from(found).ok() != Some(folders.len()) {
            return Ok(Some(Missing::Folder));
        }
    }
    Ok(None)
}

fn count(row: &sqlx::sqlite::SqliteRow, column: &'static str) -> Result<u64, FolderError> {
    let value: i64 = row
        .try_get(column)
        .map_err(|_| FolderError::RepositoryInvariant { column })?;
    u64::try_from(value).map_err(|_| FolderError::RepositoryInvariant { column })
}

pub async fn deletion_impact<'e, E>(
    executor: E,
    owner: UserId,
    files: &[FileId],
    folders: &[FolderId],
    now: Timestamp,
) -> Result<DeletionImpact, FolderError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query(IMPACT)
        .bind(owner.to_string())
        .bind(id_list(files)?)
        .bind(id_list(folders)?)
        .bind(now.to_string())
        .bind(MAX_LISTED_SHARES)
        .fetch_all(executor)
        .await?;
    let invariant = |column| FolderError::RepositoryInvariant { column };
    let mut impact = DeletionImpact {
        files: 0,
        folders: 0,
        total_bytes: WireBytes::MAX,
        affected_shares: Vec::new(),
        affected_share_count: 0,
        affected_embeds: 0,
    };
    for row in &rows {
        let kind: String = row.try_get("kind").map_err(|_| invariant("kind"))?;
        match kind.as_str() {
            "totals" => {
                impact.files = count(row, "n1")?;
                impact.folders = count(row, "n2")?;
                let limbs = Limbs {
                    low: row.try_get("n3").map_err(|_| invariant("n3"))?,
                    mid: row.try_get("n4").map_err(|_| invariant("n4"))?,
                    high: row.try_get("n5").map_err(|_| invariant("n5"))?,
                };
                let total = limbs
                    .total("deletion_impact_bytes")
                    .map_err(|_| invariant("total_bytes"))?;
                impact.total_bytes = WireBytes::clamped(total).0;
            }
            "embeds" => impact.affected_embeds = count(row, "n1")?,
            "share_count" => impact.affected_share_count = count(row, "n1")?,
            "share" => impact.affected_shares.push(AffectedShare {
                id: row.try_get("id").map_err(|_| invariant("id"))?,
                name: row.try_get("name").map_err(|_| invariant("name"))?,
                alias: row.try_get("alias").map_err(|_| invariant("alias"))?,
                remaining_items: count(row, "n1")?,
            }),
            _ => return Err(invariant("kind")),
        }
    }
    Ok(impact)
}

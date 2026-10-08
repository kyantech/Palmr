use sqlx::{QueryBuilder, Row, Sqlite};

use crate::domain::error_code::ErrorCode;
use crate::domain::normalize::normalize;
use crate::features::folders::{folder_paths, FolderId};
use crate::features::users::model::UserId;
use crate::infra::crypto::hkdf::KeyRing;
use crate::infra::http::error::ApiError;
use crate::infra::http::pagination::{
    invalid_param, CursorKey, Page, PageRequest, QueryParams, SearchQuery, SortAllowlist,
    SortDirection, SortField, SortKeyKind, SortSpec, SortValue, TotalCount, SEARCH_PARAM,
};

use super::error::FileError;
use super::model::{FileItem, FileRecord, SearchFileItem};
use super::repo::record_from;

pub const INDEXED: u8 = 0;
pub const SCANNED: u8 = 1;
const ENGINES: u8 = 2;

pub const SCAN_WINDOW: i64 = 10_000;

const RELEVANCE: &str = "relevance";
const SCORE_COLUMN: &str = "score";
const NAME_COLUMN: &str = "name_normalized";

const QUALIFIED_COLUMNS: &str = "f.id AS id, f.folder_id AS folder_id, f.name AS name, \
    f.name_normalized AS name_normalized, f.description AS description, \
    f.size_bytes AS size_bytes, f.mime_type AS mime_type, f.created_at AS created_at, \
    f.updated_at AS updated_at";

const COLUMNS: &str = "id, folder_id, name, name_normalized, description, size_bytes, mime_type, \
    created_at, updated_at";

static SEARCH_SORT_FIELDS: [SortField; 5] = [
    SortField::new(RELEVANCE, SCORE_COLUMN, SortKeyKind::Integer),
    SortField::new("name", NAME_COLUMN, SortKeyKind::Text),
    SortField::new("size", "size_bytes", SortKeyKind::Integer),
    SortField::new("createdAt", "created_at", SortKeyKind::Text),
    SortField::new("updatedAt", "updated_at", SortKeyKind::Text),
];
pub static SEARCH_SORT: SortAllowlist =
    SortAllowlist::new(&SEARCH_SORT_FIELDS, 0, SortDirection::Desc);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerms {
    text: String,
    expression: Option<String>,
}

impl SearchTerms {
    pub fn parse(raw: &str) -> Result<Self, ApiError> {
        let normalized = normalize(raw);
        let words: Vec<&str> = normalized
            .split(|c: char| c.is_whitespace() || c.is_control())
            .filter(|word| !word.is_empty())
            .collect();
        if words.is_empty() {
            return Err(invalid_param(SEARCH_PARAM));
        }
        let mut phrases: Vec<String> = Vec::with_capacity(words.len());
        for word in &words {
            if !word.chars().any(char::is_alphanumeric) {
                continue;
            }
            let phrase = format!("\"{}\"*", word.replace('"', "\"\""));
            if !phrases.contains(&phrase) {
                phrases.push(phrase);
            }
        }
        Ok(Self {
            text: words.join(" "),
            expression: (!phrases.is_empty()).then(|| phrases.join(" ")),
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn expression(&self) -> Option<&str> {
        self.expression.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    terms: SearchTerms,
    page: PageRequest,
}

impl SearchRequest {
    pub fn parse(params: &QueryParams, keys: &KeyRing) -> Result<Self, ApiError> {
        let query = SearchQuery::parse(params.single(SEARCH_PARAM)?)?
            .ok_or_else(|| invalid_param(SEARCH_PARAM))?;
        let terms = SearchTerms::parse(query.as_str())?;
        let page =
            PageRequest::from_bound_query(params, &SEARCH_SORT, keys, ENGINES, terms.text())?;
        if let Some(after) = page.after() {
            let engine = after.group().unwrap_or(INDEXED);
            if !cursor_value_fits(engine, page.sort(), after.value()) {
                return Err(ApiError::new(ErrorCode::CursorInvalid));
            }
        }
        Ok(Self { terms, page })
    }
}

fn is_relevance(sort: &SortSpec) -> bool {
    sort.field().name() == RELEVANCE
}

fn cursor_value_fits(engine: u8, sort: &SortSpec, value: &SortValue) -> bool {
    if is_relevance(sort) {
        return match (engine, value) {
            (INDEXED, SortValue::Integer(bits)) => score_from_cursor(*bits).is_finite(),
            (SCANNED, SortValue::Text(_)) => true,
            _ => false,
        };
    }
    matches!(
        (sort.field().kind(), value),
        (SortKeyKind::Text, SortValue::Text(_)) | (SortKeyKind::Integer, SortValue::Integer(_))
    )
}

fn score_to_cursor(score: f64) -> i64 {
    score.to_bits().cast_signed()
}

fn score_from_cursor(bits: i64) -> f64 {
    f64::from_bits(bits.cast_unsigned())
}

#[derive(Debug, Clone, Copy)]
struct Order {
    column: &'static str,
    descending: bool,
    score: bool,
}

impl Order {
    fn of(engine: u8, sort: &SortSpec) -> Self {
        if is_relevance(sort) {
            let indexed = engine == INDEXED;
            Self {
                column: if indexed { SCORE_COLUMN } else { NAME_COLUMN },
                descending: sort.direction() == SortDirection::Asc,
                score: indexed,
            }
        } else {
            Self {
                column: sort.field().column(),
                descending: sort.direction() == SortDirection::Desc,
                score: false,
            }
        }
    }

    fn push_after(
        self,
        query: &mut QueryBuilder<'_, Sqlite>,
        conjunction: &str,
        after: Option<&CursorKey>,
    ) {
        let Some(after) = after else {
            return;
        };
        query
            .push(conjunction)
            .push(" ((")
            .push(self.column)
            .push(", id) ")
            .push(if self.descending { "<" } else { ">" })
            .push(" (");
        match after.value() {
            SortValue::Text(text) => query.push_bind(text.clone()),
            SortValue::Integer(bits) if self.score => query.push_bind(score_from_cursor(*bits)),
            SortValue::Integer(integer) => query.push_bind(*integer),
        };
        query.push(", ").push_bind(after.id().to_owned()).push("))");
    }

    fn push_order(self, query: &mut QueryBuilder<'_, Sqlite>, fetch: i64) {
        let direction = if self.descending { " DESC" } else { " ASC" };
        query
            .push(" ORDER BY ")
            .push(self.column)
            .push(direction)
            .push(", id")
            .push(direction)
            .push(" LIMIT ")
            .push_bind(fetch);
    }
}

pub(crate) struct Probe<'a> {
    pub owner: UserId,
    pub expression: &'a str,
    pub needle: &'a str,
    pub sort: &'a SortSpec,
    pub after: Option<&'a CursorKey>,
    pub fetch: i64,
}

pub(crate) fn push_indexed(query: &mut QueryBuilder<'_, Sqlite>, probe: &Probe<'_>) {
    let order = Order::of(INDEXED, probe.sort);
    if order.score {
        query
            .push("WITH hits AS MATERIALIZED (SELECT ")
            .push(QUALIFIED_COLUMNS)
            .push(", bm25(files_fts, 10.0, 1.0) AS ")
            .push(SCORE_COLUMN)
            .push(
                " FROM files_fts CROSS JOIN files f ON f.rowid = files_fts.rowid WHERE files_fts MATCH ",
            )
            .push_bind(probe.expression.to_owned())
            .push(" AND f.owner_id = ")
            .push_bind(probe.owner.to_string())
            .push(") SELECT ")
            .push(COLUMNS)
            .push(", ")
            .push(SCORE_COLUMN)
            .push(" FROM hits");
        order.push_after(query, " WHERE", probe.after);
    } else {
        query
            .push("SELECT ")
            .push(QUALIFIED_COLUMNS)
            .push(
                " FROM files_fts CROSS JOIN files f ON f.rowid = files_fts.rowid WHERE files_fts MATCH ",
            )
            .push_bind(probe.expression.to_owned())
            .push(" AND f.owner_id = ")
            .push_bind(probe.owner.to_string());
        order.push_after(query, " AND", probe.after);
    }
    order.push_order(query, probe.fetch);
}

pub(crate) fn push_scanned(query: &mut QueryBuilder<'_, Sqlite>, probe: &Probe<'_>) {
    let order = Order::of(SCANNED, probe.sort);
    query
        .push("SELECT ")
        .push(COLUMNS)
        .push(" FROM (SELECT ")
        .push(COLUMNS)
        .push(" FROM files WHERE owner_id = ")
        .push_bind(probe.owner.to_string())
        .push(" ORDER BY created_at DESC, rowid ASC LIMIT ")
        .push_bind(SCAN_WINDOW)
        .push(") WHERE instr(name_normalized, ")
        .push_bind(probe.needle.to_owned())
        .push(") > 0");
    order.push_after(query, " AND", probe.after);
    order.push_order(query, probe.fetch);
}

struct Hit {
    record: FileRecord,
    score: Option<f64>,
}

async fn fetch_hits<'e, E>(
    executor: E,
    mut query: QueryBuilder<'_, Sqlite>,
    scored: bool,
) -> Result<Vec<Hit>, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let rows = query.build().fetch_all(executor).await?;
    rows.iter()
        .map(|row| {
            Ok(Hit {
                record: record_from(row)?,
                score: if scored {
                    Some(
                        row.try_get(SCORE_COLUMN)
                            .map_err(|_| FileError::RepositoryInvariant {
                                column: SCORE_COLUMN,
                            })?,
                    )
                } else {
                    None
                },
            })
        })
        .collect()
}

async fn indexed<'e, E>(executor: E, probe: &Probe<'_>) -> Result<Vec<Hit>, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = QueryBuilder::<Sqlite>::new("");
    push_indexed(&mut query, probe);
    fetch_hits(executor, query, Order::of(INDEXED, probe.sort).score).await
}

async fn scanned<'e, E>(executor: E, probe: &Probe<'_>) -> Result<Vec<Hit>, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let mut query = QueryBuilder::<Sqlite>::new("");
    push_scanned(&mut query, probe);
    fetch_hits(executor, query, false).await
}

fn cursor_key(engine: u8, binding: &str, field: &SortField, hit: &Hit) -> CursorKey {
    let record = &hit.record;
    let value = match field.name() {
        RELEVANCE if engine == INDEXED => {
            SortValue::Integer(score_to_cursor(hit.score.unwrap_or_default()))
        }
        "size" => SortValue::Integer(record.size_bytes),
        "createdAt" => SortValue::Text(record.created_at.to_string()),
        "updatedAt" => SortValue::Text(record.updated_at.to_string()),
        _ => SortValue::Text(record.name_normalized.clone()),
    };
    CursorKey::in_group(engine, value, record.id).bound_to(binding)
}

pub async fn search<'e, E>(
    executor: E,
    owner: UserId,
    request: SearchRequest,
    keys: &KeyRing,
) -> Result<Page<SearchFileItem>, FileError>
where
    E: sqlx::Executor<'e, Database = Sqlite> + Copy,
{
    let SearchRequest { terms, page } = request;
    let empty = || Page {
        items: Vec::new(),
        next_cursor: None,
        total_count: None,
    };
    let Some(expression) = terms.expression() else {
        return Ok(empty());
    };
    let after = page.after().cloned();
    let probe = Probe {
        owner,
        expression,
        needle: terms.text(),
        sort: page.sort(),
        after: after.as_ref(),
        fetch: page.fetch_size(),
    };
    let (engine, hits) = match after.as_ref().and_then(CursorKey::group) {
        Some(SCANNED) => (SCANNED, scanned(executor, &probe).await?),
        Some(_) => (INDEXED, indexed(executor, &probe).await?),
        None => {
            let hits = indexed(executor, &probe).await?;
            if hits.is_empty() {
                (SCANNED, scanned(executor, &probe).await?)
            } else {
                (INDEXED, hits)
            }
        }
    };
    if hits.is_empty() {
        return Ok(empty());
    }
    let binding = terms.text().to_owned();
    let page = page.into_page(
        hits,
        keys,
        |hit, field| cursor_key(engine, &binding, field, hit),
        TotalCount::Uncounted,
    );
    let folders: Vec<FolderId> = page
        .items
        .iter()
        .filter_map(|hit| hit.record.folder_id)
        .collect();
    let paths = folder_paths(executor, owner, &folders).await?;
    let items = page
        .items
        .into_iter()
        .filter_map(|hit| {
            let path = match hit.record.folder_id {
                Some(folder) => paths.get(&folder)?.clone(),
                None => Vec::new(),
            };
            Some(SearchFileItem {
                file: FileItem::from(hit.record),
                path,
            })
        })
        .collect();
    Ok(Page {
        items,
        next_cursor: page.next_cursor,
        total_count: page.total_count,
    })
}

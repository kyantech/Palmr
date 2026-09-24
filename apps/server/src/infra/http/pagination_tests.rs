use std::collections::HashMap;

use base64ct::{Base64UrlUnpadded, Encoding};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{QueryBuilder, Sqlite};
use tempfile::TempDir;
use time::macros::datetime;
use utoipa::{PartialSchema, ToSchema};

use super::error::ApiError;
use super::pagination::{
    cursor_parameter, decode_cursor, encode_cursor, limit_parameter, repeated_enum_parameter,
    search_parameter, Conjunction, CursorKey, Limit, Page, PageRequest, QueryParams, SearchQuery,
    SortAllowlist, SortDirection, SortField, SortKeyKind, SortSpec, SortValue, TotalCount,
    WireBytes, CURSOR_TAG_LEN, DEFAULT_LIMIT, MAX_LIMIT, MAX_WIRE_BYTES,
};
use crate::config::SqliteSynchronous;
use crate::domain::bytes::ByteSize;
use crate::domain::clock::TestClock;
use crate::domain::error_code::ErrorCode;
use crate::domain::id::Id;
use crate::infra::crypto::hkdf::{KeyRing, MacPurpose};
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::db::{DbError, DbPools};

struct Row;

static FIELDS: [SortField; 3] = [
    SortField::new("createdAt", "created_at", SortKeyKind::Text),
    SortField::new("name", "name_sort", SortKeyKind::Text),
    SortField::new("size", "size_bytes", SortKeyKind::Integer),
];
static ALLOWLIST: SortAllowlist = SortAllowlist::new(&FIELDS, 0, SortDirection::Desc);

static RANK_FIELDS: [SortField; 2] = [
    SortField::new("rank", "rank", SortKeyKind::Integer),
    SortField::new("label", "label", SortKeyKind::Text),
];
static RANK_ALLOWLIST: SortAllowlist = SortAllowlist::new(&RANK_FIELDS, 0, SortDirection::Asc);

fn ring() -> KeyRing {
    let dir = TempDir::new().unwrap();
    let (key, _) = InstanceKey::load_or_create(dir.path()).unwrap();
    KeyRing::new(&key)
}

fn spec(raw: &str) -> SortSpec {
    ALLOWLIST.parse(Some(raw)).unwrap()
}

fn row_id(clock: &TestClock) -> Id<Row> {
    Id::generate(clock)
}

fn assert_validation(error: &ApiError, field: &str) {
    assert_eq!(error.code(), ErrorCode::ValidationError);
    let details = serde_json::to_value(error.details()).unwrap();
    assert_eq!(details, json!({ "field": field }));
}

fn assert_cursor_invalid(result: Result<CursorKey, ApiError>) {
    let error = result.unwrap_err();
    assert_eq!(error.code(), ErrorCode::CursorInvalid);
    assert_eq!(error.status(), http::StatusCode::BAD_REQUEST);
    assert!(!error.retryable());
    assert!(error.details().is_empty());
}

fn reencode(payload: &[u8], tag: &[u8]) -> String {
    Base64UrlUnpadded::encode_string(&[payload, tag].concat())
}

#[test]
fn unit_cursor_roundtrip_and_tamper() {
    let keys = ring();
    let clock = TestClock::new(datetime!(2026-09-24 12:00 UTC));
    let sort = spec("createdAt:desc");
    let id = row_id(&clock);
    let key = CursorKey::new(SortValue::Text("2026-09-24T12:00:00.000Z".to_owned()), id);

    let cursor = encode_cursor(&keys, &sort, &key);
    assert!(cursor
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));

    let decoded = decode_cursor(&keys, &sort, &cursor).unwrap();
    assert_eq!(decoded, key);
    assert_eq!(decoded.id(), id.to_string());
    assert_eq!(
        decoded.value(),
        &SortValue::Text("2026-09-24T12:00:00.000Z".to_owned())
    );

    let raw = Base64UrlUnpadded::decode_vec(&cursor).unwrap();
    let (payload, tag) = raw.split_at(raw.len() - CURSOR_TAG_LEN);
    assert_eq!(tag.len(), 16);
    assert_eq!(tag, &keys.mac(MacPurpose::Cursor, payload)[..16]);
    assert_eq!(
        serde_json::from_slice::<Value>(payload).unwrap(),
        json!({ "sort": "createdAt:desc", "k": ["2026-09-24T12:00:00.000Z", id.to_string()] })
    );
    assert_eq!(
        payload,
        format!(r#"{{"sort":"createdAt:desc","k":["2026-09-24T12:00:00.000Z","{id}"]}}"#)
            .as_bytes(),
    );
    assert_eq!(encode_cursor(&keys, &sort, &key), cursor);

    let size_sort = spec("size:asc");
    let size_key = CursorKey::new(SortValue::Integer(4_831_838_208), id);
    let size_cursor = encode_cursor(&keys, &size_sort, &size_key);
    assert_eq!(
        decode_cursor(&keys, &size_sort, &size_cursor).unwrap(),
        size_key
    );

    let modified = String::from_utf8(payload.to_vec())
        .unwrap()
        .replace("2026-09-24", "2026-09-25");
    assert_cursor_invalid(decode_cursor(
        &keys,
        &sort,
        &reencode(modified.as_bytes(), tag),
    ));

    let mut flipped_tag = tag.to_vec();
    flipped_tag[CURSOR_TAG_LEN - 1] ^= 0x01;
    assert_cursor_invalid(decode_cursor(
        &keys,
        &sort,
        &reencode(payload, &flipped_tag),
    ));
    assert_cursor_invalid(decode_cursor(&keys, &sort, &reencode(payload, &tag[..15])));

    let forged =
        json!({ "sort": "createdAt:desc", "k": ["2000-01-01T00:00:00.000Z", id.to_string()] });
    let forged = serde_json::to_vec(&forged).unwrap();
    assert_cursor_invalid(decode_cursor(&keys, &sort, &reencode(&forged, tag)));

    assert_cursor_invalid(decode_cursor(&ring(), &sort, &cursor));

    for malformed in ["", "!!!!", "eyJ", "a", &format!("{cursor}="), &cursor[1..]] {
        assert_cursor_invalid(decode_cursor(&keys, &sort, malformed));
    }
    assert_cursor_invalid(decode_cursor(&keys, &sort, &"A".repeat(5000)));

    let signed = |payload: &[u8]| {
        let tag = keys.mac(MacPurpose::Cursor, payload);
        reencode(payload, &tag[..CURSOR_TAG_LEN])
    };
    for unsupported in [
        b"not json".to_vec(),
        serde_json::to_vec(&json!({ "sort": "createdAt:desc" })).unwrap(),
        serde_json::to_vec(&json!({ "sort": "createdAt:desc", "k": ["x"] })).unwrap(),
        serde_json::to_vec(&json!({ "sort": "createdAt:desc", "k": ["x", id.to_string(), 1] }))
            .unwrap(),
        serde_json::to_vec(&json!({ "sort": "createdAt:desc", "k": [1.5, id.to_string()] }))
            .unwrap(),
        serde_json::to_vec(&json!({ "sort": "createdAt:desc", "k": [7, id.to_string()] })).unwrap(),
        serde_json::to_vec(&json!({ "sort": "createdAt:desc", "k": ["x", "not-an-id"] })).unwrap(),
        serde_json::to_vec(
            &json!({ "sort": "createdAt:desc", "k": ["x", id.to_string().to_uppercase()] }),
        )
        .unwrap(),
        serde_json::to_vec(
            &json!({ "sort": "createdAt:desc", "k": ["x", id.to_string()], "extra": 1 }),
        )
        .unwrap(),
        serde_json::to_vec(&json!(["createdAt:desc", ["x", id.to_string()]])).unwrap(),
    ] {
        assert_cursor_invalid(decode_cursor(&keys, &sort, &signed(&unsupported)));
    }

    assert_cursor_invalid(decode_cursor(&keys, &spec("createdAt:asc"), &cursor));
    assert_cursor_invalid(decode_cursor(&keys, &spec("name:desc"), &cursor));
    let params = QueryParams::parse(Some(&format!("sort=createdAt:asc&cursor={cursor}")));
    let error = PageRequest::from_query(&params, &ALLOWLIST, &keys).unwrap_err();
    assert_eq!(error.code(), ErrorCode::CursorInvalid);
    let params = QueryParams::parse(Some(&format!("sort=createdAt:desc&cursor={cursor}")));
    let request = PageRequest::from_query(&params, &ALLOWLIST, &keys).unwrap();
    assert_eq!(request.after(), Some(&key));
}

#[test]
fn unit_sort_allowlist() {
    let ascending = ALLOWLIST.parse(Some("name:asc")).unwrap();
    assert_eq!(ascending.field().name(), "name");
    assert_eq!(ascending.field().column(), "name_sort");
    assert_eq!(ascending.direction(), SortDirection::Asc);
    assert_eq!(ascending.wire(), "name:asc");

    let descending = ALLOWLIST.parse(Some("size:desc")).unwrap();
    assert_eq!(descending.field().column(), "size_bytes");
    assert_eq!(descending.field().kind(), SortKeyKind::Integer);
    assert_eq!(descending.direction(), SortDirection::Desc);

    let default = ALLOWLIST.parse(None).unwrap();
    assert_eq!(default, ALLOWLIST.default_spec());
    assert_eq!(default.wire(), "createdAt:desc");

    for rejected in [
        "owner:asc",
        "name_sort:asc",
        "created_at:desc",
        "CreatedAt:desc",
        "name:ASC",
        "name:up",
        "name:",
        "name",
        ":asc",
        "",
        "name:asc,size:desc",
        "name:asc:desc",
        "name:asc;DROP TABLE users",
        " name:asc",
        "name:asc ",
    ] {
        assert_validation(&ALLOWLIST.parse(Some(rejected)).unwrap_err(), "sort");
    }
    let repeated = QueryParams::parse(Some("sort=name:asc&sort=size:desc"));
    assert_validation(
        &PageRequest::from_query(&repeated, &ALLOWLIST, &ring()).unwrap_err(),
        "sort",
    );

    for field in &FIELDS {
        for direction in [SortDirection::Asc, SortDirection::Desc] {
            let raw = format!("{}:{}", field.name(), direction.as_str());
            let parsed = ALLOWLIST.parse(Some(&raw)).unwrap();
            assert!(std::ptr::eq(parsed.field(), field));
            assert!(FIELDS
                .iter()
                .any(|known| known.column() == parsed.field().column()));
        }
    }

    assert_eq!(
        ALLOWLIST.values(),
        [
            "createdAt:asc",
            "createdAt:desc",
            "name:asc",
            "name:desc",
            "size:asc",
            "size:desc"
        ]
    );
    let parameter = serde_json::to_value(ALLOWLIST.parameter()).unwrap();
    assert_eq!(parameter["name"], "sort");
    assert_eq!(parameter["in"], "query");
    assert_eq!(parameter["required"], false);
    assert_eq!(parameter["schema"]["type"], "string");
    assert_eq!(
        parameter["schema"]["enum"],
        json!([
            "createdAt:asc",
            "createdAt:desc",
            "name:asc",
            "name:desc",
            "size:asc",
            "size:desc"
        ])
    );
    assert_eq!(parameter["schema"]["default"], "createdAt:desc");

    let spec = ALLOWLIST.parse(Some("name:desc")).unwrap();
    let request = PageRequest::from_query(
        &QueryParams::parse(Some("sort=name:desc&limit=3")),
        &ALLOWLIST,
        &ring(),
    )
    .unwrap();
    assert_eq!(request.sort(), &spec);
    let mut query = QueryBuilder::<Sqlite>::new("SELECT id FROM t WHERE owner_id = 1");
    request.push_order_and_limit(&mut query);
    assert_eq!(
        query.sql(),
        "SELECT id FROM t WHERE owner_id = 1 ORDER BY name_sort DESC, id DESC LIMIT ?"
    );
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct RankRow {
    id: String,
    rank: i64,
    label: String,
}

fn rank_key(row: &RankRow, field: &'static SortField) -> CursorKey {
    let value = match field.name() {
        "rank" => SortValue::Integer(row.rank),
        _ => SortValue::Text(row.label.clone()),
    };
    CursorKey::new(value, row.id.parse::<Id<Row>>().unwrap())
}

async fn insert(pools: &DbPools, clock: &TestClock, rows: Vec<RankRow>) {
    pools
        .write_tx(clock, "test.pagination_insert", async |tx| {
            for row in &rows {
                sqlx::query("INSERT INTO ranked(id, rank, label) VALUES (?, ?, ?)")
                    .bind(&row.id)
                    .bind(row.rank)
                    .bind(&row.label)
                    .execute(tx.executor())
                    .await?;
            }
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
}

async fn fetch_page(pools: &DbPools, keys: &KeyRing, raw_query: &str) -> Page<RankRow> {
    let request =
        PageRequest::from_query(&QueryParams::parse(Some(raw_query)), &RANK_ALLOWLIST, keys)
            .unwrap();
    let mut query = QueryBuilder::<Sqlite>::new("SELECT id, rank, label FROM ranked");
    request.push_keyset(&mut query, Conjunction::Where);
    request.push_order_and_limit(&mut query);
    assert!(!query.sql().to_ascii_uppercase().contains("OFFSET"));
    let rows = query
        .build_query_as::<RankRow>()
        .fetch_all(pools.reader().executor())
        .await
        .unwrap();
    request.into_page(rows, keys, rank_key, TotalCount::Uncounted)
}

fn row(clock: &TestClock, rank: i64, label: &str) -> RankRow {
    RankRow {
        id: row_id(clock).to_string(),
        rank,
        label: label.to_owned(),
    }
}

fn sort_rows(rows: &mut [RankRow], field: &str, direction: SortDirection) {
    rows.sort_by(|a, b| {
        let ordering = match field {
            "rank" => a.rank.cmp(&b.rank),
            _ => a.label.cmp(&b.label),
        }
        .then_with(|| a.id.cmp(&b.id));
        match direction {
            SortDirection::Asc => ordering,
            SortDirection::Desc => ordering.reverse(),
        }
    });
}

async fn traverse_with_inserts(
    pools: &DbPools,
    keys: &KeyRing,
    clock: &TestClock,
    sort: &str,
    original: &[RankRow],
) {
    let (field, direction) = sort.split_once(':').unwrap();
    let direction = if direction == "asc" {
        SortDirection::Asc
    } else {
        SortDirection::Desc
    };
    let mut expected = original.to_vec();
    sort_rows(&mut expected, field, direction);

    let mut returned: Vec<RankRow> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match &cursor {
            Some(cursor) => format!("sort={sort}&limit=4&cursor={cursor}"),
            None => format!("sort={sort}&limit=4"),
        };
        let page = fetch_page(pools, keys, &query).await;
        assert!(page.items.len() <= 4);
        returned.extend(page.items.iter().cloned());
        pages += 1;
        let Some(next) = page.next_cursor else {
            break;
        };
        let last = page.items.last().unwrap();
        let tag = format!("{sort}-{pages}");
        insert(
            pools,
            clock,
            vec![
                row(clock, last.rank, &format!("{}~{tag}", last.label)),
                row(clock, last.rank - 1, &format!("a-{tag}")),
                row(clock, last.rank + 1, &format!("z-{tag}")),
                row(clock, last.rank, &last.label.clone()),
                row(clock, -1_000, &format!("0-{tag}")),
                row(clock, 1_000, &format!("~-{tag}")),
            ],
        )
        .await;
        cursor = Some(next);
    }
    assert!(pages > 3, "{sort}: traversal must span several pages");

    let mut seen: HashMap<&str, usize> = HashMap::new();
    for item in &returned {
        *seen.entry(item.id.as_str()).or_default() += 1;
    }
    assert!(
        seen.values().all(|&count| count == 1),
        "{sort}: a row repeated"
    );
    for item in original {
        assert_eq!(
            seen.get(item.id.as_str()),
            Some(&1),
            "{sort}: {item:?} skipped"
        );
    }

    let survivors: Vec<&RankRow> = returned
        .iter()
        .filter(|item| original.iter().any(|seed| seed.id == item.id))
        .collect();
    assert_eq!(survivors, expected.iter().collect::<Vec<_>>(), "{sort}");

    let mut sorted = returned.clone();
    sort_rows(&mut sorted, field, direction);
    assert_eq!(returned, sorted, "{sort}: pages out of keyset order");
}

#[tokio::test]
async fn it_keyset_stable_under_inserts() {
    let root = TempDir::new().unwrap();
    let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
        .await
        .unwrap();
    let clock = TestClock::new(datetime!(2026-09-24 12:00 UTC));
    let keys = ring();

    pools
        .write_tx(&clock, "test.pagination_table", async |tx| {
            sqlx::raw_sql(
                "CREATE TABLE ranked(id TEXT NOT NULL PRIMARY KEY, rank INTEGER NOT NULL, label TEXT NOT NULL);
                 CREATE INDEX ix_ranked_rank ON ranked(rank, id);
                 CREATE INDEX ix_ranked_label ON ranked(label, id);",
            )
            .execute(tx.executor())
            .await?;
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();

    let mut seed: Vec<RankRow> = (0..24_i64)
        .map(|index| {
            row(
                &clock,
                (index * 7) % 5,
                ["delta", "alpha", "charlie"][usize::try_from(index % 3).unwrap()],
            )
        })
        .collect();
    seed.reverse();
    insert(&pools, &clock, seed.clone()).await;
    assert!(seed.iter().filter(|item| item.rank == 0).count() > 1);
    assert!(seed.iter().filter(|item| item.label == "alpha").count() > 1);

    for sort in ["rank:asc", "rank:desc", "label:asc", "label:desc"] {
        traverse_with_inserts(&pools, &keys, &clock, sort, &seed).await;
    }

    let first = fetch_page(&pools, &keys, "sort=rank:asc&limit=1").await;
    let cursor = first.next_cursor.unwrap();
    let boundary = first.items[0].clone();
    pools
        .write_tx(&clock, "test.pagination_delete", async |tx| {
            sqlx::query("DELETE FROM ranked WHERE (rank, id) > (?, ?)")
                .bind(boundary.rank)
                .bind(&boundary.id)
                .execute(tx.executor())
                .await?;
            Ok::<_, DbError>(())
        })
        .await
        .unwrap();
    let after_delete = fetch_page(
        &pools,
        &keys,
        &format!("sort=rank:asc&limit=1&cursor={cursor}"),
    )
    .await;
    assert!(after_delete.items.is_empty());
    assert_eq!(after_delete.next_cursor, None);

    pools.shutdown().await;
}

#[test]
fn unit_limit_bounds() {
    assert_eq!(Limit::parse(None).unwrap().get(), DEFAULT_LIMIT);
    assert_eq!(DEFAULT_LIMIT, 50);
    assert_eq!(MAX_LIMIT, 200);
    assert_eq!(Limit::parse(Some("1")).unwrap().get(), 1);
    assert_eq!(Limit::parse(Some("200")).unwrap().get(), 200);
    for rejected in [
        "201",
        "0",
        "",
        "-1",
        "+5",
        "5.0",
        "abc",
        "99999999999",
        " 5",
    ] {
        assert_validation(&Limit::parse(Some(rejected)).unwrap_err(), "limit");
    }
    let request = PageRequest::from_query(&QueryParams::parse(None), &ALLOWLIST, &ring()).unwrap();
    assert_eq!(request.limit().get(), 50);
    assert_eq!(request.after(), None);
    assert_validation(
        &PageRequest::from_query(&QueryParams::parse(Some("limit=201")), &ALLOWLIST, &ring())
            .unwrap_err(),
        "limit",
    );
    assert_validation(
        &PageRequest::from_query(
            &QueryParams::parse(Some("limit=5&limit=6")),
            &ALLOWLIST,
            &ring(),
        )
        .unwrap_err(),
        "limit",
    );
}

#[test]
fn unit_search_query_bounds() {
    assert_eq!(SearchQuery::parse(None).unwrap(), None);
    for accepted in [
        "ab".to_owned(),
        "é☃".to_owned(),
        "x".repeat(128),
        "日".repeat(128),
    ] {
        assert_eq!(
            SearchQuery::parse(Some(&accepted))
                .unwrap()
                .unwrap()
                .as_str(),
            accepted
        );
    }
    for rejected in [
        String::new(),
        "a".to_owned(),
        "é".to_owned(),
        "x".repeat(129),
    ] {
        assert_validation(&SearchQuery::parse(Some(&rejected)).unwrap_err(), "q");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Uploading,
    Failed,
}

fn state(raw: &str) -> Option<State> {
    match raw {
        "uploading" => Some(State::Uploading),
        "failed" => Some(State::Failed),
        _ => None,
    }
}

#[test]
fn unit_repeated_filters_stay_distinct() {
    let params = QueryParams::parse(Some("state=uploading&q=a%2Cb&state=failed&state=uploading"));
    assert_eq!(
        params.repeated("state", state).unwrap(),
        [State::Uploading, State::Failed, State::Uploading]
    );
    assert_eq!(params.single("q").unwrap(), Some("a,b"));

    let joined = QueryParams::parse(Some("state=uploading,failed&label=a,b"));
    assert_eq!(joined.all("label").collect::<Vec<_>>(), ["a,b"]);
    assert_validation(&joined.repeated("state", state).unwrap_err(), "state");

    for syntax in [
        "filter[state]=failed",
        "state[]=failed",
        "state=in=(failed)",
    ] {
        let params = QueryParams::parse(Some(syntax));
        let values = params.repeated("state", state);
        assert!(values.map_or(true, |values| values.is_empty()), "{syntax}");
    }
    assert!(QueryParams::parse(None)
        .repeated("state", state)
        .unwrap()
        .is_empty());

    let parameter =
        serde_json::to_value(repeated_enum_parameter("state", &["uploading", "failed"])).unwrap();
    assert_eq!(parameter["in"], "query");
    assert_eq!(parameter["style"], "form");
    assert_eq!(parameter["explode"], true);
    assert_eq!(parameter["schema"]["type"], "array");
    assert_eq!(
        parameter["schema"]["items"]["enum"],
        json!(["uploading", "failed"])
    );

    assert_eq!(
        serde_json::to_value(limit_parameter()).unwrap()["schema"]["maximum"],
        200
    );
    assert_eq!(
        serde_json::to_value(limit_parameter()).unwrap()["schema"]["default"],
        50
    );
    assert_eq!(
        serde_json::to_value(search_parameter()).unwrap()["schema"]["minLength"],
        2
    );
    assert_eq!(
        serde_json::to_value(search_parameter()).unwrap()["schema"]["maxLength"],
        128
    );
    assert_eq!(
        serde_json::to_value(cursor_parameter()).unwrap()["schema"]["type"],
        "string"
    );
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct ProbeItem {
    id: String,
}

#[test]
fn unit_total_count_always_serialized() {
    let counted = Page {
        items: vec![ProbeItem { id: "a".to_owned() }],
        next_cursor: None,
        total_count: TotalCount::Exact(1_284).into(),
    };
    assert_eq!(
        serde_json::to_value(&counted).unwrap(),
        json!({ "items": [{ "id": "a" }], "nextCursor": null, "totalCount": 1284 })
    );

    let uncounted: Page<ProbeItem> = Page {
        items: Vec::new(),
        next_cursor: Some("abc".to_owned()),
        total_count: TotalCount::Uncounted.into(),
    };
    assert_eq!(
        serde_json::to_value(&uncounted).unwrap(),
        json!({ "items": [], "nextCursor": "abc", "totalCount": null })
    );

    let schema = serde_json::to_value(<Page<ProbeItem> as PartialSchema>::schema()).unwrap();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    for field in ["items", "nextCursor", "totalCount"] {
        assert!(required.contains(&field), "{field} missing from {schema}");
    }
    assert!(schema["properties"]["totalCount"]["type"]
        .as_array()
        .is_some_and(|types| types.contains(&json!("null")) && types.contains(&json!("integer"))));
    assert!(schema["properties"]["nextCursor"]["type"]
        .as_array()
        .is_some_and(|types| types.contains(&json!("null")) && types.contains(&json!("string"))));
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ProbeUsage {
    used_bytes: WireBytes,
    #[schema(required = true)]
    quota_bytes: Option<WireBytes>,
    used_bytes_exact: bool,
}

fn bytes(value: i64) -> ByteSize {
    ByteSize::try_from(value).unwrap()
}

#[test]
fn unit_byte_count_wire_contract() {
    assert_eq!(MAX_WIRE_BYTES, 9_007_199_254_740_991);
    assert_eq!(MAX_WIRE_BYTES, (1_i64 << 53) - 1);

    let schema = serde_json::to_value(WireBytes::schema()).unwrap();
    assert_eq!(
        schema,
        json!({ "type": "integer", "format": "int64", "minimum": 0, "maximum": 9_007_199_254_740_991_i64 })
    );
    assert_eq!(WireBytes::name(), "ByteCount");

    let (used, exact) = WireBytes::clamped(bytes(4_831_838_208));
    assert!(exact);
    let usage = ProbeUsage {
        used_bytes: used,
        quota_bytes: WireBytes::limit(None).unwrap(),
        used_bytes_exact: exact,
    };
    assert_eq!(
        serde_json::to_value(&usage).unwrap(),
        json!({ "usedBytes": 4_831_838_208_i64, "quotaBytes": null, "usedBytesExact": true })
    );
    let text = serde_json::to_string(&usage).unwrap();
    assert!(text.contains("\"quotaBytes\":null"));
    assert!(!text.contains("\"4831838208\""));

    assert_eq!(
        WireBytes::try_from(bytes(MAX_WIRE_BYTES)).unwrap().get(),
        MAX_WIRE_BYTES
    );
    assert!(WireBytes::try_from(bytes(MAX_WIRE_BYTES + 1)).is_err());
    assert!(WireBytes::try_from(ByteSize::MAX).is_err());
    assert_eq!(WireBytes::clamped(ByteSize::MAX), (WireBytes::MAX, false));
    assert_eq!(
        WireBytes::limit(Some(ByteSize::ZERO)).unwrap(),
        Some(WireBytes::try_from(ByteSize::ZERO).unwrap())
    );
    assert!(WireBytes::limit(Some(ByteSize::MAX)).is_err());
    assert_eq!(
        serde_json::to_value(WireBytes::MAX).unwrap(),
        json!(9_007_199_254_740_991_i64)
    );

    let usage_schema = serde_json::to_value(ProbeUsage::schema()).unwrap();
    assert_eq!(
        usage_schema["properties"]["usedBytes"]["$ref"],
        "#/components/schemas/ByteCount"
    );
    let quota = usage_schema["properties"]["quotaBytes"].to_string();
    assert!(
        quota.contains("null") && quota.contains("#/components/schemas/ByteCount"),
        "{quota}"
    );
}

#[test]
fn unit_keyset_predicate_is_server_built() {
    let keys = ring();
    let clock = TestClock::new(datetime!(2026-09-24 12:00 UTC));
    let id = row_id(&clock);
    for (sort, operator, order) in [("size:asc", ">", "ASC"), ("size:desc", "<", "DESC")] {
        let key = CursorKey::new(SortValue::Integer(10), id);
        let cursor = encode_cursor(&keys, &spec(sort), &key);
        let request = PageRequest::from_query(
            &QueryParams::parse(Some(&format!("sort={sort}&cursor={cursor}&limit=10"))),
            &ALLOWLIST,
            &keys,
        )
        .unwrap();
        let mut query = QueryBuilder::<Sqlite>::new("SELECT id FROM files WHERE owner_id = ?");
        request.push_keyset(&mut query, Conjunction::And);
        request.push_order_and_limit(&mut query);
        assert_eq!(
            query.sql(),
            format!(
                "SELECT id FROM files WHERE owner_id = ? AND ((size_bytes, id) {operator} (?, ?)) \
                 ORDER BY size_bytes {order}, id {order} LIMIT ?"
            )
        );
    }

    let qualified = SortAllowlist::new(&FIELDS, 2, SortDirection::Asc).with_id_column("f.id");
    let request = PageRequest::from_query(&QueryParams::parse(None), &qualified, &keys).unwrap();
    let mut query = QueryBuilder::<Sqlite>::new("SELECT f.id FROM files f");
    request.push_keyset(&mut query, Conjunction::Where);
    request.push_order_and_limit(&mut query);
    assert_eq!(
        query.sql(),
        "SELECT f.id FROM files f ORDER BY size_bytes ASC, f.id ASC LIMIT ?"
    );

    let rows: Vec<u8> = (0..=u8::try_from(MAX_LIMIT).unwrap()).collect();
    let request =
        PageRequest::from_query(&QueryParams::parse(Some("limit=200")), &ALLOWLIST, &keys).unwrap();
    let page = request.into_page(
        rows,
        &keys,
        |_, field| {
            assert_eq!(field.name(), "createdAt");
            CursorKey::new(SortValue::Text("t".to_owned()), id)
        },
        TotalCount::Exact(201),
    );
    assert_eq!(page.items.len(), 200);
    assert!(page.next_cursor.is_some());

    let request =
        PageRequest::from_query(&QueryParams::parse(Some("limit=3")), &ALLOWLIST, &keys).unwrap();
    let page = request.into_page(
        vec![1, 2, 3],
        &keys,
        |_, _| unreachable!(),
        TotalCount::Exact(3),
    );
    assert_eq!(page.items, [1, 2, 3]);
    assert_eq!(page.next_cursor, None);
}

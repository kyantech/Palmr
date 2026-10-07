use std::collections::HashSet;

use futures_util::future::join_all;

use super::folders::{Member, FOLDERS, HOST_A, HOST_B, HOST_WORK};
use super::profile::assert_code;
use super::*;

const ENSURE_PATH: &str = "/api/v1/folders/ensure-path";
const KEY_ONE: &str = "ensure-path-key-0001-aaaaaaaaaaaa";
const KEY_TWO: &str = "ensure-path-key-0002-bbbbbbbbbbbb";
const KEY_THREE: &str = "ensure-path-key-0003-cccccccccccc";

fn keyed_request(
    path: &str,
    member: &Member,
    payload: &Value,
    headers: &[(&str, &str)],
) -> Request {
    let creds = &member.creds;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(ORIGIN, BASE_URL)
        .header(CSRF_HEADER, &creds.csrf)
        .header(CONTENT_TYPE, "application/json")
        .header(
            COOKIE,
            format!("palmr_session={}; palmr_csrf={}", creds.session, creds.csrf),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(payload.to_string())).unwrap()
}

impl Stack {
    async fn ensure(&self, member: &Member, parent: Option<&str>, segments: &Value) -> Fetched {
        let body = json!({ "parentId": parent, "segments": segments });
        self.api(Method::POST, ENSURE_PATH, member, Some(&body))
            .await
    }

    async fn ensured(&self, member: &Member, parent: Option<&str>, segments: &Value) -> Value {
        let response = self.ensure(member, parent, segments).await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        response.json()
    }

    async fn ensure_keyed(&self, member: &Member, payload: &Value, key: &str) -> Fetched {
        self.send(with_peer(
            keyed_request(ENSURE_PATH, member, payload, &[("idempotency-key", key)]),
            HOST_WORK,
        ))
        .await
    }

    async fn folder_ids_named(&self, normalized: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT id FROM folders WHERE name_normalized = ?1 ORDER BY id")
            .bind(normalized)
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn chain_to(&self, id: &str) -> Vec<String> {
        sqlx::query_scalar(
            "WITH RECURSIVE up(id, parent_id, name, level) AS (
                 SELECT id, parent_id, name, 0 FROM folders WHERE id = ?1
                 UNION ALL
                 SELECT f.id, f.parent_id, f.name, u.level + 1
                   FROM folders f JOIN up u ON f.id = u.parent_id
             )
             SELECT name FROM up ORDER BY level DESC",
        )
        .bind(id)
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn it_ensure_path_idempotent_under_retry() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let segments = json!(["Photos", "2026", "Iceland"]);
    let first = stack.ensured(&alice, None, &segments).await;
    let ids = strings(&first["folderIds"]);
    assert_eq!(ids.len(), 3);
    assert_eq!(strings(&first["created"]), ids, "all three were created");
    assert_eq!(first["leafFolderId"].as_str(), Some(ids[2].as_str()));
    assert_eq!(stack.folder_rows().await, 3);
    assert_eq!(
        stack.chain_to(&ids[2]).await,
        ["Photos", "2026", "Iceland"],
        "the chain nests in request order"
    );
    for (level, id) in ids.iter().enumerate() {
        assert_eq!(
            stack
                .scalar_i64(&format!("SELECT depth FROM folders WHERE id = '{id}'"))
                .await,
            i64::try_from(level).unwrap()
        );
    }

    let second = stack.ensured(&alice, None, &segments).await;
    assert_eq!(second["folderIds"], first["folderIds"]);
    assert_eq!(second["leafFolderId"], first["leafFolderId"]);
    assert_eq!(second["created"], json!([]));
    assert_eq!(stack.folder_rows().await, 3, "a retry creates nothing");

    let equivalent = stack
        .ensured(&alice, None, &json!(["photos", "2026", "ICELAND"]))
        .await;
    assert_eq!(equivalent["folderIds"], first["folderIds"]);
    assert_eq!(equivalent["created"], json!([]));

    let partly = stack
        .ensured(
            &alice,
            None,
            &json!(["PHOTOS", "2026", "Reykjavik", "Harbour"]),
        )
        .await;
    let partly_ids = strings(&partly["folderIds"]);
    assert_eq!(partly_ids[..2], ids[..2]);
    assert_eq!(strings(&partly["created"]), partly_ids[2..]);
    assert_eq!(stack.folder_rows().await, 5);

    let under = stack
        .ensured(&alice, Some(&ids[0]), &json!(["2026", "Iceland"]))
        .await;
    assert_eq!(under["folderIds"], json!([ids[1], ids[2]]));
    assert_eq!(under["created"], json!([]));
    let single = stack.ensured(&alice, None, &json!(["Photos"])).await;
    assert_eq!(single["folderIds"], json!([ids[0]]));
    assert_eq!(single["leafFolderId"].as_str(), Some(ids[0].as_str()));

    let names: Vec<(String,)> = sqlx::query_as("SELECT name FROM folders ORDER BY name_normalized")
        .fetch_all(stack.pools.reader().executor())
        .await
        .unwrap();
    assert!(
        names.iter().all(|name| !name.0.contains('(')),
        "no suffixed folder: {names:?}"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_ensure_path_merges_normalized_names_without_touching_the_survivor() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let existing = stack.make(&alice, "Photos", None).await;
    let existing_id = existing["id"].as_str().unwrap().to_owned();
    stack
        .edit(
            &alice,
            &existing_id,
            &json!({ "description": "family archive" }),
        )
        .await;
    let before = stack.folder_snapshot_of(&existing_id).await;

    let lower = stack.ensured(&alice, None, &json!(["photos"])).await;
    assert_eq!(lower["folderIds"], json!([existing_id]));
    assert_eq!(lower["created"], json!([]));
    let shouting = stack.ensured(&alice, None, &json!(["PHOTOS"])).await;
    assert_eq!(shouting["folderIds"], json!([existing_id]));
    assert_eq!(
        stack.folder_snapshot_of(&existing_id).await,
        before,
        "the survivor keeps its spelling, description and timestamps"
    );

    let composed = stack.ensured(&alice, None, &json!(["caf\u{e9}"])).await;
    let decomposed = stack.ensured(&alice, None, &json!(["cafe\u{301}"])).await;
    let spaced = stack.ensured(&alice, None, &json!(["CAFE\u{301}"])).await;
    assert_eq!(composed["folderIds"], decomposed["folderIds"]);
    assert_eq!(composed["folderIds"], spaced["folderIds"]);
    assert_eq!(strings(&composed["created"]).len(), 1);
    assert_eq!(decomposed["created"], json!([]));
    let stored: (String,) = sqlx::query_as("SELECT name FROM folders WHERE id = ?1")
        .bind(composed["leafFolderId"].as_str().unwrap())
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(stored.0, "caf\u{e9}", "the display name is stored NFC");
    assert_eq!(stack.folder_rows().await, 2);

    let bob = stack.member("bob", HOST_B).await;
    let foreign = stack.ensured(&bob, None, &json!(["Photos"])).await;
    assert_ne!(
        foreign["folderIds"],
        json!([existing_id]),
        "another owner's namespace is separate"
    );
    stack.stop().await;
}

impl Stack {
    async fn folder_snapshot_of(
        &self,
        id: &str,
    ) -> (String, Option<String>, Option<String>, String) {
        sqlx::query_as("SELECT name, parent_id, description, updated_at FROM folders WHERE id = ?1")
            .bind(id)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn it_ensure_path_depth_boundary_and_atomic_rollback() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let mut chain: Vec<String> = Vec::new();
    for level in 0..64_u8 {
        let above = chain.last().map(String::as_str);
        chain.push(
            stack
                .seed_folder(alice.id, above, &format!("deep-{level}"), level)
                .await,
        );
    }
    let before = stack.folder_rows().await;

    let refused = stack
        .ensure(&alice, Some(&chain[63]), &json!(["A", "B"]))
        .await;
    assert_code(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    assert_eq!(
        stack.folder_rows().await,
        before,
        "A fit at depth 64 but B did not: neither remains"
    );
    assert!(stack.folder_ids_named("a").await.is_empty());
    assert!(stack.folder_ids_named("b").await.is_empty());

    let exact = stack.ensured(&alice, Some(&chain[63]), &json!(["A"])).await;
    assert_eq!(strings(&exact["created"]).len(), 1);
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT depth FROM folders WHERE id = '{}'",
                exact["leafFolderId"].as_str().unwrap()
            ))
            .await,
        64
    );
    let beyond = stack
        .ensure(&alice, exact["leafFolderId"].as_str(), &json!(["Z"]))
        .await;
    assert_code(
        &beyond,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    let reused = stack.ensured(&alice, Some(&chain[63]), &json!(["a"])).await;
    assert_eq!(reused["created"], json!([]));

    let long = (0..32).map(|n| format!("s{n}")).collect::<Vec<_>>();
    let fits = stack.ensured(&alice, Some(&chain[32]), &json!(long)).await;
    assert_eq!(strings(&fits["created"]).len(), 32);
    let leaf = fits["leafFolderId"].as_str().unwrap();
    assert_eq!(
        stack
            .scalar_i64(&format!("SELECT depth FROM folders WHERE id = '{leaf}'"))
            .await,
        64,
        "a 32-segment chain ends exactly at depth 64"
    );
    let rows_after_fit = stack.folder_rows().await;
    let overflow = stack.ensure(&alice, Some(&chain[33]), &json!(long)).await;
    assert_code(
        &overflow,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    assert_eq!(
        stack.folder_rows().await,
        rows_after_fit,
        "31 folders created before the failure were rolled back"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_ensure_path_validates_the_whole_chain_before_writing() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let mut late = vec![json!("ok"); 20];
    late[16] = json!("..");
    let long_name = "a".repeat(256);
    let at_limit = "a".repeat(255);
    let over_total = vec![json!(at_limit.clone()); 5];
    let invalid_names: Vec<(&str, Value)> = vec![
        ("empty segment", json!(["a", "", "b"])),
        ("dot", json!(["a", "."])),
        ("dot dot", json!(["a", ".."])),
        ("late dot dot", Value::Array(late)),
        ("slash", json!(["a/b"])),
        ("backslash", json!(["a\\b"])),
        ("leading slash", json!(["/a"])),
        ("NUL", json!(["a\u{0}b"])),
        ("C0 control", json!(["a\u{1f}b"])),
        ("DEL", json!(["a\u{7f}b"])),
        ("C1 control", json!(["a\u{85}b"])),
        ("tab", json!(["a\tb"])),
        ("blank", json!(["   "])),
        ("drive letter", json!(["C:", "x"])),
        ("segment over 255 bytes", json!([long_name])),
        ("multibyte segment over 255 bytes", json!(["日".repeat(86)])),
    ];
    for (case, segments) in &invalid_names {
        let response = stack.ensure(&alice, None, segments).await;
        assert_code(&response, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
        assert_eq!(stack.folder_rows().await, 0, "{case}: rows were written");
    }

    let shape_errors: Vec<(&str, Value)> = vec![
        ("empty array", json!([])),
        ("33 segments", Value::Array(vec![json!("d"); 33])),
        ("total over 1024 bytes", Value::Array(over_total)),
    ];
    for (case, segments) in &shape_errors {
        let response = stack.ensure(&alice, None, segments).await;
        assert_code(
            &response,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            response.json()["error"]["details"]["fields"],
            json!(["segments"]),
            "{case}"
        );
        assert_eq!(stack.folder_rows().await, 0, "{case}: rows were written");
    }
    let non_string = stack.ensure(&alice, None, &json!(["a", 7])).await;
    assert_code(
        &non_string,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        non_string.json()["error"]["details"]["fields"],
        json!(["segments"])
    );
    for (case, body, fields) in [
        (
            "no segments",
            json!({ "parentId": null }),
            json!(["segments"]),
        ),
        (
            "null segments",
            json!({ "segments": null }),
            json!(["segments"]),
        ),
        (
            "string segments",
            json!({ "segments": "a/b" }),
            json!(["segments"]),
        ),
        (
            "numeric parent",
            json!({ "parentId": 4, "segments": ["a"] }),
            json!(["parentId"]),
        ),
        (
            "undeclared member",
            json!({ "segments": ["a"], "name": "x" }),
            json!(["body"]),
        ),
    ] {
        let response = stack
            .api(Method::POST, ENSURE_PATH, &alice, Some(&body))
            .await;
        assert_code(
            &response,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            response.json()["error"]["details"]["fields"],
            fields,
            "{case}"
        );
    }
    assert_eq!(stack.folder_rows().await, 0);

    let valid_limit = stack
        .ensured(
            &alice,
            None,
            &json!([
                at_limit.clone(),
                at_limit.clone(),
                at_limit.clone(),
                at_limit
            ]),
        )
        .await;
    assert_eq!(strings(&valid_limit["created"]).len(), 4);
    let max_segments = stack
        .ensured(&alice, None, &Value::Array(vec![json!("m"); 1]))
        .await;
    assert_eq!(strings(&max_segments["created"]).len(), 1);
    let thirty_two: Vec<Value> = (0..32).map(|n| json!(format!("t{n}"))).collect();
    let accepted = stack.ensured(&alice, None, &Value::Array(thirty_two)).await;
    assert_eq!(strings(&accepted["created"]).len(), 32);
    stack.stop().await;
}

#[tokio::test]
async fn it_ensure_path_parent_ownership_404() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let theirs = stack.make(&bob, "Theirs", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let phantom = stack.fresh_id();
    let before = stack.folder_rows().await;

    let foreign = stack
        .ensure(&alice, Some(&theirs), &json!(["a", "b"]))
        .await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let missing = stack
        .ensure(&alice, Some(&phantom), &json!(["a", "b"]))
        .await;
    assert_code(&missing, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    assert_eq!(
        foreign.error_without_request_id(),
        missing.error_without_request_id(),
        "a foreign parent must look exactly like a missing one"
    );
    assert!(!foreign.text().contains(&theirs));
    let malformed = stack.ensure(&alice, Some("not-an-id"), &json!(["a"])).await;
    assert_code(&malformed, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    assert_eq!(stack.folder_rows().await, before, "no rows were created");

    let own = stack.make(&alice, "Mine", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let nested = stack.ensured(&alice, Some(&own), &json!(["a", "b"])).await;
    assert_eq!(strings(&nested["created"]).len(), 2);
    stack.stop().await;
}

#[tokio::test]
async fn it_ensure_path_idempotency_key_replays_and_natural_uniqueness_holds() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let body = json!({ "parentId": null, "segments": ["Project", "src", "assets"] });
    let first = stack.ensure_keyed(&alice, &body, KEY_ONE).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text());
    assert!(first.headers.get("idempotency-replayed").is_none());
    let ids = strings(&first.json()["folderIds"]);
    assert_eq!(strings(&first.json()["created"]), ids);

    let leaf = ids[2].clone();
    let renamed = stack
        .edit(
            &alice,
            &leaf,
            &json!({ "name": "renamed-after-first-call" }),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK);
    let replay = stack.ensure_keyed(&alice, &body, KEY_ONE).await;
    assert_eq!(replay.status, StatusCode::OK, "{}", replay.text());
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    assert_eq!(replay.json(), first.json(), "the original body is replayed");
    assert_eq!(strings(&replay.json()["created"]), ids);
    assert_eq!(stack.folder_rows().await, 3);
    let leaf_name: (String,) = sqlx::query_as("SELECT name FROM folders WHERE id = ?1")
        .bind(&leaf)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(
        leaf_name.0, "renamed-after-first-call",
        "the replay did not execute the endpoint again"
    );
    let restored = stack
        .edit(&alice, &leaf, &json!({ "name": "assets" }))
        .await;
    assert_eq!(restored.status, StatusCode::OK);

    let different = json!({ "parentId": null, "segments": ["Project", "src"] });
    let conflict = stack.ensure_keyed(&alice, &different, KEY_ONE).await;
    assert_code(&conflict, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");

    let unkeyed = stack
        .api(Method::POST, ENSURE_PATH, &alice, Some(&body))
        .await;
    assert_eq!(unkeyed.status, StatusCode::OK);
    assert_eq!(
        strings(&unkeyed.json()["folderIds"])[..2],
        ids[..2],
        "without the header the existing folders are reused"
    );
    let fresh_key = stack.ensure_keyed(&alice, &different, KEY_TWO).await;
    assert_eq!(fresh_key.status, StatusCode::OK);
    assert_eq!(fresh_key.json()["folderIds"], json!([ids[0], ids[1]]));
    assert_eq!(fresh_key.json()["created"], json!([]));
    let other_key = stack.ensure_keyed(&alice, &body, KEY_THREE).await;
    assert_eq!(other_key.status, StatusCode::OK);
    assert!(other_key.headers.get("idempotency-replayed").is_none());
    assert_eq!(
        strings(&other_key.json()["folderIds"])[..2],
        ids[..2],
        "a different key with the same body resolves the same folders"
    );
    assert_eq!(
        stack.folder_rows().await,
        3,
        "no key and no repetition ever produced a second tree"
    );
    assert!(stack.folder_ids_named("project (1)").await.is_empty());

    let invalid = json!({ "parentId": null, "segments": [".."] });
    let refused = stack.ensure_keyed(&alice, &invalid, KEY_THREE).await;
    assert_code(&refused, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");
    let released = stack
        .ensure_keyed(&alice, &invalid, "ensure-path-key-0004-dddddddddddd")
        .await;
    assert_code(&released, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    let retried = stack
        .ensure_keyed(
            &alice,
            &json!({ "parentId": null, "segments": ["Recovered"] }),
            "ensure-path-key-0004-dddddddddddd",
        )
        .await;
    assert_eq!(
        retried.status,
        StatusCode::OK,
        "an error that committed nothing does not consume the key: {}",
        retried.text()
    );

    let short = stack.ensure_keyed(&alice, &body, "too-short").await;
    assert_code(&short, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");

    let stored: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT route_template, state, key_hash FROM idempotency_records ORDER BY created_at, id",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(!stored.is_empty());
    for (route, state, key_hash) in &stored {
        assert_eq!(route, ENSURE_PATH);
        assert_eq!(state, "completed");
        assert_eq!(key_hash.len(), 64);
        assert!(![KEY_ONE, KEY_TWO, KEY_THREE].contains(&key_hash.as_str()));
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_ensure_path_concurrent_in_process() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let spellings = [
        json!(["Project", "src", "assets"]),
        json!(["project", "SRC", "assets"]),
        json!(["PROJECT", "src", "Assets"]),
    ];
    let burst = join_all((0..12).map(|n| stack.ensure(&alice, None, &spellings[n % 3]))).await;
    let mut observed = HashSet::new();
    let mut created = 0;
    for response in &burst {
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        observed.insert(response.json()["folderIds"].to_string());
        created += strings(&response.json()["created"]).len();
    }
    assert_eq!(observed.len(), 1, "every racer saw the same folder ids");
    assert_eq!(created, 3, "exactly one racer created each folder");
    assert_eq!(stack.folder_rows().await, 3);
    for suffixed in ["project (1)", "src (1)", "assets (1)"] {
        assert!(
            stack.folder_ids_named(suffixed).await.is_empty(),
            "{suffixed}"
        );
    }
    stack.stop().await;
}

#[test]
fn unit_folder_move_and_ensure_path_routes_are_declared() {
    use crate::app::router::IdempotencyMode;

    let assembled = application_routes().build().unwrap();
    let declared: Vec<_> = assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| {
            matches!(
                entry.path(),
                "/api/v1/folders/{id}/move" | "/api/v1/folders/ensure-path"
            )
        })
        .map(|entry| {
            (
                entry.method().clone(),
                entry.path().to_owned(),
                entry.policy().auth(),
                entry.policy().rate_limit(),
                entry.policy().idempotency(),
                entry.policy().transport().is_byte_path(),
            )
        })
        .collect();
    let mut expected = vec![
        (
            Method::POST,
            "/api/v1/folders/{id}/move".to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Write,
            IdempotencyMode::None,
            false,
        ),
        (
            Method::POST,
            ENSURE_PATH.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Write,
            IdempotencyMode::Plaintext,
            false,
        ),
    ];
    let mut declared = declared;
    declared.sort_by_key(|entry| entry.1.clone());
    expected.sort_by_key(|entry| entry.1.clone());
    assert_eq!(declared, expected);

    let folder_surface: Vec<String> = assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with(FOLDERS))
        .map(|entry| format!("{} {}", entry.method(), entry.path()))
        .collect();
    let mut folder_surface = folder_surface;
    folder_surface.sort();
    assert_eq!(
        folder_surface,
        [
            "GET /api/v1/folders",
            "GET /api/v1/folders/tree",
            "GET /api/v1/folders/{id}",
            "PATCH /api/v1/folders/{id}",
            "POST /api/v1/folders",
            "POST /api/v1/folders/ensure-path",
            "POST /api/v1/folders/{id}/move",
        ],
        "the folder surface is exactly T02 plus T03"
    );
}

use std::collections::HashSet;

use futures_util::future::join_all;

use super::profile::{assert_code, Call};
use super::*;
use crate::app::router::RateLimitClass;
use crate::features::folders::{resolve_owned_folder, FolderError, FolderId, OwnedFolder};

pub(super) const FOLDERS: &str = "/api/v1/folders";
const TREE: &str = "/api/v1/folders/tree";
pub(super) const SEEDED_AT: &str = "2026-09-25T12:00:00.000Z";
pub(super) const HOST_A: u8 = 10;
pub(super) const HOST_B: u8 = 11;
pub(super) const HOST_WORK: u8 = 12;

pub(super) struct Member {
    pub(super) id: UserId,
    pub(super) creds: Credentials,
}

impl Stack {
    pub(super) async fn member(&self, username: &str, host: u8) -> Member {
        let hash = password_hash();
        let id = self
            .user(UserSpec::local(
                username,
                &format!("{username}@example.test"),
                &hash,
            ))
            .await;
        let creds = self.signed_in(username, host).await;
        Member { id, creds }
    }

    pub(super) async fn api(
        &self,
        method: Method,
        path: &str,
        member: &Member,
        body: Option<&Value>,
    ) -> Fetched {
        let mut call = Call::new(method, path, &member.creds);
        if let Some(body) = body {
            call = call.json(body);
        }
        self.call(call, HOST_WORK).await
    }

    pub(super) async fn read(&self, path: &str, member: &Member) -> Fetched {
        self.api(Method::GET, path, member, None).await
    }

    pub(super) async fn create(
        &self,
        member: &Member,
        name: &str,
        parent: Option<&str>,
    ) -> Fetched {
        let body = json!({ "name": name, "parentId": parent });
        self.api(Method::POST, FOLDERS, member, Some(&body)).await
    }

    pub(super) async fn make(&self, member: &Member, name: &str, parent: Option<&str>) -> Value {
        let created = self.create(member, name, parent).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        created.json()
    }

    pub(super) async fn edit(&self, member: &Member, id: &str, body: &Value) -> Fetched {
        self.api(
            Method::PATCH,
            &format!("{FOLDERS}/{id}"),
            member,
            Some(body),
        )
        .await
    }

    pub(super) fn fresh_id(&self) -> String {
        FolderId::generate(&self.clock).to_string()
    }

    pub(super) async fn seed_folder(
        &self,
        owner: UserId,
        parent: Option<&str>,
        name: &str,
        depth: u8,
    ) -> String {
        let id = self.fresh_id();
        let parent = parent.map_or_else(|| "NULL".to_owned(), |parent| format!("'{parent}'"));
        self.execute(&format!(
            "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             VALUES ('{id}', '{owner}', {parent}, '{name}', '{}', {depth}, '{SEEDED_AT}', '{SEEDED_AT}')",
            name.to_lowercase()
        ))
        .await;
        id
    }

    pub(super) async fn seed_file(
        &self,
        owner: UserId,
        folder: Option<&str>,
        name: &str,
        n: u32,
        size: i64,
    ) {
        let folder = folder.map_or_else(|| "NULL".to_owned(), |folder| format!("'{folder}'"));
        self.execute(&format!(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
             VALUES ('object-{n}', 'objects/00/00/{n:032x}', 'local', {size}, 'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}');
             INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized, size_bytes, created_at, updated_at)
             VALUES ('{}', '{owner}', {folder}, 'object-{n}', '{name}', '{}', {size}, '{SEEDED_AT}', '{SEEDED_AT}')",
            self.fresh_id(),
            name.to_lowercase()
        ))
        .await;
    }

    pub(super) async fn seed_many_roots(
        &self,
        owner: UserId,
        parent: Option<&str>,
        count: u32,
        depth: u8,
    ) {
        let tag = if parent.is_some() { "0001" } else { "0000" };
        let parent = parent.map_or_else(|| "NULL".to_owned(), |parent| format!("'{parent}'"));
        self.execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {count})
             INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             SELECT printf('0192f3a1-{tag}-7000-8000-%012x', i), '{owner}', {parent},
                    printf('f%05d', i), printf('f%05d', i), {depth}, '{SEEDED_AT}', '{SEEDED_AT}'
               FROM n"
        ))
        .await;
    }

    pub(super) async fn folder_rows(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM folders").await
    }
}

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn it_folder_ownership_404() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let owned = stack.make(&alice, "Private", None).await;
    let owned_id = owned["id"].as_str().unwrap().to_owned();
    let phantom = stack.fresh_id();

    let requests = |id: &str| -> Vec<(&'static str, Method, String, Option<Value>)> {
        vec![
            ("get", Method::GET, format!("{FOLDERS}/{id}"), None),
            (
                "patch",
                Method::PATCH,
                format!("{FOLDERS}/{id}"),
                Some(json!({ "name": "Hijacked", "description": "taken" })),
            ),
            (
                "create under",
                Method::POST,
                FOLDERS.to_owned(),
                Some(json!({ "name": "Child", "parentId": id })),
            ),
            (
                "list parent",
                Method::GET,
                format!("{FOLDERS}?parentId={id}"),
                None,
            ),
            (
                "tree root",
                Method::GET,
                format!("{TREE}?rootId={id}"),
                None,
            ),
        ]
    };
    for ((case, method, path, body), (_, _, phantom_path, phantom_body)) in
        requests(&owned_id).into_iter().zip(requests(&phantom))
    {
        let foreign = stack.api(method.clone(), &path, &bob, body.as_ref()).await;
        assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
        let missing = stack
            .api(method, &phantom_path, &bob, phantom_body.as_ref())
            .await;
        assert_code(&missing, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
        assert_eq!(
            foreign.error_without_request_id(),
            missing.error_without_request_id(),
            "{case}: a foreign folder must look exactly like a missing one"
        );
        assert!(!foreign.text().contains(&owned_id), "{case}");
    }
    let malformed = stack.read(&format!("{FOLDERS}/not-an-id"), &bob).await;
    assert_code(&malformed, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");

    let after = stack.read(&format!("{FOLDERS}/{owned_id}"), &alice).await;
    assert_eq!(after.json()["name"], "Private");
    assert_eq!(after.json()["description"], Value::Null);
    assert_eq!(
        stack.folder_rows().await,
        1,
        "no foreign request changed anything"
    );

    for (case, method, path, body) in requests(&owned_id) {
        let own = stack.api(method, &path, &alice, body.as_ref()).await;
        assert!(own.status.is_success(), "{case}: {}", own.text());
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_depth_limit() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let mut parent: Option<String> = None;
    let mut chain = Vec::new();
    for level in 0..=64 {
        let created = stack
            .make(&alice, &format!("level-{level}"), parent.as_deref())
            .await;
        let id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["parentId"], json!(parent), "level {level}");
        chain.push(id.clone());
        parent = Some(id);
    }
    assert_eq!(chain.len(), 65);
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT depth FROM folders WHERE id = '{}'",
                chain[0]
            ))
            .await,
        0,
        "a root-level folder has depth 0"
    );
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT depth FROM folders WHERE id = '{}'",
                chain[64]
            ))
            .await,
        64,
        "depth 64 is valid"
    );
    for (level, id) in chain.iter().enumerate() {
        assert_eq!(
            stack
                .scalar_i64(&format!("SELECT depth FROM folders WHERE id = '{id}'"))
                .await,
            i64::try_from(level).unwrap()
        );
    }

    let refused = stack.create(&alice, "level-65", Some(&chain[64])).await;
    assert_code(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    assert_eq!(stack.folder_rows().await, 65, "nothing was inserted");

    let leaf = stack
        .read(&format!("{FOLDERS}/{}", chain[64]), &alice)
        .await;
    assert_eq!(leaf.json()["path"].as_array().unwrap().len(), 65);
    let top = stack.read(&format!("{FOLDERS}/{}", chain[0]), &alice).await;
    assert_eq!(top.json()["subfolderCount"], 64);

    let renamed = stack
        .edit(&alice, &chain[64], &json!({ "name": "deepest" }))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    let sibling = stack.create(&alice, "level-64", Some(&chain[63])).await;
    assert_eq!(sibling.status, StatusCode::CREATED);
    assert_eq!(sibling.json()["name"], "level-64");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_recursive_totals_are_exact() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let a = stack.seed_folder(alice.id, None, "A", 0).await;
    let b = stack.seed_folder(alice.id, Some(&a), "B", 1).await;
    stack.seed_file(alice.id, Some(&b), "x.bin", 1, 10).await;
    stack.seed_file(alice.id, Some(&a), "y.bin", 2, 20).await;
    let c = stack.seed_folder(alice.id, None, "C", 0).await;
    let d = stack.seed_folder(alice.id, Some(&c), "D", 1).await;
    let f = stack.seed_folder(alice.id, Some(&d), "F", 2).await;
    stack.seed_file(alice.id, Some(&f), "z.bin", 3, 5).await;
    stack.seed_file(alice.id, Some(&c), "w.bin", 4, 7).await;
    stack.seed_folder(alice.id, None, "E", 0).await;
    stack.seed_file(alice.id, None, "loose.bin", 5, 999).await;
    let mut expected_pad_bytes = 0;
    for pad in 0..40_u32 {
        let id = stack
            .seed_folder(alice.id, None, &format!("pad-{pad:02}"), 0)
            .await;
        let size = i64::from(pad) + 1;
        expected_pad_bytes += size;
        stack
            .seed_file(
                alice.id,
                Some(&id),
                &format!("pad-{pad}.bin"),
                100 + pad,
                size,
            )
            .await;
    }

    let others = stack.seed_folder(bob.id, None, "A", 0).await;
    let others_child = stack.seed_folder(bob.id, Some(&others), "Z", 1).await;
    stack
        .seed_file(bob.id, Some(&others_child), "big.bin", 500, 1_000_000)
        .await;
    stack
        .seed_file(bob.id, Some(&others), "big2.bin", 501, 2_000_000)
        .await;

    let warm = stack.read(&format!("{FOLDERS}?limit=3"), &alice).await;
    assert_eq!(warm.status, StatusCode::OK, "{}", warm.text());
    let page = warm.json();
    assert_eq!(names(&page), ["A", "C", "E"]);
    let items = page["items"].as_array().unwrap();
    let totals = |index: usize| {
        (
            items[index]["fileCount"].as_u64().unwrap(),
            items[index]["subfolderCount"].as_u64().unwrap(),
            items[index]["totalBytes"].as_u64().unwrap(),
        )
    };
    assert_eq!(totals(0), (2, 1, 30), "A: x.bin, y.bin and B");
    assert_eq!(totals(1), (2, 2, 12), "C: w.bin, z.bin, D and F");
    assert_eq!(totals(2), (0, 0, 0), "E is empty");
    assert_eq!(page["totalCount"], 43);

    let small = stack.read(&format!("{FOLDERS}?limit=2"), &alice).await;
    assert_eq!(small.status, StatusCode::OK);
    assert_eq!(small.json()["items"].as_array().unwrap().len(), 2);
    let large = stack.read(&format!("{FOLDERS}?limit=200"), &alice).await;
    assert_eq!(large.status, StatusCode::OK);
    let large_page = large.json();
    assert_eq!(large_page["items"].as_array().unwrap().len(), 43);
    assert_eq!(large_page["nextCursor"], Value::Null);

    let sum = |page: &Value, field: &str| -> u64 {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item[field].as_u64().unwrap())
            .sum()
    };
    assert_eq!(sum(&large_page, "fileCount"), 4 + 40);
    assert_eq!(sum(&large_page, "subfolderCount"), 3);
    assert_eq!(
        sum(&large_page, "totalBytes"),
        30 + 12 + u64::try_from(expected_pad_bytes).unwrap()
    );

    let children = stack.read(&format!("{FOLDERS}?parentId={c}"), &alice).await;
    assert_eq!(names(&children.json()), ["D"]);
    assert_eq!(children.json()["items"][0]["fileCount"], 1);
    assert_eq!(children.json()["items"][0]["subfolderCount"], 1);
    assert_eq!(children.json()["items"][0]["totalBytes"], 5);

    let detail = stack.read(&format!("{FOLDERS}/{a}"), &alice).await.json();
    assert_eq!(detail["fileCount"], 2);
    assert_eq!(detail["subfolderCount"], 1);
    assert_eq!(detail["totalBytes"], 30);
    let bobs = stack
        .read(&format!("{FOLDERS}/{others}"), &bob)
        .await
        .json();
    assert_eq!(bobs["fileCount"], 2);
    assert_eq!(bobs["totalBytes"], 3_000_000);
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_create_keep_both() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let mut created = Vec::new();
    for _ in 0..3 {
        created.push(
            stack.make(&alice, "Docs", None).await["name"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(created, ["Docs", "Docs (1)", "Docs (2)"]);

    let lowered = stack.make(&alice, "docs", None).await;
    assert_eq!(
        lowered["name"], "docs (3)",
        "normalized collision keeps the requested spelling and takes the next free number"
    );
    let shouting = stack.make(&alice, "DOCS (1)", None).await;
    assert_eq!(shouting["name"], "DOCS (1) (1)");

    let foreign = stack.make(&bob, "Docs", None).await;
    assert_eq!(
        foreign["name"], "Docs",
        "another owner's namespace is separate"
    );

    let parent = stack.make(&alice, "Parent", None).await;
    let parent_id = parent["id"].as_str().unwrap();
    let mut nested = Vec::new();
    for _ in 0..3 {
        nested.push(
            stack.make(&alice, "Docs", Some(parent_id)).await["name"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(nested, ["Docs", "Docs (1)", "Docs (2)"]);

    stack.seed_file(alice.id, None, "Reports", 1, 1).await;
    let coexisting = stack.make(&alice, "Reports", None).await;
    assert_eq!(
        coexisting["name"], "Reports",
        "files and folders are separate namespaces"
    );

    let racers = 16;
    let burst: Vec<Fetched> =
        join_all((0..racers).map(|_| stack.create(&alice, "Race", None))).await;
    let mut raced = HashSet::new();
    for response in &burst {
        assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
        assert!(raced.insert(response.json()["name"].as_str().unwrap().to_owned()));
    }
    let expected: HashSet<String> = std::iter::once("Race".to_owned())
        .chain((1..racers).map(|n| format!("Race ({n})")))
        .collect();
    assert_eq!(raced, expected);
    let normalized: Vec<(String,)> = sqlx::query_as(
        "SELECT name_normalized FROM folders WHERE owner_id = ?1 AND parent_id IS NULL
           AND name_normalized LIKE 'race%'",
    )
    .bind(alice.id.to_string())
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    let distinct: HashSet<&String> = normalized.iter().map(|(name,)| name).collect();
    assert_eq!(normalized.len(), 16);
    assert_eq!(distinct.len(), 16);
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_name_exhaustion_is_a_conflict() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack
        .execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 1000)
             INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             SELECT printf('0192f3a1-0002-7000-8000-%012x', i), '{}', NULL,
                    CASE WHEN i = 0 THEN 'crowded' ELSE 'crowded (' || i || ')' END,
                    CASE WHEN i = 0 THEN 'crowded' ELSE 'crowded (' || i || ')' END,
                    0, '{SEEDED_AT}', '{SEEDED_AT}'
               FROM n",
            alice.id
        ))
        .await;
    let other = stack.make(&alice, "other", None).await;
    let rows = stack.folder_rows().await;

    let created = stack.create(&alice, "crowded", None).await;
    assert_code(&created, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    let renamed = stack
        .edit(
            &alice,
            other["id"].as_str().unwrap(),
            &json!({ "name": "crowded" }),
        )
        .await;
    assert_code(&renamed, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");

    assert_eq!(stack.folder_rows().await, rows, "nothing was inserted");
    let unchanged = stack
        .read(
            &format!("{FOLDERS}/{}", other["id"].as_str().unwrap()),
            &alice,
        )
        .await;
    assert_eq!(unchanged.json()["name"], "other");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_create_validates_input() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let too_long = "x".repeat(256);
    let too_long_multibyte = "é".repeat(128);
    for name in [
        "",
        "   ",
        ".",
        "..",
        "a/b",
        "a\\b",
        "tab\there",
        "nul\u{0}byte",
        too_long.as_str(),
        too_long_multibyte.as_str(),
    ] {
        let refused = stack.create(&alice, name, None).await;
        assert_code(&refused, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    }
    let longest = "é".repeat(127);
    assert_eq!(
        stack.create(&alice, &longest, None).await.status,
        StatusCode::CREATED
    );
    assert_eq!(stack.folder_rows().await, 1);

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("missing name", json!({}), vec!["name"]),
        ("null name", json!({ "name": null }), vec!["name"]),
        ("numeric name", json!({ "name": 5 }), vec!["name"]),
        (
            "undeclared member",
            json!({ "name": "ok", "ownerId": "x" }),
            vec!["body"],
        ),
        (
            "client depth",
            json!({ "name": "ok", "depth": 3 }),
            vec!["body"],
        ),
        (
            "long description",
            json!({ "name": "ok", "description": "d".repeat(2001) }),
            vec!["description"],
        ),
    ];
    for (case, body, fields) in cases {
        let refused = stack.api(Method::POST, FOLDERS, &alice, Some(&body)).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            json!(fields),
            "{case}"
        );
    }
    let boundary = stack
        .api(
            Method::POST,
            FOLDERS,
            &alice,
            Some(&json!({ "name": "boundary", "description": "d".repeat(2000) })),
        )
        .await;
    assert_eq!(boundary.status, StatusCode::CREATED, "{}", boundary.text());

    let malformed_parent = stack.create(&alice, "orphan", Some("nope")).await;
    assert_code(&malformed_parent, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let absent_parent = stack
        .create(&alice, "orphan", Some(&stack.fresh_id()))
        .await;
    assert_code(&absent_parent, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");

    let created = stack
        .api(
            Method::POST,
            FOLDERS,
            &alice,
            Some(&json!({ "name": "Shape", "description": "kept", "parentId": null })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let mut keys: Vec<String> = created
        .json()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "createdAt",
            "description",
            "fileCount",
            "id",
            "name",
            "parentId",
            "subfolderCount",
            "totalBytes",
            "updatedAt"
        ]
    );
    assert_eq!(created.json()["description"], "kept");
    assert_eq!(created.json()["parentId"], Value::Null);
    assert_eq!(created.json()["createdAt"], created.json()["updatedAt"]);

    let anonymous = stack.get(FOLDERS, None, 40).await;
    assert_code(&anonymous, StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_patch_semantics() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let created = stack
        .api(
            Method::POST,
            FOLDERS,
            &alice,
            Some(&json!({ "name": "Archive", "description": "old stuff" })),
        )
        .await
        .json();
    let id = created["id"].as_str().unwrap().to_owned();
    let docs = stack.make(&alice, "Docs", None).await;
    clock.advance(Duration::from_secs(5));

    let empty = stack.edit(&alice, &id, &json!({})).await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.text());
    assert_eq!(empty.json()["name"], "Archive");
    assert_eq!(empty.json()["description"], "old stuff");
    assert_eq!(empty.json()["updatedAt"], created["updatedAt"]);

    let name_only = stack.edit(&alice, &id, &json!({ "name": "Renamed" })).await;
    assert_eq!(name_only.json()["name"], "Renamed");
    assert_eq!(
        name_only.json()["description"],
        "old stuff",
        "an absent description is unchanged"
    );
    assert_ne!(name_only.json()["updatedAt"], created["updatedAt"]);

    clock.advance(Duration::from_secs(5));
    let description_only = stack
        .edit(&alice, &id, &json!({ "description": "new words" }))
        .await;
    assert_eq!(description_only.json()["name"], "Renamed");
    assert_eq!(description_only.json()["description"], "new words");

    let cleared = stack
        .edit(&alice, &id, &json!({ "description": null }))
        .await;
    assert_eq!(cleared.json()["description"], Value::Null);
    assert_eq!(cleared.json()["name"], "Renamed");
    let stored: Option<String> =
        sqlx::query_scalar("SELECT description FROM folders WHERE id = ?1")
            .bind(&id)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(stored, None);

    let name_null = stack.edit(&alice, &id, &json!({ "name": null })).await;
    assert_code(
        &name_null,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        name_null.json()["error"]["details"]["fields"],
        json!(["name"])
    );
    let long = stack
        .edit(&alice, &id, &json!({ "description": "d".repeat(2001) }))
        .await;
    assert_code(&long, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(
        long.json()["error"]["details"]["fields"],
        json!(["description"])
    );
    let boundary = stack
        .edit(&alice, &id, &json!({ "description": "d".repeat(2000) }))
        .await;
    assert_eq!(boundary.status, StatusCode::OK);
    let unknown = stack.edit(&alice, &id, &json!({ "parentId": null })).await;
    assert_code(
        &unknown,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        unknown.json()["error"]["details"]["fields"],
        json!(["body"])
    );
    for name in ["", ".", "..", "a/b", "ctl\u{7}"] {
        let refused = stack.edit(&alice, &id, &json!({ "name": name })).await;
        assert_code(&refused, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    }
    assert_eq!(
        stack.read(&format!("{FOLDERS}/{id}"), &alice).await.json()["name"],
        "Renamed"
    );

    let collided = stack.edit(&alice, &id, &json!({ "name": "Docs" })).await;
    assert_eq!(collided.status, StatusCode::OK, "{}", collided.text());
    assert_eq!(collided.json()["name"], "Docs (1)");
    assert_eq!(
        stack
            .read(
                &format!("{FOLDERS}/{}", docs["id"].as_str().unwrap()),
                &alice
            )
            .await
            .json()["name"],
        "Docs",
        "the existing folder is never overwritten"
    );
    let collided_again = stack.edit(&alice, &id, &json!({ "name": "docs" })).await;
    assert_eq!(
        collided_again.json()["name"],
        "docs (1)",
        "the folder does not collide with its own current normalized name"
    );
    let nothing_changed = stack
        .edit(&alice, &id, &json!({ "name": "docs (1)" }))
        .await;
    let before = nothing_changed.json()["updatedAt"].clone();

    clock.advance(Duration::from_secs(30));
    let same = stack
        .edit(&alice, &id, &json!({ "name": "docs (1)" }))
        .await;
    assert_eq!(same.json()["name"], "docs (1)");
    assert_eq!(
        same.json()["updatedAt"],
        before,
        "renaming to the exact current name writes nothing"
    );

    let case_only = stack
        .edit(
            &alice,
            docs["id"].as_str().unwrap(),
            &json!({ "name": "DOCS" }),
        )
        .await;
    assert_eq!(case_only.json()["name"], "DOCS");
    let case_back = stack
        .edit(
            &alice,
            docs["id"].as_str().unwrap(),
            &json!({ "name": "Docs" }),
        )
        .await;
    assert_eq!(case_back.json()["name"], "Docs");

    let foreign = stack.edit(&bob, &id, &json!({ "name": "Mine" })).await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, name_normalized FROM folders WHERE owner_id = ?1 ORDER BY name_normalized",
    )
    .bind(alice.id.to_string())
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            ("Docs".to_owned(), "docs".to_owned()),
            ("docs (1)".to_owned(), "docs (1)".to_owned()),
        ]
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_breadcrumbs_run_root_to_leaf() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let top = stack.make(&alice, "Root", None).await;
    let a = stack.make(&alice, "A", top["id"].as_str()).await;
    let b = stack.make(&alice, "B", a["id"].as_str()).await;
    let c = stack.make(&alice, "C", b["id"].as_str()).await;
    stack.make(&bob, "Root", None).await;
    let c_id = c["id"].as_str().unwrap();

    let detail = stack.read(&format!("{FOLDERS}/{c_id}"), &alice).await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    let body = detail.json();
    let path: Vec<(&str, &str)> = body["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|crumb| {
            (
                crumb["id"].as_str().unwrap(),
                crumb["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        path,
        [
            (top["id"].as_str().unwrap(), "Root"),
            (a["id"].as_str().unwrap(), "A"),
            (b["id"].as_str().unwrap(), "B"),
            (c_id, "C"),
        ]
    );
    assert_eq!(body["name"], "C");
    assert_eq!(body["parentId"], b["id"]);
    for crumb in body["path"].as_array().unwrap() {
        let mut keys: Vec<&String> = crumb.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["id", "name"]);
    }
    let topmost = stack
        .read(
            &format!("{FOLDERS}/{}", top["id"].as_str().unwrap()),
            &alice,
        )
        .await
        .json();
    assert_eq!(topmost["path"].as_array().unwrap().len(), 1);
    assert_eq!(topmost["subfolderCount"], 3);

    let foreign = stack.read(&format!("{FOLDERS}/{c_id}"), &bob).await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_tree_bounds_depth_ownership_and_nodes() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let mut parent: Option<String> = None;
    let mut chain = Vec::new();
    for level in 1..=5 {
        let id = stack
            .seed_folder(
                alice.id,
                parent.as_deref(),
                &format!("L{level}"),
                u8::try_from(level - 1).unwrap(),
            )
            .await;
        chain.push(id.clone());
        parent = Some(id);
    }
    let side = stack.seed_folder(alice.id, None, "Aside", 0).await;
    stack.seed_folder(bob.id, None, "Bobs", 0).await;

    let default = stack.read(TREE, &alice).await;
    assert_eq!(default.status, StatusCode::OK, "{}", default.text());
    let default = default.json();
    let listed: Vec<&str> = default["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["Aside", "L1", "L2", "L3"]);
    assert_eq!(default["truncated"], false);
    assert_eq!(default["truncationPoint"], Value::Null);
    let node = |tree: &Value, name: &str| -> Value {
        tree["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(node(&default, "L1")["parentId"], Value::Null);
    assert_eq!(node(&default, "L2")["parentId"], chain[0]);
    assert_eq!(
        node(&default, "L3")["hasChildren"],
        true,
        "L4 exists but is below the depth"
    );
    assert_eq!(node(&default, "Aside")["hasChildren"], false);
    assert!(!default.to_string().contains("Bobs"));

    let shallow = stack.read(&format!("{TREE}?depth=1"), &alice).await.json();
    assert_eq!(shallow["nodes"].as_array().unwrap().len(), 2);
    let deep = stack.read(&format!("{TREE}?depth=8"), &alice).await.json();
    assert_eq!(deep["nodes"].as_array().unwrap().len(), 6);
    assert_eq!(node(&deep, "L5")["hasChildren"], false);

    let rooted = stack
        .read(&format!("{TREE}?rootId={}&depth=2", chain[1]), &alice)
        .await
        .json();
    let rooted_names: Vec<&str> = rooted["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(rooted_names, ["L2", "L3"], "the root is level 1");
    assert_eq!(rooted["nodes"][0]["parentId"], chain[0]);

    for depth in ["0", "9", "-1", "x", ""] {
        let refused = stack.read(&format!("{TREE}?depth={depth}"), &alice).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            json!(["depth"]),
            "{depth:?}"
        );
    }
    let foreign = stack
        .read(&format!("{TREE}?rootId={}", chain[0]), &bob)
        .await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let missing = stack
        .read(&format!("{TREE}?rootId={}", stack.fresh_id()), &alice)
        .await;
    assert_code(&missing, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let leaf_tree = stack
        .read(&format!("{TREE}?rootId={side}"), &alice)
        .await
        .json();
    assert_eq!(leaf_tree["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(leaf_tree["truncated"], false);

    let again = stack.read(TREE, &alice).await.json();
    assert_eq!(again, default, "the order is deterministic");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_tree_never_exceeds_the_node_cap() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    stack.seed_many_roots(alice.id, None, 2005, 0).await;
    stack.seed_folder(bob.id, None, "Bobs", 0).await;

    let capped = stack.read(&format!("{TREE}?depth=8"), &alice).await;
    assert_eq!(capped.status, StatusCode::OK, "{}", capped.text());
    let body = capped.json();
    let nodes = body["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2000);
    assert_eq!(body["truncated"], true);
    assert_eq!(body["truncationPoint"]["afterId"], nodes[1999]["id"]);
    assert_eq!(body["truncationPoint"]["level"], 1);
    let listed: Vec<&str> = nodes
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    assert_eq!(listed, sorted, "a deterministic prefix of the full order");
    assert_eq!(listed[0], "f00001");
    assert_eq!(listed[1999], "f02000");
    assert!(!body.to_string().contains("Bobs"));

    let parent = stack.seed_folder(alice.id, None, "Holder", 0).await;
    stack
        .seed_many_roots(alice.id, Some(&parent), 2000, 1)
        .await;
    let rooted = stack
        .read(&format!("{TREE}?rootId={parent}&depth=2"), &alice)
        .await
        .json();
    assert_eq!(rooted["nodes"].as_array().unwrap().len(), 2000);
    assert_eq!(rooted["nodes"][0]["name"], "Holder");
    assert_eq!(rooted["truncated"], true);
    assert_eq!(rooted["truncationPoint"]["level"], 2);

    stack
        .execute(&format!(
            "DELETE FROM folders WHERE parent_id = '{parent}' AND name_normalized > 'f01998'"
        ))
        .await;
    let exact = stack
        .read(&format!("{TREE}?rootId={parent}&depth=2"), &alice)
        .await
        .json();
    assert_eq!(exact["nodes"].as_array().unwrap().len(), 1999);
    assert_eq!(exact["truncated"], false);
    assert_eq!(exact["truncationPoint"], Value::Null);
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_list_paginates_with_signed_cursors() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let mut created = Vec::new();
    for name in [
        "gamma", "Alpha", "echo", "Bravo", "delta", "Foxtrot", "charlie",
    ] {
        created.push(stack.make(&alice, name, None).await);
        clock.advance(Duration::from_secs(1));
    }
    let expected_names = [
        "Alpha", "Bravo", "charlie", "delta", "echo", "Foxtrot", "gamma",
    ];
    let expected_ids: Vec<String> = expected_names
        .iter()
        .map(|name| {
            created
                .iter()
                .find(|folder| folder["name"] == *name)
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let parent = created[0]["id"].as_str().unwrap().to_owned();
    stack.make(&alice, "Child-one", Some(&parent)).await;
    stack.make(&alice, "Child-two", Some(&parent)).await;
    stack.make(&bob, "Alpha", None).await;

    let walk = |query: &'static str| {
        let stack = &stack;
        let alice = &alice;
        async move {
            let mut collected = Vec::new();
            let mut pages = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let path = match &cursor {
                    Some(cursor) => format!("{FOLDERS}?{query}&cursor={cursor}"),
                    None => format!("{FOLDERS}?{query}"),
                };
                let fetched = stack.read(&path, alice).await;
                assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
                let page = fetched.json();
                collected.extend(ids(&page));
                cursor = page["nextCursor"].as_str().map(str::to_owned);
                pages.push(page);
                if cursor.is_none() {
                    return (collected, pages);
                }
                assert!(pages.len() < 20);
            }
        }
    };

    let (default_order, default_pages) = walk("limit=3").await;
    assert_eq!(default_order, expected_ids, "default sort is name:asc");
    assert_eq!(default_pages.len(), 3);
    for page in &default_pages {
        assert_eq!(page["totalCount"], 7);
    }
    assert_eq!(default_pages[2]["nextCursor"], Value::Null);
    assert_eq!(default_pages[0]["items"].as_array().unwrap().len(), 3);
    assert_eq!(default_pages[2]["items"].as_array().unwrap().len(), 1);

    let (explicit, _) = walk("limit=2&sort=name:asc").await;
    assert_eq!(explicit, expected_ids);
    let (descending, _) = walk("limit=4&sort=name:desc").await;
    let mut reversed = expected_ids.clone();
    reversed.reverse();
    assert_eq!(descending, reversed);

    let (newest_first, _) = walk("limit=3&sort=createdAt:desc").await;
    let by_creation: Vec<String> = created
        .iter()
        .rev()
        .map(|folder| folder["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(newest_first, by_creation);
    let (oldest_updated, _) = walk("limit=5&sort=updatedAt:asc").await;
    let mut by_creation_asc = by_creation.clone();
    by_creation_asc.reverse();
    assert_eq!(oldest_updated, by_creation_asc);

    let exact = stack
        .read(&format!("{FOLDERS}?limit=7"), &alice)
        .await
        .json();
    assert_eq!(exact["nextCursor"], Value::Null);
    assert_eq!(ids(&exact), expected_ids);

    let first = stack
        .read(&format!("{FOLDERS}?limit=2"), &alice)
        .await
        .json();
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let changed = stack
        .read(
            &format!("{FOLDERS}?limit=2&sort=createdAt:asc&cursor={cursor}"),
            &alice,
        )
        .await;
    assert_code(&changed, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let reversed_direction = stack
        .read(
            &format!("{FOLDERS}?limit=2&sort=name:desc&cursor={cursor}"),
            &alice,
        )
        .await;
    assert_code(
        &reversed_direction,
        StatusCode::BAD_REQUEST,
        "CURSOR_INVALID",
    );
    let mut tampered = cursor.clone();
    tampered.replace_range(4..5, if &cursor[4..5] == "A" { "B" } else { "A" });
    let forged = stack
        .read(&format!("{FOLDERS}?limit=2&cursor={tampered}"), &alice)
        .await;
    assert_code(&forged, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let garbage = stack.read(&format!("{FOLDERS}?cursor=%%%"), &alice).await;
    assert_eq!(garbage.status, StatusCode::BAD_REQUEST);

    let under = stack
        .read(&format!("{FOLDERS}?parentId={parent}&limit=1"), &alice)
        .await
        .json();
    assert_eq!(under["totalCount"], 2);
    assert_eq!(names(&under), ["Child-one"]);
    let under_next = stack
        .read(
            &format!(
                "{FOLDERS}?parentId={parent}&limit=1&cursor={}",
                under["nextCursor"].as_str().unwrap()
            ),
            &alice,
        )
        .await
        .json();
    assert_eq!(names(&under_next), ["Child-two"]);
    assert_eq!(under_next["nextCursor"], Value::Null);

    for (query, field) in [
        ("sort=size:asc", "sort"),
        ("sort=name", "sort"),
        ("sort=name:sideways", "sort"),
        ("sort=name;drop:asc", "sort"),
        ("limit=0", "limit"),
        ("limit=201", "limit"),
        ("limit=abc", "limit"),
    ] {
        let refused = stack.read(&format!("{FOLDERS}?{query}"), &alice).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            json!([field]),
            "{query}"
        );
    }

    let foreign_parent = stack
        .read(&format!("{FOLDERS}?parentId={parent}"), &bob)
        .await;
    assert_code(&foreign_parent, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    assert!(!foreign_parent.text().contains("Child-one"));
    let bobs = stack.read(FOLDERS, &bob).await.json();
    assert_eq!(names(&bobs), ["Alpha"]);
    assert_eq!(bobs["totalCount"], 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_q_filters_direct_children_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let reports = stack.make(&alice, "Reports", None).await;
    let reports_id = reports["id"].as_str().unwrap().to_owned();
    stack
        .make(&alice, "Quarterly report", Some(&reports_id))
        .await;
    stack.make(&alice, "Budget", Some(&reports_id)).await;
    stack.make(&alice, "Annual REPORT", None).await;
    stack.make(&alice, "Photos", None).await;
    let nested = stack.make(&alice, "Archive", None).await;
    stack
        .make(&alice, "report-deep", nested["id"].as_str())
        .await;
    stack.make(&bob, "Bob report", None).await;

    let root_hits = stack
        .read(&format!("{FOLDERS}?q=report"), &alice)
        .await
        .json();
    assert_eq!(names(&root_hits), ["Annual REPORT", "Reports"]);
    assert_eq!(root_hits["totalCount"], 2, "totalCount reflects the filter");

    let child_hits = stack
        .read(&format!("{FOLDERS}?q=REPORT&parentId={reports_id}"), &alice)
        .await
        .json();
    assert_eq!(names(&child_hits), ["Quarterly report"]);
    assert_eq!(child_hits["totalCount"], 1);

    let none = stack.read(&format!("{FOLDERS}?q=zzz"), &alice).await.json();
    assert!(names(&none).is_empty());
    assert_eq!(none["totalCount"], 0);
    assert_eq!(none["nextCursor"], Value::Null);

    let descendant = stack
        .read(&format!("{FOLDERS}?q=deep"), &alice)
        .await
        .json();
    assert!(
        names(&descendant).is_empty(),
        "a descendant outside the selected parent is never returned"
    );
    let bobs = stack
        .read(&format!("{FOLDERS}?q=report"), &bob)
        .await
        .json();
    assert_eq!(names(&bobs), ["Bob report"]);
    assert!(!stack
        .read(&format!("{FOLDERS}?q=bob"), &alice)
        .await
        .text()
        .contains("Bob report"));

    let paged = stack
        .read(&format!("{FOLDERS}?q=report&limit=1"), &alice)
        .await
        .json();
    assert_eq!(names(&paged), ["Annual REPORT"]);
    let next = stack
        .read(
            &format!(
                "{FOLDERS}?q=report&limit=1&cursor={}",
                paged["nextCursor"].as_str().unwrap()
            ),
            &alice,
        )
        .await
        .json();
    assert_eq!(names(&next), ["Reports"]);
    assert_eq!(next["nextCursor"], Value::Null);

    let too_long = format!("q={}", "x".repeat(129));
    for query in ["q=a", "q=", "q=%20%20", too_long.as_str()] {
        let refused = stack.read(&format!("{FOLDERS}?{query}"), &alice).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            json!(["q"]),
            "{query}"
        );
    }
    let longest = "x".repeat(128);
    let accepted = stack.read(&format!("{FOLDERS}?q={longest}"), &alice).await;
    assert_eq!(accepted.status, StatusCode::OK);
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_owned_folder_seam_is_owner_scoped() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let top = stack.make(&alice, "Top", None).await;
    let child = stack.make(&alice, "Child", top["id"].as_str()).await;
    let top_id: FolderId = top["id"].as_str().unwrap().parse().unwrap();
    let child_id: FolderId = child["id"].as_str().unwrap().parse().unwrap();
    let reader = stack.pools.reader().executor();

    let resolved: OwnedFolder = resolve_owned_folder(reader, alice.id, child_id)
        .await
        .unwrap();
    assert_eq!(resolved.id, child_id);
    assert_eq!(resolved.parent_id, Some(top_id));
    assert_eq!(resolved.depth, 1);
    assert_eq!(
        resolve_owned_folder(reader, alice.id, top_id)
            .await
            .unwrap()
            .depth,
        0
    );

    let foreign = resolve_owned_folder(reader, bob.id, child_id).await;
    assert!(matches!(foreign, Err(FolderError::NotFound)), "{foreign:?}");
    let absent = resolve_owned_folder(reader, alice.id, FolderId::generate(&stack.clock)).await;
    assert!(matches!(absent, Err(FolderError::NotFound)), "{absent:?}");
    stack.stop().await;
}

#[test]
fn unit_folder_routes_are_declared_with_their_classes() {
    let assembled = application_routes().build().unwrap();
    let declared: Vec<(Method, String, AuthClass, RateLimitClass)> = assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with(FOLDERS))
        .map(|entry| {
            (
                entry.method().clone(),
                entry.path().to_owned(),
                entry.policy().auth(),
                entry.policy().rate_limit(),
            )
        })
        .collect();
    let mut expected = vec![
        (
            Method::GET,
            FOLDERS.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
        (
            Method::GET,
            TREE.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
        (
            Method::POST,
            FOLDERS.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
        (
            Method::GET,
            format!("{FOLDERS}/{{id}}"),
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
        (
            Method::PATCH,
            format!("{FOLDERS}/{{id}}"),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
        (
            Method::POST,
            format!("{FOLDERS}/{{id}}/move"),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
        (
            Method::POST,
            format!("{FOLDERS}/ensure-path"),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
    ];
    let order = |entry: &(Method, String, AuthClass, RateLimitClass)| {
        (entry.1.clone(), entry.0.to_string())
    };
    expected.sort_by_key(order);
    let mut declared = declared;
    declared.sort_by_key(order);
    assert_eq!(declared, expected);
}

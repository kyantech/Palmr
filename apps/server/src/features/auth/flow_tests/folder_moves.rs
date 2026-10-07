use std::collections::HashMap;

use futures_util::future::join_all;

use super::folders::{Member, FOLDERS, HOST_A, HOST_B, HOST_WORK, SEEDED_AT};
use super::profile::{assert_code, Call};
use super::*;

type Row = (String, Option<String>, String, String, i64, String);

impl Stack {
    async fn folder_snapshot(&self) -> Vec<Row> {
        sqlx::query_as(
            "SELECT id, parent_id, name, name_normalized, depth, updated_at
               FROM folders ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn move_to(&self, member: &Member, id: &str, parent: Option<&str>) -> Fetched {
        let body = json!({ "parentId": parent });
        self.api(
            Method::POST,
            &format!("{FOLDERS}/{id}/move"),
            member,
            Some(&body),
        )
        .await
    }

    async fn moved(&self, member: &Member, id: &str, parent: Option<&str>) -> Value {
        let response = self.move_to(member, id, parent).await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        response.json()
    }

    async fn depth_of(&self, id: &str) -> i64 {
        self.scalar_i64(&format!("SELECT depth FROM folders WHERE id = '{id}'"))
            .await
    }

    async fn seed_chain(
        &self,
        owner: UserId,
        parent: Option<&str>,
        first_depth: u8,
        prefix: &str,
        count: u8,
    ) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for level in 0..count {
            let above = ids.last().map(String::as_str).or(parent);
            ids.push(
                self.seed_folder(
                    owner,
                    above,
                    &format!("{prefix}{level}"),
                    first_depth + level,
                )
                .await,
            );
        }
        ids
    }

    async fn assert_forest_is_sound(&self) {
        let rows = self.folder_snapshot().await;
        let parents: HashMap<&str, (Option<&str>, i64)> = rows
            .iter()
            .map(|row| (row.0.as_str(), (row.1.as_deref(), row.4)))
            .collect();
        for (id, (_, depth)) in &parents {
            let mut cursor = *id;
            let mut hops = 0_i64;
            while let Some((Some(parent), _)) = parents.get(cursor) {
                cursor = parent;
                hops += 1;
                assert!(hops <= 64, "{id} sits on a cycle or beyond depth 64");
            }
            assert_eq!(
                hops, *depth,
                "{id} stores a depth that disagrees with its ancestry"
            );
        }
    }
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn it_folder_move_cycle_rejected() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let chain = stack.seed_chain(alice.id, None, 0, "level-", 4).await;
    let (a, b, c, d) = (&chain[0], &chain[1], &chain[2], &chain[3]);
    let e = stack.seed_folder(alice.id, None, "E", 0).await;
    let f = stack.seed_folder(alice.id, None, "F", 0).await;
    let before = stack.folder_snapshot().await;

    for (case, source, destination) in [
        ("A into A", a, a),
        ("A into its child", a, b),
        ("A into its grandchild", a, c),
        ("A into its deepest descendant", a, d),
        ("B into B", b, b),
        ("B into its descendant", b, d),
        ("D into D", d, d),
    ] {
        let refused = stack.move_to(&alice, source, Some(destination)).await;
        assert_code(&refused, StatusCode::UNPROCESSABLE_ENTITY, "FOLDER_CYCLE");
        assert_eq!(
            stack.folder_snapshot().await,
            before,
            "{case}: a refused move changed parent, depth, name or timestamp"
        );
    }

    let into_c = stack.moved(&alice, &e, Some(c)).await;
    assert_eq!(into_c["parentId"].as_str(), Some(c.as_str()));
    assert_eq!(stack.depth_of(&e).await, 3);
    let into_e = stack.moved(&alice, &f, Some(&e)).await;
    assert_eq!(into_e["parentId"].as_str(), Some(e.as_str()));
    assert_eq!(stack.depth_of(&f).await, 4);

    let hoisted = stack.moved(&alice, c, None).await;
    assert_eq!(hoisted["parentId"], Value::Null);
    for (id, depth) in [(c, 0), (&e, 1), (&f, 2), (d, 1), (a, 0), (b, 1)] {
        assert_eq!(stack.depth_of(id).await, depth, "{id}");
    }

    stack.moved(&alice, a, Some(&f)).await;
    for (id, depth) in [(a, 3), (b, 4)] {
        assert_eq!(stack.depth_of(id).await, depth, "{id}");
    }
    let snapshot = stack.folder_snapshot().await;
    let refused = stack.move_to(&alice, c, Some(a)).await;
    assert_code(&refused, StatusCode::UNPROCESSABLE_ENTITY, "FOLDER_CYCLE");
    let refused = stack.move_to(&alice, &e, Some(b)).await;
    assert_code(&refused, StatusCode::UNPROCESSABLE_ENTITY, "FOLDER_CYCLE");
    assert_eq!(
        stack.folder_snapshot().await,
        snapshot,
        "a cycle through moved ancestors is still found"
    );
    stack.assert_forest_is_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_concurrent_never_builds_a_cycle() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    for round in 0..6 {
        let a = stack
            .seed_folder(alice.id, None, &format!("A{round}"), 0)
            .await;
        let b = stack
            .seed_folder(alice.id, None, &format!("B{round}"), 0)
            .await;
        let attempts = join_all([
            stack.move_to(&alice, &a, Some(&b)),
            stack.move_to(&alice, &b, Some(&a)),
        ])
        .await;
        let succeeded = attempts
            .iter()
            .filter(|attempt| attempt.status == StatusCode::OK)
            .count();
        assert_eq!(succeeded, 1, "round {round}: exactly one move may win");
        let refused = attempts
            .iter()
            .find(|attempt| attempt.status != StatusCode::OK)
            .unwrap();
        assert_code(refused, StatusCode::UNPROCESSABLE_ENTITY, "FOLDER_CYCLE");
        stack.assert_forest_is_sound().await;
    }

    let nodes = stack.seed_chain(alice.id, None, 0, "n", 1).await;
    let mut ids = nodes;
    for index in 1..6 {
        ids.push(
            stack
                .seed_folder(alice.id, None, &format!("n{index}"), 0)
                .await,
        );
    }
    let mut requests = Vec::new();
    for source in 0..ids.len() {
        for step in 1..=2 {
            let destination = (source + step) % ids.len();
            requests.push(stack.move_to(&alice, &ids[source], Some(&ids[destination])));
        }
    }
    let outcomes = join_all(requests).await;
    for outcome in &outcomes {
        assert!(
            outcome.status == StatusCode::OK || outcome.json()["error"]["code"] == "FOLDER_CYCLE",
            "{}",
            outcome.text()
        );
    }
    assert!(outcomes
        .iter()
        .any(|outcome| outcome.status == StatusCode::OK));
    stack.assert_forest_is_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_depth_boundary_and_subtree_rewrite() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let destinations = stack.seed_chain(alice.id, None, 0, "dest-", 62).await;
    let subtree = stack.seed_folder(alice.id, None, "S", 0).await;
    let s1 = stack.seed_folder(alice.id, Some(&subtree), "S1", 1).await;
    let s1_short = stack.seed_folder(alice.id, Some(&subtree), "S1b", 1).await;
    let s2 = stack.seed_folder(alice.id, Some(&s1), "S2", 2).await;
    let s3 = stack.seed_folder(alice.id, Some(&s2), "S3", 3).await;
    stack.seed_file(alice.id, Some(&s3), "kept.bin", 1, 7).await;
    let others = stack.seed_chain(bob.id, None, 0, "bob-", 3).await;
    let bob_before: Vec<Row> = stack
        .folder_snapshot()
        .await
        .into_iter()
        .filter(|row| others.contains(&row.0))
        .collect();

    let exact = stack.moved(&alice, &subtree, Some(&destinations[60])).await;
    assert_eq!(exact["parentId"].as_str(), Some(destinations[60].as_str()));
    for (id, depth) in [
        (&subtree, 61),
        (&s1, 62),
        (&s1_short, 62),
        (&s2, 63),
        (&s3, 64),
    ] {
        assert_eq!(
            stack.depth_of(id).await,
            depth,
            "{id} after the 64-deep move"
        );
    }
    assert_eq!(
        exact["fileCount"], 1,
        "the moved folder reports its subtree"
    );
    assert_eq!(exact["subfolderCount"], 4);
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM files WHERE folder_id = '{s3}'"
            ))
            .await,
        1
    );

    let before = stack.folder_snapshot().await;
    let refused = stack
        .move_to(&alice, &subtree, Some(&destinations[61]))
        .await;
    assert_code(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    assert_eq!(
        stack.folder_snapshot().await,
        before,
        "the 65-deep move changed nothing"
    );

    stack.moved(&alice, &subtree, None).await;
    for (id, depth) in [(&subtree, 0), (&s1, 1), (&s1_short, 1), (&s2, 2), (&s3, 3)] {
        assert_eq!(
            stack.depth_of(id).await,
            depth,
            "{id} after moving to the root"
        );
    }
    stack.moved(&alice, &subtree, Some(&destinations[59])).await;
    for (id, depth) in [
        (&subtree, 60),
        (&s1, 61),
        (&s1_short, 61),
        (&s2, 62),
        (&s3, 63),
    ] {
        assert_eq!(stack.depth_of(id).await, depth, "{id} one level shallower");
    }
    let leaf = stack.moved(&alice, &s3, Some(&destinations[61])).await;
    assert_eq!(leaf["parentId"].as_str(), Some(destinations[61].as_str()));
    assert_eq!(stack.depth_of(&s3).await, 62);
    stack.assert_forest_is_sound().await;

    let bob_after: Vec<Row> = stack
        .folder_snapshot()
        .await
        .into_iter()
        .filter(|row| others.contains(&row.0))
        .collect();
    assert_eq!(bob_after, bob_before, "another owner's folders never move");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_keep_both_at_the_destination() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let target = stack.make(&alice, "Target", None).await;
    let target_id = id_of(&target);
    stack.make(&alice, "Docs", Some(&target_id)).await;
    stack.make(&alice, "Docs (1)", Some(&target_id)).await;

    let first = stack.make(&alice, "Docs", None).await;
    let first_id = id_of(&first);
    let inner = stack.make(&alice, "Inner", Some(&first_id)).await;
    let inner_id = id_of(&inner);
    stack
        .seed_file(alice.id, Some(&inner_id), "x.bin", 1, 5)
        .await;

    let moved = stack.moved(&alice, &first_id, Some(&target_id)).await;
    assert_eq!(moved["name"], "Docs (2)");
    assert_eq!(moved["parentId"].as_str(), Some(target_id.as_str()));
    assert_eq!(moved["subfolderCount"], 1);
    assert_eq!(moved["fileCount"], 1);
    let stored: (String, String, i64) =
        sqlx::query_as("SELECT name, name_normalized, depth FROM folders WHERE id = ?1")
            .bind(&first_id)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(stored, ("Docs (2)".to_owned(), "docs (2)".to_owned(), 1));
    assert_eq!(stack.depth_of(&inner_id).await, 2);
    let inner_after = stack
        .read(&format!("{FOLDERS}/{inner_id}"), &alice)
        .await
        .json();
    assert_eq!(inner_after["parentId"].as_str(), Some(first_id.as_str()));
    assert_eq!(inner_after["name"], "Inner");

    let shouting = stack.make(&alice, "DOCS", None).await;
    let shouting = stack
        .moved(&alice, &id_of(&shouting), Some(&target_id))
        .await;
    assert_eq!(
        shouting["name"], "DOCS (3)",
        "a normalized collision keeps the spelling and takes the next free number"
    );

    let again = stack.make(&alice, "Docs", None).await;
    let again = stack.moved(&alice, &id_of(&again), Some(&target_id)).await;
    assert_eq!(again["name"], "Docs (4)");

    stack.make(&alice, "Archive", None).await;
    let nested = stack.make(&alice, "Archive", Some(&target_id)).await;
    let hoisted = stack.moved(&alice, &id_of(&nested), None).await;
    assert_eq!(hoisted["name"], "Archive (1)");
    assert_eq!(hoisted["parentId"], Value::Null);
    stack.assert_forest_is_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_to_the_current_parent_is_a_no_op() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let parent = stack.make(&alice, "Parent", None).await;
    let parent_id = id_of(&parent);
    let child = stack.make(&alice, "Docs (1)", Some(&parent_id)).await;
    let child_id = id_of(&child);
    stack.make(&alice, "Docs", Some(&parent_id)).await;
    let rooted = stack.make(&alice, "Rooted", None).await;
    let rooted_id = id_of(&rooted);
    stack.make(&alice, "Under", Some(&rooted_id)).await;
    let before = stack.folder_snapshot().await;

    let same = stack.moved(&alice, &child_id, Some(&parent_id)).await;
    assert_eq!(same["name"], "Docs (1)", "no suffix is invented");
    assert_eq!(same["parentId"].as_str(), Some(parent_id.as_str()));
    let rooted_again = stack.moved(&alice, &rooted_id, None).await;
    assert_eq!(rooted_again["name"], "Rooted");
    assert_eq!(rooted_again["parentId"], Value::Null);
    assert_eq!(rooted_again["subfolderCount"], 1);
    assert_eq!(
        stack.folder_snapshot().await,
        before,
        "a no-op move writes nothing, not even updated_at"
    );

    let elsewhere = stack.make(&alice, "Elsewhere", None).await;
    let moved_out = stack
        .moved(&alice, &child_id, Some(&id_of(&elsewhere)))
        .await;
    assert_eq!(
        moved_out["name"], "Docs (1)",
        "an unchanged namespace keeps the name"
    );
    let back = stack.moved(&alice, &child_id, Some(&parent_id)).await;
    assert_eq!(back["name"], "Docs (1)");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_name_exhaustion_is_a_conflict() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let crowded = stack.make(&alice, "Crowded", None).await;
    let crowded_id = id_of(&crowded);
    stack
        .execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 1000)
             INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             SELECT printf('0192f3a1-0003-7000-8000-%012x', i), '{}', '{crowded_id}',
                    CASE WHEN i = 0 THEN 'dup' ELSE 'dup (' || i || ')' END,
                    CASE WHEN i = 0 THEN 'dup' ELSE 'dup (' || i || ')' END,
                    1, '{SEEDED_AT}', '{SEEDED_AT}'
               FROM n",
            alice.id
        ))
        .await;
    let source = stack.make(&alice, "dup", None).await;
    let source_id = id_of(&source);
    let below = stack.make(&alice, "below", Some(&source_id)).await;
    let before = stack.folder_snapshot().await;

    let refused = stack.move_to(&alice, &source_id, Some(&crowded_id)).await;
    assert_code(&refused, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    assert_eq!(
        stack.folder_snapshot().await,
        before,
        "exhaustion moved neither the folder nor its subtree"
    );
    assert_eq!(stack.depth_of(&id_of(&below)).await, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_ownership_404() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let mine = id_of(&stack.make(&alice, "Mine", None).await);
    let mine_child = id_of(&stack.make(&alice, "Child", Some(&mine)).await);
    let theirs = id_of(&stack.make(&bob, "Theirs", None).await);
    let phantom = stack.fresh_id();
    let before = stack.folder_snapshot().await;

    for (case, source, destination) in [
        ("foreign source", theirs.as_str(), None),
        (
            "foreign source to own folder",
            theirs.as_str(),
            Some(mine.as_str()),
        ),
        ("missing source", phantom.as_str(), None),
        (
            "foreign destination",
            mine_child.as_str(),
            Some(theirs.as_str()),
        ),
        (
            "missing destination",
            mine_child.as_str(),
            Some(phantom.as_str()),
        ),
        (
            "malformed destination",
            mine_child.as_str(),
            Some("not-an-id"),
        ),
    ] {
        let attempt = stack.move_to(&alice, source, destination).await;
        assert_code(&attempt, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
        assert!(!attempt.text().contains(&theirs), "{case}");
    }
    let foreign = stack.move_to(&alice, &theirs, None).await;
    let missing = stack.move_to(&alice, &phantom, None).await;
    assert_eq!(
        foreign.error_without_request_id(),
        missing.error_without_request_id(),
        "a foreign folder must look exactly like a missing one"
    );
    let malformed = stack
        .api(
            Method::POST,
            &format!("{FOLDERS}/not-an-id/move"),
            &alice,
            Some(&json!({ "parentId": null })),
        )
        .await;
    assert_code(&malformed, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    assert_eq!(
        stack.folder_snapshot().await,
        before,
        "no refused move changed anything"
    );
    let _ = bob;
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_validates_the_body() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folder = id_of(&stack.make(&alice, "Folder", None).await);
    let path = format!("{FOLDERS}/{folder}/move");

    for (case, body, fields) in [
        ("absent parentId", json!({}), json!(["parentId"])),
        (
            "numeric parentId",
            json!({ "parentId": 7 }),
            json!(["parentId"]),
        ),
        (
            "array parentId",
            json!({ "parentId": [] }),
            json!(["parentId"]),
        ),
        (
            "undeclared member",
            json!({ "parentId": null, "name": "x" }),
            json!(["body"]),
        ),
        ("not an object", json!([]), json!(["body"])),
    ] {
        let response = stack.api(Method::POST, &path, &alice, Some(&body)).await;
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
    let garbage = stack
        .call(
            Call::new(Method::POST, &path, &alice.creds).raw("{"),
            HOST_WORK,
        )
        .await;
    assert_code(&garbage, StatusCode::BAD_REQUEST, "INVALID_JSON");
    let unchanged = stack.moved(&alice, &folder, None).await;
    assert_eq!(unchanged["name"], "Folder");
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_is_atomic_when_a_later_statement_fails() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let parent = stack.seed_folder(alice.id, None, "P", 4).await;
    let moved = stack.seed_folder(alice.id, Some(&parent), "R", 5).await;
    let child = stack.seed_folder(alice.id, Some(&moved), "C", 3).await;
    stack.seed_folder(alice.id, Some(&child), "G", 6).await;
    stack.seed_folder(alice.id, None, "R", 0).await;
    let before = stack.folder_snapshot().await;

    let failed = stack.move_to(&alice, &moved, None).await;
    assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert_eq!(
        stack.folder_snapshot().await,
        before,
        "the relocation of the root was rolled back with the failed depth rewrite"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_folder_move_and_ensure_path_keep_distinct_naming_semantics() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let target = id_of(&stack.make(&alice, "Target", None).await);
    let existing = id_of(&stack.make(&alice, "Docs", Some(&target)).await);
    let other = id_of(&stack.make(&alice, "Docs", None).await);

    let moved = stack.moved(&alice, &other, Some(&target)).await;
    assert_eq!(moved["name"], "Docs (1)", "an ordinary move keeps both");

    let ensured = stack
        .api(
            Method::POST,
            &format!("{FOLDERS}/ensure-path"),
            &alice,
            Some(&json!({ "parentId": target, "segments": ["docs"] })),
        )
        .await;
    assert_eq!(ensured.status, StatusCode::OK, "{}", ensured.text());
    assert_eq!(
        ensured.json()["folderIds"],
        json!([existing]),
        "ensure-path resolves the existing normalized folder"
    );
    assert_eq!(ensured.json()["created"], json!([]));
    let names: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM folders WHERE parent_id = ?1 ORDER BY name_normalized")
            .bind(&target)
            .fetch_all(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        names,
        [("Docs".to_owned(),), ("Docs (1)".to_owned(),)],
        "ensure-path invented no suffixed sibling"
    );
    stack.stop().await;
}

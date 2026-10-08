use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

use super::folders::{Member, FOLDERS, HOST_A, HOST_B, SEEDED_AT};
use super::profile::assert_code;
use super::*;
use crate::domain::naming::NameCandidate;
use crate::infra::db::DbError;

pub(super) const FILES: &str = "/api/v1/files";
pub(super) const NAME_CHECK: &str = "/api/v1/files/name-check";
pub(super) const BATCH_MOVE: &str = "/api/v1/files/batch/move";

static OBJECTS: AtomicU32 = AtomicU32::new(10_000);

pub(super) type FileRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
);

impl Stack {
    pub(super) async fn put_file(
        &self,
        owner: UserId,
        folder: Option<&str>,
        name: &str,
        size: i64,
    ) -> String {
        self.put_file_at(owner, folder, name, size, SEEDED_AT, SEEDED_AT)
            .await
    }

    pub(super) async fn put_file_at(
        &self,
        owner: UserId,
        folder: Option<&str>,
        name: &str,
        size: i64,
        created: &str,
        updated: &str,
    ) -> String {
        let n = OBJECTS.fetch_add(1, Ordering::Relaxed);
        let id = self.fresh_id();
        let candidate = NameCandidate::new(name).unwrap();
        self.pools
            .write_tx(&self.clock, "files.test_seed", async |tx| {
                sqlx::query(
                    "INSERT INTO storage_objects
                         (id, object_key, provider, size_bytes, state, refcount, created_at,
                          updated_at, finalized_at)
                     VALUES (?1, ?2, 'local', ?3, 'active', 1, ?4, ?4, ?4)",
                )
                .bind(format!("object-{n}"))
                .bind(format!("objects/00/00/{n:032x}"))
                .bind(size)
                .bind(SEEDED_AT)
                .execute(tx.executor())
                .await?;
                sqlx::query(
                    "INSERT INTO files
                         (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                          extension, size_bytes, mime_type, mime_source, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'application/pdf', 'sniffed', ?9, ?10)",
                )
                .bind(&id)
                .bind(owner.to_string())
                .bind(folder)
                .bind(format!("object-{n}"))
                .bind(candidate.display())
                .bind(candidate.normalized())
                .bind(candidate.extension())
                .bind(size)
                .bind(created)
                .bind(updated)
                .execute(tx.executor())
                .await?;
                Ok::<(), DbError>(())
            })
            .await
            .unwrap();
        id
    }

    pub(super) async fn file_rows(&self) -> Vec<FileRow> {
        sqlx::query_as(
            "SELECT id, folder_id, name, name_normalized, extension, updated_at,
                    storage_object_id, size_bytes, mime_type, mime_source
               FROM files ORDER BY id",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn file_row(&self, id: &str) -> FileRow {
        self.file_rows()
            .await
            .into_iter()
            .find(|row| row.0 == id)
            .unwrap()
    }

    pub(super) async fn set_times(&self, table: &str, id: &str, created: &str, updated: &str) {
        self.execute(&format!(
            "UPDATE {table} SET created_at = '{created}', updated_at = '{updated}' WHERE id = '{id}'"
        ))
        .await;
    }

    pub(super) async fn page_through(
        &self,
        member: &Member,
        query: &str,
        limit: usize,
    ) -> Vec<Value> {
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("{FILES}?limit={limit}");
            if !query.is_empty() {
                path.push('&');
                path.push_str(query);
            }
            if let Some(cursor) = &cursor {
                path.push_str("&cursor=");
                path.push_str(cursor);
            }
            let fetched = self.read(&path, member).await;
            assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
            let page = fetched.json();
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            pages.push(page);
            assert!(pages.len() < 200, "the traversal does not terminate");
            if cursor.is_none() {
                return pages;
            }
        }
    }

    pub(super) async fn move_file_to(
        &self,
        member: &Member,
        id: &str,
        folder: Option<&str>,
    ) -> Fetched {
        let body = json!({ "folderId": folder });
        self.api(
            Method::POST,
            &format!("{FILES}/{id}/move"),
            member,
            Some(&body),
        )
        .await
    }

    pub(super) async fn rename_file(&self, member: &Member, id: &str, body: &Value) -> Fetched {
        self.api(Method::PATCH, &format!("{FILES}/{id}"), member, Some(body))
            .await
    }

    pub(super) async fn batch_move(&self, member: &Member, body: &Value) -> Fetched {
        self.api(Method::POST, BATCH_MOVE, member, Some(body)).await
    }

    pub(super) async fn name_check(
        &self,
        member: &Member,
        folder: Option<&str>,
        name: &str,
    ) -> Fetched {
        let mut path = format!(
            "{NAME_CHECK}?name={}",
            url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>()
        );
        if let Some(folder) = folder {
            path.push_str("&folderId=");
            path.push_str(folder);
        }
        self.read(&path, member).await
    }
}

pub(super) fn flatten(pages: &[Value]) -> Vec<Value> {
    pages
        .iter()
        .flat_map(|page| page["items"].as_array().unwrap().clone())
        .collect()
}

pub(super) fn item_names(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

pub(super) async fn write_surface(
    stack: &Stack,
) -> (
    Vec<i64>,
    Vec<FileRow>,
    Vec<(String, Option<String>, String, i64, String)>,
) {
    let mut counts = Vec::new();
    for table in [
        "files",
        "folders",
        "storage_objects",
        "audit_events",
        "idempotency_records",
        "jobs",
        "transfer_sessions",
        "quota_reservations",
    ] {
        counts.push(
            stack
                .scalar_i64(&format!("SELECT COUNT(*) FROM {table}"))
                .await,
        );
    }
    (
        counts,
        stack.file_rows().await,
        stack.folder_snapshot_rows().await,
    )
}

struct Expected {
    id: String,
    kind: &'static str,
    normalized: String,
    size: i64,
    created: String,
    updated: String,
}

fn expected_order(nodes: &[Expected], sort: &str) -> Vec<String> {
    let (field, direction) = sort.split_once(':').unwrap();
    let key = |node: &Expected| -> (String, i64, String) {
        match field {
            "name" => (node.normalized.clone(), 0, node.id.clone()),
            "size" => (String::new(), node.size, node.id.clone()),
            "createdAt" => (node.created.clone(), 0, node.id.clone()),
            _ => (node.updated.clone(), 0, node.id.clone()),
        }
    };
    let mut ordered = Vec::new();
    for kind in ["folder", "file"] {
        let mut group: Vec<&Expected> = nodes.iter().filter(|node| node.kind == kind).collect();
        group.sort_by_key(|node| key(node));
        if direction == "desc" {
            group.reverse();
        }
        ordered.extend(group.into_iter().map(|node| node.id.clone()));
    }
    ordered
}

const T1: &str = "2026-09-20T10:00:00.000Z";
const T2: &str = "2026-09-21T10:00:00.000Z";
const T3: &str = "2026-09-22T10:00:00.000Z";
const T4: &str = "2026-09-23T10:00:00.000Z";

#[tokio::test]
async fn it_files_browse_mixed_pages_put_folders_first_for_every_sort() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let mut nodes: Vec<Expected> = Vec::new();
    let folder_specs = [
        ("alpha", 100, T3, T2),
        ("Bravo", 100, T1, T4),
        ("charlie", 0, T2, T1),
        ("delta", 5, T4, T3),
        ("Echo", 7, T2, T2),
    ];
    for (name, bytes, created, updated) in folder_specs {
        let id = stack.seed_folder(alice.id, None, name, 0).await;
        stack.set_times("folders", &id, created, updated).await;
        if bytes > 0 {
            let child = stack.seed_folder(alice.id, Some(&id), "inner", 1).await;
            stack
                .put_file(alice.id, Some(&child), "inner.bin", bytes)
                .await;
        }
        nodes.push(Expected {
            normalized: name.to_lowercase(),
            id,
            kind: "folder",
            size: bytes,
            created: created.to_owned(),
            updated: updated.to_owned(),
        });
    }
    let file_specs = [
        ("a.txt", 30, T3, T1),
        ("B.txt", 10, T1, T3),
        ("c.txt", 20, T4, T4),
        ("D.txt", 10, T2, T2),
        ("e.txt", 40, T2, T3),
        ("f.txt", 30, T3, T1),
        ("g.txt", 0, T1, T2),
    ];
    for (name, size, created, updated) in file_specs {
        let id = stack
            .put_file_at(alice.id, None, name, size, created, updated)
            .await;
        nodes.push(Expected {
            normalized: name.to_lowercase(),
            id,
            kind: "file",
            size,
            created: created.to_owned(),
            updated: updated.to_owned(),
        });
    }
    let foreign = stack.seed_folder(bob.id, None, "aaa-bob", 0).await;
    stack.put_file(bob.id, None, "aaa-bob.txt", 1).await;
    stack
        .put_file(bob.id, Some(&foreign), "deep-bob.txt", 1)
        .await;

    for sort in [
        "name:asc",
        "name:desc",
        "size:asc",
        "size:desc",
        "createdAt:asc",
        "createdAt:desc",
        "updatedAt:asc",
        "updatedAt:desc",
    ] {
        let expected = expected_order(&nodes, sort);
        for limit in [1, 3, 5, 12, 200] {
            let pages = stack
                .page_through(&alice, &format!("sort={sort}"), limit)
                .await;
            let items = flatten(&pages);
            let seen: Vec<String> = items
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(seen, expected, "{sort} limit {limit}");
            let kinds: Vec<&str> = items
                .iter()
                .map(|item| item["kind"].as_str().unwrap())
                .collect();
            assert_eq!(
                kinds,
                [vec!["folder"; 5], vec!["file"; 7]].concat(),
                "{sort} limit {limit}: every folder precedes every file"
            );
            for page in &pages {
                assert_eq!(page["totalCount"], 12, "{sort} limit {limit}");
            }
            let last = pages.last().unwrap();
            assert_eq!(last["nextCursor"], Value::Null);
            for page in &pages[..pages.len() - 1] {
                assert_eq!(page["items"].as_array().unwrap().len(), limit);
                assert!(page["nextCursor"].is_string());
            }
            let unique: HashSet<&String> = seen.iter().collect();
            assert_eq!(unique.len(), seen.len(), "{sort} limit {limit}: duplicates");
        }
    }

    let by_size = stack.page_through(&alice, "sort=size:desc", 200).await;
    let folders: Vec<(String, i64)> = flatten(&by_size)
        .iter()
        .filter(|item| item["kind"] == "folder")
        .map(|item| {
            (
                item["name"].as_str().unwrap().to_owned(),
                item["totalBytes"].as_i64().unwrap(),
            )
        })
        .collect();
    let mut sizes: Vec<i64> = folders.iter().map(|folder| folder.1).collect();
    sizes.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(
        folders.iter().map(|folder| folder.1).collect::<Vec<_>>(),
        sizes,
        "a folder sorts by its recursive totalBytes: {folders:?}"
    );
    assert_eq!(sizes, [100, 100, 7, 5, 0]);

    let mid = stack.page_through(&alice, "", 3).await;
    assert_eq!(mid.len(), 4);
    let kinds: Vec<Vec<&str>> = mid
        .iter()
        .map(|page| {
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["kind"].as_str().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(kinds[0], ["folder"; 3]);
    assert_eq!(kinds[1], ["folder", "folder", "file"]);
    assert_eq!(kinds[2], ["file"; 3]);
    assert_eq!(kinds[3], ["file"; 3]);
    stack.stop().await;
}

#[tokio::test]
async fn it_files_browse_is_direct_children_only_and_owner_scoped() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let top = stack.seed_folder(alice.id, None, "Top", 0).await;
    let middle = stack.seed_folder(alice.id, Some(&top), "Middle", 1).await;
    let bottom = stack
        .seed_folder(alice.id, Some(&middle), "Bottom", 2)
        .await;
    stack.put_file(alice.id, None, "root.txt", 1).await;
    stack.put_file(alice.id, Some(&top), "top.txt", 2).await;
    stack
        .put_file(alice.id, Some(&middle), "middle.txt", 4)
        .await;
    stack
        .put_file(alice.id, Some(&bottom), "bottom.txt", 8)
        .await;
    let bobs = stack.seed_folder(bob.id, None, "BobTop", 0).await;
    stack.put_file(bob.id, Some(&bobs), "bob.txt", 16).await;

    let root_page = stack.read(FILES, &alice).await.json();
    assert_eq!(
        item_names(&flatten(std::slice::from_ref(&root_page))),
        ["Top", "root.txt"]
    );
    assert_eq!(root_page["totalCount"], 2);
    let top_page = stack
        .read(&format!("{FILES}?folderId={top}"), &alice)
        .await
        .json();
    assert_eq!(
        item_names(&flatten(std::slice::from_ref(&top_page))),
        ["Middle", "top.txt"]
    );
    assert_eq!(top_page["totalCount"], 2);
    let middle_item = &top_page["items"][0];
    assert_eq!(middle_item["kind"], "folder");
    assert_eq!(middle_item["fileCount"], 2);
    assert_eq!(middle_item["subfolderCount"], 1);
    assert_eq!(middle_item["totalBytes"], 12);
    let file_item = &top_page["items"][1];
    assert_eq!(file_item["kind"], "file");
    assert_eq!(file_item["sizeBytes"], 2);
    assert_eq!(file_item["contentType"], "application/pdf");
    assert_eq!(file_item["folderId"], top);
    let leaf = stack
        .read(&format!("{FILES}?folderId={bottom}"), &alice)
        .await
        .json();
    assert_eq!(
        item_names(&flatten(std::slice::from_ref(&leaf))),
        ["bottom.txt"]
    );
    assert_eq!(leaf["totalCount"], 1);

    let empty = stack
        .read(
            &format!(
                "{FILES}?folderId={}",
                stack.seed_folder(alice.id, None, "Empty", 0).await
            ),
            &alice,
        )
        .await
        .json();
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["totalCount"], 0);
    assert_eq!(empty["nextCursor"], Value::Null);

    let phantom = stack.fresh_id();
    for folder in [bobs.as_str(), phantom.as_str(), "not-a-uuid"] {
        let refused = stack
            .read(&format!("{FILES}?folderId={folder}"), &alice)
            .await;
        assert_code(&refused, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    }
    let serialized = stack
        .read(&format!("{FILES}?limit=200"), &alice)
        .await
        .text();
    assert!(!serialized.contains("bob.txt") && !serialized.contains("BobTop"));
    stack.stop().await;
}

#[tokio::test]
async fn it_files_browse_rejects_search_bad_sorts_and_foreign_cursors() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    for index in 0..4 {
        stack
            .seed_folder(alice.id, None, &format!("folder-{index}"), 0)
            .await;
        stack
            .put_file(alice.id, None, &format!("file-{index}.txt"), index)
            .await;
    }

    for path in [
        format!("{FILES}?q=file"),
        format!("{FILES}?q="),
        format!("{FILES}?sort=type:asc"),
        format!("{FILES}?sort=name"),
        format!("{FILES}?sort=name:up"),
        format!("{FILES}?limit=0"),
        format!("{FILES}?limit=201"),
        format!("{FILES}?limit=ten"),
    ] {
        let refused = stack.read(&path, &alice).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    let search = stack.read(&format!("{FILES}?q=file"), &alice).await;
    assert_eq!(search.json()["error"]["details"]["fields"], json!(["q"]));

    let first = stack
        .read(&format!("{FILES}?limit=2&sort=name:asc"), &alice)
        .await
        .json();
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let resumed = stack
        .read(
            &format!("{FILES}?limit=2&sort=name:asc&cursor={cursor}"),
            &alice,
        )
        .await;
    assert_eq!(resumed.status, StatusCode::OK);

    let mut tampered = cursor.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    for path in [
        format!("{FILES}?limit=2&sort=name:asc&cursor={tampered}"),
        format!("{FILES}?limit=2&sort=name:desc&cursor={cursor}"),
        format!("{FILES}?limit=2&sort=size:asc&cursor={cursor}"),
        format!("{FILES}?cursor=%%%"),
        format!("{FILES}?cursor="),
        format!("{FILES}?cursor={}", &cursor[..cursor.len() / 2]),
    ] {
        let refused = stack.read(&path, &alice).await;
        assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    }

    let folder_page = stack
        .read(&format!("{FOLDERS}?limit=2&sort=name:asc"), &alice)
        .await
        .json();
    let ungrouped = folder_page["nextCursor"].as_str().unwrap();
    let refused = stack
        .read(
            &format!("{FILES}?limit=2&sort=name:asc&cursor={ungrouped}"),
            &alice,
        )
        .await;
    assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let refused = stack
        .read(
            &format!("{FOLDERS}?limit=2&sort=name:asc&cursor={cursor}"),
            &alice,
        )
        .await;
    assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    stack.stop().await;
}

#[tokio::test]
async fn it_files_browse_keyset_is_stable_under_concurrent_inserts() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let mut original = Vec::new();
    for name in ["b-one", "c-two", "d-three", "e-four"] {
        original.push(stack.seed_folder(alice.id, None, name, 0).await);
    }
    for name in ["b.txt", "c.txt", "d.txt", "e.txt"] {
        original.push(stack.put_file(alice.id, None, name, 1).await);
    }

    let first = stack.read(&format!("{FILES}?limit=3"), &alice).await.json();
    let mut seen: Vec<String> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect();
    let mut cursor = first["nextCursor"].as_str().unwrap().to_owned();

    let before_folder = stack.seed_folder(alice.id, None, "a-new", 0).await;
    let after_folder = stack.seed_folder(alice.id, None, "z-new", 0).await;
    let before_file = stack.put_file(alice.id, None, "a-new.txt", 1).await;
    let after_file = stack.put_file(alice.id, None, "z-new.txt", 1).await;

    loop {
        let page = stack
            .read(&format!("{FILES}?limit=3&cursor={cursor}"), &alice)
            .await
            .json();
        seen.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_owned()),
        );
        match page["nextCursor"].as_str() {
            Some(next) => next.clone_into(&mut cursor),
            None => break,
        }
    }
    let unique: HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), seen.len(), "a row repeated");
    for id in &original {
        assert!(seen.contains(id), "{id} was skipped");
    }
    for id in [&after_folder, &before_file, &after_file] {
        assert!(
            seen.contains(id),
            "{id} sorts after the cursor and must appear"
        );
    }
    assert!(
        !seen.contains(&before_folder),
        "a folder that sorts before the cursor is not revisited"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_file_get_returns_canonical_metadata_without_storage_identifiers() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folder = stack.seed_folder(alice.id, None, "Docs", 0).await;
    let id = stack
        .put_file_at(alice.id, Some(&folder), "Report.PDF", 2048, T1, T2)
        .await;
    stack
        .execute(&format!(
            "UPDATE files SET description = 'Q4', mime_type = 'application/pdf' WHERE id = '{id}'"
        ))
        .await;

    let fetched = stack.read(&format!("{FILES}/{id}"), &alice).await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
    let file = fetched.json();
    assert_eq!(
        file,
        json!({
            "kind": "file",
            "id": id,
            "name": "Report.PDF",
            "description": "Q4",
            "sizeBytes": 2048,
            "contentType": "application/pdf",
            "folderId": folder,
            "createdAt": T1,
            "updatedAt": T2,
        })
    );

    let listed = stack
        .read(&format!("{FILES}?folderId={folder}"), &alice)
        .await
        .json();
    assert_eq!(
        listed["items"][0], file,
        "listing and detail share one shape"
    );

    let storage_object = stack
        .scalar_text(&format!(
            "SELECT storage_object_id FROM files WHERE id = '{id}'"
        ))
        .await;
    let object_key = stack
        .scalar_text(&format!(
            "SELECT object_key FROM storage_objects WHERE id = '{storage_object}'"
        ))
        .await;
    for body in [
        fetched.text(),
        stack
            .read(&format!("{FILES}?folderId={folder}"), &alice)
            .await
            .text(),
        stack
            .rename_file(&alice, &id, &json!({ "description": "again" }))
            .await
            .text(),
        stack.move_file_to(&alice, &id, None).await.text(),
        stack
            .batch_move(
                &alice,
                &json!({ "fileIds": [id], "targetFolderId": folder }),
            )
            .await
            .text(),
    ] {
        for forbidden in [
            storage_object.as_str(),
            object_key.as_str(),
            "storage",
            "objects/",
            "bucket",
            "uploadId",
            "mimeSource",
            "nameNormalized",
            "ownerId",
        ] {
            assert!(!body.contains(forbidden), "{forbidden} leaked: {body}");
        }
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_file_other_owner_404() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let alices_folder = stack.seed_folder(alice.id, None, "Private", 0).await;
    let alices_file = stack
        .put_file(alice.id, Some(&alices_folder), "secret.txt", 9)
        .await;
    let bobs_folder = stack.seed_folder(bob.id, None, "Mine", 0).await;
    let bobs_file = stack.put_file(bob.id, None, "mine.txt", 3).await;
    let phantom_file = stack.fresh_id();
    let phantom_folder = stack.fresh_id();
    let before = stack.file_rows().await;
    let folders_before = stack.folder_snapshot_rows().await;

    type Case = (&'static str, Method, String, Option<Value>);
    let requests = |file: &str, folder: &str| -> Vec<Case> {
        vec![
            ("get", Method::GET, format!("{FILES}/{file}"), None),
            (
                "patch",
                Method::PATCH,
                format!("{FILES}/{file}"),
                Some(json!({ "name": "hijacked.txt", "description": "taken" })),
            ),
            (
                "move foreign file to own folder",
                Method::POST,
                format!("{FILES}/{file}/move"),
                Some(json!({ "folderId": bobs_folder })),
            ),
            (
                "move foreign file to root",
                Method::POST,
                format!("{FILES}/{file}/move"),
                Some(json!({ "folderId": null })),
            ),
            (
                "batch with a foreign file",
                Method::POST,
                BATCH_MOVE.to_owned(),
                Some(json!({ "fileIds": [bobs_file, file], "targetFolderId": bobs_folder })),
            ),
            (
                "batch of only a foreign file",
                Method::POST,
                BATCH_MOVE.to_owned(),
                Some(json!({ "fileIds": [file], "targetFolderId": bobs_folder })),
            ),
            (
                "name-check in foreign folder",
                Method::GET,
                format!("{NAME_CHECK}?name=a.txt&folderId={folder}"),
                None,
            ),
            (
                "browse foreign folder",
                Method::GET,
                format!("{FILES}?folderId={folder}"),
                None,
            ),
            (
                "move own file into foreign folder",
                Method::POST,
                format!("{FILES}/{bobs_file}/move"),
                Some(json!({ "folderId": folder })),
            ),
            (
                "batch into a foreign target",
                Method::POST,
                BATCH_MOVE.to_owned(),
                Some(json!({ "fileIds": [bobs_file], "targetFolderId": folder })),
            ),
            (
                "batch of a foreign folder",
                Method::POST,
                BATCH_MOVE.to_owned(),
                Some(
                    json!({ "fileIds": [bobs_file], "folderIds": [folder], "targetFolderId": null }),
                ),
            ),
        ]
    };
    let foreign = requests(&alices_file, &alices_folder);
    let missing = requests(&phantom_file, &phantom_folder);
    for ((label, method, path, body), (_, phantom_method, phantom_path, phantom_body)) in
        foreign.into_iter().zip(missing)
    {
        let refused = stack.api(method, &path, &bob, body.as_ref()).await;
        let absent = stack
            .api(phantom_method, &phantom_path, &bob, phantom_body.as_ref())
            .await;
        assert!(
            refused.status == StatusCode::NOT_FOUND,
            "{label}: {}",
            refused.text()
        );
        assert!(matches!(
            refused.error_code().as_str(),
            "FILE_NOT_FOUND" | "FOLDER_NOT_FOUND"
        ));
        assert_eq!(refused.status, absent.status, "{label}");
        assert_eq!(
            refused.error_without_request_id(),
            absent.error_without_request_id(),
            "{label}: a foreign id answers exactly like a missing one"
        );
    }
    assert_eq!(
        stack.file_rows().await,
        before,
        "no foreign mutation leaked through"
    );
    assert_eq!(stack.folder_snapshot_rows().await, folders_before);

    let own = stack.read(&format!("{FILES}/{bobs_file}"), &bob).await;
    assert_eq!(own.status, StatusCode::OK);
    stack.stop().await;
}

#[tokio::test]
async fn it_file_name_check_is_advisory_and_read_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let folder = stack.seed_folder(alice.id, None, "Photos", 0).await;
    stack.seed_folder(alice.id, None, "docs", 0).await;
    stack
        .put_file(alice.id, Some(&folder), "photo.jpg", 1)
        .await;
    stack
        .put_file(alice.id, Some(&folder), "photo (1).jpg", 1)
        .await;
    stack.put_file(alice.id, None, "root.txt", 1).await;
    stack
        .put_file(
            bob.id,
            Some(&stack.seed_folder(bob.id, None, "Photos", 0).await),
            "free.jpg",
            1,
        )
        .await;
    stack.put_file(bob.id, None, "other.txt", 1).await;

    let before = write_surface(&stack).await;

    let available = stack.name_check(&alice, Some(&folder), "fresh.jpg").await;
    assert_eq!(available.status, StatusCode::OK, "{}", available.text());
    assert_eq!(available.headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(
        available.json(),
        json!({ "available": true, "suggestedName": null })
    );
    let taken = stack.name_check(&alice, Some(&folder), "photo.jpg").await;
    assert_eq!(
        taken.json(),
        json!({ "available": false, "suggestedName": "photo (2).jpg" })
    );
    let case_insensitive = stack.name_check(&alice, Some(&folder), "PHOTO.JPG").await;
    assert_eq!(
        case_insensitive.json(),
        json!({ "available": false, "suggestedName": "PHOTO (2).JPG" })
    );
    let second = stack
        .name_check(&alice, Some(&folder), "photo (1).jpg")
        .await;
    assert_eq!(
        second.json(),
        json!({ "available": false, "suggestedName": "photo (1) (1).jpg" })
    );
    let root_taken = stack.name_check(&alice, None, "root.txt").await;
    assert_eq!(
        root_taken.json(),
        json!({ "available": false, "suggestedName": "root (1).txt" })
    );
    let root_free = stack.name_check(&alice, None, "photo.jpg").await;
    assert_eq!(
        root_free.json(),
        json!({ "available": true, "suggestedName": null }),
        "the root and a folder are separate namespaces"
    );
    let own_namespace = stack.name_check(&bob, None, "root.txt").await;
    assert_eq!(
        own_namespace.json(),
        json!({ "available": true, "suggestedName": null }),
        "another owner's files never make a name unavailable"
    );
    let folder_namespace = stack.name_check(&alice, None, "docs").await;
    assert_eq!(
        folder_namespace.json(),
        json!({ "available": true, "suggestedName": null }),
        "a folder named docs does not take the file name docs"
    );

    for _ in 0..3 {
        stack.name_check(&alice, Some(&folder), "photo.jpg").await;
    }
    assert_eq!(
        write_surface(&stack).await,
        before,
        "name-check wrote nothing"
    );

    stack.clock.advance(Duration::from_secs(60));
    let suggested = stack
        .name_check(&alice, Some(&folder), "photo.jpg")
        .await
        .json();
    let target = suggested["suggestedName"].as_str().unwrap().to_owned();
    let incoming = stack.put_file(alice.id, None, "incoming.jpg", 1).await;
    stack.move_file_to(&alice, &incoming, Some(&folder)).await;
    let renamed = stack
        .rename_file(&alice, &incoming, &json!({ "name": "photo.jpg" }))
        .await
        .json();
    assert_eq!(
        renamed["name"], target,
        "the suggestion is what a write picks today"
    );

    let raced = stack
        .name_check(&alice, Some(&folder), "photo.jpg")
        .await
        .json();
    assert_eq!(
        raced["suggestedName"], "photo (3).jpg",
        "a suggestion is not a reservation: the next check moves on"
    );

    let invalid = stack.name_check(&alice, None, "a/b").await;
    assert_code(&invalid, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    let dots = stack.name_check(&alice, None, "..").await;
    assert_code(&dots, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    let empty = stack.name_check(&alice, None, "").await;
    assert_code(&empty, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    let missing = stack.read(NAME_CHECK, &alice).await;
    assert_code(
        &missing,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(
        missing.json()["error"]["details"]["fields"],
        json!(["name"])
    );
    let twice = stack
        .read(&format!("{NAME_CHECK}?name=a&name=b"), &alice)
        .await;
    assert_code(&twice, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    let malformed = stack
        .read(&format!("{NAME_CHECK}?name=a.txt&folderId=nope"), &alice)
        .await;
    assert_code(&malformed, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    stack.stop().await;
}

#[tokio::test]
async fn it_file_name_check_exhaustion_is_a_conflict() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.put_file(alice.id, None, "crowded.txt", 1).await;
    stack
        .seed_numbered_names(alice.id, None, "crowded", "txt", 1_000)
        .await;

    let exhausted = stack.name_check(&alice, None, "crowded.txt").await;
    assert_code(&exhausted, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    stack.stop().await;
}

impl Stack {
    pub(super) async fn scalar_text(&self, sql: &str) -> String {
        sqlx::query_scalar(sql)
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn folder_snapshot_rows(
        &self,
    ) -> Vec<(String, Option<String>, String, i64, String)> {
        sqlx::query_as("SELECT id, parent_id, name, depth, updated_at FROM folders ORDER BY id")
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn seed_numbered_names(
        &self,
        owner: UserId,
        folder: Option<&str>,
        base: &str,
        extension: &str,
        count: u32,
    ) {
        let folder = folder.map_or_else(|| "NULL".to_owned(), |folder| format!("'{folder}'"));
        let seed = OBJECTS.fetch_add(count + 1, Ordering::Relaxed);
        self.execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {count})
             INSERT INTO storage_objects
                 (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
             SELECT printf('bulk-{seed}-%05d', i), printf('objects/ff/ff/%032x', i + {seed}), 'local', 1,
                    'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}' FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {count})
             INSERT INTO files
                 (id, owner_id, folder_id, storage_object_id, name, name_normalized, extension,
                  size_bytes, created_at, updated_at)
             SELECT printf('0192f3a1-0002-7000-8000-%012x', i + {seed}), '{owner}', {folder},
                    printf('bulk-{seed}-%05d', i), printf('{base} (%d).{extension}', i),
                    printf('{base} (%d).{extension}', i), '{extension}', 1, '{SEEDED_AT}', '{SEEDED_AT}'
               FROM n"
        ))
        .await;
    }
}

#[test]
fn unit_file_routes_are_declared_with_their_classes() {
    let assembled = application_routes().build().unwrap();
    let mut declared: Vec<(Method, String, AuthClass, RateLimitClass)> = assembled
        .inventory
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with(FILES))
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
            FILES.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
        (
            Method::GET,
            NAME_CHECK.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
        (
            Method::GET,
            format!("{FILES}/{{id}}"),
            AuthClass::Authenticated,
            RateLimitClass::Read,
        ),
        (
            Method::PATCH,
            format!("{FILES}/{{id}}"),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
        (
            Method::POST,
            format!("{FILES}/{{id}}/move"),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
        (
            Method::POST,
            BATCH_MOVE.to_owned(),
            AuthClass::Authenticated,
            RateLimitClass::Write,
        ),
    ];
    let order = |entry: &(Method, String, AuthClass, RateLimitClass)| {
        (entry.1.clone(), entry.0.to_string())
    };
    expected.sort_by_key(order);
    declared.sort_by_key(order);
    assert_eq!(
        declared, expected,
        "no delete, content, preview, thumbnail or search route belongs to this task"
    );
}

#[tokio::test]
async fn it_file_static_routes_are_not_swallowed_by_the_id_route() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let checked = stack
        .read(&format!("{NAME_CHECK}?name=a.txt"), &alice)
        .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text());
    assert_eq!(checked.json()["available"], true);
    let batch = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [stack.fresh_id()], "targetFolderId": null }),
        )
        .await;
    assert_code(&batch, StatusCode::NOT_FOUND, "FILE_NOT_FOUND");
    assert!(batch.json()["error"]["code"] != "METHOD_NOT_ALLOWED");
    for (method, path) in [
        (Method::DELETE, format!("{FILES}/{}", stack.fresh_id())),
        (Method::GET, format!("{FILES}/{}/content", stack.fresh_id())),
        (Method::GET, format!("{FILES}/{}/preview", stack.fresh_id())),
        (
            Method::GET,
            format!("{FILES}/{}/thumbnail", stack.fresh_id()),
        ),
        (Method::POST, "/api/v1/files/batch/delete".to_owned()),
    ] {
        let absent = stack.api(method, &path, &alice, None).await;
        assert!(
            absent.status == StatusCode::NOT_FOUND
                || absent.status == StatusCode::METHOD_NOT_ALLOWED,
            "{path}: {}",
            absent.status
        );
    }
    stack.stop().await;
}

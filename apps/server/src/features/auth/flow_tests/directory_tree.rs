use futures_util::future::join_all;

use super::finalize::{bytes, Rig};
use super::folders::{FOLDERS, HOST_A, HOST_B};
use super::profile::assert_code;
use super::transfers::{
    field_codes, pathed, request_body, request_in, s3_stack_storage, KEY_A, KEY_B, KEY_C,
};
use super::tus::{metadata, Planned, TusReq, TUS};
use super::tus_patch::patch;
use super::*;
use crate::features::transfers::finalize::Point;
use crate::infra::db::DbError;
use crate::storage::s3::profile::ProviderProfile;

type FolderRow = (String, String, Option<String>, i64);

async fn folder_rows(stack: &Stack) -> Vec<FolderRow> {
    sqlx::query_as("SELECT id, name, parent_id, depth FROM folders ORDER BY depth, name, id")
        .fetch_all(stack.pools.reader().executor())
        .await
        .unwrap()
}

async fn folder_path(stack: &Stack, folder: Option<String>) -> String {
    let mut names = Vec::new();
    let mut current = folder;
    while let Some(id) = current {
        let (name, parent): (String, Option<String>) =
            sqlx::query_as("SELECT name, parent_id FROM folders WHERE id = ?1")
                .bind(id)
                .fetch_one(stack.pools.reader().executor())
                .await
                .unwrap();
        names.push(name);
        current = parent;
    }
    names.reverse();
    names.join("/")
}

async fn duplicate_folder_rows(stack: &Stack) -> i64 {
    stack
        .scalar_i64(
            "SELECT COUNT(*) FROM (
                SELECT owner_id, parent_id, name_normalized FROM folders
                 GROUP BY owner_id, parent_id, name_normalized HAVING COUNT(*) > 1)",
        )
        .await
}

async fn execute_fallible(stack: &Stack, sql: &str) -> Result<(), DbError> {
    stack
        .pools
        .write_tx(&stack.clock, "auth.test_execute_fallible", async |tx| {
            sqlx::query(sql).execute(tx.executor()).await?;
            Ok::<(), DbError>(())
        })
        .await
}

fn leaf_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap()
}

fn file_json(id: &str, name: &str, path: &str, size: u64) -> Value {
    json!({ "clientId": id, "name": name, "sizeBytes": size, "relativePath": path })
}

async fn open_tree(rig: &Rig, target: Option<&str>, files: &[(&str, usize)]) -> Vec<Planned> {
    let items: Vec<Value> = files
        .iter()
        .enumerate()
        .map(|(index, (path, size))| pathed(&format!("c{index}"), path, *size as u64))
        .collect();
    let created = rig
        .stack
        .open(&rig.alice, &request_body(target, &items))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let session = created.json()["id"].as_str().unwrap().to_owned();
    rig.stack
        .item_ids(&session)
        .await
        .into_iter()
        .zip(files)
        .map(|(item, (path, _))| Planned {
            session: session.clone(),
            item,
            name: leaf_of(path).to_owned(),
        })
        .collect()
}

async fn create_upload(rig: &Rig, planned: &Planned, path: &str, length: usize) -> Fetched {
    let extra: Vec<(&str, &str)> = if path.contains('/') {
        vec![("relativePath", path)]
    } else {
        Vec::new()
    };
    rig.stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &rig.alice)
                .header(
                    "upload-metadata",
                    &metadata(&planned.session, &planned.item, &planned.name, &extra),
                )
                .header("upload-length", &length.to_string()),
        )
        .await
}

async fn push_item(rig: &Rig, planned: &Planned, path: &str, data: &[u8]) -> String {
    let created = create_upload(rig, planned, path, data.len()).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let upload = super::tus::upload_id_of(&created);
    if !data.is_empty() {
        let patched = rig
            .stack
            .tus_send(patch(&rig.alice, &upload, 0).bytes(data))
            .await;
        assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
    }
    upload
}

async fn placed(rig: &Rig) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for row in rig.file_rows().await {
        found.push((folder_path(&rig.stack, row.folder).await, row.name));
    }
    found.sort();
    found
}

#[tokio::test]
async fn it_directory_tree_retry_no_duplicate_tree() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let project = stack.make(&alice, "Project", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let files = [
        pathed("c1", "Project/docs/a.md", 10),
        pathed("c2", "Project/docs/img/b.png", 20),
        pathed("c3", "Project/c.txt", 5),
        pathed("c4", "top.txt", 1),
    ];
    let body = request_body(None, &files);

    let first = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text());
    let tree = folder_rows(&stack).await;
    let names: Vec<&str> = tree.iter().map(|row| row.1.as_str()).collect();
    assert_eq!(
        names,
        ["Project", "docs", "img"],
        "the existing root is reused"
    );
    assert_eq!(
        tree[0].0, project,
        "the pre-existing folder keeps its identity"
    );
    let counts = stack.counts().await;
    assert_eq!(
        (
            counts.sessions,
            counts.items,
            counts.reservations,
            counts.held
        ),
        (1, 4, 1, 36)
    );
    let session_one = first.json()["id"].as_str().unwrap().to_owned();
    let items_one = stack.item_ids(&session_one).await;

    let replay = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(
        replay.body, first.body,
        "the original session, byte for byte"
    );
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    assert_eq!(
        folder_rows(&stack).await,
        tree,
        "the same folder identities"
    );
    let replayed = stack.counts().await;
    assert_eq!(
        (
            replayed.sessions,
            replayed.items,
            replayed.reservations,
            replayed.held
        ),
        (1, 4, 1, 36)
    );

    let lost_response = stack.open(&alice, &body).await;
    assert_eq!(lost_response.status, StatusCode::CREATED);
    let session_two = lost_response.json()["id"].as_str().unwrap().to_owned();
    assert_ne!(session_two, session_one, "no key means a new operation");
    assert_ne!(stack.item_ids(&session_two).await, items_one);
    assert_eq!(
        folder_rows(&stack).await,
        tree,
        "a new session reuses the tree"
    );
    let counts = stack.counts().await;
    assert_eq!(
        (
            counts.sessions,
            counts.items,
            counts.reservations,
            counts.held
        ),
        (2, 8, 2, 72)
    );

    let canceled = stack.cancel_session(&alice, &session_one).await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );
    assert_eq!(folder_rows(&stack).await, tree, "cancel keeps the skeleton");
    let redrop = stack.open_keyed(&alice, &body, KEY_B).await;
    assert_eq!(redrop.status, StatusCode::CREATED, "{}", redrop.text());
    assert_eq!(
        folder_rows(&stack).await,
        tree,
        "re-dropping reuses the folders"
    );
    assert_eq!(duplicate_folder_rows(&stack).await, 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM folders WHERE name LIKE '%(%)%'")
            .await,
        0,
        "no keep-both suffix is ever applied to a folder"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_quota_whole_batch() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(1_000)).await;
    let folders = stack.counts().await.folders;

    let over = request_body(
        None,
        &[
            pathed("c1", "a/x.bin", 400),
            pathed("c2", "a/b/y.bin", 400),
            pathed("c3", "c/z.bin", 201),
        ],
    );
    let refused = stack.open_keyed(&alice, &over, KEY_A).await;
    assert_code(&refused, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    stack.assert_untouched(folders).await;
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM tus_uploads").await,
        0
    );

    let exact = request_body(
        None,
        &[
            pathed("c1", "a/x.bin", 400),
            pathed("c2", "a/b/y.bin", 400),
            pathed("c3", "c/z.bin", 200),
        ],
    );
    let admitted = stack.open_keyed(&alice, &exact, KEY_B).await;
    assert_eq!(admitted.status, StatusCode::CREATED, "{}", admitted.text());
    let counts = stack.counts().await;
    assert_eq!(
        (
            counts.sessions,
            counts.items,
            counts.reservations,
            counts.held
        ),
        (1, 3, 1, 1_000),
        "one session and one reservation for the whole tree"
    );
    assert_eq!(counts.folders, folders + 3, "a, a/b and c");

    let next = request_body(None, &[pathed("d1", "d/e/f.bin", 1)]);
    let refused = stack.open_keyed(&alice, &next, KEY_C).await;
    assert_code(&refused, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    let after = stack.counts().await;
    assert_eq!(
        (
            after.sessions,
            after.items,
            after.reservations,
            after.held,
            after.folders
        ),
        (1, 3, 1, 1_000, folders + 3),
        "the held bytes count and the refused tree leaves nothing"
    );

    let bob = stack.member("bob", HOST_B).await;
    stack.set_quota(bob.id, Some(1_000)).await;
    stack.set_max_file_size(300).await;
    let unknown = request_body(
        None,
        &[
            pathed("u1", "u/one.bin", 150),
            json!({ "clientId": "u2", "name": "two.bin", "sizeBytes": null, "relativePath": "u/two.bin" }),
            json!({ "clientId": "u3", "name": "three.bin", "relativePath": "u/v/three.bin" }),
            json!({ "clientId": "u4", "name": "four.bin", "relativePath": "u/four.bin" }),
        ],
    );
    let before = stack.counts().await;
    let refused = stack.open(&bob, &unknown).await;
    assert_code(&refused, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    let unchanged = stack.counts().await;
    assert_eq!(
        (
            unchanged.sessions,
            unchanged.items,
            unchanged.reservations,
            unchanged.folders
        ),
        (
            before.sessions,
            before.items,
            before.reservations,
            before.folders
        ),
        "150 + 3 x 300 exceeds 1000: each unknown size reserves the finite maximum"
    );
    let fits = request_body(
        None,
        &[
            pathed("u1", "u/one.bin", 100),
            json!({ "clientId": "u2", "name": "two.bin", "relativePath": "u/two.bin" }),
            json!({ "clientId": "u3", "name": "three.bin", "relativePath": "u/v/three.bin" }),
        ],
    );
    let admitted = stack.open(&bob, &fits).await;
    assert_eq!(admitted.status, StatusCode::CREATED, "{}", admitted.text());
    assert_eq!(admitted.json()["reservedBytes"], 700);
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_quota_overflow_checked() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folders = stack.counts().await.folders;
    let huge = 9_007_199_254_740_991_u64;

    for sizes in [[huge, huge], [huge, 1]] {
        let body = request_body(
            None,
            &[
                pathed("c1", "deep/er/one.bin", sizes[0]),
                pathed("c2", "deep/two.bin", sizes[1]),
            ],
        );
        let refused = stack.open(&alice, &body).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(field_codes(&refused), ["files"]);
        stack.assert_untouched(folders).await;
    }
    let single = stack
        .open(
            &alice,
            &request_body(None, &[pathed("c1", "deep/one.bin", huge)]),
        )
        .await;
    assert_eq!(single.status, StatusCode::CREATED, "{}", single.text());
    stack.stop().await;
}

fn hostile_cases() -> Vec<(String, String)> {
    let mut cases: Vec<(String, String)> = [
        "C:\\Windows\\system.ini",
        "c:/Windows/system.ini",
        "\\\\server\\share\\file.txt",
        "../secret.txt",
        "photos/../../secret.txt",
        "photos//file.txt",
        "/photos/file.txt",
        "\\photos\\file.txt",
        "photos/file.txt/",
        "photos/./file.txt",
        "photos/../file.txt",
        "photos/..\\file.txt",
        "a\u{0}b/file.txt",
        "a/b\u{0}/file.txt",
        "a/\u{85}b/file.txt",
        "a/\u{7}b/file.txt",
        "a/\u{9f}/file.txt",
        "a/b\u{200b}\u{0}/file.txt",
    ]
    .iter()
    .map(|path| ((*path).to_owned(), leaf_of(path).trim().to_owned()))
    .map(|(path, leaf)| {
        let leaf = if leaf.is_empty() {
            "file.txt".to_owned()
        } else {
            leaf
        };
        (path, leaf)
    })
    .collect();
    let deep = (0..32).map(|_| "d").collect::<Vec<_>>().join("/");
    cases.push((format!("{deep}/file.txt"), "file.txt".to_owned()));
    cases.push((
        format!("{}/file.txt", "s".repeat(256)),
        "file.txt".to_owned(),
    ));
    let long = (0..9)
        .map(|_| "x".repeat(120))
        .collect::<Vec<_>>()
        .join("/");
    cases.push((format!("{long}/file.txt"), "file.txt".to_owned()));
    cases
}

#[tokio::test]
async fn it_directory_tree_hostile_paths_rejected() {
    for s3 in [false, true] {
        let root = TempDir::new().unwrap();
        let clock = TestClock::new(START);
        let stack = if s3 {
            Stack::start_with_storage(
                root.path(),
                &clock,
                s3_stack_storage(ProviderProfile::Minio, false),
            )
            .await
        } else {
            Stack::start(root.path(), &clock).await
        };
        let alice = stack.member("alice", HOST_A).await;
        let folders = stack.counts().await.folders;
        let valid = |id: &str| pathed(id, &format!("ok/{id}/file.bin"), 1);

        for (path, leaf) in hostile_cases() {
            let hostile = file_json("bad", &leaf, &path, 1);
            for position in 0..3 {
                let mut files = vec![valid("v0"), valid("v1")];
                files.insert(position.min(files.len()), hostile.clone());
                let refused = stack.open(&alice, &request_body(None, &files)).await;
                let label = format!("{path:?} at {position} (s3: {s3})");
                assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{label}");
                assert_eq!(refused.error_code(), "VALIDATION_ERROR", "{label}");
                assert_eq!(field_codes(&refused), ["relativePath"], "{label}");
                stack.assert_untouched(folders).await;
            }
        }

        let mismatch = request_body(
            None,
            &[
                valid("v0"),
                file_json("m", "other.jpg", "a/photo.jpg", 1),
                valid("v1"),
            ],
        );
        let refused = stack.open(&alice, &mismatch).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(field_codes(&refused), ["name", "relativePath"]);
        let case_only = request_body(None, &[file_json("m", "Photo.jpg", "a/photo.jpg", 1)]);
        let refused = stack.open(&alice, &case_only).await;
        assert_eq!(field_codes(&refused), ["name", "relativePath"]);
        let separator_in_name =
            request_body(None, &[file_json("m", "a/photo.jpg", "a/photo.jpg", 1)]);
        let refused = stack.open(&alice, &separator_in_name).await;
        assert_eq!(refused.error_code(), "NAME_INVALID");
        stack.assert_untouched(folders).await;
        assert_eq!(stack.tus_count().await, 0);
        assert_eq!(
            stack
                .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
                .await,
            0
        );
        assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM jobs").await, 0);
        stack.stop().await;
    }
}

#[tokio::test]
async fn it_directory_tree_windows_and_unicode_paths_are_canonicalized() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let deep_ok = (0..31).map(|_| "d").collect::<Vec<_>>().join("/");

    let created = stack
        .open(
            &alice,
            &request_body(
                None,
                &[
                    file_json("c1", "photo.jpg", "Photos\\2026\\photo.jpg", 1),
                    file_json("c2", "photo.jpg", "caf\u{e9}/photo.jpg", 1),
                    file_json("c3", "photo.jpg", "cafe\u{301}/photo.jpg", 1),
                    file_json("c4", "COM1", "CON/NUL/COM1", 1),
                    file_json("c5", "README.md", "README.md", 1),
                    file_json("c6", "file.txt", &format!("{deep_ok}/file.txt"), 1),
                ],
            ),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let stored: Vec<(String, String)> = sqlx::query_as(
        "SELECT relative_path, display_name FROM transfer_session_files ORDER BY ordinal",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        stored[..5],
        [
            ("Photos/2026".to_owned(), "photo.jpg".to_owned()),
            ("caf\u{e9}".to_owned(), "photo.jpg".to_owned()),
            ("caf\u{e9}".to_owned(), "photo.jpg".to_owned()),
            ("CON/NUL".to_owned(), "COM1".to_owned()),
            (String::new(), "README.md".to_owned()),
        ],
        "the column holds the directory prefix, never the leaf"
    );
    assert_eq!(stored[5].0.split('/').count(), 31);
    let names: Vec<String> = folder_rows(&stack)
        .await
        .into_iter()
        .map(|row| row.1)
        .filter(|name| name != "d")
        .collect();
    assert_eq!(
        names,
        ["CON", "Photos", "caf\u{e9}", "2026", "NUL"],
        "no folder carries a leaf name, and the NFC pair is one folder"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM folders WHERE name IN ('photo.jpg', 'README.md', 'COM1', 'file.txt')")
            .await,
        0
    );
    assert_eq!(duplicate_folder_rows(&stack).await, 0);
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_target_folder_depth_exceeded() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let deep = (0..31).map(|_| "d").collect::<Vec<_>>().join("/");
    let path = format!("{deep}/file.txt");
    let mut chain = Vec::new();
    let mut parent: Option<String> = None;
    for depth in 0..=34_u8 {
        let id = stack
            .seed_folder(alice.id, parent.as_deref(), &format!("t{depth}"), depth)
            .await;
        chain.push(id.clone());
        parent = Some(id);
    }
    let folders = stack.counts().await.folders;

    let too_deep = stack
        .open(&alice, &request_in(&chain[34], &[pathed("c1", &path, 1)]))
        .await;
    assert_code(
        &too_deep,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    assert_eq!(stack.counts().await.folders, folders, "no partial chain");
    assert_eq!(stack.counts().await.sessions, 0);

    let fits = stack
        .open(&alice, &request_in(&chain[33], &[pathed("c1", &path, 1)]))
        .await;
    assert_eq!(fits.status, StatusCode::CREATED, "{}", fits.text());
    let deepest: i64 = stack.scalar_i64("SELECT MAX(depth) FROM folders").await;
    assert_eq!(deepest, 64, "exactly the database limit");
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_client_key_conflicts_never_alias_items() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folders = stack.counts().await.folders;

    let duplicate = request_body(
        None,
        &[pathed("same", "a/x.bin", 1), pathed("same", "b/x.bin", 1)],
    );
    let refused = stack.open(&alice, &duplicate).await;
    assert_code(
        &refused,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(field_codes(&refused), ["clientId"]);
    stack.assert_untouched(folders).await;

    let body = request_body(
        None,
        &[pathed("k1", "a/x.bin", 1), pathed("k2", "b/x.bin", 1)],
    );
    let created = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let dump = stack.dump().await;
    for changed in [
        request_body(
            None,
            &[pathed("k1", "c/x.bin", 1), pathed("k2", "b/x.bin", 1)],
        ),
        request_body(
            None,
            &[pathed("k1", "a/x.bin", 2), pathed("k2", "b/x.bin", 1)],
        ),
        request_body(
            None,
            &[pathed("k2", "a/x.bin", 1), pathed("k1", "b/x.bin", 1)],
        ),
    ] {
        let conflict = stack.open_keyed(&alice, &changed, KEY_A).await;
        assert_code(&conflict, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");
    }
    assert_eq!(stack.dump().await, dump, "a reused key aliases nothing");
    assert_eq!(stack.counts().await.folders, folders + 2);

    let session = created.json()["id"].as_str().unwrap().to_owned();
    let duplicate_row = execute_fallible(
        &stack,
        &format!(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
                  declared_size_bytes, reserved_bytes, upload_kind, final_object_id,
                  final_object_key, created_at, updated_at)
             SELECT '{}', transfer_session_id, 9, client_file_key, display_name, relative_path,
                    declared_size_bytes, reserved_bytes, upload_kind, '{}', 'objects/00/00/{}',
                    created_at, updated_at
               FROM transfer_session_files WHERE transfer_session_id = '{session}' LIMIT 1",
            stack.fresh_id(),
            stack.fresh_id(),
            "0".repeat(32),
        ),
    )
    .await;
    assert!(
        duplicate_row.is_err(),
        "ux_tsf_session_client_key is the last guard"
    );
    assert_eq!(stack.counts().await.items, 2);
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_cross_owner_and_unavailable_targets_are_not_found() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let alices = stack.make(&alice, "Private", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stack.make(&alice, "Shared", None).await;
    let folders = stack.counts().await.folders;

    let foreign = stack
        .open(
            &bob,
            &request_in(&alices, &[pathed("c1", "x/y/file.bin", 1)]),
        )
        .await;
    let missing = stack
        .open(
            &bob,
            &request_in(&stack.fresh_id(), &[pathed("c1", "x/y/file.bin", 1)]),
        )
        .await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    assert_eq!(
        foreign.error_without_request_id(),
        missing.error_without_request_id(),
        "a foreign folder is indistinguishable from a missing one"
    );
    stack.assert_untouched(folders).await;

    let own = stack
        .open(
            &bob,
            &request_body(None, &[pathed("c1", "Private/Shared/file.bin", 1)]),
        )
        .await;
    assert_eq!(own.status, StatusCode::CREATED, "{}", own.text());
    let owners: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT owner_id, name, parent_id FROM folders WHERE owner_id = ?1 ORDER BY depth",
    )
    .bind(bob.id.to_string())
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(owners.len(), 2, "Bob builds his own chain");
    assert!(
        owners[0].2.is_none(),
        "it hangs from Bob's root, not Alice's folder"
    );
    let alice_rows: i64 = stack
        .scalar_i64(&format!(
            "SELECT COUNT(*) FROM folders WHERE owner_id = '{}'",
            alice.id
        ))
        .await;
    assert_eq!(alice_rows, 2, "Alice's folders are untouched");
    assert!(!own.text().contains(&alices));

    stack
        .execute(&format!(
            "UPDATE folders SET deleting = 1 WHERE id = '{alices}'"
        ))
        .await;
    let before = stack.counts().await;
    let deleting = stack
        .open(
            &alice,
            &request_in(&alices, &[pathed("c1", "x/file.bin", 1)]),
        )
        .await;
    assert_code(&deleting, StatusCode::CONFLICT, "FOLDER_DELETING");
    let after = stack.counts().await;
    assert_eq!(
        (
            after.sessions,
            after.items,
            after.reservations,
            after.folders
        ),
        (
            before.sessions,
            before.items,
            before.reservations,
            before.folders
        )
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_admission_rollback_at_every_position() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(100_000)).await;
    let folders = stack.counts().await.folders;
    let files: Vec<Value> = (0..40)
        .map(|index| {
            pathed(
                &format!("c{index}"),
                &format!("Tree/Dir{}/Leaf{index}/item-{index}.bin", index % 5),
                10,
            )
        })
        .collect();
    let body = request_body(None, &files);
    let triggers = [
        (
            "first item",
            "BEFORE INSERT ON transfer_session_files WHEN NEW.display_name = 'item-0.bin'",
        ),
        (
            "middle item",
            "BEFORE INSERT ON transfer_session_files WHEN NEW.display_name = 'item-20.bin'",
        ),
        (
            "final item",
            "BEFORE INSERT ON transfer_session_files WHEN NEW.display_name = 'item-39.bin'",
        ),
        (
            "last folder",
            "BEFORE INSERT ON folders WHEN NEW.name = 'Leaf39'",
        ),
        (
            "first folder",
            "BEFORE INSERT ON folders WHEN NEW.name = 'Tree'",
        ),
        ("session row", "BEFORE INSERT ON transfer_sessions"),
        ("reservation row", "BEFORE INSERT ON quota_reservations"),
    ];
    for (label, trigger) in triggers {
        stack
            .execute(&format!(
                "CREATE TRIGGER inject_failure {trigger}
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END"
            ))
            .await;
        let failed = stack.open_keyed(&alice, &body, KEY_A).await;
        assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR, "{label}");
        stack.assert_untouched(folders).await;
        assert_eq!(stack.tus_count().await, 0, "{label}");
        assert_eq!(
            stack.scalar_i64("SELECT COUNT(*) FROM jobs").await,
            0,
            "{label}"
        );
        stack.execute("DROP TRIGGER inject_failure").await;
    }
    let recovered = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(
        recovered.status,
        StatusCode::CREATED,
        "{}",
        recovered.text()
    );
    let counts = stack.counts().await;
    assert_eq!(
        (
            counts.sessions,
            counts.items,
            counts.reservations,
            counts.held
        ),
        (1, 40, 1, 400)
    );
    assert_eq!(counts.folders, folders + 1 + 5 + 40);
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_concurrent_admission_shares_one_tree() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let body = request_body(
        None,
        &[
            pathed("c1", "Photos/2026/a.jpg", 10),
            pathed("c2", "photos/2026/b.jpg", 10),
            pathed("c3", "Photos/2025/c.jpg", 10),
        ],
    );

    let same_key = join_all((0..5).map(|_| stack.open_keyed(&alice, &body, KEY_A))).await;
    let originals: Vec<&Fetched> = same_key
        .iter()
        .filter(|fetched| {
            fetched.status == StatusCode::CREATED
                && fetched.headers.get("idempotency-replayed").is_none()
        })
        .collect();
    assert_eq!(originals.len(), 1, "one logical session");
    for fetched in &same_key {
        match fetched.status {
            StatusCode::CREATED => assert_eq!(fetched.body, originals[0].body),
            StatusCode::CONFLICT => {
                assert_eq!(fetched.error_code(), "IDEMPOTENCY_REQUEST_IN_PROGRESS");
            }
            other => panic!("unexpected {other}: {}", fetched.text()),
        }
    }
    let counts = stack.counts().await;
    assert_eq!(
        (
            counts.sessions,
            counts.items,
            counts.reservations,
            counts.held
        ),
        (1, 3, 1, 30)
    );
    let tree = folder_rows(&stack).await;
    assert_eq!(tree.len(), 3, "Photos, 2026 and 2025");

    let distinct = join_all((0..4).map(|index| {
        let key = format!("distinct-key-{index:04}-{}", "x".repeat(16));
        let stack = &stack;
        let alice = &alice;
        let body = &body;
        async move { stack.open_keyed(alice, body, &key).await }
    }))
    .await;
    let mut sessions = Vec::new();
    for fetched in &distinct {
        assert_eq!(fetched.status, StatusCode::CREATED, "{}", fetched.text());
        sessions.push(fetched.json()["id"].as_str().unwrap().to_owned());
    }
    sessions.sort();
    sessions.dedup();
    assert_eq!(sessions.len(), 4, "independent sessions");
    let mut items = Vec::new();
    for session in &sessions {
        items.extend(stack.item_ids(session).await);
    }
    items.sort();
    items.dedup();
    assert_eq!(items.len(), 12, "independent item identities");
    assert_eq!(
        folder_rows(&stack).await,
        tree,
        "the same folder rows are reused"
    );
    assert_eq!(duplicate_folder_rows(&stack).await, 0);
    let counts = stack.counts().await;
    assert_eq!(
        (
            counts.sessions,
            counts.items,
            counts.reservations,
            counts.held
        ),
        (5, 15, 5, 150)
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_directory_tree_unknown_length_items_follow_the_deferred_length_contract() {
    let rig = Rig::new().await;
    let created = rig
        .stack
        .open(
            &rig.alice,
            &request_body(
                None,
                &[
                    json!({ "clientId": "u1", "name": "u.bin", "relativePath": "stream/u.bin" }),
                    pathed("c2", "stream/k.bin", 3),
                ],
            ),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(
        created.json()["reservedBytes"],
        3,
        "Unlimited keeps the running check"
    );
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let item = rig.stack.item_ids(&session).await.remove(0);
    let planned = Planned {
        session,
        item,
        name: "u.bin".to_owned(),
    };
    let deferred = rig
        .stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &rig.alice)
                .header(
                    "upload-metadata",
                    &metadata(
                        &planned.session,
                        &planned.item,
                        &planned.name,
                        &[("relativePath", "stream/u.bin")],
                    ),
                )
                .header("upload-defer-length", "1"),
        )
        .await;
    assert_eq!(deferred.status, StatusCode::CREATED, "{}", deferred.text());
    assert_eq!(rig.stack.counts().await.folders, 1);
}

#[tokio::test]
async fn it_directory_tree_tus_finalization() {
    let rig = Rig::new().await;
    let files = [
        ("Photos/2026/Iceland/photo.jpg", 5_000_usize),
        ("Photos/2026/Iceland/map.png", 1_200),
        ("Photos/2025/c.jpg", 700),
        ("README.md", 300),
        ("docs", 90),
        ("docs/inner.txt", 40),
    ];
    let planned = open_tree(&rig, None, &files).await;
    let tree = folder_rows(&rig.stack).await;
    assert_eq!(
        tree.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
        ["Photos", "docs", "2025", "2026", "Iceland"],
        "the hierarchy exists before a single byte moves"
    );
    assert_eq!(rig.stack.tus_count().await, 0);

    let other = rig.stack.make(&rig.alice, "Other", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let tree = folder_rows(&rig.stack).await;
    let foreign_folder = rig
        .stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &rig.alice)
                .header(
                    "upload-metadata",
                    &metadata(
                        &planned[0].session,
                        &planned[0].item,
                        &planned[0].name,
                        &[("relativePath", files[0].0), ("folderId", &other)],
                    ),
                )
                .header("upload-length", "5000"),
        )
        .await;
    assert_eq!(foreign_folder.status, StatusCode::BAD_REQUEST);
    assert_eq!(foreign_folder.json()["error"]["details"]["key"], "folderId");
    let inconsistent =
        create_upload(&rig, &planned[0], "Photos/2026/elsewhere/photo.jpg", 5_000).await;
    assert_eq!(inconsistent.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        inconsistent.json()["error"]["details"]["key"],
        "relativePath"
    );
    assert_eq!(rig.stack.tus_count().await, 0);

    let mut total = 0;
    for (planned, (path, size)) in planned.iter().zip(&files) {
        let data = bytes(*size);
        push_item(&rig, planned, path, &data).await;
        rig.settle().await;
        assert_eq!(rig.item_row(&planned.item).await.0, "completed", "{path}");
        assert_eq!(
            Rig::digest_of(&rig.final_path(&planned.item).await).unwrap(),
            (*size as u64, Rig::digest(&data)),
            "{path}"
        );
        total += i64::try_from(*size).unwrap();
    }
    assert_eq!(
        placed(&rig).await,
        [
            (String::new(), "README.md".to_owned()),
            (String::new(), "docs".to_owned()),
            ("Photos/2025".to_owned(), "c.jpg".to_owned()),
            ("Photos/2026/Iceland".to_owned(), "map.png".to_owned()),
            ("Photos/2026/Iceland".to_owned(), "photo.jpg".to_owned()),
            ("docs".to_owned(), "inner.txt".to_owned()),
        ],
        "a folder and a file may share a name"
    );
    assert_eq!(
        folder_rows(&rig.stack).await,
        tree,
        "finalization creates no folder"
    );
    assert_eq!(rig.used().await, total);
    assert_eq!(rig.audit_uploads().await, 6);
    assert_eq!(rig.stack.counts().await.objects, 6);
    let session = &planned[0].session;
    assert_eq!(rig.stack.session_row(session).await.0, "completed");
    let (state, reserved, committed, _) = rig.stack.reservation(session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("committed", 0, Some(total))
    );
    for row in rig.file_rows().await {
        let key = rig.final_key(&planned[0].item).await;
        assert!(key.starts_with("objects/"), "{key}");
        assert!(
            !key.contains(&row.name),
            "no name or path reaches a storage key"
        );
    }
}

#[tokio::test]
async fn it_directory_tree_same_leaf_in_different_paths_and_keep_both_at_finalization() {
    let rig = Rig::new().await;
    let first = [
        ("a/same.txt", 100_usize),
        ("b/same.txt", 200),
        ("a/photo.jpg", 10),
        ("a/.env", 5),
        ("a/archive.tar.gz", 7),
    ];
    let planned = open_tree(&rig, None, &first).await;
    let items: std::collections::HashSet<&str> =
        planned.iter().map(|item| item.item.as_str()).collect();
    assert_eq!(items.len(), 5, "same leaf names are distinct items");
    for (planned, (path, size)) in planned.iter().zip(&first) {
        push_item(&rig, planned, path, &bytes(*size)).await;
        rig.settle().await;
    }
    let tree = folder_rows(&rig.stack).await;
    assert_eq!(tree.len(), 2);
    let sizes: Vec<(String, i64)> = {
        let mut rows = Vec::new();
        for row in rig.file_rows().await {
            let folder = folder_path(&rig.stack, row.folder.clone()).await;
            if row.name == "same.txt" {
                rows.push((folder, row.size));
            }
        }
        rows.sort();
        rows
    };
    assert_eq!(sizes, [("a".to_owned(), 100), ("b".to_owned(), 200)]);

    let second = [
        ("a/photo.jpg", 11_usize),
        ("a/Photo.jpg", 12),
        ("a/.env", 6),
        ("a/archive.tar.gz", 8),
        ("b/same.txt", 9),
    ];
    let planned = open_tree(&rig, None, &second).await;
    assert_eq!(
        folder_rows(&rig.stack).await,
        tree,
        "the second drop reuses both folders"
    );
    for (planned, (path, size)) in planned.iter().zip(&second) {
        push_item(&rig, planned, path, &bytes(*size)).await;
        rig.settle().await;
        assert_eq!(rig.item_row(&planned.item).await.0, "completed", "{path}");
    }
    let expected: Vec<(String, String)> = [
        ("a", ".env"),
        ("a", ".env (1)"),
        ("a", "Photo (2).jpg"),
        ("a", "archive.tar (1).gz"),
        ("a", "archive.tar.gz"),
        ("a", "photo (1).jpg"),
        ("a", "photo.jpg"),
        ("a", "same.txt"),
        ("b", "same.txt"),
        ("b", "same (1).txt"),
    ]
    .iter()
    .map(|(folder, name)| ((*folder).to_owned(), (*name).to_owned()))
    .collect();
    let mut expected = expected;
    expected.sort();
    assert_eq!(placed(&rig).await, expected);
    assert_eq!(
        folder_rows(&rig.stack).await,
        tree,
        "keep-both never touches folders"
    );
    assert_eq!(duplicate_folder_rows(&rig.stack).await, 0);
}

#[tokio::test]
async fn it_directory_tree_restart_preserves_hierarchy() {
    let rig = Rig::new().await;
    let files = [
        ("t/one/a.bin", 6_000_usize),
        ("t/two/b.bin", 7_000),
        ("t/two/deeper/c.bin", 8_000),
    ];
    let planned = open_tree(&rig, None, &files).await;
    let tree = folder_rows(&rig.stack).await;
    let identities: Vec<String> = rig.stack.item_ids(&planned[0].session).await;

    push_item(&rig, &planned[0], files[0].0, &bytes(files[0].1)).await;
    rig.settle().await;
    rig.hooks.crash_at(Point::BeforeCommit);
    push_item(&rig, &planned[1], files[1].0, &bytes(files[1].1)).await;
    rig.settle().await;
    assert_eq!(rig.item_row(&planned[1].item).await.0, "finalizing");

    let data = bytes(files[2].1);
    let upload = create_upload(&rig, &planned[2], files[2].0, data.len()).await;
    let upload = super::tus::upload_id_of(&upload);
    let half = rig
        .stack
        .tus_send(patch(&rig.alice, &upload, 0).bytes(&data[..3_000]))
        .await;
    assert_eq!(half.status, StatusCode::NO_CONTENT, "{}", half.text());

    rig.restart().await;
    assert_eq!(rig.item_row(&planned[1].item).await.0, "completed");
    assert_eq!(
        folder_rows(&rig.stack).await,
        tree,
        "the restart rebuilt nothing"
    );
    assert_eq!(rig.stack.item_ids(&planned[0].session).await, identities);
    assert_eq!(
        super::tus_patch::head_offset(&rig.stack, &rig.alice, &upload).await,
        3_000,
        "the authoritative offset survives"
    );
    let rest = rig
        .stack
        .tus_send(patch(&rig.alice, &upload, 3_000).bytes(&data[3_000..]))
        .await;
    assert_eq!(rest.status, StatusCode::NO_CONTENT, "{}", rest.text());
    rig.settle().await;
    assert_eq!(
        placed(&rig).await,
        [
            ("t/one".to_owned(), "a.bin".to_owned()),
            ("t/two".to_owned(), "b.bin".to_owned()),
            ("t/two/deeper".to_owned(), "c.bin".to_owned()),
        ]
    );
    assert_eq!(folder_rows(&rig.stack).await, tree);
    assert_eq!(rig.stack.counts().await.sessions, 1);
    assert_eq!(
        rig.stack.session_row(&planned[0].session).await.0,
        "completed"
    );
}

#[tokio::test]
async fn it_directory_tree_original_target_folder_immutable() {
    let rig = Rig::new().await;
    let target = rig.stack.make(&rig.alice, "Target", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let elsewhere = rig.stack.make(&rig.alice, "Elsewhere", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let files = [("t/x.bin", 900_usize), ("t/y.bin", 800)];
    let planned = open_tree(&rig, Some(&target), &files).await;
    let leaf: String = sqlx::query_scalar("SELECT id FROM folders WHERE name = 't'")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();

    let redirected = rig
        .stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &rig.alice)
                .header(
                    "upload-metadata",
                    &metadata(
                        &planned[0].session,
                        &planned[0].item,
                        "x.bin",
                        &[("relativePath", "t/x.bin"), ("folderId", &elsewhere)],
                    ),
                )
                .header("upload-length", "900"),
        )
        .await;
    assert_eq!(redirected.status, StatusCode::BAD_REQUEST);
    assert_eq!(redirected.json()["error"]["details"]["key"], "folderId");

    push_item(&rig, &planned[0], files[0].0, &bytes(files[0].1)).await;
    rig.settle().await;
    let moved = rig
        .stack
        .api(
            Method::POST,
            &format!("{FOLDERS}/{target}/move"),
            &rig.alice,
            Some(&json!({ "parentId": elsewhere })),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    let renamed = rig
        .stack
        .edit(&rig.alice, &target, &json!({ "name": "Renamed" }))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    push_item(&rig, &planned[1], files[1].0, &bytes(files[1].1)).await;
    rig.settle().await;

    let rows = rig.file_rows().await;
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|row| row.folder.as_deref() == Some(leaf.as_str())));
    assert_eq!(
        folder_path(&rig.stack, Some(leaf)).await,
        "Elsewhere/Renamed/t",
        "the session follows the folder it captured, wherever it now lives"
    );
    let session = rig.stack.session_row(&planned[0].session).await;
    assert_eq!(session.1.as_deref(), Some(target.as_str()));

    let bob = rig.stack.member("bob", HOST_B).await;
    let foreign = rig
        .stack
        .open(&bob, &request_in(&target, &[pathed("c1", "t/z.bin", 1)]))
        .await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
}

#[tokio::test]
async fn it_directory_tree_partial_failure_keeps_completed_files() {
    let rig = Rig::new().await;
    let files = [
        ("p/ok/one.bin", 1_000_usize),
        ("p/gone/two.bin", 2_000),
        ("p/ok/three.bin", 3_000),
    ];
    let planned = open_tree(&rig, None, &files).await;
    let tree = folder_rows(&rig.stack).await;
    let session = planned[0].session.clone();
    let mut uploads = Vec::new();
    for (planned, (path, size)) in planned.iter().zip(&files) {
        let created = create_upload(&rig, planned, path, *size).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        uploads.push(super::tus::upload_id_of(&created));
    }
    let send = |index: usize| {
        let data = bytes(files[index].1);
        let upload = uploads[index].clone();
        let rig = &rig;
        async move {
            rig.stack
                .tus_send(patch(&rig.alice, &upload, 0).bytes(&data))
                .await
        }
    };
    let first = send(0).await;
    assert_eq!(first.status, StatusCode::NO_CONTENT);
    rig.settle().await;

    let gone: String = sqlx::query_scalar("SELECT id FROM folders WHERE name = 'gone'")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    let renamed = rig
        .stack
        .edit(&rig.alice, &gone, &json!({ "name": "moved-away" }))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    let folders_before = rig.stack.counts().await.folders;

    let second = send(1).await;
    assert_eq!(second.status, StatusCode::NO_CONTENT, "{}", second.text());
    rig.settle().await;
    assert_eq!(
        rig.failed_with(&planned[1].item).await,
        "FOLDER_NOT_FOUND",
        "the admitted folder is never silently recreated"
    );
    assert_eq!(rig.stack.counts().await.folders, folders_before);
    assert_eq!(
        rig.stack
            .scalar_i64("SELECT COUNT(*) FROM folders WHERE name = 'gone'")
            .await,
        0
    );
    assert!(
        !rig.final_path(&planned[1].item).await.exists(),
        "no blob was placed"
    );
    assert_eq!(
        placed(&rig).await,
        [("p/ok".to_owned(), "one.bin".to_owned())],
        "the completed sibling is untouched by the failure"
    );
    assert_eq!(rig.used().await, 1_000);
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("held", 3_000, None),
        "only the live sibling's share is still held"
    );

    let canceled = rig.stack.cancel_session(&rig.alice, &session).await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );
    assert_eq!(
        rig.stack.item_states(&session).await,
        ["completed", "canceled", "canceled"]
    );
    let (state, _, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((state.as_str(), committed), ("committed", Some(1_000)));
    assert_eq!(
        placed(&rig).await,
        [("p/ok".to_owned(), "one.bin".to_owned())],
        "cancel keeps the completed file"
    );
    assert_eq!(rig.used().await, 1_000);
    assert_eq!(folder_rows(&rig.stack).await.len(), tree.len());
    assert_eq!(rig.stack.counts().await.folders, folders_before);
}

async fn rename_folder(rig: &Rig, name: &str, to: &str) {
    let id: String = sqlx::query_scalar("SELECT id FROM folders WHERE name = ?1")
        .bind(name)
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    let renamed = rig
        .stack
        .edit(&rig.alice, &id, &json!({ "name": to }))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
}

struct Counts {
    files: i64,
    objects: i64,
    uploads: i64,
    used: i64,
}

async fn tally(rig: &Rig) -> Counts {
    Counts {
        files: rig.stack.scalar_i64("SELECT COUNT(*) FROM files").await,
        objects: rig.stack.counts().await.objects,
        uploads: rig.audit_uploads().await,
        used: rig.used().await,
    }
}

async fn assert_two_of_three(rig: &Rig, session: &str, tree_len: usize) {
    let counts = tally(rig).await;
    assert_eq!(
        (counts.files, counts.objects, counts.uploads, counts.used),
        (2, 2, 2, 4_000),
        "one file, one object, one audit event and one charge per completed item"
    );
    assert_eq!(
        placed(rig).await,
        [
            ("d/ok".to_owned(), "a.bin".to_owned()),
            ("d/ok".to_owned(), "c.bin".to_owned()),
        ]
    );
    assert_eq!(rig.stack.counts().await.sessions, 1, "no second session");
    assert_eq!(folder_rows(&rig.stack).await.len(), tree_len);
    assert_eq!(
        rig.stack.item_states(session).await,
        ["completed", "failed", "completed"]
    );
    let row: (i64, i64) = sqlx::query_as(
        "SELECT completed_file_count, completed_bytes FROM transfer_sessions WHERE id = ?1",
    )
    .bind(session)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(row, (2, 4_000));
}

#[tokio::test]
async fn it_directory_tree_failed_item_does_not_strand_a_pending_sibling() {
    let rig = Rig::new().await;
    let files = [
        ("d/ok/a.bin", 1_000_usize),
        ("d/gone/b.bin", 2_000),
        ("d/ok/c.bin", 3_000),
    ];
    let planned = open_tree(&rig, None, &files).await;
    let session = planned[0].session.clone();
    let tree_len = folder_rows(&rig.stack).await.len();

    push_item(&rig, &planned[0], files[0].0, &bytes(files[0].1)).await;
    rig.settle().await;
    assert_eq!(rig.item_row(&planned[0].item).await.0, "completed");

    rename_folder(&rig, "gone", "moved-away").await;
    push_item(&rig, &planned[1], files[1].0, &bytes(files[1].1)).await;
    rig.settle().await;
    assert_eq!(rig.failed_with(&planned[1].item).await, "FOLDER_NOT_FOUND");
    assert_eq!(
        rig.stack.session_row(&session).await.0,
        "uploading",
        "a sibling is still pending, so the session keeps running"
    );
    assert_eq!(
        placed(&rig).await,
        [("d/ok".to_owned(), "a.bin".to_owned())],
        "the completed sibling is untouched"
    );
    let (state, reserved, _, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved),
        ("held", 3_000),
        "only the pending sibling's share remains held"
    );
    assert_eq!(rig.used().await, 1_000);

    push_item(&rig, &planned[2], files[2].0, &bytes(files[2].1)).await;
    rig.settle().await;
    assert_eq!(rig.item_row(&planned[2].item).await.0, "completed");
    assert_eq!(
        rig.stack.session_row(&session).await.0,
        "failed",
        "nothing is live any more and one item failed, so the aggregate is failed"
    );
    assert_two_of_three(&rig, &session, tree_len).await;
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((state.as_str(), reserved), ("held", 0), "{committed:?}");

    let closed = rig.stack.complete_session(&rig.alice, &session).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.text());
    assert_eq!(rig.stack.session_row(&session).await.0, "completed");
    let (state, _, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((state.as_str(), committed), ("committed", Some(4_000)));
    assert_two_of_three(&rig, &session, tree_len).await;

    rig.restart().await;
    assert_two_of_three(&rig, &session, tree_len).await;
    assert_eq!(rig.stack.session_row(&session).await.0, "completed");
    let (state, _, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((state.as_str(), committed), ("committed", Some(4_000)));
}

#[tokio::test]
async fn it_directory_tree_failed_item_does_not_stop_an_uploading_sibling() {
    let rig = Rig::new().await;
    let files = [
        ("d/ok/a.bin", 1_000_usize),
        ("d/gone/b.bin", 2_000),
        ("d/ok/c.bin", 3_000),
    ];
    let planned = open_tree(&rig, None, &files).await;
    let session = planned[0].session.clone();
    let tree_len = folder_rows(&rig.stack).await.len();
    let mut uploads = Vec::new();
    for (planned, (path, size)) in planned.iter().zip(&files) {
        let created = create_upload(&rig, planned, path, *size).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        uploads.push(super::tus::upload_id_of(&created));
    }
    let data = |index: usize| bytes(files[index].1);

    let half = rig
        .stack
        .tus_send(patch(&rig.alice, &uploads[2], 0).bytes(&data(2)[..1_500]))
        .await;
    assert_eq!(half.status, StatusCode::NO_CONTENT, "{}", half.text());
    let sent = rig
        .stack
        .tus_send(patch(&rig.alice, &uploads[0], 0).bytes(&data(0)))
        .await;
    assert_eq!(sent.status, StatusCode::NO_CONTENT);
    rig.settle().await;
    rename_folder(&rig, "gone", "moved-away").await;
    let doomed = rig
        .stack
        .tus_send(patch(&rig.alice, &uploads[1], 0).bytes(&data(1)))
        .await;
    assert_eq!(doomed.status, StatusCode::NO_CONTENT);
    rig.settle().await;
    assert_eq!(rig.failed_with(&planned[1].item).await, "FOLDER_NOT_FOUND");
    assert_eq!(rig.stack.session_row(&session).await.0, "uploading");

    let rest = rig
        .stack
        .tus_send(patch(&rig.alice, &uploads[2], 1_500).bytes(&data(2)[1_500..]))
        .await;
    assert_eq!(rest.status, StatusCode::NO_CONTENT, "{}", rest.text());
    rig.settle().await;
    assert_eq!(rig.stack.session_row(&session).await.0, "failed");
    assert_two_of_three(&rig, &session, tree_len).await;

    let canceled = rig
        .stack
        .cancel_item(&rig.alice, &session, &planned[1].item)
        .await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );
    rig.restart().await;
    assert_eq!(
        placed(&rig).await,
        [
            ("d/ok".to_owned(), "a.bin".to_owned()),
            ("d/ok".to_owned(), "c.bin".to_owned()),
        ]
    );
    assert_eq!(tally(&rig).await.uploads, 2);
}

#[tokio::test]
async fn it_directory_tree_double_dot_components_are_valid_and_persisted() {
    let rig = Rig::new().await;
    let files = [
        ("versions..old/file.txt", 10_usize),
        ("project...backup/readme.md", 20),
        ("a/..hidden/b.txt", 30),
        ("a/mid..dle/deeper...x/c.txt", 40),
    ];
    let planned = open_tree(&rig, None, &files).await;
    let stored: Vec<(String, String)> = sqlx::query_as(
        "SELECT relative_path, display_name FROM transfer_session_files ORDER BY ordinal",
    )
    .fetch_all(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        stored,
        [
            ("versions..old".to_owned(), "file.txt".to_owned()),
            ("project...backup".to_owned(), "readme.md".to_owned()),
            ("a/..hidden".to_owned(), "b.txt".to_owned()),
            ("a/mid..dle/deeper...x".to_owned(), "c.txt".to_owned()),
        ]
    );
    for (planned, (path, size)) in planned.iter().zip(&files) {
        push_item(&rig, planned, path, &bytes(*size)).await;
        rig.settle().await;
        assert_eq!(rig.item_row(&planned.item).await.0, "completed", "{path}");
    }
    assert_eq!(
        placed(&rig).await,
        [
            ("a/..hidden".to_owned(), "b.txt".to_owned()),
            ("a/mid..dle/deeper...x".to_owned(), "c.txt".to_owned()),
            ("project...backup".to_owned(), "readme.md".to_owned()),
            ("versions..old".to_owned(), "file.txt".to_owned()),
        ]
    );

    let folders = rig.stack.counts().await.folders;
    for hostile in [
        "../secret.txt",
        "folder/../secret.txt",
        "folder/./secret.txt",
        "folder/..",
        "..",
    ] {
        let leaf = if hostile.ends_with("..") {
            "x"
        } else {
            leaf_of(hostile)
        };
        let refused = rig
            .stack
            .open(
                &rig.alice,
                &request_body(None, &[file_json("h", leaf, hostile, 1)]),
            )
            .await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{hostile}"
        );
    }
    assert_eq!(rig.stack.counts().await.folders, folders);
    assert_eq!(rig.stack.counts().await.sessions, 1);

    let copy = |prefix: &str| {
        format!(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
                  declared_size_bytes, reserved_bytes, upload_kind, final_object_id,
                  final_object_key, created_at, updated_at)
             SELECT '{}', transfer_session_id, 90, 'raw-{}', display_name, '{prefix}',
                    declared_size_bytes, reserved_bytes, upload_kind, '{}',
                    'objects/00/00/{}', created_at, updated_at
               FROM transfer_session_files WHERE ordinal = 0 LIMIT 1",
            rig.stack.fresh_id(),
            rig.stack.fresh_id(),
            rig.stack.fresh_id(),
            "0".repeat(32),
        )
    };
    for traversal in ["..", "../x", "x/..", "x/../y"] {
        assert!(
            execute_fallible(&rig.stack, &copy(traversal))
                .await
                .is_err(),
            "the schema still refuses {traversal:?}"
        );
    }
    execute_fallible(&rig.stack, &copy("ok..ok/x...y"))
        .await
        .unwrap();
}

use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};

use super::files::FILES;
use super::folders::{Member, FOLDERS, HOST_A, HOST_B, SEEDED_AT};
use super::*;
use crate::domain::clock::Clock;
use crate::features::files::FileId;
use crate::features::folders::delete::{
    register_jobs as register_folder_deletion, step, DeleteTreeContext, Step,
};
use crate::features::folders::FolderId;
use crate::infra::db::InstanceId;
use crate::infra::jobs::{Claimant, Dispatcher, Jitter, JobAudit, JobKind, Registry};
use crate::storage::key::{KeyNamespace, ObjectKey};
use crate::storage::lifecycle::thumbnails::ThumbnailCache;
use crate::storage::lifecycle::{
    register_jobs as register_storage_lifecycle, LifecycleContext, StorageObjectId,
};
use crate::storage::local::LocalProvider;
use crate::storage::provider::{PutHint, StorageProvider};

pub(super) const BATCH_DELETE: &str = "/api/v1/files/batch/delete";
pub(super) const BATCH_IMPACT: &str = "/api/v1/files/batch/deletion-impact";

pub(super) struct Seeded {
    pub(super) id: String,
    pub(super) object_id: String,
    pub(super) key: String,
}

pub(super) struct World {
    pub(super) stack: Stack,
    pub(super) provider: Arc<LocalProvider>,
    pub(super) dispatcher: Dispatcher,
    pub(super) claimant: Claimant,
    pub(super) tree: DeleteTreeContext,
    pub(super) _root: TempDir,
}

impl World {
    pub(super) async fn start() -> Self {
        Self::start_with_batches(None).await
    }

    pub(super) async fn start_with_batches(batches_per_run: Option<u32>) -> Self {
        let root = TempDir::new().unwrap();
        let clock = TestClock::new(START);
        let stack = Stack::start(root.path(), &clock).await;
        let provider = Arc::new(LocalProvider::temporary());
        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        let storage: Arc<dyn StorageProvider> = provider.clone();
        let lifecycle = LifecycleContext::new(
            stack.pools.clone(),
            Arc::clone(&shared),
            storage,
            ThumbnailCache::under(root.path()),
            stack.audit.clone(),
            false,
        );
        let mut tree = DeleteTreeContext::new(
            stack.pools.clone(),
            Arc::clone(&shared),
            stack.audit.clone(),
        );
        if let Some(batches) = batches_per_run {
            tree = tree.with_batches_per_run(batches);
        }
        let registry = register_folder_deletion(
            register_storage_lifecycle(Registry::production(), lifecycle),
            tree.clone(),
        );
        let dispatcher = Dispatcher::new(
            stack.pools.clone(),
            Arc::clone(&shared),
            registry,
            Jitter::from_fn(|| u32::MAX / 2),
            JobAudit::new(Arc::new(stack.audit.clone())),
            Duration::from_secs(60),
        );
        let claimant = Claimant::worker(InstanceId::generate(&clock), 0);
        Self {
            stack,
            provider,
            dispatcher,
            claimant,
            tree,
            _root: root,
        }
    }

    pub(super) async fn run_all(&self, kind: JobKind) -> u64 {
        let mut executed = 0;
        while self
            .dispatcher
            .run_next_kind(&self.claimant, kind, || true)
            .await
            .unwrap()
            .is_some()
        {
            executed += 1;
        }
        executed
    }

    pub(super) async fn blob(
        &self,
        owner: UserId,
        folder: Option<&str>,
        name: &str,
        content: &[u8],
    ) -> Seeded {
        let key = ObjectKey::allocate(KeyNamespace::Objects);
        self.provider
            .put_stream(
                &key,
                Box::pin(Cursor::new(content.to_vec())),
                PutHint {
                    declared_len: Some(content.len() as u64),
                    content_type: None,
                },
            )
            .await
            .unwrap();
        let object_id = StorageObjectId::generate(&self.stack.clock).to_string();
        let id = FileId::generate(&self.stack.clock).to_string();
        let size = i64::try_from(content.len()).unwrap();
        let folder_sql = folder.map_or_else(|| "NULL".to_owned(), |id| format!("'{id}'"));
        self.stack
            .execute(&format!(
                "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
                 VALUES ('{object_id}', '{}', 'local', {size}, 'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}');
                 INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized, size_bytes, created_at, updated_at)
                 VALUES ('{id}', '{owner}', {folder_sql}, '{object_id}', '{name}', '{}', {size}, '{SEEDED_AT}', '{SEEDED_AT}');
                 UPDATE users SET used_bytes = used_bytes + {size} WHERE id = '{owner}'",
                key.as_str(),
                name.to_lowercase()
            ))
            .await;
        Seeded {
            id,
            object_id,
            key: key.as_str().to_owned(),
        }
    }

    pub(super) async fn on_disk(&self, key: &str) -> bool {
        StorageProvider::exists(&*self.provider, &ObjectKey::parse(key).unwrap())
            .await
            .unwrap()
    }

    pub(super) async fn used_bytes(&self, owner: UserId) -> i64 {
        self.stack
            .scalar_i64(&format!(
                "SELECT used_bytes FROM users WHERE id = '{owner}'"
            ))
            .await
    }

    pub(super) async fn count(&self, sql: &str) -> i64 {
        self.stack.scalar_i64(sql).await
    }

    pub(super) async fn delete(&self, member: &Member, path: &str) -> Fetched {
        self.stack.api(Method::DELETE, path, member, None).await
    }

    pub(super) async fn post(&self, member: &Member, path: &str, body: &Value) -> Fetched {
        self.stack.api(Method::POST, path, member, Some(body)).await
    }

    pub(super) async fn folder(&self, member: &Member, name: &str, parent: Option<&str>) -> String {
        self.stack.make(member, name, parent).await["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

pub(super) fn token(prefix: &str, n: u32) -> String {
    format!("{prefix}{n:0>20}")
}

impl World {
    pub(super) async fn share(&self, owner: UserId, alias: &str, items: &[(&str, &str)]) -> String {
        let id = FolderId::generate(&self.stack.clock).to_string();
        let mut sql = format!(
            "INSERT INTO shares (id, owner_id, public_id, alias, name, created_at, updated_at)
             VALUES ('{id}', '{owner}', '{}', '{alias}', 'Share {alias}', '{SEEDED_AT}', '{SEEDED_AT}');",
            id.replace('-', "")
        );
        for (kind, target) in items {
            let item = FolderId::generate(&self.stack.clock);
            let column = if *kind == "file" {
                "file_id"
            } else {
                "folder_id"
            };
            sql.push_str(&format!(
                "INSERT INTO share_items (id, share_id, item_type, {column}, added_at)
                 VALUES ('{item}', '{id}', '{kind}', '{target}', '{SEEDED_AT}');"
            ));
        }
        self.stack.execute(&sql).await;
        id
    }

    pub(super) async fn embed(
        &self,
        owner: UserId,
        file: &str,
        n: u32,
        revoked: bool,
        expires: Option<&str>,
    ) {
        let id = FolderId::generate(&self.stack.clock);
        let revoked = if revoked {
            format!("'{SEEDED_AT}'")
        } else {
            "NULL".to_owned()
        };
        let expires = expires.map_or_else(|| "NULL".to_owned(), |at| format!("'{at}'"));
        self.stack
            .execute(&format!(
                "INSERT INTO embed_grants (id, file_id, owner_id, public_id, token_hash, created_at, expires_at, revoked_at)
                 VALUES ('{id}', '{file}', '{owner}', '{}', '{n:0>64}', '{SEEDED_AT}', {expires}, {revoked})",
                token("emb", n),
            ))
            .await;
    }
}

#[tokio::test]
#[allow(non_snake_case, reason = "the accepted regression identifier is R-060")]
async fn regression_R060_recursive_folder_delete_reclaims_blobs() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let keep = world.blob(alice.id, None, "keep.bin", b"keep-me").await;

    let top = world.folder(&alice, "Top", None).await;
    let middle = world.folder(&alice, "Middle", Some(&top)).await;
    let bottom = world.folder(&alice, "Bottom", Some(&middle)).await;
    world.folder(&alice, "Empty", Some(&top)).await;
    let doomed = [
        world.blob(alice.id, Some(&top), "a.txt", b"aaaa").await,
        world.blob(alice.id, Some(&top), "b.txt", b"bbbbbb").await,
        world.blob(alice.id, Some(&middle), "c.txt", b"cc").await,
        world
            .blob(alice.id, Some(&bottom), "d.txt", b"ddddddd")
            .await,
        world.blob(alice.id, Some(&bottom), "e.txt", b"e").await,
        world.blob(alice.id, Some(&bottom), "zero.txt", b"").await,
    ];
    let before = world.used_bytes(alice.id).await;
    assert_eq!(before, 4 + 6 + 2 + 7 + 1 + 7);
    for seeded in doomed.iter().chain([&keep]) {
        assert!(world.on_disk(&seeded.key).await);
    }

    let deleted = world.delete(&alice, &format!("{FOLDERS}/{top}")).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text());
    assert!(deleted.body.is_empty());

    let gone = |response: Fetched| {
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "{}",
            response.text()
        );
    };
    gone(world.stack.read(&format!("{FOLDERS}/{top}"), &alice).await);
    gone(
        world
            .stack
            .read(&format!("{FOLDERS}/{bottom}"), &alice)
            .await,
    );
    gone(
        world
            .stack
            .read(&format!("{FILES}?folderId={middle}"), &alice)
            .await,
    );
    for seeded in &doomed {
        gone(
            world
                .stack
                .read(&format!("{FILES}/{}", seeded.id), &alice)
                .await,
        );
    }
    let roots = world.stack.read(FILES, &alice).await.json();
    let names: Vec<&str> = roots["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["keep.bin"]);
    let search = world
        .stack
        .read(&format!("{FILES}?q=txt"), &alice)
        .await
        .json();
    assert!(search["items"].as_array().unwrap().is_empty(), "{search}");
    let listed = world.stack.read(FOLDERS, &alice).await.json();
    assert!(listed["items"].as_array().unwrap().is_empty(), "{listed}");

    assert_eq!(
        world
            .count(&format!("SELECT deleting FROM folders WHERE id = '{top}'"))
            .await,
        1
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'folders.delete_tree' AND state = 'pending'")
            .await,
        1
    );
    assert_eq!(world.used_bytes(alice.id).await, before);
    for seeded in &doomed {
        assert!(
            world.on_disk(&seeded.key).await,
            "no byte moves in the claim"
        );
    }

    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    let state: (String, Option<String>) =
        sqlx::query_as("SELECT state, last_error FROM jobs WHERE kind = 'folders.delete_tree'")
            .fetch_one(world.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(state, ("succeeded".to_owned(), None));
    assert_eq!(world.count("SELECT COUNT(*) FROM files").await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 0);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        0
    );
    assert_eq!(
        world
            .count(
                "SELECT COUNT(*) FROM storage_objects WHERE state = 'tombstoned' AND refcount = 0"
            )
            .await,
        6
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue WHERE reason = 'folder_deleted' AND state = 'pending'")
            .await,
        6
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob' AND state = 'pending'")
            .await,
        6
    );
    assert_eq!(world.used_bytes(alice.id).await, 7);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM deletion_receipts").await,
        4 + 6
    );
    for seeded in &doomed {
        assert!(
            world.on_disk(&seeded.key).await,
            "bytes outlive the metadata"
        );
    }

    assert_eq!(world.run_all(JobKind::StorageDeleteBlob).await, 6);
    for seeded in &doomed {
        assert!(
            !world.on_disk(&seeded.key).await,
            "{} is erased",
            seeded.key
        );
    }
    assert!(world.on_disk(&keep.key).await);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM storage_objects WHERE state = 'deleted'")
            .await,
        6
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue WHERE state = 'done'")
            .await,
        6
    );
    assert_eq!(world.stack.audit_rows("FOLDER_DELETED").await.len(), 1);
    assert!(!world.stack.audit_rows("FILE_DELETED").await.is_empty());

    let again = world.delete(&alice, &format!("{FOLDERS}/{top}")).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
}

fn share_summary(impact: &Value) -> Vec<(String, u64)> {
    impact["affectedShares"]
        .as_array()
        .unwrap()
        .iter()
        .map(|share| {
            (
                share["alias"].as_str().unwrap().to_owned(),
                share["remainingItems"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn it_delete_impact_counts_shares() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;

    let r = world.folder(&alice, "R", None).await;
    let s = world.folder(&alice, "S", Some(&r)).await;
    let o = world.folder(&alice, "O", None).await;
    let p = world.folder(&alice, "P", None).await;
    let q = world.folder(&alice, "Q", Some(&p)).await;
    let r1 = world.blob(alice.id, Some(&r), "r1.txt", b"11111").await;
    let r2 = world.blob(alice.id, Some(&r), "r2.txt", b"2222222").await;
    let s1 = world
        .blob(alice.id, Some(&s), "s1.txt", b"3333333333")
        .await;
    let o1 = world.blob(alice.id, Some(&o), "o1.txt", b"444").await;
    let q1 = world.blob(alice.id, Some(&q), "q1.txt", b"55").await;
    world.blob(alice.id, Some(&p), "p1.txt", b"6666").await;
    let bob_file = world.blob(bob.id, None, "b.txt", b"bob").await;

    world
        .share(
            alice.id,
            "alpha",
            &[("file", &r1.id), ("file", &r2.id), ("file", &o1.id)],
        )
        .await;
    world.share(alice.id, "bravo", &[("folder", &r)]).await;
    world.share(alice.id, "charlie", &[("folder", &s)]).await;
    world
        .share(alice.id, "delta", &[("file", &s1.id), ("folder", &r)])
        .await;
    world.share(alice.id, "echo", &[("folder", &o)]).await;
    world.share(alice.id, "hotel", &[("folder", &p)]).await;
    world.share(bob.id, "zulu", &[("file", &bob_file.id)]).await;
    world.embed(alice.id, &r1.id, 1, false, None).await;
    world.embed(alice.id, &s1.id, 2, true, None).await;
    world.embed(alice.id, &o1.id, 3, false, None).await;
    world
        .embed(alice.id, &r2.id, 4, false, Some("2026-01-01T00:00:00.000Z"))
        .await;

    let folder = world
        .stack
        .read(&format!("{FOLDERS}/{r}/deletion-impact"), &alice)
        .await;
    assert_eq!(folder.status, StatusCode::OK, "{}", folder.text());
    let folder = folder.json();
    assert_eq!(folder["files"], 3);
    assert_eq!(folder["folders"], 2);
    assert_eq!(folder["totalBytes"], 22);
    assert_eq!(folder["affectedEmbeds"], 1);
    assert_eq!(folder["affectedShareCount"], 4);
    assert_eq!(
        share_summary(&folder),
        [
            ("alpha".to_owned(), 1),
            ("bravo".to_owned(), 0),
            ("charlie".to_owned(), 0),
            ("delta".to_owned(), 0)
        ]
    );
    let first = &folder["affectedShares"][0];
    assert_eq!(first["name"], "Share alpha");
    assert!(first["id"].is_string());
    assert!(!folder.to_string().contains("zulu"));

    let file = world
        .stack
        .read(&format!("{FILES}/{}/deletion-impact", r1.id), &alice)
        .await
        .json();
    assert_eq!(
        (file["files"].clone(), file["folders"].clone()),
        (1.into(), 0.into())
    );
    assert_eq!(file["totalBytes"], 5);
    assert_eq!(file["affectedEmbeds"], 1);
    assert_eq!(
        share_summary(&file),
        [
            ("alpha".to_owned(), 2),
            ("bravo".to_owned(), 1),
            ("delta".to_owned(), 2)
        ]
    );

    let nested = world
        .stack
        .read(&format!("{FOLDERS}/{q}/deletion-impact"), &alice)
        .await
        .json();
    assert_eq!(
        (
            nested["files"].clone(),
            nested["folders"].clone(),
            nested["totalBytes"].clone()
        ),
        (1.into(), 1.into(), 2.into())
    );
    assert_eq!(share_summary(&nested), [("hotel".to_owned(), 1)]);
    assert_eq!(nested["affectedEmbeds"], 0);
    assert_eq!(q1.id.len(), 36);

    let overlapping = world
        .post(
            &alice,
            BATCH_IMPACT,
            &json!({ "fileIds": [r1.id], "folderIds": [r, s] }),
        )
        .await;
    assert_eq!(overlapping.status, StatusCode::OK, "{}", overlapping.text());
    let overlapping = overlapping.json();
    assert_eq!(overlapping["files"], 3);
    assert_eq!(overlapping["folders"], 2);
    assert_eq!(overlapping["totalBytes"], 22);
    assert_eq!(overlapping["affectedShareCount"], 4);

    let folder_only = world
        .post(&alice, BATCH_IMPACT, &json!({ "folderIds": [o] }))
        .await;
    assert_eq!(folder_only.status, StatusCode::OK, "{}", folder_only.text());
    assert_eq!(
        share_summary(&folder_only.json()),
        [("alpha".to_owned(), 2), ("echo".to_owned(), 0)]
    );

    let foreign = world
        .stack
        .read(&format!("{FILES}/{}/deletion-impact", bob_file.id), &alice)
        .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    assert_eq!(foreign.error_code(), "FILE_NOT_FOUND");
    let foreign_folder = world
        .stack
        .read(&format!("{FOLDERS}/{r}/deletion-impact"), &bob)
        .await;
    assert_eq!(foreign_folder.error_code(), "FOLDER_NOT_FOUND");
    let mixed = world
        .post(
            &alice,
            BATCH_IMPACT,
            &json!({ "fileIds": [r1.id, bob_file.id] }),
        )
        .await;
    assert_eq!(mixed.status, StatusCode::NOT_FOUND);
    assert_eq!(mixed.error_code(), "FILE_NOT_FOUND");
    assert!(!mixed.text().contains(&bob_file.id));
    let phantom = world
        .post(
            &alice,
            BATCH_IMPACT,
            &json!({ "folderIds": [world.stack.fresh_id()] }),
        )
        .await;
    assert_eq!(phantom.error_code(), "FOLDER_NOT_FOUND");
    let bobs = world
        .stack
        .read(&format!("{FILES}/{}/deletion-impact", bob_file.id), &bob)
        .await
        .json();
    assert_eq!(share_summary(&bobs), [("zulu".to_owned(), 0)]);
    assert!(!bobs.to_string().contains("alpha"));

    for body in [json!({}), json!({ "fileIds": [], "folderIds": [] })] {
        let empty = world.post(&alice, BATCH_IMPACT, &body).await;
        assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(empty.error_code(), "VALIDATION_ERROR");
    }
    let repeated = world
        .post(&alice, BATCH_IMPACT, &json!({ "fileIds": [r1.id, r1.id] }))
        .await;
    assert_eq!(repeated.error_code(), "VALIDATION_ERROR");
    let ids: Vec<String> = (0..501).map(|_| world.stack.fresh_id()).collect();
    let too_many = world
        .post(&alice, BATCH_IMPACT, &json!({ "fileIds": ids }))
        .await;
    assert_eq!(too_many.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(too_many.error_code(), "BATCH_TOO_LARGE");

    assert_eq!(world.count("SELECT COUNT(*) FROM jobs").await, 0);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM deletion_receipts").await,
        0
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM storage_objects WHERE state <> 'active'")
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM folders WHERE deleting = 1")
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action LIKE '%DELETED'")
            .await,
        0
    );
}

#[tokio::test]
async fn it_delete_impact_caps_the_listed_shares_and_reports_the_total() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let file = world.blob(alice.id, None, "popular.txt", b"x").await;
    for n in 0..105 {
        world
            .share(alice.id, &format!("link-{n:03}"), &[("file", &file.id)])
            .await;
    }
    let impact = world
        .stack
        .read(&format!("{FILES}/{}/deletion-impact", file.id), &alice)
        .await
        .json();
    assert_eq!(impact["affectedShares"].as_array().unwrap().len(), 100);
    assert_eq!(impact["affectedShareCount"], 105);
    assert_eq!(impact["affectedShares"][0]["alias"], "link-000");
}

fn status_of(fetched: &Fetched) -> StatusCode {
    fetched.status
}

#[tokio::test]
async fn it_delete_idempotent_204() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let file = world.blob(alice.id, None, "once.txt", b"once").await;
    let folder = world.folder(&alice, "Dir", None).await;
    let inner = world
        .blob(alice.id, Some(&folder), "inner.txt", b"inner")
        .await;
    let live = world.blob(alice.id, None, "live.txt", b"live").await;

    let file_path = format!("{FILES}/{}", file.id);
    assert_eq!(
        status_of(&world.delete(&alice, &file_path).await),
        StatusCode::NO_CONTENT
    );
    let second = world.delete(&alice, &file_path).await;
    assert_eq!(second.status, StatusCode::NO_CONTENT);
    assert!(second.body.is_empty());
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_DELETED'")
            .await,
        1
    );
    assert_eq!(world.used_bytes(alice.id).await, 5 + 4 + 4 - 4);

    assert_eq!(world.run_all(JobKind::StorageDeleteBlob).await, 1);
    world
        .stack
        .execute(
            "DELETE FROM file_deletion_queue; DELETE FROM storage_objects WHERE state = 'deleted'",
        )
        .await;
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        0
    );
    assert_eq!(
        status_of(&world.delete(&alice, &file_path).await),
        StatusCode::NO_CONTENT,
        "the receipt, not the blob queue, proves ownership"
    );
    assert_eq!(world.used_bytes(alice.id).await, 5 + 4);

    let folder_path = format!("{FOLDERS}/{folder}");
    assert_eq!(
        status_of(&world.delete(&alice, &folder_path).await),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status_of(&world.delete(&alice, &folder_path).await),
        StatusCode::NO_CONTENT,
        "repeat while deleting"
    );
    assert_eq!(
        status_of(&world.delete(&alice, &format!("{FILES}/{}", inner.id)).await),
        StatusCode::NO_CONTENT,
        "a file inside a deleting folder is already logically deleted"
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'folders.delete_tree'")
            .await,
        1
    );
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 0);
    assert_eq!(
        status_of(&world.delete(&alice, &folder_path).await),
        StatusCode::NO_CONTENT,
        "repeat after completion"
    );
    assert_eq!(
        status_of(&world.delete(&alice, &format!("{FILES}/{}", inner.id)).await),
        StatusCode::NO_CONTENT
    );
    world.run_all(JobKind::StorageDeleteBlob).await;
    world
        .stack
        .execute(
            "DELETE FROM file_deletion_queue; DELETE FROM storage_objects WHERE state = 'deleted'",
        )
        .await;
    assert_eq!(
        status_of(&world.delete(&alice, &folder_path).await),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FOLDER_DELETED'")
            .await,
        1
    );
    assert_eq!(world.used_bytes(alice.id).await, 4);

    let phantom_file = world
        .delete(&alice, &format!("{FILES}/{}", world.stack.fresh_id()))
        .await;
    assert_eq!(phantom_file.status, StatusCode::NOT_FOUND);
    assert_eq!(phantom_file.error_code(), "FILE_NOT_FOUND");
    let phantom_folder = world
        .delete(&alice, &format!("{FOLDERS}/{}", world.stack.fresh_id()))
        .await;
    assert_eq!(phantom_folder.status, StatusCode::NOT_FOUND);
    assert_eq!(phantom_folder.error_code(), "FOLDER_NOT_FOUND");
    assert_eq!(
        world
            .delete(&alice, &format!("{FILES}/not-an-id"))
            .await
            .error_code(),
        "FILE_NOT_FOUND"
    );
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/not-an-id"))
            .await
            .error_code(),
        "FOLDER_NOT_FOUND"
    );

    let replay_file = world.delete(&bob, &file_path).await;
    assert_eq!(
        replay_file.status,
        StatusCode::NOT_FOUND,
        "another user replaying a known deleted id"
    );
    assert_eq!(replay_file.error_code(), "FILE_NOT_FOUND");
    let replay_folder = world.delete(&bob, &folder_path).await;
    assert_eq!(replay_folder.status, StatusCode::NOT_FOUND);
    assert_eq!(replay_folder.error_code(), "FOLDER_NOT_FOUND");
    let live_foreign = world.delete(&bob, &format!("{FILES}/{}", live.id)).await;
    assert_eq!(live_foreign.status, StatusCode::NOT_FOUND);
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                live.id
            ))
            .await,
        1
    );
}

#[tokio::test]
async fn it_file_delete_commits_every_effect_in_one_transaction() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let doomed = world
        .blob(alice.id, None, "report.pdf", b"twelve bytes")
        .await;
    let other = world.blob(alice.id, None, "other.pdf", b"other").await;
    world
        .share(
            alice.id,
            "shared",
            &[("file", &doomed.id), ("file", &other.id)],
        )
        .await;
    world.embed(alice.id, &doomed.id, 1, false, None).await;
    let before = world.used_bytes(alice.id).await;
    assert_eq!(before, 12 + 5);

    let deleted = world
        .delete(&alice, &format!("{FILES}/{}", doomed.id))
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text());

    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                doomed.id
            ))
            .await,
        0
    );
    assert_eq!(world.count("SELECT COUNT(*) FROM share_items").await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM embed_grants").await, 0);
    let object: (String, i64, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT state, refcount, tombstoned_at, deleted_at FROM storage_objects WHERE id = ?1",
    )
    .bind(&doomed.object_id)
    .fetch_one(world.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(object.0, "tombstoned");
    assert_eq!(object.1, 0);
    assert!(object.2.is_some() && object.3.is_none());
    let queue: (String, String, i64) = sqlx::query_as(
        "SELECT reason, state, attempts FROM file_deletion_queue WHERE storage_object_id = ?1",
    )
    .bind(&doomed.object_id)
    .fetch_one(world.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(queue, ("file_deleted".to_owned(), "pending".to_owned(), 0));
    let job: (String, String) =
        sqlx::query_as("SELECT dedup_key, state FROM jobs WHERE kind = 'storage.delete_blob'")
            .fetch_one(world.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        job,
        (
            format!("storage_object:{}", doomed.object_id),
            "pending".to_owned()
        )
    );
    assert_eq!(world.used_bytes(alice.id).await, 5);
    let receipt: (String, String) =
        sqlx::query_as("SELECT resource_kind, owner_id FROM deletion_receipts WHERE id = ?1")
            .bind(&doomed.id)
            .fetch_one(world.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(receipt, ("file".to_owned(), alice.id.to_string()));
    assert!(
        world.on_disk(&doomed.key).await,
        "no storage I/O happens in the transaction"
    );
    let found = world
        .stack
        .read(&format!("{FILES}?q=report"), &alice)
        .await
        .json();
    assert!(found["items"].as_array().unwrap().is_empty());

    let audit: (String, String, String, String, String) = sqlx::query_as(
        "SELECT action, actor_user_id, target_type, target_id, metadata_json FROM audit_events
          WHERE action = 'FILE_DELETED'",
    )
    .fetch_one(world.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(audit.1, alice.id.to_string());
    assert_eq!(
        (audit.2.as_str(), audit.3.as_str()),
        ("file", doomed.id.as_str())
    );
    let metadata: Value = serde_json::from_str(&audit.4).unwrap();
    assert_eq!(
        metadata,
        json!({
            "scope": "file",
            "files": 1,
            "bytes_released": 12,
            "shares_detached": 1,
            "embeds_revoked": 1
        })
    );
    assert!(!audit.4.contains(&doomed.key) && !audit.4.contains(&doomed.object_id));

    assert_eq!(
        world
            .delete(&alice, &format!("{FILES}/{}", doomed.id))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(world.used_bytes(alice.id).await, 5, "no second decrement");
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob'")
            .await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_DELETED'")
            .await,
        1
    );
}

#[tokio::test]
async fn it_file_delete_quota_underflow_rolls_the_whole_deletion_back() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let file = world.blob(alice.id, None, "kept.bin", b"0123456789").await;
    world.share(alice.id, "kept", &[("file", &file.id)]).await;
    world.embed(alice.id, &file.id, 1, false, None).await;
    world
        .stack
        .execute(&format!(
            "UPDATE users SET used_bytes = 3 WHERE id = '{}'",
            alice.id
        ))
        .await;

    let failed = world.delete(&alice, &format!("{FILES}/{}", file.id)).await;
    assert_eq!(
        failed.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        failed.text()
    );
    assert_eq!(failed.error_code(), "INTERNAL_ERROR");
    assert_eq!(world.count("SELECT COUNT(*) FROM files").await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM share_items").await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM embed_grants").await, 1);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        0
    );
    assert_eq!(world.count("SELECT COUNT(*) FROM jobs").await, 0);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM deletion_receipts").await,
        0
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_DELETED'")
            .await,
        0
    );
    assert_eq!(world.used_bytes(alice.id).await, 3, "never clamped to zero");
    assert!(world.on_disk(&file.key).await);
}

#[tokio::test]
async fn it_delete_quota_accounting_keeps_received_and_held_reservations_apart() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let doomed = world.blob(alice.id, None, "forty.bin", &[7_u8; 40]).await;
    world.blob(alice.id, None, "sixty.bin", &[8_u8; 60]).await;
    world.blob(alice.id, None, "zero.bin", b"").await;

    let received_object = StorageObjectId::generate(&world.stack.clock).to_string();
    let reverse = FolderId::generate(&world.stack.clock).to_string();
    let received = FolderId::generate(&world.stack.clock).to_string();
    let session = FolderId::generate(&world.stack.clock).to_string();
    let reservation = FolderId::generate(&world.stack.clock).to_string();
    world
        .stack
        .execute(&format!(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
             VALUES ('{received_object}', 'objects/00/00/{:032x}', 'local', 60, 'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}');
             INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES ('{reverse}', '{}', 'reverse-public-id-01', 'inbox', '{SEEDED_AT}', '{SEEDED_AT}');
             INSERT INTO received_files (id, owner_id, reverse_share_id, storage_object_id, name, name_normalized, size_bytes, received_at, updated_at)
             VALUES ('{received}', '{}', '{reverse}', '{received_object}', 'in.bin', 'in.bin', 60, '{SEEDED_AT}', '{SEEDED_AT}');
             UPDATE users SET used_bytes = used_bytes + 60 WHERE id = '{}';
             INSERT INTO transfer_sessions (id, context, user_id, provider, state, created_at, updated_at, expires_at)
             VALUES ('{session}', 'my_files', '{}', 'local', 'created', '{SEEDED_AT}', '{SEEDED_AT}', '2027-01-01T00:00:00.000Z');
             INSERT INTO quota_reservations (id, user_id, transfer_session_id, context, reserved_bytes, state, created_at, expires_at)
             VALUES ('{reservation}', '{}', '{session}', 'my_files', 500, 'held', '{SEEDED_AT}', '2027-01-01T00:00:00.000Z')",
            0xfeed_u64,
            alice.id, alice.id, alice.id, alice.id, alice.id,
        ))
        .await;
    assert_eq!(world.used_bytes(alice.id).await, 160);

    let deleted = world
        .delete(&alice, &format!("{FILES}/{}", doomed.id))
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text());
    assert_eq!(world.used_bytes(alice.id).await, 120);
    assert_eq!(world.count("SELECT COUNT(*) FROM received_files").await, 1);
    assert_eq!(
        world
            .count(&format!("SELECT COUNT(*) FROM storage_objects WHERE id = '{received_object}' AND state = 'active'"))
            .await,
        1
    );
    assert_eq!(
        world
            .count(&format!("SELECT reserved_bytes FROM quota_reservations WHERE id = '{reservation}' AND state = 'held'"))
            .await,
        500,
        "a held reservation is neither released nor required by a deletion"
    );

    let zero = world.stack.read(FILES, &alice).await.json();
    let zero_id = zero["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "zero.bin")
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .unwrap();
    assert_eq!(
        world
            .delete(&alice, &format!("{FILES}/{zero_id}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(world.used_bytes(alice.id).await, 120);

    world
        .stack
        .execute(&format!(
            "UPDATE users SET quota_override_mode = 'bytes', quota_bytes = 10 WHERE id = '{}'",
            alice.id
        ))
        .await;
    let over_quota = world.stack.read(FILES, &alice).await.json();
    let sixty = over_quota["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "sixty.bin")
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .unwrap();
    let allowed = world.delete(&alice, &format!("{FILES}/{sixty}")).await;
    assert_eq!(allowed.status, StatusCode::NO_CONTENT, "{}", allowed.text());
    assert_eq!(
        world.used_bytes(alice.id).await,
        60,
        "an over-quota owner may always delete"
    );
}

#[tokio::test]
async fn it_delete_selects_the_object_by_identity_not_by_name_or_path() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let target_dir = world.folder(&alice, "Elsewhere", None).await;
    let first = world.blob(alice.id, None, "same.txt", b"first-bytes").await;
    let second = world
        .blob(alice.id, Some(&target_dir), "same.txt", b"second")
        .await;

    let renamed = world.stack.edit(&alice, &first.id, &json!({})).await;
    assert_eq!(
        renamed.status,
        StatusCode::NOT_FOUND,
        "folders endpoint is not for files"
    );
    let patch = world
        .stack
        .api(
            Method::PATCH,
            &format!("{FILES}/{}", first.id),
            &alice,
            Some(&json!({ "name": "../../etc/passwd?.txt" })),
        )
        .await;
    assert_eq!(patch.status, StatusCode::UNPROCESSABLE_ENTITY);
    let patch = world
        .stack
        .api(
            Method::PATCH,
            &format!("{FILES}/{}", first.id),
            &alice,
            Some(&json!({ "name": "renamed.txt" })),
        )
        .await;
    assert_eq!(patch.status, StatusCode::OK, "{}", patch.text());
    let moved = world
        .stack
        .api(
            Method::POST,
            &format!("{FILES}/{}/move", first.id),
            &alice,
            Some(&json!({ "folderId": target_dir })),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());

    assert_eq!(
        world
            .delete(&alice, &format!("{FILES}/{}", first.id))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let tombstoned: Vec<String> =
        sqlx::query_scalar("SELECT id FROM storage_objects WHERE state = 'tombstoned'")
            .fetch_all(world.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(tombstoned, std::slice::from_ref(&first.object_id));
    world.run_all(JobKind::StorageDeleteBlob).await;
    assert!(!world.on_disk(&first.key).await);
    assert!(world.on_disk(&second.key).await);
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                second.id
            ))
            .await,
        1
    );
}

static BULK: AtomicU32 = AtomicU32::new(0x1000);

impl World {
    pub(super) async fn bulk_files(
        &self,
        owner: UserId,
        folder: &str,
        count: u32,
        size: i64,
    ) -> Vec<String> {
        let tag = BULK.fetch_add(2, Ordering::Relaxed);
        let (objects, files) = (tag, tag + 1);
        self.stack
            .execute(&format!(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {count})
                 INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
                 SELECT printf('0192f3a1-{objects:04x}-7000-8000-%012x', i),
                        'objects/00/00/' || printf('%032x', {objects} * 1000000 + i),
                        'local', {size}, 'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}'
                   FROM n;
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {count})
                 INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized, size_bytes, created_at, updated_at)
                 SELECT printf('0192f3a1-{files:04x}-7000-8000-%012x', i), '{owner}', '{folder}',
                        printf('0192f3a1-{objects:04x}-7000-8000-%012x', i),
                        printf('bulk-{tag:04x}-%05d.bin', i), printf('bulk-{tag:04x}-%05d.bin', i),
                        {size}, '{SEEDED_AT}', '{SEEDED_AT}'
                   FROM n;
                 UPDATE users SET used_bytes = used_bytes + {count} * {size} WHERE id = '{owner}'"
            ))
            .await;
        (1..=count)
            .map(|i| format!("0192f3a1-{objects:04x}-7000-8000-{i:012x}"))
            .collect()
    }

    pub(super) async fn duplicates(&self, sql: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) - COUNT(DISTINCT {sql}) FROM jobs WHERE kind = 'storage.delete_blob'"
        ))
        .await
    }
}

#[tokio::test]
async fn it_folder_delete_crash_mid_batch_resumes() {
    let world = World::start_with_batches(Some(1)).await;
    let alice = world.stack.member("alice", HOST_A).await;
    let outside = world.blob(alice.id, None, "outside.bin", b"outside").await;
    let top = world.folder(&alice, "Top", None).await;
    let sub = world.folder(&alice, "Sub", Some(&top)).await;
    let real: Vec<Seeded> = vec![
        world
            .blob(alice.id, Some(&top), "real-top.bin", b"top-bytes")
            .await,
        world
            .blob(alice.id, Some(&sub), "real-sub.bin", b"sub-bytes")
            .await,
    ];
    world.bulk_files(alice.id, &top, 700, 10).await;
    world.bulk_files(alice.id, &sub, 500, 10).await;
    let total_files: i64 = 1_202;
    let before = world.used_bytes(alice.id).await;
    assert_eq!(before, 7 + 9 + 9 + 12_000);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM files").await,
        total_files + 1
    );

    let top_id: FolderId = top.parse().unwrap();
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let clock: &TestClock = &world.stack.clock;
    let claimed = crate::infra::jobs::claim::claim(
        &world.stack.pools,
        clock,
        &world.claimant,
        &[JobKind::FoldersDeleteTree],
        1,
        || true,
    )
    .await
    .unwrap();
    assert_eq!(claimed.len(), 1);
    let first = step(&world.tree, top_id).await.unwrap();
    assert_eq!(first, Step::Files { files: 500 });

    assert_eq!(
        world.count("SELECT COUNT(*) FROM files").await,
        total_files + 1 - 500
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        500
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM storage_objects WHERE state = 'tombstoned'")
            .await,
        500
    );
    let after_first = world.used_bytes(alice.id).await;
    assert!(after_first < before && after_first >= 0);
    let removed_first = world
        .count("SELECT COUNT(*) FROM deletion_receipts WHERE resource_kind = 'file'")
        .await;
    assert_eq!(removed_first, 500);

    clock.advance(Duration::from_secs(6 * 60));
    assert_eq!(
        crate::infra::jobs::claim::sweep_expired_leases(&world.stack.pools, clock)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        1
    );

    let executions = world.run_all(JobKind::FoldersDeleteTree).await;
    assert!(executions >= 3, "one batch per job run: {executions}");
    assert_eq!(world.count("SELECT COUNT(*) FROM files").await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 0);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        0
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        total_files
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'folders.delete_tree' AND state <> 'succeeded'")
            .await,
        0
    );
    assert_eq!(world.duplicates("dedup_key").await, 0);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob'")
            .await,
        total_files
    );
    assert_eq!(world.used_bytes(alice.id).await, 7);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM deletion_receipts").await,
        total_files + 2
    );
    let counted: Vec<(i64, String)> = sqlx::query_as(
        "SELECT json_extract(metadata_json, '$.files'), json_extract(metadata_json, '$.scope')
           FROM audit_events WHERE action = 'FILE_DELETED' ORDER BY id",
    )
    .fetch_all(world.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        counted.iter().map(|(files, _)| files).sum::<i64>(),
        total_files
    );
    assert!(counted
        .iter()
        .all(|(files, scope)| *files <= 500 && scope == "folder_tree"));
    let finished: (i64, i64, i64) = sqlx::query_as(
        "SELECT json_extract(metadata_json, '$.files'), json_extract(metadata_json, '$.folders'),
                json_extract(metadata_json, '$.bytes_released')
           FROM audit_events WHERE action = 'FOLDER_DELETED'",
    )
    .fetch_one(world.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(finished, (total_files, 2, 12_000 + 18));

    for seeded in &real {
        assert!(world.on_disk(&seeded.key).await);
    }
    assert_eq!(
        world.run_all(JobKind::StorageDeleteBlob).await,
        u64::try_from(total_files).unwrap()
    );
    for seeded in &real {
        assert!(!world.on_disk(&seeded.key).await);
    }
    assert!(world.on_disk(&outside.key).await);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM storage_objects WHERE state = 'deleted'")
            .await,
        total_files
    );
}

#[tokio::test]
async fn it_folder_delete_crash_matrix_resumes_from_every_boundary() {
    for completed_steps in 0..=4 {
        let world = World::start().await;
        let alice = world.stack.member("alice", HOST_A).await;
        let top = world.folder(&alice, "Top", None).await;
        let sub = world.folder(&alice, "Sub", Some(&top)).await;
        let a = world.blob(alice.id, Some(&top), "a.bin", b"aaa").await;
        let b = world.blob(alice.id, Some(&sub), "b.bin", b"bbbb").await;
        world.bulk_files(alice.id, &sub, 600, 2).await;
        let total_files = 602;
        let top_id: FolderId = top.parse().unwrap();
        assert_eq!(
            world
                .delete(&alice, &format!("{FOLDERS}/{top}"))
                .await
                .status,
            StatusCode::NO_CONTENT
        );

        let expected = [
            Step::Files { files: 500 },
            Step::Files { files: 102 },
            Step::Folders { folders: 1 },
            Step::Completed,
        ];
        for (index, want) in expected.iter().enumerate().take(completed_steps) {
            assert_eq!(
                &step(&world.tree, top_id).await.unwrap(),
                want,
                "step {index}"
            );
        }

        assert_eq!(
            world.run_all(JobKind::FoldersDeleteTree).await,
            1,
            "boundary {completed_steps}"
        );
        assert_eq!(world.count("SELECT COUNT(*) FROM files").await, 0);
        assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 0);
        assert_eq!(
            world.count("SELECT COUNT(*) FROM folder_deletions").await,
            0
        );
        assert_eq!(
            world
                .count("SELECT COUNT(*) FROM file_deletion_queue")
                .await,
            total_files
        );
        assert_eq!(world.duplicates("dedup_key").await, 0);
        assert_eq!(
            world.used_bytes(alice.id).await,
            0,
            "boundary {completed_steps}"
        );
        assert_eq!(
            world.stack.audit_rows("FOLDER_DELETED").await.len(),
            1,
            "no duplicate lifecycle event at boundary {completed_steps}"
        );
        assert_eq!(
            world.count("SELECT COUNT(*) FROM deletion_receipts").await,
            total_files + 2
        );
        assert_eq!(
            world.run_all(JobKind::StorageDeleteBlob).await,
            u64::try_from(total_files).unwrap()
        );
        assert!(!world.on_disk(&a.key).await && !world.on_disk(&b.key).await);
        assert_eq!(
            world
                .count("SELECT COUNT(*) FROM storage_objects WHERE state = 'tombstoned'")
                .await,
            0
        );
    }
}

fn code_of(fetched: &Fetched) -> (StatusCode, String) {
    (fetched.status, fetched.error_code())
}

const DELETING: (StatusCode, &str) = (StatusCode::CONFLICT, "FOLDER_DELETING");

#[tokio::test]
async fn it_deleting_subtree_is_invisible_and_refuses_every_write() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let stack = &world.stack;

    let top = world.folder(&alice, "Top", None).await;
    let child = world.folder(&alice, "Child", Some(&top)).await;
    let outside = world.folder(&alice, "Outside", None).await;
    let top_file = world
        .blob(alice.id, Some(&top), "inside-top.txt", b"t")
        .await;
    let child_file = world
        .blob(alice.id, Some(&child), "inside-child.txt", b"c")
        .await;
    let loose = world.blob(alice.id, None, "loose.txt", b"l").await;
    let guest = world
        .blob(alice.id, Some(&outside), "guest.txt", b"g")
        .await;

    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    for path in [
        format!("{FOLDERS}/{top}"),
        format!("{FOLDERS}/{child}"),
        format!("{FOLDERS}?parentId={top}"),
        format!("{FOLDERS}/tree?rootId={child}"),
        format!("{FILES}?folderId={child}"),
        format!("{FILES}/name-check?folderId={top}&name=x.txt"),
        format!("{FILES}/{}", top_file.id),
        format!("{FILES}/{}", child_file.id),
    ] {
        let read = stack.read(&path, &alice).await;
        assert_eq!(
            read.status,
            StatusCode::NOT_FOUND,
            "{path}: {}",
            read.text()
        );
    }
    let root_folders = stack.read(FOLDERS, &alice).await.json();
    assert_eq!(names_of(&root_folders), ["Outside"]);
    let tree = stack
        .read("/api/v1/folders/tree?depth=8", &alice)
        .await
        .json();
    let tree_names: Vec<&str> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(tree_names, ["Outside"]);
    let root = stack.read(FILES, &alice).await.json();
    assert_eq!(names_of(&root), ["Outside", "loose.txt"]);
    for query in ["inside", "txt"] {
        let found = stack
            .read(&format!("{FILES}?q={query}"), &alice)
            .await
            .json();
        let hits = names_of(&found);
        assert!(
            !hits.iter().any(|name| name.starts_with("inside-")),
            "{hits:?}"
        );
    }
    let scanned = stack
        .read(&format!("{FILES}?q=side-c"), &alice)
        .await
        .json();
    assert!(names_of(&scanned).is_empty(), "{scanned}");
    let outside_detail = stack
        .read(&format!("{FOLDERS}/{outside}"), &alice)
        .await
        .json();
    assert_eq!(outside_detail["fileCount"], 1);

    let member = &alice;
    let post = move |path: String, body: Value| async move {
        stack.api(Method::POST, &path, member, Some(&body)).await
    };
    let patch = move |path: String, body: Value| async move {
        stack.api(Method::PATCH, &path, member, Some(&body)).await
    };
    let is_deleting = |fetched: &Fetched, what: &str| {
        let (status, code) = code_of(fetched);
        assert_eq!(
            (status, code.as_str()),
            DELETING,
            "{what}: {}",
            fetched.text()
        );
    };

    is_deleting(
        &world.stack.create(&alice, "New", Some(&top)).await,
        "create under root",
    );
    is_deleting(
        &world.stack.create(&alice, "New", Some(&child)).await,
        "create under descendant",
    );
    is_deleting(
        &patch(format!("{FOLDERS}/{top}"), json!({ "name": "Renamed" })).await,
        "rename root",
    );
    is_deleting(
        &patch(format!("{FOLDERS}/{child}"), json!({ "description": "x" })).await,
        "describe descendant",
    );
    is_deleting(
        &post(
            format!("{FOLDERS}/{outside}/move"),
            json!({ "parentId": top }),
        )
        .await,
        "move folder in",
    );
    is_deleting(
        &post(
            format!("{FOLDERS}/{outside}/move"),
            json!({ "parentId": child }),
        )
        .await,
        "move folder into descendant",
    );
    is_deleting(
        &post(
            format!("{FOLDERS}/{child}/move"),
            json!({ "parentId": null }),
        )
        .await,
        "move out of the tree",
    );
    is_deleting(
        &post(
            format!("{FILES}/{}/move", loose.id),
            json!({ "folderId": top }),
        )
        .await,
        "move file in",
    );
    is_deleting(
        &post(
            format!("{FILES}/{}/move", loose.id),
            json!({ "folderId": child }),
        )
        .await,
        "move file into descendant",
    );
    is_deleting(
        &post(
            format!("{FILES}/{}/move", child_file.id),
            json!({ "folderId": null }),
        )
        .await,
        "move file out",
    );
    is_deleting(
        &patch(
            format!("{FILES}/{}", top_file.id),
            json!({ "name": "x.txt" }),
        )
        .await,
        "rename file",
    );
    is_deleting(
        &patch(
            format!("{FILES}/{}", child_file.id),
            json!({ "description": "x" }),
        )
        .await,
        "describe file",
    );
    let batch = post(
        format!("{FILES}/batch/move"),
        json!({ "fileIds": [loose.id, guest.id], "targetFolderId": child }),
    )
    .await;
    is_deleting(&batch, "batch move into");
    let batch = post(
        format!("{FILES}/batch/move"),
        json!({ "fileIds": [loose.id, child_file.id], "targetFolderId": outside }),
    )
    .await;
    is_deleting(&batch, "batch move out");
    let batch = post(
        format!("{FILES}/batch/move"),
        json!({ "fileIds": [loose.id], "folderIds": [top], "targetFolderId": outside }),
    )
    .await;
    is_deleting(&batch, "batch move of the root");
    let ensure = post(
        format!("{FOLDERS}/ensure-path"),
        json!({ "parentId": child, "segments": ["a", "b"] }),
    )
    .await;
    is_deleting(&ensure, "ensure-path under");
    let through = post(
        format!("{FOLDERS}/ensure-path"),
        json!({ "parentId": null, "segments": ["Top", "Child", "deeper"] }),
    )
    .await;
    is_deleting(&through, "ensure-path through the deleting root");

    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}' AND folder_id IS NULL",
                loose.id
            ))
            .await,
        1,
        "every refused batch rolled back"
    );
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}' AND folder_id = '{outside}'",
                guest.id
            ))
            .await,
        1
    );
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 3);

    let sibling = world.stack.create(&alice, "Top", None).await;
    assert_eq!(sibling.status, StatusCode::CREATED);
    assert_eq!(
        sibling.json()["name"],
        "Top (1)",
        "keep-both next to a deleting sibling"
    );

    let foreign = world.stack.create(&bob, "Mine", Some(&top)).await;
    assert_eq!(
        code_of(&foreign),
        (StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND".to_owned())
    );
    assert!(!foreign.text().contains("DELETING"));

    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    let gone = world.stack.create(&alice, "New", Some(&top)).await;
    assert_eq!(
        code_of(&gone),
        (StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND".to_owned())
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM folders WHERE name <> 'Outside' AND name <> 'Top (1)'")
            .await,
        0
    );
}

fn names_of(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

type Race<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = (&'static str, Fetched)> + 'a>>;

fn race<'a>(
    label: &'static str,
    future: impl std::future::Future<Output = Fetched> + 'a,
) -> Race<'a> {
    Box::pin(async move { (label, future.await) })
}

fn assert_outcome(label: &str, fetched: &Fetched, accepted: StatusCode) -> bool {
    if fetched.status == accepted {
        return true;
    }
    assert_eq!(
        (fetched.status, fetched.error_code().as_str()),
        DELETING,
        "{label}: {}",
        fetched.text()
    );
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_delete_races_create_and_ensure_path_through_the_writer() {
    for round in 0..10 {
        let world = World::start().await;
        let alice = world.stack.member("alice", HOST_A).await;
        let top = world.folder(&alice, "Top", None).await;
        let child = world.folder(&alice, "Child", Some(&top)).await;
        world
            .blob(alice.id, Some(&child), "seed.txt", b"seed")
            .await;
        let stack = &world.stack;
        let ensure_body = json!({ "parentId": child, "segments": ["p", "q"] });

        let results = futures_util::future::join_all(vec![
            race("delete", world.delete(&alice, &format!("{FOLDERS}/{top}"))),
            race("create-a", stack.create(&alice, "A", Some(&top))),
            race("create-b", stack.create(&alice, "B", Some(&child))),
            race("create-c", stack.create(&alice, "C", Some(&child))),
            race(
                "ensure-a",
                stack.api(
                    Method::POST,
                    &format!("{FOLDERS}/ensure-path"),
                    &alice,
                    Some(&ensure_body),
                ),
            ),
            race(
                "ensure-b",
                stack.api(
                    Method::POST,
                    &format!("{FOLDERS}/ensure-path"),
                    &alice,
                    Some(&ensure_body),
                ),
            ),
        ])
        .await;
        for (label, fetched) in &results {
            match *label {
                "delete" => assert_eq!(fetched.status, StatusCode::NO_CONTENT, "round {round}"),
                "create-a" | "create-b" | "create-c" => {
                    assert_outcome(label, fetched, StatusCode::CREATED);
                }
                _ => {
                    assert_outcome(label, fetched, StatusCode::OK);
                }
            }
        }
        assert_eq!(
            world.count("SELECT COUNT(*) FROM folder_deletions").await,
            1
        );
        world.run_all(JobKind::FoldersDeleteTree).await;
        assert_eq!(
            world.count("SELECT COUNT(*) FROM folders").await,
            0,
            "round {round}"
        );
        assert_eq!(world.count("SELECT COUNT(*) FROM files").await, 0);
        assert_eq!(
            world.count("SELECT COUNT(*) FROM folder_deletions").await,
            0
        );
        assert_eq!(world.used_bytes(alice.id).await, 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_delete_races_moves_and_renames_through_the_writer() {
    for round in 0..10 {
        let world = World::start().await;
        let alice = world.stack.member("alice", HOST_A).await;
        let top = world.folder(&alice, "Top", None).await;
        let inner = world.folder(&alice, "Inner", Some(&top)).await;
        let inside = world.blob(alice.id, Some(&top), "inside.txt", b"in").await;
        let outer = world.folder(&alice, "Outer", None).await;
        let guest = world.blob(alice.id, Some(&outer), "guest.txt", b"gg").await;
        let loose = world.blob(alice.id, None, "loose.txt", b"l").await;
        let stack = &world.stack;
        let post = |path: String, body: Value| {
            let alice = &alice;
            async move { stack.api(Method::POST, &path, alice, Some(&body)).await }
        };

        let results = futures_util::future::join_all(vec![
            race("delete", world.delete(&alice, &format!("{FOLDERS}/{top}"))),
            race(
                "folder-in",
                post(
                    format!("{FOLDERS}/{outer}/move"),
                    json!({ "parentId": top }),
                ),
            ),
            race(
                "file-in",
                post(
                    format!("{FILES}/{}/move", loose.id),
                    json!({ "folderId": inner }),
                ),
            ),
            race(
                "folder-out",
                post(
                    format!("{FOLDERS}/{inner}/move"),
                    json!({ "parentId": null }),
                ),
            ),
            race(
                "file-out",
                post(
                    format!("{FILES}/{}/move", inside.id),
                    json!({ "folderId": null }),
                ),
            ),
            race(
                "rename",
                stack.api(
                    Method::PATCH,
                    &format!("{FOLDERS}/{top}"),
                    &alice,
                    Some(&json!({ "name": "Renamed" })),
                ),
            ),
            race(
                "batch-in",
                post(
                    format!("{FILES}/batch/move"),
                    json!({ "fileIds": [loose.id], "targetFolderId": top }),
                ),
            ),
        ])
        .await;
        let mut moved = std::collections::HashMap::new();
        for (label, fetched) in &results {
            if *label == "delete" {
                assert_eq!(fetched.status, StatusCode::NO_CONTENT, "round {round}");
            } else {
                moved.insert(*label, assert_outcome(label, fetched, StatusCode::OK));
            }
        }
        world.run_all(JobKind::FoldersDeleteTree).await;

        let survives = |id: String, table: &'static str| {
            let world = &world;
            async move {
                world
                    .count(&format!("SELECT COUNT(*) FROM {table} WHERE id = '{id}'"))
                    .await
                    == 1
            }
        };
        let outer_moved_in = moved["folder-in"];
        assert_eq!(
            survives(outer.clone(), "folders").await,
            !outer_moved_in,
            "round {round}"
        );
        assert_eq!(
            survives(guest.id.clone(), "files").await,
            !outer_moved_in,
            "round {round}"
        );
        let loose_moved_in = moved["file-in"] || moved["batch-in"];
        assert_eq!(
            survives(loose.id.clone(), "files").await,
            !loose_moved_in,
            "round {round}"
        );
        assert_eq!(
            survives(inner.clone(), "folders").await,
            moved["folder-out"],
            "round {round}"
        );
        assert_eq!(
            survives(inside.id.clone(), "files").await,
            moved["file-out"],
            "round {round}"
        );
        assert!(!survives(top.clone(), "folders").await);
        assert_eq!(
            world.count("SELECT COUNT(*) FROM folder_deletions").await,
            0
        );
        assert_eq!(
            world
                .count("SELECT COUNT(*) FROM folders WHERE deleting = 1")
                .await,
            0
        );
        let remaining: i64 = world
            .count("SELECT COALESCE(SUM(size_bytes), 0) FROM files")
            .await;
        assert_eq!(world.used_bytes(alice.id).await, remaining, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_delete_duplicate_requests_race_to_one_logical_deletion() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let folder = world.folder(&alice, "Dup", None).await;
    world.blob(alice.id, Some(&folder), "in.txt", b"in").await;
    let file = world.blob(alice.id, None, "solo.txt", b"solo").await;

    let folder_path = format!("{FOLDERS}/{folder}");
    let file_path = format!("{FILES}/{}", file.id);
    let mut calls = Vec::new();
    for _ in 0..6 {
        calls.push(race("folder", world.delete(&alice, &folder_path)));
        calls.push(race("file", world.delete(&alice, &file_path)));
    }
    for (label, fetched) in futures_util::future::join_all(calls).await {
        assert_eq!(
            fetched.status,
            StatusCode::NO_CONTENT,
            "{label}: {}",
            fetched.text()
        );
    }
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'folders.delete_tree'")
            .await,
        1
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_DELETED'")
            .await,
        1
    );
    assert_eq!(world.used_bytes(alice.id).await, 2);
    world.run_all(JobKind::FoldersDeleteTree).await;
    assert_eq!(world.used_bytes(alice.id).await, 0);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FOLDER_DELETED'")
            .await,
        1
    );
}

#[tokio::test]
async fn it_delete_create_before_claim_is_swept_up_by_the_tree_deletion() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let top = world.folder(&alice, "Top", None).await;
    let made = world.folder(&alice, "Made-before", Some(&top)).await;
    let file = world.blob(alice.id, Some(&made), "late.txt", b"late").await;
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    world.run_all(JobKind::FoldersDeleteTree).await;
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 0);
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                file.id
            ))
            .await,
        0
    );
    assert_eq!(world.used_bytes(alice.id).await, 0);
}

#[tokio::test]
async fn it_batch_delete_reports_each_item_and_never_rolls_back_the_others() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let f1 = world.blob(alice.id, None, "one.txt", b"1").await;
    let f2 = world.blob(alice.id, None, "two.txt", b"22").await;
    let dir = world.folder(&alice, "Dir", None).await;
    let nested = world.folder(&alice, "Nested", Some(&dir)).await;
    let in_dir = world.blob(alice.id, Some(&dir), "in-dir.txt", b"333").await;
    let in_nested = world
        .blob(alice.id, Some(&nested), "in-nested.txt", b"4444")
        .await;
    let theirs = world.blob(bob.id, None, "theirs.txt", b"bob").await;
    let phantom_file = world.stack.fresh_id();
    let phantom_folder = world.stack.fresh_id();

    let mixed = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({
                "fileIds": [f1.id, phantom_file, theirs.id, in_dir.id],
                "folderIds": [nested, dir, phantom_folder]
            }),
        )
        .await;
    assert_eq!(mixed.status, StatusCode::OK, "{}", mixed.text());
    let mixed = mixed.json();
    assert_eq!(
        mixed["succeeded"],
        json!([f1.id, in_dir.id, nested, dir]),
        "request order, files then folders; a nested folder is covered by its ancestor"
    );
    assert_eq!(
        mixed["failed"],
        json!([
            { "id": phantom_file, "code": "FILE_NOT_FOUND" },
            { "id": theirs.id, "code": "FILE_NOT_FOUND" },
            { "id": phantom_folder, "code": "FOLDER_NOT_FOUND" }
        ])
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'folders.delete_tree'")
            .await,
        1
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        1
    );
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                theirs.id
            ))
            .await,
        1
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM file_deletion_queue")
            .await,
        1,
        "only f1 so far"
    );
    assert_eq!(world.used_bytes(alice.id).await, 2 + 3 + 4);

    world.run_all(JobKind::FoldersDeleteTree).await;
    assert_eq!(world.used_bytes(alice.id).await, 2);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM audit_events WHERE action = 'FOLDER_DELETED'")
            .await,
        1
    );
    assert!(world.on_disk(&in_nested.key).await);

    let repeated = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "fileIds": [f1.id, in_dir.id], "folderIds": [dir, nested] }),
        )
        .await;
    assert_eq!(repeated.status, StatusCode::OK, "{}", repeated.text());
    assert_eq!(repeated.json()["failed"], json!([]));
    assert_eq!(repeated.json()["succeeded"].as_array().unwrap().len(), 4);

    let reversed_dir = world.folder(&alice, "Outer", None).await;
    let reversed_inner = world.folder(&alice, "Inner", Some(&reversed_dir)).await;
    let ordered = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "folderIds": [reversed_inner, reversed_dir] }),
        )
        .await;
    assert_eq!(ordered.status, StatusCode::OK, "{}", ordered.text());
    assert_eq!(
        ordered.json()["succeeded"],
        json!([reversed_inner, reversed_dir])
    );
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM jobs WHERE dedup_key = 'folder-del:{reversed_inner}'"
            ))
            .await,
        0,
        "the ancestor is claimed first, so the nested selection is not enqueued twice"
    );
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM jobs WHERE kind = 'folders.delete_tree' AND state = 'pending'")
            .await,
        1
    );

    let only_files = world
        .post(&alice, BATCH_DELETE, &json!({ "fileIds": [f2.id] }))
        .await;
    assert_eq!(only_files.status, StatusCode::OK);
    assert_eq!(only_files.json()["succeeded"], json!([f2.id]));
    let only_folders = world.folder(&alice, "Alone", None).await;
    let only_folders = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "folderIds": [only_folders] }),
        )
        .await;
    assert_eq!(
        only_folders.status,
        StatusCode::OK,
        "{}",
        only_folders.text()
    );
    assert_eq!(world.used_bytes(alice.id).await, 0);
}

#[tokio::test]
async fn it_batch_delete_validates_the_selection() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let mine = world.blob(alice.id, None, "mine.txt", b"m").await;
    let theirs = world.blob(bob.id, None, "theirs.txt", b"t").await;

    for body in [
        json!({}),
        json!({ "fileIds": [], "folderIds": [] }),
        json!({ "fileIds": [mine.id, mine.id] }),
        json!({ "folderIds": "nope" }),
        json!({ "fileIds": [], "extra": 1 }),
    ] {
        let rejected = world.post(&alice, BATCH_DELETE, &body).await;
        assert_eq!(
            rejected.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            rejected.text()
        );
        assert_eq!(rejected.error_code(), "VALIDATION_ERROR");
    }
    let ids: Vec<String> = (0..501).map(|_| world.stack.fresh_id()).collect();
    let split = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "fileIds": &ids[..300], "folderIds": &ids[300..] }),
        )
        .await;
    assert_eq!(split.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(split.error_code(), "BATCH_TOO_LARGE");
    assert_eq!(split.json()["error"]["details"]["maxItems"], 500);
    let malformed = world
        .post(&alice, BATCH_DELETE, &json!({ "fileIds": ["not-an-id"] }))
        .await;
    assert_eq!(malformed.status, StatusCode::NOT_FOUND);
    assert_eq!(malformed.error_code(), "FILE_NOT_FOUND");

    let all_foreign = world
        .post(&alice, BATCH_DELETE, &json!({ "fileIds": [theirs.id] }))
        .await;
    assert_eq!(
        all_foreign.status,
        StatusCode::NOT_FOUND,
        "{}",
        all_foreign.text()
    );
    assert_eq!(all_foreign.error_code(), "FILE_NOT_FOUND");
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                theirs.id
            ))
            .await,
        1
    );

    let route = world
        .stack
        .read(&format!("{FILES}/batch/delete"), &alice)
        .await;
    assert_eq!(
        route.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "GET is not swallowed by /files/{{id}}"
    );
    let impact_route = world
        .stack
        .read(&format!("{FILES}/batch/deletion-impact"), &alice)
        .await;
    assert_eq!(impact_route.status, StatusCode::METHOD_NOT_ALLOWED);

    let exact: Vec<String> = (0..500).map(|_| world.stack.fresh_id()).collect();
    let boundary = world
        .post(&alice, BATCH_DELETE, &json!({ "fileIds": exact }))
        .await;
    assert_eq!(
        boundary.status,
        StatusCode::NOT_FOUND,
        "500 ids is accepted and all fail the same way"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_folder_delete_large_tree_batches_stay_bounded_and_interleave_with_other_writes() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let top = world.folder(&alice, "Top", None).await;
    let owner = alice.id;
    world
        .stack
        .execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1100)
             INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             SELECT printf('0192f3a1-0c01-7000-8000-%012x', i), '{owner}', '{top}',
                    printf('flat%04d', i), printf('flat%04d', i), 1, '{SEEDED_AT}', '{SEEDED_AT}'
               FROM n"
        ))
        .await;
    let mut parent = top.clone();
    for level in 1..=64_u32 {
        let id = format!("0192f3a1-0c02-7000-8000-{level:012x}");
        world
            .stack
            .execute(&format!(
                "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
                 VALUES ('{id}', '{owner}', '{parent}', 'chain{level}', 'chain{level}', {level}, '{SEEDED_AT}', '{SEEDED_AT}')"
            ))
            .await;
        parent = id;
    }
    world.bulk_files(owner, &top, 1_150, 3).await;
    world.bulk_files(owner, &parent, 10, 3).await;
    let total_folders = 1 + 1_100 + 64;
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folders").await,
        total_folders
    );
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let top_id: FolderId = top.parse().unwrap();
    let tree = world.tree.clone();
    let drainer = tokio::spawn(async move {
        let mut steps = Vec::new();
        loop {
            let next = step(&tree, top_id).await.unwrap();
            steps.push(next);
            if matches!(next, Step::Completed | Step::Gone) {
                return steps;
            }
        }
    });
    let mut created = 0;
    while !drainer.is_finished() && created < 200 {
        let made = world.stack.create(&bob, &format!("B{created}"), None).await;
        assert_eq!(made.status, StatusCode::CREATED, "{}", made.text());
        created += 1;
    }
    let steps = drainer.await.unwrap();
    assert!(created > 0, "another owner wrote while the tree drained");

    let files: Vec<u64> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Files { files } => Some(*files),
            _ => None,
        })
        .collect();
    let folders: Vec<u64> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Folders { folders } => Some(*folders),
            _ => None,
        })
        .collect();
    assert_eq!(files, [500, 500, 160]);
    assert_eq!(
        folders,
        [500, 500, 164],
        "the root is removed by the final step"
    );
    assert_eq!(steps.last(), Some(&Step::Completed));
    assert!(files.iter().chain(&folders).all(|batch| *batch <= 500));
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM folders WHERE owner_id = '{owner}'"
            ))
            .await,
        0
    );
    assert_eq!(world.count("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(world.used_bytes(owner).await, 0);
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, created);
}

impl World {
    pub(super) async fn session(&self, owner: UserId, state: &str, target: &str) -> String {
        self.session_to(owner, state, Some(target)).await
    }

    pub(super) async fn session_to(
        &self,
        owner: UserId,
        state: &str,
        target: Option<&str>,
    ) -> String {
        let id = FolderId::generate(&self.stack.clock).to_string();
        let target = target.map_or_else(|| "NULL".to_owned(), |folder| format!("'{folder}'"));
        self.stack
            .execute(&format!(
                "INSERT INTO transfer_sessions (id, context, user_id, provider, state, target_folder_id, created_at, updated_at, expires_at)
                 VALUES ('{id}', 'my_files', '{owner}', 'local', '{state}', {target}, '{SEEDED_AT}', '{SEEDED_AT}', '2027-01-01T00:00:00.000Z')"
            ))
            .await;
        id
    }

    pub(super) async fn transfer_target(&self, id: &str) -> (Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT target_folder_id, deleted_target_folder_id FROM transfer_sessions WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn it_folder_delete_defers_while_a_live_transfer_targets_the_subtree() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let top = world.folder(&alice, "Top", None).await;
    let child = world.folder(&alice, "Child", Some(&top)).await;
    let inner = world
        .blob(alice.id, Some(&child), "inner.txt", b"inner")
        .await;
    let history = world.session(alice.id, "completed", &top).await;
    let live = world.session(alice.id, "uploading", &child).await;
    let retryable = world.session(alice.id, "failed", &child).await;

    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                inner.id
            ))
            .await,
        0,
        "files do not depend on the transfer rows"
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folders").await,
        2,
        "the folder rows wait"
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        1
    );
    for (id, state, target) in [
        (&live, "uploading", Some(&child)),
        (&retryable, "failed", Some(&child)),
        (&history, "completed", Some(&top)),
    ] {
        let row: (String, Option<String>) =
            sqlx::query_as("SELECT state, target_folder_id FROM transfer_sessions WHERE id = ?1")
                .bind(id)
                .fetch_one(world.stack.pools.reader().executor())
                .await
                .unwrap();
        assert_eq!(
            row,
            (state.to_owned(), target.cloned()),
            "no transfer row is edited yet"
        );
        if state != "completed" {
            let marker: Option<String> = sqlx::query_scalar(
                "SELECT deleted_target_folder_id FROM transfer_sessions WHERE id = ?1",
            )
            .bind(id)
            .fetch_one(world.stack.pools.reader().executor())
            .await
            .unwrap();
            assert_eq!(
                marker, None,
                "a live or retryable session is never detached"
            );
        }
    }
    let waiting: (String, String) = sqlx::query_as(
        "SELECT state, run_at FROM jobs WHERE kind = 'folders.delete_tree' AND state = 'pending'",
    )
    .fetch_one(world.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(waiting.0, "pending");
    assert_eq!(
        world.run_all(JobKind::FoldersDeleteTree).await,
        0,
        "the deferral is a scheduled continuation, not a busy retry"
    );
    assert_eq!(
        world
            .count("SELECT attempts FROM jobs WHERE kind = 'folders.delete_tree' AND state = 'pending'")
            .await,
        0
    );

    world
        .stack
        .execute(&format!(
            "UPDATE transfer_sessions SET state = 'canceled' WHERE id IN ('{live}', '{retryable}')"
        ))
        .await;
    world.stack.clock.advance(Duration::from_secs(6 * 60));
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    assert_eq!(world.count("SELECT COUNT(*) FROM folders").await, 0);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM folder_deletions").await,
        0
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM transfer_sessions").await,
        3,
        "history is kept"
    );
    for (id, original) in [(&history, &top), (&live, &child), (&retryable, &child)] {
        assert_eq!(
            world.transfer_target(id).await,
            (None, Some(original.clone()))
        );
    }
    assert_eq!(world.stack.audit_rows("FOLDER_DELETED").await.len(), 1);
}

#[tokio::test]
async fn it_deleting_child_is_excluded_from_visible_parent_listings_and_totals() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let stack = &world.stack;
    let parent = world.folder(&alice, "Parent", None).await;
    let keep = world.folder(&alice, "Keep", Some(&parent)).await;
    let gone = world.folder(&alice, "Gone", Some(&parent)).await;
    world.blob(alice.id, Some(&parent), "p.txt", b"123").await;
    world.blob(alice.id, Some(&keep), "k.txt", b"12345").await;
    world.blob(alice.id, Some(&gone), "g.txt", b"1234567").await;

    let before = stack
        .read(&format!("{FOLDERS}/{parent}"), &alice)
        .await
        .json();
    assert_eq!(
        (
            before["fileCount"].clone(),
            before["subfolderCount"].clone(),
            before["totalBytes"].clone()
        ),
        (3.into(), 2.into(), 15.into())
    );
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{gone}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let after = stack
        .read(&format!("{FOLDERS}/{parent}"), &alice)
        .await
        .json();
    assert_eq!(
        (
            after["fileCount"].clone(),
            after["subfolderCount"].clone(),
            after["totalBytes"].clone()
        ),
        (2.into(), 1.into(), 8.into()),
        "a deleting child no longer counts toward its visible parent"
    );
    let children = stack
        .read(&format!("{FOLDERS}?parentId={parent}"), &alice)
        .await
        .json();
    assert_eq!(names_of(&children), ["Keep"]);
    assert_eq!(children["totalCount"], 1);
    let browse = stack
        .read(&format!("{FILES}?folderId={parent}"), &alice)
        .await
        .json();
    assert_eq!(names_of(&browse), ["Keep", "p.txt"]);
    assert_eq!(browse["totalCount"], 2);
    for sort in ["name", "size", "createdAt"] {
        let sorted = stack
            .read(
                &format!("{FILES}?folderId={parent}&sort={sort}:desc"),
                &alice,
            )
            .await
            .json();
        assert_eq!(sorted["totalCount"], 2, "{sort}");
        assert!(!names_of(&sorted).contains(&"Gone".to_owned()), "{sort}");
    }
    let tree = stack
        .read(&format!("/api/v1/folders/tree?rootId={parent}"), &alice)
        .await
        .json();
    let nodes: Vec<&str> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(nodes, ["Parent", "Keep"]);
    let roots = stack.read(FOLDERS, &alice).await.json();
    assert_eq!(roots["items"][0]["totalBytes"], 8);
    assert!(names_of(&stack.read(&format!("{FILES}?q=g.txt"), &alice).await.json()).is_empty());
    let impact = stack
        .read(&format!("{FOLDERS}/{parent}/deletion-impact"), &alice)
        .await
        .json();
    assert_eq!(
        (
            impact["files"].clone(),
            impact["folders"].clone(),
            impact["totalBytes"].clone()
        ),
        (2.into(), 2.into(), 8.into()),
        "the impact preview ignores a subtree that is already being deleted"
    );
}

#[tokio::test]
async fn it_folder_delete_keeps_the_original_destination_of_terminal_transfers() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let top = world.folder(&alice, "Top", None).await;
    let child = world.folder(&alice, "Child", Some(&top)).await;
    let elsewhere = world.folder(&alice, "Elsewhere", None).await;
    let to_root = world.session_to(alice.id, "completed", None).await;
    let completed = world.session(alice.id, "completed", &child).await;
    let canceled = world.session(alice.id, "canceled", &top).await;
    let expired = world.session(alice.id, "expired", &child).await;
    let unrelated = world.session(alice.id, "completed", &elsewhere).await;

    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    assert_eq!(
        world
            .count("SELECT COUNT(*) FROM folders WHERE name <> 'Elsewhere'")
            .await,
        0
    );

    assert_eq!(
        world.transfer_target(&to_root).await,
        (None, None),
        "root history is not a deleted folder"
    );
    assert_eq!(
        world.transfer_target(&completed).await,
        (None, Some(child.clone()))
    );
    assert_eq!(
        world.transfer_target(&canceled).await,
        (None, Some(top.clone()))
    );
    assert_eq!(
        world.transfer_target(&expired).await,
        (None, Some(child.clone()))
    );
    assert_eq!(
        world.transfer_target(&unrelated).await,
        (Some(elsewhere.clone()), None),
        "a session whose folder survives is untouched"
    );
    assert_eq!(
        world.count("SELECT COUNT(*) FROM transfer_sessions").await,
        5
    );
    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(world.stack.pools.reader().executor())
        .await
        .unwrap();
    assert!(violations.is_empty(), "{violations:?}");

    let rewrite = |session: String, value: &'static str| {
        let world = &world;
        async move {
            world
                .stack
                .pools
                .write_tx(&world.stack.clock, "test.rewrite_history", async |tx| {
                    sqlx::query(&format!(
                        "UPDATE transfer_sessions SET deleted_target_folder_id = {value} WHERE id = '{session}'"
                    ))
                    .execute(tx.executor())
                    .await
                    .map_err(crate::infra::db::DbError::from)?;
                    Ok::<(), crate::infra::db::DbError>(())
                })
                .await
        }
    };
    assert!(
        rewrite(completed.clone(), "NULL").await.is_err(),
        "the marker cannot be cleared"
    );
    assert!(
        rewrite(completed.clone(), "'0192f3a1-0000-7000-8000-00000000ffff'")
            .await
            .is_err(),
        "nor rewritten"
    );
    assert_eq!(
        world.transfer_target(&completed).await,
        (None, Some(child.clone()))
    );
    let live = world.session_to(alice.id, "uploading", None).await;
    assert!(
        rewrite(live, format!("'{child}'").leak()).await.is_err(),
        "only a terminal session can carry a deleted destination"
    );

    let second = world.folder(&alice, "Second", None).await;
    let later = world.session(alice.id, "completed", &second).await;
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{second}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    world.run_all(JobKind::FoldersDeleteTree).await;
    assert_eq!(world.transfer_target(&later).await, (None, Some(second)));
    assert_eq!(
        world.transfer_target(&completed).await,
        (None, Some(child)),
        "a later deletion never rewrites earlier history"
    );
}

#[tokio::test]
async fn it_batch_delete_with_no_success_is_never_http_200() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let mine = world.blob(alice.id, None, "mine.txt", b"m").await;
    let mine_folder = world.folder(&alice, "Mine", None).await;
    let theirs = world.blob(bob.id, None, "theirs.txt", b"t").await;
    let their_folder = world.folder(&bob, "Theirs", None).await;
    let ghost_file = world.stack.fresh_id();
    let ghost_folder = world.stack.fresh_id();

    let all_files = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "fileIds": [theirs.id, ghost_file] }),
        )
        .await;
    assert_eq!(
        (all_files.status, all_files.error_code().as_str()),
        (StatusCode::NOT_FOUND, "FILE_NOT_FOUND")
    );
    assert_eq!(
        all_files.json()["error"]["details"]["failed"],
        json!([
            { "id": theirs.id, "code": "FILE_NOT_FOUND" },
            { "id": ghost_file, "code": "FILE_NOT_FOUND" }
        ])
    );
    let all_folders = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "folderIds": [their_folder, ghost_folder] }),
        )
        .await;
    assert_eq!(
        (all_folders.status, all_folders.error_code().as_str()),
        (StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND")
    );

    let mixed = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "fileIds": [theirs.id], "folderIds": [their_folder, ghost_folder] }),
        )
        .await;
    assert_eq!(
        mixed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        mixed.text()
    );
    assert_eq!(mixed.error_code(), "BATCH_DELETE_FAILED");
    assert_eq!(
        mixed.json()["error"]["details"]["failed"],
        json!([
            { "id": theirs.id, "code": "FILE_NOT_FOUND" },
            { "id": their_folder, "code": "FOLDER_NOT_FOUND" },
            { "id": ghost_folder, "code": "FOLDER_NOT_FOUND" }
        ])
    );
    assert!(mixed.json().get("succeeded").is_none());

    for body in [
        json!({ "fileIds": [theirs.id] }),
        json!({ "fileIds": [ghost_file], "folderIds": [ghost_folder] }),
        json!({ "folderIds": [their_folder] }),
    ] {
        let failed = world.post(&alice, BATCH_DELETE, &body).await;
        assert_ne!(failed.status, StatusCode::OK, "{body}");
        assert!(failed.status.is_client_error());
    }
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{}'",
                theirs.id
            ))
            .await,
        1
    );
    assert_eq!(
        world
            .count(&format!(
                "SELECT COUNT(*) FROM folders WHERE id = '{their_folder}'"
            ))
            .await,
        1
    );
    assert_eq!(world.count("SELECT COUNT(*) FROM jobs").await, 0);
    assert_eq!(
        world.count("SELECT COUNT(*) FROM deletion_receipts").await,
        0
    );

    let all_ok = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "fileIds": [mine.id], "folderIds": [mine_folder] }),
        )
        .await;
    assert_eq!(all_ok.status, StatusCode::OK);
    assert_eq!(all_ok.json()["failed"], json!([]));
    let partial = world
        .post(
            &alice,
            BATCH_DELETE,
            &json!({ "fileIds": [mine.id, ghost_file] }),
        )
        .await;
    assert_eq!(
        partial.status,
        StatusCode::OK,
        "an already-deleted item succeeds, the ghost fails"
    );
    assert_eq!(partial.json()["succeeded"], json!([mine.id]));
    assert_eq!(
        partial.json()["failed"],
        json!([{ "id": ghost_file, "code": "FILE_NOT_FOUND" }])
    );
}

#[tokio::test]
async fn it_recursive_delete_leaves_a_receipt_for_every_descendant() {
    let world = World::start().await;
    let alice = world.stack.member("alice", HOST_A).await;
    let bob = world.stack.member("bob", HOST_B).await;
    let top = world.folder(&alice, "Top", None).await;
    let middle = world.folder(&alice, "Middle", Some(&top)).await;
    let bottom = world.folder(&alice, "Bottom", Some(&middle)).await;
    let files = [
        world.blob(alice.id, Some(&top), "a.txt", b"a").await,
        world.blob(alice.id, Some(&middle), "b.txt", b"bb").await,
        world.blob(alice.id, Some(&bottom), "c.txt", b"ccc").await,
        world.blob(alice.id, Some(&bottom), "d.txt", b"").await,
    ];
    let folders = [top.clone(), middle.clone(), bottom.clone()];

    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    world.run_all(JobKind::FoldersDeleteTree).await;
    world.run_all(JobKind::StorageDeleteBlob).await;

    let receipts: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, resource_kind, owner_id FROM deletion_receipts ORDER BY id")
            .fetch_all(world.stack.pools.reader().executor())
            .await
            .unwrap();
    let mut expected: Vec<(String, String, String)> = folders
        .iter()
        .map(|id| (id.clone(), "folder".to_owned(), alice.id.to_string()))
        .chain(
            files
                .iter()
                .map(|f| (f.id.clone(), "file".to_owned(), alice.id.to_string())),
        )
        .collect();
    expected.sort();
    assert_eq!(
        receipts, expected,
        "root, descendant folders and descendant files"
    );

    world
        .stack
        .execute(
            "DELETE FROM jobs; DELETE FROM file_deletion_queue; DELETE FROM storage_objects; DELETE FROM audit_events",
        )
        .await;
    let root = world._root.path().to_path_buf();
    let clock = world.stack.clock.clone();
    let alice_id = alice.id;
    let bob_id = bob.id;
    let World { stack, .. } = world;
    stack.stop().await;

    let restarted = Stack::start(&root, &clock).await;
    let alice = Member {
        id: alice_id,
        creds: restarted.signed_in("alice", HOST_A).await,
    };
    let bob = Member {
        id: bob_id,
        creds: restarted.signed_in("bob", HOST_B).await,
    };
    for file in &files {
        let path = format!("{FILES}/{}", file.id);
        let owner = restarted.api(Method::DELETE, &path, &alice, None).await;
        assert_eq!(
            owner.status,
            StatusCode::NO_CONTENT,
            "{path}: {}",
            owner.text()
        );
        let other = restarted.api(Method::DELETE, &path, &bob, None).await;
        assert_eq!(
            (other.status, other.error_code().as_str()),
            (StatusCode::NOT_FOUND, "FILE_NOT_FOUND")
        );
    }
    for folder in &folders {
        let path = format!("{FOLDERS}/{folder}");
        let owner = restarted.api(Method::DELETE, &path, &alice, None).await;
        assert_eq!(
            owner.status,
            StatusCode::NO_CONTENT,
            "{path}: {}",
            owner.text()
        );
        let other = restarted.api(Method::DELETE, &path, &bob, None).await;
        assert_eq!(
            (other.status, other.error_code().as_str()),
            (StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND")
        );
    }
    let crossed = restarted
        .api(
            Method::DELETE,
            &format!("{FILES}/{}", folders[0]),
            &alice,
            None,
        )
        .await;
    assert_eq!(
        crossed.status,
        StatusCode::NOT_FOUND,
        "a folder receipt never satisfies a file delete"
    );
    assert_eq!(
        restarted.scalar_i64("SELECT COUNT(*) FROM jobs").await,
        0,
        "re-deleting starts nothing"
    );
    restarted.stop().await;
}

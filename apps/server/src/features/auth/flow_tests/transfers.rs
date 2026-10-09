use std::collections::HashSet;

use super::folders::{Member, HOST_A, HOST_B, HOST_WORK, SEEDED_AT};
use super::profile::assert_code;
use super::*;
use crate::features::folders::FolderId;
use crate::features::transfers::TransferStorage;
use crate::storage::health::StorageHealth;
use crate::storage::key::ObjectKey;
use crate::storage::planning::UploadPlanner;
use crate::storage::s3::profile::{ProviderProfile, GIB, MIB, TIB};

pub(super) const SESSIONS: &str = "/api/v1/transfers/sessions";
pub(super) const KEY_A: &str = "session-key-0001-aaaaaaaaaaaa";
pub(super) const KEY_B: &str = "session-key-0002-bbbbbbbbbbbb";
pub(super) const KEY_C: &str = "session-key-0003-cccccccccccc";
pub(super) const EXPIRES: &str = "2026-10-02T12:00:00.000Z";

pub(super) fn sized(id: &str, name: &str, size: u64) -> Value {
    json!({ "clientId": id, "name": name, "sizeBytes": size })
}

pub(super) fn unsized_file(id: &str, name: &str) -> Value {
    json!({ "clientId": id, "name": name, "sizeBytes": null })
}

pub(super) fn pathed(id: &str, path: &str, size: u64) -> Value {
    let name = path.rsplit('/').next().unwrap();
    json!({ "clientId": id, "name": name, "sizeBytes": size, "relativePath": path })
}

pub(super) fn request_body(target: Option<&str>, files: &[Value]) -> Value {
    json!({ "target": { "kind": "my_files", "folderId": target }, "files": files })
}

pub(super) fn request_in(folder: &str, files: &[Value]) -> Value {
    request_body(Some(folder), files)
}

fn session_request(member: &Member, body: &Value, key: Option<&str>) -> Request {
    let creds = &member.creds;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(SESSIONS)
        .header(ORIGIN, BASE_URL)
        .header(CSRF_HEADER, &creds.csrf)
        .header(CONTENT_TYPE, "application/json")
        .header(
            COOKIE,
            format!("palmr_session={}; palmr_csrf={}", creds.session, creds.csrf),
        );
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

pub(super) struct Counts {
    pub sessions: i64,
    pub items: i64,
    pub reservations: i64,
    pub held: i64,
    pub folders: i64,
    pub objects: i64,
}

impl Stack {
    pub(super) async fn open(&self, member: &Member, body: &Value) -> Fetched {
        self.send(with_peer(session_request(member, body, None), HOST_WORK))
            .await
    }

    pub(super) async fn open_keyed(&self, member: &Member, body: &Value, key: &str) -> Fetched {
        self.send(with_peer(
            session_request(member, body, Some(key)),
            HOST_WORK,
        ))
        .await
    }

    pub(super) async fn opened(&self, member: &Member, files: &[Value]) -> Value {
        let created = self.open(member, &request_body(None, files)).await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        created.json()
    }

    pub(super) async fn session_detail(&self, member: &Member, id: &str) -> Fetched {
        self.read(&format!("{SESSIONS}/{id}"), member).await
    }

    pub(super) async fn cancel_session(&self, member: &Member, id: &str) -> Fetched {
        self.api(Method::DELETE, &format!("{SESSIONS}/{id}"), member, None)
            .await
    }

    pub(super) async fn complete_session(&self, member: &Member, id: &str) -> Fetched {
        self.api(
            Method::POST,
            &format!("{SESSIONS}/{id}/complete"),
            member,
            None,
        )
        .await
    }

    pub(super) async fn retry_item(&self, member: &Member, id: &str, item: &str) -> Fetched {
        self.api(
            Method::POST,
            &format!("{SESSIONS}/{id}/files/{item}/retry"),
            member,
            None,
        )
        .await
    }

    pub(super) async fn cancel_item(&self, member: &Member, id: &str, item: &str) -> Fetched {
        self.api(
            Method::DELETE,
            &format!("{SESSIONS}/{id}/files/{item}"),
            member,
            None,
        )
        .await
    }

    pub(super) async fn counts(&self) -> Counts {
        Counts {
            sessions: self.scalar_i64("SELECT COUNT(*) FROM transfer_sessions").await,
            items: self
                .scalar_i64("SELECT COUNT(*) FROM transfer_session_files")
                .await,
            reservations: self
                .scalar_i64("SELECT COUNT(*) FROM quota_reservations")
                .await,
            held: self
                .scalar_i64(
                    "SELECT COALESCE(SUM(reserved_bytes), 0) FROM quota_reservations WHERE state = 'held'",
                )
                .await,
            folders: self.scalar_i64("SELECT COUNT(*) FROM folders").await,
            objects: self.scalar_i64("SELECT COUNT(*) FROM storage_objects").await,
        }
    }

    pub(super) async fn assert_untouched(&self, folders: i64) {
        let counts = self.counts().await;
        assert_eq!(counts.sessions, 0, "no session persists");
        assert_eq!(counts.items, 0, "no item persists");
        assert_eq!(counts.reservations, 0, "no reservation persists");
        assert_eq!(counts.folders, folders, "no folder chain persists");
        assert_eq!(counts.objects, 0, "no storage object is created");
    }

    pub(super) async fn set_quota(&self, user: UserId, quota: Option<i64>) {
        let sql = match quota {
            Some(bytes) => format!(
                "UPDATE users SET quota_override_mode = 'bytes', quota_bytes = {bytes} WHERE id = '{user}'"
            ),
            None => format!(
                "UPDATE users SET quota_override_mode = 'unlimited', quota_bytes = NULL WHERE id = '{user}'"
            ),
        };
        self.execute(&sql).await;
    }

    pub(super) async fn used_bytes(&self, user: UserId) -> i64 {
        self.scalar_i64(&format!("SELECT used_bytes FROM users WHERE id = '{user}'"))
            .await
    }

    pub(super) async fn set_max_file_size(&self, bytes: i64) {
        self.setting_in(
            "quotas",
            "max_file_size_bytes",
            "integer",
            &bytes.to_string(),
        )
        .await;
    }

    pub(super) async fn item_ids(&self, session: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT id FROM transfer_session_files WHERE transfer_session_id = ?1 ORDER BY ordinal",
        )
        .bind(session)
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn reservation(
        &self,
        session: &str,
    ) -> (String, i64, Option<i64>, Option<String>) {
        sqlx::query_as(
            "SELECT state, reserved_bytes, committed_bytes, release_reason
               FROM quota_reservations WHERE transfer_session_id = ?1",
        )
        .bind(session)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn session_row(
        &self,
        session: &str,
    ) -> (String, Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT state, target_folder_id, deleted_target_folder_id
               FROM transfer_sessions WHERE id = ?1",
        )
        .bind(session)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn item_states(&self, session: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT state FROM transfer_session_files WHERE transfer_session_id = ?1 ORDER BY ordinal",
        )
        .bind(session)
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn set_session_state(&self, session: &str, state: &str) {
        self.execute(&format!(
            "UPDATE transfer_sessions SET state = '{state}' WHERE id = '{session}'"
        ))
        .await;
    }

    pub(super) async fn set_item_state(&self, item: &str, state: &str) {
        self.execute(&format!(
            "UPDATE transfer_session_files SET state = '{state}' WHERE id = '{item}'"
        ))
        .await;
    }

    pub(super) async fn seed_tus(
        &self,
        owner: UserId,
        item: &str,
        length: i64,
        offset: i64,
        state: &str,
    ) {
        self.execute(&format!(
            "INSERT INTO tus_uploads (id, transfer_session_file_id, owner_user_id, upload_length,
                 upload_offset, staging_path, state, created_at, updated_at, expires_at)
             VALUES ('tus-{item}', '{item}', '{owner}', {length}, {offset}, 'uploads/{item}/blob',
                     '{state}', '{SEEDED_AT}', '{SEEDED_AT}', '2026-09-26T12:00:00.000Z')"
        ))
        .await;
    }

    pub(super) async fn seed_multipart(
        &self,
        owner: UserId,
        item: &str,
        parts: &[(i64, i64, &str)],
        state: &str,
    ) {
        let key: String =
            sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
                .bind(item)
                .fetch_one(self.pools.reader().executor())
                .await
                .unwrap();
        let total: i64 = parts.iter().map(|(_, size, _)| size).sum();
        self.execute(&format!(
            "INSERT INTO s3_multipart_uploads (id, transfer_session_file_id, s3_upload_id, bucket,
                 object_key, owner_user_id, total_size_bytes, part_size_bytes, part_count, state,
                 created_at, updated_at, expires_at)
             VALUES ('s3mp-{item}', '{item}', 'upload-id', 'bucket', '{key}', '{owner}', {total},
                     5242880, {}, '{state}', '{SEEDED_AT}', '{SEEDED_AT}', '2026-09-26T12:00:00.000Z')",
            parts.len().max(1)
        ))
        .await;
        for (number, size, part_state) in parts {
            let etag = if matches!(*part_state, "uploaded" | "verified") {
                "'etag'"
            } else {
                "NULL"
            };
            self.execute(&format!(
                "INSERT INTO s3_multipart_parts (s3_multipart_upload_id, part_number, size_bytes, etag, state)
                 VALUES ('s3mp-{item}', {number}, {size}, {etag}, '{part_state}')"
            ))
            .await;
        }
    }

    pub(super) async fn seed_completed_item(
        &self,
        owner: UserId,
        item: &str,
        n: u32,
        size: i64,
    ) -> String {
        let (object_id, key): (String, String) = sqlx::query_as(
            "SELECT final_object_id, final_object_key FROM transfer_session_files WHERE id = ?1",
        )
        .bind(item)
        .fetch_one(self.pools.reader().executor())
        .await
        .unwrap();
        let file_id = self.fresh_id();
        self.execute(&format!(
            "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
             VALUES ('{object_id}', '{key}', 'local', {size}, 'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}');
             INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized, size_bytes, created_at, updated_at)
             VALUES ('{file_id}', '{owner}', NULL, '{object_id}', 'done-{n}.bin', 'done-{n}.bin', {size}, '{SEEDED_AT}', '{SEEDED_AT}');
             UPDATE transfer_session_files
                SET state = 'completed', finalize_stage = 'committed', resulting_file_id = '{file_id}',
                    completed_at = '{SEEDED_AT}'
              WHERE id = '{item}';
             UPDATE transfer_sessions
                SET completed_file_count = completed_file_count + 1,
                    completed_bytes = completed_bytes + {size}
              WHERE id = (SELECT transfer_session_id FROM transfer_session_files WHERE id = '{item}');
             UPDATE users SET used_bytes = used_bytes + {size} WHERE id = '{owner}';
             UPDATE quota_reservations
                SET reserved_bytes = reserved_bytes - MIN(reserved_bytes, {size})
              WHERE transfer_session_id = (SELECT transfer_session_id FROM transfer_session_files WHERE id = '{item}')
                AND state = 'held'"
        ))
        .await;
        file_id
    }

    pub(super) async fn dump(&self) -> Vec<String> {
        let mut dump = Vec::new();
        for sql in [
            "SELECT id || '|' || state || '|' || ifnull(target_folder_id, '') || '|' || completed_bytes || '|' || updated_at FROM transfer_sessions ORDER BY id",
            "SELECT id || '|' || state || '|' || reserved_bytes || '|' || attempts || '|' || finalize_stage || '|' || updated_at FROM transfer_session_files ORDER BY id",
            "SELECT id || '|' || state || '|' || reserved_bytes || '|' || ifnull(committed_bytes, '') || '|' || ifnull(release_reason, '') FROM quota_reservations ORDER BY id",
            "SELECT id || '|' || used_bytes FROM users ORDER BY id",
            "SELECT id || '|' || state FROM tus_uploads ORDER BY id",
            "SELECT id || '|' || state FROM s3_multipart_uploads ORDER BY id",
            "SELECT kind || '|' || state FROM jobs ORDER BY id",
            "SELECT id || '|' || state FROM storage_objects ORDER BY id",
        ] {
            let rows: Vec<String> = sqlx::query_scalar(sql)
                .fetch_all(self.pools.reader().executor())
                .await
                .unwrap();
            dump.push(format!("{sql}\n{}", rows.join("\n")));
        }
        dump
    }
}

pub(super) fn field_codes(fetched: &Fetched) -> Vec<String> {
    fetched.json()["error"]["details"]["fields"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|field| field.as_str().unwrap().to_owned())
        .collect()
}

pub(super) fn s3_stack_storage(profile: ProviderProfile, proxied: bool) -> TransferStorage {
    TransferStorage::new(
        UploadPlanner::s3(profile, Arc::new(move || proxied)),
        Arc::new(|| StorageHealth::Ok),
    )
}

#[tokio::test]
async fn it_transfer_session_preflight_order() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    stack.set_max_file_size(1_000).await;
    stack.set_quota(alice.id, Some(500)).await;

    let many: Vec<Value> = (0..2_001)
        .map(|index| sized(&format!("c{index}"), "a.bin", 1))
        .collect();
    let mut malformed = many.clone();
    malformed[0] = json!("not an object");
    let batch = stack.open(&alice, &request_body(None, &malformed)).await;
    assert_code(&batch, StatusCode::UNPROCESSABLE_ENTITY, "BATCH_TOO_LARGE");
    stack.assert_untouched(0).await;

    let empty = stack.open(&alice, &request_body(None, &[])).await;
    assert_code(&empty, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    assert_eq!(field_codes(&empty), ["files"]);

    let bobs_folder = stack.make(&bob, "Bob", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alices_folder = stack.make(&alice, "Mine", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let folders = stack.counts().await.folders;
    assert_eq!(folders, 2);

    let down_and_foreign = {
        stack
            .storage_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let fetched = stack
            .open(
                &alice,
                &request_in(&bobs_folder, &[sized("c1", "a.bin", 10)]),
            )
            .await;
        stack
            .storage_down
            .store(false, std::sync::atomic::Ordering::SeqCst);
        fetched
    };
    assert_code(
        &down_and_foreign,
        StatusCode::SERVICE_UNAVAILABLE,
        "STORAGE_UNAVAILABLE",
    );
    stack.assert_untouched(folders).await;

    let foreign = stack
        .open(
            &alice,
            &request_in(&bobs_folder, &[sized("c1", "a.bin", 10)]),
        )
        .await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let malformed_target = stack
        .open(
            &alice,
            &request_body(Some("not-a-uuid"), &[sized("c1", "a.bin", 10)]),
        )
        .await;
    assert_code(&malformed_target, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    stack.assert_untouched(folders).await;

    stack
        .execute(&format!(
            "UPDATE folders SET deleting = 1 WHERE id = '{alices_folder}'"
        ))
        .await;
    let deleting = stack
        .open(
            &alice,
            &request_in(&alices_folder, &[sized("c1", "a.bin", 10)]),
        )
        .await;
    assert_code(&deleting, StatusCode::CONFLICT, "FOLDER_DELETING");
    stack
        .execute(&format!(
            "UPDATE folders SET deleting = 0 WHERE id = '{alices_folder}'"
        ))
        .await;
    stack.assert_untouched(folders).await;

    let oversized_and_over_quota = stack
        .open(
            &alice,
            &request_body(None, &[pathed("c1", "Deep/Tree/big.bin", 1_001)]),
        )
        .await;
    assert_code(
        &oversized_and_over_quota,
        StatusCode::PAYLOAD_TOO_LARGE,
        "FILE_TOO_LARGE",
    );
    let details = &oversized_and_over_quota.json()["error"]["details"];
    assert_eq!(details["itemClientId"], "c1");
    assert_eq!(details["declaredBytes"], 1_001);
    assert_eq!(details["maxBytes"], 1_000);
    assert_eq!(details["reason"], "max_file_size");
    stack.assert_untouched(folders).await;

    let duplicates = stack
        .open(
            &alice,
            &request_body(None, &[sized("dup", "a.bin", 1), sized("dup", "b.bin", 1)]),
        )
        .await;
    assert_code(
        &duplicates,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(field_codes(&duplicates), ["clientId"]);
    let bad_name = stack
        .open(&alice, &request_body(None, &[sized("c1", "..", 1)]))
        .await;
    assert_code(&bad_name, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    stack.assert_untouched(folders).await;

    let over_quota = stack
        .open(
            &alice,
            &request_body(
                None,
                &[
                    pathed("c1", "Projects/2026/a.bin", 300),
                    pathed("c2", "Projects/2026/b.bin", 300),
                ],
            ),
        )
        .await;
    assert_code(
        &over_quota,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    let quota_details = &over_quota.json()["error"]["details"];
    assert_eq!(quota_details["requestedBytes"], 600);
    assert_eq!(quota_details["quotaBytes"], 500);
    assert_eq!(quota_details["heldBytes"], 0);
    stack.assert_untouched(folders).await;
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM folders WHERE name = 'Projects'")
            .await,
        0,
        "the folder chain created before the quota check is rolled back"
    );

    let accepted = stack
        .open(
            &alice,
            &request_body(
                None,
                &[
                    pathed("c1", "Projects/2026/a.bin", 300),
                    pathed("c2", "Projects/2026/b.bin", 100),
                ],
            ),
        )
        .await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());
    let counts = stack.counts().await;
    assert_eq!(
        (counts.sessions, counts.items, counts.reservations),
        (1, 2, 1)
    );
    assert_eq!(counts.held, 400);
    assert_eq!(counts.folders, folders + 2);
    assert_eq!(counts.objects, 0);
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM tus_uploads").await
            + stack
                .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
                .await,
        0,
        "no protocol resource is allocated at admission"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_created_response_shape() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let created = stack
        .opened(
            &alice,
            &[
                pathed("c1", "Trip/Day 1/video.mkv", 53_687_091_200),
                unsized_file("c2", "notes.txt"),
                sized("c3", "empty.txt", 0),
            ],
        )
        .await;
    assert_eq!(created["state"], "created");
    assert_eq!(created["provider"], "local");
    assert_eq!(created["expiresAt"], EXPIRES);
    assert_eq!(created["createdAt"], "2026-09-25T12:00:00.000Z");
    assert_eq!(created["reservedBytes"], 53_687_091_200_u64);
    assert_eq!(created["totalBytes"], 53_687_091_200_u64);
    assert_eq!(created["uploadedBytes"], 0);
    let files = created["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0]["clientId"], "c1");
    assert_eq!(files[0]["name"], "video.mkv");
    assert_eq!(files[0]["relativePath"], "Trip/Day 1/video.mkv");
    assert_eq!(files[0]["state"], "created");
    assert_eq!(files[0]["protocol"], "tus");
    assert_eq!(files[0]["sizeBytes"], 53_687_091_200_u64);
    assert_eq!(files[0]["tus"]["createUrl"], "/api/v1/uploads/tus");
    assert_eq!(files[0]["fileId"], Value::Null);
    assert_eq!(files[0]["error"], Value::Null);
    assert_eq!(files[0]["uploadedBytes"], 0);
    assert_eq!(files[1]["sizeBytes"], Value::Null);
    assert_eq!(files[1]["relativePath"], Value::Null);
    assert_eq!(files[2]["protocol"], "tus");
    for file in files {
        assert!(file.get("s3").is_none());
        assert!(file["itemId"].as_str().unwrap().parse::<FolderId>().is_ok());
    }

    let stored: Vec<(String, String, String, Option<i64>, i64, String)> = sqlx::query_as(
        "SELECT state, display_name, relative_path, declared_size_bytes, reserved_bytes, upload_kind
           FROM transfer_session_files ORDER BY ordinal",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        stored,
        [
            (
                "pending".to_owned(),
                "video.mkv".to_owned(),
                "Trip/Day 1".to_owned(),
                Some(53_687_091_200),
                53_687_091_200,
                "tus".to_owned()
            ),
            (
                "pending".to_owned(),
                "notes.txt".to_owned(),
                String::new(),
                None,
                0,
                "tus".to_owned()
            ),
            (
                "pending".to_owned(),
                "empty.txt".to_owned(),
                String::new(),
                Some(0),
                0,
                "tus".to_owned()
            ),
        ]
    );
    let session: (String, i64, i64, String, String) = sqlx::query_as(
        "SELECT state, declared_file_count, declared_bytes, created_at, expires_at FROM transfer_sessions",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        session,
        (
            "created".to_owned(),
            3,
            53_687_091_200,
            "2026-09-25T12:00:00.000Z".to_owned(),
            EXPIRES.to_owned()
        )
    );
    let reservation = stack
        .scalar_i64("SELECT COUNT(*) FROM quota_reservations WHERE state = 'held' AND context = 'my_files' AND expires_at = '2026-10-02T12:00:00.000Z'")
        .await;
    assert_eq!(reservation, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_file_too_large_before_bytes() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_max_file_size(1_000).await;

    for (size, accepted) in [(999, true), (1_000, true), (1_001, false), (0, true)] {
        let fetched = stack
            .open(&alice, &request_body(None, &[sized("c1", "f.bin", size)]))
            .await;
        if accepted {
            assert_eq!(
                fetched.status,
                StatusCode::CREATED,
                "{size}: {}",
                fetched.text()
            );
        } else {
            assert_code(&fetched, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
            let details = &fetched.json()["error"]["details"];
            assert_eq!(details["maxBytes"], 1_000);
            assert_eq!(details["declaredBytes"], 1_001);
            assert!(details.get("providerMaxObjectBytes").is_none());
        }
    }
    assert_eq!(stack.counts().await.sessions, 3);

    let unknown = stack
        .open(&alice, &request_body(None, &[unsized_file("c1", "u.bin")]))
        .await;
    assert_eq!(unknown.status, StatusCode::CREATED, "{}", unknown.text());
    assert_eq!(unknown.json()["reservedBytes"], 1_000);
    let reserved: i64 = sqlx::query_scalar(
        "SELECT reserved_bytes FROM transfer_session_files WHERE client_file_key = 'c1' AND declared_size_bytes IS NULL",
    )
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(reserved, 1_000);

    let counts = stack.counts().await;
    assert_eq!(counts.objects, 0);
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM tus_uploads").await,
        0
    );
    stack.stop().await;

    for (profile, proxied, capacity, reason) in [
        (ProviderProfile::Minio, false, 5 * TIB, "object_size"),
        (
            ProviderProfile::R2,
            true,
            8 * MIB * 9_900,
            "proxy_part_count",
        ),
    ] {
        let root = TempDir::new().unwrap();
        let storage = s3_stack_storage(profile, proxied);
        let stack = Stack::start_with_storage(root.path(), &TestClock::new(START), storage).await;
        let alice = stack.member("alice", HOST_A).await;
        stack.set_max_file_size(10 * TIB as i64).await;

        for (size, accepted) in [
            (capacity - 1, true),
            (capacity, true),
            (capacity + 1, false),
            (0, true),
        ] {
            let fetched = stack
                .open(&alice, &request_body(None, &[sized("c1", "f.bin", size)]))
                .await;
            if accepted {
                assert_eq!(
                    fetched.status,
                    StatusCode::CREATED,
                    "{reason} {size}: {}",
                    fetched.text()
                );
                let protocol = &fetched.json()["files"][0]["protocol"];
                assert_eq!(
                    *protocol,
                    if size == 0 {
                        "s3-single"
                    } else {
                        "s3-multipart"
                    }
                );
            } else {
                assert_code(&fetched, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
                let details = &fetched.json()["error"]["details"];
                assert_eq!(details["reason"], reason);
                assert_eq!(details["providerMaxObjectBytes"], capacity);
                assert_eq!(details["declaredBytes"], capacity + 1);
                assert_eq!(details["itemClientId"], "c1");
            }
        }
        assert_eq!(stack.counts().await.sessions, 3);
        assert_eq!(stack.counts().await.objects, 0);
        assert_eq!(
            stack
                .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
                .await,
            0
        );
        assert_eq!(
            stack.scalar_i64("SELECT COUNT(*) FROM tus_uploads").await,
            0
        );
        assert_eq!(
            stack
                .scalar_i64("SELECT COUNT(*) FROM s3_multipart_parts")
                .await,
            0
        );
        stack.stop().await;
    }
}

#[tokio::test]
async fn it_transfer_session_s3_part_plan_is_checked_before_bytes() {
    let root = TempDir::new().unwrap();
    let storage = s3_stack_storage(ProviderProfile::Minio, false);
    let stack = Stack::start_with_storage(root.path(), &TestClock::new(START), storage).await;
    let alice = stack.member("alice", HOST_A).await;

    let accepted = stack
        .opened(
            &alice,
            &[
                sized("zero", "zero.bin", 0),
                sized("small", "small.bin", 5 * MIB - 1),
                sized("boundary", "boundary.bin", 52_428_800_000),
                sized("top", "top.bin", 5 * TIB),
                unsized_file("stream", "stream.bin"),
            ],
        )
        .await;
    assert_eq!(accepted["provider"], "s3");
    let files = accepted["files"].as_array().unwrap();
    assert_eq!(files[0]["protocol"], "s3-single");
    assert!(files[0].get("s3").is_none());
    assert!(files[0].get("tus").is_none());
    assert_eq!(files[1]["protocol"], "s3-multipart");
    assert_eq!(files[1]["s3"]["partSizeBytes"], 8 * MIB);
    assert_eq!(files[1]["s3"]["partCount"], 1);
    assert_eq!(files[1]["s3"]["maxPresignBatch"], 16);
    assert_eq!(files[1]["s3"]["presignTtlSeconds"], 900);
    assert_eq!(files[2]["s3"]["partSizeBytes"], 8 * MIB);
    assert_eq!(files[2]["s3"]["partCount"], 6_250);
    assert_eq!(files[3]["s3"]["partSizeBytes"], GIB);
    assert_eq!(files[3]["s3"]["partCount"], 5_120);
    assert_eq!(files[4]["protocol"], "s3-multipart");
    assert_eq!(files[4]["s3"]["partSizeBytes"], Value::Null);
    assert_eq!(files[4]["s3"]["partCount"], Value::Null);
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT upload_kind FROM transfer_session_files ORDER BY ordinal")
            .fetch_all(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        kinds,
        [
            "s3_single",
            "s3_multipart",
            "s3_multipart",
            "s3_multipart",
            "s3_multipart"
        ]
    );
    let before = stack.counts().await;

    let rejected = stack
        .open(
            &alice,
            &request_body(
                None,
                &[
                    sized("fine", "fine.bin", 1),
                    sized("huge", "huge.bin", 5 * TIB + 1),
                ],
            ),
        )
        .await;
    assert_code(&rejected, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    let details = &rejected.json()["error"]["details"];
    assert_eq!(details["itemClientId"], "huge");
    assert_eq!(details["declaredBytes"], 5 * TIB + 1);
    assert_eq!(details["providerMaxObjectBytes"], 5 * TIB);
    assert_eq!(details["reason"], "object_size");
    assert!(details.get("maxBytes").is_none());

    let after = stack.counts().await;
    assert_eq!(
        (after.sessions, after.items, after.reservations, after.held),
        (
            before.sessions,
            before.items,
            before.reservations,
            before.held
        )
    );
    assert_eq!(after.objects, 0);
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM s3_multipart_uploads")
            .await,
        0
    );

    let both = stack
        .open(
            &alice,
            &request_body(None, &[sized("c1", "a.bin", 5 * TIB + 1)]),
        )
        .await;
    assert_code(&both, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_proxied_profile_uses_the_degraded_ceiling() {
    let root = TempDir::new().unwrap();
    let storage = s3_stack_storage(ProviderProfile::R2, true);
    let stack = Stack::start_with_storage(root.path(), &TestClock::new(START), storage).await;
    let alice = stack.member("alice", HOST_A).await;
    let ceiling = 8 * MIB * 9_900;

    let accepted = stack
        .opened(
            &alice,
            &[
                sized("c1", "ok.bin", ceiling),
                sized("c2", "small.bin", 100 * MIB),
            ],
        )
        .await;
    assert_eq!(accepted["files"][1]["s3"]["partSizeBytes"], 8 * MIB);
    assert_eq!(accepted["files"][1]["s3"]["partCount"], 13);

    let rejected = stack
        .open(
            &alice,
            &request_body(None, &[sized("c3", "big.bin", ceiling + 1)]),
        )
        .await;
    assert_code(&rejected, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    let details = &rejected.json()["error"]["details"];
    assert_eq!(details["reason"], "proxy_part_count");
    assert_eq!(details["providerMaxObjectBytes"], ceiling);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_allocates_final_identity_without_object_row() {
    let world = super::deletion_tests::World::start().await;
    let stack = &world.stack;
    let alice = stack.member("alice", HOST_A).await;
    let files: Vec<Value> = (0..6)
        .map(|index| {
            pathed(
                &format!("c{index}"),
                &format!("Set/Sub{}/f{index}.bin", index % 2),
                10 + index,
            )
        })
        .collect();
    let body = request_body(None, &files);

    let first = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text());
    let identities = || async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT final_object_id, final_object_key FROM transfer_session_files ORDER BY ordinal",
        )
        .fetch_all(stack.pools.reader().executor())
        .await
        .unwrap()
    };
    let recorded = identities().await;
    assert_eq!(recorded.len(), 6);
    let ids: HashSet<&String> = recorded.iter().map(|(id, _)| id).collect();
    let keys: HashSet<&String> = recorded.iter().map(|(_, key)| key).collect();
    assert_eq!((ids.len(), keys.len()), (6, 6));
    for (id, key) in &recorded {
        assert_eq!(id.len(), 36);
        assert!(id.parse::<FolderId>().is_ok());
        let parsed = ObjectKey::parse(key).expect("the key follows the objects grammar");
        assert_eq!(parsed.as_str(), key);
        assert_eq!(key.len(), 46);
        assert!(
            world.provider.exists(&parsed).is_ok_and(|exists| !exists),
            "no blob exists at {key}"
        );
    }
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await,
        0
    );

    let text = first.text();
    for (id, key) in &recorded {
        assert!(!text.contains(id.as_str()), "the response leaks {id}");
        assert!(!text.contains(key.as_str()), "the response leaks {key}");
    }
    for forbidden in [
        "objects/",
        "finalObject",
        "objectKey",
        "storageObject",
        "bucket",
        "uploadId",
        "stagingPath",
    ] {
        assert!(!text.contains(forbidden), "the response names {forbidden}");
    }

    let replay = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.body, first.body);
    assert_eq!(
        identities().await,
        recorded,
        "the identities are unchanged on replay"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM transfer_session_files")
            .await,
        6
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await,
        0
    );

    let detail = stack
        .session_detail(&alice, first.json()["id"].as_str().unwrap())
        .await;
    assert_eq!(detail.status, StatusCode::OK);
    let detail_text = detail.text();
    for (id, key) in &recorded {
        assert!(!detail_text.contains(id.as_str()));
        assert!(!detail_text.contains(key.as_str()));
    }
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-034")]
#[tokio::test]
async fn regression_R034_cross_owner_upload_session_injection() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    stack.set_quota(alice.id, Some(10_000)).await;
    stack.set_quota(bob.id, Some(10_000)).await;

    let alices = stack
        .opened(
            &alice,
            &[sized("a1", "a1.bin", 100), sized("a2", "a2.bin", 200)],
        )
        .await;
    let bobs = stack
        .opened(
            &bob,
            &[sized("b1", "b1.bin", 300), sized("b2", "b2.bin", 400)],
        )
        .await;
    let alice_session = alices["id"].as_str().unwrap();
    let alice_item = alices["files"][0]["itemId"].as_str().unwrap();
    let bob_session = bobs["id"].as_str().unwrap();
    let bob_item = bobs["files"][0]["itemId"].as_str().unwrap();
    stack.set_item_state(bob_item, "uploading").await;
    stack.set_session_state(bob_session, "uploading").await;
    stack
        .seed_tus(bob.id, bob_item, 300, 120, "in_progress")
        .await;

    let before = stack.dump().await;

    let read = stack.session_detail(&alice, bob_session).await;
    assert_code(&read, StatusCode::NOT_FOUND, "TRANSFER_SESSION_NOT_FOUND");
    let cancel = stack.cancel_session(&alice, bob_session).await;
    assert_code(&cancel, StatusCode::NOT_FOUND, "TRANSFER_SESSION_NOT_FOUND");
    let complete = stack.complete_session(&alice, bob_session).await;
    assert_code(
        &complete,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let retry = stack.retry_item(&alice, bob_session, bob_item).await;
    assert_code(&retry, StatusCode::NOT_FOUND, "TRANSFER_SESSION_NOT_FOUND");
    let cancel_item = stack.cancel_item(&alice, bob_session, bob_item).await;
    assert_code(
        &cancel_item,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );

    let crossed_retry = stack.retry_item(&alice, alice_session, bob_item).await;
    assert_code(
        &crossed_retry,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let crossed_cancel = stack.cancel_item(&alice, alice_session, bob_item).await;
    assert_code(
        &crossed_cancel,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let crossed_by_bob = stack.cancel_item(&bob, bob_session, alice_item).await;
    assert_code(
        &crossed_by_bob,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );

    for (method, path) in [
        (Method::GET, format!("{SESSIONS}/not-a-uuid")),
        (Method::DELETE, format!("{SESSIONS}/not-a-uuid")),
        (
            Method::POST,
            format!("{SESSIONS}/{bob_session}/files/not-a-uuid/retry"),
        ),
        (Method::DELETE, format!("{SESSIONS}/{}", stack.fresh_id())),
    ] {
        let fetched = stack.api(method, &path, &alice, None).await;
        assert_code(
            &fetched,
            StatusCode::NOT_FOUND,
            "TRANSFER_SESSION_NOT_FOUND",
        );
    }

    let listing = stack.read(SESSIONS, &alice).await;
    let listed: Vec<String> = listing.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(listed, [alice_session]);

    assert_eq!(
        stack.dump().await,
        before,
        "Bob's rows, quota and jobs are untouched"
    );
    assert_eq!(
        stack.reservation(bob_session).await,
        ("held".to_owned(), 700, None, None)
    );
    stack.stop().await;
}

use futures_util::future::join_all;
use unicode_normalization::UnicodeNormalization;

use super::folders::{HOST_A, HOST_B};
use super::profile::assert_code;
use super::transfers::{
    field_codes, pathed, request_body, request_in, s3_stack_storage, sized, unsized_file, KEY_A,
};
use super::*;
use crate::storage::s3::profile::ProviderProfile;

fn bulk_files(count: usize) -> Vec<Value> {
    let padding = "n".repeat(60);
    (0..count)
        .map(|index| {
            pathed(
                &format!("file-{index:05}"),
                &format!(
                    "Batch{}/Sub{}/document-{index:05}-{padding}.bin",
                    index % 20,
                    index % 7
                ),
                1_000 + u64::try_from(index).unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn it_transfer_session_keyed_two_thousand_files_replays_byte_identical() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    let body = request_body(None, &bulk_files(2_000));
    assert!(body.to_string().len() < 2 * 1024 * 1024);

    let original = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(original.status, StatusCode::CREATED, "{}", original.text());
    assert!(
        original.body.len() > 500_000,
        "the response is far above the 16 KiB envelope of every other route"
    );
    assert_eq!(original.json()["files"].as_array().unwrap().len(), 2_000);
    let session = original.json()["id"].as_str().unwrap().to_owned();
    let counts = stack.counts().await;
    assert_eq!(
        (counts.sessions, counts.items, counts.reservations),
        (1, 2_000, 1)
    );
    assert_eq!(counts.folders, 160);
    let stored: i64 = stack
        .scalar_i64(
            "SELECT length(CAST(response_json AS BLOB)) FROM idempotency_records WHERE state = 'completed'",
        )
        .await;
    println!(
        "2000 plain files: request {} bytes, response {} bytes, stored envelope {stored} bytes",
        body.to_string().len(),
        original.body.len()
    );
    assert!(stored > 500_000 && stored <= 6 * 1024 * 1024, "{stored}");

    let dump = stack.dump().await;
    let replay = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.body, original.body, "byte-identical replay");
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    assert_eq!(stack.dump().await, dump);
    let again = stack.counts().await;
    assert_eq!(
        (
            again.sessions,
            again.items,
            again.reservations,
            again.held,
            again.folders
        ),
        (1, 2_000, 1, counts.held, 160)
    );

    let items = stack.item_ids(&session).await;
    stack.set_item_state(&items[0], "uploading").await;
    stack
        .seed_tus(alice.id, &items[0], 1_000, 400, "in_progress")
        .await;
    stack.set_session_state(&session, "uploading").await;
    assert_eq!(
        stack.cancel_session(&alice, &session).await.status,
        StatusCode::NO_CONTENT
    );
    let detail = stack.session_detail(&alice, &session).await.json();
    assert_eq!(detail["state"], "canceled");
    let after_change = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(after_change.status, StatusCode::CREATED);
    assert_eq!(
        after_change.body, original.body,
        "the replay is the original response even after the session changed"
    );
    assert_eq!(after_change.json()["state"], "created");
    assert_eq!(stack.counts().await.sessions, 1);

    let mut changed = bulk_files(2_000);
    changed[1_999] = sized("file-01999", "document-01999.bin", 1);
    for variant in [
        request_body(None, &changed),
        request_body(None, &bulk_files(1_999)),
    ] {
        let conflict = stack.open_keyed(&alice, &variant, KEY_A).await;
        assert_code(&conflict, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");
    }
    assert_eq!(stack.counts().await.sessions, 1);

    stack.stop().await;
    let restarted = Stack::start(root.path(), &clock).await;
    let after_restart = restarted.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(after_restart.status, StatusCode::CREATED);
    assert_eq!(after_restart.body, original.body);
    assert_eq!(
        after_restart.headers.get("idempotency-replayed").unwrap(),
        "true"
    );
    let persisted = restarted.counts().await;
    assert_eq!(
        (persisted.sessions, persisted.items, persisted.reservations),
        (1, 2_000, 1)
    );
    assert_eq!(persisted.folders, 160);
    restarted.stop().await;
}

#[tokio::test]
async fn it_transfer_session_unicode_expansion_response_stays_inside_the_envelope_bound() {
    let expanding = "\u{1D15E}".repeat(31);
    assert_eq!(expanding.nfc().count(), 62);
    let segment = || expanding.clone();
    let root = TempDir::new().unwrap();
    let storage = s3_stack_storage(ProviderProfile::Minio, false);
    let stack = Stack::start_with_storage(root.path(), &TestClock::new(START), storage).await;
    let alice = stack.member("alice", HOST_A).await;

    let files: Vec<Value> = (0..2_000)
        .map(|index| {
            let path = format!("{}/{}/{}/{}", segment(), segment(), segment(), segment());
            json!({
                "clientId": format!("{index:05}{}", "c".repeat(123)),
                "name": segment(),
                "sizeBytes": 4_000_000_000_000_u64,
                "relativePath": path,
            })
        })
        .collect();
    let body = request_body(None, &files);
    let request_bytes = body.to_string().len();
    assert!(request_bytes < 2 * 1024 * 1024, "{request_bytes}");

    let created = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(
        created.status,
        StatusCode::CREATED,
        "{}",
        &created.text()[..200.min(created.body.len())]
    );
    let first = &created.json()["files"][0];
    assert_eq!(first["name"].as_str().unwrap().len(), 248);
    assert_eq!(first["relativePath"].as_str().unwrap().len(), 995);
    let stored = stack
        .scalar_i64(
            "SELECT length(CAST(response_json AS BLOB)) FROM idempotency_records WHERE state = 'completed'",
        )
        .await;
    println!("unicode expansion: request {request_bytes} bytes, stored envelope {stored} bytes");
    assert!(
        stored > 3_000_000 && stored <= 6 * 1024 * 1024,
        "stored envelope {stored} bytes for a {request_bytes} byte request"
    );
    let replay = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(replay.body, created.body);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_completion_failure_commits_no_effect() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(1_000)).await;
    let folders = stack.counts().await.folders;
    stack
        .execute(
            "CREATE TRIGGER inject_completion BEFORE UPDATE ON idempotency_records
             BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END",
        )
        .await;

    let body = request_body(None, &[pathed("c1", "New/Tree/a.bin", 100)]);
    let failed = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    stack.assert_untouched(folders).await;
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM idempotency_records")
            .await,
        0,
        "the claim is cleared, never left completed without its effect"
    );

    stack.execute("DROP TRIGGER inject_completion").await;
    let recovered = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(
        recovered.status,
        StatusCode::CREATED,
        "{}",
        recovered.text()
    );
    assert!(recovered.headers.get("idempotency-replayed").is_none());
    assert_eq!(stack.counts().await.sessions, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_injected_failure_rolls_back_folders_items_and_hold() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(10_000)).await;
    let folders = stack.counts().await.folders;
    stack
        .execute(
            "CREATE TRIGGER inject_item_failure BEFORE INSERT ON transfer_session_files
             WHEN NEW.display_name = 'boom.bin'
             BEGIN SELECT RAISE(ABORT, 'injected item failure'); END",
        )
        .await;

    let failing = request_body(
        None,
        &[
            pathed("c1", "A/B/C/ok.bin", 10),
            pathed("c2", "A/D/ok2.bin", 10),
            pathed("c3", "A/B/boom.bin", 10),
        ],
    );
    let failed = stack.open_keyed(&alice, &failing, KEY_A).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failed.error_code(), "INTERNAL_ERROR");
    assert!(
        !failed.text().contains("injected"),
        "no internal detail leaks"
    );
    stack.assert_untouched(folders).await;

    stack.execute("DROP TRIGGER inject_item_failure").await;
    let retried = stack.open_keyed(&alice, &failing, KEY_A).await;
    assert_eq!(retried.status, StatusCode::CREATED, "{}", retried.text());
    let counts = stack.counts().await;
    assert_eq!(
        (counts.sessions, counts.items, counts.reservations),
        (1, 3, 1)
    );
    assert_eq!(counts.folders, folders + 4, "A, A/B, A/B/C, A/D");
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_directory_chains_merge_deterministically() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let existing = stack.make(&alice, "Photos", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let created = stack
        .open(
            &alice,
            &request_body(
                None,
                &[
                    pathed("c1", "photos/2026/a.jpg", 1),
                    pathed("c2", "PHOTOS/2026/b.jpg", 1),
                    pathed("c3", "Photos/2027/c.jpg", 1),
                    pathed("c4", "cafe\u{301}/d.jpg", 1),
                    pathed("c5", "caf\u{e9}/e.jpg", 1),
                    sized("c6", "root.jpg", 1),
                ],
            ),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let names: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT name, parent_id FROM folders ORDER BY depth, name")
            .fetch_all(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        names,
        [
            ("Photos".to_owned(), None),
            ("caf\u{e9}".to_owned(), None),
            ("2026".to_owned(), Some(existing.clone())),
            ("2027".to_owned(), Some(existing.clone())),
        ],
        "the first spelling wins and nothing is suffixed"
    );
    let files = created.json();
    assert_eq!(files["files"][3]["relativePath"], "caf\u{e9}/d.jpg");
    assert_eq!(files["files"][4]["relativePath"], "caf\u{e9}/e.jpg");
    assert_eq!(files["files"][1]["relativePath"], "PHOTOS/2026/b.jpg");

    let repeat = stack
        .open(
            &alice,
            &request_body(None, &[pathed("c1", "Photos/2026/a.jpg", 1)]),
        )
        .await;
    assert_eq!(repeat.status, StatusCode::CREATED);
    assert_eq!(
        stack.scalar_i64("SELECT COUNT(*) FROM folders").await,
        4,
        "a second session reuses the existing chain"
    );

    let nested = stack.make(&alice, "Nested", Some(&existing)).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let in_nested = stack
        .open(
            &alice,
            &request_in(&nested, &[pathed("n1", "Deep/Er/f.bin", 1)]),
        )
        .await;
    assert_eq!(
        in_nested.status,
        StatusCode::CREATED,
        "{}",
        in_nested.text()
    );
    let deep: (i64, Option<String>) =
        sqlx::query_as("SELECT depth, parent_id FROM folders WHERE name = 'Er'")
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(deep.0, 3, "Photos(0) / Nested(1) / Deep(2) / Er(3)");
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_depth_counts_the_existing_target_ancestry() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let mut parent: Option<String> = None;
    for depth in 0..=62_u8 {
        parent = Some(
            stack
                .seed_folder(alice.id, parent.as_deref(), &format!("d{depth}"), depth)
                .await,
        );
    }
    let deepest = parent.unwrap();
    let folders = stack.counts().await.folders;

    let fits = stack
        .open(&alice, &request_in(&deepest, &[pathed("c1", "x/f.bin", 1)]))
        .await;
    assert_eq!(fits.status, StatusCode::CREATED, "{}", fits.text());
    assert_eq!(stack.counts().await.folders, folders + 1);

    let too_deep = stack
        .open(
            &alice,
            &request_in(
                &deepest,
                &[pathed("c1", "y/z/f.bin", 1), pathed("c2", "q/w/e/f.bin", 1)],
            ),
        )
        .await;
    assert_code(
        &too_deep,
        StatusCode::UNPROCESSABLE_ENTITY,
        "FOLDER_DEPTH_EXCEEDED",
    );
    assert_eq!(
        stack.counts().await.folders,
        folders + 1,
        "nothing was truncated or created"
    );
    assert_eq!(stack.counts().await.sessions, 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_accepts_one_and_two_thousand_files_and_refuses_the_next() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let single = stack
        .open(&alice, &request_body(None, &[sized("only", "only.bin", 1)]))
        .await;
    assert_eq!(single.status, StatusCode::CREATED);
    assert_eq!(single.json()["files"].as_array().unwrap().len(), 1);

    let flat = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| sized(&format!("c{index}"), &format!("f{index}.bin"), 1))
            .collect()
    };
    let full = stack.open(&alice, &request_body(None, &flat(2_000))).await;
    assert_eq!(full.status, StatusCode::CREATED, "{}", &full.text()[..200]);
    assert_eq!(full.json()["files"].as_array().unwrap().len(), 2_000);
    assert_eq!(full.json()["reservedBytes"], 2_000);

    let before = stack.counts().await;
    let over = stack.open(&alice, &request_body(None, &flat(2_001))).await;
    assert_code(&over, StatusCode::UNPROCESSABLE_ENTITY, "BATCH_TOO_LARGE");
    assert_eq!(over.json()["error"]["details"]["maxItems"], 2_000);
    let after = stack.counts().await;
    assert_eq!(
        (after.sessions, after.items, after.reservations),
        (before.sessions, before.items, before.reservations)
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_quota_cases() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let admin = stack.member("admin", HOST_B).await;
    stack
        .execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{}'",
            admin.id
        ))
        .await;

    stack.set_quota(admin.id, Some(100)).await;
    let admin_over = stack
        .open(&admin, &request_body(None, &[sized("c", "a.bin", 200)]))
        .await;
    assert_code(
        &admin_over,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    assert!(!crate::domain::error_code::ErrorCode::QuotaExceeded.retryable());
    assert_eq!(
        crate::domain::error_code::ErrorCode::QuotaExceeded
            .status()
            .as_u16(),
        507
    );
    stack.set_quota(admin.id, None).await;
    let admin_unlimited = stack
        .open(
            &admin,
            &request_body(None, &[sized("c", "a.bin", 5_000_000_000)]),
        )
        .await;
    assert_eq!(admin_unlimited.status, StatusCode::CREATED);

    stack.set_quota(alice.id, Some(0)).await;
    let zero_byte = stack
        .open(&alice, &request_body(None, &[sized("c", "empty.bin", 0)]))
        .await;
    assert_eq!(
        zero_byte.status,
        StatusCode::CREATED,
        "{}",
        zero_byte.text()
    );
    let one_byte = stack
        .open(&alice, &request_body(None, &[sized("c", "one.bin", 1)]))
        .await;
    assert_code(
        &one_byte,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    assert_eq!(one_byte.json()["error"]["details"]["quotaBytes"], 0);

    stack.set_quota(alice.id, Some(100)).await;
    let first = stack
        .open(&alice, &request_body(None, &[sized("c", "a.bin", 60)]))
        .await;
    assert_eq!(first.status, StatusCode::CREATED);
    let held = stack
        .open(&alice, &request_body(None, &[sized("c", "b.bin", 50)]))
        .await;
    assert_code(&held, StatusCode::INSUFFICIENT_STORAGE, "QUOTA_EXCEEDED");
    let details = &held.json()["error"]["details"];
    assert_eq!(details["heldBytes"], 60);
    assert_eq!(details["usedBytes"], 0);
    assert_eq!(details["requestedBytes"], 50);
    let exact = stack
        .open(&alice, &request_body(None, &[sized("c", "c.bin", 40)]))
        .await;
    assert_eq!(
        exact.status,
        StatusCode::CREATED,
        "used + held + requested == quota is admitted"
    );

    stack.set_quota(alice.id, None).await;
    stack
        .setting_in("quotas", "default_user_quota_bytes", "integer", "10")
        .await;
    stack
        .execute(&format!(
            "UPDATE users SET quota_override_mode = 'inherit', quota_bytes = NULL WHERE id = '{}'",
            alice.id
        ))
        .await;
    let inherited = stack
        .open(&alice, &request_body(None, &[sized("c", "d.bin", 11)]))
        .await;
    assert_code(
        &inherited,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    assert_eq!(inherited.json()["error"]["details"]["quotaBytes"], 10);

    let overflow: Vec<Value> = (0..1_100)
        .map(|index| {
            sized(
                &format!("c{index}"),
                &format!("o{index}.bin"),
                9_007_199_254_740_991,
            )
        })
        .collect();
    stack.set_quota(alice.id, None).await;
    let before = stack.counts().await;
    let wrapped = stack.open(&alice, &request_body(None, &overflow)).await;
    assert_code(
        &wrapped,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(field_codes(&wrapped), ["files"]);
    let after = stack.counts().await;
    assert_eq!(
        (after.sessions, after.items, after.held),
        (before.sessions, before.items, before.held)
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_inactive_owner_cannot_open_a_session() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack
        .execute(&format!(
            "UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z' WHERE id = '{}'",
            alice.id
        ))
        .await;

    let refused = stack
        .open(&alice, &request_body(None, &[sized("c", "a.bin", 1)]))
        .await;
    assert!(
        matches!(
            refused.status,
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
        ),
        "{}",
        refused.text()
    );
    stack.assert_untouched(0).await;
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_concurrent_creates_never_exceed_quota() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(100)).await;

    let bodies: Vec<Value> = (0..10)
        .map(|index| {
            request_body(
                None,
                &[
                    sized(&format!("a{index}"), &format!("a{index}.bin"), 10),
                    sized(&format!("b{index}"), &format!("b{index}.bin"), 20),
                ],
            )
        })
        .collect();
    let outcomes = join_all(bodies.iter().map(|body| stack.open(&alice, body))).await;
    let created = outcomes
        .iter()
        .filter(|fetched| fetched.status == StatusCode::CREATED)
        .count();
    let refused = outcomes
        .iter()
        .filter(|fetched| fetched.status == StatusCode::INSUFFICIENT_STORAGE)
        .count();
    assert_eq!(created, 3, "100 / 30 admits exactly three");
    assert_eq!(
        created + refused,
        10,
        "{:?}",
        outcomes.iter().map(|o| o.status).collect::<Vec<_>>()
    );
    let counts = stack.counts().await;
    assert_eq!(
        (counts.sessions, counts.items, counts.reservations),
        (3, 6, 3)
    );
    assert_eq!(counts.held, 90);
    assert_eq!(
        stack
            .scalar_i64(
                "SELECT COUNT(*) FROM transfer_sessions s
                  WHERE NOT EXISTS (SELECT 1 FROM quota_reservations q WHERE q.transfer_session_id = s.id)",
            )
            .await,
        0,
        "no session exists without its reservation"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_error_precedence_is_fixed() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    stack.set_quota(alice.id, Some(10)).await;
    stack.set_max_file_size(5).await;
    let bobs = stack.make(&bob, "Bobs", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let folders = stack.counts().await.folders;

    let cases: [(Value, StatusCode, &str); 6] = [
        (
            request_in(&bobs, &[sized("c1", "a.bin", 50)]),
            StatusCode::NOT_FOUND,
            "FOLDER_NOT_FOUND",
        ),
        (
            request_body(None, &[sized("c1", "a.bin", 50), sized("c1", "b.bin", 1)]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            request_body(None, &[sized("c1", "..", 50)]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "NAME_INVALID",
        ),
        (
            request_body(None, &[sized("c1", "a.bin", 50)]),
            StatusCode::PAYLOAD_TOO_LARGE,
            "FILE_TOO_LARGE",
        ),
        (
            request_body(
                None,
                &[
                    json!({ "clientId": "c1", "name": "f.bin", "sizeBytes": 1, "relativePath": "../x/f.bin" }),
                ],
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
        (
            request_body(
                None,
                &[
                    json!({ "clientId": "c1", "name": "other.bin", "sizeBytes": 1, "relativePath": "dir/f.bin" }),
                ],
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        ),
    ];
    for (body, status, code) in cases {
        let fetched = stack.open(&alice, &body).await;
        assert_code(&fetched, status, code);
        stack.assert_untouched(folders).await;
    }

    let unknown_and_over_quota = stack
        .open(
            &alice,
            &request_body(
                None,
                &[
                    unsized_file("c1", "a.bin"),
                    unsized_file("c2", "b.bin"),
                    unsized_file("c3", "c.bin"),
                ],
            ),
        )
        .await;
    assert_code(
        &unknown_and_over_quota,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    assert_eq!(
        unknown_and_over_quota.json()["error"]["details"]["requestedBytes"],
        15
    );
    stack.assert_untouched(folders).await;
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_client_ids_are_opaque_and_safe_in_error_details() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_max_file_size(100).await;

    let ids = [
        "name|1048576|1790000000000",
        "e2UmP+/9a==",
        "has space/and\\slash",
        "\u{e9}\u{1F600}\u{2028}<b>\"q\"</b>",
        &"\u{1F600}".repeat(128),
    ];
    let files: Vec<Value> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| json!({ "clientId": id, "name": format!("f{index}.bin"), "sizeBytes": 1 }))
        .collect();
    let created = stack.opened(&alice, &files).await;
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(created["files"][index]["clientId"], *id);
    }

    for id in ids {
        let refused = stack
            .open(
                &alice,
                &request_body(
                    None,
                    &[json!({ "clientId": id, "name": "big.bin", "sizeBytes": 101 })],
                ),
            )
            .await;
        assert_code(&refused, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
        assert_eq!(refused.json()["error"]["details"]["itemClientId"], id);
    }

    for bad in [
        json!("".to_owned()),
        json!("x".repeat(129)),
        json!("\u{1F600}".repeat(129)),
        json!("tab\there"),
        json!("nul\u{0}"),
        json!("bell\u{7}"),
    ] {
        let rejected = stack
            .open(
                &alice,
                &request_body(
                    None,
                    &[json!({ "clientId": bad, "name": "a.bin", "sizeBytes": 1 })],
                ),
            )
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(field_codes(&rejected), ["clientId"]);
    }
    stack.stop().await;
}

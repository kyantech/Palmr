use std::sync::atomic::Ordering;

use super::folders::{HOST_A, HOST_B};
use super::profile::assert_code;
use super::transfers::{pathed, request_body, sized};
use super::tus::{
    assert_tus_error, header, metadata, small_limits, upload_id_of, Planned, TestStaging, TusReq,
    TUS,
};
use super::*;
use futures_util::future::join_all;

const UNKNOWN_ID: &str = "0192f3a7-2a01-7c4d-8e11-aa0192f3a799";

async fn assert_nothing_created(stack: &Stack, item: &str) {
    assert_eq!(stack.tus_count().await, 0);
    assert!(
        stack.staging_dirs().is_empty(),
        "{:?}",
        stack.staging_dirs()
    );
    assert_eq!(stack.item_state(item).await, "pending");
    assert_eq!(stack.counts().await.objects, 0);
}

#[tokio::test]
async fn it_tus_options_capabilities() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let options = stack
        .tus_send(TusReq::new(Method::OPTIONS, TUS, &alice).no_version())
        .await;
    assert_eq!(options.status, StatusCode::NO_CONTENT);
    assert!(options.body.is_empty());
    assert_eq!(header(&options, "tus-resumable"), "1.0.0");
    assert_eq!(header(&options, "tus-version"), "1.0.0");
    assert_eq!(
        header(&options, "tus-extension"),
        "creation,creation-with-upload,expiration,termination,checksum"
    );
    assert!(!header(&options, "tus-extension").contains("concatenation"));
    assert_eq!(header(&options, "tus-checksum-algorithm"), "sha256");
    assert!(options.headers.contains_key("x-request-id"));
    assert_eq!(header(&options, "cache-control"), "no-store");
    assert!(
        !options.headers.contains_key("tus-max-size"),
        "an unlimited instance advertises no maximum"
    );
    assert_eq!(options.headers.get_all("tus-extension").iter().count(), 1);

    let with_version = stack
        .tus_send(TusReq::new(Method::OPTIONS, TUS, &alice))
        .await;
    assert_eq!(with_version.status, StatusCode::NO_CONTENT);

    stack.set_max_file_size(5_000).await;
    let finite = stack
        .tus_send(TusReq::new(Method::OPTIONS, TUS, &alice).no_version())
        .await;
    assert_eq!(header(&finite, "tus-max-size"), "5000");

    let anonymous = stack
        .tus_send(TusReq::anonymous(Method::OPTIONS, TUS).no_version())
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn it_tus_concat_501() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(100)).await;
    let before = stack.counts().await;
    let held_before = stack.reservation(&planned.session).await;

    for concat in [
        "partial",
        "final;/api/v1/uploads/tus/a /api/v1/uploads/tus/b",
        "",
    ] {
        let response = stack
            .tus_send(
                stack
                    .create_req(&alice, &planned, Some(100))
                    .header("upload-concat", concat),
            )
            .await;
        assert_tus_error(
            &response,
            StatusCode::NOT_IMPLEMENTED,
            "TUS_EXTENSION_UNSUPPORTED",
        );
    }
    let without_length = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-concat", "partial")
                .header(
                    "upload-metadata",
                    &metadata(&planned.session, &planned.item, "a.bin", &[]),
                ),
        )
        .await;
    assert_tus_error(
        &without_length,
        StatusCode::NOT_IMPLEMENTED,
        "TUS_EXTENSION_UNSUPPORTED",
    );

    assert_nothing_created(&stack, &planned.item).await;
    let after = stack.counts().await;
    assert_eq!(after.reservations, before.reservations);
    assert_eq!(after.held, before.held);
    assert_eq!(stack.reservation(&planned.session).await, held_before);
    assert_eq!(stack.session_row(&planned.session).await.0, "created");
}

#[tokio::test]
async fn it_tus_create_sets_up_the_resource_for_a_planned_item() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    for (name, planned_size, length) in [
        ("known.bin", Some(100), Some(100)),
        ("zero.bin", Some(0), Some(0)),
        ("deferred.bin", None, None),
        ("known-when-unsized.bin", None, Some(77)),
    ] {
        let planned = stack.planned(&alice, name, planned_size).await;
        let held = stack.reservation(&planned.session).await;
        let key_before: String =
            sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
                .bind(&planned.item)
                .fetch_one(stack.pools.reader().executor())
                .await
                .unwrap();

        let created = stack.tus_create(&alice, &planned, length).await;
        assert_eq!(
            created.status,
            StatusCode::CREATED,
            "{name}: {}",
            created.text()
        );
        assert!(created.body.is_empty());
        assert_eq!(header(&created, "tus-resumable"), "1.0.0");
        assert_eq!(header(&created, "upload-offset"), "0");
        assert_eq!(header(&created, "cache-control"), "no-store");
        assert_eq!(
            header(&created, "upload-expires"),
            "Sat, 26 Sep 2026 12:00:00 GMT"
        );
        assert!(created.headers.contains_key("x-request-id"));
        let id = upload_id_of(&created);
        assert_eq!(
            header(&created, "location"),
            format!("https://files.example.test/api/v1/uploads/tus/{id}")
        );

        let row = stack.tus_row(&id).await;
        assert_eq!(row.state, "created", "{name}");
        assert_eq!(row.offset, 0);
        assert_eq!(
            row.length,
            length.map(|length| i64::try_from(length).unwrap())
        );
        assert_eq!(row.defer, i64::from(length.is_none()));
        assert_eq!(
            row.staging_path,
            format!("uploads/{}/blob", id.replace('-', ""))
        );
        assert!(row.locked_by.is_none());
        assert_eq!(row.expires_at, "2026-09-26T12:00:00.000Z");
        assert_eq!(stack.staged_bytes(&id), Some(Vec::new()), "{name}");
        let hint = stack.staged_hint(&id);
        let hint: Value = serde_json::from_str(&hint).unwrap();
        assert_eq!(hint["uploadId"], id);
        assert_eq!(hint["itemId"], planned.item);
        for forbidden in ["cookie", "csrf", "password", "token", "filename", "name"] {
            assert!(hint.get(forbidden).is_none(), "{forbidden}");
        }

        assert_eq!(stack.item_state(&planned.item).await, "uploading");
        assert_eq!(stack.session_row(&planned.session).await.0, "uploading");
        assert_eq!(
            stack.reservation(&planned.session).await,
            held,
            "no second hold"
        );
        let key_after: String =
            sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
                .bind(&planned.item)
                .fetch_one(stack.pools.reader().executor())
                .await
                .unwrap();
        assert_eq!(key_after, key_before, "the final identity is untouched");
        assert!(!created.text().contains(&key_before));
    }
    assert_eq!(
        stack.counts().await.objects,
        0,
        "no storage object at creation"
    );
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM files").await, 0);
    assert_eq!(stack.used_bytes(alice.id).await, 0);
    assert_eq!(stack.counts().await.reservations, 4);
}

#[tokio::test]
async fn it_tus_create_is_idempotent_per_item() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", None).await;

    let first = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(first.status, StatusCode::CREATED);
    let second = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(second.status, StatusCode::CREATED);
    assert_eq!(header(&first, "location"), header(&second, "location"));
    assert_eq!(header(&second, "upload-offset"), "0");
    assert_eq!(stack.tus_count().await, 1);
    assert_eq!(stack.staging_dirs().len(), 1);
    assert_eq!(stack.counts().await.reservations, 1);

    let conflicting = stack.tus_create(&alice, &planned, Some(11)).await;
    assert_tus_error(
        &conflicting,
        StatusCode::UNPROCESSABLE_ENTITY,
        "UPLOAD_LENGTH_MISMATCH",
    );
    let deferred = stack.tus_create(&alice, &planned, None).await;
    assert_tus_error(
        &deferred,
        StatusCode::UNPROCESSABLE_ENTITY,
        "UPLOAD_LENGTH_MISMATCH",
    );
    let row = stack.tus_row(&upload_id_of(&first)).await;
    assert_eq!((row.length, row.defer, row.offset), (Some(10), 0, 0));
    assert_eq!(stack.tus_count().await, 1);

    let racing = join_all((0..4).map(|_| stack.tus_create(&alice, &planned, Some(10)))).await;
    for response in &racing {
        assert_eq!(response.status, StatusCode::CREATED);
        assert_eq!(header(response, "location"), header(&first, "location"));
    }
    assert_eq!(stack.tus_count().await, 1);

    let other = stack.planned(&alice, "b.bin", None).await;
    let deferred_first = stack.tus_create(&alice, &other, None).await;
    assert_eq!(deferred_first.status, StatusCode::CREATED);
    let known_after = stack.tus_create(&alice, &other, Some(5)).await;
    assert_tus_error(
        &known_after,
        StatusCode::UNPROCESSABLE_ENTITY,
        "UPLOAD_LENGTH_MISMATCH",
    );
}

#[tokio::test]
async fn it_tus_create_rejects_invalid_headers_before_creating_anything() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", None).await;
    let meta = metadata(&planned.session, &planned.item, "a.bin", &[]);
    let base = || TusReq::new(Method::POST, TUS, &alice).header("upload-metadata", &meta);

    let bad_length = |value: &'static str| base().header("upload-length", value);
    for value in ["-1", "1.5", "abc", "", "01", " 1", "1e3", "+4"] {
        let response = stack.tus_send(bad_length(value)).await;
        assert_tus_error(
            &response,
            StatusCode::BAD_REQUEST,
            "UPLOAD_METADATA_INVALID",
        );
        assert_eq!(
            response.json()["error"]["details"]["key"],
            "Upload-Length",
            "{value:?}"
        );
    }
    for value in ["9007199254740992", "18446744073709551615"] {
        let response = stack.tus_send(bad_length(value)).await;
        assert_tus_error(&response, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    }
    let overflow = stack.tus_send(bad_length("18446744073709551616")).await;
    assert_tus_error(
        &overflow,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );

    let neither = stack.tus_send(base()).await;
    assert_tus_error(&neither, StatusCode::BAD_REQUEST, "UPLOAD_METADATA_INVALID");
    let both = stack
        .tus_send(
            base()
                .header("upload-length", "1")
                .header("upload-defer-length", "1"),
        )
        .await;
    assert_tus_error(&both, StatusCode::BAD_REQUEST, "UPLOAD_METADATA_INVALID");
    for value in ["0", "2", "true"] {
        let response = stack
            .tus_send(base().header("upload-defer-length", value))
            .await;
        assert_tus_error(
            &response,
            StatusCode::BAD_REQUEST,
            "UPLOAD_METADATA_INVALID",
        );
    }
    let repeated = stack
        .tus_send(
            base()
                .header("upload-length", "1")
                .header("upload-length", "1"),
        )
        .await;
    assert_tus_error(
        &repeated,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );
    let no_metadata = stack
        .tus_send(TusReq::new(Method::POST, TUS, &alice).header("upload-length", "1"))
        .await;
    assert_tus_error(
        &no_metadata,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );

    let missing = stack
        .tus_send(base().header("upload-length", "1").no_version())
        .await;
    assert_tus_error(
        &missing,
        StatusCode::PRECONDITION_FAILED,
        "TUS_VERSION_UNSUPPORTED",
    );
    assert_eq!(header(&missing, "tus-version"), "1.0.0");
    for version in ["1.0.1", "0.2.2", "2"] {
        let response = stack
            .tus_send(
                base()
                    .header("upload-length", "1")
                    .no_version()
                    .header("tus-resumable", version),
            )
            .await;
        assert_tus_error(
            &response,
            StatusCode::PRECONDITION_FAILED,
            "TUS_VERSION_UNSUPPORTED",
        );
        assert_eq!(header(&response, "tus-version"), "1.0.0");
    }
    assert_nothing_created(&stack, &planned.item).await;
}

#[tokio::test]
async fn it_tus_create_binds_the_exact_item_and_scope() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folder = stack.make(&alice, "target", None).await;
    let folder_id = folder["id"].as_str().unwrap().to_owned();
    let created = stack
        .open(
            &alice,
            &request_body(
                Some(&folder_id),
                &[
                    pathed("c1", "tree/inner/a.bin", 10),
                    pathed("c2", "tree/inner/a.bin.copy", 10),
                    sized("c3", "same.bin", 10),
                    sized("c4", "same.bin.2", 10),
                ],
            ),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    let post = |meta: String, length: &str| {
        TusReq::new(Method::POST, TUS, &alice)
            .header("upload-metadata", &meta)
            .header("upload-length", length)
    };

    let wrong_name = stack
        .tus_send(post(
            metadata(
                &session,
                &items[0],
                "other.bin",
                &[("relativePath", "tree/inner/other.bin")],
            ),
            "10",
        ))
        .await;
    assert_tus_error(
        &wrong_name,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );
    assert_eq!(wrong_name.json()["error"]["details"]["key"], "filename");

    let no_path = stack
        .tus_send(post(metadata(&session, &items[0], "a.bin", &[]), "10"))
        .await;
    assert_eq!(no_path.json()["error"]["details"]["key"], "relativePath");
    let other_path = stack
        .tus_send(post(
            metadata(
                &session,
                &items[0],
                "a.bin",
                &[("relativePath", "elsewhere/a.bin")],
            ),
            "10",
        ))
        .await;
    assert_eq!(other_path.json()["error"]["details"]["key"], "relativePath");
    let traversal = stack
        .tus_send(post(
            metadata(
                &session,
                &items[0],
                "passwd",
                &[("relativePath", "../../etc/passwd")],
            ),
            "10",
        ))
        .await;
    assert_eq!(traversal.json()["error"]["details"]["key"], "relativePath");
    let wrong_folder = stack
        .tus_send(post(
            metadata(&session, &items[2], "same.bin", &[("folderId", UNKNOWN_ID)]),
            "10",
        ))
        .await;
    assert_eq!(wrong_folder.json()["error"]["details"]["key"], "folderId");
    let public_scope = stack
        .tus_send(post(
            metadata(
                &session,
                &items[2],
                "same.bin",
                &[("reverseShareSessionId", UNKNOWN_ID)],
            ),
            "10",
        ))
        .await;
    assert_eq!(
        public_scope.json()["error"]["details"]["key"],
        "reverseShareSessionId"
    );
    let swapped = stack
        .tus_send(post(metadata(&session, &items[3], "same.bin", &[]), "10"))
        .await;
    assert_eq!(swapped.json()["error"]["details"]["key"], "filename");
    assert_eq!(stack.tus_count().await, 0);

    let ok = stack
        .tus_send(post(
            metadata(
                &session,
                &items[0],
                "a.bin",
                &[
                    ("relativePath", "tree/inner/a.bin"),
                    ("folderId", &folder_id),
                    ("filetype", "application/x-evil; charset=\"x\""),
                    ("ownerId", "someone-else"),
                    ("storageKey", "objects/00/00/forged"),
                ],
            ),
            "10",
        ))
        .await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.text());
    let stored: String = sqlx::query_scalar("SELECT metadata_json FROM tus_uploads WHERE id = ?1")
        .bind(upload_id_of(&ok))
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert!(!stored.contains("someone-else") && !stored.contains("forged"));
    let target_after: Option<String> =
        sqlx::query_scalar("SELECT target_folder_id FROM transfer_sessions WHERE id = ?1")
            .bind(&session)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(target_after.as_deref(), Some(folder_id.as_str()));
    assert_eq!(
        stack.item_state(&items[1]).await,
        "pending",
        "siblings are untouched"
    );
    assert_eq!(stack.item_state(&items[0]).await, "uploading");
}

#[tokio::test]
async fn it_tus_create_refuses_unknown_foreign_and_unusable_sessions() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;

    let invalid_session = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-length", "10")
                .header(
                    "upload-metadata",
                    &metadata("nope", &planned.item, "a.bin", &[]),
                ),
        )
        .await;
    assert_tus_error(
        &invalid_session,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );
    assert_eq!(
        invalid_session.json()["error"]["details"]["key"],
        "transferSessionId"
    );
    let invalid_item = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-length", "10")
                .header(
                    "upload-metadata",
                    &metadata(&planned.session, "nope", "a.bin", &[]),
                ),
        )
        .await;
    assert_eq!(invalid_item.json()["error"]["details"]["key"], "itemId");

    let post = |session: &str, item: &str| {
        stack.tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-length", "10")
                .header("upload-metadata", &metadata(session, item, "a.bin", &[])),
        )
    };
    assert_code(
        &post(UNKNOWN_ID, &planned.item).await,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    assert_code(
        &post(&planned.session, UNKNOWN_ID).await,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let bob_planned = stack.planned(&bob, "a.bin", Some(10)).await;
    assert_code(
        &post(&planned.session, &bob_planned.item).await,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let foreign = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &bob)
                .header("upload-length", "10")
                .header(
                    "upload-metadata",
                    &metadata(&planned.session, &planned.item, "a.bin", &[]),
                ),
        )
        .await;
    assert_code(
        &foreign,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    assert_eq!(stack.tus_count().await, 0);

    let canceled = stack.planned(&alice, "c.bin", Some(10)).await;
    assert_eq!(
        stack.cancel_session(&alice, &canceled.session).await.status,
        StatusCode::NO_CONTENT
    );
    let response = stack.tus_create(&alice, &canceled, Some(10)).await;
    assert_tus_error(
        &response,
        StatusCode::CONFLICT,
        "TRANSFER_SESSION_STATE_INVALID",
    );

    let item_canceled = stack.planned(&alice, "d.bin", Some(10)).await;
    stack.set_item_state(&item_canceled.item, "canceled").await;
    let response = stack.tus_create(&alice, &item_canceled, Some(10)).await;
    assert_tus_error(
        &response,
        StatusCode::CONFLICT,
        "TRANSFER_SESSION_STATE_INVALID",
    );

    let expired_live = stack.planned(&alice, "e.bin", Some(10)).await;
    stack
        .execute(&format!(
            "UPDATE transfer_sessions SET expires_at = '2026-09-25T11:00:00.000Z' WHERE id = '{}'",
            expired_live.session
        ))
        .await;
    let response = stack.tus_create(&alice, &expired_live, Some(10)).await;
    assert_tus_error(&response, StatusCode::GONE, "TRANSFER_SESSION_EXPIRED");

    let reaped = stack.planned(&alice, "f.bin", Some(10)).await;
    stack.set_session_state(&reaped.session, "expired").await;
    let response = stack.tus_create(&alice, &reaped, Some(10)).await;
    assert_tus_error(
        &response,
        StatusCode::CONFLICT,
        "TRANSFER_SESSION_STATE_INVALID",
    );

    let finished = stack.planned(&alice, "g.bin", Some(10)).await;
    stack.set_item_state(&finished.item, "finalizing").await;
    let response = stack.tus_create(&alice, &finished, Some(10)).await;
    assert_tus_error(
        &response,
        StatusCode::CONFLICT,
        "TRANSFER_SESSION_STATE_INVALID",
    );

    assert_eq!(stack.tus_count().await, 0);
    assert!(stack.staging_dirs().is_empty());
}

#[tokio::test]
async fn it_tus_create_checks_length_against_the_plan_and_policy() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let known = stack.planned(&alice, "known.bin", Some(100)).await;

    for length in [Some(99), Some(101), Some(0), None] {
        let response = stack.tus_create(&alice, &known, length).await;
        assert_tus_error(
            &response,
            StatusCode::UNPROCESSABLE_ENTITY,
            "UPLOAD_LENGTH_MISMATCH",
        );
    }
    stack.set_max_file_size(50).await;
    let too_large = stack.tus_create(&alice, &known, Some(100)).await;
    assert_tus_error(&too_large, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    assert_eq!(too_large.json()["error"]["details"]["maxBytes"], 50);
    let unsized_file = stack.planned(&alice, "free.bin", None).await;
    let over = stack.tus_create(&alice, &unsized_file, Some(51)).await;
    assert_tus_error(&over, StatusCode::PAYLOAD_TOO_LARGE, "FILE_TOO_LARGE");
    let at_limit = stack.tus_create(&alice, &unsized_file, Some(50)).await;
    assert_eq!(at_limit.status, StatusCode::CREATED);
    assert_eq!(stack.tus_count().await, 1);
    assert_eq!(stack.item_state(&known.item).await, "pending");
}

#[tokio::test]
async fn it_tus_create_refuses_a_deleting_folder_and_unavailable_storage() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folder = stack.make(&alice, "target", None).await;
    let folder_id = folder["id"].as_str().unwrap().to_owned();
    let created = stack
        .open(
            &alice,
            &request_body(Some(&folder_id), &[sized("c1", "a.bin", 10)]),
        )
        .await;
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let item = stack.item_ids(&session).await.remove(0);
    let planned = Planned {
        session,
        item,
        name: "a.bin".to_owned(),
    };

    stack
        .execute(&format!(
            "UPDATE folders SET deleting = 1 WHERE id = '{folder_id}'"
        ))
        .await;
    let response = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_tus_error(&response, StatusCode::CONFLICT, "FOLDER_DELETING");
    assert_nothing_created(&stack, &planned.item).await;
    stack
        .execute(&format!(
            "UPDATE folders SET deleting = 0 WHERE id = '{folder_id}'"
        ))
        .await;

    stack.storage_down.store(true, Ordering::SeqCst);
    let down = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_tus_error(
        &down,
        StatusCode::SERVICE_UNAVAILABLE,
        "STORAGE_UNAVAILABLE",
    );
    assert_nothing_created(&stack, &planned.item).await;
    stack.storage_down.store(false, Ordering::SeqCst);
    assert_eq!(
        stack.tus_create(&alice, &planned, Some(10)).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn it_tus_create_is_limited_by_the_session_expiry() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    stack
        .execute(&format!(
            "UPDATE transfer_sessions SET expires_at = '2026-09-25T14:00:00.000Z' WHERE id = '{}'",
            planned.session
        ))
        .await;
    let created = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(
        header(&created, "upload-expires"),
        "Fri, 25 Sep 2026 14:00:00 GMT"
    );
    let row = stack.tus_row(&upload_id_of(&created)).await;
    assert_eq!(row.expires_at, "2026-09-25T14:00:00.000Z");
}

#[tokio::test]
async fn it_tus_create_recovers_from_staging_and_database_failures() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let staging = TestStaging::new(root.path(), &clock);
    let stack = Stack::start_with_tus(
        root.path(),
        &clock,
        TusSetup {
            staging: Some(staging.clone()),
            limits: small_limits(),
        },
    )
    .await;
    *staging.probe.lock().unwrap() = Some(stack.pools.clone());
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;

    stack
        .execute(
            "CREATE TRIGGER fail_tus_insert BEFORE INSERT ON tus_uploads
             BEGIN SELECT RAISE(ABORT, 'injected'); END",
        )
        .await;
    let failed = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_nothing_created(&stack, &planned.item).await;
    assert_eq!(stack.session_row(&planned.session).await.0, "created");
    assert_eq!(staging.ensure_calls.load(Ordering::SeqCst), 0);
    stack.execute("DROP TRIGGER fail_tus_insert").await;

    staging.fail_ensure.store(true, Ordering::SeqCst);
    let unavailable = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_tus_error(
        &unavailable,
        StatusCode::SERVICE_UNAVAILABLE,
        "STORAGE_UNAVAILABLE",
    );
    assert_eq!(
        stack.tus_count().await,
        1,
        "the durable intent survives the failure"
    );
    assert!(stack.staging_dirs().is_empty());
    staging.fail_ensure.store(false, Ordering::SeqCst);

    let retried = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(retried.status, StatusCode::CREATED);
    let id = upload_id_of(&retried);
    assert_eq!(stack.tus_count().await, 1);
    assert_eq!(
        stack.staged_bytes(&id),
        Some(Vec::new()),
        "the retry heals staging"
    );

    std::fs::remove_dir_all(root.path().join("uploads").join(id.replace('-', ""))).unwrap();
    let head = stack
        .tus_send(TusReq::new(Method::HEAD, &format!("{TUS}/{id}"), &alice))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(
        header(&head, "upload-offset"),
        "0",
        "a missing blob justifies no bytes"
    );
    let healed = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(header(&healed, "location"), header(&retried, "location"));
    assert_eq!(stack.staged_bytes(&id), Some(Vec::new()));
    assert!(
        !staging.overlapped_transaction.load(Ordering::SeqCst),
        "staging I/O never overlaps a write transaction"
    );
}

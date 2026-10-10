use std::path::Path as FsPath;

use super::folders::{Member, HOST_A, HOST_B};
use super::profile::assert_code;
use super::transfers::{request_body, s3_stack_storage, sized};
use super::tus::{
    assert_head_error, assert_tus_error, header, metadata, upload_id_of, Planned, TusReq, TUS,
};
use super::*;
use crate::storage::s3::profile::ProviderProfile;

const UNKNOWN: &str = "0192f3a7-2a01-7c4d-8e11-aa0192f3a799";

fn resource<'a>(member: &'a Member, id: &str, method: Method) -> TusReq<'a> {
    TusReq::new(method, &format!("{TUS}/{id}"), member)
}

#[tokio::test]
async fn it_tus_non_owner_404() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let alice_plan = stack.planned(&alice, "a.bin", Some(100)).await;
    let bob_plan = stack.planned(&bob, "b.bin", Some(100)).await;
    let id = stack.tus_created(&alice, &alice_plan, Some(100)).await;
    let before = stack.dump().await;
    let alice_used = stack.used_bytes(alice.id).await;
    let bob_used = stack.used_bytes(bob.id).await;

    let head = stack.tus_send(resource(&bob, &id, Method::HEAD)).await;
    assert_head_error(&head, StatusCode::NOT_FOUND);
    let delete = stack.tus_send(resource(&bob, &id, Method::DELETE)).await;
    assert_tus_error(&delete, StatusCode::NOT_FOUND, "NOT_FOUND");
    let overridden = stack
        .tus_send(resource(&bob, &id, Method::POST).header("x-http-method-override", "DELETE"))
        .await;
    assert_tus_error(&overridden, StatusCode::NOT_FOUND, "NOT_FOUND");
    let unknown_delete = stack
        .tus_send(resource(
            &bob,
            "0192f3a7-2a01-7c4d-8e11-aa0192f3a799",
            Method::DELETE,
        ))
        .await;
    assert_eq!(
        delete.error_without_request_id(),
        unknown_delete.error_without_request_id()
    );

    let duplicate = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &bob)
                .header("upload-length", "100")
                .header(
                    "upload-metadata",
                    &metadata(&alice_plan.session, &alice_plan.item, "a.bin", &[]),
                ),
        )
        .await;
    assert_tus_error(
        &duplicate,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let alice_item_in_bob_session = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &bob)
                .header("upload-length", "100")
                .header(
                    "upload-metadata",
                    &metadata(&bob_plan.session, &alice_plan.item, "a.bin", &[]),
                ),
        )
        .await;
    assert_tus_error(
        &alice_item_in_bob_session,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let bob_item_in_alice_session = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-length", "100")
                .header(
                    "upload-metadata",
                    &metadata(&alice_plan.session, &bob_plan.item, "b.bin", &[]),
                ),
        )
        .await;
    assert_tus_error(
        &bob_item_in_alice_session,
        StatusCode::NOT_FOUND,
        "TRANSFER_SESSION_NOT_FOUND",
    );
    let own_item_forged_as_other = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &bob)
                .header("upload-length", "100")
                .header(
                    "upload-metadata",
                    &metadata(
                        &bob_plan.session,
                        &bob_plan.item,
                        "b.bin",
                        &[("ownerId", &alice.id.to_string())],
                    ),
                ),
        )
        .await;
    assert_eq!(
        own_item_forged_as_other.status,
        StatusCode::CREATED,
        "metadata never selects the owner"
    );
    let bob_upload = upload_id_of(&own_item_forged_as_other);
    let owner: String = sqlx::query_scalar("SELECT owner_user_id FROM tus_uploads WHERE id = ?1")
        .bind(&bob_upload)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(owner, bob.id.to_string());

    assert_eq!(stack.tus_row(&id).await.state, "created");
    assert_eq!(stack.item_state(&alice_plan.item).await, "uploading");
    assert_eq!(stack.used_bytes(alice.id).await, alice_used);
    assert_eq!(stack.used_bytes(bob.id).await, bob_used);
    let mut after = stack.dump().await;
    let tus_dump = after
        .iter()
        .position(|entry| entry.contains("FROM tus_uploads"))
        .unwrap();
    assert!(after[tus_dump].contains(&bob_upload));
    after.clear();
    assert_eq!(before.len(), 8);
}

#[tokio::test]
async fn it_tus_requires_csrf_for_state_changes_and_authentication_everywhere() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;

    let no_csrf = stack
        .tus_send(stack.create_req(&alice, &planned, Some(10)).no_csrf())
        .await;
    assert_tus_error(&no_csrf, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING");
    let evil_origin = stack
        .tus_send(
            stack
                .create_req(&alice, &planned, Some(10))
                .header("origin", "https://evil.test"),
        )
        .await;
    assert_eq!(evil_origin.status, StatusCode::FORBIDDEN);
    let anonymous = stack
        .tus_send(TusReq::anonymous(Method::POST, TUS).header("upload-length", "1"))
        .await;
    assert_eq!(anonymous.status, StatusCode::FORBIDDEN);
    assert_eq!(header(&anonymous, "tus-resumable"), "1.0.0");
    let anonymous_read = stack
        .tus_send(TusReq::anonymous(Method::HEAD, &format!("{TUS}/{UNKNOWN}")))
        .await;
    assert_eq!(anonymous_read.status, StatusCode::UNAUTHORIZED);
    assert_eq!(header(&anonymous_read, "tus-resumable"), "1.0.0");
    assert_eq!(stack.tus_count().await, 0);

    let id = stack.tus_created(&alice, &planned, Some(10)).await;
    let read = stack
        .tus_send(resource(&alice, &id, Method::HEAD).no_csrf())
        .await;
    assert_eq!(
        read.status,
        StatusCode::OK,
        "reads follow the read-method rules"
    );
    let bypass = stack
        .tus_send(
            resource(&alice, &id, Method::POST)
                .no_csrf()
                .header("x-http-method-override", "DELETE"),
        )
        .await;
    assert_tus_error(&bypass, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING");
    let bypass_head = stack
        .tus_send(
            resource(&alice, &id, Method::POST)
                .no_csrf()
                .header("x-http-method-override", "HEAD"),
        )
        .await;
    assert_tus_error(&bypass_head, StatusCode::FORBIDDEN, "CSRF_TOKEN_MISSING");
    assert_eq!(stack.tus_row(&id).await.state, "created");
}

#[tokio::test]
async fn it_tus_method_override_is_scoped_to_the_resource_route() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let id = stack.tus_created(&alice, &planned, Some(10)).await;
    let post = |value: &'static str| {
        resource(&alice, &id, Method::POST).header("x-http-method-override", value)
    };

    let head = stack.tus_send(post("HEAD")).await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(header(&head, "upload-offset"), "0");
    assert_eq!(header(&head, "upload-length"), "10");

    let patch = stack.tus_send(post("PATCH")).await;
    assert_tus_error(&patch, StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED");
    let real_patch = stack.tus_send(resource(&alice, &id, Method::PATCH)).await;
    assert_eq!(real_patch.status, StatusCode::METHOD_NOT_ALLOWED);
    for value in ["GET", "PUT", "OPTIONS", "head", "TRACE", ""] {
        let rejected = stack.tus_send(post(value)).await;
        assert_tus_error(
            &rejected,
            StatusCode::BAD_REQUEST,
            "UPLOAD_METADATA_INVALID",
        );
    }
    let plain = stack.tus_send(resource(&alice, &id, Method::POST)).await;
    assert_tus_error(&plain, StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED");
    let no_version = stack.tus_send(post("DELETE").no_version()).await;
    assert_tus_error(
        &no_version,
        StatusCode::PRECONDITION_FAILED,
        "TUS_VERSION_UNSUPPORTED",
    );

    let collection = stack
        .tus_send(
            stack
                .create_req(&alice, &planned, Some(10))
                .header("x-http-method-override", "DELETE"),
        )
        .await;
    assert_tus_error(
        &collection,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );
    assert_eq!(
        stack.tus_row(&id).await.state,
        "created",
        "no override silently acted"
    );

    let other = stack
        .api(
            Method::POST,
            &format!("/api/v1/transfers/sessions/{}/complete", planned.session),
            &alice,
            None,
        )
        .await;
    let with_override = stack
        .call(
            super::profile::Call::new(
                Method::POST,
                &format!("/api/v1/transfers/sessions/{}/complete", planned.session),
                &alice.creds,
            ),
            super::folders::HOST_WORK,
        )
        .await;
    assert_eq!(
        other.status, with_override.status,
        "other routes are untouched"
    );

    let deleted = stack.tus_send(post("DELETE")).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.tus_row(&id).await.state, "terminated");
    let twice = stack.tus_send(post("DELETE")).await;
    assert_eq!(twice.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn it_tus_location_is_built_only_from_the_configured_base_url() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let created = stack
        .tus_send(
            stack
                .create_req(&alice, &planned, Some(10))
                .header("host", "evil.example")
                .header("x-forwarded-host", "evil.example")
                .header("x-forwarded-proto", "http")
                .header("forwarded", "host=evil.example;proto=http")
                .header("x-forwarded-prefix", "/evil"),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = upload_id_of(&created);
    let location = header(&created, "location");
    assert_eq!(
        location,
        format!("https://files.example.test/api/v1/uploads/tus/{id}")
    );
    for leaked in ["evil", "objects/", "/blob", "palmr_session", "csrf"] {
        assert!(!location.contains(leaked), "{leaked}");
    }
    let origin = stack
        .tus_send(
            stack
                .create_req(&alice, &planned, Some(10))
                .header("origin", "https://evil.example"),
        )
        .await;
    assert_eq!(origin.status, StatusCode::FORBIDDEN);

    let proxied_root = TempDir::new().unwrap();
    let proxied = Stack::start_configured(
        proxied_root.path(),
        &TestClock::new(START),
        Routes::new(),
        &[("PALMR_TRUST_PROXY", "10.0.0.0/8")],
    )
    .await;
    let bob = proxied.member("bob", HOST_B).await;
    let plan = proxied.planned(&bob, "a.bin", Some(10)).await;
    let created = proxied
        .tus_send(
            proxied
                .create_req(&bob, &plan, Some(10))
                .header("x-forwarded-host", "proxy.example")
                .header("x-forwarded-proto", "http")
                .via_peer(10),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert!(
        header(&created, "location").starts_with("https://files.example.test/api/v1/uploads/tus/")
    );
}

#[tokio::test]
async fn it_tus_location_supports_a_non_root_base_path() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start_configured(
        root.path(),
        &TestClock::new(START),
        Routes::new(),
        &[("PALMR_BASE_URL", "https://files.example.test/palmr")],
    )
    .await;
    let alice = stack.member("alice", HOST_A).await;
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let created = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(
        header(&created, "location"),
        format!(
            "https://files.example.test/palmr/api/v1/uploads/tus/{}",
            upload_id_of(&created)
        )
    );
}

fn walk(path: &FsPath, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        found.push(entry.file_name().to_string_lossy().into_owned());
        if entry.file_type().unwrap().is_dir() {
            walk(&entry.path(), found);
        }
    }
}

#[tokio::test]
async fn regression_290_utf8_filename_large_upload() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let names = [
        format!("{}.7z", "日本語".repeat(28)),
        format!("{}.mkv", "🎬".repeat(60)),
        "Relatório_de_Março_Ação_final (versão 2).pdf".to_owned(),
        "Cafe\u{301} re\u{301}sume\u{301}.txt".to_owned(),
        "中文文件名 测试.docx".to_owned(),
        "CON.txt".to_owned(),
        "name.with.many.dots.tar.gz.bak".to_owned(),
        "  spaced  name  .bin".trim().to_owned(),
        "😀".to_owned(),
    ];
    let size = 150 * 1024 * 1024_u64;
    let mut ids = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let created = stack
            .open(
                &alice,
                &request_body(None, &[sized(&format!("c{index}"), name, size)]),
            )
            .await;
        assert_eq!(
            created.status,
            StatusCode::CREATED,
            "{name}: {}",
            created.text()
        );
        let session = created.json()["id"].as_str().unwrap().to_owned();
        let item = stack.item_ids(&session).await.remove(0);
        let planned = Planned {
            session,
            item,
            name: name.clone(),
        };
        let key_before: String =
            sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
                .bind(&planned.item)
                .fetch_one(stack.pools.reader().executor())
                .await
                .unwrap();
        let normalized: String = {
            use unicode_normalization::UnicodeNormalization;
            name.nfc().collect()
        };
        let response = stack
            .tus_send(
                TusReq::new(Method::POST, TUS, &alice)
                    .header("upload-length", &size.to_string())
                    .header(
                        "upload-metadata",
                        &metadata(
                            &planned.session,
                            &planned.item,
                            name,
                            &[("filetype", "application/octet-stream")],
                        ),
                    ),
            )
            .await;
        assert_eq!(
            response.status,
            StatusCode::CREATED,
            "{name}: {}",
            response.text()
        );
        let id = upload_id_of(&response);
        let row = stack.tus_row(&id).await;
        assert_eq!(
            row.staging_path,
            format!("uploads/{}/blob", id.replace('-', ""))
        );
        let key_after: String =
            sqlx::query_scalar("SELECT final_object_key FROM transfer_session_files WHERE id = ?1")
                .bind(&planned.item)
                .fetch_one(stack.pools.reader().executor())
                .await
                .unwrap();
        assert_eq!(key_after, key_before);
        let stored: String =
            sqlx::query_scalar("SELECT display_name FROM transfer_session_files WHERE id = ?1")
                .bind(&planned.item)
                .fetch_one(stack.pools.reader().executor())
                .await
                .unwrap();
        assert_eq!(stored, normalized);
        ids.push(id.replace('-', ""));
    }

    let mut found = Vec::new();
    walk(root.path(), &mut found);
    for name in &names {
        assert!(
            !found.iter().any(|entry| entry.contains(name.trim())
                || name.contains(entry.as_str()) && entry.len() > 3),
            "{name} appears on the filesystem"
        );
    }
    let mut staged = Vec::new();
    walk(&root.path().join("uploads"), &mut staged);
    staged.sort();
    let mut expected: Vec<String> = ids
        .iter()
        .flat_map(|id| [id.clone(), "blob".to_owned(), "meta.json".to_owned()])
        .collect();
    expected.sort();
    assert_eq!(
        staged, expected,
        "only server-generated names exist under uploads/"
    );
    assert_eq!(stack.counts().await.objects, 0);

    let planned = stack.planned(&alice, "short.bin", Some(1)).await;
    let oversized = "x".repeat(600);
    let rejected = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-length", "1")
                .header(
                    "upload-metadata",
                    &metadata(&planned.session, &planned.item, &oversized, &[]),
                ),
        )
        .await;
    assert_tus_error(
        &rejected,
        StatusCode::BAD_REQUEST,
        "UPLOAD_METADATA_INVALID",
    );
    assert_eq!(rejected.json()["error"]["details"]["key"], "filename");
    let traversal = stack
        .tus_send(
            TusReq::new(Method::POST, TUS, &alice)
                .header("upload-length", "1")
                .header(
                    "upload-metadata",
                    &metadata(&planned.session, &planned.item, "../../../etc/passwd", &[]),
                ),
        )
        .await;
    assert_eq!(traversal.json()["error"]["details"]["key"], "filename");
}

#[tokio::test]
async fn it_tus_is_not_offered_when_uploads_are_not_local() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start_with_storage(
        root.path(),
        &TestClock::new(START),
        s3_stack_storage(ProviderProfile::Generic, false),
    )
    .await;
    let alice = stack.member("alice", HOST_A).await;
    let options = stack
        .tus_send(TusReq::new(Method::OPTIONS, TUS, &alice).no_version())
        .await;
    assert_code(&options, StatusCode::NOT_FOUND, "NOT_FOUND");
    let planned = stack.planned(&alice, "a.bin", Some(10)).await;
    let created = stack.tus_create(&alice, &planned, Some(10)).await;
    assert_code(&created, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(stack.tus_count().await, 0);
}

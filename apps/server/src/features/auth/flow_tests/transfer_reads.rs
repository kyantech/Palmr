use super::folders::{Member, HOST_A, HOST_B};
use super::profile::assert_code;
use super::transfers::{request_body, s3_stack_storage, sized, unsized_file, SESSIONS};
use super::*;
use crate::storage::s3::profile::{ProviderProfile, MIB};

impl Stack {
    async fn sessions_for(&self, member: &Member, count: usize) -> Vec<String> {
        let mut ids = Vec::new();
        for index in 0..count {
            let created = self
                .opened(
                    member,
                    &[sized(
                        &format!("c{index}"),
                        &format!("f{index}.bin"),
                        10 + u64::try_from(index).unwrap(),
                    )],
                )
                .await;
            ids.push(created["id"].as_str().unwrap().to_owned());
        }
        ids
    }

    async fn listing(&self, member: &Member, query: &str) -> Fetched {
        let path = if query.is_empty() {
            SESSIONS.to_owned()
        } else {
            format!("{SESSIONS}?{query}")
        };
        self.read(&path, member).await
    }
}

fn ids_of(listing: &Fetched) -> Vec<String> {
    listing.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn it_transfer_session_list_is_owner_scoped_ordered_and_filterable() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let carol = stack.member("carol", 13).await;

    let empty = stack.listing(&carol, "").await;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.json()["items"], json!([]));
    assert_eq!(empty.json()["nextCursor"], Value::Null);
    assert_eq!(empty.json()["totalCount"], 0);

    let mine = stack.sessions_for(&alice, 5).await;
    let others = stack.sessions_for(&bob, 2).await;
    for (id, state) in mine
        .iter()
        .zip(["created", "uploading", "failed", "canceled", "uploading"])
    {
        stack.set_session_state(id, state).await;
    }
    let newest_first: Vec<String> = mine.iter().rev().cloned().collect();

    let all = stack.listing(&alice, "").await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.text());
    assert_eq!(
        ids_of(&all),
        newest_first,
        "created_at then id, newest first"
    );
    assert_eq!(all.json()["totalCount"], 5);
    for other in &others {
        assert!(
            !all.text().contains(other.as_str()),
            "a foreign id appears in the list"
        );
    }
    assert!(!all.text().contains("objects/"));
    let first = &all.json()["items"][4];
    assert_eq!(first["id"], mine[0]);
    assert_eq!(first["fileCount"], 1);
    assert_eq!(first["completedFileCount"], 0);
    assert_eq!(first["totalBytes"], 10);
    assert_eq!(first["uploadedBytes"], 0);
    assert_eq!(first["reservedBytes"], 10);
    assert!(
        first.get("files").is_none(),
        "the list carries summaries, not files"
    );

    let live = stack.listing(&alice, "state=uploading&state=failed").await;
    assert_eq!(
        ids_of(&live),
        [mine[4].clone(), mine[2].clone(), mine[1].clone()]
    );
    assert_eq!(live.json()["totalCount"], 3);
    let single = stack.listing(&alice, "state=canceled").await;
    assert_eq!(ids_of(&single), [mine[3].clone()]);
    let none = stack.listing(&alice, "state=completed").await;
    assert_eq!(ids_of(&none), Vec::<String>::new());

    for client_only in ["paused", "queued", "preparing", "resumable", "pending", ""] {
        let rejected = stack.listing(&alice, &format!("state={client_only}")).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }
    let mixed = stack.listing(&alice, "state=uploading&state=paused").await;
    assert_code(&mixed, StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR");
    for limit in ["0", "201", "abc"] {
        let rejected = stack.listing(&alice, &format!("limit={limit}")).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
    }

    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match &cursor {
            Some(cursor) => format!("limit=2&cursor={cursor}"),
            None => "limit=2".to_owned(),
        };
        let page = stack.listing(&alice, &query).await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.text());
        walked.extend(ids_of(&page));
        pages += 1;
        assert_eq!(page.json()["totalCount"], 5);
        cursor = page.json()["nextCursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(
        walked, newest_first,
        "a small-page walk yields every session once, in order"
    );

    let page = stack.listing(&alice, "limit=2").await;
    let cursor = page.json()["nextCursor"].as_str().unwrap().to_owned();
    let mut tampered: Vec<char> = cursor.chars().collect();
    let middle = tampered.len() / 2;
    tampered[middle] = if tampered[middle] == 'A' { 'B' } else { 'A' };
    let tampered: String = tampered.into_iter().collect();
    let forged = stack
        .listing(&alice, &format!("limit=2&cursor={tampered}"))
        .await;
    assert_code(&forged, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let garbage = stack.listing(&alice, "cursor=not-a-cursor").await;
    assert_code(&garbage, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let other_filter = stack
        .listing(&alice, &format!("limit=2&state=uploading&cursor={cursor}"))
        .await;
    assert_code(&other_filter, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let from_bob = stack
        .listing(&bob, &format!("limit=2&cursor={cursor}"))
        .await;
    assert_eq!(
        from_bob.status,
        StatusCode::OK,
        "a cursor carries a position, never an owner"
    );
    assert_eq!(from_bob.json()["totalCount"], 2);
    assert!(
        !from_bob.text().contains(mine[0].as_str()),
        "a cursor cannot expose another owner's sessions"
    );

    let service = include_str!("../../transfers/service.rs");
    let repo = include_str!("../../transfers/repo.rs");
    assert!(!service.to_uppercase().contains(" OFFSET "));
    assert!(!repo.to_uppercase().contains(" OFFSET "));
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_detail_reports_persisted_progress_only() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let created = stack
        .opened(
            &alice,
            &[
                sized("c1", "one.bin", 100),
                sized("c2", "two.bin", 200),
                sized("c3", "three.bin", 300),
                unsized_file("c4", "four.bin"),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    let fresh = stack.session_detail(&alice, &session).await.json();
    assert_eq!(fresh["uploadedBytes"], 0, "admission is not progress");
    for file in fresh["files"].as_array().unwrap() {
        assert_eq!(file["uploadedBytes"], 0);
        assert_eq!(file["fileId"], Value::Null);
        assert_eq!(file["state"], "created");
    }
    assert_eq!(fresh["totalBytes"], 600);
    assert_eq!(fresh["reservedBytes"], 600);

    stack.set_item_state(&items[0], "uploading").await;
    stack
        .seed_tus(alice.id, &items[0], 100, 40, "in_progress")
        .await;
    stack.set_item_state(&items[1], "failed").await;
    stack
        .seed_tus(alice.id, &items[1], 200, 120, "in_progress")
        .await;
    let file_id = stack.seed_completed_item(alice.id, &items[2], 3, 300).await;
    stack.set_session_state(&session, "uploading").await;

    let detail = stack.session_detail(&alice, &session).await;
    assert_eq!(detail.status, StatusCode::OK);
    let body = detail.json();
    assert_eq!(body["state"], "uploading");
    assert_eq!(body["files"][0]["uploadedBytes"], 40);
    assert_eq!(body["files"][1]["uploadedBytes"], 120);
    assert_eq!(body["files"][2]["uploadedBytes"], 300);
    assert_eq!(body["files"][3]["uploadedBytes"], 0);
    assert_eq!(body["uploadedBytes"], 460);
    assert_eq!(body["files"][2]["fileId"], file_id);
    assert_eq!(body["files"][2]["state"], "completed");
    for index in [0, 1, 3] {
        assert_eq!(body["files"][index]["fileId"], Value::Null);
    }
    assert!(!detail.text().contains("objects/"));
    assert!(!detail.text().contains("staging"));
    assert!(!detail.text().contains("/blob"));

    let summary = stack.listing(&alice, "").await.json();
    assert_eq!(
        summary["items"][0]["uploadedBytes"], 460,
        "list and detail agree"
    );
    assert_eq!(summary["items"][0]["completedFileCount"], 1);

    stack.set_session_state(&session, "canceled").await;
    let terminal = stack.session_detail(&alice, &session).await;
    assert_eq!(
        terminal.status,
        StatusCode::OK,
        "a terminal session stays readable"
    );
    assert_eq!(terminal.json()["state"], "canceled");
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_detail_reads_s3_progress_from_persisted_parts() {
    let root = TempDir::new().unwrap();
    let storage = s3_stack_storage(ProviderProfile::Minio, false);
    let stack = Stack::start_with_storage(root.path(), &TestClock::new(START), storage).await;
    let alice = stack.member("alice", HOST_A).await;

    let created = stack
        .opened(
            &alice,
            &[
                sized("c1", "big.bin", 13 * MIB),
                sized("c2", "plain.bin", 9 * MIB),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    assert_eq!(created["files"][0]["s3"]["partSizeBytes"], 8 * MIB);
    assert_eq!(created["files"][0]["s3"]["partCount"], 2);
    assert!(created["files"][0]["s3"].get("completedParts").is_none());

    stack.set_item_state(&items[0], "uploading").await;
    stack
        .seed_multipart(
            alice.id,
            &items[0],
            &[
                (1, 5_242_880, "uploaded"),
                (2, 5_242_880, "uploaded"),
                (3, 1_048_576, "planned"),
            ],
            "in_progress",
        )
        .await;
    stack.set_session_state(&session, "uploading").await;

    let detail = stack.session_detail(&alice, &session).await.json();
    let file = &detail["files"][0];
    assert_eq!(file["state"], "uploading");
    assert_eq!(file["uploadedBytes"], 10_485_760);
    assert_eq!(file["s3"]["completedParts"], 2);
    assert_eq!(
        file["s3"]["partSizeBytes"], 5_242_880,
        "the stored multipart plan wins once it exists"
    );
    assert_eq!(file["s3"]["partCount"], 3);
    assert_eq!(detail["files"][1]["uploadedBytes"], 0);
    assert!(detail["files"][1]["s3"].get("completedParts").is_none());
    assert_eq!(detail["uploadedBytes"], 10_485_760);
    let text = serde_json::to_string(&detail).unwrap();
    for forbidden in ["upload-id", "bucket", "etag", "objects/"] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }

    let summary = stack.listing(&alice, "").await.json();
    assert_eq!(summary["items"][0]["uploadedBytes"], 10_485_760);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_requires_authentication_and_a_csrf_proof() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let created = stack.opened(&alice, &[sized("c", "a.bin", 1)]).await;
    let session = created["id"].as_str().unwrap();
    let item = stack.item_ids(session).await.remove(0);

    for (method, path) in [
        (Method::POST, SESSIONS.to_owned()),
        (Method::GET, SESSIONS.to_owned()),
        (Method::GET, format!("{SESSIONS}/{session}")),
        (Method::DELETE, format!("{SESSIONS}/{session}")),
        (Method::POST, format!("{SESSIONS}/{session}/complete")),
        (
            Method::POST,
            format!("{SESSIONS}/{session}/files/{item}/retry"),
        ),
        (Method::DELETE, format!("{SESSIONS}/{session}/files/{item}")),
    ] {
        let anonymous = stack
            .send(with_peer(
                Request::builder()
                    .method(method.clone())
                    .uri(&path)
                    .header(ORIGIN, BASE_URL)
                    .header(CONTENT_TYPE, "application/json")
                    .header(COOKIE, format!("palmr_csrf={}", alice.creds.csrf))
                    .header(CSRF_HEADER, &alice.creds.csrf)
                    .body(Body::from("{}"))
                    .unwrap(),
                HOST_A,
            ))
            .await;
        assert_eq!(
            anonymous.status,
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }

    let dump = stack.dump().await;
    for (method, path) in [
        (Method::POST, SESSIONS.to_owned()),
        (Method::DELETE, format!("{SESSIONS}/{session}")),
        (Method::POST, format!("{SESSIONS}/{session}/complete")),
        (
            Method::POST,
            format!("{SESSIONS}/{session}/files/{item}/retry"),
        ),
        (Method::DELETE, format!("{SESSIONS}/{session}/files/{item}")),
    ] {
        let no_csrf = stack
            .send(with_peer(
                Request::builder()
                    .method(method.clone())
                    .uri(&path)
                    .header(ORIGIN, BASE_URL)
                    .header(CONTENT_TYPE, "application/json")
                    .header(
                        COOKIE,
                        format!(
                            "palmr_session={}; palmr_csrf={}",
                            alice.creds.session, alice.creds.csrf
                        ),
                    )
                    .body(Body::from(
                        request_body(None, &[sized("x", "x.bin", 1)]).to_string(),
                    ))
                    .unwrap(),
                HOST_A,
            ))
            .await;
        assert_eq!(no_csrf.status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert_eq!(stack.dump().await, dump, "a refused call changes nothing");
    stack.stop().await;
}

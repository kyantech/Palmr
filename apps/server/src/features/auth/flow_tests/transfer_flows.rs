use futures_util::future::join_all;

use super::deletion_tests::World;
use super::folders::SEEDED_AT;
use super::folders::{Member, FOLDERS, HOST_A, HOST_B};
use super::profile::assert_code;
use super::transfers::{
    pathed, request_body, request_in, sized, unsized_file, KEY_A, KEY_B, KEY_C, SESSIONS,
};
use super::*;
use crate::features::transfers::model::SESSION_TTL;
use crate::infra::jobs::JobKind;

const INVALID: &str = "TRANSFER_SESSION_STATE_INVALID";

struct Scenario {
    session: String,
    items: Vec<String>,
}

impl Stack {
    async fn scenario(&self, member: &Member, state: &str, n: u32) -> Scenario {
        let created = self
            .opened(
                member,
                &[
                    sized(&format!("a{n}"), &format!("a{n}.bin"), 10),
                    sized(&format!("b{n}"), &format!("b{n}.bin"), 20),
                ],
            )
            .await;
        let session = created["id"].as_str().unwrap().to_owned();
        let items = self.item_ids(&session).await;
        match state {
            "created" => {}
            "uploading" => {
                self.set_item_state(&items[0], "uploading").await;
                self.seed_tus(member.id, &items[0], 10, 0, "in_progress")
                    .await;
                self.set_session_state(&session, "uploading").await;
            }
            "finalizing" => {
                self.set_item_state(&items[0], "finalizing").await;
                self.set_session_state(&session, "finalizing").await;
            }
            "failed" => {
                self.set_item_state(&items[0], "failed").await;
                self.seed_tus(member.id, &items[0], 10, 4, "in_progress")
                    .await;
                self.set_item_state(&items[1], "uploading").await;
                self.seed_tus(member.id, &items[1], 20, 0, "in_progress")
                    .await;
                self.set_session_state(&session, "failed").await;
            }
            "completed" => {
                self.seed_completed_item(member.id, &items[0], n, 10).await;
                self.set_item_state(&items[1], "canceled").await;
                self.execute(&format!(
                    "UPDATE quota_reservations
                        SET state = 'committed', committed_bytes = 10, settled_at = '{SEEDED_AT}'
                      WHERE transfer_session_id = '{session}';
                     UPDATE transfer_sessions
                        SET state = 'completed', completed_at = '{SEEDED_AT}'
                      WHERE id = '{session}'"
                ))
                .await;
            }
            "canceled" => {
                let canceled = self.cancel_session(member, &session).await;
                assert_eq!(canceled.status, StatusCode::NO_CONTENT);
            }
            "expired" => {
                self.execute(&format!(
                    "UPDATE transfer_session_files SET state = 'expired' WHERE transfer_session_id = '{session}';
                     UPDATE quota_reservations
                        SET state = 'released', release_reason = 'expired', settled_at = '{SEEDED_AT}'
                      WHERE transfer_session_id = '{session}';
                     UPDATE transfer_sessions
                        SET state = 'expired', completed_at = '{SEEDED_AT}'
                      WHERE id = '{session}'"
                ))
                .await;
            }
            other => panic!("unknown scenario {other}"),
        }
        Scenario { session, items }
    }

    async fn transfer_probe(&self, member: &Member, scenario: &Scenario, probe: usize) -> Fetched {
        match probe {
            0 => self.cancel_session(member, &scenario.session).await,
            1 => self.complete_session(member, &scenario.session).await,
            2 => {
                self.retry_item(member, &scenario.session, &scenario.items[0])
                    .await
            }
            _ => {
                self.cancel_item(member, &scenario.session, &scenario.items[0])
                    .await
            }
        }
    }
}

#[tokio::test]
async fn it_transfer_state_machine_terminal_409() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let ok = StatusCode::OK;
    let no_content = StatusCode::NO_CONTENT;
    let conflict = StatusCode::CONFLICT;
    let matrix: [(&str, [StatusCode; 4]); 7] = [
        ("created", [no_content, conflict, conflict, no_content]),
        ("uploading", [no_content, conflict, ok, no_content]),
        ("finalizing", [no_content, conflict, conflict, no_content]),
        ("failed", [no_content, conflict, ok, no_content]),
        ("completed", [conflict, ok, conflict, conflict]),
        ("canceled", [no_content, conflict, conflict, no_content]),
        ("expired", [conflict, conflict, conflict, conflict]),
    ];
    let mut n = 0;
    for (state, expected) in matrix {
        for (probe, expected_status) in expected.into_iter().enumerate() {
            n += 1;
            let scenario = stack.scenario(&alice, state, n).await;
            let objects = stack.counts().await.objects;
            let fetched = stack.transfer_probe(&alice, &scenario, probe).await;
            assert_eq!(
                fetched.status,
                expected_status,
                "{state} probe {probe}: {}",
                fetched.text()
            );
            if expected_status == conflict {
                assert_eq!(fetched.error_code(), INVALID, "{state} probe {probe}");
            }
            assert_eq!(
                stack.counts().await.objects,
                objects,
                "{state} probe {probe} created storage metadata"
            );
        }
    }

    let terminal = stack.scenario(&alice, "completed", 100).await;
    let again = stack.complete_session(&alice, &terminal.session).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json()["state"], "completed");
    assert_eq!(again.json()["files"][0]["state"], "completed");
    assert!(again.json()["files"][0]["fileId"].is_string());
    assert_eq!(again.json()["files"][1]["fileId"], Value::Null);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_session_expiry_gates_mutations() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    let failed = stack.scenario(&alice, "failed", 1).await;
    let created = stack.scenario(&alice, "created", 2).await;

    clock.advance(SESSION_TTL);
    let alice = stack.relogin(&alice).await;
    let before = stack.dump().await;

    let retry = stack
        .retry_item(&alice, &failed.session, &failed.items[0])
        .await;
    assert_code(&retry, StatusCode::GONE, "TRANSFER_SESSION_EXPIRED");
    let complete = stack.complete_session(&alice, &created.session).await;
    assert_code(&complete, StatusCode::GONE, "TRANSFER_SESSION_EXPIRED");
    assert_eq!(
        stack.dump().await,
        before,
        "an expired call mutates nothing"
    );

    let detail = stack.session_detail(&alice, &created.session).await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(
        detail.json()["state"],
        "created",
        "a read never expires a session"
    );
    let listing = stack.read(SESSIONS, &alice).await;
    assert_eq!(listing.status, StatusCode::OK);
    assert_eq!(stack.dump().await, before);

    let canceled = stack.cancel_session(&alice, &created.session).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.reservation(&created.session).await.0, "released");
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_retry_resumes_only_an_existing_resource_without_readmission() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(1_000)).await;

    let scenario = stack.scenario(&alice, "failed", 1).await;
    let held = stack.reservation(&scenario.session).await;
    stack.set_quota(alice.id, Some(0)).await;
    stack
        .execute(&format!(
            "UPDATE transfer_session_files SET error_code = 'STORAGE_UNAVAILABLE', error_request_id = 'req-1'
              WHERE id = '{}'",
            scenario.items[0]
        ))
        .await;
    let detail = stack.session_detail(&alice, &scenario.session).await.json();
    assert_eq!(detail["files"][0]["state"], "failed");
    assert_eq!(detail["files"][0]["error"]["code"], "STORAGE_UNAVAILABLE");
    assert_eq!(detail["files"][0]["error"]["requestId"], "req-1");
    assert_eq!(detail["files"][0]["uploadedBytes"], 4);

    let retried = stack
        .retry_item(&alice, &scenario.session, &scenario.items[0])
        .await;
    assert_eq!(retried.status, StatusCode::OK, "{}", retried.text());
    let item = retried.json();
    assert_eq!(item["state"], "uploading");
    assert_eq!(item["attempts"], 1);
    assert_eq!(item["error"], Value::Null);
    assert_eq!(
        stack.reservation(&scenario.session).await,
        held,
        "a retry never admits quota again, even for an account now over quota"
    );
    let (state, _, _) = stack.session_row(&scenario.session).await;
    assert_eq!(state, "uploading");
    let again = stack
        .retry_item(&alice, &scenario.session, &scenario.items[0])
        .await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        again.json()["attempts"],
        1,
        "a repeated retry is not a second attempt"
    );

    stack.set_quota(alice.id, Some(1_000)).await;
    let bare = stack.scenario(&alice, "created", 2).await;
    stack.set_item_state(&bare.items[0], "failed").await;
    let without_resource = stack
        .retry_item(&alice, &bare.session, &bare.items[0])
        .await;
    assert_code(&without_resource, StatusCode::CONFLICT, INVALID);
    assert_eq!(
        stack.item_states(&bare.session).await,
        ["failed", "pending"],
        "no fake restart is recorded"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_target_folder_immutable() {
    let world = World::start().await;
    let stack = &world.stack;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let top = world.folder(&alice, "Top", None).await;
    let nested = world.folder(&alice, "Nested", Some(&top)).await;
    let doomed = world.folder(&alice, "Doomed", None).await;
    let doomed_child = world.folder(&alice, "Child", Some(&doomed)).await;
    let bobs = world.folder(&bob, "Bobs", None).await;

    let at_root = stack.opened(&alice, &[sized("r", "r.bin", 1)]).await;
    let root_id = at_root["id"].as_str().unwrap();
    assert_eq!(
        stack.session_row(root_id).await,
        ("created".to_owned(), None, None)
    );

    let in_nested = stack
        .open(&alice, &request_in(&nested, &[sized("n", "n.bin", 1)]))
        .await;
    assert_eq!(
        in_nested.status,
        StatusCode::CREATED,
        "{}",
        in_nested.text()
    );
    let nested_id = in_nested.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        stack.session_row(&nested_id).await,
        ("created".to_owned(), Some(nested.clone()), None)
    );

    let foreign = stack
        .open(&alice, &request_in(&bobs, &[sized("f", "f.bin", 1)]))
        .await;
    assert_code(&foreign, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let missing = stack
        .open(
            &alice,
            &request_in(&stack.fresh_id(), &[sized("m", "m.bin", 1)]),
        )
        .await;
    assert_code(&missing, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");

    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{doomed}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    for target in [&doomed, &doomed_child] {
        let deleting = stack
            .open(&alice, &request_in(target, &[sized("d", "d.bin", 1)]))
            .await;
        assert_code(&deleting, StatusCode::CONFLICT, "FOLDER_DELETING");
    }
    assert_eq!(stack.counts().await.sessions, 2);
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);

    for body in [
        json!({ "targetFolderId": top, "target": { "kind": "my_files", "folderId": top } }),
        json!({ "folderId": top }),
    ] {
        let complete = stack
            .api(
                Method::POST,
                &format!("{SESSIONS}/{nested_id}/complete"),
                &alice,
                Some(&body),
            )
            .await;
        assert_code(&complete, StatusCode::CONFLICT, INVALID);
        let items = stack.item_ids(&nested_id).await;
        let retry = stack
            .api(
                Method::POST,
                &format!("{SESSIONS}/{nested_id}/files/{}/retry", items[0]),
                &alice,
                Some(&body),
            )
            .await;
        assert_code(&retry, StatusCode::CONFLICT, INVALID);
        assert_eq!(
            stack.session_row(&nested_id).await,
            ("created".to_owned(), Some(nested.clone()), None),
            "no request can redirect the session"
        );
    }

    let repo = include_str!("../../transfers/repo.rs");
    let service = include_str!("../../transfers/service.rs");
    for source in [repo, service] {
        let compact: String = source.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            !compact.contains("SET target_folder_id") && !compact.contains(", target_folder_id ="),
            "target_folder_id is written by the insert only"
        );
    }

    let canceled = stack.cancel_session(&alice, &nested_id).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    let cancel_root = stack.cancel_session(&alice, root_id).await;
    assert_eq!(cancel_root.status, StatusCode::NO_CONTENT);
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{top}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    assert_eq!(
        stack.session_row(&nested_id).await,
        ("canceled".to_owned(), None, Some(nested.clone())),
        "a deleted destination is history, not the root"
    );
    assert_eq!(
        stack.session_row(root_id).await,
        ("canceled".to_owned(), None, None),
        "an originally root session keeps (NULL, NULL)"
    );
    let marker_before = stack.dump().await;
    for fetched in [
        stack.complete_session(&alice, &nested_id).await,
        stack
            .retry_item(&alice, &nested_id, &stack.item_ids(&nested_id).await[0])
            .await,
    ] {
        assert_code(&fetched, StatusCode::CONFLICT, INVALID);
    }
    assert_eq!(stack.dump().await, marker_before);
    let detached = stack.session_detail(&alice, &nested_id).await;
    assert_eq!(detached.status, StatusCode::OK);
    assert_eq!(detached.json()["state"], "canceled");
}

#[tokio::test]
async fn it_transfer_target_folder_delete_race_serializes_with_admission() {
    let world = World::start().await;
    let stack = &world.stack;
    let alice = stack.member("alice", HOST_A).await;

    let first = world.folder(&alice, "CreateWins", None).await;
    let created = stack
        .open(&alice, &request_in(&first, &[sized("c", "c.bin", 5)]))
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let session = created.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{first}"))
            .await
            .status,
        StatusCode::NO_CONTENT,
        "the claim after a session create still succeeds"
    );
    assert_eq!(world.run_all(JobKind::FoldersDeleteTree).await, 1);
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM folders WHERE id = '{first}'"
            ))
            .await,
        1,
        "the live session defers the deletion"
    );
    assert_eq!(
        stack.session_row(&session).await,
        ("created".to_owned(), Some(first.clone()), None)
    );

    let second = world.folder(&alice, "DeleteWins", None).await;
    assert_eq!(
        world
            .delete(&alice, &format!("{FOLDERS}/{second}"))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let before = stack.counts().await;
    let refused = stack
        .open(&alice, &request_in(&second, &[pathed("c", "Sub/c.bin", 5)]))
        .await;
    assert_code(&refused, StatusCode::CONFLICT, "FOLDER_DELETING");
    let after = stack.counts().await;
    assert_eq!(
        (
            after.sessions,
            after.items,
            after.reservations,
            after.held,
            after.folders
        ),
        (
            before.sessions,
            before.items,
            before.reservations,
            before.held,
            before.folders
        )
    );

    for round in 0..6 {
        let folder = world.folder(&alice, &format!("Race{round}"), None).await;
        let body = request_in(&folder, &[sized("c", "c.bin", 5)]);
        let path = format!("{FOLDERS}/{folder}");
        let create = stack.open(&alice, &body);
        let delete = world.delete(&alice, &path);
        let (created, deleted) = if round % 2 == 0 {
            futures_util::future::join(create, delete).await
        } else {
            let (deleted, created) = futures_util::future::join(delete, create).await;
            (created, deleted)
        };
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        let sessions = stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM transfer_sessions WHERE target_folder_id = '{folder}'"
            ))
            .await;
        match created.status {
            StatusCode::CREATED => assert_eq!(sessions, 1),
            StatusCode::CONFLICT => {
                assert_eq!(created.error_code(), "FOLDER_DELETING");
                assert_eq!(sessions, 0);
            }
            other => panic!("unexpected create outcome {other}: {}", created.text()),
        }
        world.run_all(JobKind::FoldersDeleteTree).await;
        let remaining = stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM folders WHERE id = '{folder}'"
            ))
            .await;
        assert_eq!(
            remaining, sessions,
            "a created session keeps the folder; a refused create lets it go"
        );
    }
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM quota_reservations WHERE state = 'held'")
            .await,
        stack
            .scalar_i64("SELECT COUNT(*) FROM transfer_sessions WHERE state = 'created'")
            .await,
        "every session has exactly one hold and nothing else does"
    );
}

#[tokio::test]
async fn it_idempotency_replay_returns_original_result() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(1_000)).await;
    let other_folder = stack.make(&alice, "Other", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let folders = stack.counts().await.folders;

    let files = [
        pathed("c1", "Tree/Sub/a.bin", 100),
        pathed("c2", "Tree/b.bin", 50),
        unsized_file("c3", "c.bin"),
    ];
    let body = request_body(None, &files);
    let original = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(original.status, StatusCode::CREATED, "{}", original.text());
    assert!(original.headers.get("idempotency-replayed").is_none());
    let counts = stack.counts().await;
    assert_eq!(
        (counts.sessions, counts.items, counts.reservations),
        (1, 3, 1)
    );
    assert_eq!(counts.held, 150);
    assert_eq!(counts.folders, folders + 2);
    let identities: Vec<(String, String)> = sqlx::query_as(
        "SELECT final_object_id, final_object_key FROM transfer_session_files ORDER BY ordinal",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();

    let replay = stack.open_keyed(&alice, &body, KEY_A).await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.body, original.body, "the exact original body");
    assert_eq!(replay.headers.get("idempotency-replayed").unwrap(), "true");
    let replayed = stack.counts().await;
    assert_eq!(
        (
            replayed.sessions,
            replayed.items,
            replayed.reservations,
            replayed.held,
            replayed.folders
        ),
        (1, 3, 1, counts.held, counts.folders)
    );
    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT final_object_id, final_object_key FROM transfer_session_files ORDER BY ordinal",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(after, identities);

    let variants = [
        request_in(&other_folder, &files),
        request_body(None, &files[..2]),
        request_body(
            None,
            &[
                pathed("c1", "Tree/Sub/renamed.bin", 100),
                files[1].clone(),
                files[2].clone(),
            ],
        ),
        request_body(
            None,
            &[
                pathed("c1", "Tree/Sub/a.bin", 101),
                files[1].clone(),
                files[2].clone(),
            ],
        ),
        request_body(
            None,
            &[
                pathed("c1", "Other/Sub/a.bin", 100),
                files[1].clone(),
                files[2].clone(),
            ],
        ),
    ];
    let dump = stack.dump().await;
    for variant in &variants {
        let conflict = stack.open_keyed(&alice, variant, KEY_A).await;
        assert_code(&conflict, StatusCode::CONFLICT, "IDEMPOTENCY_KEY_CONFLICT");
    }
    assert_eq!(
        stack.dump().await,
        dump,
        "a conflicting replay has no effect"
    );
    assert_eq!(stack.counts().await.folders, counts.folders);

    let parallel_body = request_body(None, &[sized("p1", "p.bin", 40)]);
    let attempts = join_all((0..6).map(|_| stack.open_keyed(&alice, &parallel_body, KEY_B))).await;
    let created: Vec<&Fetched> = attempts
        .iter()
        .filter(|fetched| {
            fetched.status == StatusCode::CREATED
                && fetched.headers.get("idempotency-replayed").is_none()
        })
        .collect();
    assert_eq!(created.len(), 1, "exactly one request creates the session");
    for fetched in &attempts {
        match fetched.status {
            StatusCode::CREATED => assert_eq!(fetched.body, created[0].body),
            StatusCode::CONFLICT => {
                assert_eq!(fetched.error_code(), "IDEMPOTENCY_REQUEST_IN_PROGRESS");
            }
            other => panic!("unexpected outcome {other}: {}", fetched.text()),
        }
    }
    let parallel = stack.counts().await;
    assert_eq!((parallel.sessions, parallel.reservations), (2, 2));
    assert_eq!(parallel.held, counts.held + 40);
    let settled = stack.open_keyed(&alice, &parallel_body, KEY_B).await;
    assert_eq!(settled.status, StatusCode::CREATED);
    assert_eq!(settled.body, created[0].body);
    assert_eq!(stack.counts().await.sessions, 2);

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
        (2, 4, 2)
    );
    assert_eq!(persisted.held, counts.held + 40);
    restarted.stop().await;
}

#[tokio::test]
async fn it_transfer_session_failed_admission_releases_the_idempotency_claim() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    stack.set_quota(alice.id, Some(100)).await;
    let bobs = stack.make(&bob, "Bobs", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let folders = stack.counts().await.folders;

    let over = request_body(None, &[pathed("c1", "Tree/big.bin", 200)]);
    let rejected = stack.open_keyed(&alice, &over, KEY_A).await;
    assert_code(
        &rejected,
        StatusCode::INSUFFICIENT_STORAGE,
        "QUOTA_EXCEEDED",
    );
    assert!(!rejected.error_code().is_empty());
    stack.assert_untouched(folders).await;
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM idempotency_records")
            .await,
        0,
        "the claim is cleared when no effect committed"
    );
    stack.set_quota(alice.id, None).await;
    let corrected = stack.open_keyed(&alice, &over, KEY_A).await;
    assert_eq!(
        corrected.status,
        StatusCode::CREATED,
        "{}",
        corrected.text()
    );
    assert!(corrected.headers.get("idempotency-replayed").is_none());
    assert_eq!(stack.counts().await.sessions, 1);

    let foreign = request_in(&bobs, &[sized("c1", "a.bin", 1)]);
    let missing = stack.open_keyed(&alice, &foreign, KEY_B).await;
    assert_code(&missing, StatusCode::NOT_FOUND, "FOLDER_NOT_FOUND");
    let invalid = request_body(None, &[sized("c1", "..", 1)]);
    let bad = stack.open_keyed(&alice, &invalid, KEY_C).await;
    assert_code(&bad, StatusCode::UNPROCESSABLE_ENTITY, "NAME_INVALID");
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM idempotency_records WHERE state = 'in_progress'")
            .await,
        0
    );
    let fixed = request_body(None, &[sized("c1", "fixed.bin", 1)]);
    let accepted = stack.open_keyed(&alice, &fixed, KEY_C).await;
    assert_eq!(accepted.status, StatusCode::CREATED, "{}", accepted.text());
    stack.stop().await;
}

#[tokio::test]
async fn it_cancel_keeps_completed_items() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(10_000)).await;

    let created = stack
        .opened(
            &alice,
            &[
                sized("c1", "one.bin", 100),
                sized("c2", "two.bin", 200),
                sized("c3", "three.bin", 300),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    assert_eq!(stack.reservation(&session).await.1, 600);

    let file_id = stack.seed_completed_item(alice.id, &items[0], 1, 100).await;
    stack.set_item_state(&items[1], "uploading").await;
    stack
        .seed_tus(alice.id, &items[1], 200, 50, "in_progress")
        .await;
    stack.set_item_state(&items[2], "failed").await;
    stack.set_session_state(&session, "uploading").await;
    assert_eq!(stack.reservation(&session).await.1, 500);
    assert_eq!(stack.used_bytes(alice.id).await, 100);

    let canceled = stack.cancel_session(&alice, &session).await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );
    assert!(canceled.body.is_empty());

    assert_eq!(
        stack.item_states(&session).await,
        ["completed", "canceled", "canceled"]
    );
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT COUNT(*) FROM files WHERE id = '{file_id}'"
            ))
            .await,
        1,
        "the completed file survives"
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
            .await,
        1
    );
    assert_eq!(
        stack.used_bytes(alice.id).await,
        100,
        "committed usage is not adjusted"
    );
    let (state, held, committed, reason) = stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), committed, reason),
        ("committed", Some(100), None),
        "{held}"
    );
    assert_eq!(stack.counts().await.held, 0);
    let (session_state, _, _) = stack.session_row(&session).await;
    assert_eq!(session_state, "canceled");
    assert_eq!(
        stack
            .scalar_i64(&format!(
                "SELECT cancel_requested FROM transfer_sessions WHERE id = '{session}'"
            ))
            .await,
        1
    );
    let tus: String = sqlx::query_scalar("SELECT state FROM tus_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(tus, "terminated");

    let settled = stack.dump().await;
    let repeated = stack.cancel_session(&alice, &session).await;
    assert_eq!(repeated.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.dump().await,
        settled,
        "a repeated cancel changes nothing"
    );

    let detail = stack.session_detail(&alice, &session).await;
    assert_eq!(detail.status, StatusCode::OK);
    let text = detail.text();
    assert_eq!(detail.json()["files"][0]["fileId"], file_id);
    assert_eq!(detail.json()["files"][0]["uploadedBytes"], 100);
    assert_eq!(detail.json()["files"][1]["state"], "canceled");
    assert_eq!(detail.json()["files"][1]["uploadedBytes"], 0);
    assert!(!text.contains("objects/"));
    assert_eq!(detail.json()["reservedBytes"], 0);

    let nothing_done = stack
        .opened(
            &alice,
            &[sized("d1", "d1.bin", 70), sized("d2", "d2.bin", 30)],
        )
        .await;
    let other = nothing_done["id"].as_str().unwrap().to_owned();
    assert_eq!(
        stack.cancel_session(&alice, &other).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        stack.reservation(&other).await,
        (
            "released".to_owned(),
            100,
            None,
            Some("canceled".to_owned())
        )
    );
    assert_eq!(stack.counts().await.held, 0);
    assert_eq!(stack.used_bytes(alice.id).await, 100);
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_cancel_tombstones_a_placed_object_and_marks_protocol_rows() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;

    let created = stack
        .opened(
            &alice,
            &[sized("c1", "one.bin", 100), sized("c2", "two.bin", 200)],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'finalizing', finalize_stage = 'placing' WHERE id = '{}';
             UPDATE transfer_session_files SET state = 'uploading' WHERE id = '{}';
             UPDATE transfer_sessions SET state = 'finalizing' WHERE id = '{session}'",
            items[0], items[1]
        ))
        .await;
    stack
        .seed_multipart(
            alice.id,
            &items[1],
            &[(1, 5_242_880, "uploaded"), (2, 1_000, "planned")],
            "in_progress",
        )
        .await;
    let (object_id, key): (String, String) = sqlx::query_as(
        "SELECT final_object_id, final_object_key FROM transfer_session_files WHERE id = ?1",
    )
    .bind(&items[0])
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();

    let canceled = stack.cancel_session(&alice, &session).await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );

    let object: (String, String, String) =
        sqlx::query_as("SELECT state, object_key, provider FROM storage_objects WHERE id = ?1")
            .bind(&object_id)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(object, ("tombstoned".to_owned(), key, "local".to_owned()));
    let queued: (String, String) = sqlx::query_as(
        "SELECT reason, state FROM file_deletion_queue WHERE storage_object_id = ?1",
    )
    .bind(&object_id)
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        queued,
        ("upload_abandoned".to_owned(), "pending".to_owned())
    );
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 'storage.delete_blob' AND state = 'pending'")
            .await,
        1,
        "the cleanup is an intent; nothing was deleted by the request"
    );
    let multipart: String = sqlx::query_scalar("SELECT state FROM s3_multipart_uploads")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(multipart, "abandoned");
    assert_eq!(
        stack
            .scalar_i64("SELECT COUNT(*) FROM jobs WHERE kind = 's3.abort_abandoned_multipart'")
            .await,
        0,
        "the abort job belongs to the S3 adapter"
    );
    assert_eq!(stack.reservation(&session).await.0, "released");
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_item_cancel_releases_its_share_once() {
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
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;
    assert_eq!(stack.reservation(&session).await.1, 600);

    let first = stack.cancel_item(&alice, &session, &items[1]).await;
    assert_eq!(first.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.reservation(&session).await.1, 400);
    let dump = stack.dump().await;
    let repeated = stack.cancel_item(&alice, &session, &items[1]).await;
    assert_eq!(repeated.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.dump().await,
        dump,
        "a repeated DELETE changes nothing"
    );
    assert_eq!(
        stack.item_states(&session).await,
        ["pending", "canceled", "pending"]
    );

    stack.seed_completed_item(alice.id, &items[0], 1, 100).await;
    let refused = stack.cancel_item(&alice, &session, &items[0]).await;
    assert_code(&refused, StatusCode::CONFLICT, INVALID);
    assert_eq!(stack.item_states(&session).await[0], "completed");
    assert_eq!(stack.reservation(&session).await.1, 300);

    let last = stack.cancel_item(&alice, &session, &items[2]).await;
    assert_eq!(last.status, StatusCode::NO_CONTENT);
    assert_eq!(stack.reservation(&session).await.1, 0);
    let (state, _, _) = stack.session_row(&session).await;
    assert_eq!(state, "created", "a completed item keeps the session open");

    let solo = stack.opened(&alice, &[sized("s1", "solo.bin", 10)]).await;
    let solo_session = solo["id"].as_str().unwrap().to_owned();
    let solo_items = stack.item_ids(&solo_session).await;
    let removed = stack
        .cancel_item(&alice, &solo_session, &solo_items[0])
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    assert_eq!(
        stack.session_row(&solo_session).await.0,
        "canceled",
        "an emptied session is canceled"
    );
    assert_eq!(
        stack.reservation(&solo_session).await,
        ("released".to_owned(), 0, None, Some("canceled".to_owned()))
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_transfer_complete_closes_only_a_settled_session() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.set_quota(alice.id, Some(10_000)).await;

    let created = stack
        .opened(
            &alice,
            &[sized("c1", "one.bin", 100), sized("c2", "two.bin", 200)],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = stack.item_ids(&session).await;

    let pending = stack.complete_session(&alice, &session).await;
    assert_code(&pending, StatusCode::CONFLICT, INVALID);
    assert_eq!(stack.item_states(&session).await, ["pending", "pending"]);

    stack.seed_completed_item(alice.id, &items[0], 1, 100).await;
    stack.set_item_state(&items[1], "failed").await;
    stack.set_session_state(&session, "failed").await;
    let used_before = stack.used_bytes(alice.id).await;
    let closed = stack.complete_session(&alice, &session).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.text());
    assert_eq!(closed.json()["state"], "completed");
    assert_eq!(closed.json()["reservedBytes"], 0);
    assert_eq!(
        stack.reservation(&session).await,
        ("committed".to_owned(), 200, Some(100), None),
        "the settled bytes are the authoritative completed bytes"
    );
    assert_eq!(
        stack.used_bytes(alice.id).await,
        used_before,
        "closing never touches used_bytes"
    );
    assert_eq!(stack.counts().await.held, 0);
    let completed_at: Option<String> =
        sqlx::query_scalar("SELECT completed_at FROM transfer_sessions")
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert!(completed_at.is_some());

    let dump = stack.dump().await;
    let again = stack.complete_session(&alice, &session).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        again.body, closed.body,
        "a repeated close returns the same representation"
    );
    assert_eq!(stack.dump().await, dump);

    let fabricated = stack.opened(&alice, &[sized("f1", "fab.bin", 50)]).await;
    let fabricated_session = fabricated["id"].as_str().unwrap().to_owned();
    let fabricated_items = stack.item_ids(&fabricated_session).await;
    stack
        .execute(&format!(
            "UPDATE transfer_session_files SET state = 'canceled' WHERE id = '{}'",
            fabricated_items[0]
        ))
        .await;
    stack.set_session_state(&fabricated_session, "failed").await;
    let no_content = stack.complete_session(&alice, &fabricated_session).await;
    assert_eq!(no_content.status, StatusCode::OK);
    assert_eq!(stack.reservation(&fabricated_session).await.0, "released");
    assert_eq!(
        stack.counts().await.objects,
        1,
        "closing creates no storage metadata"
    );
    stack.stop().await;
}

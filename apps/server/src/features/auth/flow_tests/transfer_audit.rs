use serde_json::Value;

use super::finalize::Rig;
use super::folders::HOST_B;
use super::transfer_reaper::{context, pass, start_in_session, start_upload, DAY};
use super::transfers::sized;
use super::tus::{TusReq, TUS};
use super::*;
use crate::features::transfers::model::SESSION_TTL;
use crate::features::transfers::reaper;

const ACTIONS: &str = "'TRANSFER_CANCELED', 'TRANSFER_EXPIRED'";
const ALLOWED_KEYS: [&str; 8] = [
    "completed_items_kept",
    "item_id",
    "items_ended",
    "owner_id",
    "provider",
    "scope",
    "session_closed",
    "transfer_session_id",
];

#[derive(Debug, Clone)]
pub(super) struct Row {
    pub(super) id: String,
    pub(super) action: String,
    pub(super) actor_type: String,
    pub(super) actor_user_id: Option<String>,
    pub(super) actor_label: Option<String>,
    pub(super) target_type: Option<String>,
    pub(super) target_id: Option<String>,
    pub(super) result: String,
    pub(super) request_id: Option<String>,
    pub(super) client_ip: Option<String>,
    pub(super) metadata: Value,
}

type RawRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub(super) async fn rows(stack: &Stack, filter: &str) -> Vec<Row> {
    let raw: Vec<RawRow> = sqlx::query_as(&format!(
        "SELECT id, action, actor_type, actor_user_id, actor_label, target_type, target_id, result,
                request_id, client_ip, metadata_json
           FROM audit_events WHERE action IN ({filter}) ORDER BY id"
    ))
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    raw.into_iter()
        .map(|row| Row {
            id: row.0,
            action: row.1,
            actor_type: row.2,
            actor_user_id: row.3,
            actor_label: row.4,
            target_type: row.5,
            target_id: row.6,
            result: row.7,
            request_id: row.8,
            client_ip: row.9,
            metadata: serde_json::from_str(&row.10.unwrap()).unwrap(),
        })
        .collect()
}

pub(super) async fn transfer_rows(stack: &Stack) -> Vec<Row> {
    rows(stack, ACTIONS).await
}

async fn count(stack: &Stack, action: &str) -> i64 {
    stack
        .scalar_i64(&format!(
            "SELECT COUNT(*) FROM audit_events WHERE action = '{action}'"
        ))
        .await
}

fn assert_clean(rows: &[Row], forbidden: &[String]) {
    for row in rows {
        let object = row.metadata.as_object().unwrap();
        for key in object.keys() {
            assert!(ALLOWED_KEYS.contains(&key.as_str()), "unexpected key {key}");
        }
        assert!(row.metadata.to_string().len() < 1_024);
        let everything = format!("{row:?}");
        for needle in forbidden {
            assert!(
                !everything.contains(needle.as_str()),
                "{needle} leaked into {row:?}"
            );
        }
        for banned in [
            "uploads/",
            "objects/",
            "blob",
            "staging",
            "palmr_session",
            "csrf",
        ] {
            assert!(!everything.contains(banned), "{banned} leaked into {row:?}");
        }
    }
}

fn leaks(rig: &Rig) -> Vec<String> {
    vec![
        rig.alice.creds.session.clone(),
        rig.alice.creds.csrf.clone(),
        "a.bin".to_owned(),
        "b.bin".to_owned(),
        "c.bin".to_owned(),
        "upload-metadata".to_owned(),
    ]
}

async fn session_of(rig: &Rig, files: &[Value]) -> (String, Vec<String>) {
    let created = rig.stack.opened(&rig.alice, files).await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    (session, items)
}

#[tokio::test]
async fn it_transfer_cancel_session_writes_one_in_transaction_audit_event() {
    let mut rig = Rig::manual().await;
    let alice = rig.alice.id;
    let (session, items) = session_of(
        &rig,
        &[
            sized("c1", "a.bin", 100),
            sized("c2", "b.bin", 200),
            sized("c3", "c.bin", 300),
        ],
    )
    .await;
    rig.stack
        .seed_completed_item(alice, &items[0], 1, 100)
        .await;
    start_in_session(&rig, &session, &items[1], "b.bin", 200, 50).await;
    assert!(transfer_rows(&rig.stack).await.is_empty());

    let canceled = rig.stack.cancel_session(&rig.alice, &session).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(
        events.len(),
        1,
        "written inside the cancellation transaction, visible without any drain"
    );
    let event = &events[0];
    assert_eq!(event.action, "TRANSFER_CANCELED");
    assert_eq!(event.actor_type, "user");
    assert_eq!(
        event.actor_user_id.as_deref(),
        Some(alice.to_string().as_str())
    );
    assert_eq!(event.actor_label.as_deref(), Some("alice"));
    assert_eq!(event.target_type.as_deref(), Some("session"));
    assert_eq!(event.target_id.as_deref(), Some(session.as_str()));
    assert_eq!(event.result, "success");
    assert!(event.request_id.is_some());
    assert!(event.client_ip.is_some());
    assert_eq!(
        event.metadata,
        serde_json::json!({
            "scope": "session",
            "transfer_session_id": session,
            "provider": "local",
            "items_ended": 2,
            "completed_items_kept": 1,
            "session_closed": true,
        })
    );
    assert_clean(&events, &leaks(&rig));

    for _ in 0..3 {
        let again = rig.stack.cancel_session(&rig.alice, &session).await;
        assert_eq!(again.status, StatusCode::NO_CONTENT);
    }
    rig.stack.flush_audit().await;
    assert_eq!(
        transfer_rows(&rig.stack).await.len(),
        1,
        "DELETE stays idempotent"
    );
    assert_eq!(count(&rig.stack, "FILE_DELETED").await, 0);
    assert_eq!(
        rig.stack.scalar_i64("SELECT COUNT(*) FROM files").await,
        1,
        "the completed file survives"
    );
}

#[tokio::test]
async fn it_transfer_cancel_item_audit_distinguishes_scope_and_ignores_no_ops() {
    let mut rig = Rig::manual().await;
    let bob = rig.stack.member("bob", HOST_B).await;
    let (session, items) = session_of(
        &rig,
        &[sized("c1", "a.bin", 100), sized("c2", "b.bin", 200)],
    )
    .await;
    rig.stack
        .seed_completed_item(rig.alice.id, &items[1], 1, 200)
        .await;

    let foreign = rig.stack.cancel_item(&bob, &session, &items[0]).await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    let completed = rig.stack.cancel_item(&rig.alice, &session, &items[1]).await;
    assert_eq!(completed.status, StatusCode::CONFLICT);
    assert!(
        transfer_rows(&rig.stack).await.is_empty(),
        "failed requests leave no event"
    );

    let first = rig.stack.cancel_item(&rig.alice, &session, &items[0]).await;
    assert_eq!(first.status, StatusCode::NO_CONTENT);
    for _ in 0..2 {
        let again = rig.stack.cancel_item(&rig.alice, &session, &items[0]).await;
        assert_eq!(again.status, StatusCode::NO_CONTENT);
    }
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1, "{events:?}");
    let event = &events[0];
    assert_eq!(event.action, "TRANSFER_CANCELED");
    assert_eq!(event.target_type.as_deref(), Some("session"));
    assert_eq!(event.target_id.as_deref(), Some(session.as_str()));
    assert_eq!(
        event.metadata,
        serde_json::json!({
            "scope": "item",
            "transfer_session_id": session,
            "item_id": items[0],
            "provider": "local",
            "items_ended": 1,
            "completed_items_kept": 1,
            "session_closed": false,
        })
    );
    assert_clean(&events, &leaks(&rig));
    rig.stack.flush_audit().await;
    assert_eq!(count(&rig.stack, "FILE_DELETED").await, 0);
}

#[tokio::test]
async fn it_transfer_cancel_last_item_closes_the_session_in_one_event() {
    let rig = Rig::manual().await;
    let (session, items) = session_of(&rig, &[sized("c1", "a.bin", 100)]).await;
    let response = rig.stack.cancel_item(&rig.alice, &session, &items[0]).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].metadata["scope"], "item");
    assert_eq!(events[0].metadata["session_closed"], true);
    assert_eq!(rig.stack.session_row(&session).await.0, "canceled");
}

#[tokio::test]
async fn it_tus_delete_records_the_owner_cancellation_once() {
    let rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 500, 100).await;
    let delete = || TusReq::new(Method::DELETE, &format!("{TUS}/{}", upload.id), &rig.alice);
    let first = rig.stack.tus_send(delete()).await;
    assert_eq!(first.status, StatusCode::NO_CONTENT, "{}", first.text());
    let second = rig.stack.tus_send(delete()).await;
    assert_eq!(second.status, StatusCode::NO_CONTENT);
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].actor_type, "user");
    assert_eq!(events[0].metadata["scope"], "item");
    assert_eq!(events[0].metadata["item_id"], upload.planned.item.as_str());
    assert!(events[0].request_id.is_some());
    assert_clean(&events, &leaks(&rig));
}

#[tokio::test]
async fn it_transfer_session_expiry_is_audited_once_as_the_system() {
    let mut rig = Rig::manual().await;
    let alice = rig.alice.id;
    let (session, items) = session_of(
        &rig,
        &[sized("c1", "a.bin", 100), sized("c2", "b.bin", 200)],
    )
    .await;
    rig.stack
        .seed_completed_item(alice, &items[0], 1, 100)
        .await;
    start_in_session(&rig, &session, &items[1], "b.bin", 200, 20).await;

    rig.stack.clock.advance(SESSION_TTL);
    let report = pass(&rig).await;
    assert_eq!(report.sessions_expired, 1, "{report:?}");
    rig.stack.flush_audit().await;
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1, "{events:?}");
    let event = &events[0];
    assert_eq!(event.action, "TRANSFER_EXPIRED");
    assert_eq!(
        event.actor_type, "system",
        "never attributed to the uploader"
    );
    assert_eq!(event.actor_user_id, None);
    assert_eq!(event.actor_label, None);
    assert_eq!(event.target_type.as_deref(), Some("session"));
    assert_eq!(event.target_id.as_deref(), Some(session.as_str()));
    assert_eq!(event.result, "success");
    assert_eq!(
        event.metadata,
        serde_json::json!({
            "scope": "session",
            "transfer_session_id": session,
            "provider": "local",
            "items_ended": 1,
            "completed_items_kept": 1,
            "session_closed": true,
            "owner_id": alice.to_string(),
        })
    );
    assert_clean(&events, &leaks(&rig));

    for _ in 0..3 {
        assert_eq!(pass(&rig).await.sessions_expired, 0);
        rig.stack.clock.advance(HOUR_STEP);
        rig.stack.flush_audit().await;
    }
    assert_eq!(
        transfer_rows(&rig.stack).await.len(),
        1,
        "a repeated reaper run adds nothing"
    );
    assert_eq!(count(&rig.stack, "TRANSFER_CANCELED").await, 0);
    assert_eq!(count(&rig.stack, "FILE_DELETED").await, 0);
    assert_eq!(rig.stack.scalar_i64("SELECT COUNT(*) FROM files").await, 1);
}

const HOUR_STEP: Duration = Duration::from_secs(3_600);

#[tokio::test]
async fn it_transfer_upload_expiry_is_audited_per_upload_with_a_tus_target() {
    let mut rig = Rig::manual().await;
    let alice = rig.alice.id;
    let (session, items) = session_of(
        &rig,
        &[sized("c1", "a.bin", 100), sized("c2", "b.bin", 200)],
    )
    .await;
    let idle = start_in_session(&rig, &session, &items[0], "a.bin", 100, 10).await;
    let busy = start_in_session(&rig, &session, &items[1], "b.bin", 200, 10).await;
    rig.stack.clock.advance(DAY - Duration::from_secs(60));
    let renewed = rig
        .stack
        .tus_send(super::tus_patch::patch(&rig.alice, &busy, 10).bytes(&super::tus::pattern(5)))
        .await;
    assert_eq!(renewed.status, StatusCode::NO_CONTENT);
    rig.stack.clock.advance(Duration::from_secs(60));

    let report = pass(&rig).await;
    assert_eq!(report.uploads_expired, 1, "{report:?}");
    rig.stack.flush_audit().await;
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.action, "TRANSFER_EXPIRED");
    assert_eq!(event.actor_type, "system");
    assert_eq!(event.target_type.as_deref(), Some("tus_upload"));
    assert_eq!(event.target_id.as_deref(), Some(idle.as_str()));
    assert_eq!(
        event.metadata,
        serde_json::json!({
            "scope": "upload",
            "transfer_session_id": session,
            "item_id": items[0],
            "provider": "local",
            "items_ended": 1,
            "completed_items_kept": 0,
            "session_closed": false,
            "owner_id": alice.to_string(),
        })
    );
    assert_clean(&events, &leaks(&rig));

    assert_eq!(pass(&rig).await.uploads_expired, 0);
    rig.stack.flush_audit().await;
    assert_eq!(transfer_rows(&rig.stack).await.len(), 1);

    rig.stack.clock.advance(DAY);
    assert_eq!(pass(&rig).await.uploads_expired, 1);
    rig.stack.flush_audit().await;
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].target_id.as_deref(), Some(busy.as_str()));
    assert_eq!(events[1].metadata["session_closed"], true);
    assert_eq!(events[1].metadata["items_ended"], 1);
}

#[tokio::test]
async fn it_transfer_retry_cleanup_and_noop_passes_emit_no_audit() {
    let mut rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 500, 100).await;
    rig.stack.clock.advance(DAY);
    rig.staging
        .fail_remove
        .store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..3 {
        pass(&rig).await;
        rig.stack.flush_audit().await;
    }
    rig.staging
        .fail_remove
        .store(false, std::sync::atomic::Ordering::SeqCst);
    pass(&rig).await;
    rig.stack.clock.advance(40 * DAY);
    pass(&rig).await;
    rig.stack.flush_audit().await;
    assert_eq!(
        transfer_rows(&rig.stack).await.len(),
        1,
        "one lifecycle transition, not one event per cleanup attempt: {}",
        upload.id
    );
}

#[tokio::test]
async fn it_transfer_cancel_wins_or_expiry_wins_never_both() {
    let mut rig = Rig::manual().await;
    let canceled = start_upload(&rig, "a.bin", 500, 100).await;
    let expiring = start_upload(&rig, "b.bin", 500, 100).await;
    let cancel_first = rig
        .stack
        .cancel_session(&rig.alice, &canceled.planned.session)
        .await;
    assert_eq!(cancel_first.status, StatusCode::NO_CONTENT);

    rig.stack.clock.advance(SESSION_TTL);
    let after = pass(&rig).await;
    assert_eq!(
        after.sessions_expired, 1,
        "only the live session: {after:?}"
    );
    assert_eq!(
        reaper::expire_upload(&context(&rig), &canceled.id)
            .await
            .unwrap(),
        reaper::UploadOutcome::Gone
    );
    let alice = rig.stack.relogin(&rig.alice).await;
    let late = rig
        .stack
        .cancel_session(&alice, &expiring.planned.session)
        .await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.text());
    rig.stack.flush_audit().await;
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].action, "TRANSFER_CANCELED");
    assert_eq!(
        events[0].target_id.as_deref(),
        Some(canceled.planned.session.as_str())
    );
    assert_eq!(events[1].action, "TRANSFER_EXPIRED");
    assert_eq!(
        events[1].target_id.as_deref(),
        Some(expiring.planned.session.as_str())
    );
}

#[tokio::test]
async fn it_transfer_concurrent_cancel_and_reaper_record_exactly_one_outcome() {
    let mut rig = Rig::manual().await;
    let upload = start_upload(&rig, "a.bin", 500, 100).await;
    rig.stack.clock.advance(SESSION_TTL);
    let alice = rig.stack.relogin(&rig.alice).await;
    let cancel = rig.stack.cancel_session(&alice, &upload.planned.session);
    let reap = pass(&rig);
    let (canceled, _report) = futures_util::future::join(cancel, reap).await;
    assert!(
        matches!(
            canceled.status,
            StatusCode::NO_CONTENT | StatusCode::CONFLICT
        ),
        "{}",
        canceled.text()
    );
    rig.stack.flush_audit().await;
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(
        events.len(),
        1,
        "exactly one winner is recorded: {events:?}"
    );
    match events[0].action.as_str() {
        "TRANSFER_CANCELED" => assert_eq!(canceled.status, StatusCode::NO_CONTENT),
        "TRANSFER_EXPIRED" => assert_eq!(canceled.status, StatusCode::CONFLICT),
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn it_transfer_cancel_keeps_the_finalized_upload_event_and_deletes_nothing() {
    let mut rig = Rig::new().await;
    let (session, items) = session_of(
        &rig,
        &[sized("c1", "a.bin", 100), sized("c2", "b.bin", 200)],
    )
    .await;
    start_in_session(&rig, &session, &items[0], "a.bin", 100, 100).await;
    rig.settle().await;
    start_in_session(&rig, &session, &items[1], "b.bin", 200, 40).await;
    rig.stack.flush_audit().await;
    assert_eq!(count(&rig.stack, "FILE_UPLOADED").await, 1);
    let uploaded = rows(&rig.stack, "'FILE_UPLOADED'").await;

    let canceled = rig.stack.cancel_session(&rig.alice, &session).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    rig.stack.flush_audit().await;
    assert_eq!(
        rows(&rig.stack, "'FILE_UPLOADED'").await[0].id,
        uploaded[0].id
    );
    assert_eq!(count(&rig.stack, "FILE_UPLOADED").await, 1);
    assert_eq!(count(&rig.stack, "FILE_DELETED").await, 0);
    assert_eq!(rig.file_rows().await.len(), 1);
    let events = transfer_rows(&rig.stack).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].metadata["completed_items_kept"], 1);
    assert_eq!(events[0].metadata["items_ended"], 1);
}

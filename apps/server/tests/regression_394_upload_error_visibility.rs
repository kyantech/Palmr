pub mod support;

use std::time::Duration;

use anyhow::{ensure, Result};
use palmr_server::{Classification, ErrorCode, RetryClass};

use support::transfer_errors::{blob_of, error_of, failure, failure_of, World};

const KIB: usize = 1024;

struct Snapshot {
    state: String,
    error_code: Option<String>,
    request_id: Option<String>,
    held: String,
    used: i64,
}

async fn snapshot(world: &World, session: &str, item: &str) -> Result<Snapshot> {
    let (error_code, request_id) = world.db_error("transfer_session_files", item).await?;
    Ok(Snapshot {
        state: world
            .db
            .scalar_string(&format!(
                "SELECT state FROM transfer_session_files WHERE id = '{item}'"
            ))
            .await?,
        error_code,
        request_id,
        held: world
            .db
            .scalar_string(&format!(
                "SELECT COALESCE(GROUP_CONCAT(state || ':' || reserved_bytes), '')
                   FROM quota_reservations WHERE transfer_session_id = '{session}'"
            ))
            .await?,
        used: world
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_394_upload_error_visibility() -> Result<()> {
    let world = World::start("regression_394_upload_error_visibility").await?;

    let too_large = world
        .planned_upload("too-large.bin", (64 * KIB) as u64)
        .await?;
    let refused = world.patch(&too_large, 0, vec![7_u8; 64 * KIB + 1]).await?;
    failure(&refused)?.expect(413, "FILE_TOO_LARGE")?;
    assert_eq!(
        world.item_view(&too_large.session, &too_large.item).await?["error"],
        serde_json::Value::Null,
        "a request refused before its first byte has nothing to record"
    );
    let overrun = world
        .streamed_patch(&too_large, 0, &vec![7_u8; 64 * KIB + 1])
        .await?;
    let too_large_failure = failure_of(&overrun)?;
    too_large_failure.expect(413, "FILE_TOO_LARGE")?;
    let blob_a = std::fs::metadata(blob_of(&world, &too_large))?.len();

    world
        .db
        .execute("UPDATE users SET quota_override_mode = 'bytes', quota_bytes = 131072")
        .await?;
    let (quota_session, quota_items) = world.plan_files(&[("quota.bin", None)]).await?;
    let quota = world
        .create_upload("quota.bin", &quota_session, &quota_items[0], None)
        .await?;
    let exceeded = world.patch(&quota, 0, vec![9_u8; 96 * KIB]).await?;
    let quota_failure = failure(&exceeded)?;
    quota_failure.expect(507, "QUOTA_EXCEEDED")?;
    let blob_b = std::fs::metadata(blob_of(&world, &quota))?.len();

    let expiring = world
        .planned_upload("expiring.bin", (32 * KIB) as u64)
        .await?;
    world
        .db
        .execute(&format!(
            "UPDATE transfer_sessions SET expires_at = '2026-01-01T00:30:00.000Z' WHERE id = '{}'",
            expiring.session
        ))
        .await?;
    world.app.clock().advance(Duration::from_secs(60 * 60));
    let expired = world.retry(&expiring.session, &expiring.item).await?;
    let expiry_failure = failure(&expired)?;
    expiry_failure.expect(410, "TRANSFER_SESSION_EXPIRED")?;

    let before_a = snapshot(&world, &too_large.session, &too_large.item).await?;
    let before_b = snapshot(&world, &quota.session, &quota.item).await?;
    assert_eq!(before_a.state, "failed");
    assert_eq!(before_a.error_code.as_deref(), Some("FILE_TOO_LARGE"));
    assert_eq!(
        before_a.request_id.as_deref(),
        Some(too_large_failure.header_id.as_str()),
        "the persisted correlation id is the failing request's X-Request-Id"
    );
    assert_eq!(before_b.state, "failed");
    assert_eq!(before_b.error_code.as_deref(), Some("QUOTA_EXCEEDED"));
    assert_eq!(
        before_b.request_id.as_deref(),
        Some(quota_failure.header_id.as_str())
    );
    let (session_code, session_request) = world
        .db_error("transfer_sessions", &expiring.session)
        .await?;
    assert_eq!(session_code.as_deref(), Some("TRANSFER_SESSION_EXPIRED"));
    assert_eq!(
        session_request.as_deref(),
        Some(expiry_failure.header_id.as_str())
    );
    assert_eq!(
        world
            .db
            .scalar_string(&format!(
                "SELECT state FROM transfer_sessions WHERE id = '{}'",
                expiring.session
            ))
            .await?,
        "uploading",
        "an observed expiry is recorded, never acted on by the request"
    );
    assert_eq!(before_b.used, 0);

    let world = world.restarted().await?;

    let reloaded_a = world.session(&too_large.session).await?;
    let item_a = world.item_view(&too_large.session, &too_large.item).await?;
    assert_eq!(
        error_of(&item_a),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(too_large_failure.header_id.clone())
        )
    );
    assert_eq!(
        error_of(&reloaded_a),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(too_large_failure.header_id.clone())
        ),
        "the failed item's classification also names the session"
    );
    let item_b = world.item_view(&quota.session, &quota.item).await?;
    assert_eq!(
        error_of(&item_b),
        (
            Some("QUOTA_EXCEEDED".to_owned()),
            Some(quota_failure.header_id.clone())
        )
    );
    let reloaded_c = world.session(&expiring.session).await?;
    assert_eq!(
        error_of(&reloaded_c),
        (
            Some("TRANSFER_SESSION_EXPIRED".to_owned()),
            Some(expiry_failure.header_id.clone())
        )
    );

    let after_a = snapshot(&world, &too_large.session, &too_large.item).await?;
    let after_b = snapshot(&world, &quota.session, &quota.item).await?;
    for (before, after) in [(&before_a, &after_a), (&before_b, &after_b)] {
        assert_eq!(before.state, after.state);
        assert_eq!(before.error_code, after.error_code);
        assert_eq!(before.request_id, after.request_id);
        assert_eq!(
            before.held, after.held,
            "the reservation survives the restart"
        );
        assert_eq!(
            before.used, after.used,
            "no usage is committed by a failure"
        );
    }
    ensure!(
        std::fs::metadata(blob_of(&world, &too_large))?.len() == blob_a
            && std::fs::metadata(blob_of(&world, &quota))?.len() == blob_b,
        "a restart deletes no staged bytes"
    );
    assert_eq!(world.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);

    for (code, class) in [
        (ErrorCode::FileTooLarge, RetryClass::Fatal),
        (ErrorCode::QuotaExceeded, RetryClass::Fatal),
        (ErrorCode::TransferSessionExpired, RetryClass::Terminal),
    ] {
        let classification = Classification::of(code);
        assert_eq!(classification.class(), class, "{code:?}");
        assert!(
            !classification.class().backs_off(),
            "{code:?} never backs off"
        );
        assert_ne!(classification.code, ErrorCode::InternalError);
    }
    let retried = world.retry(&expiring.session, &expiring.item).await?;
    let late = failure(&retried)?;
    assert!(
        late.status == 410 || late.status == 409,
        "an expired or reaped session is never resurrected: {} {}",
        late.status,
        late.code
    );
    world.app.shutdown().await;
    Ok(())
}

pub mod support;

use std::os::unix::fs::PermissionsExt;

use anyhow::{ensure, Result};
use palmr_server::{Classification, ErrorCode, RetryClass};
use serde_json::Value;
use sha2::{Digest, Sha256};

use support::transfer_errors::{error_of, failure, failure_of, World};
use support::tus::{digest_header, wait_until, SESSIONS};

const KIB: usize = 1024;

fn clean(world: &World, body: &str) -> Result<()> {
    let data_dir = world.data_dir.display().to_string();
    for secret in [
        world.creds.session.as_str(),
        world.creds.csrf.as_str(),
        data_dir.as_str(),
        "objects/",
        "palmr.db",
        "X-Amz",
        "SQLITE",
    ] {
        ensure!(!body.contains(secret), "{secret:?} leaked into {body}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_transfer_error_negotiation_errors_never_fail_an_item() -> Result<()> {
    let world = World::start("it_transfer_error_negotiation").await?;
    let upload = world
        .planned_upload("negotiate.bin", (64 * KIB) as u64)
        .await?;

    let stale = world.patch(&upload, 4_096, vec![1_u8; 16]).await?;
    clean(&world, &stale.body)?;
    let stale_failure = failure(&stale)?;
    stale_failure.expect(409, "UPLOAD_OFFSET_MISMATCH")?;
    assert_eq!(
        stale.json()?["error"]["details"]["expectedOffset"],
        Value::from(0)
    );

    let holder = world
        .open(
            &support::tus::Upload {
                id: upload.id.clone(),
            },
            0,
            None,
        )
        .await?;
    wait_until("the streaming request to hold the lease", || async {
        world
            .locked(&support::tus::Upload {
                id: upload.id.clone(),
            })
            .await
    })
    .await?;
    let locked = world.patch(&upload, 0, vec![1_u8; 16]).await?;
    failure(&locked)?.expect(423, "UPLOAD_LOCKED")?;
    drop(holder);
    wait_until("the dropped request to release its lease", || async {
        Ok(!world
            .locked(&support::tus::Upload {
                id: upload.id.clone(),
            })
            .await?)
    })
    .await?;

    let bad_checksum = digest_header(&Sha256::digest(b"not the stream"));
    let mut checked = world
        .open(
            &support::tus::Upload {
                id: upload.id.clone(),
            },
            0,
            Some(&bad_checksum),
        )
        .await?;
    checked.frame(&vec![3_u8; 4 * KIB]).await?;
    checked.finish().await?;
    let mismatch = checked.response().await?;
    failure_of(&mismatch)?.expect(460, "UPLOAD_CHECKSUM_MISMATCH")?;
    clean(&world, &mismatch.body)?;

    for code in [
        ErrorCode::UploadOffsetMismatch,
        ErrorCode::UploadLocked,
        ErrorCode::UploadChecksumMismatch,
    ] {
        let classification = Classification::of(code);
        assert!(
            classification.is_protocol_negotiation() || classification.class().reconciles_first()
        );
        assert!(!classification.class().is_final(), "{code:?}");
    }
    let view = world.item_view(&upload.session, &upload.item).await?;
    assert_eq!(error_of(&view), (None, None), "{view}");
    assert_eq!(
        error_of(&world.session(&upload.session).await?),
        (None, None)
    );
    assert_eq!(
        world
            .db_error("transfer_session_files", &upload.item)
            .await?,
        (None, None)
    );
    assert_eq!(
        world
            .offset(&support::tus::Upload {
                id: upload.id.clone()
            })
            .await?,
        0
    );

    world
        .db
        .execute(&format!(
            "UPDATE tus_uploads SET expires_at = '2020-01-01T00:00:00.000Z' WHERE id = '{}'",
            upload.id
        ))
        .await?;
    let gone = world.patch(&upload, 0, vec![1_u8; 16]).await?;
    clean(&world, &gone.body)?;
    failure(&gone)?.expect(410, "UPLOAD_SESSION_EXPIRED")?;
    assert_eq!(
        Classification::of(ErrorCode::UploadSessionExpired).class(),
        RetryClass::Terminal
    );
    let again = world.patch(&upload, 0, vec![1_u8; 16]).await?;
    failure(&again)?.expect(410, "UPLOAD_SESSION_EXPIRED")?;
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_transfer_error_sibling_failures_aggregate_survive_restart_and_retry() -> Result<()> {
    let world = World::start("it_transfer_error_siblings").await?;
    let size = (32 * KIB) as u64;
    let (session, items) = world
        .plan_files(&[
            ("a.bin", Some(size)),
            ("b.bin", Some(size)),
            ("d.bin", Some(size)),
        ])
        .await?;
    let a = world
        .create_upload("a.bin", &session, &items[0], Some(size))
        .await?;
    let b = world
        .create_upload("b.bin", &session, &items[1], Some(size))
        .await?;
    let d = world
        .create_upload("d.bin", &session, &items[2], Some(size))
        .await?;
    let body = vec![5_u8; 32 * KIB + 1];

    let first = failure_of(&world.streamed_patch(&a, 0, &body).await?)?;
    first.expect(413, "FILE_TOO_LARGE")?;
    let second = failure_of(&world.streamed_patch(&d, 0, &body).await?)?;
    second.expect(413, "FILE_TOO_LARGE")?;
    assert_ne!(first.header_id, second.header_id);

    let live = world.session(&session).await?;
    assert_eq!(
        live["state"], "uploading",
        "failed items do not strand the live sibling"
    );
    assert_eq!(
        error_of(&live),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(second.header_id.clone())
        ),
        "the session names its most recent failed item"
    );
    assert_eq!(
        error_of(&world.item_view(&session, &items[0]).await?),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(first.header_id.clone())
        )
    );
    assert_eq!(
        error_of(&world.item_view(&session, &items[1]).await?),
        (None, None)
    );

    let bob = world.http.invite_and_accept(&world.creds, "bob").await?;
    for (method, path) in [
        (reqwest::Method::GET, format!("{SESSIONS}/{session}")),
        (
            reqwest::Method::POST,
            format!("{SESSIONS}/{session}/files/{}/retry", items[0]),
        ),
    ] {
        let foreign = world.http.send(method, &path, Some(&bob), None).await?;
        failure(&foreign)?.expect(404, "TRANSFER_SESSION_NOT_FOUND")?;
        clean(&world, &foreign.body)?;
        assert!(
            !foreign.body.contains("FILE_TOO_LARGE") && !foreign.body.contains(&first.header_id),
            "another user learns nothing about the failure: {}",
            foreign.body
        );
    }

    let accepted = world.patch(&b, 0, vec![8_u8; 32 * KIB]).await?;
    assert_eq!(accepted.status.as_u16(), 204, "{}", accepted.body);
    world.wait_item(&items[1], "completed").await?;
    let settled = world.session(&session).await?;
    assert_eq!(
        settled["state"], "failed",
        "completed + failed settles as failed"
    );
    assert_eq!(
        error_of(&settled),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(second.header_id.clone())
        ),
        "a sibling that succeeds erases no failure"
    );
    assert_eq!(world.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 1);

    let world = world.restarted().await?;
    let reloaded = world.session(&session).await?;
    assert_eq!(reloaded["state"], "failed");
    assert_eq!(error_of(&reloaded), error_of(&settled));
    for (item, request) in [(&items[0], &first), (&items[2], &second)] {
        assert_eq!(
            error_of(&world.item_view(&session, item).await?),
            (
                Some("FILE_TOO_LARGE".to_owned()),
                Some(request.header_id.clone())
            ),
            "{item} keeps its own failure across a restart"
        );
    }

    let retried = world.retry(&session, &items[0]).await?;
    assert_eq!(retried.status.as_u16(), 200, "{}", retried.body);
    assert_eq!(
        retried.json()?["error"],
        Value::Null,
        "a retried item exposes no stale failure"
    );
    world.wait_item(&items[0], "completed").await?;
    let after_first_retry = world.session(&session).await?;
    assert_eq!(
        after_first_retry["state"], "failed",
        "the other failure still holds the session"
    );
    assert_eq!(
        error_of(&after_first_retry),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(second.header_id.clone())
        ),
        "the aggregate names the failure that is still active, not the cleared one"
    );
    assert_eq!(
        error_of(&world.item_view(&session, &items[2]).await?),
        (
            Some("FILE_TOO_LARGE".to_owned()),
            Some(second.header_id.clone())
        )
    );

    let retried = world.retry(&session, &items[2]).await?;
    assert_eq!(retried.status.as_u16(), 200, "{}", retried.body);
    world.wait_item(&items[2], "completed").await?;
    wait_until(
        "the session to close once nothing is live or failed",
        || async {
            Ok(world
                .db
                .scalar_string(&format!(
                    "SELECT state FROM transfer_sessions WHERE id = '{session}'"
                ))
                .await?
                == "completed")
        },
    )
    .await?;
    let done = world.session(&session).await?;
    assert_eq!(error_of(&done), (None, None), "{done}");
    assert_eq!(world.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 3);
    assert_eq!(
        world
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        i64::try_from(3 * size)?
    );
    assert_eq!(
        world
            .db
            .scalar_string(&format!(
                "SELECT state FROM quota_reservations WHERE transfer_session_id = '{session}'"
            ))
            .await?,
        "committed"
    );
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_transfer_error_local_permission_failure_is_durable_and_retry_recovers() -> Result<()> {
    let world = World::start("it_transfer_error_permission").await?;
    let objects = world.data_dir.join("storage").join("objects");
    std::fs::create_dir_all(&objects)?;
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o555))?;
    ensure!(
        std::fs::create_dir(objects.join("probe")).is_err(),
        "the test needs an unprivileged user so that a read-only directory denies writes"
    );

    let size = (24 * KIB) as u64;
    let upload = world.planned_upload("denied.bin", size).await?;
    let accepted = world.patch(&upload, 0, vec![2_u8; 24 * KIB]).await?;
    assert_eq!(accepted.status.as_u16(), 204, "{}", accepted.body);
    let request_id = accepted
        .headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .expect("the PATCH response carries X-Request-Id");
    world.wait_item(&upload.item, "failed").await?;

    let view = world.item_view(&upload.session, &upload.item).await?;
    assert_eq!(
        error_of(&view),
        (
            Some("STORAGE_PERMISSION_DENIED".to_owned()),
            Some(request_id.clone())
        ),
        "a failure after the response stays correlated to the request that triggered it"
    );
    clean(&world, &view.to_string())?;
    let classification = Classification::of(ErrorCode::StoragePermissionDenied);
    assert_eq!(classification.class(), RetryClass::Fatal);
    assert!(
        !classification.class().backs_off(),
        "a persisted EACCES failure decodes as fatal from its code alone"
    );
    assert_eq!(world.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);
    assert_eq!(
        world
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        0
    );
    assert!(
        std::fs::metadata(
            world
                .data_dir
                .join("uploads")
                .join(upload.id.replace('-', ""))
                .join("blob")
        )?
        .len()
            == size,
        "the staged bytes are retained so a retry costs no retransmission"
    );

    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o755))?;
    let world = world.restarted().await?;
    let reloaded = world.item_view(&upload.session, &upload.item).await?;
    assert_eq!(
        error_of(&reloaded),
        (
            Some("STORAGE_PERMISSION_DENIED".to_owned()),
            Some(request_id)
        ),
        "the permission failure survives a real restart with the same code and request id"
    );
    let decoded = ErrorCode::ALL
        .iter()
        .copied()
        .find(|code| Some(code.as_str()) == reloaded["error"]["code"].as_str())
        .expect("the persisted code is in the catalogue");
    assert_eq!(Classification::of(decoded).class(), RetryClass::Fatal);
    assert_eq!(world.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 0);

    let retried = world.retry(&upload.session, &upload.item).await?;
    assert_eq!(retried.status.as_u16(), 200, "{}", retried.body);
    assert_eq!(retried.json()?["error"], Value::Null);
    world.wait_item(&upload.item, "completed").await?;
    assert_eq!(world.db.scalar_i64("SELECT COUNT(*) FROM files").await?, 1);
    assert_eq!(
        world
            .db
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await?,
        i64::try_from(size)?
    );
    assert_eq!(
        world
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects")
            .await?,
        1,
        "the retry stores the object exactly once"
    );
    world.app.shutdown().await;
    Ok(())
}

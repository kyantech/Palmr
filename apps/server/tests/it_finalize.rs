pub mod support;

use std::time::Duration;

use anyhow::Result;
use reqwest::{Method, StatusCode};

use support::client::Http;
use support::tus::{generated_digest, send_range, wait_until, TusWorld, Upload, MIB};
use support::TestApplication;

struct World {
    app: TestApplication,
    tus: TusWorld,
}

impl std::ops::Deref for World {
    type Target = TusWorld;

    fn deref(&self) -> &TusWorld {
        &self.tus
    }
}

impl World {
    async fn start(name: &str) -> Result<Self> {
        let app = TestApplication::start(name).await?;
        let http = Http::new(app.url("/")?)?;
        let creds = http.setup_admin().await?;
        let tus = TusWorld::new(app.url("/")?, app.data_dir().to_path_buf(), http, creds);
        Ok(Self { app, tus })
    }

    async fn restarted(self) -> Result<Self> {
        let Self { app, tus } = self;
        let (_drain, app) = app.restart(Duration::from_secs(10)).await?;
        let http = Http::new(app.url("/")?)?;
        let tus = TusWorld::new(app.url("/")?, tus.data_dir, http, tus.creds);
        Ok(Self { app, tus })
    }

    async fn stream(&self, upload: &Upload, size: u64) -> Result<()> {
        let mut patch = self.open(upload, 0, None).await?;
        send_range(&mut patch, size, 0, size).await?;
        patch.finish().await?;
        let response = patch.response().await?;
        assert_eq!(response.status, 204, "{}", response.body);
        assert_eq!(response.headers["upload-offset"], size.to_string());
        assert!(response.body.is_empty(), "a TUS 204 carries no JSON");
        Ok(())
    }

    async fn counts(&self) -> Result<(i64, i64, i64, i64)> {
        Ok((
            self.db.scalar_i64("SELECT COUNT(*) FROM files").await?,
            self.db
                .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'active'")
                .await?,
            self.db
                .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
                .await?,
            self.db
                .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FILE_UPLOADED'")
                .await?,
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_finalize_generated_multi_checkpoint_upload_becomes_a_real_file() -> Result<()> {
    let world = World::start("it_finalize_generated_upload").await?;
    let size = 24 * MIB + 12_345;
    let expected = generated_digest(size)?;
    let upload = world.upload("generated.bin", size).await?;

    world.stream(&upload, size).await?;
    world.wait_finalized(&upload).await?;

    let (staged, digest) = world.blob_digest(&upload).await?;
    assert_eq!((staged, digest), (size, expected), "exact SHA-256 and size");
    assert!(
        !world.blob(&upload).exists(),
        "the staging blob moved to its final key"
    );
    let key = world.final_path(&upload).await?;
    assert!(key.starts_with(world.data_dir.join("storage/objects")));
    assert_eq!(
        world
            .db
            .scalar_i64(&format!(
                "SELECT f.size_bytes FROM files f JOIN storage_objects o ON o.id = f.storage_object_id
                  WHERE o.object_key = 'objects/{}'",
                key.strip_prefix(world.data_dir.join("storage/objects"))?.display()
            ))
            .await?,
        i64::try_from(size)?
    );
    assert_eq!(world.counts().await?, (1, 1, i64::try_from(size)?, 1));
    assert_eq!(
        world.head(&upload).await?,
        size,
        "HEAD keeps reporting the full offset after finalization"
    );

    let file_id = world.db.scalar_string("SELECT id FROM files").await?;
    let fetched = world
        .http
        .send(
            Method::GET,
            &format!("/api/v1/files/{file_id}"),
            Some(&world.creds),
            None,
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert_eq!(fetched["sizeBytes"], size);
    assert_eq!(fetched["name"], "generated.bin");
    assert_eq!(fetched["contentType"], "application/octet-stream");

    let session = world
        .db
        .scalar_string("SELECT transfer_session_id FROM transfer_session_files")
        .await?;
    let detail = world
        .http
        .send(
            Method::GET,
            &format!("/api/v1/transfers/sessions/{session}"),
            Some(&world.creds),
            None,
        )
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    assert_eq!(detail["state"], "completed");
    assert_eq!(detail["files"][0]["fileId"], file_id.as_str());
    let leaked = detail.to_string();
    let key = world
        .db
        .scalar_string("SELECT final_object_key FROM transfer_session_files")
        .await?;
    let object_id = world
        .db
        .scalar_string("SELECT final_object_id FROM transfer_session_files")
        .await?;
    assert!(!leaked.contains("objects/") && !leaked.contains("staging"));
    assert!(!leaked.contains(&key) && !leaked.contains(&object_id));
    assert!(!leaked.contains(&upload.id.replace('-', "")));
    world.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_finalize_restart_preserves_identity_and_repeats_nothing() -> Result<()> {
    let world = World::start("it_finalize_restart").await?;
    let size = 9 * MIB + 7;
    let upload = world.upload("kept.bin", size).await?;
    world.stream(&upload, size).await?;
    world.wait_finalized(&upload).await?;
    let before = world.counts().await?;
    let file = world.db.scalar_string("SELECT id FROM files").await?;
    let object = world
        .db
        .scalar_string("SELECT storage_object_id FROM files")
        .await?;

    let world = world.restarted().await?;
    wait_until("startup to settle", || async {
        Ok(world.head(&upload).await? == size)
    })
    .await?;
    let again = world.restarted().await?;

    assert_eq!(
        again.counts().await?,
        before,
        "no second file, object, charge or event"
    );
    assert_eq!(again.db.scalar_string("SELECT id FROM files").await?, file);
    assert_eq!(
        again
            .db
            .scalar_string("SELECT storage_object_id FROM files")
            .await?,
        object
    );
    assert_eq!(again.item_state(&upload).await?, "completed");
    again.app.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_finalize_a_graceful_restart_drains_in_flight_finalizations() -> Result<()> {
    let world = World::start("it_finalize_drain").await?;
    let size = 3 * MIB;
    let upload = world.upload("drain.bin", size).await?;
    world.stream(&upload, size).await?;
    let world = world.restarted().await?;
    world.wait_finalized(&upload).await?;
    assert_eq!(world.counts().await?.0, 1);
    assert_eq!(world.counts().await?.1, 1);
    world.app.shutdown().await;
    Ok(())
}

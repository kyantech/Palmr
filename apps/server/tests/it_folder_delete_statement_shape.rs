pub mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use support::client::{Creds, Db, Http, EPOCH};
use support::TestApplication;

const FILES: u32 = 1_300;
const SUBFOLDERS: u32 = 600;
const REAL_BLOBS: u32 = 5;

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<String>>>);

impl Recorded {
    fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn all(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|statement| statement.trim().to_owned())
            .collect()
    }
}

struct SqlRecorder(Recorded);

#[derive(Default)]
struct StatementText(String);

impl Visit for StatementText {
    fn record_str(&mut self, field: &Field, value: &str) {
        if matches!(field.name(), "summary" | "db.statement") {
            self.0.push(' ');
            self.0.push_str(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl<S: Subscriber> Layer<S> for SqlRecorder {
    fn on_event(&self, event: &Event<'_>, _context: LayerContext<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut text = StatementText::default();
        event.record(&mut text);
        self.0
             .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(text.0);
    }
}

struct World {
    app: TestApplication,
    http: Http,
    db: Db,
    admin: Creds,
    owner: String,
    recorded: Recorded,
}

impl World {
    async fn start() -> Result<Self> {
        let recorded = Recorded::default();
        tracing::subscriber::set_global_default(
            Registry::default().with(SqlRecorder(recorded.clone())),
        )
        .context("install the statement recorder")?;
        let app = TestApplication::start("it_folder_delete_statement_shape").await?;
        let http = Http::new(app.url("/")?)?;
        let db = Db::new(app.data_dir());
        let admin = http.setup_admin().await?;
        let owner = db
            .scalar_string("SELECT id FROM users WHERE username = 'ada'")
            .await?;
        Ok(Self {
            app,
            http,
            db,
            admin,
            owner,
            recorded,
        })
    }
}

fn count(statements: &[String], needle: &str) -> usize {
    statements
        .iter()
        .filter(|statement| statement.contains(needle))
        .count()
}

fn blob_path(world: &World, n: u32) -> std::path::PathBuf {
    world
        .app
        .data_dir()
        .join("storage/objects/00/00")
        .join(format!("{:032x}", 9_000_000 + n))
}

#[tokio::test(flavor = "multi_thread")]
async fn it_folder_delete_statement_shape() -> Result<()> {
    let world = World::start().await?;
    let owner = &world.owner;

    std::fs::create_dir_all(world.app.data_dir().join("storage/objects/00/00"))?;
    for n in 1..=REAL_BLOBS {
        std::fs::write(blob_path(&world, n), format!("blob-{n}"))?;
    }

    world
        .db
        .execute(&format!(
            "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             VALUES ('0192f3a1-aaaa-7000-8000-000000000001', '{owner}', NULL, 'Big', 'big', 0, '{EPOCH}', '{EPOCH}'),
                    ('0192f3a1-aaaa-7000-8000-000000000002', '{owner}', '0192f3a1-aaaa-7000-8000-000000000001', 'Sub', 'sub', 1, '{EPOCH}', '{EPOCH}');
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {SUBFOLDERS})
             INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             SELECT printf('0192f3a1-bbbb-7000-8000-%012x', i), '{owner}', '0192f3a1-aaaa-7000-8000-000000000002',
                    printf('d%04d', i), printf('d%04d', i), 2, '{EPOCH}', '{EPOCH}'
               FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {FILES})
             INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount, created_at, updated_at, finalized_at)
             SELECT printf('0192f3a1-cccc-7000-8000-%012x', i),
                    'objects/00/00/' || printf('%032x', CASE WHEN i <= {REAL_BLOBS} THEN 9000000 + i ELSE 8000000 + i END),
                    'local', 10, 'active', 1, '{EPOCH}', '{EPOCH}', '{EPOCH}'
               FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {FILES})
             INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized, size_bytes, created_at, updated_at)
             SELECT printf('0192f3a1-dddd-7000-8000-%012x', i), '{owner}',
                    CASE WHEN i % 2 = 0 THEN '0192f3a1-aaaa-7000-8000-000000000001' ELSE '0192f3a1-aaaa-7000-8000-000000000002' END,
                    printf('0192f3a1-cccc-7000-8000-%012x', i),
                    printf('f%05d.bin', i), printf('f%05d.bin', i), 10, '{EPOCH}', '{EPOCH}'
               FROM n;
             UPDATE users SET used_bytes = used_bytes + {FILES} * 10 WHERE id = '{owner}'"
        ))
        .await?;

    world.recorded.clear();
    let started = Instant::now();
    world
        .http
        .send(
            Method::DELETE,
            "/api/v1/folders/0192f3a1-aaaa-7000-8000-000000000001",
            Some(&world.admin),
            None,
        )
        .await?
        .expect(StatusCode::NO_CONTENT)?;
    let claim = world.recorded.all();
    let claim_writes = claim
        .iter()
        .filter(|statement| statement.starts_with("INSERT") || statement.starts_with("UPDATE"))
        .count();
    ensure!(
        claim_writes <= 3,
        "the first phase marks the root, records the intent and enqueues one job: {claim:#?}"
    );
    ensure!(
        count(&claim, "DELETE FROM files") == 0 && count(&claim, "DELETE FROM folders") == 0,
        "the claim deletes nothing: {claim:#?}"
    );

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let folders = world.db.scalar_i64("SELECT COUNT(*) FROM folders").await?;
        let blobs_pending = world
            .db
            .scalar_i64(
                "SELECT COUNT(*) FROM jobs WHERE kind IN ('folders.delete_tree', 'storage.delete_blob')
                   AND state IN ('pending', 'claimed')",
            )
            .await?;
        if folders == 0 && blobs_pending == 0 {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "the background deletion did not finish: {folders} folders and {blobs_pending} jobs remain"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!(
        "drained a {FILES}-file, {SUBFOLDERS}-folder tree in {:?}",
        started.elapsed()
    );

    let statements = world.recorded.all();
    ensure!(
        count(&statements, "OFFSET") == 0,
        "deletion never pages with OFFSET over a mutating dataset"
    );
    let walks = count(&statements, "WITH RECURSIVE walk(id, level)");
    eprintln!(
        "claim statements: {} ({claim_writes} writes); recursive batch statements: {walks}; total statements while draining: {}",
        claim.len(),
        statements.len()
    );
    ensure!(
        (4..=12).contains(&walks),
        "one bounded recursive statement per batch, not per file or folder: {walks} for {FILES} files and {SUBFOLDERS} folders"
    );
    ensure!(
        count(&statements, "FROM folders WHERE parent_id = ") == 0
            && count(&statements, "WHERE parent_id = ?") == 0,
        "no per-folder child lookup loop"
    );
    ensure!(
        count(&statements, "DELETE FROM files WHERE owner_id") <= 4,
        "files are removed in set-based batches of at most 500"
    );

    ensure!(
        world.db.scalar_i64("SELECT COUNT(*) FROM files").await? == 0
            && world.db.scalar_i64("SELECT COUNT(*) FROM folders").await? == 0
    );
    ensure!(
        world
            .db
            .scalar_i64(&format!(
                "SELECT used_bytes FROM users WHERE id = '{owner}'"
            ))
            .await?
            == 0,
        "every byte was returned exactly once"
    );
    ensure!(
        world
            .db
            .scalar_i64("SELECT COUNT(*) FROM storage_objects WHERE state = 'deleted'")
            .await?
            == i64::from(FILES),
        "the real storage.delete_blob job confirmed every object"
    );
    for n in 1..=REAL_BLOBS {
        ensure!(
            !blob_path(&world, n).exists(),
            "blob {n} was erased from the local filesystem"
        );
    }
    let widest: i64 = world
        .db
        .scalar_i64(
            "SELECT COALESCE(MAX(json_extract(metadata_json, '$.files')), 0)
               FROM audit_events WHERE action = 'FILE_DELETED'",
        )
        .await?;
    ensure!(widest <= 500, "no batch exceeded 500 files: {widest}");
    ensure!(
        world
            .db
            .scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action = 'FOLDER_DELETED'")
            .await?
            == 1
    );

    world.app.shutdown().await;
    Ok(())
}

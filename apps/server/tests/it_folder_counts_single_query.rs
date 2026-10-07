pub mod support;

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{ensure, Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use support::client::{v7, Creds, Db, Http, EPOCH};
use support::TestApplication;

const FOLDERS: &str = "/api/v1/folders";
const TREE: &str = "/api/v1/folders/tree";
const PADS: u64 = 60;

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<String>>>);

impl Recorded {
    fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn folder_statements(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|statement| statement.contains("folders"))
            .cloned()
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
    other: String,
    recorded: Recorded,
}

impl World {
    async fn start() -> Result<Self> {
        let recorded = Recorded::default();
        tracing::subscriber::set_global_default(
            Registry::default().with(SqlRecorder(recorded.clone())),
        )
        .context("install the statement recorder")?;
        let app = TestApplication::start("it_folder_counts_single_query").await?;
        let http = Http::new(app.url("/")?)?;
        let db = Db::new(app.data_dir());
        let admin = http.setup_admin().await?;
        let owner = db
            .scalar_string("SELECT id FROM users WHERE username = 'ada'")
            .await?;
        let other = v7(9_000);
        db.insert_user(&other, "mallory", "user").await?;
        Ok(Self {
            app,
            http,
            db,
            admin,
            owner,
            other,
            recorded,
        })
    }

    async fn folder(
        &self,
        owner: &str,
        n: u64,
        parent: Option<u64>,
        name: &str,
        depth: u8,
    ) -> Result<String> {
        let id = v7(n);
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        )
        .bind(&id)
        .bind(owner)
        .bind(parent.map(v7))
        .bind(name)
        .bind(name.to_lowercase())
        .bind(i64::from(depth))
        .bind(EPOCH)
        .execute(&mut connection)
        .await
        .context("seed folder")?;
        Ok(id)
    }

    async fn file(&self, owner: &str, folder: Option<u64>, n: u64, size: i64) -> Result<()> {
        let object = format!("object-{n}");
        self.db
            .execute(&format!(
                "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                    created_at, updated_at, finalized_at)
                 VALUES ('{object}', 'objects/00/00/{n:032x}', 'local', {size}, 'active', 1,
                    '{EPOCH}', '{EPOCH}', '{EPOCH}');
                 INSERT INTO files (id, owner_id, folder_id, storage_object_id, name,
                    name_normalized, size_bytes, created_at, updated_at)
                 VALUES ('{}', '{owner}', {}, '{object}', 'file-{n}.bin', 'file-{n}.bin', {size},
                    '{EPOCH}', '{EPOCH}')",
                v7(100_000 + n),
                folder.map_or_else(|| "NULL".to_owned(), |folder| format!("'{}'", v7(folder)))
            ))
            .await
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.http
            .get(path, Some(&self.admin))
            .await?
            .expect(StatusCode::OK)?
            .json()
    }

    async fn observed(&self, path: &str) -> Result<(Value, Vec<String>)> {
        self.recorded.clear();
        let body = self.get(path).await?;
        Ok((body, self.recorded.folder_statements()))
    }
}

fn recursive(statements: &[String]) -> usize {
    statements
        .iter()
        .filter(|statement| statement.contains("RECURSIVE"))
        .count()
}

fn total(page: &Value, field: &str) -> u64 {
    page["items"].as_array().map_or(0, |items| {
        items.iter().filter_map(|item| item[field].as_u64()).sum()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn it_folder_counts_single_query() -> Result<()> {
    let world = World::start().await?;
    let (owner, other) = (world.owner.clone(), world.other.clone());

    world.folder(&owner, 1, None, "A", 0).await?;
    world.folder(&owner, 2, Some(1), "B", 1).await?;
    world.file(&owner, Some(2), 1, 10).await?;
    world.file(&owner, Some(1), 2, 20).await?;
    world.folder(&owner, 3, None, "C", 0).await?;
    world.folder(&owner, 4, Some(3), "D", 1).await?;
    world.folder(&owner, 5, Some(4), "F", 2).await?;
    world.file(&owner, Some(5), 3, 5).await?;
    world.file(&owner, Some(3), 4, 7).await?;
    world.folder(&owner, 6, None, "E", 0).await?;
    let mut pad_bytes = 0;
    for pad in 0..PADS {
        world
            .folder(&owner, 1_000 + pad, None, &format!("pad-{pad:03}"), 0)
            .await?;
        let size = i64::try_from(pad)? + 1;
        pad_bytes += size;
        world
            .file(&owner, Some(1_000 + pad), 1_000 + pad, size)
            .await?;
    }
    world.folder(&other, 5_000, None, "A", 0).await?;
    world.folder(&other, 5_001, Some(5_000), "Z", 1).await?;
    world.file(&other, Some(5_001), 5_000, 1_000_000).await?;
    world.file(&other, Some(5_000), 5_001, 2_000_000).await?;
    world
        .db
        .execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 40)
             INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth,
                 created_at, updated_at)
             SELECT printf('0192f3a1-0001-7000-8000-%012x', i), '{owner}',
                    CASE WHEN i = 0 THEN '{}'
                         ELSE printf('0192f3a1-0001-7000-8000-%012x', i - 1) END,
                    'chain-' || i, 'chain-' || i, i + 1, '{EPOCH}', '{EPOCH}'
               FROM n",
            v7(6)
        ))
        .await?;

    world.get(&format!("{FOLDERS}?limit=2")).await?;

    let (small, small_statements) = world.observed(&format!("{FOLDERS}?limit=2")).await?;
    let (large, large_statements) = world.observed(&format!("{FOLDERS}?limit=200")).await?;
    ensure!(small["items"].as_array().context("items")?.len() == 2);
    ensure!(large["items"].as_array().context("items")?.len() == 3 + usize::try_from(PADS)?);
    ensure!(large["nextCursor"].is_null());
    ensure!(
        recursive(&small_statements) == 1 && recursive(&large_statements) == 1,
        "one recursive aggregate per request: {small_statements:#?} {large_statements:#?}"
    );
    ensure!(
        small_statements.len() == large_statements.len(),
        "the folder statement count grew with the page size: {} vs {}",
        small_statements.len(),
        large_statements.len()
    );
    ensure!(
        large_statements.len() == 3,
        "list, count and one aggregate: {large_statements:#?}"
    );

    let first = &large["items"][0];
    ensure!(first["name"] == "A" && first["fileCount"] == 2);
    ensure!(first["subfolderCount"] == 1 && first["totalBytes"] == 30);
    let second = &large["items"][1];
    ensure!(second["name"] == "C" && second["fileCount"] == 2);
    ensure!(second["subfolderCount"] == 2 && second["totalBytes"] == 12);
    let third = &large["items"][2];
    ensure!(third["name"] == "E" && third["fileCount"] == 0);
    ensure!(third["subfolderCount"] == 41 && third["totalBytes"] == 0);
    ensure!(total(&large, "fileCount") == 4 + PADS);
    ensure!(total(&large, "subfolderCount") == 1 + 2 + 41);
    ensure!(total(&large, "totalBytes") == 42 + u64::try_from(pad_bytes)?);

    let (_, scoped_statements) = world
        .observed(&format!("{FOLDERS}?parentId={}&limit=200", v7(3)))
        .await?;
    ensure!(
        recursive(&scoped_statements) == 1 && scoped_statements.len() == 4,
        "parent check, list, count and one aggregate: {scoped_statements:#?}"
    );

    let (shallow, shallow_statements) = world.observed(&format!("{FOLDERS}/{}", v7(2))).await?;
    let (deep, deep_statements) = world
        .observed(&format!(
            "{FOLDERS}/{}",
            "0192f3a1-0001-7000-8000-000000000028"
        ))
        .await?;
    ensure!(shallow["path"].as_array().context("path")?.len() == 2);
    ensure!(deep["path"].as_array().context("path")?.len() == 42);
    let crumbs = |statements: &[String]| {
        statements
            .iter()
            .filter(|statement| statement.contains("crumbs"))
            .count()
    };
    ensure!(
        crumbs(&shallow_statements) == 1 && crumbs(&deep_statements) == 1,
        "a breadcrumb is one statement at any depth"
    );
    ensure!(
        shallow_statements.len() == deep_statements.len(),
        "the breadcrumb statement count grew with the depth: {} vs {}",
        shallow_statements.len(),
        deep_statements.len()
    );

    let (tree, tree_statements) = world.observed(&format!("{TREE}?depth=8")).await?;
    ensure!(!tree["nodes"].as_array().context("nodes")?.is_empty());
    ensure!(
        tree_statements.len() == 1 && recursive(&tree_statements) == 1,
        "the tree is one bounded recursive statement: {tree_statements:#?}"
    );
    let (_, rooted_statements) = world
        .observed(&format!("{TREE}?depth=8&rootId={}", v7(3)))
        .await?;
    ensure!(rooted_statements.len() == 1 && recursive(&rooted_statements) == 1);

    let foreign = world.get(&format!("{FOLDERS}?limit=1")).await?;
    ensure!(!foreign.to_string().contains(&other));
    world.app.shutdown().await;
    Ok(())
}

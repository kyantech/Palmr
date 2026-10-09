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

const FILES: &str = "/api/v1/files";

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
            .clone()
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

#[derive(Debug)]
struct Observed {
    body: Value,
    statements: Vec<String>,
}

impl Observed {
    fn matching(&self, needle: &str) -> usize {
        self.statements
            .iter()
            .filter(|statement| statement.contains(needle))
            .count()
    }

    fn indexed(&self) -> usize {
        self.matching("files_fts")
    }

    fn scans(&self) -> usize {
        self.matching("instr(")
    }

    fn crumbs(&self) -> usize {
        self.matching("WITH RECURSIVE crumbs(")
    }
}

impl World {
    async fn start() -> Result<Self> {
        let recorded = Recorded::default();
        tracing::subscriber::set_global_default(
            Registry::default().with(SqlRecorder(recorded.clone())),
        )
        .context("install the statement recorder")?;
        let app = TestApplication::start("it_file_search_statement_shape").await?;
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

    async fn folder(&self, owner: &str, n: u64, parent: Option<u64>, depth: u8) -> Result<()> {
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?6)",
        )
        .bind(v7(n))
        .bind(owner)
        .bind(parent.map(v7))
        .bind(format!("folder-{n}"))
        .bind(i64::from(depth))
        .bind(EPOCH)
        .execute(&mut connection)
        .await
        .context("seed folder")?;
        Ok(())
    }

    async fn file(&self, owner: &str, folder: Option<u64>, n: u64, name: &str) -> Result<()> {
        let object = format!("object-{n}");
        self.db
            .execute(&format!(
                "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                    created_at, updated_at, finalized_at)
                 VALUES ('{object}', 'objects/00/00/{n:032x}', 'local', 1, 'active', 1,
                    '{EPOCH}', '{EPOCH}', '{EPOCH}');
                 INSERT INTO files (id, owner_id, folder_id, storage_object_id, name,
                    name_normalized, size_bytes, created_at, updated_at)
                 VALUES ('{}', '{owner}', {}, '{object}', '{name}', '{name}', 1,
                    '{EPOCH}', '{EPOCH}')",
                v7(100_000 + n),
                folder.map_or_else(|| "NULL".to_owned(), |folder| format!("'{}'", v7(folder)))
            ))
            .await
    }

    async fn observed(&self, query: &str) -> Result<Observed> {
        self.recorded.clear();
        let body = self
            .http
            .get(&format!("{FILES}?{query}"), Some(&self.admin))
            .await?
            .expect(StatusCode::OK)?
            .json()?;
        let statements = self
            .recorded
            .all()
            .into_iter()
            .filter(|statement| statement.contains("files") || statement.contains("folders"))
            .collect();
        Ok(Observed { body, statements })
    }
}

fn hits(page: &Value) -> Result<usize> {
    Ok(page["items"].as_array().context("items")?.len())
}

fn path_lengths(page: &Value) -> Result<Vec<usize>> {
    let mut lengths: Vec<usize> = page["items"]
        .as_array()
        .context("items")?
        .iter()
        .map(|item| item["path"].as_array().map_or(usize::MAX, Vec::len))
        .collect();
    lengths.sort_unstable();
    Ok(lengths)
}

#[tokio::test(flavor = "multi_thread")]
async fn it_file_search_statement_shape() -> Result<()> {
    let world = World::start().await?;
    let (owner, other) = (world.owner.clone(), world.other.clone());

    world.folder(&owner, 50, None, 0).await?;
    for level in 0..8u8 {
        let n = 100 + u64::from(level);
        world
            .folder(&owner, n, level.checked_sub(1).map(|_| n - 1), level)
            .await?;
        world
            .file(&owner, Some(n), 10_000 + n, &format!("chain-{level}.pdf"))
            .await?;
    }
    for index in 0..150u64 {
        world
            .file(
                &owner,
                Some(50),
                1_000 + index,
                &format!("report-pad-{index}.pdf"),
            )
            .await?;
    }
    for index in 0..60u64 {
        world
            .file(
                &owner,
                None,
                2_000 + index,
                &format!("report-root-{index}.pdf"),
            )
            .await?;
    }
    world.folder(&other, 5_000, None, 0).await?;
    for index in 0..50u64 {
        world
            .file(
                &other,
                Some(5_000),
                6_000 + index,
                &format!("report-pad-other-{index}.pdf"),
            )
            .await?;
    }

    world.observed("q=pad&limit=2").await?;

    let small = world.observed("q=pad&limit=5").await?;
    let medium = world.observed("q=pad&limit=50").await?;
    let large = world.observed("q=pad&limit=200").await?;
    ensure!(hits(&small.body)? == 5 && hits(&medium.body)? == 50 && hits(&large.body)? == 150);
    ensure!(large.body["nextCursor"].is_null() && large.body["totalCount"].is_null());
    for observed in [&small, &medium, &large] {
        ensure!(
            observed.statements.len() == 2 && observed.indexed() == 1 && observed.crumbs() == 1,
            "one indexed page query and one batched path query whatever the page size: {:#?}",
            observed.statements
        );
        ensure!(observed.scans() == 0, "{:#?}", observed.statements);
    }

    let root_only = world.observed("q=root&limit=200").await?;
    ensure!(hits(&root_only.body)? == 60);
    ensure!(
        root_only.statements.len() == 1 && root_only.crumbs() == 0,
        "files at the root need no path query: {:#?}",
        root_only.statements
    );

    let chain = world.observed("q=chain&limit=200").await?;
    ensure!(
        path_lengths(&chain.body)? == [1, 2, 3, 4, 5, 6, 7, 8],
        "{:#?}",
        chain.body
    );
    ensure!(
        chain.statements.len() == 2 && chain.crumbs() == 1,
        "path resolution does not grow with the folder depth: {:#?}",
        chain.statements
    );

    let first = world.observed("q=pad&limit=10").await?;
    let cursor = first.body["nextCursor"]
        .as_str()
        .context("cursor")?
        .to_owned();
    let resumed = world
        .observed(&format!("q=pad&limit=10&cursor={cursor}"))
        .await?;
    ensure!(hits(&resumed.body)? == 10);
    ensure!(
        resumed.statements.len() == 2 && resumed.indexed() == 1 && resumed.crumbs() == 1,
        "{:#?}",
        resumed.statements
    );

    let scanned = world.observed("q=hain-3&limit=200").await?;
    ensure!(hits(&scanned.body)? == 1);
    ensure!(
        scanned.statements.len() == 3
            && scanned.indexed() == 1
            && scanned.scans() == 1
            && scanned.crumbs() == 1,
        "index miss, bounded scan, one path query: {:#?}",
        scanned.statements
    );
    let scan_statement = scanned
        .statements
        .iter()
        .find(|statement| statement.contains("instr("))
        .context("scan statement")?;
    ensure!(scan_statement.contains("LIMIT"), "{scan_statement}");
    ensure!(!scan_statement.contains(" LIKE "), "{scan_statement}");

    let miss = world.observed("q=zzzqqq&limit=200").await?;
    ensure!(hits(&miss.body)? == 0 && miss.body["nextCursor"].is_null());
    ensure!(
        miss.statements.len() == 2 && miss.indexed() == 1 && miss.scans() == 1,
        "{:#?}",
        miss.statements
    );

    let tokenless = world.observed("q=%3F%3F%3F").await?;
    ensure!(hits(&tokenless.body)? == 0);
    ensure!(
        tokenless.statements.is_empty(),
        "a query without a usable token touches nothing: {:#?}",
        tokenless.statements
    );

    let everything = [
        &small, &medium, &large, &root_only, &chain, &first, &resumed, &scanned, &miss,
    ];
    for observed in everything {
        for statement in &observed.statements {
            let upper = statement.to_uppercase();
            let words: Vec<&str> = upper
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(|word| !word.is_empty())
                .collect();
            for forbidden in [
                "OFFSET",
                "STORAGE_OBJECTS",
                "SHARE_ITEMS",
                "RECEIVED_FILES",
                "RECEIVED_FILES_FTS",
                "INSERT",
                "UPDATE",
                "DELETE",
                "REPLACE",
                "REBUILD",
                "LIKE",
            ] {
                ensure!(
                    !words.contains(&forbidden),
                    "{forbidden} in a search statement: {statement}"
                );
            }
        }
    }

    for observed in [&large, &chain, &scanned] {
        ensure!(!observed.body.to_string().contains(&other));
        ensure!(!observed.body.to_string().contains("pad-other"));
    }
    world.app.shutdown().await;
    Ok(())
}

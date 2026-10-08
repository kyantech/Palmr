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

    fn listing_statements(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|statement| statement.contains("folders") || statement.contains("files"))
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
        let app = TestApplication::start("it_file_browse_statement_shape").await?;
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
        let name = format!("file-{n}.bin");
        let object = format!("object-{n}");
        self.db
            .execute(&format!(
                "INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                    created_at, updated_at, finalized_at)
                 VALUES ('{object}', 'objects/00/00/{n:032x}', 'local', {size}, 'active', 1,
                    '{EPOCH}', '{EPOCH}', '{EPOCH}');
                 INSERT INTO files (id, owner_id, folder_id, storage_object_id, name,
                    name_normalized, size_bytes, created_at, updated_at)
                 VALUES ('{}', '{owner}', {}, '{object}', '{name}', '{name}', {size},
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
        Ok((body, self.recorded.listing_statements()))
    }
}

fn recursive(statements: &[String]) -> usize {
    statements
        .iter()
        .filter(|statement| statement.contains("RECURSIVE"))
        .count()
}

fn kinds(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["kind"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn it_file_browse_statement_shape() -> Result<()> {
    let world = World::start().await?;
    let (owner, other) = (world.owner.clone(), world.other.clone());

    for pad in 0..PADS {
        world
            .folder(&owner, 1_000 + pad, None, &format!("pad-{pad:03}"), 0)
            .await?;
        world
            .file(
                &owner,
                Some(1_000 + pad),
                1_000 + pad,
                i64::try_from(pad)? + 1,
            )
            .await?;
        world
            .file(&owner, None, 2_000 + pad, i64::try_from(pad)? + 1)
            .await?;
    }
    world.folder(&other, 5_000, None, "A", 0).await?;
    world.file(&other, Some(5_000), 5_000, 1_000_000).await?;
    world.file(&other, None, 5_001, 2_000_000).await?;

    world.get(&format!("{FILES}?limit=2")).await?;

    for sort in ["name:asc", "size:desc", "createdAt:desc", "updatedAt:asc"] {
        let expected_recursive = usize::from(sort.starts_with("size")) + 1;
        let (first, first_statements) = world
            .observed(&format!("{FILES}?limit=2&sort={sort}"))
            .await?;
        let (second, second_statements) = world
            .observed(&format!("{FILES}?limit=3&sort={sort}"))
            .await?;
        ensure!(first["items"].as_array().context("items")?.len() == 2);
        ensure!(second["items"].as_array().context("items")?.len() == 3);
        ensure!(kinds(&first).iter().all(|kind| kind == "folder"));
        ensure!(
            first_statements.len() == second_statements.len(),
            "{sort}: a page wholly inside the folders grew with the page size: {} vs {}",
            first_statements.len(),
            second_statements.len()
        );
        ensure!(
            first_statements.len() == 4,
            "{sort}: folder list, folder aggregate, folder count, file count: {first_statements:#?}"
        );

        let (mid, mid_statements) = world
            .observed(&format!("{FILES}?limit=70&sort={sort}"))
            .await?;
        let (large, large_statements) = world
            .observed(&format!("{FILES}?limit=200&sort={sort}"))
            .await?;
        ensure!(mid["items"].as_array().context("items")?.len() == 70);
        ensure!(large["items"].as_array().context("items")?.len() == 2 * usize::try_from(PADS)?);
        ensure!(large["nextCursor"].is_null() && large["totalCount"] == 2 * PADS);
        ensure!(kinds(&large)
            .iter()
            .take(usize::try_from(PADS)?)
            .all(|kind| kind == "folder"));
        ensure!(
            mid_statements.len() == large_statements.len(),
            "{sort}: a page crossing into the files grew with the page size: {} vs {}",
            mid_statements.len(),
            large_statements.len()
        );
        ensure!(
            large_statements.len() == 5,
            "{sort}: folder list, folder aggregate, file list and two counts: {large_statements:#?}"
        );
        for statements in [&first_statements, &large_statements] {
            ensure!(
                recursive(statements) == expected_recursive,
                "{sort}: recursive statements {} != {expected_recursive}: {statements:#?}",
                recursive(statements)
            );
            ensure!(
                statements
                    .iter()
                    .all(|statement| !statement.to_uppercase().contains("OFFSET")),
                "{sort}: OFFSET paging: {statements:#?}"
            );
            ensure!(
                statements.iter().all(|statement| {
                    !statement.contains("storage_objects") && !statement.contains("share_items")
                }),
                "{sort}: the listing joined storage or share state: {statements:#?}"
            );
        }
    }

    let (_, resumed) = world
        .observed(&format!("{FILES}?limit=10&sort=name:asc&cursor={}", {
            let page = world
                .get(&format!("{FILES}?limit=70&sort=name:asc"))
                .await?;
            page["nextCursor"].as_str().context("cursor")?.to_owned()
        }))
        .await?;
    ensure!(
        resumed.len() == 3,
        "a page entirely inside the files: file list and two counts: {resumed:#?}"
    );
    ensure!(recursive(&resumed) == 0, "{resumed:#?}");

    let nested = world
        .observed(&format!("{FILES}?folderId={}&limit=200", v7(1_000)))
        .await?;
    ensure!(nested.0["totalCount"] == 1);
    ensure!(
        nested.1.len() == 5,
        "parent check, lists and counts: {:#?}",
        nested.1
    );

    let foreign = world.get(&format!("{FILES}?limit=200")).await?;
    ensure!(!foreign.to_string().contains(&other));
    world.app.shutdown().await;
    Ok(())
}

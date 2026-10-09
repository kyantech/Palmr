pub mod support;

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use support::client::{v7, Creds, Db, Http, EPOCH};
use support::TestApplication;

const FOLDERS: &str = "/api/v1/folders";

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
        let app = TestApplication::start("it_folder_move_statement_shape").await?;
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

    async fn folder(&self, n: u64, parent: Option<u64>, name: &str, depth: u8) -> Result<()> {
        let mut connection = self.db.writer().await?;
        sqlx::query(
            "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        )
        .bind(v7(n))
        .bind(&self.owner)
        .bind(parent.map(v7))
        .bind(name)
        .bind(name.to_lowercase())
        .bind(i64::from(depth))
        .bind(EPOCH)
        .execute(&mut connection)
        .await
        .context("seed folder")?;
        Ok(())
    }

    async fn chain(
        &self,
        first: u64,
        parent: u64,
        first_depth: u8,
        count: u8,
        tag: &str,
    ) -> Result<()> {
        let mut above = parent;
        for level in 0..count {
            let id = first + u64::from(level);
            self.folder(
                id,
                Some(above),
                &format!("{tag}-{level}"),
                first_depth + level,
            )
            .await?;
            above = id;
        }
        Ok(())
    }

    async fn observed_move(&self, n: u64, parent: Option<u64>) -> Result<(Value, Vec<String>)> {
        self.recorded.clear();
        let body = self
            .http
            .send(
                Method::POST,
                &format!("{FOLDERS}/{}/move", v7(n)),
                Some(&self.admin),
                Some(json!({ "parentId": parent.map(v7) })),
            )
            .await?
            .expect(StatusCode::OK)?
            .json()?;
        Ok((body, self.recorded.folder_statements()))
    }
}

fn count(statements: &[String], needle: &str) -> usize {
    statements
        .iter()
        .filter(|statement| statement.contains(needle))
        .count()
}

fn shape(statements: &[String]) -> (usize, usize, usize, usize) {
    (
        statements.len(),
        statements
            .iter()
            .filter(|statement| {
                statement
                    .replace("WITH RECURSIVE up(", "")
                    .contains("RECURSIVE")
            })
            .count(),
        count(statements, "UPDATE folders SET parent_id"),
        count(statements, "UPDATE folders SET depth"),
    )
}

fn name_probes(statements: &[String]) -> Vec<&String> {
    statements
        .iter()
        .filter(|statement| {
            statement.starts_with("SELECT") && statement.contains("name_normalized = ")
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn it_folder_move_statement_shape() -> Result<()> {
    let world = World::start().await?;

    world.folder(1, None, "destination", 0).await?;
    world.folder(2, None, "clash-home", 0).await?;
    world.folder(3, Some(2), "clash", 1).await?;

    world.folder(10, None, "small", 0).await?;
    world.chain(11, 10, 1, 2, "small").await?;

    world.folder(100, None, "large", 0).await?;
    world.chain(101, 100, 1, 50, "large").await?;
    for branch in 0..10_u64 {
        world
            .folder(300 + branch, Some(101), &format!("branch-{branch}"), 2)
            .await?;
    }
    world.folder(200, None, "clash", 0).await?;
    world.chain(201, 200, 1, 3, "clash-child").await?;

    let (small, small_statements) = world.observed_move(10, Some(1)).await?;
    let (large, large_statements) = world.observed_move(100, Some(1)).await?;
    ensure!(small["parentId"] == json!(v7(1)) && large["parentId"] == json!(v7(1)));
    ensure!(
        shape(&small_statements) == shape(&large_statements),
        "the move statement shape grew with the subtree: {small_statements:#?} vs {large_statements:#?}"
    );
    let (total, recursive, relocations, shifts) = shape(&large_statements);
    ensure!(
        recursive == 3 && relocations == 1 && shifts == 1,
        "one subtree profile, one relocation, one set-based depth rewrite and the response aggregate: {large_statements:#?}"
    );
    ensure!(
        name_probes(&large_statements).is_empty(),
        "the destination name was probed before the update: {:#?}",
        name_probes(&large_statements)
    );
    ensure!(
        total == 7,
        "source, destination, profile, relocation, rewrite, record and aggregate: {large_statements:#?}"
    );

    ensure!(
        world
            .db
            .scalar_i64("SELECT MAX(depth) FROM folders")
            .await?
            == 51,
        "the 50-level chain moved one level down in a single set-based update"
    );
    ensure!(
        world
            .db
            .scalar_i64(&format!(
                "SELECT depth FROM folders WHERE id = '{}'",
                v7(150)
            ))
            .await?
            == 51
    );

    let (clash, clash_statements) = world.observed_move(200, Some(2)).await?;
    ensure!(clash["name"] == "clash (1)", "{clash}");
    ensure!(
        count(&clash_statements, "UPDATE folders SET parent_id") == 2,
        "candidate 0 collided on the unique index and candidate 1 was stored: {clash_statements:#?}"
    );
    ensure!(count(&clash_statements, "UPDATE folders SET depth") == 1);
    ensure!(name_probes(&clash_statements).is_empty());
    ensure!(
        world
            .db
            .scalar_i64(&format!(
                "SELECT depth FROM folders WHERE id = '{}'",
                v7(203)
            ))
            .await?
            == 4
    );

    world.app.shutdown().await;
    Ok(())
}

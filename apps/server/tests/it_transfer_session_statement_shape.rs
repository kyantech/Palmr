pub mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use support::client::{Creds, Http};
use support::TestApplication;

const SESSIONS: &str = "/api/v1/transfers/sessions";
const DIRECTORIES: usize = 40;
const SMALL: usize = 200;
const LARGE: usize = 2_000;

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Captured>>);

#[derive(Default)]
struct Captured {
    statements: Vec<String>,
    transactions: Vec<(String, u64)>,
}

impl Recorded {
    fn clear(&self) {
        let mut captured = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        captured.statements.clear();
        captured.transactions.clear();
    }

    fn statements(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .statements
            .clone()
    }

    fn transactions(&self) -> Vec<(String, u64)> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .transactions
            .clone()
    }
}

struct Recorder(Recorded);

#[derive(Default)]
struct Fields {
    text: String,
    transaction: Option<String>,
    elapsed_ms: Option<u64>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "summary" | "db.statement" => {
                self.text.push(' ');
                self.text.push_str(value);
            }
            "transaction" => self.transaction = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "elapsed_ms" {
            self.elapsed_ms = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "transaction" && self.transaction.is_none() {
            self.transaction = Some(format!("{value:?}").trim_matches('"').to_owned());
        }
    }
}

impl<S: Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &Event<'_>, _context: LayerContext<'_, S>) {
        let target = event.metadata().target();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut captured = self.0 .0.lock().unwrap_or_else(PoisonError::into_inner);
        if target == "sqlx::query" {
            captured.statements.push(fields.text);
        } else if let (Some(name), Some(elapsed)) = (fields.transaction, fields.elapsed_ms) {
            captured.transactions.push((name, elapsed));
        }
    }
}

fn files(count: usize, tag: &str) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "clientId": format!("{tag}-{index:05}"),
                "name": format!("file-{index:05}.bin"),
                "sizeBytes": 1_000 + index,
                "relativePath": format!(
                    "{tag}/Dir{:02}/Sub{}/file-{index:05}.bin",
                    index % DIRECTORIES,
                    index % 3
                ),
            })
        })
        .collect()
}

struct Measured {
    statements: Vec<String>,
    transactions: Vec<(String, u64)>,
    wall_ms: u128,
}

async fn create(
    http: &Http,
    recorded: &Recorded,
    admin: &Creds,
    tag: &str,
    count: usize,
) -> Result<Measured> {
    let body =
        json!({ "target": { "kind": "my_files", "folderId": null }, "files": files(count, tag) });
    recorded.clear();
    let started = Instant::now();
    let reply = http
        .send_with_headers(
            Method::POST,
            SESSIONS,
            Some(admin),
            Some(body),
            &[("Idempotency-Key", &format!("statement-shape-{tag}-0001"))],
        )
        .await?
        .expect(StatusCode::CREATED)?;
    let wall_ms = started.elapsed().as_millis();
    ensure!(reply.json()?["files"].as_array().map(Vec::len) == Some(count));
    Ok(Measured {
        statements: recorded.statements(),
        transactions: recorded.transactions(),
        wall_ms,
    })
}

fn about(statements: &[String], needle: &str) -> usize {
    statements
        .iter()
        .filter(|statement| statement.contains(needle))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn it_transfer_session_statement_shape() -> Result<()> {
    let recorded = Recorded::default();
    tracing::subscriber::set_global_default(Registry::default().with(Recorder(recorded.clone())))
        .context("install the statement recorder")?;
    let app = TestApplication::start("it_transfer_session_statement_shape").await?;
    let http = Http::new(app.url("/")?)?;
    let admin = http.setup_admin().await?;

    let small = create(&http, &recorded, &admin, "small", SMALL).await?;
    let large = create(&http, &recorded, &admin, "large", LARGE).await?;

    for (label, run, count) in [("200 files", &small, SMALL), ("2000 files", &large, LARGE)] {
        let writes = run.transactions.len();
        let item_inserts = about(&run.statements, "INSERT INTO transfer_session_files");
        let folder_inserts = about(&run.statements, "INSERT INTO folders");
        let session_inserts = about(&run.statements, "INSERT INTO transfer_sessions");
        let holds = about(&run.statements, "INSERT INTO quota_reservations");
        let creation = run
            .transactions
            .iter()
            .filter(|(name, _)| name == "transfers.create_session")
            .collect::<Vec<_>>();
        println!(
            "{label}: statements={} write_transactions={writes} {:?} item_inserts={item_inserts} folder_inserts={folder_inserts} request_wall_ms={}",
            run.statements.len(),
            run.transactions.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            run.wall_ms,
        );
        ensure!(
            writes <= 3,
            "{label}: {writes} write transactions, expected the claim, the admission and at most one session touch"
        );
        ensure!(
            creation.len() == 1,
            "{label}: the admission is exactly one transaction"
        );
        ensure!(
            session_inserts == 1 && holds == 1,
            "{label}: one session, one hold"
        );
        ensure!(
            item_inserts <= count.div_ceil(200),
            "{label}: {item_inserts} item insert statements for {count} files"
        );
        ensure!(
            folder_inserts <= DIRECTORIES * 3 + DIRECTORIES + 1 + 3,
            "{label}: folder inserts are bounded by the distinct directories, got {folder_inserts}"
        );
        ensure!(
            run.statements.len() <= 60 + folder_inserts * 3,
            "{label}: {} statements is not independent of the file count",
            run.statements.len()
        );
    }
    ensure!(
        large.statements.len() <= small.statements.len() + 40,
        "ten times the files cost {} extra statements",
        large
            .statements
            .len()
            .saturating_sub(small.statements.len())
    );

    let sessions = http
        .get(SESSIONS, Some(&admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(sessions["totalCount"] == 2);
    app.shutdown().await;
    Ok(())
}

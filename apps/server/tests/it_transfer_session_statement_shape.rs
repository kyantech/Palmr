pub mod support;

use std::time::Instant;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

use support::client::{Creds, Http};
use support::statements::{about, Recorded, Recorder};
use support::TestApplication;

const SESSIONS: &str = "/api/v1/transfers/sessions";
const DIRECTORIES: usize = 40;
const SMALL: usize = 200;
const LARGE: usize = 2_000;

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

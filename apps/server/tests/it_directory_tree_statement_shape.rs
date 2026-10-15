pub mod support;

use std::collections::HashSet;
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

fn path_of(tag: &str, index: usize) -> String {
    match index % 10 {
        0 => format!("{tag}-root-{index:05}.bin"),
        1..=4 => format!(
            "{tag}/Deep/a/b/c/d/e/f/g/h/leaf{}/file-{index:05}.bin",
            index % 5
        ),
        5..=7 => format!("{tag}/Wide/Dir{:02}/file-{index:05}.bin", index % 97),
        8 => format!("{tag}/Wide/dir{:02}/file-{index:05}.bin", index % 97),
        _ => format!(
            "{tag}/Wide/Dir{:02}/Sub{}/file-{index:05}.bin",
            index % 97,
            index % 3
        ),
    }
}

fn distinct_folders(paths: &[String]) -> usize {
    let mut seen = HashSet::new();
    for path in paths {
        let segments: Vec<&str> = path.split('/').collect();
        for end in 1..segments.len() {
            seen.insert(segments[..end].join("/").to_lowercase());
        }
    }
    seen.len()
}

struct Measured {
    statements: Vec<String>,
    admission_ms: u64,
    wall_ms: u128,
    folders: usize,
}

async fn create(
    http: &Http,
    recorded: &Recorded,
    admin: &Creds,
    tag: &str,
    key: &str,
    count: usize,
) -> Result<Measured> {
    let paths: Vec<String> = (0..count).map(|index| path_of(tag, index)).collect();
    let files: Vec<Value> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            json!({
                "clientId": format!("{tag}-{index:05}"),
                "name": path.rsplit('/').next().unwrap_or(path),
                "sizeBytes": 1_000 + index,
                "relativePath": path,
            })
        })
        .collect();
    let body = json!({ "target": { "kind": "my_files", "folderId": null }, "files": files });
    recorded.clear();
    let started = Instant::now();
    let reply = http
        .send_with_headers(
            Method::POST,
            SESSIONS,
            Some(admin),
            Some(body),
            &[("Idempotency-Key", key)],
        )
        .await?
        .expect(StatusCode::CREATED)?;
    let wall_ms = started.elapsed().as_millis();
    ensure!(reply.json()?["files"].as_array().map(Vec::len) == Some(count));
    let admission_ms = recorded
        .transactions()
        .iter()
        .find(|(name, _)| name == "transfers.create_session")
        .map(|(_, elapsed)| *elapsed)
        .context("the admission transaction was not recorded")?;
    Ok(Measured {
        statements: recorded.statements(),
        admission_ms,
        wall_ms,
        folders: distinct_folders(&paths),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn it_directory_tree_large_tree_statement_shape() -> Result<()> {
    let recorded = Recorded::default();
    tracing::subscriber::set_global_default(Registry::default().with(Recorder(recorded.clone())))
        .context("install the statement recorder")?;
    let app = TestApplication::start("it_directory_tree_large_tree_statement_shape").await?;
    let http = Http::new(app.url("/")?)?;
    let admin = http.setup_admin().await?;

    let mut runs = Vec::new();
    for (label, count) in [("1", 1_usize), ("100", 100), ("400", 400), ("2000", 2_000)] {
        let tag = format!("t{label}");
        let run = create(
            &http,
            &recorded,
            &admin,
            &tag,
            &format!("shape-{label}-key-0001"),
            count,
        )
        .await?;
        runs.push((label, count, run));
    }
    let redrop = create(
        &http,
        &recorded,
        &admin,
        "t2000",
        "shape-2000-key-0002",
        2_000,
    )
    .await?;

    let mut total_rows = 0_usize;
    for (label, count, run) in runs
        .iter()
        .map(|(label, count, run)| (*label, *count, run))
        .chain([("2000 again", 2_000, &redrop)])
    {
        let inserts = about(&run.statements, "INSERT INTO folders");
        let item_inserts = about(&run.statements, "INSERT INTO transfer_session_files");
        let holds = about(&run.statements, "INSERT INTO quota_reservations");
        let sessions = about(&run.statements, "INSERT INTO transfer_sessions");
        let folder_selects = run
            .statements
            .iter()
            .filter(|statement| statement.contains("FROM folders") && statement.contains("SELECT"))
            .count();
        let redrop_run = label == "2000 again";
        println!(
            "tree {label}: files={count} distinct_folders={} statements={} folder_inserts={inserts} folder_lookups={folder_selects} item_inserts={item_inserts} reservations={holds} admission_ms={} request_wall_ms={}",
            run.folders,
            run.statements.len(),
            run.admission_ms,
            run.wall_ms,
        );
        ensure!(
            sessions == 1 && holds == 1,
            "{label}: one session and one reservation"
        );
        ensure!(
            item_inserts <= count.div_ceil(200),
            "{label}: {item_inserts} item insert statements"
        );
        if redrop_run {
            ensure!(
                inserts <= run.folders,
                "{label}: the re-drop attempts at most one insert per distinct folder, got {inserts}"
            );
        } else {
            ensure!(
                inserts == run.folders,
                "{label}: one folder insert per distinct directory, got {inserts} for {}",
                run.folders
            );
        }
        ensure!(
            run.statements.len() <= 60 + run.folders * 2 + item_inserts,
            "{label}: {} statements is not bounded by the distinct directories",
            run.statements.len()
        );
        total_rows += run.folders;
    }
    ensure!(total_rows > 0);

    let rows = http
        .get(SESSIONS, Some(&admin))
        .await?
        .expect(StatusCode::OK)?
        .json()?;
    ensure!(rows["totalCount"] == 5);
    app.shutdown().await;
    Ok(())
}

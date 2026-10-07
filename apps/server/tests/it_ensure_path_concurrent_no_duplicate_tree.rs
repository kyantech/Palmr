pub mod support;

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use support::client::{Creds, Db, Http};
use support::TestApplication;

const ENSURE_PATH: &str = "/api/v1/folders/ensure-path";
const RACERS: usize = 24;

fn ids(value: &Value, field: &str) -> Result<Vec<String>> {
    value[field]
        .as_array()
        .with_context(|| format!("{field} is not an array: {value}"))?
        .iter()
        .map(|id| Ok(id.as_str().context("id is not a string")?.to_owned()))
        .collect()
}

async fn race(
    http: &Arc<Http>,
    creds: &Creds,
    bodies: Vec<Value>,
    keyed: bool,
) -> Result<Vec<Value>> {
    let mut tasks = Vec::new();
    for (index, body) in bodies.into_iter().enumerate() {
        let http = Arc::clone(http);
        let creds = creds.clone();
        tasks.push(tokio::spawn(async move {
            let key = format!("concurrent-ensure-path-key-{index:04}");
            let headers: Vec<(&str, &str)> = if keyed && index % 2 == 0 {
                vec![("idempotency-key", key.as_str())]
            } else {
                Vec::new()
            };
            http.send_with_headers(
                Method::POST,
                ENSURE_PATH,
                Some(&creds),
                Some(body),
                &headers,
            )
            .await?
            .expect(StatusCode::OK)?
            .json()
        }));
    }
    let mut replies = Vec::new();
    for task in tasks {
        replies.push(task.await.context("racer panicked")??);
    }
    Ok(replies)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_ensure_path_concurrent_no_duplicate_tree() -> Result<()> {
    let app = TestApplication::start("it_ensure_path_concurrent_no_duplicate_tree").await?;
    let http = Arc::new(Http::new(app.url("/")?)?);
    let db = Db::new(app.data_dir());
    let admin = http.setup_admin().await?;

    let spellings = [
        json!(["Project", "src", "assets"]),
        json!(["project", "SRC", "Assets"]),
        json!(["PROJECT", "src", "assets"]),
    ];
    let bodies: Vec<Value> = (0..RACERS)
        .map(|n| json!({ "parentId": null, "segments": spellings[n % spellings.len()] }))
        .collect();
    let replies = race(&http, &admin, bodies, true).await?;

    let distinct: HashSet<Vec<String>> = replies
        .iter()
        .map(|reply| ids(reply, "folderIds"))
        .collect::<Result<_>>()?;
    ensure!(
        distinct.len() == 1,
        "racers saw different chains: {distinct:?}"
    );
    let chain = distinct.into_iter().next().context("no chain")?;
    ensure!(chain.len() == 3);
    for reply in &replies {
        ensure!(reply["leafFolderId"].as_str() == Some(chain[2].as_str()));
    }
    let mut created: Vec<String> = Vec::new();
    for reply in &replies {
        created.extend(ids(reply, "created")?);
    }
    created.sort();
    let mut expected = chain.clone();
    expected.sort();
    ensure!(
        created == expected,
        "each folder is created by exactly one racer: {created:?} vs {expected:?}"
    );

    ensure!(db.scalar_i64("SELECT COUNT(*) FROM folders").await? == 3);
    ensure!(
        db.scalar_i64(
            "SELECT COUNT(*) FROM folders WHERE name LIKE '%(%' OR name_normalized LIKE '%(%'"
        )
        .await?
            == 0,
        "no suffixed folder"
    );
    ensure!(
        db.scalar_i64(
            "SELECT COUNT(*) FROM (SELECT 1 FROM folders GROUP BY owner_id, parent_id, name_normalized HAVING COUNT(*) > 1)"
        )
        .await?
            == 0,
        "a normalized sibling name is duplicated"
    );
    ensure!(
        db.scalar_string(
            "SELECT group_concat(name_normalized, '/') FROM (SELECT name_normalized FROM folders ORDER BY depth)"
        )
        .await?
            == "project/src/assets",
        "the first racer's spelling wins; the chain is the same under any spelling"
    );

    let deeper: Vec<Value> = (0..RACERS)
        .map(|n| {
            json!({
                "parentId": chain[1],
                "segments": [if n % 2 == 0 { "assets" } else { "ASSETS" }, "icons", "svg"],
            })
        })
        .collect();
    let replies = race(&http, &admin, deeper, false).await?;
    let mut created = 0;
    let mut leaves = HashSet::new();
    for reply in &replies {
        ensure!(ids(reply, "folderIds")?[0] == chain[2]);
        created += ids(reply, "created")?.len();
        leaves.insert(reply["leafFolderId"].as_str().context("leaf")?.to_owned());
    }
    ensure!(
        created == 2,
        "icons and svg are created exactly once: {created}"
    );
    ensure!(leaves.len() == 1);
    ensure!(db.scalar_i64("SELECT COUNT(*) FROM folders").await? == 5);
    ensure!(db.scalar_i64("SELECT MAX(depth) FROM folders").await? == 4);

    app.shutdown().await;
    Ok(())
}

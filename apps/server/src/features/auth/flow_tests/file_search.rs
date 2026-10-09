use std::collections::{BTreeSet, HashSet};

use base64ct::{Base64UrlUnpadded, Encoding};
use sqlx::{QueryBuilder, Row, Sqlite};

use super::files::FILES;
use super::folders::{Member, FOLDERS, HOST_A, HOST_B, SEEDED_AT};
use super::profile::assert_code;
use super::*;
use crate::features::files::{push_indexed, push_scanned, Probe, SCAN_WINDOW, SEARCH_SORT};
use crate::features::folders::FolderId;
use crate::infra::http::pagination::{CursorKey, SortValue, CURSOR_TAG_LEN};

const EARLY: &str = "2026-09-01T08:00:00.000Z";
const MIDDLE: &str = "2026-09-02T08:00:00.000Z";
const LATE: &str = "2026-09-03T08:00:00.000Z";

const INDEXED: u64 = 0;
const SCANNED: u64 = 1;

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_owned())
        .collect()
}

fn name_set(page: &Value) -> BTreeSet<String> {
    names(page).into_iter().collect()
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

fn cursor_payload(cursor: &str) -> Value {
    let bytes = Base64UrlUnpadded::decode_vec(cursor).unwrap();
    serde_json::from_slice(&bytes[..bytes.len() - CURSOR_TAG_LEN]).unwrap()
}

fn engine(page: &Value) -> u64 {
    cursor_payload(page["nextCursor"].as_str().unwrap())["g"]
        .as_u64()
        .unwrap()
}

fn paths(page: &Value) -> Vec<(String, Vec<String>)> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["name"].as_str().unwrap().to_owned(),
                item["path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|crumb| crumb["name"].as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect()
}

fn many_words() -> String {
    ('0'..='9')
        .chain('a'..='z')
        .chain('\u{3b1}'..='\u{3c9}')
        .map(String::from)
        .collect::<Vec<_>>()
        .join(" ")
}

fn escaped(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

impl Stack {
    async fn find(&self, member: &Member, query: &str) -> Fetched {
        self.read(&format!("{FILES}?{query}"), member).await
    }

    async fn found(&self, member: &Member, query: &str) -> Value {
        let fetched = self.find(member, query).await;
        assert_eq!(
            fetched.status,
            StatusCode::OK,
            "{query}: {}",
            fetched.text()
        );
        let page = fetched.json();
        assert_eq!(page["totalCount"], Value::Null, "{query}");
        page
    }

    async fn found_browse(&self, member: &Member, query: &str) -> Value {
        let fetched = self.read(&format!("{FILES}?{query}"), member).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    async fn search_names(&self, member: &Member, term: &str) -> BTreeSet<String> {
        name_set(&self.found(member, &format!("q={}", escaped(term))).await)
    }

    async fn walk_search(&self, member: &Member, query: &str, limit: usize) -> Vec<Value> {
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("{query}&limit={limit}");
            if let Some(cursor) = &cursor {
                path.push_str("&cursor=");
                path.push_str(cursor);
            }
            let page = self.found(member, &path).await;
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            pages.push(page);
            if cursor.is_none() {
                return pages;
            }
            assert!(pages.len() < 500, "pagination did not terminate: {query}");
        }
    }

    async fn describe(&self, id: &str, description: Option<&str>) {
        let sql = match description {
            Some(text) => format!("UPDATE files SET description = '{text}' WHERE id = '{id}'"),
            None => format!("UPDATE files SET description = NULL WHERE id = '{id}'"),
        };
        self.execute(&sql).await;
    }

    async fn rename_raw(&self, id: &str, name: &str) {
        let normalized = crate::domain::normalize::normalize(name);
        let (id, name) = (id.to_owned(), name.to_owned());
        self.pools
            .write_tx(&self.clock, "files.test_rename_raw", async move |tx| {
                sqlx::query("UPDATE files SET name = ?1, name_normalized = ?2 WHERE id = ?3")
                    .bind(name)
                    .bind(normalized)
                    .bind(id)
                    .execute(tx.executor())
                    .await?;
                Ok::<(), crate::infra::db::DbError>(())
            })
            .await
            .unwrap();
    }

    async fn indexed_ids(&self, term: &str) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT f.id FROM files_fts JOIN files f ON f.rowid = files_fts.rowid
              WHERE files_fts MATCH ?1 ORDER BY f.id",
        )
        .bind(format!("\"{term}\"*"))
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn fill(&self, owner: UserId, prefix: &str, count: u32) {
        for index in 0..count {
            self.put_file(owner, None, &format!("{prefix}-{index}.dat"), 1)
                .await;
        }
    }

    async fn plan(&self, build: impl FnOnce(&mut QueryBuilder<'_, Sqlite>)) -> Vec<String> {
        let mut query = QueryBuilder::<Sqlite>::new("EXPLAIN QUERY PLAN ");
        build(&mut query);
        query
            .build()
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect()
    }
}

#[tokio::test]
async fn it_search_global_recursive_with_path() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let projects = stack.make(&alice, "Projects", None).await;
    let projects_id = projects["id"].as_str().unwrap();
    let year = stack.make(&alice, "2026", Some(projects_id)).await;
    let year_id = year["id"].as_str().unwrap();
    let reports = stack.make(&alice, "Reports", Some(year_id)).await;
    let reports_id = reports["id"].as_str().unwrap();
    let archive = stack.make(&alice, "Archive", None).await;
    let archive_id = archive["id"].as_str().unwrap();
    let templates = stack.make(&alice, "Report Templates", None).await;

    let deep = stack
        .put_file(alice.id, Some(reports_id), "report.pdf", 10)
        .await;
    let old = stack
        .put_file(alice.id, Some(archive_id), "old-report.txt", 20)
        .await;
    let top = stack.put_file(alice.id, None, "Q4 report.docx", 30).await;
    stack
        .put_file(alice.id, Some(projects_id), "invoice.pdf", 40)
        .await;
    let bob_folder = stack.make(&bob, "Report Archive", None).await;
    let bob_file = stack
        .put_file(
            bob.id,
            Some(bob_folder["id"].as_str().unwrap()),
            "report.pdf",
            50,
        )
        .await;

    let page = stack.found(&alice, "q=report").await;
    assert_eq!(page["nextCursor"], Value::Null);
    assert_eq!(page["totalCount"], Value::Null);
    assert!(page["totalCount"].is_null() && page.get("totalCount").is_some());
    let mut found = paths(&page);
    found.sort();
    assert_eq!(
        found,
        vec![
            ("Q4 report.docx".to_owned(), vec![]),
            ("old-report.txt".to_owned(), vec!["Archive".to_owned()]),
            (
                "report.pdf".to_owned(),
                vec![
                    "Projects".to_owned(),
                    "2026".to_owned(),
                    "Reports".to_owned()
                ]
            ),
        ]
    );
    assert_eq!(
        ids(&page).into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([deep.clone(), old.clone(), top.clone()])
    );
    assert!(!page.to_string().contains(&bob_file));
    assert!(!page.to_string().contains("Report Templates"));
    assert!(!page.to_string().contains(templates["id"].as_str().unwrap()));
    for item in page["items"].as_array().unwrap() {
        assert_eq!(item["kind"], "file");
    }

    let by_name = |name: &str| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .unwrap()
            .clone()
    };
    let nested = by_name("report.pdf");
    assert_eq!(
        nested["path"],
        json!([
            { "id": projects_id, "name": "Projects" },
            { "id": year_id, "name": "2026" },
            { "id": reports_id, "name": "Reports" }
        ])
    );
    assert_eq!(nested["folderId"], reports_id);
    assert_eq!(
        nested["path"].as_array().unwrap().last().unwrap()["id"],
        nested["folderId"]
    );
    let at_root = by_name("Q4 report.docx");
    assert_eq!(at_root["folderId"], Value::Null);
    assert_eq!(at_root["path"], json!([]));
    let keys: BTreeSet<&str> = nested
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "kind",
            "id",
            "name",
            "description",
            "sizeBytes",
            "contentType",
            "folderId",
            "createdAt",
            "updatedAt",
            "path"
        ])
    );

    for ignored in [
        format!("folderId={archive_id}"),
        format!("folderId={}", bob_folder["id"].as_str().unwrap()),
        "folderId=not-a-uuid".to_owned(),
        "folderId=".to_owned(),
        format!("folderId={archive_id}&folderId={year_id}"),
        format!("folderId={}", stack.fresh_id()),
    ] {
        let same = stack.found(&alice, &format!("q=report&{ignored}")).await;
        assert_eq!(
            ids(&same).into_iter().collect::<BTreeSet<_>>(),
            ids(&page).into_iter().collect::<BTreeSet<_>>(),
            "{ignored}"
        );
    }

    let browsed = stack.found_browse(&alice, "").await;
    assert_eq!(browsed["totalCount"], 4);
    assert!(browsed["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item.get("path").is_none()));
    stack.stop().await;
}

#[tokio::test]
async fn it_search_owner_scoped() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let alice_docs = stack.make(&alice, "Docs", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let bob_docs = stack.make(&bob, "Docs", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut alice_ids = BTreeSet::new();
    let mut bob_ids = BTreeSet::new();
    for index in 0..12 {
        let name = format!("plan-{index:02}.txt");
        let own = stack.put_file(alice.id, Some(&alice_docs), &name, 5).await;
        stack.describe(&own, Some("quarterly plan")).await;
        alice_ids.insert(own);
        let theirs = stack.put_file(bob.id, Some(&bob_docs), &name, 5).await;
        stack.describe(&theirs, Some("quarterly plan")).await;
        bob_ids.insert(theirs);
    }
    stack.put_file(alice.id, None, "alicenote.txt", 1).await;
    let secret = stack.put_file(bob.id, None, "bobsecretfile.txt", 1).await;
    stack.make(&bob, "BobOnlyFolder", Some(&bob_docs)).await;

    for (member, own, foreign, foreign_folder) in [
        (&alice, &alice_ids, &bob_ids, &bob_docs),
        (&bob, &bob_ids, &alice_ids, &alice_docs),
    ] {
        let pages = stack.walk_search(member, "q=plan", 5).await;
        assert_eq!(pages.len(), 3);
        let seen: Vec<String> = pages.iter().flat_map(ids).collect();
        assert_eq!(seen.len(), 12);
        assert_eq!(&seen.iter().cloned().collect::<BTreeSet<_>>(), own);
        let serialized = serde_json::to_string(&pages).unwrap();
        for id in foreign {
            assert!(!serialized.contains(id.as_str()));
        }
        assert!(!serialized.contains(foreign_folder.as_str()));
        assert!(!serialized.contains("BobOnlyFolder"));

        let steered = stack
            .walk_search(member, &format!("q=plan&folderId={foreign_folder}"), 5)
            .await;
        assert_eq!(steered.iter().flat_map(ids).collect::<BTreeSet<_>>(), *own);
    }

    for term in ["bobsecretfile", "bobsec", "obsecretf", "bobonlyfolder"] {
        let page = stack.found(&alice, &format!("q={term}")).await;
        assert_eq!(page["items"], json!([]), "{term}");
        assert_eq!(page["nextCursor"], Value::Null);
    }
    assert_eq!(
        stack.search_names(&bob, "bobsecret").await,
        BTreeSet::from(["bobsecretfile.txt".to_owned()])
    );
    assert_eq!(
        stack.indexed_ids("bobsecretfile").await,
        vec![secret.clone()]
    );

    let bob_first = stack.found(&bob, "q=plan&limit=5").await;
    let bob_cursor = bob_first["nextCursor"].as_str().unwrap();
    let replayed = stack
        .found(&alice, &format!("q=plan&limit=50&cursor={bob_cursor}"))
        .await;
    let serialized = replayed.to_string();
    for id in &bob_ids {
        assert!(!serialized.contains(id.as_str()));
    }
    assert!(ids(&replayed).iter().all(|id| alice_ids.contains(id)));
    stack.stop().await;
}

#[tokio::test]
async fn it_search_never_touches_contents() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.fill(alice.id, "padding", 8).await;

    let plain = stack.put_file(alice.id, None, "plain.bin", 64).await;
    let key: String = sqlx::query_scalar(
        "SELECT o.object_key FROM files f JOIN storage_objects o ON o.id = f.storage_object_id
          WHERE f.id = ?1",
    )
    .bind(&plain)
    .fetch_one(stack.pools.reader().executor())
    .await
    .unwrap();
    let blob = root.path().join(&key);
    std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
    std::fs::write(&blob, b"xylophonezebra-content-only marker hidden-inside").unwrap();

    for term in [
        "xylophonezebra",
        "xylophone",
        "ylophonezeb",
        "hidden-inside",
        "marker",
    ] {
        assert_eq!(
            stack.found(&alice, &format!("q={term}")).await["items"],
            json!([]),
            "{term}"
        );
    }
    assert!(blob.exists());

    let described = stack.put_file(alice.id, None, "notes.md", 10).await;
    stack
        .describe(&described, Some("contains a platypus sketch"))
        .await;
    for term in ["platypus", "plat", "sketch"] {
        assert_eq!(
            stack.search_names(&alice, term).await,
            BTreeSet::from(["notes.md".to_owned()]),
            "{term}"
        );
    }
    assert_eq!(
        stack.search_names(&alice, "plain").await,
        BTreeSet::from(["plain.bin".to_owned()])
    );
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE '%fts%'",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap();
    assert!(tables
        .iter()
        .all(|table| !table.starts_with("received") || table.starts_with("received_files_fts")));
    stack.stop().await;
}

#[tokio::test]
async fn it_search_ranks_filename_above_description_with_stable_ties() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.fill(alice.id, "padding", 12).await;
    let first = stack.make(&alice, "One", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = stack.make(&alice, "Two", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let notes = stack.put_file(alice.id, None, "notes.txt", 5).await;
    stack.describe(&notes, Some("report")).await;
    let tied = [
        stack.put_file(alice.id, None, "report.pdf", 5).await,
        stack
            .put_file(alice.id, Some(&first), "report.pdf", 5)
            .await,
        stack
            .put_file(alice.id, Some(&second), "report.pdf", 5)
            .await,
    ];

    let best_first = stack.found(&alice, "q=report").await;
    let order = ids(&best_first);
    assert_eq!(order.len(), 4);
    assert_eq!(
        order[3], notes,
        "a description-only match ranks below filenames"
    );
    let mut sorted_ties = tied.to_vec();
    sorted_ties.sort();
    assert_eq!(
        order[..3],
        sorted_ties[..],
        "tied scores fall back to id ascending"
    );

    let explicit = stack.found(&alice, "q=report&sort=relevance:desc").await;
    assert_eq!(ids(&explicit), order);

    let worst_first = stack.found(&alice, "q=report&sort=relevance:asc").await;
    let mut reversed = order.clone();
    reversed.reverse();
    assert_eq!(ids(&worst_first), reversed);

    let by_name = stack.found(&alice, "q=report&sort=name:asc").await;
    assert_eq!(
        names(&by_name),
        ["notes.txt", "report.pdf", "report.pdf", "report.pdf"]
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_search_prefix_matches_by_index_and_substring_by_bounded_scan() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.fill(alice.id, "padding", 8).await;
    for name in [
        "report.pdf",
        "quarterly-report.txt",
        "archive.txt",
        "archive-two.txt",
    ] {
        stack.put_file(alice.id, None, name, 5).await;
    }
    let both = BTreeSet::from(["report.pdf".to_owned(), "quarterly-report.txt".to_owned()]);

    for term in ["rep", "report"] {
        let page = stack.found(&alice, &format!("q={term}&limit=1")).await;
        assert_eq!(engine(&page), INDEXED, "{term}");
        assert_eq!(stack.search_names(&alice, term).await, both, "{term}");
    }
    let interior = stack.found(&alice, "q=port&limit=1").await;
    assert_eq!(engine(&interior), SCANNED, "no token starts with port");
    assert_eq!(stack.search_names(&alice, "port").await, both);
    let archive = stack.found(&alice, "q=archive&limit=1").await;
    assert_eq!(engine(&archive), INDEXED);
    assert_eq!(
        stack.search_names(&alice, "archive").await,
        BTreeSet::from(["archive.txt".to_owned(), "archive-two.txt".to_owned()])
    );
    assert_eq!(stack.found(&alice, "q=zzz").await["items"], json!([]));

    stack.put_file(alice.id, None, "portfolio.pdf", 5).await;
    stack.put_file(alice.id, None, "portrait.png", 5).await;
    let prefix_wins = stack.found(&alice, "q=port&limit=1").await;
    assert_eq!(engine(&prefix_wins), INDEXED);
    assert_eq!(
        stack.search_names(&alice, "port").await,
        BTreeSet::from(["portfolio.pdf".to_owned(), "portrait.png".to_owned()]),
        "when the index has a hit the scan does not run, so interior matches are not added"
    );
    assert_eq!(
        stack.search_names(&alice, "ortfol").await,
        BTreeSet::from(["portfolio.pdf".to_owned()])
    );

    let described = stack.put_file(alice.id, None, "notes2.txt", 5).await;
    stack
        .describe(&described, Some("interior-substring-here"))
        .await;
    assert_eq!(
        stack.found(&alice, "q=ubstring-he").await["items"],
        json!([]),
        "the scan reads names only"
    );
    assert_eq!(
        stack.search_names(&alice, "interior-sub").await,
        BTreeSet::from(["notes2.txt".to_owned()])
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_search_scan_treats_like_wildcards_literally() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    for name in [
        "xxab%cdxx.txt",
        "xxabZcdxx.txt",
        "xxab_cdxx.txt",
        "xxabZZcdxx.txt",
    ] {
        stack.put_file(alice.id, None, name, 5).await;
    }
    for (term, expected) in [
        ("ab%cd", "xxab%cdxx.txt"),
        ("ab_cd", "xxab_cdxx.txt"),
        ("abzcd", "xxabZcdxx.txt"),
    ] {
        let page = stack.found(&alice, &format!("q={}", escaped(term))).await;
        assert_eq!(names(&page), [expected], "{term}");
    }
    assert_eq!(
        stack.found(&alice, "q=%25%25").await["items"],
        json!([]),
        "a lone wildcard has no token and matches nothing"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_search_folds_case_accents_and_unicode_forms() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.fill(alice.id, "padding", 6).await;
    for name in [
        "Relat\u{f3}rio Anual.pdf",
        "RELAT\u{d3}RIO FINAL.docx",
        "f\u{e9}rias 2026.txt",
        "\u{5e74}\u{5ea6}\u{62a5}\u{544a}2026.pdf",
        "\u{30ec}\u{30dd}\u{30fc}\u{30c8}.docx",
        "\u{e2a}\u{e23}\u{e38}\u{e1b}\u{e23}\u{e32}\u{e22}\u{e07}\u{e32}\u{e19}.pdf",
        "README",
        "archive.tar.gz",
        "report_final-v2.pdf",
    ] {
        stack.put_file(alice.id, None, name, 5).await;
    }
    let decomposed = stack.put_file(alice.id, None, "cafe-menu.pdf", 5).await;
    stack.rename_raw(&decomposed, "cafe\u{301} menu.pdf").await;

    let relatorio = BTreeSet::from([
        "Relat\u{f3}rio Anual.pdf".to_owned(),
        "RELAT\u{d3}RIO FINAL.docx".to_owned(),
    ]);
    let ferias = BTreeSet::from(["f\u{e9}rias 2026.txt".to_owned()]);
    let cafe = BTreeSet::from(["cafe\u{301} menu.pdf".to_owned()]);
    for (term, expected) in [
        ("relatorio", &relatorio),
        ("RELATORIO", &relatorio),
        ("relat\u{f3}rio", &relatorio),
        ("RELAT\u{d3}RIO", &relatorio),
        ("relato\u{301}rio", &relatorio),
        ("ReLaT\u{d3}rIo", &relatorio),
        ("ferias", &ferias),
        ("F\u{c9}RIAS", &ferias),
        ("f\u{e9}rias", &ferias),
        ("fe\u{301}rias", &ferias),
        ("cafe", &cafe),
        ("caf\u{e9}", &cafe),
        ("CAFE\u{301}", &cafe),
        ("\u{ff23}\u{ff21}\u{ff26}\u{ff25}", &cafe),
    ] {
        assert_eq!(
            stack.search_names(&alice, term).await,
            *expected,
            "{term:?}"
        );
    }

    let cases = [
        (
            "\u{5e74}\u{5ea6}",
            "\u{5e74}\u{5ea6}\u{62a5}\u{544a}2026.pdf",
        ),
        (
            "\u{62a5}\u{544a}",
            "\u{5e74}\u{5ea6}\u{62a5}\u{544a}2026.pdf",
        ),
        ("\u{30ec}\u{30dd}", "\u{30ec}\u{30dd}\u{30fc}\u{30c8}.docx"),
        (
            "\u{30dd}\u{30fc}\u{30c8}",
            "\u{30ec}\u{30dd}\u{30fc}\u{30c8}.docx",
        ),
        (
            "\u{e2a}\u{e23}\u{e38}\u{e1b}",
            "\u{e2a}\u{e23}\u{e38}\u{e1b}\u{e23}\u{e32}\u{e22}\u{e07}\u{e32}\u{e19}.pdf",
        ),
        (
            "\u{e23}\u{e32}\u{e22}\u{e07}\u{e32}\u{e19}",
            "\u{e2a}\u{e23}\u{e38}\u{e1b}\u{e23}\u{e32}\u{e22}\u{e07}\u{e32}\u{e19}.pdf",
        ),
        ("readme", "README"),
        ("ead", "README"),
        ("tar", "archive.tar.gz"),
        ("gz", "archive.tar.gz"),
        ("tar.gz", "archive.tar.gz"),
        ("archive.tar", "archive.tar.gz"),
        ("final", "report_final-v2.pdf"),
        ("v2", "report_final-v2.pdf"),
        ("report_final", "report_final-v2.pdf"),
        ("final-v2.pdf", "report_final-v2.pdf"),
    ];
    for (term, expected) in cases {
        let found = stack.search_names(&alice, term).await;
        assert!(found.contains(expected), "{term:?} found {found:?}");
        if term != "final" && term != "report_final" {
            assert_eq!(found.len(), 1, "{term:?} found {found:?}");
        }
    }

    let prefix = stack.found(&alice, "q=%E5%B9%B4%E5%BA%A6&limit=1").await;
    assert_eq!(names(&prefix).len(), 1);
    let interior = stack.found(&alice, "q=%E6%8A%A5%E5%91%8A&limit=1").await;
    assert_eq!(names(&interior).len(), 1);
    stack.stop().await;
}

#[tokio::test]
async fn it_search_index_follows_every_write_to_files() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.fill(alice.id, "padding", 6).await;

    let triggers: BTreeSet<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'files'",
    )
    .fetch_all(stack.pools.reader().executor())
    .await
    .unwrap()
    .into_iter()
    .collect();
    assert_eq!(
        triggers,
        BTreeSet::from([
            "files_fts_ad".to_owned(),
            "files_fts_ai".to_owned(),
            "files_fts_au".to_owned()
        ])
    );

    let archive = stack.make(&alice, "Archive", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = stack.put_file(alice.id, None, "alpha-draft.txt", 5).await;
    assert_eq!(stack.indexed_ids("alpha").await, vec![id.clone()]);
    assert_eq!(
        stack.search_names(&alice, "alpha").await,
        BTreeSet::from(["alpha-draft.txt".to_owned()])
    );

    let renamed = stack
        .api(
            Method::PATCH,
            &format!("{FILES}/{id}"),
            &alice,
            Some(&json!({ "name": "omega-final.txt" })),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    assert!(stack.indexed_ids("alpha").await.is_empty());
    assert!(stack.found(&alice, "q=alpha").await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(stack.indexed_ids("omega").await, vec![id.clone()]);
    assert_eq!(
        stack.search_names(&alice, "omega").await,
        BTreeSet::from(["omega-final.txt".to_owned()])
    );

    let described = stack
        .api(
            Method::PATCH,
            &format!("{FILES}/{id}"),
            &alice,
            Some(&json!({ "description": "zeppelin flight notes" })),
        )
        .await;
    assert_eq!(described.status, StatusCode::OK);
    assert_eq!(stack.indexed_ids("zeppelin").await, vec![id.clone()]);
    assert_eq!(
        stack.search_names(&alice, "zeppelin").await,
        BTreeSet::from(["omega-final.txt".to_owned()])
    );
    assert_eq!(stack.search_names(&alice, "omega").await.len(), 1);

    let changed = stack
        .api(
            Method::PATCH,
            &format!("{FILES}/{id}"),
            &alice,
            Some(&json!({ "description": "balloon flight notes" })),
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK);
    assert!(stack.indexed_ids("zeppelin").await.is_empty());
    assert_eq!(stack.indexed_ids("balloon").await, vec![id.clone()]);

    let cleared = stack
        .api(
            Method::PATCH,
            &format!("{FILES}/{id}"),
            &alice,
            Some(&json!({ "description": null })),
        )
        .await;
    assert_eq!(cleared.status, StatusCode::OK);
    assert!(stack.indexed_ids("balloon").await.is_empty());
    assert!(stack.found(&alice, "q=balloon").await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        stack.search_names(&alice, "omega").await,
        BTreeSet::from(["omega-final.txt".to_owned()]),
        "clearing the description keeps the filename searchable"
    );

    let moved = stack
        .api(
            Method::POST,
            &format!("{FILES}/{id}/move"),
            &alice,
            Some(&json!({ "folderId": archive })),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK);
    let after_move = stack.found(&alice, "q=omega").await;
    assert_eq!(names(&after_move), ["omega-final.txt"]);
    assert_eq!(after_move["items"][0]["path"][0]["name"], "Archive");
    assert_eq!(stack.indexed_ids("omega").await, vec![id.clone()]);

    stack
        .describe(&id, Some("direct fixture edit quokka"))
        .await;
    assert_eq!(stack.indexed_ids("quokka").await, vec![id.clone()]);
    assert_eq!(
        stack.search_names(&alice, "quokka").await,
        BTreeSet::from(["omega-final.txt".to_owned()])
    );

    stack
        .execute(&format!("DELETE FROM files WHERE id = '{id}'"))
        .await;
    for term in ["omega", "quokka"] {
        assert!(stack.indexed_ids(term).await.is_empty(), "{term}");
        assert!(stack.found(&alice, &format!("q={term}")).await["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    stack
        .execute("INSERT INTO files_fts(files_fts, rank) VALUES ('integrity-check', 1)")
        .await;
    stack.stop().await;
}

#[tokio::test]
async fn it_search_path_follows_folder_rename_and_move() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let projects = stack.make(&alice, "Projects", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let old = stack.make(&alice, "Old", Some(&projects)).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let archive = stack.make(&alice, "Archive", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let file = stack.put_file(alice.id, Some(&old), "report.pdf", 5).await;
    let before = stack.file_row(&file).await;

    let crumbs = |page: &Value| paths(page).remove(0).1;
    let page = stack.found(&alice, "q=report").await;
    assert_eq!(crumbs(&page), ["Projects", "Old"]);

    let moved = stack
        .api(
            Method::POST,
            &format!("{FOLDERS}/{old}/move"),
            &alice,
            Some(&json!({ "parentId": archive })),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    let page = stack.found(&alice, "q=report").await;
    assert_eq!(crumbs(&page), ["Archive", "Old"]);
    assert_eq!(page["items"][0]["path"][1]["id"], old);
    assert_eq!(page["items"][0]["folderId"], old);

    let renamed = stack.edit(&alice, &old, &json!({ "name": "Older" })).await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    let page = stack.found(&alice, "q=report").await;
    assert_eq!(crumbs(&page), ["Archive", "Older"]);

    let renamed_ancestor = stack
        .edit(&alice, &archive, &json!({ "name": "Cold Storage" }))
        .await;
    assert_eq!(renamed_ancestor.status, StatusCode::OK);
    let page = stack.found(&alice, "q=report").await;
    assert_eq!(crumbs(&page), ["Cold Storage", "Older"]);

    let to_root = stack
        .api(
            Method::POST,
            &format!("{FOLDERS}/{old}/move"),
            &alice,
            Some(&json!({ "parentId": null })),
        )
        .await;
    assert_eq!(to_root.status, StatusCode::OK);
    let page = stack.found(&alice, "q=report").await;
    assert_eq!(crumbs(&page), ["Older"]);

    assert_eq!(
        stack.file_row(&file).await,
        before,
        "folder edits never rewrite the file row, so its index entry is untouched"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_search_validates_the_query_like_every_other_collection() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.put_file(alice.id, None, "report.pdf", 5).await;

    let longest = "a".repeat(128);
    let too_long = "a".repeat(129);
    let invalid = [
        "q=".to_owned(),
        "q".to_owned(),
        "q=a".to_owned(),
        "q=%E4%BD%A0".to_owned(),
        format!("q={too_long}"),
        "q=ab&q=cd".to_owned(),
        "q=ab&q=".to_owned(),
        "q=report&q=report".to_owned(),
        "q=%20%20".to_owned(),
        "q=%09%0A".to_owned(),
        "q=%C2%A0%E3%80%80".to_owned(),
        "q=%00%00".to_owned(),
        "q=%01%02".to_owned(),
        format!("q={}", "%20".repeat(100)),
        format!("q={}", "%E4%BD%A0".repeat(129)),
    ];
    for query in &invalid {
        let refused = stack.find(&alice, query).await;
        assert_code(
            &refused,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            json!(["q"]),
            "{query}"
        );
    }

    let accepted = [
        "q=ab".to_owned(),
        format!("q={longest}"),
        "q=%E4%BD%A0%E5%A5%BD".to_owned(),
        format!("q={}", "%E4%BD%A0".repeat(128)),
        "q=re%CC%81".to_owned(),
        "q=annual+report".to_owned(),
        "q=annual%20%20%20report".to_owned(),
        "q=a%20".to_owned(),
        "q=%20ab%20".to_owned(),
        "q=ab%00cd".to_owned(),
        format!("q={}", escaped(&many_words())),
    ];
    for query in &accepted {
        let page = stack.found(&alice, query).await;
        assert!(page["items"].is_array(), "{query}");
    }

    for query in [
        "q=report&sort=type:asc",
        "q=report&sort=name",
        "q=report&sort=relevance:up",
    ] {
        let refused = stack.find(&alice, query).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert_eq!(
            refused.json()["error"]["details"]["fields"],
            json!(["sort"]),
            "{query}"
        );
    }
    for query in [
        "q=report&limit=0",
        "q=report&limit=201",
        "q=report&limit=ten",
        "q=report&limit=1&limit=2",
    ] {
        let refused = stack.find(&alice, query).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }

    assert_eq!(stack.found(&alice, "q=%FF%FE").await["items"], json!([]));
    for query in ["q=%FFrep", "q=%EF%BF%BDrep", "q=rep%FF"] {
        assert_eq!(
            name_set(&stack.found(&alice, query).await),
            BTreeSet::from(["report.pdf".to_owned()]),
            "{query}: bytes that are not UTF-8 become U+FFFD, which separates words"
        );
    }
    for term in [
        "???", "...", "---", "()", "\"\"", "%%", "__", "~~", "**", "::",
    ] {
        let page = stack.found(&alice, &format!("q={}", escaped(term))).await;
        assert_eq!(page["items"], json!([]), "{term}");
        assert_eq!(page["nextCursor"], Value::Null, "{term}");
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_search_treats_fts_and_sql_syntax_as_plain_text() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    stack.fill(alice.id, "padding", 6).await;
    let report = stack.put_file(alice.id, None, "report.txt", 5).await;
    let secret = stack.put_file(alice.id, None, "secret.txt", 5).await;
    let bob_secret = stack.put_file(bob.id, None, "secret-report.txt", 5).await;
    stack
        .describe(&bob_secret, Some("name secret report"))
        .await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM files")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();

    let hostile = [
        "a\" OR name:secret",
        "name:secret",
        "description:report",
        "{name description}: report",
        "report OR secret",
        "report AND secret",
        "report NOT secret",
        "NOT secret",
        "NEAR(report txt)",
        "NEAR(report txt, 5)",
        "report NEAR secret",
        "^report",
        "report + secret",
        "\"report\" \"secret\"",
        "\"",
        "\"\"",
        "\"\"\"",
        "'",
        "''",
        "' OR 1=1 --",
        "x' OR '1'='1",
        "%' OR 1=1 --",
        "'; DROP TABLE files; --",
        "report'); DELETE FROM files; --",
        "*",
        "**",
        "*report",
        ":",
        "::",
        "()",
        "(report",
        "report)",
        "(((",
        ")))",
        "-",
        "--",
        "+",
        "++",
        "%",
        "%%",
        "_",
        "__",
        "OR",
        "NOT",
        "AND",
        "NEAR",
        "OR OR",
        "NOT NOT",
        "ab\0cd",
        "re\u{7}port",
        "[a-z]",
        "\\",
        "\\\"",
        "report\\",
        "files_fts",
        "rank",
        "rowid:1",
        "bm25(files_fts)",
    ];
    let own: HashSet<&String> = [&report, &secret].into_iter().collect();
    for term in hostile {
        let fetched = stack.find(&alice, &format!("q={}", escaped(term))).await;
        let expected = if term.chars().count() < 2 {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        };
        assert_eq!(fetched.status, expected, "{term:?}: {}", fetched.text());
        if expected == StatusCode::OK {
            let page = fetched.json();
            assert!(
                ids(&page).iter().all(|id| own.contains(id)),
                "{term:?} returned {page}"
            );
            assert!(!page.to_string().contains(&bob_secret), "{term:?}");
        }
    }

    for (term, expected) in [
        ("report OR secret", vec![]),
        ("report AND secret", vec![]),
        ("name:secret", vec![]),
        ("description:report", vec![]),
        ("NEAR(report txt)", vec![]),
        ("a\" OR name:secret", vec![]),
        ("OR", vec!["report.txt"]),
        ("-secret", vec!["secret.txt"]),
        ("secret*", vec!["secret.txt"]),
        ("\"secret\"", vec!["secret.txt"]),
        ("^report", vec!["report.txt"]),
        ("report.txt", vec!["report.txt"]),
    ] {
        assert_eq!(
            stack.search_names(&alice, term).await,
            expected
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>(),
            "{term:?}"
        );
    }

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM files")
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(after, before);
    stack
        .execute("INSERT INTO files_fts(files_fts, rank) VALUES ('integrity-check', 1)")
        .await;
    stack.stop().await;
}

struct Seeded {
    ids: Vec<(String, String, i64, String, String)>,
}

async fn seed_page_world(stack: &Stack, owner: UserId) -> Seeded {
    let mut folders: Vec<Option<String>> = vec![None];
    for name in ["Alpha", "Beta", "Gamma"] {
        let id = stack.seed_folder(owner, None, name, 0).await;
        folders.push(Some(id));
    }
    let created = [EARLY, MIDDLE, LATE];
    let updated = [LATE, EARLY];
    let mut seeded = Vec::new();
    for index in 0..36usize {
        let name = format!("report-{:02}.pdf", index % 9);
        let size = i64::try_from(index % 3).unwrap() * 100 + 1;
        let id = stack
            .put_file_at(
                owner,
                folders[index % 4].as_deref(),
                &name,
                size,
                created[index % 3],
                updated[index % 2],
            )
            .await;
        if index % 5 == 0 {
            stack.describe(&id, Some("report copy")).await;
        }
        seeded.push((
            id,
            name.to_lowercase(),
            size,
            created[index % 3].to_owned(),
            updated[index % 2].to_owned(),
        ));
    }
    stack.fill(owner, "filler", 12).await;
    Seeded { ids: seeded }
}

#[tokio::test]
async fn it_search_keyset_pages_are_complete_ordered_and_duplicate_free_for_every_sort() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let seeded = seed_page_world(&stack, alice.id).await;
    let total = seeded.ids.len();

    let expected = |key: fn(&(String, String, i64, String, String)) -> String, descending: bool| {
        let mut rows = seeded.ids.clone();
        rows.sort_by(|left, right| key(left).cmp(&key(right)).then(left.0.cmp(&right.0)));
        if descending {
            rows.reverse();
        }
        rows.into_iter().map(|row| row.0).collect::<Vec<_>>()
    };
    let by_name = |row: &(String, String, i64, String, String)| row.1.clone();
    let by_size = |row: &(String, String, i64, String, String)| format!("{:020}", row.2);
    let by_created = |row: &(String, String, i64, String, String)| row.3.clone();
    let by_updated = |row: &(String, String, i64, String, String)| row.4.clone();

    let sorts: [(&str, Option<Vec<String>>); 10] = [
        ("name:asc", Some(expected(by_name, false))),
        ("name:desc", Some(expected(by_name, true))),
        ("size:asc", Some(expected(by_size, false))),
        ("size:desc", Some(expected(by_size, true))),
        ("createdAt:asc", Some(expected(by_created, false))),
        ("createdAt:desc", Some(expected(by_created, true))),
        ("updatedAt:asc", Some(expected(by_updated, false))),
        ("updatedAt:desc", Some(expected(by_updated, true))),
        ("relevance:desc", None),
        ("relevance:asc", None),
    ];

    for (sort, exact) in sorts {
        let query = format!("q=report&sort={sort}");
        let whole = stack.found(&alice, &format!("{query}&limit=200")).await;
        let whole_ids = ids(&whole);
        assert_eq!(whole_ids.len(), total, "{sort}");
        assert_eq!(whole["nextCursor"], Value::Null);
        if let Some(exact) = exact {
            assert_eq!(whole_ids, exact, "{sort}");
        }
        for limit in [1usize, 3, 8, 35, 36] {
            if limit == 1 && !matches!(sort, "name:asc" | "relevance:desc") {
                continue;
            }
            let pages = stack.walk_search(&alice, &query, limit).await;
            let walked: Vec<String> = pages.iter().flat_map(ids).collect();
            assert_eq!(walked, whole_ids, "{sort} limit {limit}");
            assert_eq!(
                walked.iter().collect::<HashSet<_>>().len(),
                total,
                "{sort} limit {limit}: duplicates"
            );
            assert_eq!(pages.len(), total.div_ceil(limit), "{sort} limit {limit}");
            let last = pages.len() - 1;
            for (index, page) in pages.iter().enumerate() {
                assert_eq!(page["totalCount"], Value::Null);
                if index < last {
                    assert_eq!(ids(page).len(), limit);
                    assert!(page["nextCursor"].is_string());
                } else {
                    assert_eq!(page["nextCursor"], Value::Null, "{sort} limit {limit}");
                }
            }
        }
    }

    let order = ids(&stack.found(&alice, "q=report&limit=200").await);
    let by_id: std::collections::HashMap<&String, &(String, String, i64, String, String)> =
        seeded.ids.iter().map(|row| (&row.0, row)).collect();
    let descriptions: std::collections::HashMap<String, Option<String>> =
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT id, description FROM files WHERE name LIKE 'report-%'",
        )
        .fetch_all(stack.pools.reader().executor())
        .await
        .unwrap()
        .into_iter()
        .collect();
    for pair in order.windows(2) {
        let (left, right) = (by_id[&pair[0]], by_id[&pair[1]]);
        if left.1 == right.1 && descriptions[&pair[0]] == descriptions[&pair[1]] {
            assert!(pair[0] < pair[1], "tied scores must ascend by id");
        }
    }
    let reversed = ids(&stack
        .found(&alice, "q=report&sort=relevance:asc&limit=200")
        .await);
    let mut flipped = order.clone();
    flipped.reverse();
    assert_eq!(reversed, flipped);
    stack.stop().await;
}

#[tokio::test]
async fn it_search_cursors_are_bound_to_query_sort_and_mode() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    for index in 0..5 {
        stack
            .put_file(alice.id, None, &format!("report-{index}.pdf"), 5)
            .await;
        stack
            .put_file(alice.id, None, &format!("invoice-{index}.pdf"), 5)
            .await;
    }
    stack.seed_folder(alice.id, None, "One", 0).await;
    stack.seed_folder(alice.id, None, "Two", 0).await;

    let first = stack.found(&alice, "q=report&limit=2").await;
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let rest = stack
        .found(&alice, &format!("q=report&limit=10&cursor={cursor}"))
        .await;
    let combined: Vec<String> = ids(&first).into_iter().chain(ids(&rest)).collect();
    assert_eq!(combined.len(), 5);
    assert_eq!(combined.iter().collect::<HashSet<_>>().len(), 5);

    for same in ["q=Report", "q=%20report%20", "q=REPORT"] {
        let resumed = stack
            .find(&alice, &format!("{same}&limit=10&cursor={cursor}"))
            .await;
        assert_eq!(resumed.status, StatusCode::OK, "{same}");
        assert_eq!(ids(&resumed.json()), ids(&rest), "{same}");
    }

    let browse = stack.found_browse(&alice, "limit=2").await;
    let browse_cursor = browse["nextCursor"].as_str().unwrap().to_owned();
    let folders = stack
        .read(&format!("{FOLDERS}?limit=1"), &alice)
        .await
        .json();
    let folder_cursor = folders["nextCursor"].as_str().unwrap().to_owned();
    let mut tampered = cursor.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });

    for query in [
        format!("q=invoice&cursor={cursor}"),
        format!("q=reports&cursor={cursor}"),
        format!("q=report+pdf&cursor={cursor}"),
        format!("q=repor&cursor={cursor}"),
        format!("q=report&sort=name:asc&cursor={cursor}"),
        format!("q=report&sort=relevance:asc&cursor={cursor}"),
        format!("q=report&sort=size:desc&cursor={cursor}"),
        format!("q=report&cursor={browse_cursor}"),
        format!("q=report&sort=name:asc&cursor={browse_cursor}"),
        format!("q=report&cursor={folder_cursor}"),
        format!("q=report&cursor={tampered}"),
        format!("q=report&cursor={}", &cursor[..cursor.len() / 2]),
        "q=report&cursor=".to_owned(),
        "q=report&cursor=%25%25%25".to_owned(),
    ] {
        let refused = stack.find(&alice, &query).await;
        assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    }
    let repeated = stack
        .find(&alice, &format!("q=report&cursor={cursor}&cursor={cursor}"))
        .await;
    assert_code(
        &repeated,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    let refused = stack
        .read(&format!("{FILES}?limit=2&cursor={cursor}"), &alice)
        .await;
    assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let refused = stack
        .read(&format!("{FOLDERS}?limit=2&cursor={cursor}"), &alice)
        .await;
    assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");

    let name_page = stack.found(&alice, "q=report&sort=name:asc&limit=2").await;
    let name_cursor = name_page["nextCursor"].as_str().unwrap();
    let refused = stack
        .find(&alice, &format!("q=report&limit=2&cursor={name_cursor}"))
        .await;
    assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    let resumed = stack
        .find(
            &alice,
            &format!("q=report&sort=name:asc&limit=2&cursor={name_cursor}"),
        )
        .await;
    assert_eq!(resumed.status, StatusCode::OK);

    let scanned = stack.found(&alice, "q=eport-&limit=2").await;
    assert_eq!(engine(&scanned), SCANNED);
    let scan_cursor = scanned["nextCursor"].as_str().unwrap().to_owned();
    let refused = stack
        .find(&alice, &format!("q=report&cursor={scan_cursor}"))
        .await;
    assert_code(&refused, StatusCode::BAD_REQUEST, "CURSOR_INVALID");
    stack.put_file(alice.id, None, "eportx.txt", 5).await;
    let fresh = stack.walk_search(&alice, "q=eport-", 2).await;
    assert_eq!(
        fresh.iter().flat_map(names).collect::<Vec<_>>(),
        ["eportx.txt"],
        "a fresh traversal now finds the new indexed hit instead of scanning"
    );
    let mut resumed_scan = Vec::new();
    let mut cursor = Some(scan_cursor);
    while let Some(current) = cursor {
        let page = stack
            .found(&alice, &format!("q=eport-&limit=2&cursor={current}"))
            .await;
        resumed_scan.extend(ids(&page));
        cursor = page["nextCursor"].as_str().map(str::to_owned);
    }
    assert_eq!(
        resumed_scan.len(),
        3,
        "a scan cursor keeps scanning to the end"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_search_scan_reads_a_bounded_window_of_the_newest_files() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bulk = SCAN_WINDOW + 100;
    stack
        .execute(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {bulk})
             INSERT INTO storage_objects (id, object_key, provider, size_bytes, state, refcount,
                                          created_at, updated_at, finalized_at)
             SELECT 'bulk-' || i, 'objects/00/00/' || printf('%032x', 700000 + i), 'local', 1,
                    'active', 1, '{SEEDED_AT}', '{SEEDED_AT}', '{SEEDED_AT}' FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {bulk})
             INSERT INTO files (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                                size_bytes, created_at, updated_at)
             SELECT printf('0192f3a1-0000-7000-8000-%012x', i), '{}', NULL, 'bulk-' || i,
                    'bulk-' || printf('%05d', i) || '.dat', 'bulk-' || printf('%05d', i) || '.dat',
                    1, strftime('%Y-%m-%dT%H:%M:%fZ', 1790000000 + i, 'unixepoch'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 1790000000 + i, 'unixepoch') FROM n",
            alice.id
        ))
        .await;
    for (index, name) in [
        (1, "zebraunique.dat"),
        (100, "edgeoutside.dat"),
        (101, "edgeinside.dat"),
        (bulk, "zzinteriorfind.dat"),
    ] {
        let id = format!("0192f3a1-0000-7000-8000-{index:012x}");
        stack.rename_raw(&id, name).await;
    }

    assert_eq!(
        stack.search_names(&alice, "ebraun").await,
        BTreeSet::new(),
        "the oldest files fall outside the scanned window"
    );
    assert_eq!(
        stack.search_names(&alice, "dgeoutsi").await,
        BTreeSet::new(),
        "the file one position outside the window is not scanned"
    );
    assert_eq!(
        stack.search_names(&alice, "dgeinsid").await,
        BTreeSet::from(["edgeinside.dat".to_owned()])
    );
    assert_eq!(
        stack.search_names(&alice, "nteriorfi").await,
        BTreeSet::from(["zzinteriorfind.dat".to_owned()])
    );
    assert_eq!(
        stack.search_names(&alice, "zebra").await,
        BTreeSet::from(["zebraunique.dat".to_owned()]),
        "a token prefix is found by the index wherever the file sits"
    );

    let broad = stack.found(&alice, "q=ulk-0&limit=200").await;
    assert_eq!(ids(&broad).len(), 200);
    assert!(broad["nextCursor"].is_string());
    let narrow = stack.found(&alice, "q=ulk-0&limit=7").await;
    assert_eq!(ids(&narrow).len(), 7);
    assert_eq!(&ids(&broad)[..7], &ids(&narrow)[..]);
    stack.stop().await;
}

#[tokio::test]
async fn it_search_primary_query_is_driven_by_files_fts() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.fill(alice.id, "padding", 4).await;
    let sorts = [
        "relevance:desc",
        "relevance:asc",
        "name:asc",
        "size:desc",
        "createdAt:asc",
        "updatedAt:desc",
    ];
    for sort in sorts {
        let spec = SEARCH_SORT.parse(Some(sort)).unwrap();
        for with_cursor in [false, true] {
            let cursor = with_cursor.then(|| {
                CursorKey::in_group(
                    0,
                    if sort.starts_with("relevance") || sort.starts_with("size") {
                        SortValue::Integer(1)
                    } else {
                        SortValue::Text("a".to_owned())
                    },
                    FolderId::generate(&stack.clock),
                )
                .bound_to("zzword")
            });
            let probe = Probe {
                owner: alice.id,
                expression: "\"zzword\"*",
                needle: "zzword",
                sort: &spec,
                after: cursor.as_ref(),
                fetch: 51,
            };
            let mut text = QueryBuilder::<Sqlite>::new("");
            push_indexed(&mut text, &probe);
            let sql = text.sql().to_owned();
            assert!(sql.contains("files_fts MATCH ?"), "{sort}: {sql}");
            assert!(sql.contains("f.owner_id = ?"), "{sort}: {sql}");
            assert!(
                !sql.contains("zzword"),
                "{sort}: the phrase is bound, not spliced"
            );
            let upper = sql.to_uppercase();
            for forbidden in [" LIKE ", "OFFSET", "INSTR(", "STORAGE_OBJECTS", "RECEIVED"] {
                assert!(!upper.contains(forbidden), "{sort}: {forbidden} in {sql}");
            }
            let plan = stack.plan(|query| push_indexed(query, &probe)).await;
            let joined = plan.join("\n");
            assert!(
                joined.contains("files_fts VIRTUAL TABLE INDEX"),
                "{sort}: {joined}"
            );
            assert!(
                joined.contains("SEARCH f USING INTEGER PRIMARY KEY (rowid=?)"),
                "{sort}: {joined}"
            );
            assert!(
                plan.iter()
                    .filter(|line| line.starts_with("SCAN"))
                    .all(|line| line.contains("files_fts")
                        || line.contains("hits")
                        || line == "SCAN up"),
                "{sort}: no full scan of files: {joined}"
            );
        }
    }

    let spec = SEARCH_SORT.parse(Some("name:asc")).unwrap();
    let probe = Probe {
        owner: alice.id,
        expression: "\"zzword\"*",
        needle: "zzword",
        sort: &spec,
        after: None,
        fetch: 51,
    };
    let mut text = QueryBuilder::<Sqlite>::new("");
    push_scanned(&mut text, &probe);
    let sql = text.sql().to_owned();
    assert!(sql.contains("owner_id = ?"), "{sql}");
    assert!(sql.contains("LIMIT ?"), "{sql}");
    assert!(sql.contains("instr(name_normalized, ?"), "{sql}");
    assert!(!sql.to_uppercase().contains(" LIKE "), "{sql}");
    assert!(!sql.to_uppercase().contains("OFFSET"), "{sql}");
    assert!(!sql.contains("zzword"));
    let plan = stack
        .plan(|query| push_scanned(query, &probe))
        .await
        .join("\n");
    assert!(
        plan.contains("ix_files_owner_created") && plan.contains("owner_id=?"),
        "{plan}"
    );
    assert!(
        !plan
            .lines()
            .any(|line| line.starts_with("SCAN files") || line.starts_with("SCAN f ")),
        "{plan}"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_search_leaves_browse_untouched_and_never_merges_received() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let docs = stack.make(&alice, "Docs", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    stack.put_file(alice.id, Some(&docs), "report.pdf", 5).await;
    stack.put_file(alice.id, None, "root-report.pdf", 5).await;

    let before = stack.found_browse(&alice, "limit=50").await;
    assert_eq!(before["totalCount"], 2);
    assert_eq!(before["items"][0]["kind"], "folder");
    assert_eq!(before["items"][1]["kind"], "file");
    let nested = stack
        .found_browse(&alice, &format!("folderId={docs}"))
        .await;
    assert_eq!(names(&nested), ["report.pdf"]);
    assert_eq!(nested["totalCount"], 1);

    let search = stack.found(&alice, "q=report").await;
    assert_eq!(search["items"].as_array().unwrap().len(), 2);

    assert_eq!(stack.found_browse(&alice, "limit=50").await, before);
    let refused = stack
        .read(&format!("{FILES}/search?q=report"), &alice)
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    stack.stop().await;
}

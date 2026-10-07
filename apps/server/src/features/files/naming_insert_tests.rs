use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;
use time::macros::datetime;
use uuid::Uuid;

use super::naming_insert::{
    insert_with_unique_name, Attempt, NameNamespace, NamedInsertError, Stored,
};
use crate::config::SqliteSynchronous;
use crate::domain::clock::TestClock;
use crate::domain::error_code::ErrorCode;
use crate::domain::naming::{
    CandidateError, InvalidName, NameCandidate, MAX_NAME_ATTEMPTS, MAX_NAME_BYTES,
};
use crate::infra::db::{DbError, DbErrorKind, DbPools, MIGRATOR};
use crate::infra::http::error::ApiError;
use http::StatusCode;

const START: time::OffsetDateTime = datetime!(2026-10-07 12:00 UTC);
const NOW: &str = "2026-10-07T12:00:00.000Z";
const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";
const WRITERS: usize = 16;

struct Harness {
    _root: TempDir,
    pools: DbPools,
    clock: TestClock,
    next_object: AtomicU32,
}

impl Harness {
    async fn open() -> Arc<Self> {
        let root = TempDir::new().unwrap();
        let pools = DbPools::open(root.path(), 4, SqliteSynchronous::Full)
            .await
            .unwrap();
        pools.migrate(&MIGRATOR).await.unwrap();
        let harness = Arc::new(Self {
            _root: root,
            pools,
            clock: TestClock::new(START),
            next_object: AtomicU32::new(1),
        });
        for (id, username) in [(ALICE, "alice"), (BOB, "bob")] {
            harness
                .pools
                .write_tx(&harness.clock, "naming.test_user", async |tx| {
                    sqlx::query(
                        "INSERT INTO users
                             (id, email, email_normalized, username, username_normalized,
                              created_at, updated_at)
                         VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?4)",
                    )
                    .bind(id)
                    .bind(format!("{username}@example.test"))
                    .bind(username)
                    .bind(NOW)
                    .execute(tx.executor())
                    .await
                    .map_err(DbError::from)?;
                    Ok::<(), DbError>(())
                })
                .await
                .unwrap();
        }
        harness
    }

    fn object_seed(&self) -> u32 {
        self.next_object.fetch_add(1, Ordering::Relaxed)
    }
}

async fn insert_object(connection: &mut SqliteConnection, seed: u32) -> Result<String, DbError> {
    let id = format!("object-{seed}");
    sqlx::query(
        "INSERT INTO storage_objects
             (id, object_key, provider, size_bytes, state, refcount,
              created_at, updated_at, finalized_at)
         VALUES (?1, ?2, 'local', 0, 'active', 1, ?3, ?3, ?3)",
    )
    .bind(&id)
    .bind(format!("objects/00/00/{seed:032x}"))
    .bind(NOW)
    .execute(connection)
    .await?;
    Ok(id)
}

fn folder_namespace(parent: Option<&str>) -> NameNamespace {
    if parent.is_some() {
        NameNamespace::FoldersInFolder
    } else {
        NameNamespace::FoldersAtRoot
    }
}

fn file_namespace(folder: Option<&str>) -> NameNamespace {
    if folder.is_some() {
        NameNamespace::FilesInFolder
    } else {
        NameNamespace::FilesAtRoot
    }
}

async fn try_insert_folder(
    connection: &mut SqliteConnection,
    owner: &str,
    parent: Option<&str>,
    candidate: &NameCandidate,
) -> Result<Attempt<()>, DbError> {
    let sql = format!(
        "INSERT INTO folders
             (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) {}",
        folder_namespace(parent).conflict_clause()
    );
    let result = sqlx::query(&sql)
        .bind(Uuid::now_v7().to_string())
        .bind(owner)
        .bind(parent)
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(i64::from(parent.is_some()))
        .bind(NOW)
        .execute(connection)
        .await?;
    Ok(Attempt::from_insert(&result))
}

async fn try_insert_file(
    connection: &mut SqliteConnection,
    owner: &str,
    folder: Option<&str>,
    object: &str,
    candidate: &NameCandidate,
) -> Result<Attempt<()>, DbError> {
    let sql = format!(
        "INSERT INTO files
             (id, owner_id, folder_id, storage_object_id, name, name_normalized,
              size_bytes, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7) {}",
        file_namespace(folder).conflict_clause()
    );
    let result = sqlx::query(&sql)
        .bind(Uuid::now_v7().to_string())
        .bind(owner)
        .bind(folder)
        .bind(object)
        .bind(candidate.display())
        .bind(candidate.normalized())
        .bind(NOW)
        .execute(connection)
        .await?;
    Ok(Attempt::from_insert(&result))
}

async fn make_folder(
    harness: &Harness,
    owner: &str,
    parent: Option<&str>,
    name: &str,
) -> Result<Stored<()>, NamedInsertError<DbError>> {
    harness
        .pools
        .write_tx(&harness.clock, "naming.test_folder", async |tx| {
            insert_with_unique_name(name, async |candidate: NameCandidate| {
                try_insert_folder(tx.executor(), owner, parent, &candidate).await
            })
            .await
        })
        .await
}

async fn make_file(
    harness: &Harness,
    owner: &str,
    folder: Option<&str>,
    name: &str,
) -> Result<Stored<()>, NamedInsertError<DbError>> {
    let seed = harness.object_seed();
    harness
        .pools
        .write_tx(&harness.clock, "naming.test_file", async |tx| {
            let object = insert_object(tx.executor(), seed).await?;
            let object = object.as_str();
            insert_with_unique_name(name, async |candidate: NameCandidate| {
                try_insert_file(tx.executor(), owner, folder, object, &candidate).await
            })
            .await
        })
        .await
}

async fn folder_id(harness: &Harness, owner: &str, name: &str) -> String {
    sqlx::query_scalar("SELECT id FROM folders WHERE owner_id = ?1 AND name = ?2")
        .bind(owner)
        .bind(name)
        .fetch_one(harness.pools.reader().executor())
        .await
        .unwrap()
}

async fn names(harness: &Harness, table: &str, owner: &str) -> Vec<(String, String)> {
    sqlx::query_as(&format!(
        "SELECT name, name_normalized FROM {table} WHERE owner_id = ?1 ORDER BY rowid"
    ))
    .bind(owner)
    .fetch_all(harness.pools.reader().executor())
    .await
    .unwrap()
}

fn assert_pairwise_distinct(rows: &[(String, String)]) {
    let normalized: HashSet<&str> = rows
        .iter()
        .map(|(_, normalized)| normalized.as_str())
        .collect();
    assert_eq!(normalized.len(), rows.len(), "{rows:?}");
}

#[tokio::test]
async fn it_naming_insert_keep_both_sequence_in_root_and_nested_namespaces() {
    let harness = Harness::open().await;

    let root: Vec<_> = futures_util::future::join_all(
        [0; 4].map(|_| async { make_file(&harness, ALICE, None, "photo.jpg").await.unwrap() }),
    )
    .await;
    assert_eq!(root.len(), 4);
    let stored: Vec<String> = names(&harness, "files", ALICE)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        stored,
        [
            "photo.jpg",
            "photo (1).jpg",
            "photo (2).jpg",
            "photo (3).jpg"
        ]
    );

    let docs = make_folder(&harness, ALICE, None, "docs").await.unwrap();
    assert_eq!(docs.name.display(), "docs");
    assert!(!docs.renamed());
    let docs_id = folder_id(&harness, ALICE, "docs").await;

    for expected in ["README", "README (1)", "README (2)"] {
        let stored = make_file(&harness, ALICE, Some(&docs_id), "README")
            .await
            .unwrap();
        assert_eq!(stored.name.display(), expected);
    }
    for expected in [".env", ".env (1)"] {
        let stored = make_file(&harness, ALICE, Some(&docs_id), ".env")
            .await
            .unwrap();
        assert_eq!(stored.name.display(), expected);
    }
    for expected in ["archive.tar.gz", "archive.tar (1).gz", "archive.tar (2).gz"] {
        let stored = make_file(&harness, ALICE, None, "archive.tar.gz")
            .await
            .unwrap();
        assert_eq!(stored.name.display(), expected);
    }
    for expected in ["photo (1) (1).jpg", "photo (1) (2).jpg"] {
        let stored = make_file(&harness, ALICE, None, "photo (1).jpg")
            .await
            .unwrap();
        assert_eq!(stored.name.display(), expected);
        assert!(stored.renamed());
    }

    assert_pairwise_distinct(&names(&harness, "files", ALICE).await);
}

#[tokio::test]
async fn it_naming_insert_namespaces_are_independent() {
    let harness = Harness::open().await;

    assert_eq!(
        make_folder(&harness, ALICE, None, "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs"
    );
    assert_eq!(
        make_file(&harness, ALICE, None, "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs"
    );
    assert_eq!(
        make_folder(&harness, BOB, None, "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs"
    );
    assert_eq!(
        make_file(&harness, BOB, None, "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs"
    );

    let docs = folder_id(&harness, ALICE, "docs").await;
    assert_eq!(
        make_folder(&harness, ALICE, Some(&docs), "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs"
    );
    assert_eq!(
        make_file(&harness, ALICE, Some(&docs), "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs"
    );
    assert_eq!(
        make_folder(&harness, ALICE, None, "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs (1)"
    );
    assert_eq!(
        make_file(&harness, ALICE, None, "docs")
            .await
            .unwrap()
            .name
            .display(),
        "docs (1)"
    );
}

#[tokio::test]
async fn it_naming_insert_case_and_unicode_equivalent_names_collide() {
    let harness = Harness::open().await;

    let expected = [
        ("photo.jpg", "photo.jpg"),
        ("Photo.jpg", "Photo (1).jpg"),
        ("PHOTO.JPG", "PHOTO (2).JPG"),
        ("\u{ff30}hoto.jpg", "\u{ff30}hoto (3).jpg"),
        ("caf\u{e9}", "caf\u{e9}"),
        ("cafe\u{301}", "cafe\u{301} (1)"),
        ("CAFE\u{301}", "CAFE\u{301} (2)"),
    ];
    for (requested, display) in expected {
        let stored = make_file(&harness, ALICE, None, requested).await.unwrap();
        assert_eq!(stored.name.display(), display);
    }
    let rows = names(&harness, "files", ALICE).await;
    assert_eq!(rows.len(), expected.len());
    assert_pairwise_distinct(&rows);
    assert!(rows.iter().any(|(name, _)| name == "PHOTO (2).JPG"));
}

#[tokio::test]
async fn it_naming_insert_trailing_space_collides_with_the_trimmed_name() {
    let harness = Harness::open().await;

    assert_eq!(
        make_file(&harness, ALICE, None, "report")
            .await
            .unwrap()
            .name
            .display(),
        "report"
    );
    let spaced = make_file(&harness, ALICE, None, "report ").await.unwrap();
    assert_eq!(spaced.name.display(), "report  (1)");
    assert_eq!(spaced.name.normalized(), "report  (1)");
    assert_eq!(spaced.attempt, 1);
}

#[tokio::test]
async fn it_naming_insert_16_concurrent_pool_writers_get_pairwise_distinct_names() {
    let harness = Harness::open().await;
    let docs = make_folder(&harness, ALICE, None, "docs").await.unwrap();
    assert_eq!(docs.attempt, 0);
    let docs_id = folder_id(&harness, ALICE, "docs").await;

    for folder in [None, Some(docs_id)] {
        let folder = folder.as_deref();
        let results = futures_util::future::join_all((0..WRITERS).map(|_| async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            make_file(&harness, ALICE, folder, "report.pdf").await
        }))
        .await;
        let mut displays = HashSet::new();
        for stored in results {
            assert!(displays.insert(stored.unwrap().name.normalized().to_owned()));
        }
        assert_eq!(displays.len(), WRITERS);
        let expected: HashSet<String> = std::iter::once("report.pdf".to_owned())
            .chain((1..WRITERS).map(|n| format!("report ({n}).pdf")))
            .collect();
        assert_eq!(displays, expected);
    }
    let per_folder: Vec<(Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT folder_id, count(*), count(DISTINCT name_normalized)
         FROM files WHERE owner_id = ?1 GROUP BY folder_id",
    )
    .bind(ALICE)
    .fetch_all(harness.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(per_folder.len(), 2);
    for (_, total, distinct) in per_folder {
        assert_eq!(total, 16);
        assert_eq!(distinct, 16);
    }
}

#[tokio::test]
async fn it_naming_insert_16_independent_connections_race_without_a_pre_read() {
    let harness = Harness::open().await;
    let attempts = Arc::new(AtomicU32::new(0));
    let options = SqliteConnectOptions::new()
        .filename(harness.pools.path())
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(30));

    let results = futures_util::future::join_all((0..WRITERS).map(|writer| {
        let harness = &harness;
        let options = &options;
        let attempts = &attempts;
        async move {
            let mut connection = SqliteConnection::connect_with(options).await.unwrap();
            let object = insert_object(&mut connection, harness.object_seed())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(u64::try_from(writer % 4).unwrap())).await;
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
            let stored = insert_with_unique_name("report.pdf", async |candidate: NameCandidate| {
                attempts.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
                try_insert_file(&mut tx, ALICE, None, &object, &candidate).await
            })
            .await
            .unwrap();
            tx.commit().await.unwrap();
            stored.name.normalized().to_owned()
        }
    }))
    .await;
    let mut normalized = HashSet::new();
    for name in results {
        assert!(normalized.insert(name));
    }
    assert_eq!(normalized.len(), WRITERS);
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        u32::try_from(WRITERS * (WRITERS + 1) / 2).unwrap(),
        "each writer tries exactly the names already taken plus its own"
    );
    let rows = names(&harness, "files", ALICE).await;
    assert_eq!(rows.len(), WRITERS);
    assert_pairwise_distinct(&rows);
}

#[tokio::test]
async fn it_naming_insert_does_not_retry_unrelated_constraint_failures() {
    let harness = Harness::open().await;
    make_file(&harness, ALICE, None, "taken.txt").await.unwrap();

    let calls = AtomicU32::new(0);
    let foreign_key = harness
        .pools
        .write_tx(&harness.clock, "naming.test_fk", async |tx| {
            insert_with_unique_name("fresh.txt", async |candidate: NameCandidate| {
                calls.fetch_add(1, Ordering::Relaxed);
                try_insert_folder(tx.executor(), "user-missing", None, &candidate).await
            })
            .await
        })
        .await;
    match foreign_key {
        Err(NamedInsertError::Failed(error)) => {
            assert_eq!(error.kind(), DbErrorKind::ForeignKeyViolation);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    calls.store(0, Ordering::Relaxed);
    let check = harness
        .pools
        .write_tx(&harness.clock, "naming.test_check", async |tx| {
            insert_with_unique_name("fresh", async |candidate: NameCandidate| {
                calls.fetch_add(1, Ordering::Relaxed);
                sqlx::query(
                    "INSERT INTO folders
                         (id, owner_id, parent_id, name, name_normalized, depth,
                          created_at, updated_at)
                     VALUES (?1, ?2, NULL, ?3, ?4, 99, ?5, ?5)
                     ON CONFLICT (owner_id, name_normalized) WHERE parent_id IS NULL DO NOTHING",
                )
                .bind(Uuid::now_v7().to_string())
                .bind(ALICE)
                .bind(candidate.display())
                .bind(candidate.normalized())
                .bind(NOW)
                .execute(tx.executor())
                .await
                .map_err(DbError::from)
                .map(|result| Attempt::from_insert(&result))
            })
            .await
        })
        .await;
    match check {
        Err(NamedInsertError::Failed(error)) => {
            assert_eq!(error.kind(), DbErrorKind::CheckViolation);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    calls.store(0, Ordering::Relaxed);
    let unrelated_unique = harness
        .pools
        .write_tx(&harness.clock, "naming.test_unrelated_unique", async |tx| {
            let object: String = sqlx::query_scalar(
                "SELECT storage_object_id FROM files WHERE owner_id = ?1 AND name = 'taken.txt'",
            )
            .bind(ALICE)
            .fetch_one(tx.executor())
            .await
            .map_err(DbError::from)
            .map_err(NamedInsertError::Failed)?;
            insert_with_unique_name("other.txt", async |candidate: NameCandidate| {
                calls.fetch_add(1, Ordering::Relaxed);
                try_insert_file(tx.executor(), ALICE, None, &object, &candidate).await
            })
            .await
        })
        .await;
    match unrelated_unique {
        Err(NamedInsertError::Failed(error)) => {
            assert_eq!(error.kind(), DbErrorKind::UniqueViolation);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    let rows = names(&harness, "files", ALICE).await;
    assert_eq!(rows, [("taken.txt".to_owned(), "taken.txt".to_owned())]);
}

#[tokio::test]
async fn it_naming_insert_exhaustion_after_one_thousand_suffixed_attempts() {
    let harness = Harness::open().await;
    harness
        .pools
        .write_tx(&harness.clock, "naming.test_fill", async |tx| {
            sqlx::query(
                "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < 1000)
                 INSERT INTO folders
                     (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
                 SELECT 'folder-' || i, ?1,
                        NULL,
                        CASE WHEN i = 0 THEN 'crowded' ELSE 'crowded (' || i || ')' END,
                        CASE WHEN i = 0 THEN 'crowded' ELSE 'crowded (' || i || ')' END,
                        0, ?2, ?2
                 FROM n",
            )
            .bind(ALICE)
            .bind(NOW)
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
            Ok::<(), DbError>(())
        })
        .await
        .unwrap();

    let calls = AtomicU32::new(0);
    let outcome = harness
        .pools
        .write_tx(&harness.clock, "naming.test_exhaust", async |tx| {
            insert_with_unique_name("crowded", async |candidate: NameCandidate| {
                calls.fetch_add(1, Ordering::Relaxed);
                try_insert_folder(tx.executor(), ALICE, None, &candidate).await
            })
            .await
        })
        .await;
    assert!(
        matches!(
            outcome,
            Err(NamedInsertError::Conflict(
                CandidateError::AttemptsExhausted
            ))
        ),
        "{outcome:?}"
    );
    assert_eq!(calls.load(Ordering::Relaxed), MAX_NAME_ATTEMPTS + 1);
    assert_eq!(names(&harness, "folders", ALICE).await.len(), 1001);

    let free = make_folder(&harness, ALICE, None, "crowded (1)")
        .await
        .unwrap();
    assert_eq!(free.name.display(), "crowded (1) (1)");
}

#[tokio::test]
async fn unit_naming_insert_loop_bounds_without_a_database() {
    let calls = AtomicU32::new(0);
    let outcome: Result<Stored<()>, NamedInsertError<()>> =
        insert_with_unique_name("x.txt", async |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(Attempt::NameTaken)
        })
        .await;
    assert_eq!(
        outcome,
        Err(NamedInsertError::Conflict(
            CandidateError::AttemptsExhausted
        ))
    );
    assert_eq!(calls.load(Ordering::Relaxed), MAX_NAME_ATTEMPTS + 1);

    let seen = std::sync::Mutex::new(Vec::new());
    let stored: Result<Stored<u8>, NamedInsertError<()>> =
        insert_with_unique_name("archive.tar.gz", async |candidate: NameCandidate| {
            let mut seen = seen.lock().unwrap();
            seen.push(candidate.display().to_owned());
            Ok(if seen.len() < 3 {
                Attempt::NameTaken
            } else {
                Attempt::Stored(7)
            })
        })
        .await;
    let stored = stored.unwrap();
    assert_eq!(stored.value, 7);
    assert_eq!(stored.attempt, 2);
    assert!(stored.renamed());
    assert_eq!(
        *seen.lock().unwrap(),
        ["archive.tar.gz", "archive.tar (1).gz", "archive.tar (2).gz"]
    );
}

#[tokio::test]
async fn unit_naming_insert_invalid_names_never_reach_the_database() {
    let calls = AtomicU32::new(0);
    for (name, expected) in [
        ("", InvalidName::Empty),
        ("..", InvalidName::Reserved),
        ("a/b", InvalidName::Separator),
        ("a\u{0}b", InvalidName::Control),
    ] {
        let outcome: Result<Stored<()>, NamedInsertError<()>> =
            insert_with_unique_name(name, async |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(Attempt::Stored(()))
            })
            .await;
        assert_eq!(
            outcome,
            Err(NamedInsertError::InvalidName(expected)),
            "{name:?}"
        );
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn it_naming_insert_maximum_length_collisions_keep_both_by_truncating_the_base() {
    let harness = Harness::open().await;

    let ascii = "x".repeat(MAX_NAME_BYTES);
    let first = make_file(&harness, ALICE, None, &ascii).await.unwrap();
    assert_eq!(first.name.display(), ascii);
    for (attempt, suffix) in [(1, " (1)"), (2, " (2)")] {
        let stored = make_file(&harness, ALICE, None, &ascii).await.unwrap();
        assert_eq!(stored.attempt, attempt);
        assert_eq!(
            stored.name.display(),
            format!("{}{suffix}", "x".repeat(MAX_NAME_BYTES - suffix.len()))
        );
        assert_eq!(stored.name.display().len(), MAX_NAME_BYTES);
    }

    let emoji = format!("{}abc", "\u{1f4e6}".repeat(63));
    assert_eq!(emoji.len(), MAX_NAME_BYTES);
    make_file(&harness, ALICE, None, &emoji).await.unwrap();
    let stored = make_file(&harness, ALICE, None, &emoji).await.unwrap();
    assert_eq!(
        stored.name.display(),
        format!("{} (1)", "\u{1f4e6}".repeat(62))
    );
    assert!(stored.name.display().len() <= MAX_NAME_BYTES);

    let with_extension = format!("{}.txt", "y".repeat(MAX_NAME_BYTES - 4));
    make_file(&harness, ALICE, None, &with_extension)
        .await
        .unwrap();
    let stored = make_file(&harness, ALICE, None, &with_extension)
        .await
        .unwrap();
    assert_eq!(
        stored.name.display(),
        format!("{} (1).txt", "y".repeat(MAX_NAME_BYTES - 8))
    );
    assert_eq!(stored.name.display().len(), MAX_NAME_BYTES);

    let rows = names(&harness, "files", ALICE).await;
    assert_eq!(rows.len(), 7);
    assert_pairwise_distinct(&rows);
    for (name, _) in rows {
        assert!(name.len() <= MAX_NAME_BYTES);
    }
}

#[tokio::test]
async fn it_naming_insert_unrepresentable_collision_is_a_conflict_and_invalid_input_is_not() {
    let harness = Harness::open().await;

    let squeezed = format!("a.{}", "e".repeat(MAX_NAME_BYTES - 2));
    assert_eq!(squeezed.len(), MAX_NAME_BYTES);
    let first = make_file(&harness, ALICE, None, &squeezed).await.unwrap();
    assert_eq!(first.name.display(), squeezed);
    let second = make_file(&harness, ALICE, None, &squeezed).await;
    assert!(
        matches!(
            second,
            Err(NamedInsertError::Conflict(CandidateError::DoesNotFit))
        ),
        "{second:?}"
    );

    let too_long = make_file(&harness, ALICE, None, &"z".repeat(MAX_NAME_BYTES + 1)).await;
    assert!(
        matches!(
            too_long,
            Err(NamedInsertError::InvalidName(InvalidName::TooLong))
        ),
        "{too_long:?}"
    );
    assert_eq!(names(&harness, "files", ALICE).await.len(), 1);
}

#[test]
fn unit_naming_insert_errors_map_to_the_canonical_catalog_codes() {
    for invalid in [
        InvalidName::Empty,
        InvalidName::Reserved,
        InvalidName::Separator,
        InvalidName::Control,
        InvalidName::TooLong,
        InvalidName::NormalizedOutOfRange,
    ] {
        let error = ApiError::from(NamedInsertError::<DbError>::InvalidName(invalid));
        assert_eq!(error.code(), ErrorCode::NameInvalid);
        assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!error.retryable());
        assert_eq!(ApiError::from(invalid).code(), ErrorCode::NameInvalid);
    }
    for conflict in [
        CandidateError::AttemptsExhausted,
        CandidateError::DoesNotFit,
    ] {
        let error = ApiError::from(NamedInsertError::<DbError>::Conflict(conflict));
        assert_eq!(error.code(), ErrorCode::FileNameConflict);
        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert!(!error.retryable());
        assert_eq!(ApiError::from(conflict).code(), ErrorCode::FileNameConflict);
    }
    let busy = ApiError::from(NamedInsertError::Failed(DbError::Busy(
        sqlx::Error::PoolTimedOut,
    )));
    assert_eq!(busy.code(), ErrorCode::DatabaseBusy);
}

#[tokio::test]
async fn it_naming_insert_exhaustion_and_invalid_names_surface_as_api_errors() {
    let harness = Harness::open().await;
    let invalid = make_file(&harness, ALICE, None, "a/b").await.unwrap_err();
    assert_eq!(ApiError::from(invalid).code(), ErrorCode::NameInvalid);

    let exhausted: Result<Stored<()>, NamedInsertError<DbError>> =
        insert_with_unique_name("x.txt", async |_| Ok(Attempt::NameTaken)).await;
    assert_eq!(
        ApiError::from(exhausted.unwrap_err()).code(),
        ErrorCode::FileNameConflict
    );
}

#[tokio::test]
async fn it_naming_insert_received_files_share_one_flat_namespace() {
    let harness = Harness::open().await;
    harness
        .pools
        .write_tx(&harness.clock, "naming.test_reverse_share", async |tx| {
            sqlx::query(
                "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
                 VALUES ('rs-1', ?1, 'public-id-0123456789', 'inbox', ?2, ?2)",
            )
            .bind(ALICE)
            .bind(NOW)
            .execute(tx.executor())
            .await
            .map_err(DbError::from)?;
            Ok::<(), DbError>(())
        })
        .await
        .unwrap();

    for (requested, expected) in [
        ("invoice.pdf", "invoice.pdf"),
        ("invoice.pdf", "invoice (1).pdf"),
        ("Invoice.PDF", "Invoice (2).PDF"),
    ] {
        let stored = harness
            .pools
            .write_tx(&harness.clock, "naming.test_received_insert", async |tx| {
                let object = insert_object(tx.executor(), harness.object_seed()).await?;
                insert_with_unique_name(requested, async |candidate: NameCandidate| {
                    let sql = format!(
                        "INSERT INTO received_files
                             (id, owner_id, reverse_share_id, storage_object_id, name,
                              name_normalized, size_bytes, received_at, updated_at)
                         VALUES (?1, ?2, 'rs-1', ?3, ?4, ?5, 0, ?6, ?6) {}",
                        NameNamespace::ReceivedFiles.conflict_clause()
                    );
                    let result = sqlx::query(&sql)
                        .bind(Uuid::now_v7().to_string())
                        .bind(ALICE)
                        .bind(&object)
                        .bind(candidate.display())
                        .bind(candidate.normalized())
                        .bind(NOW)
                        .execute(tx.executor())
                        .await
                        .map_err(DbError::from)?;
                    Ok::<_, DbError>(Attempt::from_insert(&result))
                })
                .await
            })
            .await
            .unwrap();
        assert_eq!(stored.name.display(), expected);
    }
}

#[tokio::test]
async fn it_naming_insert_rename_by_update_retries_only_the_name_collision() {
    let harness = Harness::open().await;
    make_file(&harness, ALICE, None, "photo.jpg").await.unwrap();
    make_file(&harness, ALICE, None, "photo (1).jpg")
        .await
        .unwrap();
    make_file(&harness, ALICE, None, "draft.jpg").await.unwrap();

    let renamed = harness
        .pools
        .write_tx(&harness.clock, "naming.test_rename", async |tx| {
            insert_with_unique_name("Photo.JPG", async |candidate: NameCandidate| {
                let result = sqlx::query(
                    "UPDATE files SET name = ?1, name_normalized = ?2
                     WHERE owner_id = ?3 AND name = 'draft.jpg'",
                )
                .bind(candidate.display())
                .bind(candidate.normalized())
                .bind(ALICE)
                .execute(tx.executor())
                .await;
                Attempt::from_name_update(result)
            })
            .await
        })
        .await
        .unwrap();
    assert_eq!(renamed.name.display(), "Photo (2).JPG");
    assert_eq!(renamed.attempt, 2);

    let docs = make_folder(&harness, ALICE, None, "docs").await.unwrap();
    assert_eq!(docs.attempt, 0);
    let docs_id = folder_id(&harness, ALICE, "docs").await;
    make_file(&harness, ALICE, Some(&docs_id), "Photo (2).JPG")
        .await
        .unwrap();

    let moved = harness
        .pools
        .write_tx(&harness.clock, "naming.test_move", async |tx| {
            insert_with_unique_name("Photo (2).JPG", async |candidate: NameCandidate| {
                let result = sqlx::query(
                    "UPDATE files SET folder_id = ?1, name = ?2, name_normalized = ?3
                     WHERE owner_id = ?4 AND name = 'Photo (2).JPG' AND folder_id IS NULL",
                )
                .bind(&docs_id)
                .bind(candidate.display())
                .bind(candidate.normalized())
                .bind(ALICE)
                .execute(tx.executor())
                .await;
                Attempt::from_name_update(result)
            })
            .await
        })
        .await
        .unwrap();
    assert_eq!(moved.name.display(), "Photo (2) (1).JPG");

    let calls = AtomicU32::new(0);
    let foreign_key = harness
        .pools
        .write_tx(&harness.clock, "naming.test_move_fk", async |tx| {
            insert_with_unique_name("draft.jpg", async |candidate: NameCandidate| {
                calls.fetch_add(1, Ordering::Relaxed);
                let result = sqlx::query(
                    "UPDATE files SET folder_id = 'folder-missing', name = ?1, name_normalized = ?2
                     WHERE owner_id = ?3 AND name = 'photo.jpg'",
                )
                .bind(candidate.display())
                .bind(candidate.normalized())
                .bind(ALICE)
                .execute(tx.executor())
                .await;
                Attempt::from_name_update(result)
            })
            .await
        })
        .await;
    match foreign_key {
        Err(NamedInsertError::Failed(error)) => {
            assert_eq!(error.kind(), DbErrorKind::ForeignKeyViolation);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_pairwise_distinct(&names(&harness, "files", ALICE).await);
}

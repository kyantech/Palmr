pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{IndexColumn, Schema};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00.000Z";
const LATER: &str = "2026-01-01T00:05:00.000Z";
const RESTRICT_VIOLATION: &str = "1811";
const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";
const FILES_FTS: &str = "CREATE VIRTUAL TABLE files_fts USING fts5( name, description, \
    content = 'files', content_rowid = 'rowid', \
    tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3 4' )";

macro_rules! arguments {
    ($($value:expr),* $(,)?) => {{
        let mut arguments = SqliteArguments::default();
        $(sqlx::Arguments::add(&mut arguments, $value).expect("bind argument");)*
        arguments
    }};
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Unique,
    Foreign,
}

use Outcome::{Accepted, Foreign, Unique};

struct MigratedDatabase {
    application: TestApplication,
    connection: SqliteConnection,
}

impl MigratedDatabase {
    async fn open(test_name: &str) -> Result<Self> {
        let application = TestApplication::start(test_name).await?;
        let connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(application.data_dir().join(DATABASE_FILE))
                .foreign_keys(true),
        )
        .await
        .context("open migrated database for writing")?;
        let mut database = Self {
            application,
            connection,
        };
        for (id, username) in [(ALICE, "alice"), (BOB, "bob")] {
            assert_eq!(
                database
                    .run(
                        "INSERT INTO users
                             (id, email, email_normalized, username, username_normalized,
                              created_at, updated_at)
                         VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?4)",
                        arguments![id, format!("{username}@example.test"), username, NOW],
                    )
                    .await?,
                Accepted
            );
        }
        Ok(database)
    }

    async fn run(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<Outcome> {
        match sqlx::query_with(sql, arguments)
            .execute(&mut self.connection)
            .await
        {
            Ok(_) => Ok(Accepted),
            Err(sqlx::Error::Database(error)) => match error.kind() {
                ErrorKind::UniqueViolation => Ok(Unique),
                ErrorKind::ForeignKeyViolation => Ok(Foreign),
                ErrorKind::Other if error.code().as_deref() == Some(RESTRICT_VIOLATION) => {
                    Ok(Foreign)
                }
                kind => Err(anyhow::anyhow!("unexpected {kind:?}: {error}")),
            },
            Err(error) => Err(error.into()),
        }
    }

    async fn strings(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar_with(sql, arguments)
            .fetch_all(&mut self.connection)
            .await?)
    }

    async fn total_changes(&mut self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT total_changes()")
            .fetch_one(&mut self.connection)
            .await?)
    }

    async fn folder(
        &mut self,
        id: &str,
        owner: &str,
        parent: Option<&str>,
        name: &str,
        name_normalized: &str,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO folders
                 (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     COALESCE((SELECT depth + 1 FROM folders WHERE id = ?3), 0), ?6, ?6)",
            arguments![id, owner, parent, name, name_normalized, NOW],
        )
        .await
    }

    async fn object(&mut self, seed: u64) -> Result<String> {
        let id = format!("object-{seed}");
        assert_eq!(
            self.run(
                "INSERT INTO storage_objects
                     (id, object_key, provider, size_bytes, state, refcount,
                      created_at, updated_at, finalized_at)
                 VALUES (?1, ?2, 'local', 0, 'active', 1, ?3, ?3, ?3)",
                arguments![id.as_str(), format!("objects/00/00/{seed:032x}"), NOW],
            )
            .await?,
            Accepted
        );
        Ok(id)
    }

    async fn file(&mut self, file: File<'_>) -> Result<Outcome> {
        self.run(
            "INSERT INTO files
                 (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                  extension, description, size_bytes, mime_type, mime_source,
                  created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 'sniffed', ?10, ?10)",
            arguments![
                file.id,
                file.owner,
                file.folder,
                file.object,
                file.name,
                file.name_normalized,
                file.extension,
                file.description,
                file.mime_type,
                NOW,
            ],
        )
        .await
    }

    async fn hits(&mut self, query: &str) -> Result<Vec<String>> {
        self.strings(
            "SELECT f.id
               FROM files_fts
               JOIN files f ON f.rowid = files_fts.rowid
              WHERE files_fts MATCH ?1
              ORDER BY f.id",
            arguments![query],
        )
        .await
    }

    async fn assert_index_consistent(&mut self) -> Result<()> {
        sqlx::query("INSERT INTO files_fts(files_fts, rank) VALUES ('integrity-check', 1)")
            .execute(&mut self.connection)
            .await
            .context("files_fts integrity-check against files")?;
        Ok(())
    }

    async fn close(self) -> Result<()> {
        self.connection.close().await?;
        self.application.shutdown().await;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct File<'a> {
    id: &'a str,
    owner: &'a str,
    folder: Option<&'a str>,
    object: &'a str,
    name: &'a str,
    name_normalized: &'a str,
    extension: &'a str,
    description: Option<&'a str>,
    mime_type: &'a str,
}

impl<'a> File<'a> {
    const fn new(
        id: &'a str,
        owner: &'a str,
        folder: Option<&'a str>,
        object: &'a str,
        name: &'a str,
        name_normalized: &'a str,
    ) -> Self {
        Self {
            id,
            owner,
            folder,
            object,
            name,
            name_normalized,
            extension: "",
            description: None,
            mime_type: "application/octet-stream",
        }
    }
}

fn compact(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn columns(names: &[&str]) -> Vec<IndexColumn> {
    names
        .iter()
        .map(|name| IndexColumn {
            name: Some((*name).to_owned()),
            collation: "BINARY".to_owned(),
        })
        .collect()
}

fn assert_partial_unique(schema: &Schema, table: &str, index: &str, expected: &[&str]) {
    let found = schema
        .table(table)
        .and_then(|table| {
            table
                .indexes
                .iter()
                .find(|candidate| candidate.name == index)
        })
        .unwrap_or_else(|| panic!("{table}.{index} missing"));
    assert!(
        found.unique && found.partial,
        "{index} must be partial unique"
    );
    assert_eq!(found.columns, columns(expected), "{index} columns");
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_sibling_name_unique_root_and_folder() -> Result<()> {
    let mut database =
        MigratedDatabase::open("it_schema_sibling_name_unique_root_and_folder").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_partial_unique(
        &schema,
        "folders",
        "ux_folders_sibling_name",
        &["owner_id", "parent_id", "name_normalized"],
    );
    assert_partial_unique(
        &schema,
        "folders",
        "ux_folders_root_name",
        &["owner_id", "name_normalized"],
    );
    assert_partial_unique(
        &schema,
        "files",
        "ux_files_sibling_name",
        &["owner_id", "folder_id", "name_normalized"],
    );
    assert_partial_unique(
        &schema,
        "files",
        "ux_files_root_name",
        &["owner_id", "name_normalized"],
    );

    let db = &mut database;
    assert_eq!(
        db.folder("projects", ALICE, None, "Projects", "projects")
            .await?,
        Accepted
    );
    assert_eq!(
        db.folder("projects-dup", ALICE, None, "PROJECTS", "projects")
            .await?,
        Unique
    );
    assert_eq!(
        db.folder("archive", ALICE, None, "Archive", "archive")
            .await?,
        Accepted
    );
    assert_eq!(
        db.folder("drafts", ALICE, Some("projects"), "Drafts", "drafts")
            .await?,
        Accepted
    );
    assert_eq!(
        db.folder("drafts-dup", ALICE, Some("projects"), "DRAFTS", "drafts")
            .await?,
        Unique
    );
    assert_eq!(
        db.folder("archive-drafts", ALICE, Some("archive"), "drafts", "drafts")
            .await?,
        Accepted
    );
    assert_eq!(
        db.folder("root-drafts", ALICE, None, "Drafts", "drafts")
            .await?,
        Accepted
    );
    assert_eq!(
        db.folder("bob-projects", BOB, None, "projects", "projects")
            .await?,
        Accepted
    );
    assert_eq!(
        db.folder("bob-drafts", BOB, Some("bob-projects"), "Drafts", "drafts")
            .await?,
        Accepted
    );

    for (seed, (id, owner, folder, name, expected)) in (1..).zip([
        ("root", ALICE, None, "Report.pdf", Accepted),
        ("root-dup", ALICE, None, "REPORT.PDF", Unique),
        (
            "in-projects",
            ALICE,
            Some("projects"),
            "report.pdf",
            Accepted,
        ),
        (
            "in-projects-dup",
            ALICE,
            Some("projects"),
            "Report.PDF",
            Unique,
        ),
        ("in-archive", ALICE, Some("archive"), "Report.pdf", Accepted),
        ("in-drafts", ALICE, Some("drafts"), "Report.pdf", Accepted),
        ("bob-root", BOB, None, "Report.pdf", Accepted),
        (
            "bob-in-projects",
            BOB,
            Some("bob-projects"),
            "Report.pdf",
            Accepted,
        ),
    ]) {
        let object = db.object(seed).await?;
        let row = File::new(id, owner, folder, &object, name, "report.pdf");
        assert_eq!(db.file(row).await?, expected, "file {id}");
    }
    assert_eq!(
        db.strings(
            "SELECT id FROM files WHERE name_normalized = 'report.pdf' AND folder_id IS NULL
              ORDER BY id",
            SqliteArguments::default(),
        )
        .await?,
        ["bob-root", "root"]
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_files_storage_object_unique() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_files_storage_object_unique").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    let files = schema.table("files").expect("files");
    let index = files
        .indexes
        .iter()
        .find(|index| index.name == "ux_files_storage_object")
        .expect("ux_files_storage_object");
    assert!(index.unique && !index.partial);
    assert_eq!(index.columns, columns(&["storage_object_id"]));

    let first = database.object(1).await?;
    let second = database.object(2).await?;
    assert_eq!(
        database
            .file(File::new("a", ALICE, None, &first, "a.bin", "a.bin"))
            .await?,
        Accepted
    );
    assert_eq!(
        database
            .file(File::new("b", ALICE, None, &first, "b.bin", "b.bin"))
            .await?,
        Unique
    );
    assert_eq!(
        database
            .file(File::new("b-bob", BOB, None, &first, "b.bin", "b.bin"))
            .await?,
        Unique
    );
    assert_eq!(
        database
            .file(File::new("b", ALICE, None, &second, "b.bin", "b.bin"))
            .await?,
        Accepted
    );
    assert_eq!(
        database
            .file(File::new(
                "c",
                ALICE,
                None,
                "object-missing",
                "c.bin",
                "c.bin"
            ))
            .await?,
        Foreign
    );
    assert_eq!(
        database
            .run(
                "DELETE FROM storage_objects WHERE id = ?1",
                arguments![first.as_str()]
            )
            .await?,
        Foreign
    );
    assert_eq!(
        database
            .run(
                "UPDATE files SET storage_object_id = ?1 WHERE id = 'a'",
                arguments![second.as_str()]
            )
            .await?,
        Unique
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_files_fts_triggers() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_files_fts_triggers").await?;
    let sql = database
        .strings(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'files_fts'",
            SqliteArguments::default(),
        )
        .await?;
    assert_eq!(
        sql.iter().map(|sql| compact(sql)).collect::<Vec<_>>(),
        [compact(FILES_FTS)]
    );
    assert_eq!(
        database
            .strings(
                "SELECT name FROM pragma_table_info('files_fts') ORDER BY cid",
                SqliteArguments::default()
            )
            .await?,
        ["name", "description"]
    );
    assert_eq!(
        database
            .strings(
                "SELECT name FROM sqlite_schema WHERE type = 'trigger' AND tbl_name = 'files'
                  ORDER BY name",
                SqliteArguments::default(),
            )
            .await?,
        ["files_fts_ad", "files_fts_ai", "files_fts_au"]
    );

    assert_eq!(
        database
            .folder("travel", ALICE, None, "Travel", "travel")
            .await?,
        Accepted
    );
    assert_eq!(
        database
            .folder("europe", ALICE, Some("travel"), "Europe", "europe")
            .await?,
        Accepted
    );
    let root_object = database.object(1).await?;
    let nested_object = database.object(2).await?;
    let mut root = File::new(
        "root",
        ALICE,
        None,
        &root_object,
        "Quarterly Budget.xlsx",
        "quarterly budget.xlsx",
    );
    root.extension = "xlsx";
    root.mime_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    let mut nested = File::new(
        "nested",
        ALICE,
        Some("europe"),
        &nested_object,
        "Férias Lisboa.jpg",
        "férias lisboa.jpg",
    );
    nested.extension = "jpg";
    nested.description = Some("sunset over tagus");
    nested.mime_type = "image/jpeg";

    assert_eq!(database.hits("quarterly").await?, Vec::<String>::new());
    assert_eq!(database.file(root).await?, Accepted);
    assert_eq!(database.file(nested).await?, Accepted);
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("quarterly").await?, ["root"]);
    assert_eq!(database.hits("qua*").await?, ["root"]);
    assert_eq!(database.hits("lisboa").await?, ["nested"]);
    assert_eq!(database.hits("ferias").await?, ["nested"]);
    assert_eq!(database.hits("sunset").await?, ["nested"]);
    assert_eq!(database.hits("description : tagus").await?, ["nested"]);
    assert_eq!(database.hits("name : tagus").await?, Vec::<String>::new());

    let before = database.total_changes().await?;
    assert_eq!(
        database
            .run(
                "UPDATE files SET mime_type = 'text/plain', size_bytes = 42, updated_at = ?1
                  WHERE id = 'root'",
                arguments![LATER],
            )
            .await?,
        Accepted
    );
    assert_eq!(database.total_changes().await? - before, 1);
    database.assert_index_consistent().await?;

    let before = database.total_changes().await?;
    assert_eq!(
        database
            .run(
                "UPDATE files SET name = 'Annual Forecast.xlsx',
                                  name_normalized = 'annual forecast.xlsx', updated_at = ?1
                  WHERE id = 'root'",
                arguments![LATER],
            )
            .await?,
        Accepted
    );
    assert!(database.total_changes().await? - before > 1);
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("quarterly").await?, Vec::<String>::new());
    assert_eq!(database.hits("budget").await?, Vec::<String>::new());
    assert_eq!(database.hits("forecast").await?, ["root"]);

    assert_eq!(
        database
            .run(
                "UPDATE files SET description = 'board review draft', updated_at = ?1
                  WHERE id = 'root'",
                arguments![LATER],
            )
            .await?,
        Accepted
    );
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("board").await?, ["root"]);
    assert_eq!(
        database
            .run(
                "UPDATE files SET description = NULL, updated_at = ?1 WHERE id = 'root'",
                arguments![LATER],
            )
            .await?,
        Accepted
    );
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("board").await?, Vec::<String>::new());
    assert_eq!(database.hits("forecast").await?, ["root"]);

    assert_eq!(
        database
            .run(
                "UPDATE files SET folder_id = 'travel', updated_at = ?1 WHERE id = 'nested'",
                arguments![LATER],
            )
            .await?,
        Accepted
    );
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("lisboa").await?, ["nested"]);

    for absent in [
        "spreadsheetml",
        "openxmlformats",
        "jpeg",
        "image",
        "objects",
        "alice",
        "user",
        "travel",
        "europe",
        "sniffed",
    ] {
        assert_eq!(
            database.hits(absent).await?,
            Vec::<String>::new(),
            "{absent}"
        );
    }

    sqlx::query("CREATE VIRTUAL TABLE temp.files_fts_terms USING fts5vocab(main, files_fts, col)")
        .execute(&mut database.connection)
        .await?;
    let mut indexed = database
        .strings(
            "SELECT DISTINCT col || ':' || term FROM temp.files_fts_terms ORDER BY 1",
            SqliteArguments::default(),
        )
        .await?;
    indexed.sort();
    assert_eq!(
        indexed,
        [
            "description:over",
            "description:sunset",
            "description:tagus",
            "name:annual",
            "name:ferias",
            "name:forecast",
            "name:jpg",
            "name:lisboa",
            "name:xlsx",
        ]
    );

    assert_eq!(
        database
            .run(
                "DELETE FROM files WHERE id = 'nested'",
                SqliteArguments::default()
            )
            .await?,
        Accepted
    );
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("lisboa").await?, Vec::<String>::new());
    assert_eq!(database.hits("sunset").await?, Vec::<String>::new());
    assert_eq!(
        database
            .run(
                "DELETE FROM files WHERE id = 'root'",
                SqliteArguments::default()
            )
            .await?,
        Accepted
    );
    database.assert_index_consistent().await?;
    assert_eq!(database.hits("forecast").await?, Vec::<String>::new());
    assert_eq!(
        database
            .strings(
                "SELECT term FROM temp.files_fts_terms",
                SqliteArguments::default()
            )
            .await?,
        Vec::<String>::new()
    );
    database.close().await
}

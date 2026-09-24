pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{byte_owner_cascades, nocase_collations, IndexColumn, Schema};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00.000Z";
const LATER: &str = "2026-01-02T00:00:00.000Z";
const RESTRICT_VIOLATION: &str = "1811";
const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";
const RECEIVED_FILES_FTS: &str = "CREATE VIRTUAL TABLE received_files_fts USING fts5( name, \
    description, content = 'received_files', content_rowid = 'rowid', \
    tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3 4' )";
const LOCALES: [&str; 23] = [
    "ar-SA", "de-DE", "el-GR", "en-US", "es-ES", "fa-IR", "fr-FR", "he-IL", "hi-IN", "id-ID",
    "it-IT", "ja-JP", "ko-KR", "nl-NL", "pl-PL", "pt-BR", "ru-RU", "sv-SE", "th-TH", "tr-TR",
    "uk-UA", "vi-VN", "zh-CN",
];
const BRANDING_KINDS: [&str; 5] = [
    "logo",
    "favicon",
    "login_background",
    "email_logo",
    "og_default_image",
];

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
    Check,
    Unique,
    Foreign,
}

use Outcome::{Accepted, Check, Foreign, Unique};

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
                ErrorKind::CheckViolation => Ok(Check),
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

    async fn exec(&mut self, sql: &str) -> Result<Outcome> {
        self.run(sql, SqliteArguments::default()).await
    }

    async fn strings(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar_with(sql, arguments)
            .fetch_all(&mut self.connection)
            .await?)
    }

    async fn count(&mut self, sql: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(sql)
            .fetch_one(&mut self.connection)
            .await?)
    }

    async fn object_at(&mut self, id: &str, key: &str) -> Result<()> {
        assert_eq!(
            self.run(
                "INSERT INTO storage_objects
                     (id, object_key, provider, size_bytes, state, refcount,
                      created_at, updated_at, finalized_at)
                 VALUES (?1, ?2, 'local', 1, 'active', 1, ?3, ?3, ?3)",
                arguments![id, key, NOW],
            )
            .await?,
            Accepted,
            "{key}"
        );
        Ok(())
    }

    async fn object(&mut self, seed: u64) -> Result<String> {
        let id = format!("object-{seed}");
        self.object_at(&id, &format!("objects/00/00/{seed:032x}"))
            .await?;
        Ok(id)
    }

    async fn share(&mut self, id: &str, alias: &str) -> Result<Outcome> {
        self.run(
            "INSERT INTO shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            arguments![id, ALICE, format!("share-{id:0>16}"), alias, NOW],
        )
        .await
    }

    async fn reverse_share(&mut self, id: &str, owner: &str, alias: &str) -> Result<Outcome> {
        self.run(
            "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            arguments![id, owner, format!("rs-{id:0>16}"), alias, NOW],
        )
        .await
    }

    async fn session(&mut self, session: Session<'_>) -> Result<Outcome> {
        self.run(
            "INSERT INTO reverse_share_upload_sessions
                 (id, reverse_share_id, token_hash, uploader_email, uploader_email_normalized,
                  locale, created_at, expires_at, last_activity_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?7)",
            arguments![
                session.id,
                session.reverse_share,
                session.token_hash,
                session.email,
                session.email_normalized,
                session.locale,
                NOW,
                LATER,
            ],
        )
        .await
    }

    async fn asset(&mut self, asset: Asset<'_>) -> Result<Outcome> {
        self.run(
            "INSERT INTO reverse_share_assets
                 (id, reverse_share_id, storage_object_id, mime_type, size_bytes, width, height,
                  created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8)",
            arguments![
                asset.id,
                asset.reverse_share,
                asset.object,
                asset.mime_type,
                asset.width,
                asset.height,
                NOW,
                asset.created_by,
            ],
        )
        .await
    }

    async fn received(&mut self, received: Received<'_>) -> Result<Outcome> {
        self.run(
            "INSERT INTO received_files
                 (id, owner_id, reverse_share_id, upload_session_id, storage_object_id,
                  name, name_normalized, description, size_bytes, mime_type,
                  uploader_name, uploader_email, uploader_ip, received_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12, ?13, ?13)",
            arguments![
                received.id,
                received.owner,
                received.reverse_share,
                received.session,
                received.object,
                received.name,
                received.name.to_lowercase(),
                received.description,
                received.mime_type,
                received.uploader_name,
                received.uploader_email,
                received.uploader_ip,
                NOW,
            ],
        )
        .await
    }

    async fn branding(
        &mut self,
        id: &str,
        kind: &str,
        object: &str,
        mime_type: &str,
        is_current: i64,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO branding_assets
                 (id, kind, storage_object_id, mime_type, size_bytes, width, height, is_current,
                  created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, 1, 512, 512, ?5, ?6, ?7)",
            arguments![id, kind, object, mime_type, is_current, NOW, ALICE],
        )
        .await
    }

    async fn received_hits(&mut self, query: &str) -> Result<Vec<String>> {
        self.strings(
            "SELECT r.id
               FROM received_files_fts
               JOIN received_files r ON r.rowid = received_files_fts.rowid
              WHERE received_files_fts MATCH ?1
              ORDER BY r.id",
            arguments![query],
        )
        .await
    }

    async fn file_hits(&mut self, query: &str) -> Result<Vec<String>> {
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

    async fn assert_received_index_consistent(&mut self) -> Result<()> {
        sqlx::query(
            "INSERT INTO received_files_fts(received_files_fts, rank) VALUES ('integrity-check', 1)",
        )
        .execute(&mut self.connection)
        .await
        .context("received_files_fts integrity-check against received_files")?;
        Ok(())
    }

    async fn close(self) -> Result<()> {
        self.connection.close().await?;
        self.application.shutdown().await;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Session<'a> {
    id: &'a str,
    reverse_share: &'a str,
    token_hash: &'a str,
    email: Option<&'a str>,
    email_normalized: Option<&'a str>,
    locale: &'a str,
}

impl<'a> Session<'a> {
    const fn new(id: &'a str, reverse_share: &'a str, token_hash: &'a str) -> Self {
        Self {
            id,
            reverse_share,
            token_hash,
            email: None,
            email_normalized: None,
            locale: "en-US",
        }
    }
}

#[derive(Clone, Copy)]
struct Asset<'a> {
    id: &'a str,
    reverse_share: &'a str,
    object: &'a str,
    mime_type: &'a str,
    width: i64,
    height: i64,
    created_by: Option<&'a str>,
}

impl<'a> Asset<'a> {
    const fn new(id: &'a str, reverse_share: &'a str, object: &'a str) -> Self {
        Self {
            id,
            reverse_share,
            object,
            mime_type: "image/webp",
            width: 2560,
            height: 1440,
            created_by: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Received<'a> {
    id: &'a str,
    owner: &'a str,
    reverse_share: &'a str,
    session: Option<&'a str>,
    object: &'a str,
    name: &'a str,
    description: Option<&'a str>,
    mime_type: &'a str,
    uploader_name: Option<&'a str>,
    uploader_email: Option<&'a str>,
    uploader_ip: Option<&'a str>,
}

impl<'a> Received<'a> {
    const fn new(id: &'a str, reverse_share: &'a str, object: &'a str, name: &'a str) -> Self {
        Self {
            id,
            owner: ALICE,
            reverse_share,
            session: None,
            object,
            name,
            description: None,
            mime_type: "application/octet-stream",
            uploader_name: None,
            uploader_email: None,
            uploader_ip: None,
        }
    }
}

fn digest(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

fn compact(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn delete_actions(schema: &Schema, table: &str) -> Vec<(String, String, String)> {
    let mut actions: Vec<(String, String, String)> = schema
        .table(table)
        .unwrap_or_else(|| panic!("{table} missing"))
        .foreign_keys
        .iter()
        .map(|foreign_key| {
            (
                foreign_key.columns.join(","),
                foreign_key.parent.clone(),
                foreign_key.on_delete.clone(),
            )
        })
        .collect();
    actions.sort();
    actions
}

fn action(column: &str, parent: &str, on_delete: &str) -> (String, String, String) {
    (column.to_owned(), parent.to_owned(), on_delete.to_owned())
}

fn assert_unique_index(
    schema: &Schema,
    table: &str,
    index: &str,
    expected: &[&str],
    partial: bool,
) {
    let found = schema
        .table(table)
        .and_then(|table| {
            table
                .indexes
                .iter()
                .find(|candidate| candidate.name == index)
        })
        .unwrap_or_else(|| panic!("{table}.{index} missing"));
    assert!(found.unique, "{index} must be unique");
    assert_eq!(found.partial, partial, "{index} partial");
    let columns: Vec<IndexColumn> = expected
        .iter()
        .map(|name| IndexColumn {
            name: Some((*name).to_owned()),
            collation: "BINARY".to_owned(),
        })
        .collect();
    assert_eq!(found.columns, columns, "{index} columns");
}

fn locale_check_values(sql: &str) -> Vec<String> {
    let compact: String = sql.chars().filter(|c| !c.is_whitespace()).collect();
    let open = "localeIN(";
    let start = compact
        .find(open)
        .unwrap_or_else(|| panic!("locale CHECK missing in {sql}"))
        + open.len();
    let end = start
        + compact[start..]
            .find(')')
            .unwrap_or_else(|| panic!("locale CHECK does not close in {sql}"));
    compact[start..end]
        .split(',')
        .map(|value| value.trim_matches('\'').to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_rs_alias_independent_namespace() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_rs_alias_independent_namespace").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_unique_index(
        &schema,
        "reverse_shares",
        "ux_reverse_shares_alias",
        &["alias"],
        false,
    );
    assert_unique_index(
        &schema,
        "reverse_shares",
        "ux_reverse_shares_public_id",
        &["public_id"],
        false,
    );
    assert_eq!(nocase_collations(&schema), []);
    let alias_indexes: Vec<String> = schema
        .tables
        .iter()
        .flat_map(|table| {
            table
                .indexes
                .iter()
                .filter(|index| {
                    index
                        .columns
                        .iter()
                        .any(|column| column.name.as_deref() == Some("alias"))
                })
                .map(move |index| format!("{}.{}", table.name, index.name))
        })
        .collect();
    assert_eq!(
        alias_indexes,
        [
            "reverse_shares.ux_reverse_shares_alias",
            "shares.ux_shares_alias"
        ]
    );
    assert_eq!(
        database
            .count(
                "SELECT count(*) FROM sqlite_schema
                  WHERE type = 'trigger' AND tbl_name IN ('shares', 'reverse_shares')"
            )
            .await?,
        0
    );

    let db = &mut database;
    assert_eq!(db.share("s-report", "report").await?, Accepted);
    assert_eq!(
        db.reverse_share("r-report", ALICE, "report").await?,
        Accepted
    );
    assert_eq!(db.reverse_share("r-bob", BOB, "inbox").await?, Accepted);
    assert_eq!(db.share("s-inbox", "inbox").await?, Accepted);
    assert_eq!(
        db.strings(
            "SELECT 's:' || alias FROM shares
             UNION ALL
             SELECT 'r:' || alias FROM reverse_shares
             ORDER BY 1",
            SqliteArguments::default()
        )
        .await?,
        ["r:inbox", "r:report", "s:inbox", "s:report"]
    );

    assert_eq!(db.reverse_share("r-dup", BOB, "report").await?, Unique);
    assert_eq!(db.reverse_share("r-dup", ALICE, "inbox").await?, Unique);
    assert_eq!(db.share("s-dup", "report").await?, Unique);

    let longest = "z".repeat(64);
    for (id, alias) in [
        ("r-shortest", "abc"),
        ("r-longest", longest.as_str()),
        ("r-mixed", "q3-uploads_2026"),
        ("r-symbols", "_-_"),
        ("r-digits", "007"),
    ] {
        assert_eq!(
            db.reverse_share(id, ALICE, alias).await?,
            Accepted,
            "{alias}"
        );
    }

    let too_long = "z".repeat(65);
    for (id, alias) in [
        ("r-upper", "Report"),
        ("r-all-upper", "REPORT"),
        ("r-one-upper", "reporT"),
        ("r-too-short", "ab"),
        ("r-empty", ""),
        ("r-too-long", too_long.as_str()),
        ("r-space", "re port"),
        ("r-dot", "re.port"),
        ("r-slash", "re/port"),
        ("r-percent", "re%port"),
        ("r-bracket", "[abc]"),
        ("r-star", "abc*"),
        ("r-accented", "relatório"),
        ("r-fullwidth", "ｒｅｐｏｒｔ"),
        ("r-tab", "re\tport"),
    ] {
        assert_eq!(
            db.reverse_share(id, ALICE, alias).await?,
            Check,
            "{alias:?}"
        );
    }

    assert_eq!(
        db.exec("UPDATE reverse_shares SET alias = 'Inbox' WHERE id = 'r-bob'")
            .await?,
        Check
    );
    assert_eq!(
        db.exec("UPDATE reverse_shares SET alias = 'abc' WHERE id = 'r-bob'")
            .await?,
        Unique
    );
    assert_eq!(
        db.exec("UPDATE reverse_shares SET alias = 'report' WHERE id = 'r-bob'")
            .await?,
        Unique
    );
    assert_eq!(
        db.exec("UPDATE shares SET alias = 'abc' WHERE id = 's-inbox'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("UPDATE reverse_shares SET alias = 'inbox-two' WHERE id = 'r-bob'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.run(
            "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES ('r-public-twin', ?1, ?2, 'twin', ?3, ?3)",
            arguments![ALICE, format!("rs-{:0>16}", "r-report"), NOW],
        )
        .await?,
        Unique
    );
    assert_eq!(
        db.run(
            "INSERT INTO reverse_shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES ('r-public-short', ?1, 'short', 'shorty', ?2, ?2)",
            arguments![ALICE, NOW],
        )
        .await?,
        Check
    );
    assert_eq!(
        db.count("SELECT count(*) FROM reverse_shares WHERE alias GLOB '*[A-Z]*'")
            .await?,
        0
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_received_name_unique_per_rs() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_received_name_unique_per_rs").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_unique_index(
        &schema,
        "received_files",
        "ux_received_files_name",
        &["reverse_share_id", "name_normalized"],
        false,
    );
    assert_unique_index(
        &schema,
        "received_files",
        "ux_received_storage_object",
        &["storage_object_id"],
        false,
    );
    assert_eq!(
        delete_actions(&schema, "received_files"),
        [
            action("owner_id", "users", "RESTRICT"),
            action("reverse_share_id", "reverse_shares", "RESTRICT"),
            action("storage_object_id", "storage_objects", "RESTRICT"),
            action(
                "upload_session_id",
                "reverse_share_upload_sessions",
                "SET NULL"
            ),
        ]
    );

    let db = &mut database;
    assert_eq!(
        db.reverse_share("clients", ALICE, "clients").await?,
        Accepted
    );
    assert_eq!(
        db.reverse_share("vendors", ALICE, "vendors").await?,
        Accepted
    );
    let mut objects = Vec::new();
    for seed in 1..=8 {
        objects.push(db.object(seed).await?);
    }

    assert_eq!(
        db.received(Received::new(
            "invoice",
            "clients",
            &objects[0],
            "Invoice.pdf"
        ))
        .await?,
        Accepted
    );
    assert_eq!(
        db.received(Received::new(
            "invoice-twin",
            "clients",
            &objects[1],
            "INVOICE.PDF"
        ))
        .await?,
        Unique
    );
    assert_eq!(
        db.received(Received::new(
            "invoice-1",
            "clients",
            &objects[1],
            "invoice (1).pdf"
        ))
        .await?,
        Accepted
    );
    assert_eq!(
        db.received(Received::new(
            "vendor-invoice",
            "vendors",
            &objects[2],
            "invoice.pdf"
        ))
        .await?,
        Accepted
    );
    assert_eq!(
        db.received(Received::new(
            "reused-object",
            "vendors",
            &objects[0],
            "other.pdf"
        ))
        .await?,
        Unique
    );
    assert_eq!(
        db.received(Received::new(
            "reused-across",
            "clients",
            &objects[2],
            "copy.pdf"
        ))
        .await?,
        Unique
    );

    let file_object = &objects[3];
    assert_eq!(
        db.run(
            "INSERT INTO files
                 (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                  size_bytes, created_at, updated_at)
             VALUES ('my-invoice', ?1, NULL, ?2, 'invoice.pdf', 'invoice.pdf', 1, ?3, ?3)",
            arguments![ALICE, file_object.as_str(), NOW],
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.received(Received::new(
            "my-files-twin",
            "clients",
            &objects[4],
            "notes.pdf"
        ))
        .await?,
        Accepted
    );

    for (id, name) in [
        ("empty", ""),
        ("slash", "a/b.pdf"),
        ("dot", "."),
        ("dotdot", ".."),
    ] {
        assert_eq!(
            db.received(Received::new(id, "clients", &objects[5], name))
                .await?,
            Check,
            "{name:?}"
        );
    }
    assert_eq!(
        db.received(Received::new(
            "missing-object",
            "clients",
            "object-404",
            "x.pdf"
        ))
        .await?,
        Foreign
    );
    assert_eq!(
        db.received(Received::new(
            "missing-share",
            "nowhere",
            &objects[5],
            "x.pdf"
        ))
        .await?,
        Foreign
    );

    assert_eq!(
        db.exec(
            "UPDATE received_files SET name = 'invoice.PDF', name_normalized = 'invoice.pdf'
                  WHERE id = 'invoice-1'"
        )
        .await?,
        Unique
    );
    assert_eq!(
        db.exec("UPDATE received_files SET reverse_share_id = 'vendors' WHERE id = 'invoice'")
            .await?,
        Unique
    );
    assert_eq!(
        db.exec("UPDATE received_files SET storage_object_id = 'object-2' WHERE id = 'invoice'")
            .await?,
        Unique
    );

    assert_eq!(
        db.exec("DELETE FROM storage_objects WHERE id = 'object-1'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM reverse_shares WHERE id = 'clients'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.run("DELETE FROM users WHERE id = ?1", arguments![ALICE])
            .await?,
        Foreign
    );
    assert_eq!(db.count("SELECT count(*) FROM received_files").await?, 4);

    assert_eq!(
        db.session(Session::new("session", "vendors", &digest(1)))
            .await?,
        Accepted
    );
    let mut attributed = Received::new("attributed", "vendors", &objects[6], "report.pdf");
    attributed.session = Some("session");
    attributed.uploader_name = Some("Ana");
    attributed.uploader_email = Some("ana@example.test");
    assert_eq!(db.received(attributed).await?, Accepted);
    assert_eq!(
        db.exec("DELETE FROM reverse_share_upload_sessions WHERE id = 'session'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT coalesce(upload_session_id, '-') || ':' || uploader_name || ':'
                    || uploader_email || ':' || storage_object_id
               FROM received_files WHERE id = 'attributed'",
            SqliteArguments::default()
        )
        .await?,
        ["-:Ana:ana@example.test:object-7"]
    );

    assert_eq!(
        db.exec("DELETE FROM received_files WHERE id = 'invoice'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT state || ':' || refcount FROM storage_objects WHERE id = 'object-1'",
            SqliteArguments::default()
        )
        .await?,
        ["active:1"]
    );
    assert_eq!(
        db.received(Received::new(
            "invoice-again",
            "clients",
            &objects[7],
            "invoice.pdf"
        ))
        .await?,
        Accepted
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_received_fts_triggers() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_received_fts_triggers").await?;
    let sql = database
        .strings(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'received_files_fts'",
            SqliteArguments::default(),
        )
        .await?;
    assert_eq!(
        sql.iter().map(|sql| compact(sql)).collect::<Vec<_>>(),
        [compact(RECEIVED_FILES_FTS)]
    );
    assert_eq!(
        database
            .strings(
                "SELECT name FROM pragma_table_info('received_files_fts') ORDER BY cid",
                SqliteArguments::default()
            )
            .await?,
        ["name", "description"]
    );
    assert_eq!(
        database
            .strings(
                "SELECT name FROM sqlite_schema
                  WHERE type = 'trigger' AND tbl_name = 'received_files' ORDER BY name",
                SqliteArguments::default(),
            )
            .await?,
        [
            "received_files_fts_ad",
            "received_files_fts_ai",
            "received_files_fts_au"
        ]
    );
    assert_eq!(
        database
            .strings(
                "SELECT name FROM sqlite_schema
                  WHERE type = 'table' AND sql LIKE 'CREATE VIRTUAL TABLE%' ORDER BY name",
                SqliteArguments::default(),
            )
            .await?,
        ["files_fts", "received_files_fts"]
    );

    let db = &mut database;
    assert_eq!(db.reverse_share("inbox", ALICE, "inbox").await?, Accepted);
    let contract_object = db.object(1).await?;
    let photo_object = db.object(2).await?;
    let my_file_object = db.object(3).await?;

    let mut contract = Received::new("contract", "inbox", &contract_object, "Signed Contract.pdf");
    contract.mime_type = "application/pdf";
    contract.uploader_name = Some("Mariana Uploader");
    contract.uploader_email = Some("mariana@sender.test");
    contract.uploader_ip = Some("203.0.113.9");
    let mut photo = Received::new("photo", "inbox", &photo_object, "Férias Porto.jpg");
    photo.description = Some("sunset over douro");
    photo.mime_type = "image/jpeg";

    assert_eq!(db.received_hits("contract").await?, Vec::<String>::new());
    assert_eq!(db.received(contract).await?, Accepted);
    assert_eq!(db.received(photo).await?, Accepted);
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("contract").await?, ["contract"]);
    assert_eq!(db.received_hits("con*").await?, ["contract"]);
    assert_eq!(db.received_hits("porto").await?, ["photo"]);
    assert_eq!(db.received_hits("ferias").await?, ["photo"]);
    assert_eq!(db.received_hits("description : douro").await?, ["photo"]);
    assert_eq!(
        db.received_hits("name : douro").await?,
        Vec::<String>::new()
    );

    assert_eq!(
        db.run(
            "INSERT INTO files
                 (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                  size_bytes, created_at, updated_at)
             VALUES ('my-contract', ?1, NULL, ?2, 'Contract Draft.pdf', 'contract draft.pdf',
                     1, ?3, ?3)",
            arguments![ALICE, my_file_object.as_str(), NOW],
        )
        .await?,
        Accepted
    );
    assert_eq!(db.received_hits("contract").await?, ["contract"]);
    assert_eq!(db.received_hits("draft").await?, Vec::<String>::new());
    assert_eq!(db.file_hits("contract").await?, ["my-contract"]);
    assert_eq!(db.file_hits("signed").await?, Vec::<String>::new());
    assert_eq!(db.file_hits("porto").await?, Vec::<String>::new());

    assert_eq!(
        db.run(
            "UPDATE received_files SET name = 'Signed Agreement.pdf',
                                      name_normalized = 'signed agreement.pdf', updated_at = ?1
              WHERE id = 'contract'",
            arguments![LATER],
        )
        .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("contract").await?, Vec::<String>::new());
    assert_eq!(db.received_hits("agreement").await?, ["contract"]);
    assert_eq!(db.received_hits("signed").await?, ["contract"]);

    assert_eq!(
        db.run(
            "UPDATE received_files SET description = 'countersigned by legal', updated_at = ?1
              WHERE id = 'contract'",
            arguments![LATER],
        )
        .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("legal").await?, ["contract"]);
    assert_eq!(
        db.run(
            "UPDATE received_files SET description = 'archived copy', updated_at = ?1
              WHERE id = 'contract'",
            arguments![LATER],
        )
        .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("legal").await?, Vec::<String>::new());
    assert_eq!(db.received_hits("archived").await?, ["contract"]);
    assert_eq!(
        db.run(
            "UPDATE received_files SET description = NULL, updated_at = ?1 WHERE id = 'contract'",
            arguments![LATER],
        )
        .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("archived").await?, Vec::<String>::new());
    assert_eq!(db.received_hits("agreement").await?, ["contract"]);

    assert_eq!(
        db.run(
            "UPDATE received_files SET uploader_name = 'Renamed Sender', mime_type = 'text/plain',
                                      updated_at = ?1
              WHERE id = 'photo'",
            arguments![LATER],
        )
        .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;

    for absent in [
        "mariana",
        "uploader",
        "sender",
        "renamed",
        "203",
        "application",
        "jpeg",
        "text",
        "inbox",
        "object",
        "alice",
        "fallback",
    ] {
        assert_eq!(
            db.received_hits(absent).await?,
            Vec::<String>::new(),
            "{absent}"
        );
    }

    sqlx::query(
        "CREATE VIRTUAL TABLE temp.received_fts_terms
             USING fts5vocab(main, received_files_fts, col)",
    )
    .execute(&mut db.connection)
    .await?;
    assert_eq!(
        db.strings(
            "SELECT DISTINCT col || ':' || term FROM temp.received_fts_terms ORDER BY 1",
            SqliteArguments::default(),
        )
        .await?,
        [
            "description:douro",
            "description:over",
            "description:sunset",
            "name:agreement",
            "name:ferias",
            "name:jpg",
            "name:pdf",
            "name:porto",
            "name:signed",
        ]
    );

    assert_eq!(
        db.exec("DELETE FROM received_files WHERE id = 'photo'")
            .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("porto").await?, Vec::<String>::new());
    assert_eq!(db.received_hits("sunset").await?, Vec::<String>::new());
    assert_eq!(
        db.exec("DELETE FROM received_files WHERE id = 'contract'")
            .await?,
        Accepted
    );
    db.assert_received_index_consistent().await?;
    assert_eq!(db.received_hits("agreement").await?, Vec::<String>::new());
    assert_eq!(
        db.strings(
            "SELECT term FROM temp.received_fts_terms",
            SqliteArguments::default()
        )
        .await?,
        Vec::<String>::new()
    );
    assert_eq!(db.file_hits("contract").await?, ["my-contract"]);
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_rs_upload_session_capability_and_locale() -> Result<()> {
    let mut database =
        MigratedDatabase::open("it_schema_rs_upload_session_capability_and_locale").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    let mut reverse_share_tables: Vec<&str> = schema
        .table_names()
        .into_iter()
        .filter(|name| name.contains("reverse_share") || name.contains("grant"))
        .collect();
    reverse_share_tables.sort_unstable();
    assert_eq!(
        reverse_share_tables,
        [
            "embed_grants",
            "reverse_share_assets",
            "reverse_share_upload_sessions",
            "reverse_shares",
            "share_grants",
        ]
    );
    assert_eq!(
        delete_actions(&schema, "reverse_share_upload_sessions"),
        [action("reverse_share_id", "reverse_shares", "RESTRICT")]
    );
    assert_unique_index(
        &schema,
        "reverse_share_upload_sessions",
        "ux_rs_upload_sessions_token",
        &["token_hash"],
        false,
    );
    let secret_columns: Vec<String> = schema
        .table("reverse_share_upload_sessions")
        .expect("sessions")
        .columns
        .iter()
        .map(|column| column.name.clone())
        .filter(|name| {
            name.contains("token") || name.contains("secret") || name.contains("password")
        })
        .collect();
    assert_eq!(secret_columns, ["token_hash", "password_verified_at"]);

    let session_sql = &schema
        .table("reverse_share_upload_sessions")
        .expect("sessions")
        .sql;
    let preferences_sql = &schema.table("user_preferences").expect("preferences").sql;
    let session_locales = locale_check_values(session_sql);
    assert_eq!(session_locales, LOCALES);
    assert_eq!(session_locales, locale_check_values(preferences_sql));

    let db = &mut database;
    assert_eq!(db.reverse_share("inbox", ALICE, "inbox").await?, Accepted);
    assert_eq!(db.reverse_share("other", BOB, "other").await?, Accepted);

    for (seed, locale) in (1u8..).zip(LOCALES) {
        let id = format!("session-{locale}");
        let token = digest(seed);
        let mut session = Session::new(&id, "inbox", &token);
        session.locale = locale;
        assert_eq!(db.session(session).await?, Accepted, "{locale}");
    }
    assert_eq!(
        db.count("SELECT count(DISTINCT locale) FROM reverse_share_upload_sessions")
            .await?,
        23
    );
    for (seed, locale) in (100u8..).zip([
        "en-us", "EN-US", "en", "en_US", "pt-PT", "zh-TW", "es-MX", "", " en-US", "xx-XX",
    ]) {
        let id = format!("bad-locale-{seed}");
        let token = digest(seed);
        let mut session = Session::new(&id, "inbox", &token);
        session.locale = locale;
        assert_eq!(db.session(session).await?, Check, "{locale:?}");
    }
    assert_eq!(
        db.run(
            "INSERT INTO reverse_share_upload_sessions
                 (id, reverse_share_id, token_hash, created_at, expires_at, last_activity_at)
             VALUES ('default-locale', 'inbox', ?1, ?2, ?3, ?2)",
            arguments![digest(200), NOW, LATER],
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT state || ':' || locale || ':' || send_confirmation || ':' || files_uploaded
                    || ':' || bytes_uploaded
               FROM reverse_share_upload_sessions WHERE id = 'default-locale'",
            SqliteArguments::default()
        )
        .await?,
        ["active:en-US:0:0:0"]
    );
    assert_eq!(
        db.exec(
            "UPDATE reverse_share_upload_sessions SET locale = 'klingon'
                  WHERE id = 'default-locale'"
        )
        .await?,
        Check
    );

    let reused = digest(1);
    assert_eq!(
        db.session(Session::new("reused-token", "other", &reused))
            .await?,
        Unique
    );
    let short = &digest(201)[..63];
    assert_eq!(
        db.session(Session::new("short-token", "inbox", short))
            .await?,
        Check
    );
    let long = format!("{}0", digest(202));
    assert_eq!(
        db.session(Session::new("long-token", "inbox", &long))
            .await?,
        Check
    );
    assert_eq!(
        db.session(Session::new("orphan", "missing", &digest(203)))
            .await?,
        Foreign
    );

    let token = digest(210);
    let mut email_only = Session::new("email-only", "inbox", &token);
    email_only.email = Some("Ana@Example.test");
    assert_eq!(db.session(email_only).await?, Check);
    let mut normalized_only = Session::new("normalized-only", "inbox", &token);
    normalized_only.email_normalized = Some("ana@example.test");
    assert_eq!(db.session(normalized_only).await?, Check);
    let mut paired = Session::new("paired", "inbox", &token);
    paired.email = Some("Ana@Example.test");
    paired.email_normalized = Some("ana@example.test");
    assert_eq!(db.session(paired).await?, Accepted);
    assert_eq!(
        db.exec(
            "UPDATE reverse_share_upload_sessions SET uploader_email = NULL WHERE id = 'paired'"
        )
        .await?,
        Check
    );

    for (state, expected) in [
        ("completed", Accepted),
        ("canceled", Accepted),
        ("expired", Accepted),
        ("invalidated", Accepted),
        ("active", Accepted),
        ("revoked", Check),
        ("pending", Check),
    ] {
        assert_eq!(
            db.run(
                "UPDATE reverse_share_upload_sessions SET state = ?1 WHERE id = 'paired'",
                arguments![state],
            )
            .await?,
            expected,
            "{state}"
        );
    }
    for (column, value) in [
        ("send_confirmation", 2),
        ("files_uploaded", -1),
        ("bytes_uploaded", -1),
    ] {
        assert_eq!(
            db.run(
                &format!(
                    "UPDATE reverse_share_upload_sessions SET {column} = ?1 WHERE id = 'paired'"
                ),
                arguments![value],
            )
            .await?,
            Check,
            "{column}"
        );
    }

    assert_eq!(
        db.exec("DELETE FROM reverse_shares WHERE id = 'inbox'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM reverse_share_upload_sessions WHERE reverse_share_id = 'inbox'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM reverse_shares WHERE id = 'inbox'")
            .await?,
        Accepted
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_rs_hero_asset_mutual_references() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_rs_hero_asset_mutual_references").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_eq!(
        delete_actions(&schema, "reverse_shares"),
        [
            action("hero_asset_id", "reverse_share_assets", "SET NULL"),
            action("owner_id", "users", "RESTRICT"),
        ]
    );
    assert_eq!(
        delete_actions(&schema, "reverse_share_assets"),
        [
            action("created_by", "users", "SET NULL"),
            action("reverse_share_id", "reverse_shares", "RESTRICT"),
            action("storage_object_id", "storage_objects", "RESTRICT"),
        ]
    );
    assert_unique_index(
        &schema,
        "reverse_share_assets",
        "ux_rs_assets_storage_object",
        &["storage_object_id"],
        false,
    );

    let db = &mut database;
    for (id, seed) in [
        ("hero-1", 1u64),
        ("hero-2", 2),
        ("hero-3", 3),
        ("hero-4", 4),
    ] {
        db.object_at(id, &format!("branding/hero/{seed:032x}"))
            .await?;
    }
    assert_eq!(
        db.run(
            "INSERT INTO reverse_shares
                 (id, owner_id, public_id, alias, layout, hero_background_kind, hero_asset_id,
                  created_at, updated_at)
             VALUES ('eager', ?1, 'rs-eager-000000000', 'eager', 'hero', 'image', 'asset-1',
                     ?2, ?2)",
            arguments![ALICE, NOW],
        )
        .await?,
        Foreign
    );
    assert_eq!(
        db.run(
            "INSERT INTO reverse_shares
                 (id, owner_id, public_id, alias, layout, hero_background_kind,
                  created_at, updated_at)
             VALUES ('pointerless', ?1, 'rs-pointerless-00', 'pointerless', 'hero', 'image',
                     ?2, ?2)",
            arguments![ALICE, NOW],
        )
        .await?,
        Check
    );

    assert_eq!(
        db.reverse_share("campaign", ALICE, "campaign").await?,
        Accepted
    );
    let mut first = Asset::new("asset-1", "campaign", "hero-1");
    first.created_by = Some(ALICE);
    assert_eq!(db.asset(first).await?, Accepted);
    assert_eq!(
        db.exec(
            "UPDATE reverse_shares
                    SET layout = 'hero', hero_background_kind = 'image', hero_asset_id = 'asset-1'
                  WHERE id = 'campaign'"
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.exec("UPDATE reverse_shares SET hero_asset_id = 'asset-404' WHERE id = 'campaign'")
            .await?,
        Foreign
    );

    assert_eq!(
        db.asset(Asset::new("asset-dup-object", "campaign", "hero-1"))
            .await?,
        Unique
    );
    assert_eq!(
        db.asset(Asset::new("asset-orphan", "missing", "hero-2"))
            .await?,
        Foreign
    );
    assert_eq!(
        db.asset(Asset::new("asset-no-object", "campaign", "hero-404"))
            .await?,
        Foreign
    );
    for mime_type in [
        "image/svg+xml",
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/avif",
        "IMAGE/WEBP",
        "",
    ] {
        let mut asset = Asset::new("asset-mime", "campaign", "hero-2");
        asset.mime_type = mime_type;
        assert_eq!(db.asset(asset).await?, Check, "{mime_type:?}");
    }
    for (width, height) in [(0, 1), (1, 0), (8193, 1), (1, 8193), (-1, 10)] {
        let mut asset = Asset::new("asset-size", "campaign", "hero-2");
        asset.width = width;
        asset.height = height;
        assert_eq!(db.asset(asset).await?, Check, "{width}x{height}");
    }
    let mut largest = Asset::new("asset-largest", "campaign", "hero-2");
    largest.width = 8192;
    largest.height = 1;
    assert_eq!(db.asset(largest).await?, Accepted);
    assert_eq!(
        db.exec("UPDATE reverse_share_assets SET kind = 'logo' WHERE id = 'asset-largest'")
            .await?,
        Check
    );

    assert_eq!(
        db.exec("DELETE FROM storage_objects WHERE id = 'hero-1'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM reverse_shares WHERE id = 'campaign'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM reverse_share_assets WHERE id = 'asset-1'")
            .await?,
        Check
    );

    assert_eq!(
        db.reverse_share("gallery", ALICE, "gallery").await?,
        Accepted
    );
    assert_eq!(
        db.asset(Asset::new("asset-3", "gallery", "hero-3")).await?,
        Accepted
    );
    assert_eq!(
        db.exec(
            "UPDATE reverse_shares
                    SET layout = 'hero', hero_background_kind = 'brand', hero_asset_id = 'asset-3'
                  WHERE id = 'gallery'"
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM reverse_share_assets WHERE id = 'asset-3'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT hero_background_kind || ':' || coalesce(hero_asset_id, '-')
               FROM reverse_shares WHERE id = 'gallery'",
            SqliteArguments::default()
        )
        .await?,
        ["brand:-"]
    );
    assert_eq!(
        db.strings(
            "SELECT state || ':' || refcount FROM storage_objects WHERE id = 'hero-3'",
            SqliteArguments::default()
        )
        .await?,
        ["active:1"]
    );

    assert_eq!(
        db.exec("UPDATE reverse_shares SET hero_background_kind = 'gradient' WHERE id = 'gallery'")
            .await?,
        Check
    );
    assert_eq!(
        db.exec(
            "UPDATE reverse_shares
                    SET hero_background_kind = 'gradient', hero_gradient_preset = 'aurora'
                  WHERE id = 'gallery'"
        )
        .await?,
        Accepted
    );
    for (column, value) in [
        ("layout", "wetransfer"),
        ("hero_background_kind", "snapshot"),
        ("name_field", "mandatory"),
        ("email_field", "shown"),
        ("description_field", ""),
        ("allowed_extensions", "{\"pdf\":true}"),
        ("allowed_extensions", "not json"),
    ] {
        assert_eq!(
            db.run(
                &format!("UPDATE reverse_shares SET {column} = ?1 WHERE id = 'gallery'"),
                arguments![value],
            )
            .await?,
            Check,
            "{column}={value}"
        );
    }
    for (column, value) in [
        ("max_files", 0i64),
        ("max_file_size_bytes", 0),
        ("received_retention_days", 0),
        ("file_count", -1),
        ("total_bytes", -1),
        ("is_active", 2),
        ("notify_owner", 2),
    ] {
        assert_eq!(
            db.run(
                &format!("UPDATE reverse_shares SET {column} = ?1 WHERE id = 'gallery'"),
                arguments![value],
            )
            .await?,
            Check,
            "{column}={value}"
        );
    }
    assert_eq!(
        db.exec(
            "UPDATE reverse_shares SET allowed_extensions = '[\"pdf\",\"docx\"]',
                                           max_files = 10, max_file_size_bytes = 1048576,
                                           received_retention_days = 30
                  WHERE id = 'gallery'"
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.run(
            "UPDATE reverse_shares SET suspended_at = ?1 WHERE id = 'gallery'",
            arguments![LATER]
        )
        .await?,
        Check
    );
    assert_eq!(
        db.run(
            "UPDATE reverse_shares SET suspended_at = ?1, suspended_reason = 'owner_deactivated'
              WHERE id = 'gallery'",
            arguments![LATER]
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.exec("UPDATE reverse_shares SET suspended_reason = 'expired' WHERE id = 'gallery'")
            .await?,
        Check
    );

    let mut by_bob = Asset::new("asset-4", "gallery", "hero-4");
    by_bob.created_by = Some(BOB);
    assert_eq!(db.asset(by_bob).await?, Accepted);
    assert_eq!(
        db.run("DELETE FROM users WHERE id = ?1", arguments![BOB])
            .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT id || ':' || coalesce(created_by, '-') FROM reverse_share_assets ORDER BY id",
            SqliteArguments::default()
        )
        .await?,
        ["asset-1:user-alice", "asset-4:-", "asset-largest:-"]
    );
    assert_eq!(
        db.run("DELETE FROM users WHERE id = ?1", arguments![ALICE])
            .await?,
        Foreign
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_branding_current_and_object_constraints() -> Result<()> {
    let mut database =
        MigratedDatabase::open("it_schema_branding_current_and_object_constraints").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_eq!(
        delete_actions(&schema, "branding_assets"),
        [
            action("created_by", "users", "SET NULL"),
            action("storage_object_id", "storage_objects", "RESTRICT"),
        ]
    );
    assert_unique_index(
        &schema,
        "branding_assets",
        "ux_branding_assets_storage_object",
        &["storage_object_id"],
        false,
    );
    assert_unique_index(
        &schema,
        "branding_assets",
        "ux_branding_assets_current",
        &["kind"],
        true,
    );
    for table in ["reverse_share_assets", "received_files", "branding_assets"] {
        assert!(schema.table(table).expect("table").owns_bytes(), "{table}");
    }
    assert_eq!(byte_owner_cascades(&schema), []);

    let db = &mut database;
    assert_eq!(db.count("SELECT count(*) FROM branding_assets").await?, 0);

    for kind in BRANDING_KINDS {
        for generation in 1..=3u64 {
            let hex = format!("{generation:032x}");
            db.object_at(
                &format!("{kind}-{generation}"),
                &format!("branding/{kind}/{hex}"),
            )
            .await?;
        }
    }
    db.object_at("stray", "objects/00/00/0000000000000000000000000000000a")
        .await?;

    for kind in BRANDING_KINDS {
        let mime_type = if kind == "favicon" {
            "image/png"
        } else {
            "image/webp"
        };
        assert_eq!(
            db.branding(
                &format!("{kind}-a"),
                kind,
                &format!("{kind}-1"),
                mime_type,
                1
            )
            .await?,
            Accepted,
            "{kind}"
        );
        assert_eq!(
            db.branding(
                &format!("{kind}-b"),
                kind,
                &format!("{kind}-2"),
                mime_type,
                1
            )
            .await?,
            Unique,
            "{kind} second current"
        );
        assert_eq!(
            db.branding(
                &format!("{kind}-b"),
                kind,
                &format!("{kind}-2"),
                mime_type,
                0
            )
            .await?,
            Accepted,
            "{kind} superseded"
        );
    }
    assert_eq!(
        db.count("SELECT count(*) FROM branding_assets WHERE is_current = 1")
            .await?,
        5
    );

    assert_eq!(
        db.exec("UPDATE branding_assets SET is_current = 1 WHERE id = 'logo-b'")
            .await?,
        Unique
    );
    assert_eq!(
        db.exec("UPDATE branding_assets SET is_current = 0 WHERE id = 'logo-a'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("UPDATE branding_assets SET is_current = 1 WHERE id = 'logo-b'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT id FROM branding_assets WHERE kind = 'logo' AND is_current = 1",
            SqliteArguments::default()
        )
        .await?,
        ["logo-b"]
    );

    assert_eq!(
        db.branding("reuse", "favicon", "logo-1", "image/png", 0)
            .await?,
        Unique
    );
    assert_eq!(
        db.branding("missing-object", "logo", "logo-404", "image/webp", 0)
            .await?,
        Foreign
    );
    for mime_type in [
        "image/svg+xml",
        "image/jpeg",
        "image/gif",
        "image/avif",
        "image/x-icon",
        "IMAGE/PNG",
        "",
    ] {
        assert_eq!(
            db.branding("bad-mime", "logo", "logo-3", mime_type, 0)
                .await?,
            Check,
            "{mime_type:?}"
        );
    }
    for kind in ["avatar", "hero", "og-image", "Logo", "background", ""] {
        assert_eq!(
            db.branding("bad-kind", kind, "logo-3", "image/webp", 0)
                .await?,
            Check,
            "{kind:?}"
        );
    }
    assert_eq!(
        db.branding("bad-flag", "logo", "logo-3", "image/webp", 2)
            .await?,
        Check
    );
    for (column, value) in [("width", 0i64), ("height", 8193), ("size_bytes", 0)] {
        assert_eq!(
            db.run(
                &format!("UPDATE branding_assets SET {column} = ?1 WHERE id = 'logo-b'"),
                arguments![value],
            )
            .await?,
            Check,
            "{column}={value}"
        );
    }
    assert_eq!(
        db.exec("UPDATE branding_assets SET width = NULL, height = NULL WHERE id = 'favicon-a'")
            .await?,
        Accepted
    );

    assert_eq!(
        db.exec("DELETE FROM storage_objects WHERE id = 'logo-2'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM branding_assets WHERE id = 'logo-a'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM storage_objects WHERE id = 'logo-1'")
            .await?,
        Accepted
    );

    assert_eq!(
        db.run("DELETE FROM users WHERE id = ?1", arguments![ALICE])
            .await?,
        Accepted
    );
    assert_eq!(
        db.count("SELECT count(*) FROM branding_assets WHERE created_by IS NOT NULL")
            .await?,
        0
    );
    assert_eq!(db.count("SELECT count(*) FROM branding_assets").await?, 9);
    database.close().await
}

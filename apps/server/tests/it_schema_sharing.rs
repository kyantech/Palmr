pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{nocase_collations, IndexColumn, Schema};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00.000Z";
const LATER: &str = "2026-01-01T12:00:00.000Z";
const RESTRICT_VIOLATION: &str = "1811";
const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";
const SHARING_TABLES: [&str; 6] = [
    "shares",
    "share_items",
    "share_recipients",
    "share_grants",
    "share_access_events",
    "embed_grants",
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

    async fn strings(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar_with(sql, arguments)
            .fetch_all(&mut self.connection)
            .await?)
    }

    async fn count(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<i64> {
        Ok(sqlx::query_scalar_with(sql, arguments)
            .fetch_one(&mut self.connection)
            .await?)
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

    async fn file(&mut self, id: &str, owner: &str, seed: u64) -> Result<String> {
        let object = self.object(seed).await?;
        let name = format!("{id}.bin");
        assert_eq!(
            self.run(
                "INSERT INTO files
                     (id, owner_id, folder_id, storage_object_id, name, name_normalized,
                      size_bytes, created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?4, 0, ?5, ?5)",
                arguments![id, owner, object.as_str(), name, NOW],
            )
            .await?,
            Accepted
        );
        Ok(object)
    }

    async fn folder(&mut self, id: &str, owner: &str) -> Result<()> {
        assert_eq!(
            self.run(
                "INSERT INTO folders
                     (id, owner_id, parent_id, name, name_normalized, depth,
                      created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?1, ?1, 0, ?3, ?3)",
                arguments![id, owner, NOW],
            )
            .await?,
            Accepted
        );
        Ok(())
    }

    async fn share(&mut self, id: &str, owner: &str, alias: &str) -> Result<Outcome> {
        self.run(
            "INSERT INTO shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            arguments![id, owner, format!("public-{id:0>16}"), alias, NOW],
        )
        .await
    }

    async fn item(
        &mut self,
        id: &str,
        share: &str,
        item_type: &str,
        file: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO share_items (id, share_id, item_type, file_id, folder_id, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            arguments![id, share, item_type, file, folder, NOW],
        )
        .await
    }

    async fn recipient(&mut self, id: &str, share: &str, email: &str) -> Result<Outcome> {
        self.run(
            "INSERT INTO share_recipients (id, share_id, email, email_normalized, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            arguments![id, share, email, email.to_ascii_lowercase(), NOW],
        )
        .await
    }

    async fn grant(&mut self, id: &str, share: &str, token_hash: &str) -> Result<Outcome> {
        self.run(
            "INSERT INTO share_grants (id, share_id, token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            arguments![id, share, token_hash, NOW, LATER],
        )
        .await
    }

    async fn event(
        &mut self,
        id: &str,
        share: &str,
        kind: &str,
        counted_as: &str,
        grant: Option<&str>,
        file: Option<&str>,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO share_access_events
                 (id, share_id, at, kind, counted_as, grant_id, file_id, item_count,
                  bytes_authorized)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0)",
            arguments![id, share, NOW, kind, counted_as, grant, file],
        )
        .await
    }

    async fn embed(
        &mut self,
        id: &str,
        file: &str,
        owner: &str,
        public_id: &str,
        token_hash: &str,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO embed_grants (id, file_id, owner_id, public_id, token_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            arguments![id, file, owner, public_id, token_hash, NOW],
        )
        .await
    }

    async fn close(self) -> Result<()> {
        self.connection.close().await?;
        self.application.shutdown().await;
        Ok(())
    }
}

fn digest(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
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

fn column_names(schema: &Schema, table: &str) -> Vec<String> {
    schema
        .table(table)
        .unwrap_or_else(|| panic!("{table} missing"))
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect()
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

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_share_alias_canonical_check() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_share_alias_canonical_check").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_unique_index(&schema, "shares", "ux_shares_alias", &["alias"], false);
    assert_unique_index(
        &schema,
        "shares",
        "ux_shares_public_id",
        &["public_id"],
        false,
    );
    assert_eq!(nocase_collations(&schema), []);
    let shares_sql = schema
        .table("shares")
        .expect("shares")
        .sql
        .to_ascii_lowercase();
    assert!(!shares_sql.contains("nocase"));

    let db = &mut database;
    let longest = "a".repeat(64);
    for (id, alias) in [
        ("canonical", "clientex"),
        ("shortest", "abc"),
        ("longest", longest.as_str()),
        ("mixed", "q3-report_2026"),
        ("symbols", "_-_"),
        ("digits", "007"),
    ] {
        assert_eq!(db.share(id, ALICE, alias).await?, Accepted, "{alias}");
    }
    assert_eq!(
        db.strings(
            "SELECT alias FROM shares WHERE id = 'canonical'",
            SqliteArguments::default()
        )
        .await?,
        ["clientex"]
    );

    let too_long = "a".repeat(65);
    for (id, alias) in [
        ("upper", "ClienteX"),
        ("all-upper", "CLIENTEX"),
        ("one-upper", "clienteX"),
        ("too-short", "ab"),
        ("empty", ""),
        ("too-long", too_long.as_str()),
        ("space", "client x"),
        ("dot", "client.x"),
        ("slash", "client/x"),
        ("percent", "client%x"),
        ("bracket", "[abc]"),
        ("star", "abc*"),
        ("accented", "ação"),
        ("fullwidth", "ｃｌｉｅｎｔ"),
        ("tab", "client\tx"),
    ] {
        assert_eq!(db.share(id, BOB, alias).await?, Check, "{alias:?}");
    }

    assert_eq!(db.share("duplicate", BOB, "clientex").await?, Unique);
    assert_eq!(db.share("case-twin", BOB, "ClienteX").await?, Check);
    assert_eq!(
        db.count(
            "SELECT count(*) FROM shares WHERE alias = 'ClienteX'",
            SqliteArguments::default()
        )
        .await?,
        0
    );
    assert_eq!(
        db.run(
            "UPDATE shares SET alias = 'ClienteY' WHERE id = 'canonical'",
            SqliteArguments::default()
        )
        .await?,
        Check
    );
    assert_eq!(
        db.run(
            "UPDATE shares SET alias = 'abc' WHERE id = 'canonical'",
            SqliteArguments::default()
        )
        .await?,
        Unique
    );
    assert_eq!(
        db.run(
            "UPDATE shares SET alias = 'clientey' WHERE id = 'canonical'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.run(
            "INSERT INTO shares (id, owner_id, public_id, alias, created_at, updated_at)
             VALUES ('public-twin', ?1, ?2, 'another', ?3, ?3)",
            arguments![BOB, format!("public-{:0>16}", "canonical"), NOW],
        )
        .await?,
        Unique
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM shares WHERE alias GLOB '*[A-Z]*'",
            SqliteArguments::default()
        )
        .await?,
        0
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_embed_grants_cascade_with_file() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_embed_grants_cascade_with_file").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    assert_eq!(
        delete_actions(&schema, "embed_grants"),
        [
            action("file_id", "files", "CASCADE"),
            action("owner_id", "users", "RESTRICT"),
        ]
    );
    assert!(!schema
        .table("embed_grants")
        .expect("embed_grants")
        .owns_bytes());
    assert_unique_index(
        &schema,
        "embed_grants",
        "ux_embed_grants_token",
        &["token_hash"],
        false,
    );
    assert_unique_index(
        &schema,
        "embed_grants",
        "ux_embed_grants_public_id",
        &["public_id"],
        false,
    );

    let db = &mut database;
    let photo_object = db.file("photo", ALICE, 1).await?;
    let clip_object = db.file("clip", ALICE, 2).await?;
    for (id, file, public_id, token) in [
        ("embed-photo-1", "photo", "embed-public-0001", 1),
        ("embed-photo-2", "photo", "embed-public-0002", 2),
        ("embed-clip", "clip", "embed-public-0003", 3),
    ] {
        assert_eq!(
            db.embed(id, file, ALICE, public_id, &digest(token)).await?,
            Accepted
        );
    }
    assert_eq!(
        db.embed(
            "embed-missing",
            "missing",
            ALICE,
            "embed-public-0004",
            &digest(4)
        )
        .await?,
        Foreign
    );
    let object_state = "SELECT state || ':' || refcount || ':' || updated_at
                          FROM storage_objects WHERE id = ?1";
    let before = db
        .strings(object_state, arguments![photo_object.as_str()])
        .await?;
    assert_eq!(before, [format!("active:1:{NOW}")]);

    assert_eq!(
        db.run(
            "DELETE FROM files WHERE id = 'photo'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT id FROM embed_grants ORDER BY id",
            SqliteArguments::default()
        )
        .await?,
        ["embed-clip"]
    );
    assert_eq!(
        db.strings(object_state, arguments![photo_object.as_str()])
            .await?,
        before
    );
    assert_eq!(
        db.strings(object_state, arguments![clip_object.as_str()])
            .await?,
        [format!("active:1:{NOW}")]
    );

    let clip_grant_owned_by_bob = "UPDATE embed_grants SET owner_id = ?1 WHERE id = 'embed-clip'";
    assert_eq!(
        db.run(clip_grant_owned_by_bob, arguments![BOB]).await?,
        Accepted
    );
    assert_eq!(
        db.run("DELETE FROM users WHERE id = ?1", arguments![BOB])
            .await?,
        Foreign
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM embed_grants WHERE owner_id = ?1",
            arguments![BOB]
        )
        .await?,
        1
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_sharing_roots_recipients_and_capabilities() -> Result<()> {
    let mut database =
        MigratedDatabase::open("it_schema_sharing_roots_recipients_and_capabilities").await?;
    let schema = Schema::introspect(&mut database.connection).await?;
    for table in SHARING_TABLES {
        assert!(schema.table(table).is_some(), "{table} missing");
        assert!(!schema.table(table).expect("table").owns_bytes(), "{table}");
    }
    assert_eq!(
        delete_actions(&schema, "shares"),
        [action("owner_id", "users", "RESTRICT")]
    );
    assert_eq!(
        delete_actions(&schema, "share_items"),
        [
            action("file_id", "files", "CASCADE"),
            action("folder_id", "folders", "CASCADE"),
            action("share_id", "shares", "CASCADE"),
        ]
    );
    assert_eq!(
        delete_actions(&schema, "share_recipients"),
        [action("share_id", "shares", "CASCADE")]
    );
    assert_eq!(
        delete_actions(&schema, "share_grants"),
        [action("share_id", "shares", "CASCADE")]
    );
    assert_eq!(
        delete_actions(&schema, "share_access_events"),
        [
            action("file_id", "files", "SET NULL"),
            action("grant_id", "share_grants", "SET NULL"),
            action("share_id", "shares", "CASCADE"),
        ]
    );
    assert_unique_index(
        &schema,
        "share_items",
        "ux_share_items_file",
        &["share_id", "file_id"],
        true,
    );
    assert_unique_index(
        &schema,
        "share_items",
        "ux_share_items_folder",
        &["share_id", "folder_id"],
        true,
    );
    assert_unique_index(
        &schema,
        "share_recipients",
        "ux_share_recipients_email",
        &["share_id", "email_normalized"],
        false,
    );
    assert_unique_index(
        &schema,
        "share_grants",
        "ux_share_grants_token",
        &["token_hash"],
        false,
    );
    assert_eq!(
        column_names(&schema, "share_recipients"),
        [
            "id",
            "share_id",
            "email",
            "email_normalized",
            "added_at",
            "last_notified_at",
            "notify_count",
            "last_notify_state",
        ]
    );
    for table in ["share_grants", "embed_grants"] {
        let tokens: Vec<String> = column_names(&schema, table)
            .into_iter()
            .filter(|name| name.contains("token") || name.contains("secret"))
            .collect();
        assert_eq!(tokens, ["token_hash"], "{table}");
    }

    let db = &mut database;
    db.file("report", ALICE, 1).await?;
    db.file("notes", ALICE, 2).await?;
    db.folder("album", ALICE).await?;
    db.folder("empty", ALICE).await?;
    for (id, alias) in [("first", "first-share"), ("second", "second-share")] {
        assert_eq!(db.share(id, ALICE, alias).await?, Accepted);
    }

    for (id, item_type, file, folder, expected) in [
        ("file-root", "file", Some("report"), None, Accepted),
        ("folder-root", "folder", None, Some("album"), Accepted),
        ("file-both", "file", Some("notes"), Some("empty"), Check),
        ("folder-both", "folder", Some("notes"), Some("empty"), Check),
        ("file-as-folder", "file", None, Some("empty"), Check),
        ("folder-as-file", "folder", Some("notes"), None, Check),
        ("file-neither", "file", None, None, Check),
        ("folder-neither", "folder", None, None, Check),
        ("unknown-type", "share", Some("notes"), None, Check),
        ("file-dup", "file", Some("report"), None, Unique),
        ("folder-dup", "folder", None, Some("album"), Unique),
        ("missing-file", "file", Some("missing"), None, Foreign),
    ] {
        assert_eq!(
            db.item(id, "first", item_type, file, folder).await?,
            expected,
            "{id}"
        );
    }
    assert_eq!(
        db.item("second-file", "second", "file", Some("report"), None)
            .await?,
        Accepted
    );
    assert_eq!(
        db.item("second-folder", "second", "folder", None, Some("album"))
            .await?,
        Accepted
    );

    assert_eq!(
        db.recipient("r1", "first", "Ana@Example.test").await?,
        Accepted
    );
    assert_eq!(
        db.recipient("r2", "first", "ana@example.TEST").await?,
        Unique
    );
    assert_eq!(
        db.recipient("r3", "second", "ana@example.test").await?,
        Accepted
    );
    assert_eq!(
        db.run(
            "UPDATE share_recipients SET last_notify_state = 'granted' WHERE id = 'r1'",
            SqliteArguments::default()
        )
        .await?,
        Check
    );
    assert_eq!(
        db.run(
            "UPDATE share_recipients SET last_notify_state = 'skipped_no_smtp' WHERE id = 'r1'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );

    assert_eq!(db.grant("g1", "first", &digest(1)).await?, Accepted);
    assert_eq!(db.grant("g2", "second", &digest(1)).await?, Unique);
    assert_eq!(db.grant("g3", "first", &digest(1)[..63]).await?, Check);
    assert_eq!(
        db.grant("g4", "first", &format!("{}0", digest(1))).await?,
        Check
    );
    assert_eq!(db.grant("g5", "missing", &digest(5)).await?, Foreign);
    db.file("clip", ALICE, 3).await?;
    assert_eq!(
        db.embed("e1", "clip", ALICE, "embed-public-0001", &digest(9)[..63])
            .await?,
        Check
    );
    assert_eq!(
        db.embed("e2", "clip", ALICE, "short", &digest(9)).await?,
        Check
    );

    for (id, kind, counted_as, grant, file, expected) in [
        ("ev-view", "view", "view", Some("g1"), None, Accepted),
        (
            "ev-dl",
            "download_single",
            "download",
            Some("g1"),
            Some("report"),
            Accepted,
        ),
        (
            "ev-preview",
            "preview",
            "none",
            None,
            Some("notes"),
            Accepted,
        ),
        (
            "ev-presign",
            "presign_issued",
            "download",
            None,
            Some("report"),
            Accepted,
        ),
        ("ev-kind", "upload", "none", None, None, Check),
        ("ev-counted", "view", "upload", None, None, Check),
    ] {
        assert_eq!(
            db.event(id, "first", kind, counted_as, grant, file).await?,
            expected,
            "{id}"
        );
    }

    assert_eq!(
        db.run(
            "DELETE FROM files WHERE id = 'report'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.strings(
            "SELECT id FROM share_items ORDER BY id",
            SqliteArguments::default()
        )
        .await?,
        ["folder-root", "second-folder"]
    );
    assert_eq!(
        db.strings(
            "SELECT id || ':' || coalesce(file_id, '-') || ':' || coalesce(grant_id, '-')
               FROM share_access_events ORDER BY id",
            SqliteArguments::default()
        )
        .await?,
        [
            "ev-dl:-:g1",
            "ev-presign:-:-",
            "ev-preview:notes:-",
            "ev-view:-:g1"
        ]
    );
    assert_eq!(
        db.run(
            "DELETE FROM share_grants WHERE id = 'g1'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM share_access_events WHERE grant_id IS NOT NULL",
            SqliteArguments::default()
        )
        .await?,
        0
    );
    assert_eq!(
        db.run(
            "DELETE FROM folders WHERE id = 'album'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM share_items",
            SqliteArguments::default()
        )
        .await?,
        0
    );

    assert_eq!(
        db.run("DELETE FROM users WHERE id = ?1", arguments![ALICE])
            .await?,
        Foreign
    );
    assert_eq!(db.grant("g6", "first", &digest(6)).await?, Accepted);
    assert_eq!(
        db.run(
            "DELETE FROM shares WHERE id = 'first'",
            SqliteArguments::default()
        )
        .await?,
        Accepted
    );
    for table in ["share_recipients", "share_grants", "share_access_events"] {
        assert_eq!(
            db.count(
                &format!("SELECT count(*) FROM {table} WHERE share_id = 'first'"),
                SqliteArguments::default()
            )
            .await?,
            0,
            "{table}"
        );
    }
    assert_eq!(
        db.count("SELECT count(*) FROM files", SqliteArguments::default())
            .await?,
        2
    );
    assert_eq!(
        db.count(
            "SELECT count(*) FROM share_recipients WHERE share_id = 'second'",
            SqliteArguments::default()
        )
        .await?,
        1
    );

    assert_eq!(
        db.run(
            "UPDATE shares SET suspended_at = ?1 WHERE id = 'second'",
            arguments![LATER]
        )
        .await?,
        Check
    );
    assert_eq!(
        db.run(
            "UPDATE shares SET suspended_at = ?1, suspended_reason = 'owner_deactivated'
              WHERE id = 'second'",
            arguments![LATER]
        )
        .await?,
        Accepted
    );
    database.close().await
}

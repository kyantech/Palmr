pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{byte_owner_cascades, Schema};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00.000Z";
const LATER: &str = "2026-01-02T00:00:00.000Z";
const RESTRICT_VIOLATION: &str = "1811";
const ALICE: &str = "user-alice";
const BOB: &str = "user-bob";
const FOLDER: &str = "folder-docs";
const RS: &str = "reverse-share-clients";
const RS_SESSION: &str = "rs-session-clients";

const TRANSFER_TABLES: [&str; 6] = [
    "transfer_sessions",
    "transfer_session_files",
    "tus_uploads",
    "s3_multipart_uploads",
    "s3_multipart_parts",
    "quota_reservations",
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
    NotNull,
}

use Outcome::{Accepted, Check, Foreign, NotNull, Unique};

#[derive(Clone, Copy)]
struct Session<'a> {
    id: &'a str,
    context: &'a str,
    user: Option<&'a str>,
    rs_session: Option<&'a str>,
    target_folder: Option<&'a str>,
    provider: &'a str,
    state: &'a str,
    cancel_requested: i64,
}

impl<'a> Session<'a> {
    fn my_files(id: &'a str) -> Self {
        Self {
            id,
            context: "my_files",
            user: Some(ALICE),
            rs_session: None,
            target_folder: None,
            provider: "local",
            state: "created",
            cancel_requested: 0,
        }
    }

    fn reverse_share(id: &'a str) -> Self {
        Self {
            id,
            context: "reverse_share",
            user: None,
            rs_session: Some(RS_SESSION),
            target_folder: None,
            provider: "local",
            state: "created",
            cancel_requested: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Reservation<'a> {
    id: &'a str,
    user: &'a str,
    session: &'a str,
    context: &'a str,
    reserved: i64,
    committed: Option<i64>,
    state: &'a str,
    settled: Option<&'a str>,
    reason: Option<&'a str>,
}

impl<'a> Reservation<'a> {
    fn held(id: &'a str, session: &'a str) -> Self {
        Self {
            id,
            user: ALICE,
            session,
            context: "my_files",
            reserved: 100,
            committed: None,
            state: "held",
            settled: None,
            reason: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Object<'a> {
    id: &'a str,
    key: &'a str,
}

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
        assert_eq!(
            database
                .run(
                    "INSERT INTO folders
                         (id, owner_id, name, name_normalized, depth, created_at, updated_at)
                     VALUES (?1, ?2, 'docs', 'docs', 0, ?3, ?3)",
                    arguments![FOLDER, ALICE, NOW],
                )
                .await?,
            Accepted
        );
        assert_eq!(
            database
                .run(
                    "INSERT INTO reverse_shares
                         (id, owner_id, public_id, alias, created_at, updated_at)
                     VALUES (?1, ?2, 'rs-public-clients0', 'clients', ?3, ?3)",
                    arguments![RS, ALICE, NOW],
                )
                .await?,
            Accepted
        );
        assert_eq!(
            database
                .run(
                    "INSERT INTO reverse_share_upload_sessions
                         (id, reverse_share_id, token_hash, created_at, expires_at, last_activity_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?4)",
                    arguments![RS_SESSION, RS, "a".repeat(64), NOW, LATER],
                )
                .await?,
            Accepted
        );
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
                ErrorKind::NotNullViolation => Ok(NotNull),
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

    async fn count(&mut self, sql: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(sql)
            .fetch_one(&mut self.connection)
            .await?)
    }

    async fn sql_of(&mut self, kind: &str, name: &str) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = ?1 AND name = ?2")
                .bind(kind)
                .bind(name)
                .fetch_one(&mut self.connection)
                .await?,
        )
    }

    async fn primary_key(&mut self, table: &str) -> Result<Vec<(String, i64)>> {
        Ok(
            sqlx::query_as("SELECT name, pk FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk")
                .bind(table)
                .fetch_all(&mut self.connection)
                .await?,
        )
    }

    async fn session(&mut self, spec: Session<'_>) -> Result<Outcome> {
        self.run(
            "INSERT INTO transfer_sessions
                 (id, context, user_id, reverse_share_upload_session_id, target_folder_id,
                  provider, state, cancel_requested, created_at, updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)",
            arguments![
                spec.id,
                spec.context,
                spec.user,
                spec.rs_session,
                spec.target_folder,
                spec.provider,
                spec.state,
                spec.cancel_requested,
                NOW,
                LATER,
            ],
        )
        .await
    }

    async fn file(&mut self, id: &str, session: &str, state: &str, stage: &str) -> Result<Outcome> {
        let (object_id, object_key) = identity_for(id);
        self.file_full(
            id,
            session,
            id,
            state,
            stage,
            Object {
                id: &object_id,
                key: &object_key,
            },
        )
        .await
    }

    async fn file_keyed(&mut self, id: &str, session: &str, client_key: &str) -> Result<Outcome> {
        let (object_id, object_key) = identity_for(id);
        self.file_full(
            id,
            session,
            client_key,
            "pending",
            "none",
            Object {
                id: &object_id,
                key: &object_key,
            },
        )
        .await
    }

    async fn file_identity(
        &mut self,
        id: &str,
        session: &str,
        state: &str,
        stage: &str,
        object: Object<'_>,
    ) -> Result<Outcome> {
        self.file_full(id, session, id, state, stage, object).await
    }

    async fn file_full(
        &mut self,
        id: &str,
        session: &str,
        client_key: &str,
        state: &str,
        stage: &str,
        object: Object<'_>,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
                  upload_kind, state, finalize_stage, final_object_id, final_object_key,
                  created_at, updated_at)
             VALUES (?1, ?2, 0, ?3, ?3, '', 'tus', ?4, ?5, ?6, ?7, ?8, ?8)",
            arguments![id, session, client_key, state, stage, object.id, object.key, NOW,],
        )
        .await
    }

    async fn tus(&mut self, id: &str, file: &str, owner: Option<&str>) -> Result<Outcome> {
        let (owner_user, rs_session) = match owner {
            Some(user) => (Some(user), None),
            None => (None, Some(RS_SESSION)),
        };
        self.run(
            "INSERT INTO tus_uploads
                 (id, transfer_session_file_id, owner_user_id, reverse_share_upload_session_id,
                  upload_length, staging_path, created_at, updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, 10, ?5, ?6, ?6, ?7)",
            arguments![
                id,
                file,
                owner_user,
                rs_session,
                format!("uploads/{id}/blob"),
                NOW,
                LATER,
            ],
        )
        .await
    }

    async fn multipart(
        &mut self,
        id: &str,
        file: &str,
        object_key: &str,
        owner: Option<&str>,
    ) -> Result<Outcome> {
        let (owner_user, rs_session) = match owner {
            Some(user) => (Some(user), None),
            None => (None, Some(RS_SESSION)),
        };
        self.run(
            "INSERT INTO s3_multipart_uploads
                 (id, transfer_session_file_id, s3_upload_id, bucket, object_key,
                  owner_user_id, reverse_share_upload_session_id,
                  part_size_bytes, part_count, created_at, updated_at, expires_at)
             VALUES (?1, ?2, ?3, 'palmr-bucket', ?4, ?5, ?6, 8388608, 2, ?7, ?7, ?8)",
            arguments![
                id,
                file,
                format!("s3-{id}"),
                object_key,
                owner_user,
                rs_session,
                NOW,
                LATER,
            ],
        )
        .await
    }

    async fn part(
        &mut self,
        upload: &str,
        number: i64,
        size: i64,
        etag: Option<&str>,
        state: &str,
        attempts: i64,
    ) -> Result<Outcome> {
        self.run(
            "INSERT INTO s3_multipart_parts
                 (s3_multipart_upload_id, part_number, size_bytes, etag, state, attempts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            arguments![upload, number, size, etag, state, attempts],
        )
        .await
    }

    async fn reservation(&mut self, spec: Reservation<'_>) -> Result<Outcome> {
        self.run(
            "INSERT INTO quota_reservations
                 (id, user_id, transfer_session_id, context, reserved_bytes, committed_bytes,
                  state, created_at, expires_at, settled_at, release_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            arguments![
                spec.id,
                spec.user,
                spec.session,
                spec.context,
                spec.reserved,
                spec.committed,
                spec.state,
                NOW,
                LATER,
                spec.settled,
                spec.reason,
            ],
        )
        .await
    }

    async fn close(self) -> Result<()> {
        self.connection.close().await?;
        self.application.shutdown().await;
        Ok(())
    }
}

fn identity_for(seed: &str) -> (String, String) {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let key = format!("{hash:032x}");
    let object_id = format!("00000000-0000-7000-8000-{:012x}", hash & 0xffff_ffff_ffff);
    (object_id, format!("objects/00/00/{key}"))
}

fn compact(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
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

fn assert_index(
    schema: &Schema,
    table: &str,
    index: &str,
    expected: &[&str],
    unique: bool,
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
    assert_eq!(found.unique, unique, "{index} unique");
    assert_eq!(found.partial, partial, "{index} partial");
    let columns: Vec<Option<String>> = found
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let expected: Vec<Option<String>> = expected
        .iter()
        .map(|name| Some((*name).to_owned()))
        .collect();
    assert_eq!(columns, expected, "{index} columns");
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_transfer_context_pair() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_transfer_context_pair").await?;
    let schema = Schema::introspect(&mut database.connection).await?;

    assert_index(
        &schema,
        "transfer_sessions",
        "ix_transfer_sessions_user",
        &["user_id", "created_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "transfer_sessions",
        "ix_transfer_sessions_live",
        &["user_id", "updated_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "transfer_sessions",
        "ix_transfer_sessions_expiry",
        &["expires_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "transfer_sessions",
        "ix_transfer_sessions_rs",
        &["reverse_share_upload_session_id"],
        false,
        true,
    );
    assert_eq!(
        delete_actions(&schema, "transfer_sessions"),
        [
            action(
                "reverse_share_upload_session_id",
                "reverse_share_upload_sessions",
                "RESTRICT"
            ),
            action("target_folder_id", "folders", "RESTRICT"),
            action("user_id", "users", "CASCADE"),
        ]
    );
    assert!(compact(
        &database
            .sql_of("index", "ix_transfer_sessions_live")
            .await?
    )
    .contains("where state in ('created','uploading','finalizing')"));

    let db = &mut database;
    assert_eq!(db.session(Session::my_files("s-mine")).await?, Accepted);
    assert_eq!(
        db.session(Session::reverse_share("s-reverse")).await?,
        Accepted
    );

    let invalid: [(&str, &str, Option<&str>, Option<&str>); 6] = [
        ("bad-none", "my_files", None, None),
        ("bad-rs-in-my", "my_files", None, Some(RS_SESSION)),
        ("bad-both", "my_files", Some(ALICE), Some(RS_SESSION)),
        ("bad-user-in-rs", "reverse_share", Some(ALICE), None),
        (
            "bad-both-rs",
            "reverse_share",
            Some(ALICE),
            Some(RS_SESSION),
        ),
        ("bad-none-rs", "reverse_share", None, None),
    ];
    for (id, context, user, rs_session) in invalid {
        let mut spec = Session::my_files(id);
        spec.context = context;
        spec.user = user;
        spec.rs_session = rs_session;
        assert_eq!(db.session(spec).await?, Check, "{id}");
    }

    let mut missing_user = Session::my_files("s-missing-user");
    missing_user.user = Some("user-nobody");
    assert_eq!(db.session(missing_user).await?, Foreign);

    let mut missing_rs = Session::reverse_share("s-missing-rs");
    missing_rs.rs_session = Some("rs-nobody");
    assert_eq!(db.session(missing_rs).await?, Foreign);

    let mut mine_folder = Session::my_files("s-folder");
    mine_folder.target_folder = Some(FOLDER);
    assert_eq!(db.session(mine_folder).await?, Accepted);

    let mut unknown_folder = Session::my_files("s-bad-folder");
    unknown_folder.target_folder = Some("folder-nobody");
    assert_eq!(db.session(unknown_folder).await?, Foreign);

    let mut rs_folder = Session::reverse_share("s-rs-folder");
    rs_folder.target_folder = Some(FOLDER);
    assert_eq!(db.session(rs_folder).await?, Check);

    let mut bad_context = Session::my_files("s-bad-context");
    bad_context.context = "shared";
    assert_eq!(db.session(bad_context).await?, Check);

    let mut bad_provider = Session::my_files("s-bad-provider");
    bad_provider.provider = "ftp";
    assert_eq!(db.session(bad_provider).await?, Check);

    let mut bad_cancel = Session::my_files("s-bad-cancel");
    bad_cancel.cancel_requested = 2;
    assert_eq!(db.session(bad_cancel).await?, Check);

    assert_eq!(db.count("SELECT count(*) FROM transfer_sessions").await?, 3);
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_one_protocol_row_per_file() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_one_protocol_row_per_file").await?;
    let schema = Schema::introspect(&mut database.connection).await?;

    assert_index(
        &schema,
        "tus_uploads",
        "ux_tus_uploads_tsf",
        &["transfer_session_file_id"],
        true,
        false,
    );
    assert_index(
        &schema,
        "tus_uploads",
        "ix_tus_uploads_expiry",
        &["expires_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "tus_uploads",
        "ix_tus_uploads_locks",
        &["lock_expires_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "tus_uploads",
        "ix_tus_uploads_owner",
        &["owner_user_id", "created_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "s3_multipart_uploads",
        "ux_s3mp_tsf",
        &["transfer_session_file_id"],
        true,
        false,
    );
    assert_index(
        &schema,
        "s3_multipart_uploads",
        "ux_s3mp_object",
        &["object_key"],
        true,
        false,
    );
    assert_index(
        &schema,
        "s3_multipart_uploads",
        "ux_s3mp_provider",
        &["bucket", "object_key", "s3_upload_id"],
        true,
        false,
    );
    assert_index(
        &schema,
        "s3_multipart_uploads",
        "ix_s3mp_expiry",
        &["expires_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "s3_multipart_uploads",
        "ix_s3mp_reconcile",
        &["last_reconciled_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "transfer_session_files",
        "ux_tsf_session_client_key",
        &["transfer_session_id", "client_file_key"],
        true,
        false,
    );

    assert!(
        compact(&database.sql_of("index", "ix_tus_uploads_locks").await?)
            .contains("where locked_by is not null")
    );
    assert!(
        compact(&database.sql_of("index", "ix_tus_uploads_expiry").await?)
            .contains("where state in ('created','in_progress')")
    );
    assert!(
        compact(&database.sql_of("index", "ix_s3mp_reconcile").await?)
            .contains("where state = 'in_progress'")
    );

    assert_eq!(
        delete_actions(&schema, "tus_uploads"),
        [
            action("owner_user_id", "users", "CASCADE"),
            action(
                "reverse_share_upload_session_id",
                "reverse_share_upload_sessions",
                "RESTRICT"
            ),
            action(
                "transfer_session_file_id",
                "transfer_session_files",
                "RESTRICT"
            ),
        ]
    );
    assert_eq!(
        delete_actions(&schema, "s3_multipart_uploads"),
        [
            action("owner_user_id", "users", "CASCADE"),
            action(
                "reverse_share_upload_session_id",
                "reverse_share_upload_sessions",
                "RESTRICT"
            ),
            action(
                "transfer_session_file_id",
                "transfer_session_files",
                "RESTRICT"
            ),
        ]
    );

    let trigger_count = database
        .count(
            "SELECT count(*) FROM sqlite_schema
              WHERE type = 'trigger'
                AND tbl_name IN ('tus_uploads','s3_multipart_uploads','transfer_session_files')",
        )
        .await?;
    assert_eq!(trigger_count, 0);

    let db = &mut database;
    assert_eq!(db.session(Session::my_files("s")).await?, Accepted);
    assert_eq!(db.file("f1", "s", "pending", "none").await?, Accepted);
    assert_eq!(db.file("f2", "s", "pending", "none").await?, Accepted);

    assert_eq!(db.file_keyed("f-key-a", "s", "shared-key").await?, Accepted);
    assert_eq!(db.file_keyed("f-key-b", "s", "shared-key").await?, Unique);

    assert_eq!(db.tus("t1", "f1", Some(ALICE)).await?, Accepted);
    assert_eq!(db.tus("t1b", "f1", Some(ALICE)).await?, Unique);
    assert_eq!(db.tus("t2", "f2", None).await?, Accepted);

    let key1 = identity_for("f1").1;
    let key2 = identity_for("f2").1;
    assert_eq!(
        db.multipart("m1", "f1", &key1, Some(ALICE)).await?,
        Accepted
    );
    assert_eq!(db.multipart("m1b", "f1", &key1, Some(ALICE)).await?, Unique);
    assert_eq!(db.multipart("m2", "f2", &key1, Some(ALICE)).await?, Unique);
    assert_eq!(db.multipart("m3", "f2", &key2, None).await?, Accepted);

    assert_eq!(
        db.count("SELECT count(*) FROM tus_uploads WHERE transfer_session_file_id = 'f1'")
            .await?,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM s3_multipart_uploads WHERE transfer_session_file_id = 'f1'")
            .await?,
        1
    );
    assert_eq!(
        db.count("SELECT count(*) FROM s3_multipart_uploads WHERE transfer_session_file_id = 'f2'")
            .await?,
        1
    );

    assert_eq!(
        db.exec("DELETE FROM transfer_session_files WHERE id = 'f1'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM transfer_sessions WHERE id = 's'")
            .await?,
        Foreign
    );
    assert_eq!(
        db.exec("DELETE FROM tus_uploads WHERE id = 't1'").await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM s3_multipart_uploads WHERE id = 'm1'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM transfer_session_files WHERE id = 'f1'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM s3_multipart_uploads WHERE id = 'm3'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM tus_uploads WHERE id = 't2'").await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM transfer_session_files WHERE id = 'f2'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.exec("DELETE FROM transfer_sessions WHERE id = 's'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.count("SELECT count(*) FROM transfer_session_files")
            .await?,
        0
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_quota_reservation_states() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_quota_reservation_states").await?;
    let schema = Schema::introspect(&mut database.connection).await?;

    assert_index(
        &schema,
        "quota_reservations",
        "ux_quota_res_session_held",
        &["transfer_session_id"],
        true,
        true,
    );
    assert_index(
        &schema,
        "quota_reservations",
        "ix_quota_res_user_held",
        &["user_id"],
        false,
        true,
    );
    assert_index(
        &schema,
        "quota_reservations",
        "ix_quota_res_expiry",
        &["expires_at"],
        false,
        true,
    );
    assert_index(
        &schema,
        "quota_reservations",
        "ix_quota_res_session",
        &["transfer_session_id"],
        false,
        false,
    );
    assert!(compact(
        &database
            .sql_of("index", "ux_quota_res_session_held")
            .await?
    )
    .contains("where state = 'held'"));
    assert_eq!(
        delete_actions(&schema, "quota_reservations"),
        [
            action("transfer_session_id", "transfer_sessions", "RESTRICT"),
            action("user_id", "users", "CASCADE"),
        ]
    );

    let db = &mut database;
    assert_eq!(db.session(Session::my_files("s1")).await?, Accepted);
    assert_eq!(db.session(Session::my_files("s2")).await?, Accepted);

    assert_eq!(
        db.reservation(Reservation::held("q1", "s1")).await?,
        Accepted
    );
    assert_eq!(
        db.reservation(Reservation::held("q1-twin", "s1")).await?,
        Unique
    );

    let held = Reservation::held("q-held-settled", "s2");
    assert_eq!(
        db.reservation(Reservation {
            settled: Some(NOW),
            ..held
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            committed: Some(5),
            ..Reservation::held("q-held-committed", "s2")
        })
        .await?,
        Check
    );

    assert_eq!(
        db.reservation(Reservation {
            state: "committed",
            committed: Some(7),
            settled: Some(NOW),
            ..Reservation::held("q2", "s2")
        })
        .await?,
        Accepted
    );
    assert_eq!(
        db.reservation(Reservation {
            state: "committed",
            committed: Some(7),
            ..Reservation::held("q2-nosettle", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            state: "committed",
            settled: Some(NOW),
            ..Reservation::held("q2-nobytes", "s2")
        })
        .await?,
        Check
    );

    assert_eq!(
        db.reservation(Reservation {
            state: "released",
            settled: Some(NOW),
            reason: Some("canceled"),
            ..Reservation::held("q3", "s2")
        })
        .await?,
        Accepted
    );
    assert_eq!(
        db.reservation(Reservation {
            state: "released",
            reason: Some("canceled"),
            ..Reservation::held("q3-nosettle", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            state: "released",
            settled: Some(NOW),
            ..Reservation::held("q3-noreason", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            state: "released",
            settled: Some(NOW),
            reason: Some("exploded"),
            ..Reservation::held("q3-badreason", "s2")
        })
        .await?,
        Check
    );

    assert_eq!(
        db.reservation(Reservation {
            state: "expired",
            ..Reservation::held("q4", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            reserved: -1,
            ..Reservation::held("q-negative", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            state: "committed",
            committed: Some(-1),
            settled: Some(NOW),
            ..Reservation::held("q-negative-commit", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            context: "shared",
            ..Reservation::held("q-bad-context", "s2")
        })
        .await?,
        Check
    );
    assert_eq!(
        db.reservation(Reservation {
            user: "user-nobody",
            ..Reservation::held("q-bad-user", "s2")
        })
        .await?,
        Foreign
    );
    assert_eq!(
        db.reservation(Reservation {
            session: "s-nobody",
            ..Reservation::held("q-bad-session", "s2")
        })
        .await?,
        Foreign
    );
    assert_eq!(
        db.reservation(Reservation {
            context: "reverse_share",
            ..Reservation::held("q-rs-context", "s2")
        })
        .await?,
        Accepted
    );

    assert_eq!(
        db.exec(
            "UPDATE quota_reservations
                SET state = 'released', settled_at = '2026-01-03T00:00:00.000Z',
                    release_reason = 'expired'
              WHERE id = 'q1'"
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.reservation(Reservation::held("q1-reheld", "s1")).await?,
        Accepted
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_multipart_parts_without_rowid() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_multipart_parts_without_rowid").await?;
    let schema = Schema::introspect(&mut database.connection).await?;

    let table_sql = database.sql_of("table", "s3_multipart_parts").await?;
    assert!(compact(&table_sql).contains("without rowid"));
    assert_eq!(
        database
            .count("SELECT wr FROM pragma_table_list WHERE name = 's3_multipart_parts'")
            .await?,
        1
    );
    assert_eq!(
        database.primary_key("s3_multipart_parts").await?,
        [
            ("s3_multipart_upload_id".to_owned(), 1),
            ("part_number".to_owned(), 2),
        ]
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM pragma_table_info('s3_multipart_parts') WHERE name = 'id'")
            .await?,
        0
    );
    assert_index(
        &schema,
        "s3_multipart_parts",
        "ix_s3mp_parts_outstanding",
        &["s3_multipart_upload_id", "part_number"],
        false,
        true,
    );
    assert_eq!(
        delete_actions(&schema, "s3_multipart_parts"),
        [action(
            "s3_multipart_upload_id",
            "s3_multipart_uploads",
            "CASCADE"
        )]
    );
    assert!(compact(
        &database
            .sql_of("index", "ix_s3mp_parts_outstanding")
            .await?
    )
    .contains("where state <> 'uploaded'"));

    let db = &mut database;
    assert_eq!(db.session(Session::my_files("s")).await?, Accepted);
    assert_eq!(db.file("f1", "s", "pending", "none").await?, Accepted);
    assert_eq!(db.file("f2", "s", "pending", "none").await?, Accepted);
    assert_eq!(
        db.multipart("m1", "f1", &identity_for("f1").1, Some(ALICE))
            .await?,
        Accepted
    );
    assert_eq!(
        db.multipart("m2", "f2", &identity_for("f2").1, Some(ALICE))
            .await?,
        Accepted
    );

    assert_eq!(db.part("m1", 1, 1024, None, "planned", 0).await?, Accepted);
    assert_eq!(db.part("m1", 2, 1024, None, "uploaded", 0).await?, Check);
    assert_eq!(
        db.part("m1", 2, 1024, Some("etag-2"), "uploaded", 1)
            .await?,
        Accepted
    );
    assert_eq!(
        db.part("m1", 3, 1024, Some("etag-3"), "verified", 1)
            .await?,
        Accepted
    );
    assert_eq!(db.part("m1", 0, 1024, None, "planned", 0).await?, Check);
    assert_eq!(db.part("m1", 10001, 1024, None, "planned", 0).await?, Check);
    assert_eq!(
        db.part("m1", 10000, 1024, None, "planned", 0).await?,
        Accepted
    );
    assert_eq!(db.part("m1", 4, 0, None, "planned", 0).await?, Check);
    assert_eq!(db.part("m1", 4, 1024, None, "bogus", 0).await?, Check);
    assert_eq!(db.part("m1", 4, 1024, None, "planned", -1).await?, Check);
    assert_eq!(db.part("m1", 1, 2048, None, "planned", 0).await?, Unique);

    assert_eq!(db.part("m2", 1, 512, None, "signed", 0).await?, Accepted);
    assert_eq!(
        db.exec("DELETE FROM s3_multipart_uploads WHERE id = 'm1'")
            .await?,
        Accepted
    );
    assert_eq!(
        db.count("SELECT count(*) FROM s3_multipart_parts WHERE s3_multipart_upload_id = 'm1'")
            .await?,
        0
    );
    assert_eq!(
        db.count("SELECT count(*) FROM s3_multipart_parts WHERE s3_multipart_upload_id = 'm2'")
            .await?,
        1
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_transfer_has_no_storage_object_fk() -> Result<()> {
    let mut database =
        MigratedDatabase::open("it_schema_transfer_has_no_storage_object_fk").await?;
    let schema = Schema::introspect(&mut database.connection).await?;

    assert_eq!(byte_owner_cascades(&schema), []);
    for table in TRANSFER_TABLES {
        let introspected = schema
            .table(table)
            .unwrap_or_else(|| panic!("{table} missing"));
        assert!(!introspected.owns_bytes(), "{table} must not own bytes");
        assert!(
            introspected
                .columns
                .iter()
                .all(|column| column.name != "storage_object_id"),
            "{table} must not declare storage_object_id"
        );
        assert!(
            introspected
                .foreign_keys
                .iter()
                .all(|foreign_key| !foreign_key.parent.eq_ignore_ascii_case("storage_objects")),
            "{table} must not reference storage_objects"
        );
    }

    let tsf = schema
        .table("transfer_session_files")
        .unwrap_or_else(|| panic!("transfer_session_files missing"));
    assert!(tsf.column("final_object_id").is_some());
    assert!(tsf.column("final_object_key").is_some());
    assert!(schema
        .table("s3_multipart_uploads")
        .unwrap_or_else(|| panic!("s3_multipart_uploads missing"))
        .column("object_key")
        .is_some());

    let db = &mut database;
    assert_eq!(db.session(Session::my_files("s")).await?, Accepted);
    assert_eq!(db.count("SELECT count(*) FROM storage_objects").await?, 0);
    assert_eq!(db.file("f1", "s", "uploading", "placing").await?, Accepted);
    assert_eq!(db.count("SELECT count(*) FROM storage_objects").await?, 0);
    assert_eq!(
        db.count(
            "SELECT count(*)
               FROM transfer_session_files t
               JOIN storage_objects o ON o.object_key = t.final_object_key"
        )
        .await?,
        0
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_finalize_stage_checks() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_finalize_stage_checks").await?;
    let schema = Schema::introspect(&mut database.connection).await?;

    assert_index(
        &schema,
        "transfer_session_files",
        "ux_tsf_final_object_id",
        &["final_object_id"],
        true,
        false,
    );
    assert_index(
        &schema,
        "transfer_session_files",
        "ux_tsf_final_object_key",
        &["final_object_key"],
        true,
        false,
    );
    assert_index(
        &schema,
        "transfer_session_files",
        "ix_tsf_finalizing",
        &["updated_at"],
        false,
        true,
    );
    assert_eq!(
        delete_actions(&schema, "transfer_session_files"),
        [
            action("resulting_file_id", "files", "SET NULL"),
            action("resulting_received_file_id", "received_files", "SET NULL"),
            action("transfer_session_id", "transfer_sessions", "CASCADE"),
        ]
    );

    let db = &mut database;
    assert_eq!(db.session(Session::my_files("s")).await?, Accepted);

    assert_eq!(
        db.file("f-completed-committed", "s", "completed", "committed")
            .await?,
        Accepted
    );
    assert_eq!(
        db.file("f-completed-none", "s", "completed", "none")
            .await?,
        Check
    );
    assert_eq!(
        db.file("f-completed-placing", "s", "completed", "placing")
            .await?,
        Check
    );

    for state in [
        "pending",
        "uploading",
        "finalizing",
        "failed",
        "canceled",
        "expired",
        "skipped",
    ] {
        assert_eq!(
            db.file(&format!("f-{state}-committed"), "s", state, "committed")
                .await?,
            Check,
            "{state} + committed"
        );
    }

    for (state, stage) in [
        ("pending", "none"),
        ("uploading", "none"),
        ("finalizing", "none"),
        ("finalizing", "placing"),
        ("failed", "placing"),
        ("canceled", "none"),
        ("expired", "placing"),
        ("skipped", "none"),
    ] {
        assert_eq!(
            db.file(&format!("f-{state}-{stage}"), "s", state, stage)
                .await?,
            Accepted,
            "{state} + {stage}"
        );
        assert_eq!(
            db.run(
                "UPDATE transfer_session_files SET state = 'completed', finalize_stage = 'committed'
                  WHERE id = ?1",
                arguments![format!("f-{state}-{stage}")],
            )
            .await?,
            Accepted,
            "{state} + {stage} -> completed"
        );
        assert_eq!(
            db.run(
                "UPDATE transfer_session_files SET state = 'completed', finalize_stage = 'placing'
                  WHERE id = ?1",
                arguments![format!("f-{state}-{stage}")],
            )
            .await?,
            Check,
            "{state} + {stage} -> completed/placing"
        );
    }

    assert_eq!(
        db.run(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name,
                  final_object_key, created_at, updated_at)
             VALUES ('f-missing-object-id', 's', 0, 'f-missing-object-id', 'f-missing-object-id',
                     ?1, ?2, ?2)",
            arguments![identity_for("f-missing-object-id").1, NOW],
        )
        .await?,
        NotNull
    );
    assert_eq!(
        db.run(
            "INSERT INTO transfer_session_files
                 (id, transfer_session_id, ordinal, client_file_key, display_name,
                  final_object_id, created_at, updated_at)
             VALUES ('f-missing-object-key', 's', 0, 'f-missing-object-key', 'f-missing-object-key',
                     ?1, ?2, ?2)",
            arguments![identity_for("f-missing-object-key").0, NOW],
        )
        .await?,
        NotNull
    );

    assert_eq!(
        db.file_identity(
            "f-short-object-id",
            "s",
            "pending",
            "none",
            Object {
                id: "00000000-0000-7000-8000-0000000000",
                key: &identity_for("f-short-object-id").1,
            },
        )
        .await?,
        Check
    );

    let uppercase = format!("objects/00/00/{}", "A".repeat(32));
    let short = format!("objects/00/00/{}", "a".repeat(31));
    let nonhex = format!("objects/00/00/{}", "z".repeat(32));
    let wrong_prefix = format!("blobs/00/00/{}", "a".repeat(32));
    for (id, key) in [
        ("f-upper-key", uppercase.as_str()),
        ("f-short-key", short.as_str()),
        ("f-nonhex-key", nonhex.as_str()),
        ("f-prefix-key", wrong_prefix.as_str()),
    ] {
        assert_eq!(
            db.file_identity(
                id,
                "s",
                "pending",
                "none",
                Object {
                    id: &identity_for(id).0,
                    key,
                },
            )
            .await?,
            Check,
            "{id}"
        );
    }

    let (dup_id, dup_key) = identity_for("dup-identity");
    assert_eq!(
        db.file_identity(
            "f-dup-a",
            "s",
            "pending",
            "none",
            Object {
                id: &dup_id,
                key: &dup_key,
            },
        )
        .await?,
        Accepted
    );
    assert_eq!(
        db.file_identity(
            "f-dup-b",
            "s",
            "pending",
            "none",
            Object {
                id: &dup_id,
                key: &dup_key,
            },
        )
        .await?,
        Unique
    );
    assert_eq!(
        db.file_identity(
            "f-dup-c",
            "s",
            "pending",
            "none",
            Object {
                id: &dup_id,
                key: &identity_for("f-dup-c").1,
            },
        )
        .await?,
        Unique
    );
    assert_eq!(
        db.file_identity(
            "f-dup-d",
            "s",
            "pending",
            "none",
            Object {
                id: &identity_for("f-dup-d").0,
                key: &dup_key,
            },
        )
        .await?,
        Unique
    );
    database.close().await
}

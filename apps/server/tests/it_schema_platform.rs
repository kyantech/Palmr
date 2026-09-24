pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{migrated_schema, IndexColumn};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00Z";
const LATER: &str = "2026-01-01T00:05:00Z";
const HEX: &str = "0123456789abcdef0123456789abcdef";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ROUTE: &str = "/api/v1/received/{id}/copy";
const BRANDING_KINDS: [&str; 7] = [
    "logo",
    "favicon",
    "login_background",
    "email_logo",
    "og_default_image",
    "avatar",
    "hero",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Check,
    Unique,
}

use Outcome::{Accepted, Check, Unique};

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
        Ok(Self {
            application,
            connection,
        })
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
                kind => Err(anyhow::anyhow!("unexpected {kind:?}: {error}")),
            },
            Err(error) => Err(error.into()),
        }
    }

    async fn affected(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<u64> {
        Ok(sqlx::query_with(sql, arguments)
            .execute(&mut self.connection)
            .await?
            .rows_affected())
    }

    async fn count(&mut self, sql: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(sql)
            .fetch_one(&mut self.connection)
            .await?)
    }

    async fn close(self) -> Result<()> {
        self.connection.close().await?;
        self.application.shutdown().await;
        Ok(())
    }
}

macro_rules! arguments {
    ($($value:expr),* $(,)?) => {{
        let mut arguments = SqliteArguments::default();
        $(sqlx::Arguments::add(&mut arguments, $value).expect("bind argument");)*
        arguments
    }};
}

#[derive(Clone, Copy)]
struct StorageObject<'a> {
    key: &'a str,
    state: &'a str,
    refcount: i64,
    finalized_at: Option<&'a str>,
    tombstoned_at: Option<&'a str>,
    deleted_at: Option<&'a str>,
}

impl<'a> StorageObject<'a> {
    const fn active(key: &'a str) -> Self {
        Self {
            key,
            state: "active",
            refcount: 1,
            finalized_at: Some(NOW),
            tombstoned_at: None,
            deleted_at: None,
        }
    }

    const fn tombstoned(key: &'a str) -> Self {
        Self {
            key,
            state: "tombstoned",
            refcount: 0,
            finalized_at: Some(NOW),
            tombstoned_at: Some(LATER),
            deleted_at: None,
        }
    }

    const fn deleted(key: &'a str) -> Self {
        Self {
            key,
            state: "deleted",
            refcount: 0,
            finalized_at: Some(NOW),
            tombstoned_at: Some(LATER),
            deleted_at: Some(LATER),
        }
    }
}

async fn insert_object(
    database: &mut MigratedDatabase,
    object: StorageObject<'_>,
) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO storage_objects
                 (id, object_key, provider, size_bytes, state, refcount,
                  created_at, updated_at, finalized_at, tombstoned_at, deleted_at)
             VALUES (?1, ?2, 'local', 0, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
            arguments![
                format!("so-{}", object.key),
                object.key,
                object.state,
                object.refcount,
                NOW,
                object.finalized_at,
                object.tombstoned_at,
                object.deleted_at,
            ],
        )
        .await
}

fn object_key(prefix: &str) -> String {
    format!("{prefix}{HEX}")
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_storage_object_key_grammar() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_storage_object_key_grammar").await?;

    let mut valid = vec![
        object_key("objects/00/ff/"),
        object_key("objects/a1/9e/"),
        "objects/ff/ff/ffffffffffffffffffffffffffffffff".to_owned(),
    ];
    valid.extend(
        BRANDING_KINDS
            .iter()
            .map(|kind| object_key(&format!("branding/{kind}/"))),
    );
    for key in &valid {
        assert_eq!(
            insert_object(&mut database, StorageObject::active(key)).await?,
            Accepted,
            "{key}"
        );
    }

    let invalid = [
        String::new(),
        HEX.to_owned(),
        object_key("objects/../cd/"),
        object_key("objects/ab/../"),
        object_key("../objects/ab/cd/"),
        object_key("/objects/ab/cd/"),
        object_key("objects/ab/cd/../"),
        "objects/ab/cd/0123456789abcdef0123456789ab/../..".to_owned(),
        object_key("objects\\ab\\cd\\"),
        object_key("objects/AB/cd/"),
        object_key("Objects/ab/cd/"),
        "objects/ab/cd/0123456789ABCDEF0123456789abcdef".to_owned(),
        object_key("objects/ag/cd/"),
        "objects/ab/cd/0123456789abcdef0123456789abcdeg".to_owned(),
        object_key("objects/abc/d/"),
        object_key("objects/a/bcd/"),
        object_key("objects/ab/"),
        object_key("objects/ab/cd/ef/"),
        object_key("objects/ab/cd/0"),
        "objects/ab/cd/0123456789abcdef0123456789abcde".to_owned(),
        object_key("objects//ab/c/"),
        object_key("_palmr/probe/"),
        object_key("thumbnails/ab/cd/"),
        object_key("runtime/cache/"),
        object_key("uploads/ab/cd/"),
        object_key("staging/ab/cd/"),
        object_key("branding/banner/"),
        object_key("branding/Logo/"),
        object_key("branding/logo/x"),
        object_key("branding/hero/../"),
        object_key("branding/"),
        object_key("branding/avatar/ab/"),
        object_key("branding/favicon/x"),
        "branding/logo/".to_owned(),
    ];
    for key in &invalid {
        assert_eq!(
            insert_object(&mut database, StorageObject::active(key)).await?,
            Check,
            "{key}"
        );
    }

    assert_eq!(
        database
            .run(
                "INSERT INTO storage_objects
                     (id, object_key, provider, state, refcount, created_at, updated_at, finalized_at)
                 VALUES ('duplicate', ?1, 's3', 'active', 1, ?2, ?2, ?2)",
                arguments![valid[0].as_str(), NOW],
            )
            .await?,
        Unique
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM storage_objects")
            .await?,
        i64::try_from(valid.len())?
    );
    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_storage_object_state_checks() -> Result<()> {
    let mut database = MigratedDatabase::open("it_schema_storage_object_state_checks").await?;
    let keys: Vec<String> = (0..32)
        .map(|index| object_key(&format!("objects/{index:02x}/00/")))
        .collect();
    let key = |index: usize| keys[index].as_str();

    let cases = [
        (StorageObject::active(key(0)), Accepted),
        (
            StorageObject {
                refcount: 0,
                ..StorageObject::active(key(1))
            },
            Check,
        ),
        (
            StorageObject {
                finalized_at: None,
                ..StorageObject::active(key(2))
            },
            Check,
        ),
        (
            StorageObject {
                refcount: 2,
                ..StorageObject::active(key(3))
            },
            Check,
        ),
        (
            StorageObject {
                tombstoned_at: Some(LATER),
                ..StorageObject::active(key(4))
            },
            Check,
        ),
        (
            StorageObject {
                deleted_at: Some(LATER),
                ..StorageObject::active(key(5))
            },
            Check,
        ),
        (StorageObject::tombstoned(key(6)), Accepted),
        (
            StorageObject {
                finalized_at: None,
                ..StorageObject::tombstoned(key(7))
            },
            Accepted,
        ),
        (
            StorageObject {
                refcount: 1,
                ..StorageObject::tombstoned(key(8))
            },
            Check,
        ),
        (
            StorageObject {
                tombstoned_at: None,
                ..StorageObject::tombstoned(key(9))
            },
            Check,
        ),
        (
            StorageObject {
                deleted_at: Some(LATER),
                ..StorageObject::tombstoned(key(10))
            },
            Check,
        ),
        (
            StorageObject {
                refcount: -1,
                ..StorageObject::tombstoned(key(11))
            },
            Check,
        ),
        (StorageObject::deleted(key(12)), Accepted),
        (
            StorageObject {
                refcount: 1,
                ..StorageObject::deleted(key(13))
            },
            Check,
        ),
        (
            StorageObject {
                tombstoned_at: None,
                ..StorageObject::deleted(key(14))
            },
            Check,
        ),
        (
            StorageObject {
                deleted_at: None,
                ..StorageObject::deleted(key(15))
            },
            Check,
        ),
        (
            StorageObject {
                refcount: 2,
                ..StorageObject::deleted(key(16))
            },
            Check,
        ),
        (
            StorageObject {
                state: "unknown",
                ..StorageObject::active(key(17))
            },
            Check,
        ),
        (
            StorageObject {
                state: "ACTIVE",
                ..StorageObject::active(key(18))
            },
            Check,
        ),
    ];
    for (object, expected) in cases {
        assert_eq!(
            insert_object(&mut database, object).await?,
            expected,
            "{} refcount={} finalized={:?} tombstoned={:?} deleted={:?}",
            object.state,
            object.refcount,
            object.finalized_at,
            object.tombstoned_at,
            object.deleted_at
        );
    }

    assert_eq!(
        database
            .run(
                "INSERT INTO storage_objects (id, object_key, provider, created_at, updated_at)
                 VALUES ('defaults', ?1, 'local', ?2, ?2)",
                arguments![key(19), NOW],
            )
            .await?,
        Check
    );
    assert_eq!(
        database
            .run(
                "INSERT INTO storage_objects
                     (id, object_key, provider, state, refcount, created_at, updated_at, finalized_at)
                 VALUES ('ftp', ?1, 'ftp', 'active', 1, ?2, ?2, ?2)",
                arguments![key(20), NOW],
            )
            .await?,
        Check
    );
    for (checksum, algorithm) in [
        (Some(DIGEST), None),
        (None, Some("sha256")),
        (Some(&DIGEST[1..]), Some("sha256")),
        (Some(DIGEST), Some("md5")),
    ] {
        assert_eq!(
            database
                .run(
                    "INSERT INTO storage_objects
                         (id, object_key, provider, checksum, checksum_algo, state, refcount,
                          created_at, updated_at, finalized_at)
                     VALUES ('checksum', ?1, 'local', ?2, ?3, 'active', 1, ?4, ?4, ?4)",
                    arguments![key(21), checksum, algorithm, NOW],
                )
                .await?,
            Check
        );
    }

    let lifecycle = key(0);
    for (transition, expected) in [
        (
            "UPDATE storage_objects SET state = 'tombstoned', tombstoned_at = ?2 WHERE object_key = ?1",
            Check,
        ),
        (
            "UPDATE storage_objects SET refcount = 0 WHERE object_key = ?1 AND ?2 IS NOT NULL",
            Check,
        ),
        (
            "UPDATE storage_objects SET state = 'tombstoned', refcount = 0, tombstoned_at = ?2 WHERE object_key = ?1",
            Accepted,
        ),
        (
            "UPDATE storage_objects SET state = 'deleted' WHERE object_key = ?1 AND ?2 IS NOT NULL",
            Check,
        ),
        (
            "UPDATE storage_objects SET state = 'deleted', deleted_at = ?2 WHERE object_key = ?1",
            Accepted,
        ),
    ] {
        assert_eq!(
            database.run(transition, arguments![lifecycle, LATER]).await?,
            expected,
            "{transition}"
        );
    }

    assert_eq!(
        database
            .count("SELECT count(*) FROM storage_objects WHERE state = 'active'")
            .await?,
        0
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM storage_objects WHERE state = 'active' AND (refcount <> 1 OR finalized_at IS NULL)")
            .await?,
        0
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM storage_objects")
            .await?,
        4
    );
    database.close().await
}

async fn insert_job(
    database: &mut MigratedDatabase,
    id: &str,
    dedup_key: Option<&str>,
    conflict: &str,
) -> Result<u64> {
    database
        .affected(
            &format!(
                "INSERT INTO jobs (id, kind, payload_json, run_at, dedup_key, created_at, updated_at)
                 VALUES (?1, 'storage.delete_blob', json_object('job', ?1), ?2, ?3, ?2, ?2) {conflict}"
            ),
            arguments![id, NOW, dedup_key],
        )
        .await
}

const DEDUP_CONFLICT: &str = "ON CONFLICT (dedup_key) WHERE dedup_key IS NOT NULL DO NOTHING";

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_jobs_dedup_partial_unique() -> Result<()> {
    let schema = migrated_schema("it_schema_jobs_dedup_partial_unique").await?;
    let jobs = schema.table("jobs").context("jobs table")?;
    let index = jobs
        .indexes
        .iter()
        .find(|index| index.name == "ux_jobs_dedup_key")
        .context("ux_jobs_dedup_key")?;
    assert!(index.unique && index.partial);
    assert_eq!(
        index.columns,
        [IndexColumn {
            name: Some("dedup_key".to_owned()),
            collation: "BINARY".to_owned(),
        }]
    );

    let mut database = MigratedDatabase::open("it_schema_jobs_dedup_partial_unique").await?;
    for id in ["plain-1", "plain-2", "plain-3"] {
        assert_eq!(insert_job(&mut database, id, None, "").await?, 1);
    }
    assert_eq!(
        insert_job(&mut database, "plain-4", None, DEDUP_CONFLICT).await?,
        1
    );

    assert_eq!(
        insert_job(
            &mut database,
            "first",
            Some("storage.delete_blob:so-1"),
            DEDUP_CONFLICT
        )
        .await?,
        1
    );
    assert_eq!(
        insert_job(
            &mut database,
            "retry",
            Some("storage.delete_blob:so-1"),
            DEDUP_CONFLICT
        )
        .await?,
        0
    );
    assert_eq!(
        insert_job(
            &mut database,
            "untargeted",
            Some("storage.delete_blob:so-1"),
            "ON CONFLICT DO NOTHING"
        )
        .await?,
        0
    );
    assert_eq!(
        database
            .run(
                "INSERT INTO jobs (id, kind, run_at, dedup_key, created_at, updated_at)
                 VALUES ('duplicate', 'storage.delete_blob', ?1, 'storage.delete_blob:so-1', ?1, ?1)",
                arguments![NOW],
            )
            .await?,
        Unique
    );
    assert_eq!(
        insert_job(
            &mut database,
            "other",
            Some("storage.delete_blob:so-2"),
            DEDUP_CONFLICT
        )
        .await?,
        1
    );
    assert_eq!(
        insert_job(
            &mut database,
            "cased",
            Some("STORAGE.DELETE_BLOB:SO-1"),
            DEDUP_CONFLICT
        )
        .await?,
        1
    );

    assert_eq!(
        database
            .run(
                "UPDATE jobs SET state = 'succeeded' WHERE id = 'first' AND ?1 IS NOT NULL",
                arguments![NOW],
            )
            .await?,
        Accepted
    );
    assert_eq!(
        insert_job(
            &mut database,
            "after",
            Some("storage.delete_blob:so-1"),
            DEDUP_CONFLICT
        )
        .await?,
        0
    );

    assert_eq!(
        database
            .count("SELECT count(*) FROM jobs WHERE dedup_key IS NULL")
            .await?,
        4
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM jobs WHERE dedup_key = 'storage.delete_blob:so-1' AND id = 'first' AND payload_json = '{\"job\":\"first\"}'")
            .await?,
        1
    );
    assert_eq!(database.count("SELECT count(*) FROM jobs").await?, 7);
    database.close().await
}

struct Outbox<'a> {
    id: &'a str,
    state: &'a str,
    sent_at: Option<&'a str>,
    token_ciphertext: Option<Vec<u8>>,
    token_nonce: Option<Vec<u8>>,
    key_version: Option<i64>,
    dedup_key: Option<&'a str>,
}

impl<'a> Outbox<'a> {
    fn sealed(id: &'a str) -> Self {
        Self {
            id,
            state: "pending",
            sent_at: None,
            token_ciphertext: Some(vec![7; 48]),
            token_nonce: Some(vec![9; 24]),
            key_version: Some(1),
            dedup_key: None,
        }
    }

    fn plain(id: &'a str) -> Self {
        Self {
            token_ciphertext: None,
            token_nonce: None,
            key_version: None,
            ..Self::sealed(id)
        }
    }
}

async fn insert_outbox(database: &mut MigratedDatabase, row: Outbox<'_>) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO email_outbox
                 (id, kind, to_email, state, dedup_key, scheduled_at, created_at, updated_at,
                  sent_at, token_ciphertext, token_nonce, key_version)
             VALUES (?1, 'password_reset', 'user@example.com', ?2, ?3, ?4, ?4, ?4, ?5, ?6, ?7, ?8)",
            arguments![
                row.id,
                row.state,
                row.dedup_key,
                NOW,
                row.sent_at,
                row.token_ciphertext,
                row.token_nonce,
                row.key_version,
            ],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_outbox_token_wiped_when_terminal() -> Result<()> {
    let schema = migrated_schema("it_schema_outbox_token_wiped_when_terminal").await?;
    let outbox = schema.table("email_outbox").context("email_outbox table")?;
    let index = outbox
        .indexes
        .iter()
        .find(|index| index.name == "ux_email_outbox_dedup_key")
        .context("ux_email_outbox_dedup_key")?;
    assert!(index.unique && index.partial);
    assert_eq!(index.columns.len(), 1);
    assert_eq!(index.columns[0].name.as_deref(), Some("dedup_key"));
    for column in ["token_ciphertext", "token_nonce"] {
        assert_eq!(
            outbox
                .column(column)
                .context("sealed column")?
                .declared_type,
            "BLOB"
        );
    }
    assert_eq!(
        outbox
            .columns
            .iter()
            .filter(|column| column.name.contains("token"))
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["token_ciphertext", "token_nonce"]
    );

    let mut database = MigratedDatabase::open("it_schema_outbox_token_wiped_when_terminal").await?;
    let cases = [
        (Outbox::sealed("pending-sealed"), Accepted),
        (
            Outbox {
                state: "sending",
                ..Outbox::sealed("sending-sealed")
            },
            Accepted,
        ),
        (Outbox::plain("pending-plain"), Accepted),
        (
            Outbox {
                state: "sent",
                sent_at: Some(LATER),
                ..Outbox::sealed("sent-sealed")
            },
            Check,
        ),
        (
            Outbox {
                state: "failed",
                ..Outbox::sealed("failed-sealed")
            },
            Check,
        ),
        (
            Outbox {
                state: "canceled",
                ..Outbox::sealed("canceled-sealed")
            },
            Check,
        ),
        (
            Outbox {
                state: "sent",
                sent_at: Some(LATER),
                ..Outbox::plain("sent-plain")
            },
            Accepted,
        ),
        (
            Outbox {
                state: "sent",
                ..Outbox::plain("sent-untimed")
            },
            Check,
        ),
        (
            Outbox {
                state: "failed",
                ..Outbox::plain("failed-plain")
            },
            Accepted,
        ),
        (
            Outbox {
                state: "canceled",
                ..Outbox::plain("canceled-plain")
            },
            Accepted,
        ),
        (
            Outbox {
                state: "queued",
                ..Outbox::plain("unknown-state")
            },
            Check,
        ),
        (
            Outbox {
                token_nonce: None,
                ..Outbox::sealed("no-nonce")
            },
            Check,
        ),
        (
            Outbox {
                key_version: None,
                ..Outbox::sealed("no-version")
            },
            Check,
        ),
        (
            Outbox {
                token_ciphertext: None,
                ..Outbox::sealed("no-ciphertext")
            },
            Check,
        ),
        (
            Outbox {
                token_nonce: Some(vec![9; 23]),
                ..Outbox::sealed("short-nonce")
            },
            Check,
        ),
        (
            Outbox {
                key_version: Some(0),
                ..Outbox::sealed("zero-version")
            },
            Check,
        ),
    ];
    for (row, expected) in cases {
        let id = row.id;
        assert_eq!(insert_outbox(&mut database, row).await?, expected, "{id}");
    }

    let wipe = "UPDATE email_outbox SET state = ?2, sent_at = ?3 WHERE id = ?1";
    let wipe_sealed = "UPDATE email_outbox
                       SET state = ?2, sent_at = ?3,
                           token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
                       WHERE id = ?1";
    for (statement, id, state, sent_at, expected) in [
        (wipe, "pending-sealed", "sending", None, Accepted),
        (wipe, "pending-sealed", "sent", Some(LATER), Check),
        (wipe, "pending-sealed", "failed", None, Check),
        (wipe, "pending-sealed", "canceled", None, Check),
        (
            "UPDATE email_outbox SET state = ?2, sent_at = ?3, token_ciphertext = NULL WHERE id = ?1",
            "pending-sealed",
            "sent",
            Some(LATER),
            Check,
        ),
        (wipe_sealed, "pending-sealed", "sent", Some(LATER), Accepted),
        (wipe_sealed, "sending-sealed", "failed", None, Accepted),
        (
            "UPDATE email_outbox SET token_ciphertext = x'00', token_nonce = zeroblob(24), key_version = 1, state = ?2, sent_at = ?3 WHERE id = ?1",
            "canceled-plain",
            "canceled",
            None,
            Check,
        ),
    ] {
        assert_eq!(
            database.run(statement, arguments![id, state, sent_at]).await?,
            expected,
            "{id} -> {state}"
        );
    }

    assert_eq!(
        database
            .count("SELECT count(*) FROM email_outbox WHERE state NOT IN ('pending','sending') AND (token_ciphertext IS NOT NULL OR token_nonce IS NOT NULL OR key_version IS NOT NULL)")
            .await?,
        0
    );

    for (id, dedup_key, expected) in [
        ("dedup-null-1", None, Accepted),
        ("dedup-null-2", None, Accepted),
        ("dedup-first", Some("share:1:abc:1"), Accepted),
        ("dedup-second", Some("share:1:abc:1"), Unique),
    ] {
        assert_eq!(
            insert_outbox(
                &mut database,
                Outbox {
                    dedup_key,
                    ..Outbox::plain(id)
                }
            )
            .await?,
            expected,
            "{id}"
        );
    }
    assert_eq!(
        database
            .affected(
                "INSERT INTO email_outbox (id, kind, to_email, dedup_key, scheduled_at, created_at, updated_at)
                 VALUES ('dedup-retry', 'invite', 'user@example.com', 'share:1:abc:1', ?1, ?1, ?1)
                 ON CONFLICT (dedup_key) WHERE dedup_key IS NOT NULL DO NOTHING",
                arguments![NOW],
            )
            .await?,
        0
    );
    database.close().await
}

struct Idempotency<'a> {
    id: &'a str,
    scope_kind: &'a str,
    scope_id: &'a str,
    http_method: &'a str,
    route_template: &'a str,
    key_hash: String,
    request_hash: String,
    state: &'a str,
    lease_expires_at: Option<&'a str>,
    response_status: Option<i64>,
    response_json: Option<&'a str>,
    response_ciphertext: Option<Vec<u8>>,
    response_nonce: Option<Vec<u8>>,
    key_version: Option<i64>,
    completed_at: Option<&'a str>,
}

impl<'a> Idempotency<'a> {
    fn in_progress(id: &'a str) -> Self {
        Self {
            id,
            scope_kind: "user",
            scope_id: "01J00000000000000000000000",
            http_method: "POST",
            route_template: ROUTE,
            key_hash: DIGEST.to_owned(),
            request_hash: DIGEST.to_owned(),
            state: "in_progress",
            lease_expires_at: Some(LATER),
            response_status: None,
            response_json: None,
            response_ciphertext: None,
            response_nonce: None,
            key_version: None,
            completed_at: None,
        }
    }

    fn completed_plain(id: &'a str) -> Self {
        Self {
            state: "completed",
            lease_expires_at: None,
            response_status: Some(201),
            response_json: Some(
                r#"{"body":{"id":"f1"},"headers":{"Location":"/api/v1/files/f1"}}"#,
            ),
            completed_at: Some(LATER),
            ..Self::in_progress(id)
        }
    }

    fn completed_sealed(id: &'a str) -> Self {
        Self {
            response_json: None,
            response_ciphertext: Some(vec![5; 64]),
            response_nonce: Some(vec![6; 24]),
            key_version: Some(1),
            ..Self::completed_plain(id)
        }
    }

    fn key(self, seed: char) -> Self {
        Self {
            key_hash: seed.to_string().repeat(64),
            ..self
        }
    }
}

async fn insert_idempotency(
    database: &mut MigratedDatabase,
    row: Idempotency<'_>,
) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO idempotency_records
                 (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash,
                  state, lease_expires_at, response_status, response_json, response_ciphertext,
                  response_nonce, key_version, created_at, completed_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            arguments![
                row.id,
                row.scope_kind,
                row.scope_id,
                row.http_method,
                row.route_template,
                row.key_hash,
                row.request_hash,
                row.state,
                row.lease_expires_at,
                row.response_status,
                row.response_json,
                row.response_ciphertext,
                row.response_nonce,
                row.key_version,
                NOW,
                row.completed_at,
                "2026-01-02T00:00:00Z",
            ],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_idempotency_scope_unique_and_state_checks() -> Result<()> {
    let schema = migrated_schema("it_schema_idempotency_scope_unique_and_state_checks").await?;
    let records = schema
        .table("idempotency_records")
        .context("idempotency_records table")?;
    assert_eq!(records.foreign_keys, []);
    let scope = records
        .indexes
        .iter()
        .find(|index| index.name == "ux_idempotency_scope")
        .context("ux_idempotency_scope")?;
    assert!(scope.unique && !scope.partial);
    assert_eq!(
        scope
            .columns
            .iter()
            .map(|column| column.name.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("scope_kind"),
            Some("scope_id"),
            Some("http_method"),
            Some("route_template"),
            Some("key_hash"),
        ]
    );

    let mut database =
        MigratedDatabase::open("it_schema_idempotency_scope_unique_and_state_checks").await?;
    let other_request = "f".repeat(64);
    let scope_cases = [
        (Idempotency::in_progress("base"), Accepted),
        (
            Idempotency {
                request_hash: other_request.clone(),
                ..Idempotency::in_progress("same-scope-other-request")
            },
            Unique,
        ),
        (Idempotency::completed_plain("same-scope-completed"), Unique),
        (
            Idempotency {
                scope_kind: "reverse_share_grant",
                ..Idempotency::in_progress("other-kind")
            },
            Accepted,
        ),
        (
            Idempotency {
                scope_id: "01J00000000000000000000001",
                ..Idempotency::in_progress("other-principal")
            },
            Accepted,
        ),
        (
            Idempotency {
                http_method: "PUT",
                ..Idempotency::in_progress("other-method")
            },
            Accepted,
        ),
        (
            Idempotency {
                route_template: "/api/v1/files",
                ..Idempotency::in_progress("other-route")
            },
            Accepted,
        ),
        (Idempotency::in_progress("other-key").key('a'), Accepted),
    ];
    for (row, expected) in scope_cases {
        let id = row.id;
        assert_eq!(
            insert_idempotency(&mut database, row).await?,
            expected,
            "{id}"
        );
    }

    let state_cases = [
        (Idempotency::completed_plain("plain").key('1'), Accepted),
        (Idempotency::completed_sealed("sealed").key('2'), Accepted),
        (
            Idempotency {
                lease_expires_at: None,
                ..Idempotency::in_progress("no-lease")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_status: Some(201),
                ..Idempotency::in_progress("early-status")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                completed_at: Some(LATER),
                ..Idempotency::in_progress("early-completion")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_json: Some("{}"),
                ..Idempotency::in_progress("early-body")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                lease_expires_at: Some(LATER),
                ..Idempotency::completed_plain("leased")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                completed_at: None,
                ..Idempotency::completed_plain("untimed")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_status: None,
                ..Idempotency::completed_plain("no-status")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_json: None,
                ..Idempotency::completed_plain("no-body")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_json: Some("{}"),
                ..Idempotency::completed_sealed("both-bodies")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_nonce: None,
                ..Idempotency::completed_sealed("no-nonce")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                key_version: None,
                ..Idempotency::completed_sealed("no-version")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_nonce: Some(vec![6; 12]),
                ..Idempotency::completed_sealed("short-nonce")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_ciphertext: Some(vec![5; 16_401]),
                ..Idempotency::completed_sealed("oversized-ciphertext")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_json: Some("{not json"),
                ..Idempotency::completed_plain("invalid-json")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_status: Some(199),
                ..Idempotency::completed_plain("low-status")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                response_status: Some(600),
                ..Idempotency::completed_plain("high-status")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                state: "failed",
                ..Idempotency::in_progress("unknown-state")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                scope_kind: "session",
                ..Idempotency::in_progress("unknown-scope")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                scope_id: "",
                ..Idempotency::in_progress("empty-scope")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                http_method: "GET",
                ..Idempotency::in_progress("safe-method")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                http_method: "post",
                ..Idempotency::in_progress("lower-method")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                route_template: "/api/v2/files",
                ..Idempotency::in_progress("foreign-route")
            }
            .key('3'),
            Check,
        ),
        (
            Idempotency {
                key_hash: DIGEST[1..].to_owned(),
                ..Idempotency::in_progress("short-key")
            },
            Check,
        ),
        (
            Idempotency {
                request_hash: format!("{DIGEST}0"),
                ..Idempotency::in_progress("long-request")
            }
            .key('3'),
            Check,
        ),
    ];
    for (row, expected) in state_cases {
        let id = row.id;
        assert_eq!(
            insert_idempotency(&mut database, row).await?,
            expected,
            "{id}"
        );
    }

    for (statement, expected) in [
        (
            "UPDATE idempotency_records SET state = 'completed', completed_at = ?2 WHERE id = ?1",
            Check,
        ),
        (
            "UPDATE idempotency_records
             SET state = 'completed', completed_at = ?2, lease_expires_at = NULL,
                 response_status = 201, response_json = '{\"body\":null,\"headers\":{}}'
             WHERE id = ?1",
            Accepted,
        ),
        (
            "UPDATE idempotency_records SET state = 'in_progress', lease_expires_at = ?2 WHERE id = ?1",
            Check,
        ),
    ] {
        assert_eq!(
            database.run(statement, arguments!["base", LATER]).await?,
            expected,
            "{statement}"
        );
    }

    assert_eq!(
        database
            .run(
                "INSERT INTO idempotency_records
                     (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash,
                      lease_expires_at, created_at, expires_at)
                 VALUES ('dangling', 'reverse_share_link', 'no-such-reverse-share', 'POST',
                         '/api/v1/public/reverse-shares/{alias}/sessions', ?1, ?1, ?2, ?2, ?2)",
                arguments![other_request.as_str(), NOW],
            )
            .await?,
        Accepted
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM idempotency_records WHERE state = 'in_progress'")
            .await?,
        6
    );
    assert_eq!(
        database
            .count("SELECT count(*) FROM idempotency_records")
            .await?,
        9
    );
    database.close().await
}

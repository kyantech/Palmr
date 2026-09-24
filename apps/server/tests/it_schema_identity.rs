pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{migrated_schema, IndexColumn, Table};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00.000Z";
const LATER: &str = "2026-01-01T00:05:00.000Z";
const RESTRICT_VIOLATION: &str = "1811";
const ACTIVE_ADMINS: &str = "role = 'admin' AND is_active = 1";

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
    unenforced: SqliteConnection,
}

impl MigratedDatabase {
    async fn open(test_name: &str) -> Result<Self> {
        let application = TestApplication::start(test_name).await?;
        let path = application.data_dir().join(DATABASE_FILE);
        let connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .context("open migrated database for writing")?;
        let unenforced = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(false),
        )
        .await
        .context("open migrated database without foreign key enforcement")?;
        Ok(Self {
            application,
            connection,
            unenforced,
        })
    }

    async fn run(&mut self, sql: &str, arguments: SqliteArguments<'_>) -> Result<Outcome> {
        outcome(
            sqlx::query_with(sql, arguments)
                .execute(&mut self.connection)
                .await,
        )
    }

    async fn run_unenforced(
        &mut self,
        sql: &str,
        arguments: SqliteArguments<'_>,
    ) -> Result<Outcome> {
        outcome(
            sqlx::query_with(sql, arguments)
                .execute(&mut self.unenforced)
                .await,
        )
    }

    async fn ids(&mut self, sql: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(sql)
            .fetch_all(&mut self.connection)
            .await?)
    }

    async fn close(self) -> Result<()> {
        self.connection.close().await?;
        self.unenforced.close().await?;
        self.application.shutdown().await;
        Ok(())
    }
}

fn outcome(result: Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>) -> Result<Outcome> {
    match result {
        Ok(_) => Ok(Accepted),
        Err(sqlx::Error::Database(error)) => match error.kind() {
            ErrorKind::CheckViolation => Ok(Check),
            ErrorKind::UniqueViolation => Ok(Unique),
            ErrorKind::ForeignKeyViolation => Ok(Foreign),
            ErrorKind::Other if error.code().as_deref() == Some(RESTRICT_VIOLATION) => Ok(Foreign),
            kind => Err(anyhow::anyhow!("unexpected {kind:?}: {error}")),
        },
        Err(error) => Err(error.into()),
    }
}

macro_rules! arguments {
    ($($value:expr),* $(,)?) => {{
        let mut arguments = SqliteArguments::default();
        $(sqlx::Arguments::add(&mut arguments, $value).expect("bind argument");)*
        arguments
    }};
}

fn digest(seed: u64) -> String {
    format!("{seed:064x}")
}

fn compact(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy)]
struct User<'a> {
    id: &'a str,
    email: &'a str,
    email_normalized: &'a str,
    username: &'a str,
    username_normalized: &'a str,
    pending_email: Option<&'a str>,
    pending_email_normalized: Option<&'a str>,
    role: &'a str,
    is_active: i64,
    deactivated_at: Option<&'a str>,
    quota_override_mode: &'a str,
    quota_bytes: Option<i64>,
}

impl<'a> User<'a> {
    const fn new(
        id: &'a str,
        email: &'a str,
        email_normalized: &'a str,
        username: &'a str,
        username_normalized: &'a str,
    ) -> Self {
        Self {
            id,
            email,
            email_normalized,
            username,
            username_normalized,
            pending_email: None,
            pending_email_normalized: None,
            role: "user",
            is_active: 1,
            deactivated_at: None,
            quota_override_mode: "inherit",
            quota_bytes: None,
        }
    }

    const fn plain(id: &'a str) -> Self {
        Self::new(id, id, id, id, id)
    }
}

async fn insert_user(database: &mut MigratedDatabase, user: User<'_>) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO users
                 (id, email, email_normalized, username, username_normalized,
                  pending_email, pending_email_normalized, role, is_active, deactivated_at,
                  quota_override_mode, quota_bytes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            arguments![
                user.id,
                user.email,
                user.email_normalized,
                user.username,
                user.username_normalized,
                user.pending_email,
                user.pending_email_normalized,
                user.role,
                user.is_active,
                user.deactivated_at,
                user.quota_override_mode,
                user.quota_bytes,
                NOW,
            ],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_users_normalized_unique() -> Result<()> {
    let schema = migrated_schema("it_schema_users_normalized_unique").await?;
    let users = schema.table("users").context("users table")?;
    let mut unique: Vec<(String, bool, Vec<IndexColumn>)> = users
        .indexes
        .iter()
        .filter(|index| index.unique && !index.name.starts_with("sqlite_"))
        .map(|index| (index.name.clone(), index.partial, index.columns.clone()))
        .collect();
    unique.sort_by(|left, right| left.0.cmp(&right.0));
    let key = |name: &str| IndexColumn {
        name: Some(name.to_owned()),
        collation: "BINARY".to_owned(),
    };
    assert_eq!(
        unique,
        [
            (
                "ux_users_email_normalized".to_owned(),
                false,
                vec![key("email_normalized")]
            ),
            (
                "ux_users_pending_email_normalized".to_owned(),
                true,
                vec![key("pending_email_normalized")]
            ),
            (
                "ux_users_username_normalized".to_owned(),
                false,
                vec![key("username_normalized")]
            ),
        ]
    );
    for column in [
        "email_normalized",
        "username_normalized",
        "pending_email_normalized",
    ] {
        assert_eq!(
            users
                .column(column)
                .context("normalized column")?
                .declared_type,
            "TEXT"
        );
    }
    let sql = compact(&users.sql).to_ascii_lowercase();
    for forbidden in ["generated", "nocase", "lower("] {
        assert!(!sql.contains(forbidden), "users DDL contains {forbidden}");
    }

    let mut database = MigratedDatabase::open("it_schema_users_normalized_unique").await?;
    let generated: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pragma_table_xinfo('users') WHERE hidden <> 0")
            .fetch_one(&mut database.connection)
            .await?;
    assert_eq!(generated, 0);

    let alice = User::new(
        "u-alice",
        "Alice@Example.com",
        "alice@example.com",
        "Alice",
        "alice",
    );
    assert_eq!(insert_user(&mut database, alice).await?, Accepted);

    let cases = [
        (
            User::new(
                "u-email-case",
                "ALICE@EXAMPLE.COM",
                "alice@example.com",
                "someone",
                "someone",
            ),
            Unique,
        ),
        (
            User::new(
                "u-username-case",
                "other@example.com",
                "other@example.com",
                "ALICE",
                "alice",
            ),
            Unique,
        ),
        (
            User::new(
                "u-display-reuse",
                "Alice@Example.com",
                "alice+display@example.com",
                "Alice",
                "alice-display",
            ),
            Accepted,
        ),
        (
            User::new(
                "u-strasse-sharp",
                "sharp@example.com",
                "sharp@example.com",
                "STRA\u{1E9E}E",
                "stra\u{DF}e",
            ),
            Accepted,
        ),
        (
            User::new(
                "u-strasse-lower",
                "lower@example.com",
                "lower@example.com",
                "stra\u{DF}e",
                "stra\u{DF}e",
            ),
            Unique,
        ),
        (
            User::new(
                "u-strasse-ascii",
                "ascii@example.com",
                "ascii@example.com",
                "STRASSE",
                "strasse",
            ),
            Accepted,
        ),
    ];
    for (user, expected) in cases {
        assert_eq!(
            insert_user(&mut database, user).await?,
            expected,
            "{}",
            user.id
        );
    }

    let pending = User {
        pending_email: Some("New@Example.com"),
        pending_email_normalized: Some("new@example.com"),
        ..User::plain("u-pending-first")
    };
    let pending_cases = [
        (pending, Accepted),
        (
            User {
                pending_email: Some("NEW@EXAMPLE.COM"),
                pending_email_normalized: Some("new@example.com"),
                ..User::plain("u-pending-second")
            },
            Unique,
        ),
        (
            User {
                pending_email: Some("New@Example.com"),
                pending_email_normalized: Some("new+other@example.com"),
                ..User::plain("u-pending-display")
            },
            Accepted,
        ),
        (User::plain("u-pending-none-a"), Accepted),
        (User::plain("u-pending-none-b"), Accepted),
        (
            User {
                pending_email: Some("half@example.com"),
                ..User::plain("u-pending-half")
            },
            Check,
        ),
        (
            User {
                pending_email_normalized: Some("half@example.com"),
                ..User::plain("u-pending-orphan")
            },
            Check,
        ),
    ];
    for (user, expected) in pending_cases {
        assert_eq!(
            insert_user(&mut database, user).await?,
            expected,
            "{}",
            user.id
        );
    }

    let consistency = [
        (
            User {
                quota_override_mode: "bytes",
                quota_bytes: Some(0),
                ..User::plain("u-quota-bytes")
            },
            Accepted,
        ),
        (
            User {
                quota_override_mode: "bytes",
                ..User::plain("u-quota-missing")
            },
            Check,
        ),
        (
            User {
                quota_override_mode: "unlimited",
                quota_bytes: Some(10),
                ..User::plain("u-quota-stray")
            },
            Check,
        ),
        (
            User {
                is_active: 0,
                ..User::plain("u-inactive-untimed")
            },
            Check,
        ),
        (
            User {
                deactivated_at: Some(NOW),
                ..User::plain("u-active-timed")
            },
            Check,
        ),
    ];
    for (user, expected) in consistency {
        assert_eq!(
            insert_user(&mut database, user).await?,
            expected,
            "{}",
            user.id
        );
    }

    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_active_admin_index_covering() -> Result<()> {
    let schema = migrated_schema("it_schema_active_admin_index_covering").await?;
    let users = schema.table("users").context("users table")?;
    let index = users
        .indexes
        .iter()
        .find(|index| index.name == "ix_users_active_admins")
        .context("ix_users_active_admins")?;
    assert!(index.partial && !index.unique);
    assert_eq!(
        index.columns,
        [IndexColumn {
            name: Some("id".to_owned()),
            collation: "BINARY".to_owned(),
        }]
    );
    let definition = schema
        .objects
        .iter()
        .find(|object| object.kind == "index" && object.name == "ix_users_active_admins")
        .context("ix_users_active_admins definition")?;
    assert_eq!(
        compact(&definition.sql),
        format!("CREATE INDEX ix_users_active_admins ON users(id) WHERE {ACTIVE_ADMINS}")
    );

    let mut database = MigratedDatabase::open("it_schema_active_admin_index_covering").await?;
    let rows = [
        User {
            role: "admin",
            ..User::plain("admin-a")
        },
        User {
            role: "admin",
            ..User::plain("admin-b")
        },
        User {
            role: "admin",
            is_active: 0,
            deactivated_at: Some(NOW),
            ..User::plain("admin-inactive")
        },
        User::plain("user-active"),
        User {
            is_active: 0,
            deactivated_at: Some(NOW),
            ..User::plain("user-inactive")
        },
    ];
    for user in rows {
        assert_eq!(
            insert_user(&mut database, user).await?,
            Accepted,
            "{}",
            user.id
        );
    }

    let indexed = format!(
        "SELECT id FROM users INDEXED BY ix_users_active_admins WHERE {ACTIVE_ADMINS} ORDER BY id"
    );
    assert_eq!(database.ids(&indexed).await?, ["admin-a", "admin-b"]);

    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!(
        "EXPLAIN QUERY PLAN SELECT count(*) FROM users WHERE {ACTIVE_ADMINS}"
    ))
    .fetch_all(&mut database.connection)
    .await?;
    let plan: Vec<String> = plan.into_iter().map(|row| row.3).collect();
    assert_eq!(
        plan,
        ["SCAN users USING COVERING INDEX ix_users_active_admins"]
    );

    let unimplied = sqlx::query_scalar::<_, String>(
        "SELECT id FROM users INDEXED BY ix_users_active_admins WHERE role = 'admin'",
    )
    .fetch_all(&mut database.connection)
    .await;
    assert!(unimplied.is_err());

    sqlx::query("UPDATE users SET is_active = 0, deactivated_at = ?1 WHERE id = 'admin-a'")
        .bind(LATER)
        .execute(&mut database.connection)
        .await?;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = 'user-active'")
        .execute(&mut database.connection)
        .await?;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = 'user-inactive'")
        .execute(&mut database.connection)
        .await?;
    assert_eq!(database.ids(&indexed).await?, ["admin-b", "user-active"]);

    database.close().await
}

struct HashColumn {
    table: &'static str,
    column: &'static str,
    unique: bool,
    enforced: bool,
    insert: &'static str,
}

const HASH_COLUMNS: [HashColumn; 8] = [
    HashColumn {
        table: "sessions",
        column: "token_hash",
        unique: true,
        enforced: false,
        insert: "INSERT INTO sessions
                     (id, user_id, token_hash, csrf_token_hash, auth_method, created_at,
                      last_seen_at, last_auth_at, idle_expires_at, absolute_expires_at)
                 VALUES (?1, 'owner', ?2, ?3, 'password', ?4, ?4, ?4, ?4, ?4)",
    },
    HashColumn {
        table: "sessions",
        column: "csrf_token_hash",
        unique: false,
        enforced: false,
        insert: "INSERT INTO sessions
                     (id, user_id, csrf_token_hash, token_hash, auth_method, created_at,
                      last_seen_at, last_auth_at, idle_expires_at, absolute_expires_at)
                 VALUES (?1, 'owner', ?2, ?3, 'password', ?4, ?4, ?4, ?4, ?4)",
    },
    HashColumn {
        table: "sessions",
        column: "mfa_token_hash",
        unique: true,
        enforced: false,
        insert: "INSERT INTO sessions
                     (id, user_id, mfa_token_hash, token_hash, csrf_token_hash, state,
                      mfa_expires_at, auth_method, created_at, last_seen_at, last_auth_at,
                      idle_expires_at, absolute_expires_at)
                 VALUES (?1, 'owner', ?2, ?3, ?3, 'mfa_pending', ?4, 'password',
                         ?4, ?4, ?4, ?4, ?4)",
    },
    HashColumn {
        table: "trusted_devices",
        column: "token_hash",
        unique: true,
        enforced: true,
        insert: "INSERT INTO trusted_devices (id, user_id, token_hash, created_at, expires_at)
                 VALUES (?1, 'owner', ?2, ?4, ?4)",
    },
    HashColumn {
        table: "totp_backup_codes",
        column: "code_hash",
        unique: true,
        enforced: true,
        insert: "INSERT INTO totp_backup_codes (id, user_id, batch_id, code_hash, created_at)
                 VALUES (?1, 'owner', 'batch', ?2, ?4)",
    },
    HashColumn {
        table: "password_reset_tokens",
        column: "token_hash",
        unique: true,
        enforced: true,
        insert: "INSERT INTO password_reset_tokens
                     (id, user_id, token_hash, created_at, expires_at)
                 VALUES (?1, 'owner', ?2, ?4, ?4)",
    },
    HashColumn {
        table: "email_verifications",
        column: "token_hash",
        unique: true,
        enforced: true,
        insert: "INSERT INTO email_verifications
                     (id, user_id, purpose, email, email_normalized, token_hash,
                      created_at, expires_at, invalidated_at)
                 VALUES (?1, 'owner', 'email_change', 'a@example.com', 'a@example.com', ?2,
                         ?4, ?4, ?4)",
    },
    HashColumn {
        table: "invites",
        column: "token_hash",
        unique: true,
        enforced: true,
        insert: "INSERT INTO invites (id, token_hash, created_by, created_at, expires_at)
                 VALUES (?1, ?2, 'owner', ?4, ?4)",
    },
];

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_token_hash_shape() -> Result<()> {
    let schema = migrated_schema("it_schema_token_hash_shape").await?;
    for hash in &HASH_COLUMNS {
        let table = schema.table(hash.table).context("hash table")?;
        let column = table.column(hash.column).context("hash column")?;
        assert_eq!(
            column.declared_type, "TEXT",
            "{}.{}",
            hash.table, hash.column
        );
        assert!(
            compact(&table.sql).contains(&format!("length({}) = 64", hash.column)),
            "{}.{}",
            hash.table,
            hash.column
        );
    }

    let totp = schema.table("totp_secrets").context("totp_secrets table")?;
    assert_eq!(
        totp.columns
            .iter()
            .map(|column| (column.name.as_str(), column.declared_type.as_str()))
            .filter(|(name, _)| name.contains("secret") || name.contains("nonce"))
            .collect::<Vec<_>>(),
        [("secret_ciphertext", "BLOB"), ("secret_nonce", "BLOB")]
    );
    let backup = schema
        .table("totp_backup_codes")
        .context("totp_backup_codes table")?;
    assert_eq!(
        backup
            .columns
            .iter()
            .filter(|column| column.name.contains("code"))
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["code_hash"]
    );
    let devices = schema
        .table("trusted_devices")
        .context("trusted_devices table")?;
    let device_unique: Vec<Vec<Option<String>>> = devices
        .indexes
        .iter()
        .filter(|index| index.unique && !index.name.starts_with("sqlite_"))
        .map(|index| {
            index
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect()
        })
        .collect();
    assert_eq!(device_unique, [vec![Some("token_hash".to_owned())]]);

    let mut database = MigratedDatabase::open("it_schema_token_hash_shape").await?;
    assert_eq!(
        insert_user(&mut database, User::plain("owner")).await?,
        Accepted
    );

    let mut seed = 1_u64;
    for hash in &HASH_COLUMNS {
        let label = format!("{}.{}", hash.table, hash.column);
        let valid = digest(seed);
        let mut shapes = vec![
            (valid.clone(), Accepted),
            (valid[..63].to_owned(), Check),
            (format!("{valid}0"), Check),
            (String::new(), Check),
        ];
        if hash.unique {
            shapes.push((valid.clone(), Unique));
        }
        for (case, (value, expected)) in shapes.into_iter().enumerate() {
            seed += 1;
            let id = format!("{}-{}-{case}", hash.table, hash.column);
            let other = digest(seed);
            let arguments = arguments![id, value.as_str(), other.as_str(), NOW];
            let outcome = if hash.enforced {
                database.run(hash.insert, arguments).await?
            } else {
                database.run_unenforced(hash.insert, arguments).await?
            };
            assert_eq!(outcome, expected, "{label} length {}", value.len());
        }
    }

    let totp_insert = "INSERT INTO totp_secrets
                           (user_id, secret_ciphertext, secret_nonce, state, confirmed_at,
                            created_at, updated_at)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)";
    for id in ["totp-a", "totp-b", "totp-c", "totp-d"] {
        assert_eq!(insert_user(&mut database, User::plain(id)).await?, Accepted);
    }
    let totp_cases = [
        ("totp-a", vec![9; 24], "pending", None, Accepted),
        ("totp-b", vec![9; 12], "pending", None, Check),
        ("totp-b", vec![9; 24], "active", None, Check),
        ("totp-c", vec![9; 24], "pending", Some(NOW), Check),
        ("totp-d", vec![9; 24], "active", Some(NOW), Accepted),
    ];
    for (user, nonce, state, confirmed_at, expected) in totp_cases {
        assert_eq!(
            database
                .run(
                    totp_insert,
                    arguments![user, vec![7_u8; 48], nonce, state, confirmed_at, NOW],
                )
                .await?,
            expected,
            "{user} {state}"
        );
    }
    let stored: (String, String) = sqlx::query_as(
        "SELECT typeof(secret_ciphertext), typeof(secret_nonce) FROM totp_secrets WHERE user_id = 'totp-a'",
    )
    .fetch_one(&mut database.connection)
    .await?;
    assert_eq!(stored, ("blob".to_owned(), "blob".to_owned()));

    database.close().await
}

struct Invite<'a> {
    id: &'a str,
    token_hash: String,
    email: Option<&'a str>,
    email_normalized: Option<&'a str>,
    token_ciphertext: Option<Vec<u8>>,
    token_nonce: Option<Vec<u8>>,
    key_version: Option<i64>,
}

impl<'a> Invite<'a> {
    fn sealed(id: &'a str, seed: u64) -> Self {
        Self {
            id,
            token_hash: digest(seed),
            email: None,
            email_normalized: None,
            token_ciphertext: Some(vec![7; 48]),
            token_nonce: Some(vec![9; 24]),
            key_version: Some(1),
        }
    }
}

async fn insert_invite(database: &mut MigratedDatabase, invite: Invite<'_>) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO invites
                 (id, token_hash, email, email_normalized, created_by, created_at, expires_at,
                  token_ciphertext, token_nonce, key_version)
             VALUES (?1, ?2, ?3, ?4, 'inviter', ?5, ?6, ?7, ?8, ?9)",
            arguments![
                invite.id,
                invite.token_hash,
                invite.email,
                invite.email_normalized,
                NOW,
                LATER,
                invite.token_ciphertext,
                invite.token_nonce,
                invite.key_version,
            ],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_invite_sealed_token_pending_only() -> Result<()> {
    let schema = migrated_schema("it_schema_invite_sealed_token_pending_only").await?;
    let invites = schema.table("invites").context("invites table")?;
    for column in ["token_ciphertext", "token_nonce"] {
        assert_eq!(
            invites
                .column(column)
                .context("sealed column")?
                .declared_type,
            "BLOB"
        );
    }

    let mut database = MigratedDatabase::open("it_schema_invite_sealed_token_pending_only").await?;
    for user in ["inviter", "invitee"] {
        assert_eq!(
            insert_user(&mut database, User::plain(user)).await?,
            Accepted
        );
    }

    let inserts = [
        (Invite::sealed("sealed", 1), Accepted),
        (
            Invite {
                token_ciphertext: None,
                token_nonce: None,
                key_version: None,
                ..Invite::sealed("plain", 2)
            },
            Accepted,
        ),
        (
            Invite {
                token_nonce: None,
                ..Invite::sealed("partial-nonce", 3)
            },
            Check,
        ),
        (
            Invite {
                key_version: None,
                ..Invite::sealed("partial-version", 4)
            },
            Check,
        ),
        (
            Invite {
                token_nonce: Some(vec![9; 12]),
                ..Invite::sealed("short-nonce", 5)
            },
            Check,
        ),
        (
            Invite {
                key_version: Some(0),
                ..Invite::sealed("zero-version", 6)
            },
            Check,
        ),
        (
            Invite {
                email: Some("Guest@Example.com"),
                email_normalized: Some("guest@example.com"),
                ..Invite::sealed("bound", 7)
            },
            Accepted,
        ),
        (
            Invite {
                email: Some("GUEST@example.com"),
                email_normalized: Some("guest@example.com"),
                ..Invite::sealed("bound-twice", 8)
            },
            Unique,
        ),
        (
            Invite {
                email: Some("half@example.com"),
                ..Invite::sealed("bound-half", 9)
            },
            Check,
        ),
    ];
    for (invite, expected) in inserts {
        let id = invite.id;
        assert_eq!(
            insert_invite(&mut database, invite).await?,
            expected,
            "{id}"
        );
    }
    let terminal_insert = database
        .run(
            "INSERT INTO invites
                 (id, token_hash, state, created_by, created_at, expires_at,
                  token_ciphertext, token_nonce, key_version)
             VALUES ('born-expired', ?1, 'expired', 'inviter', ?2, ?2, ?3, ?4, 1)",
            arguments![digest(10), NOW, vec![7_u8; 48], vec![9_u8; 24]],
        )
        .await?;
    assert_eq!(terminal_insert, Check);

    let transitions = [
        (
            "UPDATE invites SET state = 'accepted', accepted_at = ?1, accepted_user_id = 'invitee'
             WHERE id = 'sealed'",
            Check,
        ),
        (
            "UPDATE invites SET state = 'revoked', revoked_at = ?1, revoked_by = 'inviter'
             WHERE id = 'sealed'",
            Check,
        ),
        (
            "UPDATE invites SET state = 'expired' WHERE id = 'sealed'",
            Check,
        ),
        (
            "UPDATE invites SET state = 'accepted', accepted_at = ?1, accepted_user_id = 'invitee',
                                token_ciphertext = NULL
             WHERE id = 'sealed'",
            Check,
        ),
        (
            "UPDATE invites SET state = 'accepted', accepted_at = ?1
             WHERE id = 'plain'",
            Check,
        ),
        (
            "UPDATE invites SET state = 'accepted', accepted_at = ?1, accepted_user_id = 'invitee',
                                token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
             WHERE id = 'sealed'",
            Accepted,
        ),
        (
            "UPDATE invites SET state = 'expired',
                                token_ciphertext = NULL, token_nonce = NULL, key_version = NULL
             WHERE id = 'bound'",
            Accepted,
        ),
    ];
    for (sql, expected) in transitions {
        assert_eq!(
            database.run(sql, arguments![LATER]).await?,
            expected,
            "{sql}"
        );
    }

    let states: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT id, state,
                token_ciphertext IS NULL AND token_nonce IS NULL AND key_version IS NULL
         FROM invites ORDER BY id",
    )
    .fetch_all(&mut database.connection)
    .await?;
    assert_eq!(
        states,
        [
            ("bound".to_owned(), "expired".to_owned(), true),
            ("plain".to_owned(), "pending".to_owned(), true),
            ("sealed".to_owned(), "accepted".to_owned(), true),
        ]
    );

    let rebound = Invite {
        email: Some("guest@example.com"),
        email_normalized: Some("guest@example.com"),
        ..Invite::sealed("bound-again", 11)
    };
    assert_eq!(insert_invite(&mut database, rebound).await?, Accepted);

    database.close().await
}

struct Provider<'a> {
    id: &'a str,
    key: &'a str,
    client_secret_ciphertext: Option<Vec<u8>>,
    client_secret_nonce: Option<Vec<u8>>,
    token_auth_method: &'a str,
}

impl<'a> Provider<'a> {
    fn sealed(id: &'a str) -> Self {
        Self {
            id,
            key: id,
            client_secret_ciphertext: Some(vec![7; 48]),
            client_secret_nonce: Some(vec![9; 24]),
            token_auth_method: "client_secret_post",
        }
    }
}

async fn insert_provider(
    database: &mut MigratedDatabase,
    provider: Provider<'_>,
) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO identity_providers
                 (id, key, display_name, kind, client_id, client_secret_ciphertext,
                  client_secret_nonce, token_auth_method, created_at, updated_at)
             VALUES (?1, ?2, ?1, 'oidc', 'client', ?3, ?4, ?5, ?6, ?6)",
            arguments![
                provider.id,
                provider.key,
                provider.client_secret_ciphertext,
                provider.client_secret_nonce,
                provider.token_auth_method,
                NOW,
            ],
        )
        .await
}

async fn insert_link(
    database: &mut MigratedDatabase,
    id: &str,
    user_id: &str,
    provider_id: &str,
    subject: &str,
) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO identity_links (id, user_id, provider_id, subject, link_method, created_at)
             VALUES (?1, ?2, ?3, ?4, 'manual', ?5)",
            arguments![id, user_id, provider_id, subject, NOW],
        )
        .await
}

fn index_shape(table: &Table, name: &str) -> Option<(bool, bool, Vec<Option<String>>)> {
    table
        .indexes
        .iter()
        .find(|index| index.name == name)
        .map(|index| {
            (
                index.unique,
                index.partial,
                index
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect(),
            )
        })
}

fn named(columns: &[&str]) -> Vec<Option<String>> {
    columns
        .iter()
        .map(|column| Some((*column).to_owned()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_identity_link_uniqueness() -> Result<()> {
    let schema = migrated_schema("it_schema_identity_link_uniqueness").await?;
    let links = schema
        .table("identity_links")
        .context("identity_links table")?;
    assert_eq!(
        index_shape(links, "ux_identity_links_provider_subject"),
        Some((true, false, named(&["provider_id", "subject"])))
    );
    assert_eq!(
        index_shape(links, "ux_identity_links_user_provider"),
        Some((true, false, named(&["user_id", "provider_id"])))
    );
    let unique_columns: Vec<Vec<Option<String>>> = links
        .indexes
        .iter()
        .filter(|index| index.unique && !index.name.starts_with("sqlite_"))
        .map(|index| {
            index
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect()
        })
        .collect();
    assert_eq!(unique_columns.len(), 2);
    assert!(unique_columns
        .iter()
        .all(|columns| !columns.contains(&Some("email_at_link".to_owned()))));

    let mut database = MigratedDatabase::open("it_schema_identity_link_uniqueness").await?;
    for user in ["alice", "bob", "carol"] {
        assert_eq!(
            insert_user(&mut database, User::plain(user)).await?,
            Accepted
        );
    }
    for provider in ["corp", "social"] {
        assert_eq!(
            insert_provider(&mut database, Provider::sealed(provider)).await?,
            Accepted
        );
    }

    let cases = [
        ("alice-corp", "alice", "corp", "subject-1", Accepted),
        ("bob-corp-taken", "bob", "corp", "subject-1", Unique),
        ("alice-corp-second", "alice", "corp", "subject-2", Unique),
        ("alice-social", "alice", "social", "subject-1", Accepted),
        ("bob-corp", "bob", "corp", "subject-2", Accepted),
        ("carol-corp", "carol", "corp", "subject-3", Accepted),
        ("carol-social-taken", "carol", "social", "subject-1", Unique),
        ("carol-empty", "carol", "social", "", Check),
    ];
    for (id, user, provider, subject, expected) in cases {
        assert_eq!(
            insert_link(&mut database, id, user, provider, subject).await?,
            expected,
            "{id}"
        );
    }

    let suspension = [
        (
            "UPDATE identity_links SET state = 'suspended' WHERE id = 'bob-corp'",
            Check,
        ),
        (
            "UPDATE identity_links SET suspended_at = ?1 WHERE id = 'bob-corp'",
            Check,
        ),
        (
            "UPDATE identity_links SET state = 'suspended', suspended_at = ?1 WHERE id = 'bob-corp'",
            Accepted,
        ),
    ];
    for (sql, expected) in suspension {
        assert_eq!(
            database.run(sql, arguments![LATER]).await?,
            expected,
            "{sql}"
        );
    }

    let stored: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT user_id, provider_id, subject FROM identity_links ORDER BY user_id, provider_id",
    )
    .fetch_all(&mut database.connection)
    .await?;
    let expected = [
        ("alice", "corp", "subject-1"),
        ("alice", "social", "subject-1"),
        ("bob", "corp", "subject-2"),
        ("carol", "corp", "subject-3"),
    ]
    .map(|(user, provider, subject)| (user.to_owned(), provider.to_owned(), subject.to_owned()));
    assert_eq!(stored, expected);

    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_provider_delete_restricted() -> Result<()> {
    let schema = migrated_schema("it_schema_provider_delete_restricted").await?;
    let delete_actions = |table: &str| -> Result<Vec<(String, String, String)>> {
        let mut actions: Vec<(String, String, String)> = schema
            .table(table)
            .context("identity table")?
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
        Ok(actions)
    };
    let action = |column: &str, parent: &str, on_delete: &str| {
        (column.to_owned(), parent.to_owned(), on_delete.to_owned())
    };
    assert_eq!(
        delete_actions("identity_links")?,
        [
            action("provider_id", "identity_providers", "RESTRICT"),
            action("user_id", "users", "CASCADE"),
        ]
    );
    assert_eq!(
        delete_actions("oauth_auth_requests")?,
        [
            action("link_user_id", "users", "CASCADE"),
            action("provider_id", "identity_providers", "CASCADE"),
        ]
    );
    assert_eq!(
        delete_actions("identity_providers")?,
        [action("updated_by", "users", "SET NULL")]
    );

    let mut database = MigratedDatabase::open("it_schema_provider_delete_restricted").await?;
    assert_eq!(
        insert_user(&mut database, User::plain("owner")).await?,
        Accepted
    );
    for provider in ["unused", "linked"] {
        assert_eq!(
            insert_provider(&mut database, Provider::sealed(provider)).await?,
            Accepted
        );
    }
    assert_eq!(
        insert_link(&mut database, "owner-linked", "owner", "linked", "subject").await?,
        Accepted
    );
    assert_eq!(
        insert_request(&mut database, Request::login("pending", 1, "linked")).await?,
        Accepted
    );

    let delete_provider = "DELETE FROM identity_providers WHERE id = ?1";
    let steps = [
        (delete_provider, "unused", Accepted),
        (delete_provider, "linked", Foreign),
        (
            "DELETE FROM identity_links WHERE id = ?1",
            "owner-linked",
            Accepted,
        ),
        (delete_provider, "linked", Accepted),
    ];
    for (sql, id, expected) in steps {
        assert_eq!(
            database.run(sql, arguments![id]).await?,
            expected,
            "{sql} {id}"
        );
    }

    let remaining: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM identity_providers),
                (SELECT count(*) FROM identity_links),
                (SELECT count(*) FROM oauth_auth_requests)",
    )
    .fetch_one(&mut database.connection)
    .await?;
    assert_eq!(remaining, (0, 0, 0));
    let dangling: Vec<(String,)> = sqlx::query_as("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&mut database.connection)
        .await?;
    assert!(dangling.is_empty());

    database.close().await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_provider_secret_sealed() -> Result<()> {
    let schema = migrated_schema("it_schema_provider_secret_sealed").await?;
    let providers = schema
        .table("identity_providers")
        .context("identity_providers table")?;
    assert_eq!(
        providers
            .columns
            .iter()
            .filter(|column| column.name.contains("secret"))
            .map(|column| (column.name.as_str(), column.declared_type.as_str()))
            .collect::<Vec<_>>(),
        [
            ("client_secret_ciphertext", "BLOB"),
            ("client_secret_nonce", "BLOB")
        ]
    );
    assert_eq!(
        index_shape(providers, "ux_identity_providers_key"),
        Some((true, false, named(&["key"])))
    );
    assert_eq!(
        index_shape(providers, "ix_identity_providers_enabled"),
        Some((false, true, named(&["sort_order", "key"])))
    );

    let mut database = MigratedDatabase::open("it_schema_provider_secret_sealed").await?;
    let cases = [
        (Provider::sealed("sealed"), Accepted),
        (
            Provider {
                client_secret_ciphertext: None,
                client_secret_nonce: None,
                token_auth_method: "none",
                ..Provider::sealed("public")
            },
            Accepted,
        ),
        (
            Provider {
                client_secret_nonce: None,
                ..Provider::sealed("missing-nonce")
            },
            Check,
        ),
        (
            Provider {
                client_secret_ciphertext: None,
                ..Provider::sealed("missing-ciphertext")
            },
            Check,
        ),
        (
            Provider {
                client_secret_nonce: Some(vec![9; 12]),
                ..Provider::sealed("short-nonce")
            },
            Check,
        ),
        (
            Provider {
                token_auth_method: "private_key_jwt",
                ..Provider::sealed("unknown-method")
            },
            Check,
        ),
        (
            Provider {
                id: "uppercase",
                ..Provider::sealed("Corp")
            },
            Check,
        ),
        (
            Provider {
                id: "duplicate",
                ..Provider::sealed("sealed")
            },
            Unique,
        ),
    ];
    for (provider, expected) in cases {
        let id = provider.id;
        assert_eq!(
            insert_provider(&mut database, provider).await?,
            expected,
            "{id}"
        );
    }

    let stored: (String, String, i64, i64) = sqlx::query_as(
        "SELECT typeof(client_secret_ciphertext), typeof(client_secret_nonce),
                auto_provision, is_enabled
         FROM identity_providers WHERE id = 'sealed'",
    )
    .fetch_one(&mut database.connection)
    .await?;
    assert_eq!(stored, ("blob".to_owned(), "blob".to_owned(), 0, 0));

    database.close().await
}

struct Request<'a> {
    id: &'a str,
    provider_id: &'a str,
    state_hash: String,
    binding_cookie_hash: String,
    pkce_verifier_nonce: Vec<u8>,
    post_auth_path: Option<String>,
    purpose: &'a str,
    link_user_id: Option<&'a str>,
}

impl<'a> Request<'a> {
    fn login(id: &'a str, seed: u64, provider_id: &'a str) -> Self {
        Self {
            id,
            provider_id,
            state_hash: digest(seed),
            binding_cookie_hash: digest(seed + 1_000),
            pkce_verifier_nonce: vec![9; 24],
            post_auth_path: None,
            purpose: "login",
            link_user_id: None,
        }
    }
}

async fn insert_request(database: &mut MigratedDatabase, request: Request<'_>) -> Result<Outcome> {
    database
        .run(
            "INSERT INTO oauth_auth_requests
                 (id, provider_id, state_hash, binding_cookie_hash, pkce_verifier_ciphertext,
                  pkce_verifier_nonce, nonce, redirect_uri, post_auth_path, purpose,
                  link_user_id, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'nonce-0123456789abcdef',
                     'https://palmr.example/api/v1/auth/providers/corp/callback',
                     ?7, ?8, ?9, ?10, ?11)",
            arguments![
                request.id,
                request.provider_id,
                request.state_hash,
                request.binding_cookie_hash,
                vec![7_u8; 48],
                request.pkce_verifier_nonce,
                request.post_auth_path,
                request.purpose,
                request.link_user_id,
                NOW,
                LATER,
            ],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_oauth_request_checks() -> Result<()> {
    let schema = migrated_schema("it_schema_oauth_request_checks").await?;
    let requests = schema
        .table("oauth_auth_requests")
        .context("oauth_auth_requests table")?;
    for (column, declared_type) in [
        ("state_hash", "TEXT"),
        ("binding_cookie_hash", "TEXT"),
        ("pkce_verifier_ciphertext", "BLOB"),
        ("pkce_verifier_nonce", "BLOB"),
        ("nonce", "TEXT"),
    ] {
        assert_eq!(
            requests
                .column(column)
                .context("request column")?
                .declared_type,
            declared_type,
            "{column}"
        );
    }
    assert!(requests.column("state").is_none());
    assert!(requests.column("pkce_verifier").is_none());
    assert_eq!(
        index_shape(requests, "ux_oauth_auth_requests_state"),
        Some((true, false, named(&["state_hash"])))
    );
    assert_eq!(
        index_shape(requests, "ix_oauth_auth_requests_expiry"),
        Some((false, true, named(&["expires_at"])))
    );

    let mut database = MigratedDatabase::open("it_schema_oauth_request_checks").await?;
    assert_eq!(
        insert_user(&mut database, User::plain("owner")).await?,
        Accepted
    );
    assert_eq!(
        insert_provider(&mut database, Provider::sealed("corp")).await?,
        Accepted
    );

    let valid = digest(1);
    let path = |value: &str| Some(value.to_owned());
    let cases = [
        (Request::login("login", 1, "corp"), Accepted),
        (Request::login("state-reused", 1, "corp"), Unique),
        (
            Request {
                state_hash: valid[..63].to_owned(),
                ..Request::login("state-short", 2, "corp")
            },
            Check,
        ),
        (
            Request {
                state_hash: format!("{valid}0"),
                ..Request::login("state-long", 3, "corp")
            },
            Check,
        ),
        (
            Request {
                binding_cookie_hash: valid[..63].to_owned(),
                ..Request::login("binding-short", 4, "corp")
            },
            Check,
        ),
        (
            Request {
                binding_cookie_hash: format!("{valid}0"),
                ..Request::login("binding-long", 5, "corp")
            },
            Check,
        ),
        (
            Request {
                pkce_verifier_nonce: vec![9; 12],
                ..Request::login("pkce-nonce-short", 6, "corp")
            },
            Check,
        ),
        (
            Request {
                pkce_verifier_nonce: vec![9; 25],
                ..Request::login("pkce-nonce-long", 7, "corp")
            },
            Check,
        ),
        (
            Request {
                post_auth_path: path("/files/reports?view=grid"),
                ..Request::login("path-relative", 8, "corp")
            },
            Accepted,
        ),
        (
            Request {
                post_auth_path: path("files"),
                ..Request::login("path-bare", 9, "corp")
            },
            Check,
        ),
        (
            Request {
                post_auth_path: path("//evil.example/"),
                ..Request::login("path-protocol-relative", 10, "corp")
            },
            Check,
        ),
        (
            Request {
                post_auth_path: path("https://evil.example/"),
                ..Request::login("path-absolute", 11, "corp")
            },
            Check,
        ),
        (
            Request {
                post_auth_path: path("/files/../admin"),
                ..Request::login("path-traversal", 12, "corp")
            },
            Check,
        ),
        (
            Request {
                post_auth_path: Some(format!("/{}", "a".repeat(256))),
                ..Request::login("path-long", 13, "corp")
            },
            Check,
        ),
        (
            Request {
                link_user_id: Some("owner"),
                ..Request::login("login-with-user", 14, "corp")
            },
            Check,
        ),
        (
            Request {
                purpose: "link",
                ..Request::login("link-without-user", 15, "corp")
            },
            Check,
        ),
        (
            Request {
                purpose: "recent_auth",
                ..Request::login("reauth-without-user", 16, "corp")
            },
            Check,
        ),
        (
            Request {
                purpose: "link",
                link_user_id: Some("owner"),
                ..Request::login("link", 17, "corp")
            },
            Accepted,
        ),
        (
            Request {
                purpose: "recent_auth",
                link_user_id: Some("owner"),
                ..Request::login("reauth", 18, "corp")
            },
            Accepted,
        ),
        (
            Request {
                purpose: "signup",
                ..Request::login("purpose-unknown", 19, "corp")
            },
            Check,
        ),
    ];
    for (request, expected) in cases {
        let id = request.id;
        assert_eq!(
            insert_request(&mut database, request).await?,
            expected,
            "{id}"
        );
    }

    let stored: Vec<(String, String)> = sqlx::query_as(
        "SELECT typeof(pkce_verifier_ciphertext), typeof(pkce_verifier_nonce)
         FROM oauth_auth_requests",
    )
    .fetch_all(&mut database.connection)
    .await?;
    assert_eq!(stored.len(), 4);
    assert!(stored
        .iter()
        .all(|types| *types == ("blob".to_owned(), "blob".to_owned())));

    sqlx::query("DELETE FROM users WHERE id = 'owner'")
        .execute(&mut database.connection)
        .await?;
    assert_eq!(
        database
            .ids("SELECT id FROM oauth_auth_requests ORDER BY id")
            .await?,
        ["login", "path-relative"]
    );

    database.close().await
}

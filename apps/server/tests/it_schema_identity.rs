pub mod support;

use anyhow::{Context, Result};
use sqlx::error::ErrorKind;
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions};
use sqlx::{Connection, SqliteConnection};
use support::schema::{migrated_schema, IndexColumn};
use support::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const NOW: &str = "2026-01-01T00:00:00.000Z";
const LATER: &str = "2026-01-01T00:05:00.000Z";
const ACTIVE_ADMINS: &str = "role = 'admin' AND is_active = 1";

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

use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;

use super::{DbPools, MigrationStatus, DATABASE_FILE, MIGRATOR};
use crate::config::SqliteSynchronous;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedMigration {
    pub version: i64,
    pub description: String,
    pub success: bool,
    pub checksum: Vec<u8>,
}

pub(crate) fn embedded_files() -> Vec<(String, String)> {
    MIGRATOR
        .iter()
        .map(|migration| {
            (
                format!(
                    "{:04}_{}.sql",
                    migration.version,
                    migration.description.replace(' ', "_")
                ),
                migration.sql.to_string(),
            )
        })
        .collect()
}

pub(crate) async fn fixture_migrator(files: &[(String, String)]) -> (TempDir, Migrator) {
    let directory = TempDir::new().unwrap();
    for (name, sql) in files {
        std::fs::write(directory.path().join(name), sql).unwrap();
    }
    let migrator = Migrator::new(directory.path().to_path_buf()).await.unwrap();
    (directory, migrator)
}

pub(crate) async fn open_pools(data_root: &Path) -> DbPools {
    DbPools::open(data_root, 1, SqliteSynchronous::Full)
        .await
        .unwrap()
}

pub(crate) async fn raw_connection(data_root: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(data_root.join(DATABASE_FILE))
            .create_if_missing(true),
    )
    .await
    .unwrap()
}

pub(crate) async fn recorded_migrations(
    connection: &mut SqliteConnection,
) -> Vec<RecordedMigration> {
    let rows: Vec<(i64, String, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT version, description, success, checksum FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.into_iter()
        .map(
            |(version, description, success, checksum)| RecordedMigration {
                version,
                description,
                success,
                checksum,
            },
        )
        .collect()
}

pub(crate) async fn schema_objects(connection: &mut SqliteConnection) -> Vec<(String, String)> {
    sqlx::query_as("SELECT type, name FROM sqlite_schema ORDER BY type, name")
        .fetch_all(&mut *connection)
        .await
        .unwrap()
}

fn embedded_records() -> Vec<RecordedMigration> {
    MIGRATOR
        .iter()
        .map(|migration| RecordedMigration {
            version: migration.version,
            description: migration.description.to_string(),
            success: true,
            checksum: migration.checksum.to_vec(),
        })
        .collect()
}

#[tokio::test]
async fn it_migrate_up_from_empty() {
    let files = embedded_files();
    assert_eq!(
        files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [
            "0001_initial_schema.sql",
            "0002_add_identity_provider_email_linking.sql",
            "0003_authorization_request_contract.sql",
            "0004_session_revoked_reason_identity_unlink.sql",
            "0005_password_login_enabled.sql"
        ]
    );
    assert_eq!(
        files[0].1,
        include_str!("../../../migrations/0001_initial_schema.sql")
    );
    assert_eq!(
        files[1].1,
        include_str!("../../../migrations/0002_add_identity_provider_email_linking.sql")
    );
    assert_eq!(
        files[2].1,
        include_str!("../../../migrations/0003_authorization_request_contract.sql")
    );
    assert_eq!(
        files[3].1,
        include_str!("../../../migrations/0004_session_revoked_reason_identity_unlink.sql")
    );
    assert_eq!(
        files[4].1,
        include_str!("../../../migrations/0005_password_login_enabled.sql")
    );

    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(pools.path(), data.path().join(DATABASE_FILE));

    let first = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        first,
        MigrationStatus {
            applied: 5,
            version: Some(5),
        }
    );
    let again = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        again,
        MigrationStatus {
            applied: 0,
            version: Some(5),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let reopened = open_pools(data.path()).await;
    assert_eq!(
        reopened.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 0,
            version: Some(5),
        }
    );
    reopened.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    assert_eq!(
        recorded_migrations(&mut connection).await,
        embedded_records()
    );
    assert_eq!(embedded_records()[0].description, "initial schema");
}

const PROVIDER_ROWS: [(&str, &str); 3] = [
    ("0192f3a1-0000-7000-8000-000000000001", "oidc"),
    ("0192f3a1-0000-7000-8000-000000000002", "oauth2"),
    ("0192f3a1-0000-7000-8000-000000000003", "oidc"),
];

#[tokio::test]
async fn it_migrate_from_0001_derives_email_linking_by_protocol() {
    let files = embedded_files();
    let (_directory, frozen) = fixture_migrator(&files[..1]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&frozen).await.unwrap(),
        MigrationStatus {
            applied: 1,
            version: Some(1),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    for (id, kind) in PROVIDER_ROWS {
        let endpoints = if kind == "oauth2" {
            "'https://idp.example.test/authorize', 'https://idp.example.test/token'"
        } else {
            "NULL, NULL"
        };
        sqlx::query(&format!(
            "INSERT INTO identity_providers
                 (id, key, display_name, kind, authorization_endpoint, token_endpoint,
                  client_id, created_at, updated_at)
             VALUES (?1, ?1, ?1, ?2, {endpoints}, 'client', '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:00:00.000Z')"
        ))
        .bind(id)
        .bind(kind)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 4,
            version: Some(5),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let migrated: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT id, kind, allow_email_linking FROM identity_providers ORDER BY id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        migrated,
        PROVIDER_ROWS
            .iter()
            .map(|(id, kind)| (
                (*id).to_owned(),
                (*kind).to_owned(),
                i64::from(*kind == "oidc")
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        recorded_migrations(&mut connection)
            .await
            .iter()
            .map(|record| (record.version, record.success))
            .collect::<Vec<_>>(),
        [(1, true), (2, true), (3, true), (4, true), (5, true)]
    );
    connection.close().await.unwrap();
}

#[tokio::test]
async fn it_migrate_oauth_requests_to_reauth_and_extended_path() {
    let files = embedded_files();
    let (_directory, frozen) = fixture_migrator(&files[..2]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    pools.migrate(&frozen).await.unwrap();
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    sqlx::query(
        "INSERT INTO identity_providers
             (id, key, display_name, kind, client_id, created_at, updated_at)
         VALUES ('corp', 'corp', 'Corp', 'oidc', 'client', '2026-09-25T12:00:00.000Z',
                 '2026-09-25T12:00:00.000Z')",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users
             (id, email, email_normalized, username, username_normalized, created_at, updated_at)
         VALUES ('owner', 'owner@example.test', 'owner@example.test', 'owner', 'owner',
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let rows = [
        ("login", None, "login"),
        ("link", Some("owner"), "link"),
        ("reauth", Some("owner"), "recent_auth"),
    ];
    for (id, user, purpose) in rows {
        sqlx::query(
            "INSERT INTO oauth_auth_requests
                 (id, provider_id, state_hash, binding_cookie_hash, pkce_verifier_ciphertext,
                  pkce_verifier_nonce, nonce, redirect_uri, purpose, link_user_id,
                  created_at, expires_at)
             VALUES (?1, 'corp', ?2, ?3, x'00', zeroblob(24),
                     'nonce-0123456789abcdef',
                     'https://palmr.example/api/v1/auth/providers/corp/callback',
                     ?4, ?5, '2026-09-25T12:00:00.000Z', '2026-09-25T12:10:00.000Z')",
        )
        .bind(id)
        .bind(format!("{id:0<64}"))
        .bind(format!("{id:1<64}"))
        .bind(purpose)
        .bind(user)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 3,
            version: Some(5),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let purposes: Vec<(String, String)> =
        sqlx::query_as("SELECT id, purpose FROM oauth_auth_requests ORDER BY id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        purposes,
        [
            ("link".to_owned(), "link".to_owned()),
            ("login".to_owned(), "login".to_owned()),
            ("reauth".to_owned(), "reauth".to_owned()),
        ]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM oauth_auth_requests WHERE link_user_id = 'owner'"
        )
        .fetch_one(&mut connection)
        .await
        .unwrap(),
        2
    );

    // The retired label is no longer accepted.
    assert!(sqlx::query(
        "INSERT INTO oauth_auth_requests
                 (id, provider_id, state_hash, binding_cookie_hash, pkce_verifier_ciphertext,
                  pkce_verifier_nonce, nonce, redirect_uri, purpose, created_at, expires_at)
             VALUES ('legacy', 'corp', ?1, ?2, x'00',
                     zeroblob(24),
                     'nonce-0123456789abcdef',
                     'https://palmr.example/api/v1/auth/providers/corp/callback',
                     'recent_auth', '2026-09-25T12:00:00.000Z', '2026-09-25T12:10:00.000Z')",
    )
    .bind(format!("{:0<64}", "legacy"))
    .bind(format!("{:1<64}", "legacy"))
    .execute(&mut connection)
    .await
    .is_err());

    // The relative path bound is now 512, not 256.
    let long = format!("/{}", "a".repeat(511));
    assert!(
        sqlx::query("UPDATE oauth_auth_requests SET post_auth_path = ?1 WHERE id = 'login'")
            .bind(&long)
            .execute(&mut connection)
            .await
            .is_ok()
    );
    assert!(
        sqlx::query("UPDATE oauth_auth_requests SET post_auth_path = ?1 WHERE id = 'login'")
            .bind(format!("/{}", "a".repeat(512)))
            .execute(&mut connection)
            .await
            .is_err()
    );

    // Primary key, foreign keys and both indexes survive the rebuild.
    let fks: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_list('oauth_auth_requests')")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(fks, 2);
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
          WHERE type = 'index' AND tbl_name = 'oauth_auth_requests' AND name NOT LIKE 'sqlite_%'
          ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        indexes,
        [
            "ix_oauth_auth_requests_expiry",
            "ux_oauth_auth_requests_state"
        ]
    );

    // The global provider toggle is materialized with the documented default.
    let (value_type, value_json, is_secret, group): (String, String, i64, String) = sqlx::query_as(
        "SELECT value_type, value_json, is_secret, group_name
           FROM app_settings WHERE key = 'auth_providers_enabled'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        (
            value_type.as_str(),
            value_json.as_str(),
            is_secret,
            group.as_str()
        ),
        ("boolean", "true", 0, "security")
    );
    connection.close().await.unwrap();
}

#[tokio::test]
async fn it_migrate_email_linking_rejects_non_boolean_values() {
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    pools.migrate(&MIGRATOR).await.unwrap();
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    for (index, value) in ["2", "-1", "NULL"].into_iter().enumerate() {
        let id = format!("0192f3a1-0000-7000-8000-00000000010{index}");
        let outcome = sqlx::query(&format!(
            "INSERT INTO identity_providers
                 (id, key, display_name, kind, client_id, allow_email_linking, created_at, updated_at)
             VALUES (?1, ?1, ?1, 'oidc', 'client', {value}, '2026-09-25T12:00:00.000Z',
                     '2026-09-25T12:00:00.000Z')"
        ))
        .bind(id)
        .execute(&mut connection)
        .await;
        assert!(
            outcome.is_err(),
            "allow_email_linking = {value} was accepted"
        );
    }
    sqlx::query(
        "INSERT INTO identity_providers
             (id, key, display_name, kind, client_id, created_at, updated_at)
         VALUES ('0192f3a1-0000-7000-8000-000000000200', 'defaulted', 'defaulted', 'oidc',
                 'client', '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    let stored: i64 = sqlx::query_scalar(
        "SELECT allow_email_linking FROM identity_providers WHERE key = 'defaulted'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(stored, 0, "an unspecified policy must fail closed");
    connection.close().await.unwrap();
}

const SESSION_REASONS: [&str; 13] = [
    "logout",
    "user_request",
    "admin_request",
    "password_changed",
    "password_reset",
    "role_changed",
    "deactivated",
    "deleted",
    "mfa_abandoned",
    "rotated",
    "policy_changed",
    "trusted_device_revoked",
    "identity_provider_unlinked",
];

const SESSION_INDEXES: [&str; 6] = [
    "ix_sessions_absolute_exp",
    "ix_sessions_idle_exp",
    "ix_sessions_prune",
    "ix_sessions_user_active",
    "ux_sessions_mfa_token_hash",
    "ux_sessions_token_hash",
];

async fn insert_session(
    connection: &mut SqliteConnection,
    id: &str,
    state: &str,
    reason: Option<&str>,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    sqlx::query(
        "INSERT INTO sessions
             (id, user_id, token_hash, csrf_token_hash, state, auth_method, trusted_device_id,
              identity_link_id, created_at, last_seen_at, last_auth_at, idle_expires_at,
              absolute_expires_at, revoked_at, revoked_reason, ip, user_agent)
         VALUES (?1, 'owner', ?2, ?3, ?4, 'external', 'device', 'link',
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:01:00.000Z',
                 '2026-09-25T12:02:00.000Z', '2026-10-02T12:00:00.000Z',
                 '2026-10-25T12:00:00.000Z',
                 CASE WHEN ?4 = 'revoked' THEN '2026-09-26T12:00:00.000Z' END,
                 ?5, '203.0.113.9', 'agent/1')",
    )
    .bind(id)
    .bind(format!("{id:0<64}"))
    .bind(format!("{id:1<64}"))
    .bind(state)
    .bind(reason)
    .execute(connection)
    .await
}

#[tokio::test]
async fn it_migrate_session_revoked_reason_accepts_identity_unlink() {
    let files = embedded_files();
    let (_directory, frozen) = fixture_migrator(&files[..3]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    pools.migrate(&frozen).await.unwrap();
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let device = format!(
        "INSERT INTO trusted_devices (id, user_id, token_hash, created_at, expires_at)
         VALUES ('device', 'owner', '{}', '2026-09-25T12:00:00.000Z', '2027-09-25T12:00:00.000Z')",
        "d".repeat(64)
    );
    for statement in [
        "INSERT INTO users
             (id, email, email_normalized, username, username_normalized, created_at, updated_at)
         VALUES ('owner', 'owner@example.test', 'owner@example.test', 'owner', 'owner',
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')",
        "INSERT INTO identity_providers
             (id, key, display_name, kind, client_id, created_at, updated_at)
         VALUES ('corp', 'corp', 'Corp', 'oidc', 'client', '2026-09-25T12:00:00.000Z',
                 '2026-09-25T12:00:00.000Z')",
        "INSERT INTO identity_links
             (id, user_id, provider_id, subject, link_method, created_at)
         VALUES ('link', 'owner', 'corp', 'subject-1', 'manual', '2026-09-25T12:00:00.000Z')",
        device.as_str(),
    ] {
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    for (index, reason) in SESSION_REASONS[..SESSION_REASONS.len() - 1]
        .iter()
        .enumerate()
    {
        insert_session(
            &mut connection,
            &format!("revoked-{index:02}"),
            "revoked",
            Some(reason),
        )
        .await
        .unwrap();
    }
    insert_session(&mut connection, "active-01", "active", None)
        .await
        .unwrap();
    assert!(
        insert_session(
            &mut connection,
            "rejected",
            "revoked",
            Some("identity_provider_unlinked")
        )
        .await
        .is_err(),
        "the frozen schema must not know the new reason"
    );
    let before = dump_sessions(&mut connection).await;
    assert_eq!(before.len(), 13);
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 2,
            version: Some(5),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    assert_eq!(dump_sessions(&mut connection).await, before);

    for reason in SESSION_REASONS {
        let id = format!("fresh-{reason}");
        insert_session(&mut connection, &id, "revoked", Some(reason))
            .await
            .unwrap_or_else(|error| panic!("{reason} was rejected: {error}"));
    }
    for rejected in [
        "identity_unlinked",
        "IDENTITY_PROVIDER_UNLINKED",
        "",
        "unknown",
    ] {
        assert!(
            insert_session(&mut connection, "unknown-reason", "revoked", Some(rejected))
                .await
                .is_err(),
            "{rejected:?} must stay rejected"
        );
    }
    assert!(
        insert_session(&mut connection, "unknown-reason", "revoked", None)
            .await
            .is_err(),
        "a revoked session still needs a reason"
    );
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
          WHERE type = 'index' AND tbl_name = 'sessions' AND name NOT LIKE 'sqlite_%'
          ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(indexes, SESSION_INDEXES);

    let foreign_keys: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT \"table\", \"from\", on_delete FROM pragma_foreign_key_list('sessions')
          ORDER BY \"from\"",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        foreign_keys,
        [
            (
                "identity_links".to_owned(),
                "identity_link_id".to_owned(),
                "SET NULL".to_owned()
            ),
            (
                "trusted_devices".to_owned(),
                "trusted_device_id".to_owned(),
                "SET NULL".to_owned()
            ),
            (
                "users".to_owned(),
                "user_id".to_owned(),
                "CASCADE".to_owned()
            ),
        ]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        0
    );
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('sessions_new', 'sessions_old')",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(leftovers, 0);
    connection.close().await.unwrap();
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct SessionDump {
    id: String,
    user_id: String,
    token_hash: String,
    csrf_token_hash: String,
    state: String,
    auth_method: String,
    mfa_token_hash: Option<String>,
    mfa_expires_at: Option<String>,
    mfa_attempts: i64,
    trusted_device_id: Option<String>,
    identity_link_id: Option<String>,
    created_at: String,
    last_seen_at: String,
    last_auth_at: String,
    idle_expires_at: String,
    absolute_expires_at: String,
    revoked_at: Option<String>,
    revoked_reason: Option<String>,
    ip: Option<String>,
    user_agent: Option<String>,
}

async fn dump_sessions(connection: &mut SqliteConnection) -> Vec<SessionDump> {
    sqlx::query_as(
        "SELECT id, user_id, token_hash, csrf_token_hash, state, auth_method, mfa_token_hash,
                mfa_expires_at, mfa_attempts, trusted_device_id, identity_link_id, created_at,
                last_seen_at, last_auth_at, idle_expires_at, absolute_expires_at, revoked_at,
                revoked_reason, ip, user_agent
           FROM sessions ORDER BY id",
    )
    .fetch_all(connection)
    .await
    .unwrap()
}

#[tokio::test]
async fn it_migrate_password_login_enabled_defaults_on_and_preserves_settings() {
    let files = embedded_files();
    let (_directory, released) = fixture_migrator(&files[..4]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&released).await.unwrap(),
        MigrationStatus {
            applied: 4,
            version: Some(4),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    sqlx::query(
        "INSERT INTO app_settings (key, group_name, value_type, value_json, is_secret, updated_at)
         VALUES ('app_name', 'general', 'string', '\"Kept\"', 0, '2026-09-25T12:00:00.000Z')",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    let before: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT key, value_json FROM app_settings ORDER BY key")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert!(!before
        .iter()
        .any(|(key, _)| key == "password_login_enabled"));
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 1,
            version: Some(5),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let (group, value_type, value_json, is_secret, updated_by): (
        String,
        String,
        String,
        i64,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT group_name, value_type, value_json, is_secret, updated_by
           FROM app_settings WHERE key = 'password_login_enabled'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        (
            group.as_str(),
            value_type.as_str(),
            value_json.as_str(),
            is_secret,
            updated_by
        ),
        ("security", "boolean", "true", 0, None)
    );
    let after: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT key, value_json FROM app_settings WHERE key <> 'password_login_enabled'
          ORDER BY key",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(after, before);
    connection.close().await.unwrap();
}

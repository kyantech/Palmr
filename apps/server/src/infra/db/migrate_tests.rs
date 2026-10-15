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
            "0005_password_login_enabled.sql",
            "0006_deletion_lifecycle.sql",
            "0007_idempotency_transfer_envelope.sql",
            "0008_cancellation_cleanup_discovery.sql",
            "0009_tus_staging_cleanup_and_expiry_indexes.sql",
            "0010_relative_path_exact_dot_segments.sql"
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
    assert_eq!(
        files[5].1,
        include_str!("../../../migrations/0006_deletion_lifecycle.sql")
    );
    assert_eq!(
        files[6].1,
        include_str!("../../../migrations/0007_idempotency_transfer_envelope.sql")
    );
    assert_eq!(
        files[7].1,
        include_str!("../../../migrations/0008_cancellation_cleanup_discovery.sql")
    );
    assert_eq!(
        files[8].1,
        include_str!("../../../migrations/0009_tus_staging_cleanup_and_expiry_indexes.sql")
    );
    assert_eq!(
        files[9].1,
        include_str!("../../../migrations/0010_relative_path_exact_dot_segments.sql")
    );

    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(pools.path(), data.path().join(DATABASE_FILE));

    let first = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        first,
        MigrationStatus {
            applied: 10,
            version: Some(10),
        }
    );
    let again = pools.migrate(&MIGRATOR).await.unwrap();
    assert_eq!(
        again,
        MigrationStatus {
            applied: 0,
            version: Some(10),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let reopened = open_pools(data.path()).await;
    assert_eq!(
        reopened.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 0,
            version: Some(10),
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
            applied: 9,
            version: Some(10),
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
        [
            (1, true),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, true),
            (7, true),
            (8, true),
            (9, true),
            (10, true)
        ]
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
            applied: 8,
            version: Some(10),
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
            applied: 7,
            version: Some(10),
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
            applied: 6,
            version: Some(10),
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

#[tokio::test]
async fn it_migrate_deletion_lifecycle_keeps_existing_folders_live_and_checksums_intact() {
    let files = embedded_files();
    let (_directory, released) = fixture_migrator(&files[..5]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&released).await.unwrap(),
        MigrationStatus {
            applied: 5,
            version: Some(5),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let before_records = recorded_migrations(&mut connection).await;
    for statement in [
        "INSERT INTO users (id, email, email_normalized, username, username_normalized, created_at, updated_at)
         VALUES ('0192f3a1-0000-7000-8000-000000000001', 'a@example.test', 'a@example.test', 'ada', 'ada',
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')",
        "INSERT INTO folders (id, owner_id, parent_id, name, name_normalized, depth, created_at, updated_at)
         VALUES ('0192f3a1-0000-7000-8000-0000000000a1', '0192f3a1-0000-7000-8000-000000000001', NULL, 'Root', 'root', 0,
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z'),
                ('0192f3a1-0000-7000-8000-0000000000a2', '0192f3a1-0000-7000-8000-000000000001',
                 '0192f3a1-0000-7000-8000-0000000000a1', 'Child', 'child', 1,
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z')",
        "INSERT INTO transfer_sessions (id, context, user_id, provider, state, target_folder_id, created_at, updated_at, expires_at)
         VALUES ('0192f3a1-0000-7000-8000-0000000000c1', 'my_files', '0192f3a1-0000-7000-8000-000000000001', 'local',
                 'completed', '0192f3a1-0000-7000-8000-0000000000a2',
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z', '2026-09-26T12:00:00.000Z')",
    ] {
        sqlx::query(statement).execute(&mut connection).await.unwrap();
    }
    let folders_before: Vec<(String, Option<String>, String, i64)> =
        sqlx::query_as("SELECT id, parent_id, name, depth FROM folders ORDER BY id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 5,
            version: Some(10),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let after: Vec<(String, Option<String>, String, i64, i64)> =
        sqlx::query_as("SELECT id, parent_id, name, depth, deleting FROM folders ORDER BY id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        after
            .iter()
            .map(|row| (row.0.clone(), row.1.clone(), row.2.clone(), row.3))
            .collect::<Vec<_>>(),
        folders_before
    );
    assert!(
        after.iter().all(|row| row.4 == 0),
        "existing folders are not deleting"
    );

    let recorded = recorded_migrations(&mut connection).await;
    assert_eq!(
        recorded[..5],
        before_records[..],
        "applied migrations are untouched"
    );
    assert_eq!(recorded[5].version, 6);
    assert!(recorded[5].success);
    assert_eq!(recorded[6].version, 7);
    assert!(recorded[6].success);
    assert_eq!(recorded[7].version, 8);
    assert!(recorded[7].success);
    assert_eq!(recorded[8].version, 9);
    assert!(recorded[8].success);

    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");

    let rejected = sqlx::query(
        "UPDATE folders SET deleting = 2 WHERE id = '0192f3a1-0000-7000-8000-0000000000a1'",
    )
    .execute(&mut connection)
    .await;
    assert!(rejected.is_err(), "deleting is a canonical boolean");
    sqlx::query(
        "UPDATE folders SET deleting = 1 WHERE id = '0192f3a1-0000-7000-8000-0000000000a1'",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN SELECT 1 FROM folders z
          WHERE z.owner_id = '0192f3a1-0000-7000-8000-000000000001' AND z.deleting = 1",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert!(
        plan.iter().any(|row| row.3.contains("ix_folders_deleting")),
        "{plan:?}"
    );
    let session: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT target_folder_id, deleted_target_folder_id FROM transfer_sessions")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        session,
        (
            Some("0192f3a1-0000-7000-8000-0000000000a2".to_owned()),
            None
        ),
        "an existing session keeps its live destination and has no historical marker"
    );
    let trigger: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'trigger' AND name = 'transfer_sessions_deleted_target_immutable'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(trigger, 1);
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name IN ('folder_deletions', 'deletion_receipts') ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(tables, ["deletion_receipts", "folder_deletions"]);
    connection.close().await.unwrap();
}

const TRANSFER_ROUTE: &str = "/api/v1/transfers/sessions";
const OTHER_ROUTE: &str = "/api/v1/folders/ensure-path";

fn json_string_of_bytes(bytes: usize) -> String {
    format!("\"{}\"", "a".repeat(bytes - 2))
}

async fn insert_completed_record(
    connection: &mut SqliteConnection,
    id: &str,
    route: &str,
    response: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO idempotency_records
            (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash, state,
             response_status, response_json, created_at, completed_at, expires_at)
         VALUES (?1, 'user', 'scope-1', 'POST', ?2, ?3, ?3, 'completed', 201, ?4,
                 '2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z', '2026-09-26T12:00:00.000Z')",
    )
    .bind(id)
    .bind(route)
    .bind(format!("{id:0>64}"))
    .bind(response)
    .execute(connection)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn it_migrate_idempotency_envelope_bound_keeps_records_and_counts_bytes() {
    let files = embedded_files();
    let (_directory, released) = fixture_migrator(&files[..6]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&released).await.unwrap(),
        MigrationStatus {
            applied: 6,
            version: Some(6),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    insert_completed_record(&mut connection, "kept-1", OTHER_ROUTE, "{\"ids\":[\"a\"]}")
        .await
        .unwrap();
    insert_completed_record(
        &mut connection,
        "kept-2",
        OTHER_ROUTE,
        &json_string_of_bytes(16_384),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO idempotency_records
            (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash, state,
             lease_expires_at, created_at, expires_at)
         VALUES ('kept-3', 'user', 'scope-2', 'POST', ?1, ?2, ?2, 'in_progress',
                 '2026-09-25T12:00:30.000Z', '2026-09-25T12:00:00.000Z', '2026-09-26T12:00:00.000Z')",
    )
    .bind(TRANSFER_ROUTE)
    .bind("b".repeat(64))
    .execute(&mut connection)
    .await
    .unwrap();
    let snapshot = "SELECT id || '|' || scope_kind || '|' || scope_id || '|' || http_method || '|' ||
                           route_template || '|' || key_hash || '|' || request_hash || '|' || state || '|' ||
                           ifnull(lease_expires_at, '-') || '|' || ifnull(response_status, '-') || '|' ||
                           ifnull(response_json, '-') || '|' || created_at || '|' ||
                           ifnull(completed_at, '-') || '|' || expires_at
                      FROM idempotency_records ORDER BY id";
    let before: Vec<String> = sqlx::query_scalar(snapshot)
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert_eq!(before.len(), 3);
    let before_records = recorded_migrations(&mut connection).await;
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 4,
            version: Some(10),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let after: Vec<String> = sqlx::query_scalar(snapshot)
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert_eq!(after, before, "every record survives the rebuild unchanged");
    assert_eq!(
        recorded_migrations(&mut connection).await[..6],
        before_records[..],
        "applied migrations are untouched"
    );
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema
          WHERE type = 'index' AND tbl_name = 'idempotency_records' AND name NOT LIKE 'sqlite_%'
          ORDER BY name",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(indexes, ["ix_idempotency_expiry", "ux_idempotency_scope"]);
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'idempotency_records_next'",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(leftovers, 0);

    let limit = 6 * 1024 * 1024;
    insert_completed_record(
        &mut connection,
        "t-exact",
        TRANSFER_ROUTE,
        &json_string_of_bytes(limit),
    )
    .await
    .unwrap();
    assert!(
        insert_completed_record(
            &mut connection,
            "t-over",
            TRANSFER_ROUTE,
            &json_string_of_bytes(limit + 1)
        )
        .await
        .is_err(),
        "one byte over the transfer-session bound is refused"
    );
    insert_completed_record(
        &mut connection,
        "o-exact",
        OTHER_ROUTE,
        &json_string_of_bytes(16_384),
    )
    .await
    .unwrap();
    assert!(
        insert_completed_record(
            &mut connection,
            "o-over",
            OTHER_ROUTE,
            &json_string_of_bytes(16_385)
        )
        .await
        .is_err(),
        "every other route keeps 16 KiB"
    );
    let two_byte_characters = format!("\"{}\"", "\u{e9}".repeat(9_000));
    assert!(two_byte_characters.chars().count() < 16_384 && two_byte_characters.len() > 16_384);
    assert!(
        insert_completed_record(&mut connection, "o-wide", OTHER_ROUTE, &two_byte_characters)
            .await
            .is_err(),
        "the bound counts bytes, not characters"
    );
    insert_completed_record(
        &mut connection,
        "t-wide",
        TRANSFER_ROUTE,
        &two_byte_characters,
    )
    .await
    .unwrap();
    assert!(
        insert_completed_record(&mut connection, "t-invalid", TRANSFER_ROUTE, "{not json")
            .await
            .is_err()
    );

    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    connection.close().await.unwrap();
}

const TSF_USER: &str = "0192f3a1-0000-7000-8000-000000000001";
const TSF_SESSION: &str = "0192f3a1-0000-7000-8000-0000000000c1";
const TSF_ITEM_ONE: &str = "0192f3a1-0000-7000-8000-0000000000d1";
const TSF_ITEM_TWO: &str = "0192f3a1-0000-7000-8000-0000000000d2";
const TSF_STAMP: &str = "'2026-09-25T12:00:00.000Z', '2026-09-25T12:00:00.000Z'";

fn transfer_item_insert(id: &str, ordinal: u32, prefix: &str, key_tail: &str) -> String {
    format!(
        "INSERT INTO transfer_session_files
             (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
              declared_size_bytes, reserved_bytes, upload_kind, final_object_id, final_object_key,
              created_at, updated_at)
         VALUES ('{id}', '{TSF_SESSION}', {ordinal}, 'key-{ordinal}', 'file-{ordinal}.bin',
                 '{prefix}', 100, 100, 'tus', '0192f3a1-0000-7000-8000-0000000000e{ordinal}',
                 'objects/ab/cd/{key_tail}', {TSF_STAMP})"
    )
}

const TRANSFER_TABLES: [&str; 4] = [
    "transfer_session_files",
    "tus_uploads",
    "s3_multipart_uploads",
    "s3_multipart_parts",
];

async fn transfer_shape(connection: &mut SqliteConnection) -> Vec<String> {
    let mut shape = Vec::new();
    for table in TRANSFER_TABLES {
        let columns: Vec<(String, String, i64, i64)> = sqlx::query_as(&format!(
            "SELECT name, type, \"notnull\", pk FROM pragma_table_info('{table}')"
        ))
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let foreign_keys: Vec<(String, String, String, String)> = sqlx::query_as(&format!(
            "SELECT \"from\", \"table\", \"to\", on_delete FROM pragma_foreign_key_list('{table}')
              ORDER BY \"from\", \"table\""
        ))
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let indexes: Vec<(String, Option<String>)> = sqlx::query_as(&format!(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'index' AND tbl_name = '{table}'
                AND sql IS NOT NULL ORDER BY name"
        ))
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        shape.push(format!("{table} {columns:?} {foreign_keys:?} {indexes:?}"));
    }
    shape
}

async fn transfer_rows(connection: &mut SqliteConnection) -> Vec<String> {
    let mut rows = Vec::new();
    for sql in [
        "SELECT json_group_array(json_object('id', id, 'session', transfer_session_id, 'ordinal', ordinal,
            'key', client_file_key, 'name', display_name, 'path', relative_path, 'size', declared_size_bytes,
            'reserved', reserved_bytes, 'kind', upload_kind, 'state', state, 'stage', finalize_stage,
            'oid', final_object_id, 'okey', final_object_key, 'attempts', attempts, 'created', created_at))
           FROM (SELECT * FROM transfer_session_files ORDER BY id)",
        "SELECT json_group_array(json_object('id', id, 'item', transfer_session_file_id, 'owner', owner_user_id,
            'length', upload_length, 'offset', upload_offset, 'staging', staging_path, 'state', state,
            'removed', staging_removed_at, 'expires', expires_at))
           FROM (SELECT * FROM tus_uploads ORDER BY id)",
        "SELECT json_group_array(json_object('id', id, 'item', transfer_session_file_id, 'upload', s3_upload_id,
            'bucket', bucket, 'key', object_key, 'owner', owner_user_id, 'part', part_size_bytes,
            'count', part_count, 'state', state, 'reconciled', last_reconciled_at))
           FROM (SELECT * FROM s3_multipart_uploads ORDER BY id)",
        "SELECT json_group_array(json_object('upload', s3_multipart_upload_id, 'n', part_number,
            'size', size_bytes, 'etag', etag, 'state', state, 'attempts', attempts))
           FROM (SELECT * FROM s3_multipart_parts ORDER BY s3_multipart_upload_id, part_number)",
    ] {
        rows.push(
            sqlx::query_scalar::<_, String>(sql)
                .fetch_one(&mut *connection)
                .await
                .unwrap(),
        );
    }
    rows
}

#[tokio::test]
async fn it_migrate_relative_path_exact_dot_segments_rebuilds_transfer_tables_without_loss() {
    let files = embedded_files();
    let (_directory, released) = fixture_migrator(&files[..9]).await;
    let data = TempDir::new().unwrap();
    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&released).await.unwrap(),
        MigrationStatus {
            applied: 9,
            version: Some(9),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    let item_one = transfer_item_insert(TSF_ITEM_ONE, 1, "photos/2026", &"a".repeat(32));
    let item_two = transfer_item_insert(TSF_ITEM_TWO, 2, "", &"b".repeat(32));
    for statement in [
        format!(
            "INSERT INTO users (id, email, email_normalized, username, username_normalized, created_at, updated_at)
             VALUES ('{TSF_USER}', 'a@example.test', 'a@example.test', 'ada', 'ada', {TSF_STAMP})"
        ),
        format!(
            "INSERT INTO transfer_sessions (id, context, user_id, provider, state, created_at, updated_at, expires_at)
             VALUES ('{TSF_SESSION}', 'my_files', '{TSF_USER}', 'local', 'uploading', {TSF_STAMP},
                     '2026-10-02T12:00:00.000Z')"
        ),
        item_one,
        item_two,
        format!(
            "INSERT INTO tus_uploads (id, transfer_session_file_id, owner_user_id, upload_length, upload_offset,
                 staging_path, state, created_at, updated_at, expires_at, staging_removed_at)
             VALUES ('tus-1', '{TSF_ITEM_ONE}', '{TSF_USER}', 100, 40, 'uploads/tus-1/blob', 'in_progress',
                     {TSF_STAMP}, '2026-09-26T12:00:00.000Z', '2026-09-25T13:00:00.000Z')"
        ),
        format!(
            "INSERT INTO s3_multipart_uploads (id, transfer_session_file_id, s3_upload_id, bucket, object_key,
                 owner_user_id, total_size_bytes, part_size_bytes, part_count, state, created_at, updated_at,
                 expires_at, last_reconciled_at)
             VALUES ('s3mp-1', '{TSF_ITEM_TWO}', 'provider-upload', 'bucket', 'objects/ab/cd/{}', '{TSF_USER}',
                     5243000, 5242880, 2, 'in_progress', {TSF_STAMP}, '2026-09-26T12:00:00.000Z',
                     '2026-09-25T14:00:00.000Z')",
            "b".repeat(32)
        ),
        "INSERT INTO s3_multipart_parts (s3_multipart_upload_id, part_number, size_bytes, etag, state, attempts)
         VALUES ('s3mp-1', 1, 5242880, 'etag-1', 'uploaded', 1), ('s3mp-1', 2, 120, NULL, 'planned', 0)"
            .to_owned(),
    ] {
        sqlx::query(&statement).execute(&mut connection).await.unwrap();
    }
    let rows_before = transfer_rows(&mut connection).await;
    let shape_before = transfer_shape(&mut connection).await;
    let records_before = recorded_migrations(&mut connection).await;
    connection.close().await.unwrap();

    let pools = open_pools(data.path()).await;
    assert_eq!(
        pools.migrate(&MIGRATOR).await.unwrap(),
        MigrationStatus {
            applied: 1,
            version: Some(10),
        }
    );
    pools.shutdown().await.checkpoint.unwrap();

    let mut connection = raw_connection(data.path()).await;
    assert_eq!(
        transfer_rows(&mut connection).await,
        rows_before,
        "every row of the four rebuilt tables survives unchanged"
    );
    let shape_after = transfer_shape(&mut connection).await;
    assert_eq!(
        shape_after.len(),
        shape_before.len(),
        "the same four tables"
    );
    for (before, after) in shape_before.iter().zip(&shape_after) {
        assert_eq!(
            before, after,
            "columns, foreign keys and indexes are identical"
        );
    }
    assert_eq!(
        recorded_migrations(&mut connection).await[..9],
        records_before[..],
        "applied migrations are untouched"
    );
    let violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("SELECT * FROM pragma_foreign_key_check")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    let leftovers: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema
          WHERE type = 'table' AND (name LIKE '%\\_held' ESCAPE '\\' OR name LIKE '%\\_next' ESCAPE '\\')",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(leftovers, Vec::<String>::new());

    let restricted = sqlx::query("DELETE FROM transfer_session_files WHERE id = ?1")
        .bind(TSF_ITEM_ONE)
        .execute(&mut connection)
        .await;
    assert!(
        restricted.is_err(),
        "the RESTRICT keys from the protocol rows still protect their item"
    );
    for (ordinal, prefix) in [
        (3, "versions..old"),
        (4, "project...backup/readme..d"),
        (5, "a/..hidden"),
        (6, "x/mid..dle/y"),
    ] {
        sqlx::query(&transfer_item_insert(
            &format!("0192f3a1-0000-7000-8000-0000000000f{ordinal}"),
            ordinal,
            prefix,
            &format!("{ordinal:0>32}"),
        ))
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("{prefix:?} must be accepted: {error}"));
    }
    for (ordinal, prefix) in [
        (7, ".."),
        (8, "../x"),
        (9, "x/.."),
        (10, "x/../y"),
        (11, "/abs"),
    ] {
        let refused = sqlx::query(&transfer_item_insert(
            &format!("0192f3a1-0000-7000-8000-0000000000f{ordinal}"),
            ordinal,
            prefix,
            &format!("{ordinal:0>32}"),
        ))
        .execute(&mut connection)
        .await;
        assert!(refused.is_err(), "{prefix:?} must still be refused");
    }
}

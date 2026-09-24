use crate::infra::crypto::aead::SealedSecret;
use crate::infra::db::{DbError, ReadPool, WriteTx};

use super::model::SettingSpec;

#[derive(Debug, Clone)]
pub struct SettingRow {
    pub key: String,
    pub group: String,
    pub value_type: String,
    pub value_json: Option<String>,
    pub ciphertext: Option<Vec<u8>>,
    pub nonce: Option<Vec<u8>>,
    pub key_version: i64,
}

const SELECT_ALL: &str = "SELECT key, group_name, value_type, value_json,
                                 secret_ciphertext, secret_nonce, key_version
                          FROM app_settings
                          ORDER BY key";

const UPSERT_VALUE: &str = "INSERT INTO app_settings (
        key, group_name, value_type, value_json, is_secret, key_version, updated_at, updated_by
    ) VALUES (?1, ?2, ?3, ?4, 0, 1, ?5, ?6)
    ON CONFLICT(key) DO UPDATE SET
        group_name = excluded.group_name,
        value_type = excluded.value_type,
        value_json = excluded.value_json,
        secret_ciphertext = NULL,
        secret_nonce = NULL,
        is_secret = 0,
        key_version = 1,
        updated_at = excluded.updated_at,
        updated_by = excluded.updated_by";

const UPSERT_SECRET: &str = "INSERT INTO app_settings (
        key, group_name, value_type, secret_ciphertext, secret_nonce,
        is_secret, key_version, updated_at, updated_by
    ) VALUES (?1, ?2, 'secret', ?3, ?4, 1, ?5, ?6, ?7)
    ON CONFLICT(key) DO UPDATE SET
        group_name = excluded.group_name,
        value_type = 'secret',
        value_json = NULL,
        secret_ciphertext = excluded.secret_ciphertext,
        secret_nonce = excluded.secret_nonce,
        is_secret = 1,
        key_version = excluded.key_version,
        updated_at = excluded.updated_at,
        updated_by = excluded.updated_by";

const DELETE: &str = "DELETE FROM app_settings WHERE key = ?1";

type RawRow = (
    String,
    String,
    String,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    i64,
);

pub async fn load_all(reader: &ReadPool) -> Result<Vec<SettingRow>, DbError> {
    let rows: Vec<RawRow> = sqlx::query_as(SELECT_ALL)
        .fetch_all(reader.executor())
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(key, group, value_type, value_json, ciphertext, nonce, key_version)| SettingRow {
                key,
                group,
                value_type,
                value_json,
                ciphertext,
                nonce,
                key_version,
            },
        )
        .collect())
}

pub async fn upsert_value(
    tx: &mut WriteTx<'_>,
    spec: &SettingSpec,
    value_json: &str,
    updated_at: &str,
    updated_by: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(UPSERT_VALUE)
        .bind(spec.key)
        .bind(spec.group.as_str())
        .bind(spec.value_type.as_str())
        .bind(value_json)
        .bind(updated_at)
        .bind(updated_by)
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn upsert_secret(
    tx: &mut WriteTx<'_>,
    key: &str,
    group: &str,
    sealed: &SealedSecret,
    updated_at: &str,
    updated_by: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(UPSERT_SECRET)
        .bind(key)
        .bind(group)
        .bind(sealed.ciphertext())
        .bind(sealed.nonce().as_slice())
        .bind(sealed.key_version())
        .bind(updated_at)
        .bind(updated_by)
        .execute(tx.executor())
        .await?;
    Ok(())
}

pub async fn delete(tx: &mut WriteTx<'_>, key: &str) -> Result<(), DbError> {
    sqlx::query(DELETE).bind(key).execute(tx.executor()).await?;
    Ok(())
}

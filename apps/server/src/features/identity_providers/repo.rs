use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use super::discovery::discovery_url;
use super::error::ProviderError;
use super::model::{
    ClaimMapping, IdentityProvider, OAuth2Provider, OidcProvider, Preset, Protocol, ProviderId,
    ProviderRecord, ProviderVariant, TokenAuthMethod,
};
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::db::{DbError, ReadPool, WriteTx};
use crate::infra::http::pagination::{Conjunction, PageRequest};

const COLUMNS: &str = "p.id, p.key, p.display_name, p.kind, p.preset, p.issuer, p.discovery_url,
    p.authorization_endpoint, p.token_endpoint, p.userinfo_endpoint, p.jwks_uri, p.scopes,
    p.client_id, p.client_secret_ciphertext, p.client_secret_nonce, p.key_version,
    p.token_auth_method, p.claim_subject, p.claim_email, p.claim_email_verified,
    p.claim_username, p.claim_name, p.claim_avatar, p.is_enabled, p.auto_provision,
    p.allow_email_linking, p.sort_order, p.validated_at, p.validation_error, p.created_at,
    p.updated_at,
    (SELECT COUNT(DISTINCT l.user_id) FROM identity_links l WHERE l.provider_id = p.id)
        AS linked_user_count";

const FROM: &str = " FROM identity_providers p";

const INSERT: &str = "INSERT INTO identity_providers
    (id, key, display_name, kind, preset, issuer, discovery_url, authorization_endpoint,
     token_endpoint, userinfo_endpoint, jwks_uri, scopes, client_id, client_secret_ciphertext,
     client_secret_nonce, key_version, token_auth_method, claim_subject, claim_email,
     claim_email_verified, claim_username, claim_name, claim_avatar, is_enabled, auto_provision,
     allow_email_linking, sort_order, validated_at, validation_error, created_at, updated_at,
     updated_by)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
            ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?30, ?31)";

const UPDATE: &str = "UPDATE identity_providers SET
    display_name = ?3, kind = ?4, preset = ?5, issuer = ?6, discovery_url = ?7,
    authorization_endpoint = ?8, token_endpoint = ?9, userinfo_endpoint = ?10, jwks_uri = ?11,
    scopes = ?12, client_id = ?13, client_secret_ciphertext = ?14, client_secret_nonce = ?15,
    key_version = ?16, token_auth_method = ?17, claim_subject = ?18, claim_email = ?19,
    claim_email_verified = ?20, claim_username = ?21, claim_name = ?22, claim_avatar = ?23,
    is_enabled = ?24, auto_provision = ?25, allow_email_linking = ?26, sort_order = ?27,
    validated_at = ?28, validation_error = ?29, updated_at = ?30, updated_by = ?31
    WHERE id = ?1 AND updated_at = ?2";

pub struct ProviderWrite<'a> {
    pub id: ProviderId,
    pub slug: &'a str,
    pub display_name: &'a str,
    pub kind: &'a ProviderVariant,
    pub preset: Preset,
    pub scopes: &'a [String],
    pub client_id: &'a str,
    pub secret: Option<&'a SealedSecret>,
    pub token_auth_method: TokenAuthMethod,
    pub claims: &'a ClaimMapping,
    pub enabled: bool,
    pub auto_provision: bool,
    pub allow_email_linking: bool,
    pub sort_order: i64,
    pub validated_at: Option<Timestamp>,
    pub validation_error: Option<&'a str>,
    pub at: Timestamp,
    pub updated_by: Option<UserId>,
}

pub async fn list(
    reader: &ReadPool,
    page: &PageRequest,
) -> Result<(Vec<ProviderRecord>, u64), ProviderError> {
    let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
    query.push(COLUMNS).push(FROM);
    page.push_keyset(&mut query, Conjunction::Where);
    page.push_order_and_limit(&mut query);
    let rows = query.build().fetch_all(reader.executor()).await?;
    let records = rows
        .iter()
        .map(record_from)
        .collect::<Result<Vec<_>, _>>()?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identity_providers")
        .fetch_one(reader.executor())
        .await?;
    let total = u64::try_from(total).map_err(|_| invariant("count"))?;
    Ok((records, total))
}

pub async fn find(
    reader: &ReadPool,
    id: ProviderId,
) -> Result<Option<ProviderRecord>, ProviderError> {
    let row = sqlx::query(&format!("SELECT {COLUMNS}{FROM} WHERE p.id = ?1"))
        .bind(id.to_string())
        .fetch_optional(reader.executor())
        .await?;
    row.as_ref().map(record_from).transpose()
}

pub async fn find_in_tx(
    tx: &mut WriteTx<'_>,
    id: ProviderId,
) -> Result<Option<ProviderRecord>, ProviderError> {
    let row = sqlx::query(&format!("SELECT {COLUMNS}{FROM} WHERE p.id = ?1"))
        .bind(id.to_string())
        .fetch_optional(tx.executor())
        .await?;
    row.as_ref().map(record_from).transpose()
}

pub async fn slug_taken(reader: &ReadPool, slug: &str) -> Result<bool, ProviderError> {
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM identity_providers WHERE key = ?1)")
            .bind(slug)
            .fetch_one(reader.executor())
            .await?;
    Ok(taken)
}

pub async fn next_sort_order(tx: &mut WriteTx<'_>) -> Result<i64, ProviderError> {
    let next: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sort_order) + 1, 0) FROM identity_providers")
            .fetch_one(tx.executor())
            .await?;
    Ok(next)
}

pub async fn insert(tx: &mut WriteTx<'_>, write: &ProviderWrite<'_>) -> Result<(), ProviderError> {
    let query = bind_all(sqlx::query(INSERT), write, write.slug.to_owned());
    match query.execute(tx.executor()).await {
        Ok(_) => Ok(()),
        Err(error) => Err(match DbError::from(error) {
            DbError::UniqueViolation(_) => ProviderError::SlugTaken,
            other => ProviderError::Db(other),
        }),
    }
}

pub async fn update(
    tx: &mut WriteTx<'_>,
    write: &ProviderWrite<'_>,
    expected_updated_at: Timestamp,
) -> Result<bool, ProviderError> {
    let query = bind_all(sqlx::query(UPDATE), write, expected_updated_at.to_string());
    let result = query.execute(tx.executor()).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn delete(tx: &mut WriteTx<'_>, id: ProviderId) -> Result<bool, ProviderError> {
    let linked: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM identity_links WHERE provider_id = ?1)")
            .bind(id.to_string())
            .fetch_one(tx.executor())
            .await?;
    if linked {
        return Err(ProviderError::HasLinks);
    }
    let result = sqlx::query("DELETE FROM identity_providers WHERE id = ?1")
        .bind(id.to_string())
        .execute(tx.executor())
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn ids_in_order(tx: &mut WriteTx<'_>) -> Result<Vec<(String, String)>, ProviderError> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, key FROM identity_providers ORDER BY sort_order, id")
            .fetch_all(tx.executor())
            .await?;
    Ok(rows)
}

pub async fn set_sort_order(
    tx: &mut WriteTx<'_>,
    id: ProviderId,
    sort_order: i64,
) -> Result<bool, ProviderError> {
    let result = sqlx::query(
        "UPDATE identity_providers SET sort_order = ?2 WHERE id = ?1 AND sort_order <> ?2",
    )
    .bind(id.to_string())
    .bind(sort_order)
    .execute(tx.executor())
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn stamp_validation(
    tx: &mut WriteTx<'_>,
    id: ProviderId,
    expected_updated_at: Timestamp,
    validated_at: Option<Timestamp>,
    validation_error: Option<&str>,
) -> Result<bool, ProviderError> {
    let result = sqlx::query(
        "UPDATE identity_providers SET validated_at = ?3, validation_error = ?4
         WHERE id = ?1 AND updated_at = ?2",
    )
    .bind(id.to_string())
    .bind(expected_updated_at.to_string())
    .bind(validated_at.map(|at| at.to_string()))
    .bind(validation_error)
    .execute(tx.executor())
    .await?;
    Ok(result.rows_affected() == 1)
}

type Query<'q> = sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>;

fn bind_all<'q>(query: Query<'q>, write: &ProviderWrite<'_>, second: String) -> Query<'q> {
    let endpoints = write.kind.endpoints();
    let kind = write.kind;
    query
        .bind(write.id.to_string())
        .bind(second)
        .bind(write.display_name.to_owned())
        .bind(kind.protocol().as_str())
        .bind(write.preset.as_str())
        .bind(kind.issuer().map(str::to_owned))
        .bind(kind.discovery_url().map(str::to_owned))
        .bind(endpoints.authorization)
        .bind(endpoints.token)
        .bind(endpoints.userinfo)
        .bind(endpoints.jwks)
        .bind(write.scopes.join(" "))
        .bind(write.client_id.to_owned())
        .bind(write.secret.map(|secret| secret.ciphertext().to_vec()))
        .bind(write.secret.map(|secret| secret.nonce().to_vec()))
        .bind(write.secret.map_or(
            crate::infra::crypto::aead::KEY_VERSION,
            SealedSecret::key_version,
        ))
        .bind(write.token_auth_method.as_str())
        .bind(write.claims.subject.clone())
        .bind(write.claims.email.clone())
        .bind(write.claims.email_verified.clone())
        .bind(write.claims.username.clone())
        .bind(write.claims.name.clone())
        .bind(write.claims.picture.clone())
        .bind(write.enabled)
        .bind(write.auto_provision)
        .bind(write.allow_email_linking)
        .bind(write.sort_order)
        .bind(write.validated_at.map(|at| at.to_string()))
        .bind(write.validation_error.map(str::to_owned))
        .bind(write.at.to_string())
        .bind(write.updated_by.map(|id| id.to_string()))
}

fn record_from(row: &SqliteRow) -> Result<ProviderRecord, ProviderError> {
    let text = |column: &'static str| -> Result<String, ProviderError> {
        row.try_get::<String, _>(column)
            .map_err(|_| invariant(column))
    };
    let optional = |column: &'static str| -> Result<Option<String>, ProviderError> {
        row.try_get::<Option<String>, _>(column)
            .map_err(|_| invariant(column))
    };
    let flag = |column: &'static str| -> Result<bool, ProviderError> {
        row.try_get::<bool, _>(column)
            .map_err(|_| invariant(column))
    };
    let timestamp = |column: &'static str| -> Result<Timestamp, ProviderError> {
        text(column)?.parse().map_err(|_| invariant(column))
    };

    let id = text("id")?
        .parse::<ProviderId>()
        .map_err(|_| invariant("id"))?;
    let protocol = Protocol::parse(&text("kind")?).ok_or_else(|| invariant("kind"))?;
    let preset = optional("preset")?
        .map_or(Some(Preset::Generic), |preset| Preset::parse(&preset))
        .ok_or_else(|| invariant("preset"))?;
    let authorization = optional("authorization_endpoint")?;
    let token = optional("token_endpoint")?;
    let userinfo = optional("userinfo_endpoint")?;
    let kind = match protocol {
        Protocol::Oidc => {
            let issuer = optional("issuer")?.ok_or_else(|| invariant("issuer"))?;
            ProviderVariant::Oidc(OidcProvider {
                discovery_url: optional("discovery_url")?.unwrap_or_else(|| discovery_url(&issuer)),
                issuer,
                authorization_endpoint: authorization
                    .ok_or_else(|| invariant("authorization_endpoint"))?,
                token_endpoint: token.ok_or_else(|| invariant("token_endpoint"))?,
                userinfo_endpoint: userinfo,
                jwks_uri: optional("jwks_uri")?.ok_or_else(|| invariant("jwks_uri"))?,
            })
        }
        Protocol::OAuth2 => ProviderVariant::OAuth2(OAuth2Provider {
            authorization_endpoint: authorization
                .ok_or_else(|| invariant("authorization_endpoint"))?,
            token_endpoint: token.ok_or_else(|| invariant("token_endpoint"))?,
            userinfo_endpoint: userinfo.ok_or_else(|| invariant("userinfo_endpoint"))?,
        }),
    };
    let ciphertext: Option<Vec<u8>> = row
        .try_get("client_secret_ciphertext")
        .map_err(|_| invariant("client_secret_ciphertext"))?;
    let nonce: Option<Vec<u8>> = row
        .try_get("client_secret_nonce")
        .map_err(|_| invariant("client_secret_nonce"))?;
    let key_version: i64 = row
        .try_get("key_version")
        .map_err(|_| invariant("key_version"))?;
    let client_secret = match (ciphertext, nonce) {
        (Some(ciphertext), Some(nonce)) => Some(
            SealedSecret::from_parts(ciphertext, &nonce, key_version)
                .map_err(|_| invariant("client_secret_ciphertext"))?,
        ),
        (None, None) => None,
        _ => return Err(invariant("client_secret_nonce")),
    };
    let token_auth_method = TokenAuthMethod::parse(&text("token_auth_method")?)
        .ok_or_else(|| invariant("token_auth_method"))?;
    let linked: i64 = row
        .try_get("linked_user_count")
        .map_err(|_| invariant("linked_user_count"))?;
    let provider = IdentityProvider {
        id,
        slug: text("key")?,
        display_name: text("display_name")?,
        preset,
        enabled: flag("is_enabled")?,
        sort_order: row
            .try_get("sort_order")
            .map_err(|_| invariant("sort_order"))?,
        auto_provision: flag("auto_provision")?,
        allow_email_linking: flag("allow_email_linking")?,
        client_id: text("client_id")?,
        client_secret,
        scopes: text("scopes")?
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        token_auth_method,
        claims: ClaimMapping {
            subject: text("claim_subject")?,
            email: text("claim_email")?,
            email_verified: text("claim_email_verified")?,
            username: text("claim_username")?,
            name: text("claim_name")?,
            picture: text("claim_avatar")?,
        },
        kind,
        validated_at: optional("validated_at")?
            .map(|at| at.parse().map_err(|_| invariant("validated_at")))
            .transpose()?,
        validation_error: optional("validation_error")?,
        created_at: timestamp("created_at")?,
        updated_at: timestamp("updated_at")?,
    };
    Ok(ProviderRecord {
        provider,
        linked_user_count: u64::try_from(linked).map_err(|_| invariant("linked_user_count"))?,
    })
}

fn invariant(column: &'static str) -> ProviderError {
    ProviderError::RepositoryInvariant { column }
}

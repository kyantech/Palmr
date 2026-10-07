//! Authorization-request entry points for external identity providers.
//!
//! This module owns everything that must happen *before* the IdP redirect: the
//! public enabled-provider list, the shared authorization-request service, the
//! cryptographic request values, the exact redirect URI and the validated
//! post-authentication path. It never exchanges a code, consumes state, resolves
//! an account or issues a session; the callback (M12-T04) owns all of that.

use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use super::error::ProviderError;
use super::model::{
    self, AuthRequestId, AuthorizePurpose, IdentityProvider, PublicProvider, PublicProviderList,
};
use super::reauth;
use super::repo::{self, AuthRequestWrite};
use crate::domain::secret::Secret;
use crate::domain::time::Timestamp;
use crate::features::users::model::UserId;
use crate::infra::crypto::hash::sha256_base64url;
use crate::infra::crypto::hkdf::SealPurpose;
use crate::infra::crypto::token::Token;
use crate::infra::crypto::{base64url_no_pad, random_bytes};
use crate::infra::http::cookies::CookiePolicy;
use crate::infra::http::json::{JsonField, JsonKind, JsonRequest};

pub const AUTHORIZE_TRANSACTION: &str = "identity_providers.authorize";
pub const AUTH_REQUEST_TTL_SECONDS: u64 = 600;
pub const PKCE_VERIFIER_BYTES: usize = 96;
pub const MAX_RETURN_TO_LEN: usize = 512;
pub const DEFAULT_RETURN_TO: &str = "/overview";

/// SPA route prefixes accepted for a post-authentication relative path.
const SPA_PREFIXES: [&str; 10] = [
    "/overview",
    "/files",
    "/shared",
    "/received",
    "/transfers",
    "/settings",
    "/admin",
    "/s/",
    "/r/",
    "/e/",
];

/// Authorization parameters Palmr generates. Any configured duplicate in the
/// provider's authorization endpoint is removed so configuration can never
/// override a security parameter.
const RESERVED_AUTHORIZE_PARAMS: [&str; 9] = [
    "response_type",
    "client_id",
    "redirect_uri",
    "scope",
    "state",
    "code_challenge",
    "code_challenge_method",
    "nonce",
    "prompt",
];

/// The internal request context of one authorization request. `Login` binds no
/// user; `Link` and `Reauth` require one. The public route may only construct
/// `Login`; `Link` and `Reauth` are built by the authenticated link and
/// re-authentication entry points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeContext {
    pub purpose: AuthorizePurpose,
    pub bound_user_id: Option<UserId>,
    pub return_to: Option<String>,
}

impl AuthorizeContext {
    pub fn login(return_to: Option<String>) -> Self {
        Self {
            purpose: AuthorizePurpose::Login,
            bound_user_id: None,
            return_to,
        }
    }

    pub fn link(user_id: UserId, return_to: Option<String>) -> Self {
        Self {
            purpose: AuthorizePurpose::Link,
            bound_user_id: Some(user_id),
            return_to,
        }
    }

    pub fn reauth(user_id: UserId, channel: &str) -> Self {
        Self {
            purpose: AuthorizePurpose::Reauth,
            bound_user_id: Some(user_id),
            return_to: Some(reauth::completion_target(channel)),
        }
    }

    fn validate(&self) -> Result<(), ProviderError> {
        match (self.purpose, self.bound_user_id) {
            (AuthorizePurpose::Login, None) => Ok(()),
            (AuthorizePurpose::Link | AuthorizePurpose::Reauth, Some(_)) => Ok(()),
            _ => Err(ProviderError::Invalid {
                fields: vec!["purpose"],
            }),
        }
    }
}

/// The result of a successful authorization-request creation. `binding` is the
/// raw browser-binding value that is *only* placed in the `palmr_oauth` cookie.
/// It is a `Secret` so no `Debug`/log path can render it.
pub struct Authorized {
    pub authorization_url: String,
    pub binding: Secret<String>,
}

/// The strict public request body. `purpose` may only be `login`; `link` and
/// `reauth` are routed through authenticated endpoints, never here.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizeRequest {
    #[schema(example = "login")]
    pub purpose: String,
    #[schema(example = "/overview")]
    pub return_to: Option<String>,
}

impl JsonRequest for AuthorizeRequest {
    const FIELDS: &'static [JsonField] = &[
        JsonField::required("purpose", JsonKind::String),
        JsonField::optional("returnTo", JsonKind::String),
    ];
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizeResponse {
    pub authorization_url: String,
}

/// The AAD binding the encrypted PKCE verifier to its authorization-request row.
pub fn authorization_request_aad(id: AuthRequestId) -> Vec<u8> {
    let id = id.to_string();
    let mut aad = Vec::with_capacity(b"oauth-request".len() + 1 + id.len());
    aad.extend_from_slice(b"oauth-request");
    aad.push(0);
    aad.extend_from_slice(id.as_bytes());
    aad
}

/// Validate the optional post-authentication relative path, silently falling
/// back to `/overview` for anything unsafe or unknown. The value is never
/// reflected in an error.
pub fn validate_return_to(raw: Option<&str>) -> String {
    raw.filter(|value| is_safe_return_to(value))
        .map_or_else(|| DEFAULT_RETURN_TO.to_owned(), str::to_owned)
}

fn is_safe_return_to(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_RETURN_TO_LEN {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[0] != b'/' || matches!(bytes.get(1), Some(b'/') | Some(b'\\')) {
        return false;
    }
    if value.contains("://") || value.contains('#') || value.contains("..") {
        return false;
    }
    if value.chars().any(char::is_control) {
        return false;
    }
    let path = value.split(['?', '#']).next().unwrap_or(value);
    SPA_PREFIXES.iter().any(|prefix| {
        path == *prefix || path.starts_with(&format!("{prefix}/")) || path.starts_with(prefix)
    })
}

impl super::service::IdentityProviderService {
    pub fn cookie_policy(&self) -> CookiePolicy {
        CookiePolicy::from_base_url(self.base_url())
    }

    /// The public, non-sensitive provider list. When the global provider toggle
    /// is off this is an empty list and never an error.
    pub async fn public_providers(&self) -> Result<PublicProviderList, ProviderError> {
        Ok(PublicProviderList {
            providers: self
                .login_providers()
                .await?
                .iter()
                .map(PublicProvider::new)
                .collect(),
        })
    }

    pub async fn login_providers(&self) -> Result<Vec<IdentityProvider>, ProviderError> {
        if !self.settings().load().security.auth_providers_enabled {
            return Ok(Vec::new());
        }
        let records = repo::list_enabled(self.pools().reader()).await?;
        Ok(records.into_iter().map(|record| record.provider).collect())
    }

    /// Create one durable authorization request and return the authorization URL
    /// and browser-binding value. One short write transaction inserts exactly one
    /// row and performs no outbound HTTP. No audit row is written: starting an
    /// external login is not itself a successful authentication event.
    pub async fn authorize(
        &self,
        slug: &str,
        context: AuthorizeContext,
    ) -> Result<Authorized, ProviderError> {
        context.validate()?;
        if !self.settings().load().security.auth_providers_enabled {
            return Err(ProviderError::Disabled);
        }
        let record = repo::find_by_slug(self.pools().reader(), slug)
            .await?
            .ok_or(ProviderError::NotFound)?;
        let provider = &record.provider;
        if !provider.enabled {
            return Err(ProviderError::Disabled);
        }
        if !usable_authorization_configuration(provider) {
            return Err(ProviderError::Invalid {
                fields: vec!["provider"],
            });
        }
        if let (AuthorizePurpose::Link, Some(user_id)) = (context.purpose, context.bound_user_id) {
            if repo::user_has_link(self.pools().reader(), user_id, provider.id).await? {
                return Err(ProviderError::IdentityAlreadyLinked);
            }
        }

        let redirect_uri = model::redirect_uri(self.base_url().url(), &provider.slug);
        let return_to = match context.purpose {
            AuthorizePurpose::Reauth => context
                .return_to
                .as_deref()
                .filter(|target| reauth::channel_of_target(target).is_some())
                .map(str::to_owned)
                .ok_or(ProviderError::Invalid {
                    fields: vec!["returnTo"],
                })?,
            AuthorizePurpose::Login | AuthorizePurpose::Link => {
                validate_return_to(context.return_to.as_deref())
            }
        };

        let state = Token::mint()?;
        let binding = Token::mint()?;
        let nonce = Token::mint()?;
        let verifier = base64url_no_pad(&random_bytes(PKCE_VERIFIER_BYTES)?);
        let challenge = sha256_base64url(verifier.as_bytes());
        let id = AuthRequestId::generate(self.clock());

        let sealed = self.keys().seal(
            SealPurpose::Oidc,
            &authorization_request_aad(id),
            verifier.as_bytes(),
        )?;

        let now = self.clock().now();
        let created_at = Timestamp::try_from(now)?;
        let expires_at =
            Timestamp::try_from(now + time::Duration::seconds(AUTH_REQUEST_TTL_SECONDS as i64))?;
        let nonce_text = nonce.encode();
        let state_hash = state.digest();
        let binding_hash = binding.digest();

        let write = AuthRequestWrite {
            id,
            provider_id: provider.id,
            state_hash: state_hash.as_str(),
            binding_cookie_hash: binding_hash.as_str(),
            verifier: &sealed,
            nonce: nonce_text.expose_secret().as_str(),
            redirect_uri: &redirect_uri,
            post_auth_path: Some(return_to.as_str()),
            purpose: context.purpose,
            link_user_id: context.bound_user_id,
            created_at,
            expires_at,
        };
        self.pools()
            .write_tx(self.clock(), AUTHORIZE_TRANSACTION, async |tx| {
                repo::insert_auth_request(tx, &write).await
            })
            .await?;

        let state_text = state.encode();
        let authorization_url = build_authorization_url(
            provider,
            &redirect_uri,
            state_text.expose_secret(),
            &challenge,
            nonce_text.expose_secret(),
            context.purpose,
        )?;

        Ok(Authorized {
            authorization_url,
            binding: binding.encode(),
        })
    }
}

fn usable_authorization_configuration(provider: &IdentityProvider) -> bool {
    provider
        .kind
        .endpoints()
        .authorization
        .as_deref()
        .is_some_and(|endpoint| Url::parse(endpoint).is_ok())
        && !provider.client_id.is_empty()
}

fn build_authorization_url(
    provider: &IdentityProvider,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    nonce: &str,
    purpose: AuthorizePurpose,
) -> Result<String, ProviderError> {
    let endpoint = provider
        .kind
        .endpoints()
        .authorization
        .ok_or(ProviderError::Invalid {
            fields: vec!["provider"],
        })?;
    let mut url = Url::parse(&endpoint).map_err(|_| ProviderError::Invalid {
        fields: vec!["provider"],
    })?;

    let preserved: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !RESERVED_AUTHORIZE_PARAMS.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    {
        let mut query = url.query_pairs_mut();
        query.clear();
        for (key, value) in preserved {
            query.append_pair(&key, &value);
        }
        query.append_pair("response_type", "code");
        query.append_pair("client_id", &provider.client_id);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("scope", &provider.scopes.join(" "));
        query.append_pair("state", state);
        query.append_pair("code_challenge", challenge);
        query.append_pair("code_challenge_method", "S256");
        if provider.protocol() == model::Protocol::Oidc {
            query.append_pair("nonce", nonce);
        }
        if purpose == AuthorizePurpose::Reauth {
            query.append_pair("prompt", "login");
            if provider.protocol() == model::Protocol::Oidc {
                query.append_pair("max_age", "0");
            }
        }
    }

    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::{is_safe_return_to, validate_return_to, DEFAULT_RETURN_TO, MAX_RETURN_TO_LEN};
    use crate::config::{EnvironmentSource, OperatorConfig};

    #[test]
    fn unit_default_base_url_redirect_uri_is_localhost() {
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([("PALMR_PORT", "8080")]))
            .unwrap()
            .config;
        assert_eq!(
            crate::features::identity_providers::model::redirect_uri(config.base_url.url(), "corp"),
            "http://localhost:8080/api/v1/auth/providers/corp/callback"
        );
    }

    #[test]
    fn unit_return_to_accepts_known_spa_paths() {
        for accepted in [
            "/overview",
            "/files",
            "/files?foo=bar",
            "/settings/security",
            "/admin/providers",
            "/shared",
            "/received",
            "/transfers",
            "/s/abc",
            "/r/abc",
            "/e/abc",
        ] {
            assert_eq!(validate_return_to(Some(accepted)), accepted, "{accepted}");
        }
    }

    #[test]
    fn unit_return_to_falls_back_for_unsafe_values() {
        for rejected in [
            "//evil.example",
            "/\\evil",
            "https://evil.example",
            "/https://evil.example",
            "/files\u{0000}",
            "/files\r\nLocation: evil",
            "/files\nevil",
            "/unknown",
            "/auth/reauth-complete?channel=AwsTGyMrMztDS1NbY2tze4OLk5ujq7O7w8vT2-Pr8_s",
            "",
            "/",
            "files",
        ] {
            assert_eq!(
                validate_return_to(Some(rejected)),
                DEFAULT_RETURN_TO,
                "{rejected}"
            );
            assert!(!is_safe_return_to(rejected), "{rejected}");
        }
        assert_eq!(validate_return_to(None), DEFAULT_RETURN_TO);
    }

    #[test]
    fn unit_return_to_bounds_length_at_512() {
        let accepted = format!("/files/{}", "a".repeat(MAX_RETURN_TO_LEN - "/files/".len()));
        assert_eq!(accepted.len(), MAX_RETURN_TO_LEN);
        assert!(is_safe_return_to(&accepted));

        let too_long = format!("/files/{}", "a".repeat(MAX_RETURN_TO_LEN));
        assert!(too_long.len() > MAX_RETURN_TO_LEN);
        assert!(!is_safe_return_to(&too_long));
        assert_eq!(validate_return_to(Some(&too_long)), DEFAULT_RETURN_TO);
    }
}

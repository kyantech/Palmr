use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::domain::id::Id;
use crate::domain::time::Timestamp;
use crate::infra::crypto::aead::SealedSecret;

pub const SLUG_MIN_LEN: usize = 2;
pub const SLUG_MAX_LEN: usize = 40;
pub const DISPLAY_NAME_MAX_CHARS: usize = 64;
pub const CLIENT_ID_MAX_CHARS: usize = 512;
pub const CLIENT_SECRET_MAX_BYTES: usize = 4096;
pub const URL_MAX_CHARS: usize = 512;
pub const SCOPES_MAX_CHARS: usize = 512;
pub const CLAIM_MAX_CHARS: usize = 64;
pub const MAX_SORT_ORDER: i64 = 1_000_000;
pub const CALLBACK_PATH_PREFIX: &str = "/api/v1/auth/providers/";
pub const CALLBACK_PATH_SUFFIX: &str = "/callback";
pub const DEFAULT_SCOPES: [&str; 3] = ["openid", "profile", "email"];

pub enum ProviderTag {}

pub type ProviderId = Id<ProviderTag>;

pub enum AuthRequestTag {}

pub type AuthRequestId = Id<AuthRequestTag>;

pub enum IdentityLinkTag {}

pub type IdentityLinkId = Id<IdentityLinkTag>;

/// The purpose of one external authorization request. Only `Login` is exposed by
/// the public route; `Link` and `Reauth` enter through dedicated authenticated
/// flows. The service supports all three through one request row contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AuthorizePurpose {
    #[serde(rename = "login")]
    #[schema(rename = "login")]
    Login,
    #[serde(rename = "link")]
    #[schema(rename = "link")]
    Link,
    #[serde(rename = "reauth")]
    #[schema(rename = "reauth")]
    Reauth,
}

impl AuthorizePurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Link => "link",
            Self::Reauth => "reauth",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "login" => Some(Self::Login),
            "link" => Some(Self::Link),
            "reauth" => Some(Self::Reauth),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum Protocol {
    #[serde(rename = "oidc")]
    #[schema(rename = "oidc")]
    Oidc,
    #[serde(rename = "oauth2")]
    #[schema(rename = "oauth2")]
    OAuth2,
}

impl Protocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Oidc => "oidc",
            Self::OAuth2 => "oauth2",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "oidc" => Some(Self::Oidc),
            "oauth2" => Some(Self::OAuth2),
            _ => None,
        }
    }

    pub const fn default_allow_email_linking(self) -> bool {
        matches!(self, Self::Oidc)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum Preset {
    #[serde(rename = "google")]
    #[schema(rename = "google")]
    Google,
    #[serde(rename = "github")]
    #[schema(rename = "github")]
    Github,
    #[serde(rename = "discord")]
    #[schema(rename = "discord")]
    Discord,
    #[serde(rename = "auth0")]
    #[schema(rename = "auth0")]
    Auth0,
    #[serde(rename = "kinde")]
    #[schema(rename = "kinde")]
    Kinde,
    #[serde(rename = "zitadel")]
    #[schema(rename = "zitadel")]
    Zitadel,
    #[serde(rename = "authentik")]
    #[schema(rename = "authentik")]
    Authentik,
    #[serde(rename = "frontegg")]
    #[schema(rename = "frontegg")]
    Frontegg,
    #[serde(rename = "pocket_id")]
    #[schema(rename = "pocket_id")]
    PocketId,
    #[serde(rename = "generic")]
    #[schema(rename = "generic")]
    Generic,
}

impl Preset {
    pub const ALL: [Self; 10] = [
        Self::Google,
        Self::Github,
        Self::Discord,
        Self::Auth0,
        Self::Kinde,
        Self::Zitadel,
        Self::Authentik,
        Self::Frontegg,
        Self::PocketId,
        Self::Generic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Github => "github",
            Self::Discord => "discord",
            Self::Auth0 => "auth0",
            Self::Kinde => "kinde",
            Self::Zitadel => "zitadel",
            Self::Authentik => "authentik",
            Self::Frontegg => "frontegg",
            Self::PocketId => "pocket_id",
            Self::Generic => "generic",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.as_str() == text)
    }

    pub const fn fixed_protocol(self) -> Option<Protocol> {
        match self {
            Self::Github | Self::Discord => Some(Protocol::OAuth2),
            Self::Generic => None,
            Self::Google
            | Self::Auth0
            | Self::Kinde
            | Self::Zitadel
            | Self::Authentik
            | Self::Frontegg
            | Self::PocketId => Some(Protocol::Oidc),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TokenAuthMethod {
    #[serde(rename = "client_secret_basic")]
    #[schema(rename = "client_secret_basic")]
    ClientSecretBasic,
    #[serde(rename = "client_secret_post")]
    #[schema(rename = "client_secret_post")]
    ClientSecretPost,
    #[serde(rename = "none")]
    #[schema(rename = "none")]
    None,
}

impl TokenAuthMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientSecretBasic => "client_secret_basic",
            Self::ClientSecretPost => "client_secret_post",
            Self::None => "none",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "client_secret_basic" => Some(Self::ClientSecretBasic),
            "client_secret_post" => Some(Self::ClientSecretPost),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    #[schema(required = true, example = "https://sso.example.com/authorize")]
    pub authorization: Option<String>,
    #[schema(required = true, example = "https://sso.example.com/token")]
    pub token: Option<String>,
    #[schema(required = true, example = "https://sso.example.com/userinfo")]
    pub userinfo: Option<String>,
    #[schema(required = true, example = "https://sso.example.com/jwks")]
    pub jwks: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimMapping {
    #[schema(example = "sub")]
    pub subject: String,
    #[schema(example = "email")]
    pub email: String,
    #[schema(example = "email_verified")]
    pub email_verified: String,
    #[schema(example = "preferred_username")]
    pub username: String,
    #[schema(example = "name")]
    pub name: String,
    #[schema(example = "picture")]
    pub picture: String,
}

impl ClaimMapping {
    pub fn standard() -> Self {
        Self {
            subject: "sub".to_owned(),
            email: "email".to_owned(),
            email_verified: "email_verified".to_owned(),
            username: "preferred_username".to_owned(),
            name: "name".to_owned(),
            picture: "picture".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcProvider {
    pub issuer: String,
    pub discovery_url: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: Option<String>,
    pub jwks_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuth2Provider {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderVariant {
    Oidc(OidcProvider),
    OAuth2(OAuth2Provider),
}

impl ProviderVariant {
    pub const fn protocol(&self) -> Protocol {
        match self {
            Self::Oidc(_) => Protocol::Oidc,
            Self::OAuth2(_) => Protocol::OAuth2,
        }
    }

    pub fn issuer(&self) -> Option<&str> {
        match self {
            Self::Oidc(oidc) => Some(&oidc.issuer),
            Self::OAuth2(_) => None,
        }
    }

    pub fn endpoints(&self) -> Endpoints {
        match self {
            Self::Oidc(oidc) => Endpoints {
                authorization: Some(oidc.authorization_endpoint.clone()),
                token: Some(oidc.token_endpoint.clone()),
                userinfo: oidc.userinfo_endpoint.clone(),
                jwks: Some(oidc.jwks_uri.clone()),
            },
            Self::OAuth2(oauth2) => Endpoints {
                authorization: Some(oauth2.authorization_endpoint.clone()),
                token: Some(oauth2.token_endpoint.clone()),
                userinfo: Some(oauth2.userinfo_endpoint.clone()),
                jwks: None,
            },
        }
    }

    pub fn discovery_url(&self) -> Option<&str> {
        match self {
            Self::Oidc(oidc) => Some(&oidc.discovery_url),
            Self::OAuth2(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct IdentityProvider {
    pub id: ProviderId,
    pub slug: String,
    pub display_name: String,
    pub preset: Preset,
    pub enabled: bool,
    pub sort_order: i64,
    pub auto_provision: bool,
    pub allow_email_linking: bool,
    pub client_id: String,
    pub client_secret: Option<SealedSecret>,
    pub scopes: Vec<String>,
    pub token_auth_method: TokenAuthMethod,
    pub claims: ClaimMapping,
    pub kind: ProviderVariant,
    pub validated_at: Option<Timestamp>,
    pub validation_error: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl IdentityProvider {
    pub const fn protocol(&self) -> Protocol {
        self.kind.protocol()
    }

    /// A non-sensitive icon discriminator for the public provider list. Derived
    /// only from the persisted preset, so it exposes no provider configuration.
    pub const fn icon_key(&self) -> &'static str {
        self.preset.as_str()
    }
}

#[derive(Debug)]
pub struct ProviderRecord {
    pub provider: IdentityProvider,
    pub linked_user_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderItem {
    #[schema(example = "0192fc3a-5c4b-7e21-9a02-3f8c1d6e4b90")]
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub protocol: Protocol,
    pub preset: Preset,
    pub enabled: bool,
    pub sort_order: i64,
    pub auto_provision: bool,
    pub allow_email_linking: bool,
    #[schema(required = true)]
    pub issuer_url: Option<String>,
    pub client_id: String,
    /// Whether a client secret is stored. The secret itself is write-only and never returned.
    pub client_secret_configured: bool,
    pub scopes: Vec<String>,
    pub token_auth_method: TokenAuthMethod,
    pub endpoints: Endpoints,
    pub claim_mapping: ClaimMapping,
    /// Set by a successful `POST /api/v1/admin/providers/{id}/test` and cleared by any connection-critical change.
    #[schema(required = true)]
    pub validated_at: Option<String>,
    /// A sanitized list of the failing checks of the last test, `null` when there is none.
    #[schema(required = true)]
    pub validation_error: Option<String>,
    /// Number of users with an identity link to this provider.
    pub linked_user_count: u64,
    /// Derived from `PALMR_BASE_URL`; register exactly this value at the identity provider.
    #[schema(read_only)]
    pub redirect_uri: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ProviderItem {
    pub fn new(record: &ProviderRecord, base_url: &Url) -> Self {
        let provider = &record.provider;
        Self {
            id: provider.id.to_string(),
            slug: provider.slug.clone(),
            display_name: provider.display_name.clone(),
            protocol: provider.protocol(),
            preset: provider.preset,
            enabled: provider.enabled,
            sort_order: provider.sort_order,
            auto_provision: provider.auto_provision,
            allow_email_linking: provider.allow_email_linking,
            issuer_url: provider.kind.issuer().map(str::to_owned),
            client_id: provider.client_id.clone(),
            client_secret_configured: provider.client_secret.is_some(),
            scopes: provider.scopes.clone(),
            token_auth_method: provider.token_auth_method,
            endpoints: provider.kind.endpoints(),
            claim_mapping: provider.claims.clone(),
            validated_at: provider.validated_at.map(|at| at.to_string()),
            validation_error: provider.validation_error.clone(),
            linked_user_count: record.linked_user_count,
            redirect_uri: redirect_uri(base_url, &provider.slug),
            created_at: provider.created_at.to_string(),
            updated_at: provider.updated_at.to_string(),
        }
    }
}

/// The public, non-sensitive view of an enabled provider. It deliberately has
/// no issuer, client id, endpoint, scope, claim mapping, secret state or
/// validation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicProvider {
    pub slug: String,
    pub display_name: String,
    pub icon_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicProviderList {
    pub providers: Vec<PublicProvider>,
}

impl PublicProvider {
    pub fn new(provider: &IdentityProvider) -> Self {
        Self {
            slug: provider.slug.clone(),
            display_name: provider.display_name.clone(),
            icon_key: provider.icon_key().to_owned(),
        }
    }
}

pub fn is_valid_slug(slug: &str) -> bool {
    (SLUG_MIN_LEN..=SLUG_MAX_LEN).contains(&slug.len())
        && slug.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

pub fn redirect_uri(base_url: &Url, slug: &str) -> String {
    format!(
        "{}{CALLBACK_PATH_PREFIX}{slug}{CALLBACK_PATH_SUFFIX}",
        base_url.as_str().trim_end_matches('/')
    )
}

pub fn client_secret_aad(id: ProviderId) -> Vec<u8> {
    let id = id.to_string();
    let mut aad = Vec::with_capacity(4 + id.len());
    aad.extend_from_slice(b"idp");
    aad.push(0);
    aad.extend_from_slice(id.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_provider_slug_rules() {
        for accepted in ["ab", "google", "my-sso_2", &"a".repeat(40)] {
            assert!(is_valid_slug(accepted), "{accepted}");
        }
        for rejected in [
            "",
            "a",
            "Google",
            "my sso",
            "sso.example",
            "é1",
            &"a".repeat(41),
        ] {
            assert!(!is_valid_slug(rejected), "{rejected}");
        }
    }

    #[test]
    fn unit_provider_protocol_defaults_email_linking() {
        assert!(Protocol::Oidc.default_allow_email_linking());
        assert!(!Protocol::OAuth2.default_allow_email_linking());
    }

    #[test]
    fn unit_provider_enum_wire_names_round_trip() {
        for preset in Preset::ALL {
            assert_eq!(Preset::parse(preset.as_str()), Some(preset));
            assert_eq!(
                serde_json::to_value(preset).unwrap(),
                serde_json::Value::from(preset.as_str())
            );
        }
        for method in [
            TokenAuthMethod::ClientSecretBasic,
            TokenAuthMethod::ClientSecretPost,
            TokenAuthMethod::None,
        ] {
            assert_eq!(TokenAuthMethod::parse(method.as_str()), Some(method));
            assert_eq!(
                serde_json::to_value(method).unwrap(),
                serde_json::Value::from(method.as_str())
            );
        }
        assert_eq!(Protocol::parse("oauth2"), Some(Protocol::OAuth2));
        assert_eq!(Protocol::parse("OIDC"), None);
        assert_eq!(Preset::parse("pocketid"), None);
    }

    #[test]
    fn unit_provider_redirect_uri_is_derived_from_the_base_url() {
        let base = Url::parse("https://palmr.example.com").unwrap();
        assert_eq!(
            redirect_uri(&base, "google"),
            "https://palmr.example.com/api/v1/auth/providers/google/callback"
        );
        let sub = Url::parse("https://example.com/palmr/").unwrap();
        assert_eq!(
            redirect_uri(&sub, "a-b"),
            "https://example.com/palmr/api/v1/auth/providers/a-b/callback"
        );
    }
}

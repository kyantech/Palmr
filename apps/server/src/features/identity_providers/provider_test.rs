use serde::Serialize;
use serde_json::Value;
use url::Url;
use utoipa::ToSchema;

use super::discovery::{discover, DiscoveryFailure};
use super::http_client::{FetchFailure, ProviderHttpClient};
use super::model::{
    redirect_uri, IdentityProvider, OAuth2Provider, OidcProvider, ProviderVariant,
    CALLBACK_PATH_PREFIX, CALLBACK_PATH_SUFFIX,
};
use crate::infra::http::error::CheckDetail;

pub const MAX_VALIDATION_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResult {
    pub ok: bool,
    pub checks: Vec<CheckDetail>,
    #[schema(example = "2026-09-22T14:20:00.000Z")]
    pub validated_at: String,
}

fn check(name: &'static str, outcome: Result<(), &'static str>) -> CheckDetail {
    CheckDetail {
        name,
        ok: outcome.is_ok(),
        detail: outcome.err(),
    }
}

pub async fn run(
    client: &ProviderHttpClient,
    provider: &IdentityProvider,
    base_url: &Url,
) -> Vec<CheckDetail> {
    let redirect = check(
        "redirect_uri",
        redirect_is_well_formed(base_url, &provider.slug),
    );
    let mut checks = match &provider.kind {
        ProviderVariant::Oidc(oidc) => oidc_checks(client, oidc).await,
        ProviderVariant::OAuth2(oauth2) => oauth2_checks(client, oauth2).await,
    };
    checks.push(redirect);
    checks
}

async fn oidc_checks(client: &ProviderHttpClient, oidc: &OidcProvider) -> Vec<CheckDetail> {
    let (discovery, jwks, token) = tokio::join!(
        discovery_check(client, &oidc.issuer),
        jwks_check(client, &oidc.jwks_uri),
        reachable(client, &oidc.token_endpoint),
    );
    vec![
        check("discovery", discovery),
        check("jwks", jwks),
        check("token_endpoint", token),
    ]
}

async fn oauth2_checks(client: &ProviderHttpClient, oauth2: &OAuth2Provider) -> Vec<CheckDetail> {
    let (authorization, token, userinfo) = tokio::join!(
        reachable(client, &oauth2.authorization_endpoint),
        reachable(client, &oauth2.token_endpoint),
        reachable(client, &oauth2.userinfo_endpoint),
    );
    vec![
        check("authorization_endpoint", authorization),
        check("token_endpoint", token),
        check("userinfo_endpoint", userinfo),
    ]
}

async fn discovery_check(client: &ProviderHttpClient, issuer: &str) -> Result<(), &'static str> {
    discover(client, issuer)
        .await
        .map(|_| ())
        .map_err(DiscoveryFailure::code)
}

async fn jwks_check(client: &ProviderHttpClient, jwks_uri: &str) -> Result<(), &'static str> {
    let document = client
        .document(jwks_uri)
        .await
        .map_err(FetchFailure::code)?;
    match serde_json::from_slice::<Value>(&document.body) {
        Ok(Value::Object(set)) => match set.get("keys").and_then(Value::as_array) {
            Some(keys) if !keys.is_empty() => Ok(()),
            Some(_) => Err("empty_key_set"),
            None => Err("malformed_document"),
        },
        _ => Err("malformed_document"),
    }
}

async fn reachable(client: &ProviderHttpClient, endpoint: &str) -> Result<(), &'static str> {
    let status = client.probe(endpoint).await.map_err(FetchFailure::code)?;
    if status.is_server_error() {
        Err(FetchFailure::Status(status.as_u16()).code())
    } else {
        Ok(())
    }
}

fn redirect_is_well_formed(base_url: &Url, slug: &str) -> Result<(), &'static str> {
    let derived = redirect_uri(base_url, slug);
    let parsed = Url::parse(&derived).map_err(|_| "malformed_redirect_uri")?;
    let expected_path = format!("{CALLBACK_PATH_PREFIX}{slug}{CALLBACK_PATH_SUFFIX}");
    let well_formed = matches!(parsed.scheme(), "http" | "https")
        && parsed.host().is_some()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.path().ends_with(&expected_path);
    if well_formed {
        Ok(())
    } else {
        Err("malformed_redirect_uri")
    }
}

pub fn summarize_failures(checks: &[CheckDetail]) -> Option<String> {
    let failing: Vec<String> = checks
        .iter()
        .filter(|check| !check.ok)
        .map(|check| format!("{}:{}", check.name, check.detail.unwrap_or("failed")))
        .collect();
    if failing.is_empty() {
        return None;
    }
    let mut summary = failing.join(",");
    summary.truncate(MAX_VALIDATION_ERROR_CHARS);
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_provider_redirect_check_accepts_a_derived_uri() {
        let https = Url::parse("https://palmr.example.com/").unwrap();
        assert_eq!(redirect_is_well_formed(&https, "google"), Ok(()));
        let sub = Url::parse("https://example.com/palmr").unwrap();
        assert_eq!(redirect_is_well_formed(&sub, "a-b"), Ok(()));
        let loopback = Url::parse("http://localhost:5487").unwrap();
        assert_eq!(redirect_is_well_formed(&loopback, "dev"), Ok(()));
    }

    #[test]
    fn unit_provider_summary_lists_only_failures_from_static_codes() {
        let checks = vec![
            CheckDetail {
                name: "discovery",
                ok: true,
                detail: None,
            },
            CheckDetail {
                name: "jwks",
                ok: false,
                detail: Some("unreachable"),
            },
            CheckDetail {
                name: "token_endpoint",
                ok: false,
                detail: Some("tls_error"),
            },
        ];
        assert_eq!(
            summarize_failures(&checks).as_deref(),
            Some("jwks:unreachable,token_endpoint:tls_error")
        );
        assert_eq!(summarize_failures(&checks[..1]), None);
    }
}

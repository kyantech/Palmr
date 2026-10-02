use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::http_client::{acceptable_url, FetchFailure, ProviderHttpClient};
use super::model::{Endpoints, URL_MAX_CHARS};

pub const WELL_KNOWN_PATH: &str = "/.well-known/openid-configuration";
const MAX_LISTED_VALUES: usize = 64;
const MAX_LISTED_VALUE_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryFailure {
    InvalidIssuer,
    Fetch(FetchFailure),
    Malformed,
    IssuerMismatch,
    IncompleteDocument,
}

impl DiscoveryFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidIssuer => "invalid_issuer",
            Self::Fetch(failure) => failure.code(),
            Self::Malformed => "malformed_document",
            Self::IssuerMismatch => "issuer_mismatch",
            Self::IncompleteDocument => "incomplete_document",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Discovered {
    /// The `issuer` of the discovery document; it always equals the requested issuer exactly.
    #[schema(example = "https://sso.example.com/application/o/palmr/")]
    pub issuer_url: String,
    pub endpoints: Endpoints,
    /// `scopes_supported`, bounded; empty when the document omits it.
    pub scopes_supported: Vec<String>,
    /// `token_endpoint_auth_methods_supported`, bounded; empty when the document omits it.
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

pub fn acceptable_issuer(text: &str) -> Option<String> {
    let url = acceptable_url(text)?;
    (url.query().is_none() && text.len() <= URL_MAX_CHARS).then(|| text.to_owned())
}

pub fn discovery_url(issuer: &str) -> String {
    let base = issuer.strip_suffix('/').unwrap_or(issuer);
    format!("{base}{WELL_KNOWN_PATH}")
}

pub async fn discover(
    client: &ProviderHttpClient,
    issuer: &str,
) -> Result<Discovered, DiscoveryFailure> {
    let issuer = acceptable_issuer(issuer).ok_or(DiscoveryFailure::InvalidIssuer)?;
    let document = client
        .document(&discovery_url(&issuer))
        .await
        .map_err(DiscoveryFailure::Fetch)?;
    parse(&issuer, &document.body)
}

pub fn parse(issuer: &str, body: &[u8]) -> Result<Discovered, DiscoveryFailure> {
    let Ok(Value::Object(document)) = serde_json::from_slice::<Value>(body) else {
        return Err(DiscoveryFailure::Malformed);
    };
    let text = |key: &str| document.get(key).and_then(Value::as_str);
    if text("issuer") != Some(issuer) {
        return Err(if text("issuer").is_some() {
            DiscoveryFailure::IssuerMismatch
        } else {
            DiscoveryFailure::Malformed
        });
    }
    let endpoint = |key: &str| -> Result<Option<String>, DiscoveryFailure> {
        match document.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => acceptable_url(value)
                .map(|_| Some(value.clone()))
                .ok_or(DiscoveryFailure::Malformed),
            Some(_) => Err(DiscoveryFailure::Malformed),
        }
    };
    let authorization = endpoint("authorization_endpoint")?;
    let token = endpoint("token_endpoint")?;
    let jwks = endpoint("jwks_uri")?;
    let userinfo = endpoint("userinfo_endpoint")?;
    if authorization.is_none() || token.is_none() || jwks.is_none() {
        return Err(DiscoveryFailure::IncompleteDocument);
    }
    Ok(Discovered {
        issuer_url: issuer.to_owned(),
        endpoints: Endpoints {
            authorization,
            token,
            userinfo,
            jwks,
        },
        scopes_supported: listed(document.get("scopes_supported")),
        token_endpoint_auth_methods_supported: listed(
            document.get("token_endpoint_auth_methods_supported"),
        ),
    })
}

fn listed(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|item| {
            !item.is_empty()
                && item.len() <= MAX_LISTED_VALUE_CHARS
                && item.bytes().all(|byte| byte.is_ascii_graphic())
        })
        .take(MAX_LISTED_VALUES)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ISSUER: &str = "https://sso.example.com/application/o/palmr/";

    fn document() -> Value {
        json!({
            "issuer": ISSUER,
            "authorization_endpoint": "https://sso.example.com/authorize",
            "token_endpoint": "https://sso.example.com/token",
            "userinfo_endpoint": "https://sso.example.com/userinfo",
            "jwks_uri": "https://sso.example.com/jwks",
            "scopes_supported": ["openid", "email", "profile", "bad scope", ""],
            "token_endpoint_auth_methods_supported": ["client_secret_basic"],
            "ignored": {"anything": true},
        })
    }

    fn parsed(document: &Value) -> Result<Discovered, DiscoveryFailure> {
        parse(ISSUER, document.to_string().as_bytes())
    }

    #[test]
    fn unit_discovery_url_is_only_the_openid_configuration_path() {
        assert_eq!(
            discovery_url("https://accounts.google.com"),
            "https://accounts.google.com/.well-known/openid-configuration"
        );
        assert_eq!(
            discovery_url(ISSUER),
            "https://sso.example.com/application/o/palmr/.well-known/openid-configuration"
        );
        assert_eq!(
            discovery_url("https://sso.example.com/realms/main"),
            "https://sso.example.com/realms/main/.well-known/openid-configuration"
        );
        assert!(!discovery_url(ISSUER).contains("openid_configuration"));
    }

    #[test]
    fn unit_discovery_accepts_a_complete_document() {
        let discovered = parsed(&document()).unwrap();
        assert_eq!(discovered.issuer_url, ISSUER);
        assert_eq!(
            discovered.endpoints,
            Endpoints {
                authorization: Some("https://sso.example.com/authorize".to_owned()),
                token: Some("https://sso.example.com/token".to_owned()),
                userinfo: Some("https://sso.example.com/userinfo".to_owned()),
                jwks: Some("https://sso.example.com/jwks".to_owned()),
            }
        );
        assert_eq!(discovered.scopes_supported, ["openid", "email", "profile"]);
        assert_eq!(
            discovered.token_endpoint_auth_methods_supported,
            ["client_secret_basic"]
        );
    }

    #[test]
    fn unit_discovery_issuer_must_equal_the_configured_issuer_exactly() {
        for variant in [
            "https://sso.example.com/application/o/palmr",
            "https://SSO.example.com/application/o/palmr/",
            "http://sso.example.com/application/o/palmr/",
            "https://sso.example.com/application/o/palmr/ ",
            "https://evil.example.com/application/o/palmr/",
        ] {
            let mut doc = document();
            doc["issuer"] = json!(variant);
            assert_eq!(
                parsed(&doc),
                Err(DiscoveryFailure::IssuerMismatch),
                "{variant}"
            );
        }
        let mut missing = document();
        missing.as_object_mut().unwrap().remove("issuer");
        assert_eq!(parsed(&missing), Err(DiscoveryFailure::Malformed));
        let mut not_text = document();
        not_text["issuer"] = json!(7);
        assert_eq!(parsed(&not_text), Err(DiscoveryFailure::Malformed));
    }

    #[test]
    fn unit_discovery_requires_the_login_endpoints() {
        for key in ["authorization_endpoint", "token_endpoint", "jwks_uri"] {
            let mut doc = document();
            doc.as_object_mut().unwrap().remove(key);
            assert_eq!(
                parsed(&doc),
                Err(DiscoveryFailure::IncompleteDocument),
                "{key}"
            );
        }
        let mut doc = document();
        doc.as_object_mut().unwrap().remove("userinfo_endpoint");
        assert_eq!(parsed(&doc).unwrap().endpoints.userinfo, None);
    }

    #[test]
    fn unit_discovery_rejects_unsafe_or_malformed_endpoint_values() {
        for bad in [
            json!("http://sso.example.com/token"),
            json!("javascript:alert(1)"),
            json!("https://user:pw@sso.example.com/token"),
            json!(7),
            json!(["https://sso.example.com/token"]),
        ] {
            let mut doc = document();
            doc["token_endpoint"] = bad.clone();
            assert_eq!(parsed(&doc), Err(DiscoveryFailure::Malformed), "{bad}");
        }
        for body in [
            &b""[..],
            b"null",
            b"[]",
            b"\"text\"",
            b"{",
            b"<html></html>",
        ] {
            assert_eq!(parse(ISSUER, body), Err(DiscoveryFailure::Malformed));
        }
    }

    #[test]
    fn unit_discovery_issuer_validation_rejects_unsafe_issuers() {
        assert!(acceptable_issuer(ISSUER).is_some());
        for rejected in [
            "",
            "http://sso.example.com/",
            "https://sso.example.com/?x=1",
            "https://sso.example.com/#a",
            "https://u:p@sso.example.com/",
            "sso.example.com",
        ] {
            assert!(acceptable_issuer(rejected).is_none(), "{rejected:?}");
        }
    }
}

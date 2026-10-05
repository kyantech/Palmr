use serde_json::Value;

use super::claims::email_verified;
use super::http_client::{FetchFailure, ProviderHttpClient};
use super::model::ClaimMapping;
use crate::domain::error_code::ErrorCode;

/// The hard cap the identity-provider HTTP client enforces while reading a
/// userinfo response; it is not trusted from `Content-Length`.
pub const USERINFO_SIZE_CAP_BYTES: usize = 256 * 1024;

/// The generic, bounded profile produced from a provider's userinfo document
/// using the provider's configured claim mapping. It never decides account
/// linking; M12-T04 owns identity resolution.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExternalProfile {
    pub subject: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserinfoError {
    /// Network, TLS, status or URL failure while contacting the endpoint.
    Fetch(FetchFailure),
    /// The response exceeded the hard 256 KiB cap.
    TooLarge,
    /// The body was not JSON.
    Malformed,
    /// The body was empty.
    Empty,
    /// The JSON was valid but not an object.
    NotObject,
}

impl UserinfoError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Fetch(failure) => failure.code(),
            Self::TooLarge => "response_too_large",
            Self::Malformed => "malformed_userinfo",
            Self::Empty => "empty_userinfo",
            Self::NotObject => "unexpected_userinfo_shape",
        }
    }

    /// Every userinfo failure classifies to the documented retryable upstream
    /// provider error; the callback layer owns the actual HTTP response.
    pub const fn api_code(&self) -> ErrorCode {
        ErrorCode::ProviderUserinfoFailed
    }
}

impl std::fmt::Display for UserinfoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fetch(failure) => write!(f, "userinfo fetch failed: {}", failure.code()),
            Self::TooLarge => f.write_str("the userinfo response exceeded the 256 KiB cap"),
            Self::Malformed => f.write_str("the userinfo response is not JSON"),
            Self::Empty => f.write_str("the userinfo response is empty"),
            Self::NotObject => f.write_str("the userinfo response is not a JSON object"),
        }
    }
}

impl std::error::Error for UserinfoError {}

/// Fetches `GET <userinfo_endpoint>` with `Authorization: Bearer <token>`
/// using the shared identity-provider HTTP client: verified TLS, a ten second
/// total timeout, no cookies, no caller headers, no token logging and a hard
/// 256 KiB body cap enforced while reading.
pub async fn fetch_userinfo(
    client: &ProviderHttpClient,
    endpoint: &str,
    access_token: &str,
) -> Result<Value, UserinfoError> {
    let value = fetch_json(client, endpoint, access_token).await?;
    if !value.is_object() {
        return Err(UserinfoError::NotObject);
    }
    Ok(value)
}

/// Same transport guarantees as [`fetch_userinfo`], for documents that are not
/// a JSON object (a provider's secondary e-mail list is an array).
pub async fn fetch_json(
    client: &ProviderHttpClient,
    endpoint: &str,
    access_token: &str,
) -> Result<Value, UserinfoError> {
    if access_token.is_empty() {
        return Err(UserinfoError::Fetch(FetchFailure::InvalidUrl));
    }
    let document = client
        .bearer_document(endpoint, access_token)
        .await
        .map_err(|failure| match failure {
            FetchFailure::TooLarge => UserinfoError::TooLarge,
            other => UserinfoError::Fetch(other),
        })?;
    if document.body.is_empty() {
        return Err(UserinfoError::Empty);
    }
    serde_json::from_slice(document.body.as_ref()).map_err(|_| UserinfoError::Malformed)
}

/// Extracts the mapped profile claims from a userinfo (or ID-token) document
/// without interpreting them. Only a boolean `true` or the string `"true"`
/// counts as a verified e-mail.
pub fn profile_from_claims(claims: &Value, mapping: &ClaimMapping) -> ExternalProfile {
    let text = |key: &str| {
        claims
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    ExternalProfile {
        // Providers such as GitHub publish a numeric `id`; the stable subject
        // is preserved as its decimal string.
        subject: match claims.get(&mapping.subject) {
            Some(Value::Number(number)) => Some(number.to_string()),
            _ => text(&mapping.subject),
        },
        email: text(&mapping.email),
        email_verified: email_verified(claims.get(&mapping.email_verified)),
        username: text(&mapping.username),
        name: text(&mapping.name),
        picture: text(&mapping.picture),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn unit_userinfo_profile_maps_standard_claims() {
        let mapping = ClaimMapping::standard();
        let profile = profile_from_claims(
            &json!({
                "sub": "u-1",
                "email": "a@example.com",
                "email_verified": true,
                "preferred_username": "alice",
                "name": "Alice",
                "picture": "https://example.com/a.png"
            }),
            &mapping,
        );
        assert_eq!(
            profile,
            ExternalProfile {
                subject: Some("u-1".to_owned()),
                email: Some("a@example.com".to_owned()),
                email_verified: true,
                username: Some("alice".to_owned()),
                name: Some("Alice".to_owned()),
                picture: Some("https://example.com/a.png".to_owned()),
            }
        );
    }

    #[test]
    fn unit_userinfo_profile_is_conservative() {
        let mapping = ClaimMapping::standard();
        let profile = profile_from_claims(
            &json!({"sub": "u-1", "email": "", "email_verified": "yes"}),
            &mapping,
        );
        assert_eq!(profile.subject.as_deref(), Some("u-1"));
        assert_eq!(profile.email, None);
        assert!(!profile.email_verified);
        assert_eq!(profile.picture, None);
    }

    #[test]
    fn unit_userinfo_profile_preserves_a_numeric_subject() {
        let mapping = ClaimMapping {
            subject: "id".to_owned(),
            ..ClaimMapping::standard()
        };
        let profile = profile_from_claims(&json!({"id": 4242}), &mapping);
        assert_eq!(profile.subject.as_deref(), Some("4242"));
    }

    #[test]
    fn unit_userinfo_error_classification_is_stable() {
        for error in [
            UserinfoError::TooLarge,
            UserinfoError::Malformed,
            UserinfoError::Empty,
            UserinfoError::NotObject,
            UserinfoError::Fetch(FetchFailure::Timeout),
            UserinfoError::Fetch(FetchFailure::Status(503)),
        ] {
            assert_eq!(
                error.api_code(),
                ErrorCode::ProviderUserinfoFailed,
                "{error:?}"
            );
        }
        assert_eq!(UserinfoError::Malformed.code(), "malformed_userinfo");
    }
}

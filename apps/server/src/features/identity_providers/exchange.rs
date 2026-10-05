use serde_json::Value;

use super::http_client::{FormPost, ProviderHttpClient};
use super::model::TokenAuthMethod;
use crate::domain::secret::Secret;

pub struct ExchangeRequest<'a> {
    pub endpoint: &'a str,
    pub client_id: &'a str,
    pub client_secret: Option<&'a Secret<String>>,
    pub method: TokenAuthMethod,
    pub code: &'a Secret<String>,
    pub redirect_uri: &'a str,
    pub verifier: &'a Secret<String>,
}

pub struct TokenResponse {
    pub access_token: Option<Secret<String>>,
    pub id_token: Option<Secret<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExchangeFailure {
    MissingClientSecret,
    Transport,
    Rejected,
    Malformed,
    UnsupportedTokenType,
}

impl ExchangeFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::MissingClientSecret => "missing_client_secret",
            Self::Transport => "token_endpoint_unreachable",
            Self::Rejected => "token_endpoint_rejected",
            Self::Malformed => "token_response_malformed",
            Self::UnsupportedTokenType => "token_type_unsupported",
        }
    }
}

pub async fn exchange_code(
    client: &ProviderHttpClient,
    request: &ExchangeRequest<'_>,
) -> Result<TokenResponse, ExchangeFailure> {
    let secret = match (request.method, request.client_secret) {
        (TokenAuthMethod::None, _) => None,
        (_, Some(secret)) => Some(secret.expose_secret().as_str()),
        (_, None) => return Err(ExchangeFailure::MissingClientSecret),
    };

    let mut fields: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", request.code.expose_secret()),
        ("redirect_uri", request.redirect_uri),
        ("code_verifier", request.verifier.expose_secret()),
    ];
    let mut basic = None;
    match (request.method, secret) {
        (TokenAuthMethod::ClientSecretBasic, Some(secret)) => {
            basic = Some((request.client_id, secret));
        }
        (TokenAuthMethod::ClientSecretPost, Some(secret)) => {
            fields.push(("client_id", request.client_id));
            fields.push(("client_secret", secret));
        }
        _ => fields.push(("client_id", request.client_id)),
    }

    let document = client
        .post_form(
            request.endpoint,
            &FormPost {
                fields: &fields,
                basic,
            },
        )
        .await
        .map_err(|failure| match failure {
            super::http_client::FetchFailure::Status(_) => ExchangeFailure::Rejected,
            _ => ExchangeFailure::Transport,
        })?;
    parse_response(document.body.as_ref())
}

pub fn parse_response(body: &[u8]) -> Result<TokenResponse, ExchangeFailure> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ExchangeFailure::Malformed)?;
    let Some(object) = value.as_object() else {
        return Err(ExchangeFailure::Malformed);
    };
    if object.contains_key("error") {
        return Err(ExchangeFailure::Rejected);
    }
    match object.get("token_type") {
        None | Some(Value::Null) => {}
        Some(Value::String(kind)) if kind.eq_ignore_ascii_case("bearer") => {}
        Some(_) => return Err(ExchangeFailure::UnsupportedTokenType),
    }
    let token = |name: &str| -> Result<Option<Secret<String>>, ExchangeFailure> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if !text.is_empty() => Ok(Some(Secret::new(text.clone()))),
            Some(_) => Err(ExchangeFailure::Malformed),
        }
    };
    Ok(TokenResponse {
        access_token: token("access_token")?,
        id_token: token("id_token")?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(value: &Value) -> Result<TokenResponse, ExchangeFailure> {
        parse_response(value.to_string().as_bytes())
    }

    #[test]
    fn unit_token_response_parses_the_protocol_fields_only() {
        let parsed = parse(&json!({
            "access_token": "at",
            "token_type": "Bearer",
            "id_token": "it",
            "refresh_token": "rt",
            "expires_in": 3600,
            "scope": "openid"
        }))
        .unwrap();
        assert_eq!(parsed.access_token.unwrap().expose_secret(), "at");
        assert_eq!(parsed.id_token.unwrap().expose_secret(), "it");

        let bare = parse(&json!({"access_token": "at"})).unwrap();
        assert!(bare.id_token.is_none());
        let lowercase = parse(&json!({"access_token": "at", "token_type": "bearer"})).unwrap();
        assert!(lowercase.access_token.is_some());
    }

    #[test]
    fn unit_token_response_rejects_errors_and_unusable_shapes() {
        assert_eq!(
            parse(&json!({"error": "bad_verification_code"})).err(),
            Some(ExchangeFailure::Rejected)
        );
        assert_eq!(
            parse(&json!({"access_token": "at", "error": "x"})).err(),
            Some(ExchangeFailure::Rejected)
        );
        assert_eq!(
            parse(&json!({"access_token": "at", "token_type": "mac"})).err(),
            Some(ExchangeFailure::UnsupportedTokenType)
        );
        assert_eq!(
            parse(&json!({"access_token": 5})).err(),
            Some(ExchangeFailure::Malformed)
        );
        assert_eq!(
            parse(&json!(["at"])).err(),
            Some(ExchangeFailure::Malformed)
        );
        assert_eq!(
            parse_response(b"access_token=at&token_type=bearer").err(),
            Some(ExchangeFailure::Malformed)
        );
        assert_eq!(parse_response(b"").err(), Some(ExchangeFailure::Malformed));
        assert_eq!(
            parse(&json!({"access_token": ""})).err(),
            Some(ExchangeFailure::Malformed)
        );
    }
}

use std::collections::BTreeMap;

use axum::response::{IntoResponse, Response};
use http::header::CONTENT_TYPE;
use http::{HeaderValue, StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::error_code::ErrorCode;

pub const MAX_DETAIL_ENTRIES: usize = 8;

pub(crate) const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

// Keys and text values are `&'static str` so runtime strings — source error
// messages, user input, paths, credentials — cannot reach `details`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum DetailValue {
    Bool(bool),
    Integer(i64),
    Text(&'static str),
}

impl From<bool> for DetailValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for DetailValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<&'static str> for DetailValue {
    fn from(value: &'static str) -> Self {
        Self::Text(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ErrorDetails(BTreeMap<&'static str, DetailValue>);

impl ErrorDetails {
    fn insert(&mut self, key: &'static str, value: DetailValue) {
        if self.0.len() < MAX_DETAIL_ENTRIES || self.0.contains_key(key) {
            self.0.insert(key, value);
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    code: ErrorCode,
    message: &'static str,
    details: ErrorDetails,
    request_id: Option<String>,
}

impl ApiError {
    pub const fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.default_message(),
            details: ErrorDetails(BTreeMap::new()),
            request_id: None,
        }
    }

    pub const fn internal() -> Self {
        Self::new(ErrorCode::InternalError)
    }

    #[must_use]
    pub const fn with_message(mut self, message: &'static str) -> Self {
        self.message = message;
        self
    }

    #[must_use]
    pub fn with_detail(mut self, key: &'static str, value: impl Into<DetailValue>) -> Self {
        self.details.insert(key, value.into());
        self
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    pub const fn status(&self) -> StatusCode {
        self.code.status()
    }

    pub const fn retryable(&self) -> bool {
        self.code.retryable()
    }

    pub const fn details(&self) -> &ErrorDetails {
        &self.details
    }

    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }
}

// `anyhow` carries arbitrary diagnostic text; none of it may reach a client.
impl From<anyhow::Error> for ApiError {
    fn from(_source: anyhow::Error) -> Self {
        Self::internal()
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiErrorBody {
    error: ApiErrorPayload,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiErrorPayload {
    /// Stable machine-readable code; the only field clients branch on.
    code: ErrorCode,
    /// English text for logs and operators; never displayed or compared.
    #[schema(value_type = String)]
    message: &'static str,
    /// Equals the `X-Request-Id` response header.
    request_id: String,
    /// Code-specific machine-readable fields; always an object, possibly empty.
    #[schema(value_type = Object)]
    details: ErrorDetails,
}

impl From<ApiError> for ApiErrorBody {
    fn from(error: ApiError) -> Self {
        Self {
            error: ApiErrorPayload {
                code: error.code,
                message: error.message,
                request_id: error.request_id.unwrap_or_default(),
                details: error.details,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code;
        let mut response = match serde_json::to_vec(&ApiErrorBody::from(self)) {
            Ok(body) => (
                status,
                [(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))],
                body,
            )
                .into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        response.extensions_mut().insert(code);
        response
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use axum::response::{IntoResponse, Response};
    use http::header::CONTENT_TYPE;
    use http::StatusCode;
    use http_body_util::{BodyExt, Limited};
    use serde_json::{json, Value};
    use utoipa::OpenApi;

    use super::{ApiError, ApiErrorBody, DetailValue, MAX_DETAIL_ENTRIES};
    use crate::domain::error_code::ErrorCode;
    use crate::domain::secret::Secret;

    const TEST_REQUEST_ID: &str = "0192f3a7-5c4b-7e21-9a02-3f8c1d6e4b90";
    const SOURCE_SECRET: &str = "palmr-leak-sentinel-super-secret";
    const SOURCE_PATH: &str = "/srv/internal/path";

    fn render(error: impl IntoResponse) -> (StatusCode, String, Value) {
        let response: Response = error.into_response();
        let status = response.status();
        let content_type = response.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_owned();
        let mut collect = pin!(Limited::new(response.into_body(), 64 * 1024).collect());
        let Poll::Ready(collected) = collect
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("an error body must be fully buffered");
        };
        let bytes = collected.unwrap().to_bytes();
        (
            status,
            content_type,
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    fn assert_no_leak(body: &Value) {
        let wire = body.to_string();
        for needle in [
            SOURCE_SECRET,
            SOURCE_PATH,
            "password=",
            "backtrace",
            "stack",
        ] {
            assert!(!wire.contains(needle), "{needle:?} leaked into {wire}");
        }
    }

    #[test]
    fn unit_error_envelope_shape() {
        let error = ApiError::new(ErrorCode::RateLimited)
            .with_request_id(TEST_REQUEST_ID)
            .with_detail("scope", "rl.test")
            .with_detail("limit", 10_i64)
            .with_detail("perAccount", false);
        assert!(error.retryable());
        assert_eq!(error.request_id(), Some(TEST_REQUEST_ID));

        let (status, content_type, body) = render(error);

        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(content_type, "application/json; charset=utf-8");
        assert_eq!(
            body,
            json!({
                "error": {
                    "code": "RATE_LIMITED",
                    "message": ErrorCode::RateLimited.default_message(),
                    "requestId": TEST_REQUEST_ID,
                    "details": { "limit": 10, "perAccount": false, "scope": "rl.test" },
                }
            })
        );
    }

    #[test]
    fn unit_error_envelope_always_carries_every_field() {
        let (_, _, body) = render(ApiError::new(ErrorCode::NotFound));

        let envelope = body.as_object().unwrap();
        assert_eq!(envelope.keys().collect::<Vec<_>>(), ["error"]);
        let error = envelope["error"].as_object().unwrap();
        assert_eq!(
            error.keys().collect::<Vec<_>>(),
            ["code", "details", "message", "requestId"]
        );
        assert_eq!(error["details"], json!({}));
        assert!(error["requestId"].is_string());
        assert_eq!(error["code"], "NOT_FOUND");
    }

    #[test]
    fn unit_error_status_derives_from_code() {
        for &code in ErrorCode::ALL {
            let error = ApiError::new(code).with_request_id(TEST_REQUEST_ID);
            assert_eq!(error.code(), code);
            assert_eq!(error.status(), code.status());
            assert_eq!(error.retryable(), code.retryable());

            let (status, content_type, body) = render(error);
            assert_eq!(status, code.status());
            assert_eq!(content_type, "application/json; charset=utf-8");
            assert_eq!(body["error"]["code"], code.as_str());
        }
    }

    #[test]
    fn unit_internal_error_never_leaks_source() {
        let credential = Secret::new(SOURCE_SECRET.to_owned());
        let source = anyhow::anyhow!(
            "database password={} failed at {SOURCE_PATH}",
            credential.expose_secret()
        )
        .context("stack backtrace: frame 0");

        let error = ApiError::from(source).with_request_id(TEST_REQUEST_ID);
        assert_eq!(error.code(), ErrorCode::InternalError);
        assert!(error.details().is_empty());
        assert!(!format!("{error:?}").contains(SOURCE_SECRET));

        let (status, _, body) = render(error);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "INTERNAL_ERROR");
        assert_eq!(
            body["error"]["message"],
            ErrorCode::InternalError.default_message()
        );
        assert_eq!(body["error"]["details"], json!({}));
        assert_no_leak(&body);
    }

    #[derive(Debug, thiserror::Error)]
    enum ProbeError {
        #[error("probe {0} does not exist")]
        Missing(String),
        #[error("probe store failed at {SOURCE_PATH}")]
        Store(#[source] std::io::Error),
    }

    impl From<ProbeError> for ApiError {
        fn from(error: ProbeError) -> Self {
            match error {
                ProbeError::Missing(_) => Self::new(ErrorCode::NotFound),
                ProbeError::Store(_) => Self::internal(),
            }
        }
    }

    fn probe_service(failure: ProbeError) -> Result<(), ProbeError> {
        Err(failure)
    }

    fn probe_handler(failure: ProbeError) -> Result<(), ApiError> {
        probe_service(failure)?;
        Ok(())
    }

    #[test]
    fn unit_feature_error_converts_through_one_boundary() {
        let missing = probe_handler(ProbeError::Missing(SOURCE_SECRET.to_owned())).unwrap_err();
        assert_eq!(missing.code(), ErrorCode::NotFound);
        let (status, _, body) = render(missing);
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_no_leak(&body);

        let io = std::io::Error::other(format!("open {SOURCE_PATH}/{SOURCE_SECRET}"));
        let store = probe_handler(ProbeError::Store(io)).unwrap_err();
        assert_eq!(store.code(), ErrorCode::InternalError);
        let (status, _, body) = render(store);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_no_leak(&body);
    }

    #[test]
    fn unit_error_details_are_bounded() {
        const KEYS: [&str; MAX_DETAIL_ENTRIES + 3] = [
            "k0", "k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9", "k10",
        ];

        let mut error = ApiError::new(ErrorCode::ValidationError);
        for (index, key) in KEYS.into_iter().enumerate() {
            error = error.with_detail(key, i64::try_from(index).unwrap());
        }
        assert_eq!(error.details().len(), MAX_DETAIL_ENTRIES);

        let error = error.with_detail("k0", "replaced").with_detail("k10", true);
        let body = serde_json::to_value(ApiErrorBody::from(error)).unwrap();
        let details = body["error"]["details"].as_object().unwrap();
        assert_eq!(details.len(), MAX_DETAIL_ENTRIES);
        assert_eq!(details["k0"], "replaced");
        assert!(!details.contains_key("k8") && !details.contains_key("k10"));
        assert_eq!(DetailValue::from(true), DetailValue::Bool(true));
    }

    #[test]
    fn unit_error_response_carries_code_extension() {
        let response = ApiError::new(ErrorCode::RequestTimeout).into_response();
        assert_eq!(
            response.extensions().get::<ErrorCode>(),
            Some(&ErrorCode::RequestTimeout)
        );
    }

    #[test]
    fn unit_error_message_override_keeps_code_and_status() {
        let error = ApiError::new(ErrorCode::BatchTooLarge)
            .with_message("The batch exceeds 500 items")
            .with_request_id(TEST_REQUEST_ID);
        let (status, _, body) = render(error);
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "BATCH_TOO_LARGE");
        assert_eq!(body["error"]["message"], "The batch exceeds 500 items");
    }

    #[derive(OpenApi)]
    #[openapi(components(schemas(ApiErrorBody)))]
    struct ErrorSchemaDoc;

    #[test]
    fn unit_error_schema_matches_envelope() {
        let doc = serde_json::to_value(ErrorSchemaDoc::openapi()).unwrap();
        let schemas = &doc["components"]["schemas"];

        let body = &schemas["ApiErrorBody"];
        assert_eq!(body["required"], json!(["error"]));

        let payload = &schemas["ApiErrorPayload"];
        assert_eq!(payload["type"], "object");
        let mut required: Vec<&str> = payload["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["code", "details", "message", "requestId"]);
        assert_eq!(payload["properties"]["message"]["type"], "string");
        assert_eq!(payload["properties"]["requestId"]["type"], "string");
        assert_eq!(payload["properties"]["details"]["type"], "object");

        let mut codes: Vec<&str> = schemas["ErrorCode"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_str().unwrap())
            .collect();
        codes.sort_unstable();
        let mut expected: Vec<&str> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        expected.sort_unstable();
        assert_eq!(codes, expected);
    }
}

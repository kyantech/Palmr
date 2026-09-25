use axum::body::Body;
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::error::ApiError;

pub const BODY_FIELD: &str = "body";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    String,
    Integer,
    Boolean,
}

impl JsonKind {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Integer => value.is_i64() || value.is_u64(),
            Self::Boolean => value.is_boolean(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonField {
    name: &'static str,
    kind: JsonKind,
    required: bool,
}

impl JsonField {
    pub const fn required(name: &'static str, kind: JsonKind) -> Self {
        Self {
            name,
            kind,
            required: true,
        }
    }

    pub const fn optional(name: &'static str, kind: JsonKind) -> Self {
        Self {
            name,
            kind,
            required: false,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn is_required(&self) -> bool {
        self.required
    }

    fn accepts(&self, value: Option<&Value>) -> bool {
        match value {
            None | Some(Value::Null) => !self.required,
            Some(value) => self.kind.accepts(value),
        }
    }
}

pub trait JsonRequest: DeserializeOwned {
    const FIELDS: &'static [JsonField];
}

pub async fn read<T: JsonRequest>(body: Body) -> Result<T, ApiError> {
    let bytes = body
        .collect()
        .await
        .map_err(|_| ApiError::invalid_json())?
        .to_bytes();
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ApiError::invalid_json())?;
    from_value(value)
}

pub fn from_value<T: JsonRequest>(value: Value) -> Result<T, ApiError> {
    let offending = offending_fields(T::FIELDS, &value);
    if !offending.is_empty() {
        return Err(ApiError::validation(offending));
    }
    serde_json::from_value(value).map_err(|_| ApiError::validation([BODY_FIELD]))
}

fn offending_fields(fields: &'static [JsonField], value: &Value) -> Vec<&'static str> {
    let Value::Object(members) = value else {
        return vec![BODY_FIELD];
    };
    let mut offending: Vec<&'static str> = fields
        .iter()
        .filter(|field| !field.accepts(members.get(field.name)))
        .map(JsonField::name)
        .collect();
    let undeclared = members
        .keys()
        .any(|key| !fields.iter().any(|field| field.name == key));
    if undeclared {
        offending.push(BODY_FIELD);
    }
    offending
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use serde::Deserialize;
    use serde_json::{json, Value};

    use super::{read, JsonField, JsonKind, JsonRequest};
    use crate::domain::error_code::ErrorCode;
    use crate::infra::http::error::{ApiError, ApiErrorBody};

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Probe {
        name: String,
        count: i64,
        #[serde(default)]
        enabled: Option<bool>,
    }

    impl JsonRequest for Probe {
        const FIELDS: &'static [JsonField] = &[
            JsonField::required("name", JsonKind::String),
            JsonField::required("count", JsonKind::Integer),
            JsonField::optional("enabled", JsonKind::Boolean),
        ];
    }

    async fn parse(raw: &str) -> Result<Probe, ApiError> {
        read::<Probe>(Body::from(raw.to_owned())).await
    }

    fn details(error: ApiError) -> Value {
        serde_json::to_value(ApiErrorBody::from(error)).unwrap()["error"]["details"].clone()
    }

    #[tokio::test]
    async fn unit_json_syntax_errors_are_invalid_json() {
        for raw in [
            "",
            "{",
            "{\"name\":",
            "nul",
            "{\"name\":\"a\",}",
            "\u{feff}{}",
        ] {
            let error = parse(raw).await.unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidJson, "{raw:?}");
            assert_eq!(error.status(), http::StatusCode::BAD_REQUEST);
            assert!(!error.retryable());
            assert_eq!(details(error), json!({}), "{raw:?}");
        }
    }

    #[tokio::test]
    async fn unit_json_shape_errors_are_validation_fields() {
        let cases = [
            ("[]", json!(["body"])),
            ("\"text\"", json!(["body"])),
            ("{}", json!(["name", "count"])),
            ("{\"count\":1}", json!(["name"])),
            ("{\"name\":7,\"count\":\"x\"}", json!(["name", "count"])),
            ("{\"count\":1,\"name\":null}", json!(["name"])),
            ("{\"name\":\"a\",\"count\":1.5}", json!(["count"])),
            (
                "{\"name\":\"a\",\"count\":1,\"enabled\":\"yes\"}",
                json!(["enabled"]),
            ),
            (
                "{\"name\":\"a\",\"count\":1,\"extra\":true}",
                json!(["body"]),
            ),
            ("{\"extra\":true}", json!(["name", "count", "body"])),
        ];
        for (raw, fields) in cases {
            let error = parse(raw).await.unwrap_err();
            assert_eq!(error.code(), ErrorCode::ValidationError, "{raw}");
            assert_eq!(error.status(), http::StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(details(error), json!({ "fields": fields }), "{raw}");
        }
    }

    #[tokio::test]
    async fn unit_json_valid_body_parses() {
        let parsed = parse("{\"name\":\"a\",\"count\":2}").await.unwrap();
        assert_eq!(parsed.name, "a");
        assert_eq!(parsed.count, 2);
        assert_eq!(parsed.enabled, None);
        let parsed = parse("{\"name\":\"a\",\"count\":2,\"enabled\":null}")
            .await
            .unwrap();
        assert_eq!(parsed.enabled, None);
    }
}

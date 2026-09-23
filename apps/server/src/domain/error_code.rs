use http::StatusCode;
use serde::Serialize;
use utoipa::ToSchema;

// Every property of a code is generated from its single row, so a code cannot
// be rendered with a second status or a second wire spelling anywhere.
macro_rules! error_catalog {
    ($($variant:ident = $wire:literal, $status:ident, retryable: $retryable:literal, $message:literal;)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ToSchema)]
        pub enum ErrorCode {
            $(#[serde(rename = $wire)] #[schema(rename = $wire)] $variant,)+
        }

        impl ErrorCode {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub const fn status(self) -> StatusCode {
                match self {
                    $(Self::$variant => StatusCode::$status,)+
                }
            }

            pub const fn retryable(self) -> bool {
                match self {
                    $(Self::$variant => $retryable,)+
                }
            }

            pub const fn default_message(self) -> &'static str {
                match self {
                    $(Self::$variant => $message,)+
                }
            }
        }
    };
}

error_catalog! {
    ValidationError = "VALIDATION_ERROR", UNPROCESSABLE_ENTITY, retryable: false,
        "The request failed validation";
    NotFound = "NOT_FOUND", NOT_FOUND, retryable: false,
        "The requested resource was not found";
    MethodNotAllowed = "METHOD_NOT_ALLOWED", METHOD_NOT_ALLOWED, retryable: false,
        "The method is not allowed for this path";
    Forbidden = "FORBIDDEN", FORBIDDEN, retryable: false,
        "The action is not permitted";
    UnsupportedMediaType = "UNSUPPORTED_MEDIA_TYPE", UNSUPPORTED_MEDIA_TYPE, retryable: false,
        "The request content type is not supported";
    RequestBodyTooLarge = "REQUEST_BODY_TOO_LARGE", PAYLOAD_TOO_LARGE, retryable: false,
        "The request body is too large";
    RequestTimeout = "REQUEST_TIMEOUT", REQUEST_TIMEOUT, retryable: false,
        "The request exceeded the server deadline";
    InternalError = "INTERNAL_ERROR", INTERNAL_SERVER_ERROR, retryable: true,
        "An internal error occurred";
    ServiceUnavailable = "SERVICE_UNAVAILABLE", SERVICE_UNAVAILABLE, retryable: true,
        "The service is temporarily unavailable";
    CursorInvalid = "CURSOR_INVALID", BAD_REQUEST, retryable: false,
        "The pagination cursor is invalid";
    RateLimited = "RATE_LIMITED", TOO_MANY_REQUESTS, retryable: true,
        "Too many requests";
    CsrfTokenMissing = "CSRF_TOKEN_MISSING", FORBIDDEN, retryable: false,
        "The CSRF token is missing";
    CsrfTokenInvalid = "CSRF_TOKEN_INVALID", FORBIDDEN, retryable: false,
        "The CSRF token is invalid";
    OriginNotAllowed = "ORIGIN_NOT_ALLOWED", FORBIDDEN, retryable: false,
        "The request origin is not allowed";
    IdempotencyKeyConflict = "IDEMPOTENCY_KEY_CONFLICT", CONFLICT, retryable: false,
        "The idempotency key was already used for a different request";
    IdempotencyRequestInProgress = "IDEMPOTENCY_REQUEST_IN_PROGRESS", CONFLICT, retryable: true,
        "A request with this idempotency key is still in progress";
    BatchTooLarge = "BATCH_TOO_LARGE", UNPROCESSABLE_ENTITY, retryable: false,
        "The batch contains too many items";
    FeatureUnavailableSmtp = "FEATURE_UNAVAILABLE_SMTP", CONFLICT, retryable: false,
        "This action requires e-mail delivery to be configured";
    SetupAlreadyCompleted = "SETUP_ALREADY_COMPLETED", CONFLICT, retryable: false,
        "Setup has already been completed";
    DatabaseBusy = "DATABASE_BUSY", SERVICE_UNAVAILABLE, retryable: true,
        "The database is temporarily busy";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CatalogEntry {
    pub code: ErrorCode,
    pub status: u16,
    pub retryable: bool,
}

impl ErrorCode {
    pub fn catalog() -> Vec<CatalogEntry> {
        let mut entries: Vec<CatalogEntry> = Self::ALL
            .iter()
            .map(|&code| CatalogEntry {
                code,
                status: code.status().as_u16(),
                retryable: code.retryable(),
            })
            .collect();
        entries.sort_by_key(|entry| entry.code.as_str());
        entries
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::ErrorCode;

    const ERROR_STATUSES: [u16; 21] = [
        400, 401, 403, 404, 405, 408, 409, 410, 412, 413, 415, 416, 422, 423, 429, 460, 500, 501,
        502, 503, 507,
    ];

    #[test]
    fn unit_error_code_status_mapping() {
        for &code in ErrorCode::ALL {
            let status = code.status();
            assert!(
                ERROR_STATUSES.contains(&status.as_u16()),
                "{} maps to {status}",
                code.as_str()
            );
            assert!(status.is_client_error() || status.is_server_error());
        }

        let catalog = serde_json::to_string_pretty(&ErrorCode::catalog()).unwrap();
        insta::with_settings!({
            snapshot_path => "../../tests/snapshots",
            prepend_module_to_snapshot => false,
            omit_expression => true,
        }, {
            insta::assert_snapshot!("error_catalog", catalog);
        });
    }

    #[test]
    fn unit_error_code_wire_names_are_stable_identifiers() {
        let mut seen = BTreeSet::new();
        for &code in ErrorCode::ALL {
            let wire = code.as_str();
            assert!(seen.insert(wire), "{wire} is registered twice");
            assert!(
                !wire.starts_with("CLIENT_"),
                "{wire} uses the client namespace"
            );
            assert!(!wire.starts_with('_') && !wire.ends_with('_') && !wire.contains("__"));
            assert!(wire.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'));
            assert_eq!(serde_json::to_value(code).unwrap(), wire);
            assert!(!code.default_message().is_empty());
        }
        assert_eq!(seen.len(), ErrorCode::ALL.len());
    }

    #[test]
    fn unit_error_catalog_is_sorted_and_complete() {
        let catalog = ErrorCode::catalog();
        assert_eq!(catalog.len(), ErrorCode::ALL.len());
        assert!(catalog
            .windows(2)
            .all(|pair| pair[0].code.as_str() < pair[1].code.as_str()));
        for entry in &catalog {
            assert_eq!(entry.status, entry.code.status().as_u16());
            assert_eq!(entry.retryable, entry.code.retryable());
        }
        assert_eq!(catalog, ErrorCode::catalog());
    }

    #[test]
    fn unit_error_code_retryability() {
        let retryable: BTreeSet<&str> = ErrorCode::ALL
            .iter()
            .filter(|code| code.retryable())
            .map(|code| code.as_str())
            .collect();
        assert_eq!(
            retryable,
            BTreeSet::from([
                "DATABASE_BUSY",
                "IDEMPOTENCY_REQUEST_IN_PROGRESS",
                "INTERNAL_ERROR",
                "RATE_LIMITED",
                "SERVICE_UNAVAILABLE",
            ])
        );

        assert_eq!(
            ErrorCode::IdempotencyKeyConflict.status(),
            ErrorCode::IdempotencyRequestInProgress.status()
        );
        assert!(!ErrorCode::IdempotencyKeyConflict.retryable());
        assert!(ErrorCode::IdempotencyRequestInProgress.retryable());
    }

    #[test]
    fn unit_middleware_error_codes_match_catalog() {
        assert_eq!(
            ErrorCode::RequestBodyTooLarge.as_str(),
            "REQUEST_BODY_TOO_LARGE"
        );
        assert_eq!(ErrorCode::RequestBodyTooLarge.status().as_u16(), 413);
        assert!(!ErrorCode::RequestBodyTooLarge.retryable());

        assert_eq!(ErrorCode::RequestTimeout.as_str(), "REQUEST_TIMEOUT");
        assert_eq!(ErrorCode::RequestTimeout.status().as_u16(), 408);
        assert!(!ErrorCode::RequestTimeout.retryable());

        assert_eq!(ErrorCode::InternalError.status().as_u16(), 500);
        assert!(ErrorCode::InternalError.retryable());
    }
}

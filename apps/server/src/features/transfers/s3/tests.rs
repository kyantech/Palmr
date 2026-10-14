use serde_json::{json, Value};

use super::model::{PresignRequest, ReportPartsRequest};
use crate::app::auth_class::AuthClass;
use crate::app::openapi::ApiDocs;
use crate::app::router::{application_routes, RateLimitClass, Transport};
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::error_code::ErrorCode;
use crate::features::transfers::error::{upstream_code, TransferError};
use crate::infra::http::json::{parse, JsonRequest};
use crate::storage::error::{Retryable, StorageError};

const PREFIX: &str = "/api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart";

#[test]
fn unit_s3_routes_are_the_six_authenticated_control_plane_routes() {
    let inventory = application_routes().build().unwrap().inventory;
    let mut found: Vec<(String, String)> = inventory
        .entries()
        .iter()
        .filter(|entry| entry.path().starts_with(PREFIX))
        .map(|entry| {
            assert_eq!(entry.policy().auth(), AuthClass::Authenticated);
            assert_eq!(entry.policy().rate_limit(), RateLimitClass::TransferControl);
            assert_eq!(entry.policy().transport(), Transport::ControlPlane);
            (
                entry.method().to_string(),
                entry.path().trim_start_matches(PREFIX).to_owned(),
            )
        })
        .collect();
    found.sort();
    let expected: Vec<(String, String)> = [
        ("GET", "/parts"),
        ("POST", ""),
        ("POST", "/abort"),
        ("POST", "/complete"),
        ("POST", "/parts"),
        ("POST", "/presign"),
    ]
    .into_iter()
    .map(|(method, tail)| (method.to_owned(), tail.to_owned()))
    .collect();
    assert_eq!(found, expected);
}

fn document() -> Value {
    let config = OperatorConfig::load(&EnvironmentSource::from_vars(std::iter::empty::<(
        &str,
        &str,
    )>()))
    .unwrap()
    .config;
    let docs = ApiDocs::new(
        application_routes().build().unwrap().openapi,
        &config.base_url,
    )
    .unwrap();
    serde_json::from_slice(docs.document()).unwrap()
}

#[test]
fn unit_s3_contract_exposes_no_storage_identity_field() {
    let document = document();
    let forbidden = [
        "bucket",
        "key",
        "objectKey",
        "objectName",
        "path",
        "uploadId",
        "s3UploadId",
        "finalObjectKey",
        "finalObjectId",
        "endpoint",
        "accessKey",
        "secretKey",
    ];
    let paths = document["paths"].as_object().unwrap();
    let mut checked = 0;
    for (path, item) in paths
        .iter()
        .filter(|(path, _)| path.contains("/s3/multipart"))
    {
        for (method, operation) in item.as_object().unwrap() {
            checked += 1;
            for parameter in operation["parameters"].as_array().into_iter().flatten() {
                let name = parameter["name"].as_str().unwrap();
                assert!(!forbidden.contains(&name), "{method} {path} takes {name}");
            }
        }
    }
    assert_eq!(checked, 6);
    let schemas = document["components"]["schemas"].as_object().unwrap();
    for name in [
        "CreateMultipartResponse",
        "PresignRequest",
        "PresignResponse",
        "PresignedPart",
        "ReportPartsRequest",
        "ReportedPart",
        "PartsResponse",
        "CompletedPartView",
        "CompleteResponse",
    ] {
        let properties = schemas[name]["properties"].as_object().unwrap();
        for field in forbidden {
            assert!(!properties.contains_key(field), "{name}.{field}");
        }
    }
}

#[test]
fn unit_s3_requests_reject_storage_identity_fields_and_malformed_shapes() {
    for extra in [
        "bucket",
        "key",
        "objectKey",
        "uploadId",
        "s3UploadId",
        "endpoint",
    ] {
        let presign = json!({ "partNumbers": [1], extra: "x" });
        assert!(parse::<PresignRequest>(presign).is_err(), "{extra}");
        let report = json!({
            "parts": [{ "partNumber": 1, "etag": "\"a\"", "sizeBytes": 8_388_608 }],
            extra: "x"
        });
        assert!(parse::<ReportPartsRequest>(report).is_err(), "{extra}");
        let nested = json!({
            "parts": [{ "partNumber": 1, "etag": "\"a\"", "sizeBytes": 8_388_608, extra: "x" }]
        });
        assert!(parse::<ReportPartsRequest>(nested).is_err(), "{extra}");
    }
    assert!(parse::<PresignRequest>(json!({})).is_err());
    assert!(parse::<PresignRequest>(json!({ "partNumbers": "1" })).is_err());
    assert!(parse::<ReportPartsRequest>(json!({ "parts": [{ "partNumber": "1" }] })).is_err());
    assert_eq!(PresignRequest::FIELDS.len(), 1);
    assert_eq!(ReportPartsRequest::FIELDS.len(), 1);
}

#[test]
fn unit_s3_error_mapping_is_stable_and_never_leaks_storage_detail() {
    let table: [(TransferError, ErrorCode, u16); 9] = [
        (
            TransferError::MultipartNotFound,
            ErrorCode::MultipartNotFound,
            404,
        ),
        (
            TransferError::MultipartAlreadyCreated,
            ErrorCode::MultipartAlreadyCreated,
            409,
        ),
        (
            TransferError::MultipartAlreadyCompleted,
            ErrorCode::MultipartAlreadyCompleted,
            409,
        ),
        (
            TransferError::MultipartIncomplete {
                missing: vec![2, 5],
            },
            ErrorCode::MultipartIncomplete,
            409,
        ),
        (
            TransferError::MultipartPartInvalid {
                reason: "part_size",
            },
            ErrorCode::MultipartPartInvalid,
            422,
        ),
        (
            TransferError::PresignBatchTooLarge,
            ErrorCode::PresignBatchTooLarge,
            422,
        ),
        (
            TransferError::Recorded(ErrorCode::QuotaExceeded),
            ErrorCode::QuotaExceeded,
            507,
        ),
        (
            TransferError::CompletionBusy,
            ErrorCode::TransferBackpressure,
            503,
        ),
        (
            TransferError::Storage(StorageError::S3(Box::new(std::io::Error::other(
                "https://secret.invalid/objects/ab/cd?X-Amz-Signature=abc",
            )))),
            ErrorCode::StorageUpstreamError,
            502,
        ),
    ];
    for (error, code, status) in table {
        let api = error.api_error();
        assert_eq!(api.code(), code, "{error}");
        assert_eq!(api.status().as_u16(), status, "{error}");
        let body =
            serde_json::to_string(&crate::infra::http::error::ApiErrorBody::from(api)).unwrap();
        assert!(!body.contains("secret.invalid"), "{body}");
        assert!(!body.contains("X-Amz"), "{body}");
    }

    let unavailable = [
        StorageError::ProviderUnavailable(Retryable::new(std::io::Error::other("down"))),
        StorageError::Io(std::io::Error::other("io")),
    ];
    for error in &unavailable {
        assert_eq!(upstream_code(error), ErrorCode::StorageUnavailable);
    }
    for error in [
        StorageError::S3(Box::new(std::io::Error::other("rejected"))),
        StorageError::PermissionDenied,
        StorageError::NotFound,
    ] {
        assert_eq!(upstream_code(&error), ErrorCode::StorageUpstreamError);
    }
    let missing = TransferError::MultipartIncomplete {
        missing: vec![2, 5],
    }
    .api_error();
    let body =
        serde_json::to_value(crate::infra::http::error::ApiErrorBody::from(missing)).unwrap();
    assert_eq!(
        body["error"]["details"]["missingPartNumbers"],
        json!([2, 5])
    );
}

const SOURCES: [(&str, &str); 7] = [
    ("abort.rs", include_str!("abort.rs")),
    ("model.rs", include_str!("model.rs")),
    ("reconcile.rs", include_str!("reconcile.rs")),
    ("repo.rs", include_str!("repo.rs")),
    ("routes.rs", include_str!("routes.rs")),
    ("service.rs", include_str!("service.rs")),
    ("finalize/s3.rs", include_str!("../finalize/s3.rs")),
];

fn production(source: &str) -> &str {
    source.split("#[cfg(test)]").next().unwrap_or(source)
}

#[test]
fn unit_s3_control_plane_has_no_forbidden_construction() {
    for (file, source) in SOURCES {
        let source = production(source);
        for token in [
            "aws_sdk_s3",
            "aws_smithy",
            "PALMR_STORAGE_PROVIDER",
            "ProviderKind",
            "to_bytes",
            "read_to_end",
            "read_to_string",
            "fs::read",
            "ByteStream",
            "collect().await",
            "SystemTime::n",
            "OffsetDateTime::now",
            "upload_part_stream",
            "InsertStorageObject",
            "INSERT INTO storage_objects",
            "tus_uploads.storage_object_id",
            "unwrap()",
            "expect(",
        ] {
            assert!(!source.contains(token), "{file} contains {token}");
        }
    }
    let repo = production(include_str!("repo.rs"));
    assert!(
        !repo.contains("storage_objects"),
        "only the commit creates object rows"
    );
}

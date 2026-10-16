use serde_json::Value;
use tempfile::TempDir;

use super::folders::{Member, HOST_A};
use super::transfers::SESSIONS;
use super::tus_patch::{stack_small, upload_for};
use super::*;
use crate::domain::error_code::ErrorCode;
use crate::features::transfers::classify::{Classification, RetryClass};
use crate::features::transfers::error::TransferError;
use crate::features::transfers::ErrorParts;
use crate::storage::error::StorageError;
use crate::storage::s3::{classify_failure, Failure, Operation};

struct Case {
    label: &'static str,
    cause: fn() -> TransferError,
    status: u16,
    code: &'static str,
    class: RetryClass,
}

fn provider(failure: Failure) -> TransferError {
    TransferError::Storage(classify_failure(
        Operation::CompleteMultipartUpload,
        failure,
    ))
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a-provider-5xx",
            cause: || {
                provider(Failure::Service {
                    status: 503,
                    code: Some("SlowDown".to_owned()),
                })
            },
            status: 502,
            code: "STORAGE_UPSTREAM_ERROR",
            class: RetryClass::Retryable,
        },
        Case {
            label: "a-provider-timeout",
            cause: || provider(Failure::Timeout),
            status: 502,
            code: "STORAGE_UPSTREAM_ERROR",
            class: RetryClass::Retryable,
        },
        Case {
            label: "b-provider-403",
            cause: || {
                provider(Failure::Service {
                    status: 403,
                    code: Some("AccessDenied".to_owned()),
                })
            },
            status: 500,
            code: "STORAGE_PERMISSION_DENIED",
            class: RetryClass::Fatal,
        },
        Case {
            label: "c-storage-unavailable",
            cause: || TransferError::Storage(StorageError::Io(std::io::Error::other("eio"))),
            status: 503,
            code: "STORAGE_UNAVAILABLE",
            class: RetryClass::Retryable,
        },
        Case {
            label: "c-provider-unreachable",
            cause: || provider(Failure::Dispatch),
            status: 503,
            code: "STORAGE_UNAVAILABLE",
            class: RetryClass::Retryable,
        },
        Case {
            label: "d-local-eacces",
            cause: || TransferError::Storage(StorageError::PermissionDenied),
            status: 500,
            code: "STORAGE_PERMISSION_DENIED",
            class: RetryClass::Fatal,
        },
        Case {
            label: "e-invalid-part",
            cause: || TransferError::MultipartPartRejected,
            status: 422,
            code: "MULTIPART_PART_REJECTED",
            class: RetryClass::NeedsReconciliation,
        },
        Case {
            label: "f-entity-too-small",
            cause: || TransferError::MultipartPartInvalid {
                reason: "provider_part_too_small",
            },
            status: 422,
            code: "MULTIPART_PART_INVALID",
            class: RetryClass::Fatal,
        },
        Case {
            label: "g-part-number",
            cause: || TransferError::MultipartPartInvalid {
                reason: "part_number",
            },
            status: 422,
            code: "MULTIPART_PART_INVALID",
            class: RetryClass::Fatal,
        },
    ]
}

fn decode(wire: &str) -> Classification {
    let code = ErrorCode::ALL
        .iter()
        .copied()
        .find(|known| known.as_str() == wire)
        .unwrap_or_else(|| panic!("{wire} is not a catalogue code"));
    Classification::of(code)
}

async fn read_errors(stack: &Stack, member: &Member, session: &str) -> (Value, Value) {
    let fetched = stack.read(&format!("{SESSIONS}/{session}"), member).await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    let view = fetched.json();
    (view["error"].clone(), view["files"][0]["error"].clone())
}

async fn side_effects(stack: &Stack) -> (i64, i64, String) {
    (
        stack.scalar_i64("SELECT COUNT(*) FROM files").await,
        stack
            .scalar_i64("SELECT COALESCE(SUM(used_bytes), 0) FROM users")
            .await,
        stack
            .scalar_text(
                "SELECT COALESCE(GROUP_CONCAT(state || ':' || reserved_bytes, ','), '')
                   FROM (SELECT state, reserved_bytes FROM quota_reservations ORDER BY id)",
            )
            .await,
    )
}

#[tokio::test]
async fn it_transfer_error_class_survives_persistence_reload_and_restart() {
    let root = TempDir::new().unwrap();
    let (stack, _staging) = stack_small(root.path()).await;
    let alice = stack.member("alice", HOST_A).await;
    let alice_id = alice.id;
    let clock = stack.clock.clone();

    let mut persisted = Vec::new();
    for case in cases() {
        let (planned, _upload) =
            upload_for(&stack, &alice, &format!("{}.bin", case.label), Some(1_000)).await;
        let cause = (case.cause)();

        let initial = cause.classification();
        assert_eq!(initial.code.as_str(), case.code, "{}", case.label);
        assert_eq!(initial.class(), case.class, "{}", case.label);
        let api = cause.api_error();
        assert_eq!(api.code(), initial.code, "{}", case.label);
        assert_eq!(api.status().as_u16(), case.status, "{}", case.label);

        let request_id = format!("req-{}", case.label);
        stack
            .transfers
            .fail_item(
                alice.id,
                planned.session.parse().unwrap(),
                planned.item.parse().unwrap(),
                ErrorParts::of(initial.code, Some(&request_id)),
            )
            .await
            .unwrap();

        let stored: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT error_code, error_request_id FROM transfer_session_files WHERE id = ?1",
        )
        .bind(&planned.item)
        .fetch_one(stack.pools.reader().executor())
        .await
        .unwrap();
        assert_eq!(
            stored,
            (Some(case.code.to_owned()), Some(request_id.clone())),
            "{}",
            case.label
        );

        let (session_error, item_error) = read_errors(&stack, &alice, &planned.session).await;
        for error in [&session_error, &item_error] {
            assert_eq!(error["code"], case.code, "{}", case.label);
            assert_eq!(error["requestId"], request_id, "{}", case.label);
        }
        assert_eq!(
            decode(item_error["code"].as_str().unwrap()).class(),
            case.class,
            "{}: the reloaded DTO decodes to the intended policy",
            case.label
        );
        persisted.push((
            case.label,
            planned.session.clone(),
            request_id,
            case.code,
            case.class,
        ));
    }
    let effects = side_effects(&stack).await;
    assert_eq!(effects.0, 0);
    assert_eq!(effects.1, 0);

    stack.stop().await;
    let restarted = Stack::start(root.path(), &clock).await;
    let alice = Member {
        id: alice_id,
        creds: restarted.signed_in("alice", HOST_A).await,
    };
    for (label, session, request_id, code, class) in &persisted {
        let (session_error, item_error) = read_errors(&restarted, &alice, session).await;
        for error in [&session_error, &item_error] {
            assert_eq!(error["code"], *code, "{label}");
            assert_eq!(error["requestId"], *request_id, "{label}");
        }
        let decoded = decode(item_error["code"].as_str().unwrap());
        assert_eq!(decoded.class(), *class, "{label}: policy after a restart");
    }
    assert_eq!(
        side_effects(&restarted).await,
        effects,
        "failures change neither quota nor files across a restart"
    );
    restarted.stop().await;
}

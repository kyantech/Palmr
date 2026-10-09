use crate::domain::bytes::ByteSize;
use crate::domain::error_code::ErrorCode;

use super::admission::TransferStorage;
use super::error::TransferError;
use super::model::{
    wire_bytes, TransferFileView, TransferItemError, TransferProtocol, TransferS3Plan,
    TransferSessionSummary, TransferSessionView, UploadKind, MAX_PRESIGN_BATCH,
    PRESIGN_TTL_SECONDS,
};
use super::repo::{ErrorParts, ItemRow, SessionRow};
use super::state::ItemState;

pub fn error_view(parts: &ErrorParts) -> TransferItemError {
    let code = ErrorCode::ALL
        .iter()
        .copied()
        .find(|code| code.as_str() == parts.code)
        .unwrap_or(ErrorCode::InternalError);
    TransferItemError {
        code,
        request_id: parts.request_id.clone(),
    }
}

pub fn file_view(item: &ItemRow, storage: &TransferStorage) -> TransferFileView {
    let relative_path = if item.directory.is_empty() {
        None
    } else {
        Some(format!("{}/{}", item.directory, item.name))
    };
    let tus = (item.kind == UploadKind::Tus).then(|| storage.tus_plan());
    let s3 = (item.kind == UploadKind::S3Multipart).then(|| s3_plan(item, storage));
    TransferFileView {
        item_id: item.id.to_string(),
        client_id: item.client_key.clone(),
        name: item.name.clone(),
        relative_path,
        state: item.state.wire(),
        protocol: TransferProtocol::from(item.kind),
        size_bytes: item.declared.map(wire_bytes),
        uploaded_bytes: wire_bytes(item.uploaded),
        file_id: if item.state == ItemState::Completed {
            item.file_id.clone()
        } else {
            None
        },
        error: item.error.as_ref().map(error_view),
        attempts: item.attempts,
        tus,
        s3,
    }
}

fn s3_plan(item: &ItemRow, storage: &TransferStorage) -> TransferS3Plan {
    match item.stored_plan {
        Some((part_size, part_count)) => TransferS3Plan {
            part_size_bytes: Some(wire_bytes(part_size)),
            part_count: Some(part_count),
            max_presign_batch: MAX_PRESIGN_BATCH,
            presign_ttl_seconds: PRESIGN_TTL_SECONDS,
            completed_parts: item.completed_parts,
        },
        None => {
            let mut plan = storage
                .s3_plan(item.kind, item.declared)
                .unwrap_or(TransferS3Plan {
                    part_size_bytes: None,
                    part_count: None,
                    max_presign_batch: MAX_PRESIGN_BATCH,
                    presign_ttl_seconds: PRESIGN_TTL_SECONDS,
                    completed_parts: None,
                });
            plan.completed_parts = item.completed_parts;
            plan
        }
    }
}

pub fn session_view(
    session: &SessionRow,
    items: &[ItemRow],
    storage: &TransferStorage,
) -> Result<TransferSessionView, TransferError> {
    let uploaded = items
        .iter()
        .try_fold(ByteSize::ZERO, |sum, item| sum.checked_add(item.uploaded))
        .ok_or(TransferError::Invariant { what: "progress" })?;
    Ok(TransferSessionView {
        id: session.id.to_string(),
        state: session.state,
        provider: session.provider,
        created_at: session.created_at.to_string(),
        updated_at: session.updated_at.to_string(),
        expires_at: session.expires_at.to_string(),
        total_bytes: wire_bytes(session.declared_bytes),
        uploaded_bytes: wire_bytes(uploaded),
        reserved_bytes: wire_bytes(session.held),
        error: session.error.as_ref().map(error_view),
        files: items.iter().map(|item| file_view(item, storage)).collect(),
    })
}

pub fn summary(session: &SessionRow) -> TransferSessionSummary {
    TransferSessionSummary {
        id: session.id.to_string(),
        state: session.state,
        provider: session.provider,
        created_at: session.created_at.to_string(),
        updated_at: session.updated_at.to_string(),
        expires_at: session.expires_at.to_string(),
        total_bytes: wire_bytes(session.declared_bytes),
        uploaded_bytes: wire_bytes(session.uploaded),
        reserved_bytes: wire_bytes(session.held),
        file_count: session.declared_file_count,
        completed_file_count: session.completed_file_count,
        error: session.error.as_ref().map(error_view),
    }
}

impl From<UploadKind> for TransferProtocol {
    fn from(kind: UploadKind) -> Self {
        kind.protocol()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ItemRow, SessionRow, TransferItemError};
    use crate::domain::bytes::ByteSize;
    use crate::features::transfers::model::{
        wire_bytes, TransferFileView, TransferProtocol, TransferProvider, TransferS3Plan,
        TransferSessionView, TransferTusPlan, ValidatedSession, MAX_FILES_PER_SESSION,
        MAX_PRESIGN_BATCH, PRESIGN_TTL_SECONDS,
    };
    use crate::features::transfers::state::{TransferItemState, TransferSessionState};
    use crate::infra::http::idempotency::TRANSFER_SESSION_ENVELOPE_BYTES;
    use crate::infra::http::pagination::MAX_WIRE_BYTES;

    const TIB: u64 = 1 << 40;
    const GIB: u64 = 1 << 30;

    fn bytes(value: u64) -> ByteSize {
        ByteSize::try_from(value).unwrap()
    }

    fn worst_file(protocol: TransferProtocol, name: &str, path: &str) -> TransferFileView {
        let is_s3 = protocol == TransferProtocol::S3Multipart;
        TransferFileView {
            item_id: "0192f3a1-0000-7000-8000-0000000000ff".to_owned(),
            client_id: "c".repeat(128),
            name: name.to_owned(),
            relative_path: Some(path.to_owned()),
            state: TransferItemState::Created,
            protocol,
            size_bytes: Some(wire_bytes(bytes(if is_s3 {
                5 * TIB
            } else {
                u64::try_from(MAX_WIRE_BYTES).unwrap()
            }))),
            uploaded_bytes: wire_bytes(ByteSize::ZERO),
            file_id: None,
            error: None::<TransferItemError>,
            attempts: 0,
            tus: (!is_s3).then(|| TransferTusPlan {
                create_url: "/api/v1/uploads/tus".to_owned(),
            }),
            s3: is_s3.then(|| TransferS3Plan {
                part_size_bytes: Some(wire_bytes(bytes(5 * GIB))),
                part_count: Some(9_900),
                max_presign_batch: MAX_PRESIGN_BATCH,
                presign_ttl_seconds: PRESIGN_TTL_SECONDS,
                completed_parts: None,
            }),
        }
    }

    fn envelope_bytes(file: TransferFileView) -> usize {
        let view = TransferSessionView {
            id: "0192f3a1-0000-7000-8000-0000000000aa".to_owned(),
            state: TransferSessionState::Created,
            provider: TransferProvider::S3,
            created_at: "2026-09-25T12:00:00.000Z".to_owned(),
            updated_at: "2026-09-25T12:00:00.000Z".to_owned(),
            expires_at: "2026-09-26T12:00:00.000Z".to_owned(),
            total_bytes: wire_bytes(bytes(u64::try_from(MAX_WIRE_BYTES).unwrap())),
            uploaded_bytes: wire_bytes(ByteSize::ZERO),
            reserved_bytes: wire_bytes(bytes(u64::try_from(MAX_WIRE_BYTES).unwrap())),
            error: None,
            files: vec![file; MAX_FILES_PER_SESSION],
        };
        let body = serde_json::to_value(&view).unwrap();
        serde_json::to_vec(&json!({ "body": body, "headers": {} }))
            .unwrap()
            .len()
    }

    #[test]
    fn unit_maximum_create_response_fits_the_replay_bound_with_headroom() {
        let segment = "\"".repeat(255);
        let path = [segment.as_str(); 4].join("/");
        assert!(path.len() <= 1_024);
        let accepted = ValidatedSession::parse(
            serde_json::from_value(json!({
                "target": { "kind": "my_files" },
                "files": [{
                    "clientId": "c".repeat(128),
                    "name": segment,
                    "sizeBytes": 5 * TIB,
                    "relativePath": path,
                }],
            }))
            .unwrap(),
        );
        assert!(accepted.is_ok(), "the worst-case item is a legal request");

        let worst = [TransferProtocol::S3Multipart, TransferProtocol::Tus]
            .into_iter()
            .map(|protocol| envelope_bytes(worst_file(protocol, &segment, &path)))
            .max()
            .unwrap();
        assert!(
            worst * 104 / 100 <= TRANSFER_SESSION_ENVELOPE_BYTES,
            "worst-case envelope {worst} bytes leaves under 4% headroom below {TRANSFER_SESSION_ENVELOPE_BYTES}"
        );
        assert!(
            TRANSFER_SESSION_ENVELOPE_BYTES < worst * 110 / 100,
            "the bound {TRANSFER_SESSION_ENVELOPE_BYTES} is not the smallest justified for {worst}"
        );
    }

    #[test]
    fn unit_session_and_item_rows_render_without_storage_identity() {
        fn assert_sendable<T: Send>() {}
        assert_sendable::<SessionRow>();
        assert_sendable::<ItemRow>();
    }
}

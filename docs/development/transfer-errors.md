# Transfer failure classification and durable errors

| | |
|---|---|
| Module | `apps/server/src/features/transfers/classify.rs` (types and mapper), `error.rs`, `tus/error.rs`, `repo.rs`, `service.rs` |
| Governing | TRANSFER_ENGINE §9, API_DESIGN §3 (catalogue authority), ADR 0025 |
| Tests | `unit_transfer_error_classification_table`, `regression_394_upload_error_visibility`, `it_transfer_error_*` |

## One classifier, typed causes

Every transfer failure is classified from a **typed cause**, never from a message, a host name or a bare HTTP status:

| Source | Cause type | Mapper |
|---|---|---|
| Domain | `TransferError`, `TusError`, `RejectCause`, `QuotaError`, `FolderError` | `TransferError::classification`, `TusError::classification`, `RejectCause::code` |
| Local storage | `StorageError` (from `io::ErrorKind` / errno) and `WriteFault` | `classify_storage(error, StorageSite)`, `classify_write` |
| S3 | `StorageError` carrying a structured `S3Failure` (`Failure::{Timeout, Dispatch, Service{status, code}}`) | `classify_storage`, `provider_fault` |
| SQLite | `DbError` kind (`Busy`, constraint codes) | `DbError::api_code` |

`Classification { code, class }` pairs the catalogue `ErrorCode` (its status comes from the catalogue row, never restated) with a `RetryClass`:

| Class | Meaning | Automatic behaviour |
|---|---|---|
| `retryable` | transient | bounded jittered exponential backoff, capped attempts |
| `needs-reconciliation` | the caller's view of the offset or parts is stale | `HEAD` (TUS) or `GET …/parts` (S3), then retry without backoff |
| `terminal` | the session or item is finished | none; never resurrected |
| `fatal` | policy, size, type or provider condition that a retry cannot change | none; manual re-admission only |

**The persisted code determines the retry class.** Only the code is persisted (`error_code`), and a client reconstructs its policy from it, so `RetryClass::for_code(code)` is the single decoder and `Classification` has no field or constructor that could carry a different class: the class is derived from the code, never chosen per cause. Two causes that need different policies therefore have different public codes. `regression_transfer_error_class_is_a_function_of_the_persisted_code` enumerates every cause the classifier knows (every `StorageError` variant at every `StorageSite`, every provider fault, `WriteFault`, `RejectCause`, every part-invalid reason, every catalogue code as `Recorded`) and fails when one code is produced with two classes, when a live class differs from the decode of its code, or when a backing-off class is not allowed by the catalogue's `retryable` flag. The class itself is internal and is **not** an envelope field: the public envelope stays `{code, message, requestId, details}`.

| Code | Class | What changed in M14-T11 |
|---|---|---|
| `STORAGE_UNAVAILABLE` | retryable | always retryable; local `EACCES`/`EPERM` no longer uses it |
| `STORAGE_UPSTREAM_ERROR` | retryable, bounded | `5xx`, throttling, timeout, other provider rejection; a provider `403` no longer uses it |
| `STORAGE_PERMISSION_DENIED` (new, `500`) | fatal | local `EACCES`/`EPERM`, provider `403`/`AccessDenied`; TRANSFER_ENGINE §9.1/§9.2 already named it as a health state, it is now also an HTTP catalogue code |
| `MULTIPART_PART_REJECTED` (new, `422`) | needs-reconciliation | provider `InvalidPart`/`InvalidPartOrder`: `GET …/parts` (`ListParts`) first, then complete again |
| `MULTIPART_PART_INVALID` | fatal | `EntityTooSmall` (`details.reason = provider_part_too_small`), part number or size defects |
| `MULTIPART_ALREADY_CREATED` | needs-reconciliation | was retryable; an unchanged resend repeats the `409`, the client reads the existing plan |

`UPLOAD_OFFSET_MISMATCH`, `UPLOAD_LOCKED` and `TRANSFER_BACKPRESSURE` are negotiation or waiting signals. `Classification::records_item_failure` is `false` for them and `TransferService::fail_item` refuses to persist them (`unit`/flow test in `transfer_reaper_scale`).

## Classification matrix

Rows are exactly TRANSFER_ENGINE §9.1; the table test builds each cause and asserts code, status, class, details and the absence of any secret. `Class` is the cause-level class.

| Condition (typed cause) | Status | Code | Class |
|---|---|---|---|
| `TusError::VersionUnsupported` | 412 | `TUS_VERSION_UNSUPPORTED` | fatal |
| `TusError::ExtensionUnsupported` | 501 | `TUS_EXTENSION_UNSUPPORTED` | fatal |
| `TusError::Header` | 400 | `UPLOAD_METADATA_INVALID` | fatal |
| `Overrun`, `TooLarge`, `LengthBeyondRange` | 413 | `FILE_TOO_LARGE` | fatal |
| `UnsupportedMedia` | 415 | `UNSUPPORTED_MEDIA_TYPE` | fatal |
| `OffsetMismatch` | 409 | `UPLOAD_OFFSET_MISMATCH` | needs-reconciliation |
| `Locked` | 423 | `UPLOAD_LOCKED` | retryable |
| `Gone` | 410 | `UPLOAD_SESSION_EXPIRED` | terminal |
| `ChecksumMismatch` | 460 | `UPLOAD_CHECKSUM_MISMATCH` | needs-reconciliation |
| write `ENOSPC`/`EDQUOT` while streaming (`WriteFault::DeviceFull`) | 507 | `STORAGE_WRITE_FAILED` | retryable |
| `ENOSPC`/`EDQUOT` while placing (`StorageSite::Place`) | 507 | `STORAGE_FULL` | retryable |
| `EACCES`/`EPERM` (`PermissionDenied`, local) | 500 | `STORAGE_PERMISSION_DENIED` | fatal |
| `EIO` and other I/O (`WriteFault::Io`) | 503 | `STORAGE_UNAVAILABLE` | retryable |
| `IdleTimeout` | 408 | `TRANSFER_IDLE_TIMEOUT` | retryable |
| `Backpressure`, `CompletionBusy` | 503 | `TRANSFER_BACKPRESSURE` | retryable (waiting) |
| `LengthMismatch`, `RejectCause::LengthMismatch` | 422 | `UPLOAD_LENGTH_MISMATCH` | fatal |
| `QuotaError::Exceeded`, `RejectCause::QuotaExceeded` | 507 | `QUOTA_EXCEEDED` | fatal |
| part plan not computable | 413 | `FILE_TOO_LARGE` | fatal |
| `MultipartAlreadyCreated` | 409 | `MULTIPART_ALREADY_CREATED` | needs-reconciliation |
| `MultipartNotFound`, provider `NoSuchUpload` (`StorageSite::S3Multipart`) | 404 | `MULTIPART_NOT_FOUND` | terminal |
| `MultipartIncomplete` | 409 | `MULTIPART_INCOMPLETE` | needs-reconciliation |
| `MultipartAlreadyCompleted` | 409 | `MULTIPART_ALREADY_COMPLETED` | terminal |
| provider `InvalidPart` / `InvalidPartOrder` (`TransferError::MultipartPartRejected`) | 422 | `MULTIPART_PART_REJECTED` | needs-reconciliation |
| provider `EntityTooSmall` (`provider_part_too_small`), part number or size defects | 422 | `MULTIPART_PART_INVALID` | fatal |
| `PresignBatchTooLarge` | 422 | `PRESIGN_BATCH_TOO_LARGE` | fatal |
| provider 5xx, throttling, timeout (`ProviderFault::{Upstream, Timeout}`) | 502 | `STORAGE_UPSTREAM_ERROR` | retryable |
| provider unreachable (`ProviderFault::Unreachable`), storage down at admission | 503 | `STORAGE_UNAVAILABLE` | retryable |
| provider 403 / `AccessDenied` | 500 | `STORAGE_PERMISSION_DENIED` | fatal |
| other provider rejection (`StorageError::S3`) | 502 | `STORAGE_UPSTREAM_ERROR` | retryable (bounded) |
| `StorageError::SizeMismatch` | 500 | `STORAGE_SIZE_MISMATCH` | fatal |
| session unknown / not owned | 404 | `TRANSFER_SESSION_NOT_FOUND` | terminal |
| session terminal | 409 | `TRANSFER_SESSION_STATE_INVALID` | terminal |
| session past `expires_at` | 410 | `TRANSFER_SESSION_EXPIRED` | terminal |
| `Idempotency-Key` reused with another body | 409 | `IDEMPOTENCY_KEY_CONFLICT` | fatal |
| DTO or relative-path validation | 422 | `VALIDATION_ERROR` | fatal |
| more than 2 000 files | 422 | `BATCH_TOO_LARGE` | fatal |
| target folder unknown / foreign | 404 | `FOLDER_NOT_FOUND` | fatal |
| folder chain deeper than 64 | 422 | `FOLDER_DEPTH_EXCEEDED` | fatal |
| invalid name | 422 | `NAME_INVALID` | fatal |
| name generator exhausted | 409 | `FILE_NAME_CONFLICT` | fatal |
| writer busy past `busy_timeout` | 503 | `DATABASE_BUSY` | retryable |
| unclassified internal fault (`Invariant`, `Lifecycle`, `Time`) | 500 | `INTERNAL_ERROR` | fatal |

No classified cause returns `INTERNAL_ERROR`; the last row is the single fallback.

### Differences reconciled

Resolved through the source order (API_DESIGN §3 is the code and status authority, TRANSFER_ENGINE §9.1 assigns conditions):

| Difference | Resolution |
|---|---|
| TRANSFER_ENGINE §9.1 `STORAGE_SIZE_MISMATCH` is `502`; API_DESIGN §3.3.5 is `500`. | The catalogue wins: `500`. A planned size that differs from the measured `HeadObject` size stays `UPLOAD_LENGTH_MISMATCH` (TRANSFER_ENGINE §4.9, STORAGE_DESIGN §4.6, the M14-T07 ruling); `STORAGE_SIZE_MISMATCH` is the provider-level `StorageError::SizeMismatch`. |
| TRANSFER_ENGINE §9.1 listed `STORAGE_PERMISSION_DENIED` (`500`, fatal) while the catalogue had no such code, and the first M14-T11 pass reused `STORAGE_UNAVAILABLE` / `STORAGE_UPSTREAM_ERROR` with a fatal class for it. | That was a lossy decode: the persisted code reconstructed a retryable class. `STORAGE_PERMISSION_DENIED` is now catalogued (`500`, not retryable); `STORAGE_CORS_MISCONFIGURED` stays a health state, and a test asserts it is not an HTTP code. |
| §9.1 `InvalidPart`/`InvalidPartOrder` shared `MULTIPART_PART_INVALID` with fatal causes. | Split: `MULTIPART_PART_REJECTED` (reconcile first) and `MULTIPART_PART_INVALID` (fatal). |
| §3.9.5 lists `ENOSPC` at the final `rename` as `STORAGE_WRITE_FAILED`; §9.1 lists it as `STORAGE_FULL` for the copy. | `place` cannot tell a rename from the cross-device copy and only the copy allocates space, so placement uses `STORAGE_FULL` (the catalogue meaning: the device reported `ENOSPC`/`EDQUOT`). Streaming writes keep `STORAGE_WRITE_FAILED`. Both are `507` and retryable. |
| §9.1 provider `5xx`/timeout `502 STORAGE_UPSTREAM_ERROR` versus earlier code that answered `503 STORAGE_UNAVAILABLE` for every provider failure. | §9.1 is implemented: `5xx`, throttling and timeouts are `502`; only an unreachable provider is `503`. |
| §9.1 `REVERSE_SHARE_INACTIVE` is `403`; API_DESIGN §3.3.8 is `410`. | The catalogue wins (`410`). The `REVERSE_SHARE_*` codes are not in the Rust `ErrorCode` catalogue yet (M20). `unit_transfer_error_reverse_share_contract_awaits_its_catalogue_codes` records their status and class and fails when a code is added without a `RetryClass::for_code` binding. |
| `UPLOAD_CHECKSUM_MISMATCH` is `retryable: yes` in the catalogue and `needs-reconciliation` in §9.1. | Both hold: the catalogue flag says the same request may be resent, the class says the offset is reconciled (`HEAD`) first. |

## Durable errors

`transfer_session_files.error_code` / `error_request_id` and `transfer_sessions.error_code` / `error_request_id` already exist since migration `0001`; no migration is needed. The persisted code is a catalogue wire name, never a message; the public DTO is `error: {code, requestId}` with `requestId` required and nullable.

| Event | Item | Session |
|---|---|---|
| a classified failure (`fail_item`, finalization rejection, S3 lost-upload reaper) | `failed`, code and request id set in the same transaction as the state change, the quota share settlement and the session move | `error` set to this failure; the session moves to `failed` only when nothing is `pending`, `uploading` or `finalizing` |
| a sibling completes | untouched | untouched: a succeeding sibling never erases a failure |
| item retry (`failed → uploading`) | code and request id cleared | `error` recomputed from the items that are still `failed` (`repo::refresh_session_error`): their most recent failure, or `null` |
| item cancel | state change only | recomputed the same way |
| a request touches a session past `expires_at` | unchanged | `error = TRANSFER_SESSION_EXPIRED` with that request's id, written once and only while the session is not terminal; nothing is cancelled or released by the request (`observe_expiry`) |

A failure raised after the response (finalization triggered by `PATCH` or `…/complete`) keeps the id of the request that triggered it (`finalize_when_complete(item, request_id)`). A failure observed by a **background job** (the S3 lost-upload reconciler, a re-drive after restart) has **no request**: `error_request_id` is `NULL` and the DTO says `"requestId": null`. The server never invents an id. Persisted ids are validated (`RequestId::is_valid_text`, at most 128 characters of the accepted alphabet); an id that fails is dropped, never stored. The id comes from the request-id middleware, which accepts an inbound `X-Request-Id` only from a trusted proxy.

Recorded on the item and session: `FILE_TOO_LARGE` and `QUOTA_EXCEEDED` observed mid-stream, every finalization rejection (`UPLOAD_LENGTH_MISMATCH`, `QUOTA_EXCEEDED`, `FILE_TOO_LARGE`, `TRANSFER_SESSION_EXPIRED`, `AUTH_ACCOUNT_INACTIVE`, `FOLDER_NOT_FOUND`, `FOLDER_DELETING`, `FILE_NAME_CONFLICT`, `MULTIPART_NOT_FOUND`) and the retryable storage failures at placement. Request-level protocol answers (`409`, `423`, `460`, `408`, `503 TRANSFER_BACKPRESSURE`, `STORAGE_WRITE_FAILED` while streaming) and requests refused before their first byte (a known `Content-Length` above the planned size, a bad header) change no item: there is no accepted work to fail, and TRANSFER_ENGINE §3.4.7 keeps transport trouble and `STORAGE_WRITE_FAILED` out of T9.

## Upstream proxy `413`

A reverse proxy that rejects the body before forwarding it never reaches Palmr: there is no request, no request id and nothing to persist, and the server must not pretend otherwise. That case is the client's: the SPA synthesises `CLIENT_PROXY_BODY_LIMIT` (API_DESIGN §3.5), which the web retry table already classes `never`. What Palmr owns is its own refusal (`FILE_TOO_LARGE` on a body that overruns the planned or maximum size), which is correlated and persisted. The client-side synthesis and its persistence in IndexedDB belong to the M16 transfer engine; M14-T11 does not implement or claim them.

## Retry contract for the client engine (M16)

The typed contract is `Classification` / `RetryClass::for_code`; `unit_transfer_error_final_classes_never_enter_the_web_generic_retry_path` reads `apps/web/src/shared/api/retryPolicy.ts` and fails when a `terminal` or `fatal` server class is not `never` in the web table (and a vitest test pins the same set). The Transfer Center engine that consumes the classes is M16: this task adds no UI. The web retry table and the presentation map gained the two new codes (`never`; their toasts reuse existing message keys).

## Evidence for the contract

| Proof | Test |
|---|---|
| table of every §9.1 cause: code, status, class, details, no secret; one intended class per code | `unit_transfer_error_classification_table` |
| class is a function of the persisted code, over every cause | `regression_transfer_error_class_is_a_function_of_the_persisted_code` |
| seven causes (provider `5xx`/timeout, `403`, `EIO`/unreachable, `EACCES`, `InvalidPart`, `EntityTooSmall`, bad part number) persisted through `fail_item`, read through `GET` (session and item), a real stack restart, decoded to the intended class, request ids intact, quota and files unchanged | `it_transfer_error_class_survives_persistence_reload_and_restart` |
| the same refusals against real MinIO (fault-injected completion), each recoverable | `it_s3mp_each_provider_refusal_has_one_code_with_one_retry_policy` |
| real `EACCES` on read-only storage, durable across a real application restart, retry recovers | `it_transfer_error_local_permission_failure_is_durable_and_retry_recovers` |

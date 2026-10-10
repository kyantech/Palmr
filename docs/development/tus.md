# TUS protocol surface (local uploads)

Introduced in M14-T03. It implements TRANSFER_ENGINE §3.1 to §3.4 and API_DESIGN §2.15.1: capability discovery, resource creation with `creation-with-upload`, offset discovery and termination for the `local` provider. The general `PATCH` loop (M14-T04), finalization (M14-T05), reapers (M14-T06), S3 multipart and the anonymous Reverse Share mount (M20) are not part of it.

| Artefact | Location |
|---|---|
| Module | `apps/server/src/features/transfers/tus` (`routes`, `service`, `repo`, `headers`, `metadata`, `append`, `error`) |
| Staging seam | `apps/server/src/storage/staging.rs` (`StagingStorage`, `StagingAppend`) implemented by `storage/local/staging.rs` |
| Routes | `OPTIONS/POST /api/v1/uploads/tus`, `HEAD/DELETE/POST /api/v1/uploads/tus/{id}` (the last `POST` is the `X-HTTP-Method-Override` form) |
| Integration tests | `apps/server/src/features/auth/flow_tests/tus*.rs` |
| Third-party client | `tests/tus-conformance` (`tus-js-client`, pinned) against a real `palmr` process; CI job `tus-conformance` |

## What a TUS resource is

A `tus_uploads` row bound to **one planned item** of an existing transfer session, identified by `transferSessionId` + `itemId` in `Upload-Metadata`. Creation reuses everything admission already decided: destination, final object identity and the single quota reservation. It creates no `storage_objects` row, no `files` row and no second hold. The upload id is a fresh UUIDv7; staging is `uploads/<id as 32 hex>/blob` plus a `meta.json` hint. Nothing a client sends reaches a path, a header or an object key.

## Creation order and recovery

1. read-only pre-check (a repeated, compatible `POST` never takes the write lock);
2. one write transaction re-validates and inserts the row, moves the item `pending → uploading` and the session `created → uploading`;
3. after the commit, the staging directory, blob and hint are created idempotently.

The durable intent comes first because every reaper in the plan acts on rows. A crash or I/O error after step 2 leaves a row with no blob: `HEAD` reports offset 0, a repeated `POST` heals the staging, and an abandoned row ages out through the expiry reaper. No filesystem call is made inside a transaction (`it_tus_create_recovers_from_staging_and_database_failures` observes this with a probe that needs the write connection).

## `creation-with-upload` and the bounded loop

`append.rs` is the loop M14-T04 reuses. It copies each frame through one `PALMR_UPLOAD_BUFFER_BYTES` buffer, flushes every 8 MiB or 2 s (sync, then one short transaction that persists the offset and renews the 60 s lease), applies a per-frame idle timeout (`408 TRANSFER_IDLE_TIMEOUT`, never a whole-transfer deadline), and runs in a spawned task so a client disconnect still persists the final offset and releases the lease. Ceilings are applied before a byte is accepted: the declared length, the effective maximum for a deferred length, and the owner's quota headroom for items whose size was never reserved. Bytes past a ceiling are not stored.

When the body fills `Upload-Length` the row is `in_progress` with `upload_offset = upload_length` and the item stays `uploading`. That predicate is the finalizer's input (M14-T05); nothing in this task reports a stored file.

## Termination while a body is streaming

DELETE and the streaming request coordinate only through durable state. Every offset write is `UPDATE … WHERE state IN ('created','in_progress') AND locked_by = <holder>`, so after the termination commits, a still-running writer matches no row: it persists nothing, ends `410`, and cannot revive the upload or its item. It may still write into its already-open, already-unlinked file until its next checkpoint (bounded by one flush window), which is unreachable and freed when the stream ends. Because staging is created after the row commit, creation re-reads the row after its filesystem work and removes what it created if the upload was terminated in between (`it_tus_delete_between_row_commit_and_staging_leaves_no_staging`, `it_tus_delete_during_creation_stream_stops_writer`).

## Errors

Every failure carries the TUS status, `Tus-Resumable`, `Cache-Control: no-store`, `X-Request-Id` and the Palmr envelope (a `HEAD` response has no body by HTTP). Unknown and foreign uploads are the same `404 NOT_FOUND`; an owned upload that expired or was terminated is `410 UPLOAD_SESSION_EXPIRED`. `Upload-Concat` is `501 TUS_EXTENSION_UNSUPPORTED` before anything is parsed. A header problem is `400 UPLOAD_METADATA_INVALID` with `details.key`.

## Running the third-party client locally

```sh
cargo build --package palmr-server --bin palmr
PALMR_BIN="$PWD/target/debug/palmr" pnpm --filter @palmr/tus-conformance test
```

The suite covers only the surface above. The full TUS 1.0 suite (`PATCH`, offset mismatch, locking, checksum) is recorded as `it.todo` until M14-T04 lands.

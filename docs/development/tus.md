# TUS protocol surface (local uploads)

Introduced in M14-T03 and completed by M14-T04; M14-T05 connects it to finalization (`finalization.md`). It implements TRANSFER_ENGINE §3.1 to §3.9 and API_DESIGN §2.15.1: capability discovery, resource creation with `creation-with-upload`, offset discovery, the streaming `PATCH`, termination, the two-layer lease and offset reconciliation for the `local` provider. Reapers (M14-T06), S3 multipart and the anonymous Reverse Share mount (M20) are not part of it.

| Artefact | Location |
|---|---|
| Module | `apps/server/src/features/transfers/tus` (`routes`, `service`, `patch`, `repo`, `headers`, `metadata`, `append`, `lease`, `permits`, `reconcile`, `error`) |
| Staging seam | `apps/server/src/storage/staging.rs` (`StagingStorage`, `StagingAppend`) implemented by `storage/local/staging.rs` |
| Routes | `OPTIONS/POST /api/v1/uploads/tus`, `HEAD/PATCH/DELETE/POST /api/v1/uploads/tus/{id}` (the last `POST` is the `X-HTTP-Method-Override` form) |
| Integration tests | `apps/server/src/features/auth/flow_tests/tus*.rs` (in-process), `apps/server/tests/it_tus_patch.rs` (real TCP, real shutdown and restart, generated data) |
| Third-party client | `tests/tus-conformance` (`tus-js-client`, pinned) against a real `palmr` process; CI job `tus-conformance` |

## What a TUS resource is

A `tus_uploads` row bound to **one planned item** of an existing transfer session, identified by `transferSessionId` + `itemId` in `Upload-Metadata`. Creation reuses everything admission already decided: destination, final object identity and the single quota reservation. It creates no `storage_objects` row, no `files` row and no second hold. The upload id is a fresh UUIDv7; staging is `uploads/<id as 32 hex>/blob` plus a `meta.json` hint. Nothing a client sends reaches a path, a header or an object key.

## Creation order and recovery

1. read-only pre-check (a repeated, compatible `POST` never takes the write lock);
2. one write transaction re-validates and inserts the row, moves the item `pending → uploading` and the session `created → uploading`;
3. after the commit, the staging directory, blob and hint are created idempotently.

The durable intent comes first because every reaper in the plan acts on rows. A crash or I/O error after step 2 leaves a row with no blob: `HEAD` reports offset 0, a repeated `POST` heals the staging, and an abandoned row ages out through the expiry reaper. No filesystem call is made inside a transaction (`it_tus_create_recovers_from_staging_and_database_failures` observes this with a probe that needs the write connection).

## `creation-with-upload` and the bounded loop

`append.rs` is the one loop shared by `POST` (creation-with-upload) and `PATCH`. It copies each frame through one `PALMR_UPLOAD_BUFFER_BYTES` buffer, flushes every 8 MiB or 2 s (sync, then one short transaction that persists the offset and renews the 60 s lease), applies a per-frame idle timeout (`408 TRANSFER_IDLE_TIMEOUT`, never a whole-transfer deadline), and runs in a spawned task so a client disconnect still persists the final offset and releases the lease. Ceilings are applied before a byte is accepted: the declared length, the effective maximum for a deferred length, and the owner's quota headroom for items whose size was never reserved. Bytes past a ceiling are not stored.

When the body fills `Upload-Length` the row is `in_progress` with `upload_offset = upload_length` and the item is `uploading`; that predicate is the finalizer's input and the completion of the request starts it (see the boundary below).

## `PATCH`

`PATCH /api/v1/uploads/tus/{id}` (and the `X-HTTP-Method-Override: PATCH` form of the resource `POST`) is a streamed byte path: authenticated, CSRF-checked, `rl.transfer.data`, no body limit, no whole-request deadline, no compression. It requires `Tus-Resumable: 1.0.0`, `Content-Type: application/offset+octet-stream` and `Upload-Offset`; `Upload-Checksum: sha256 <base64>` and, for an upload created with `Upload-Defer-Length`, `Upload-Length` are optional. Malformed headers fail before any byte is read. Success is `204` with the durable `Upload-Offset`, `Upload-Expires` and `Tus-Resumable`, never a body.

The request runs in this order inside one spawned task (so a client that disconnects still persists its offset and releases its lease):

1. a `TUS_PATCH_CONCURRENCY = 64` permit, waited for at most `PERMIT_WAIT` (5 s), otherwise `503 TRANSFER_BACKPRESSURE` with `Retry-After`;
2. the in-process per-upload lock (`lease.rs`), taken with `try_lock`: a second writer in this process is `423 UPLOAD_LOCKED` at no database cost; entries are removed when the last reference drops;
3. the database lease: one `BEGIN IMMEDIATE` conditional `UPDATE` that proves the owner, a live `created`/`in_progress` upload, a `uploading` item, a live session and a free or expired lease (`locked_by IS NULL OR lock_expires_at <= now`), sets `locked_by`/`lock_expires_at` (60 s) and extends `expires_at`. A request never takes over a holder that has not expired, whatever its instance id. After a crash the lease is released by the startup reconciliation instead (see *Startup recovery* below), so the first `PATCH` does not wait for the 60 s lease;
4. reconciliation (`reconcile.rs`): `authoritative = min(recorded, fstat(blob))` and an unconditional `ftruncate` to it, then a downward correction of a phantom offset in a short transaction, before any byte is accepted. `HEAD` computes the same minimum and never writes;
5. the client offset must equal the authoritative offset, otherwise `409 UPLOAD_OFFSET_MISMATCH` with the true `Upload-Offset` and `details.expectedOffset`, and not a byte is read;
6. a deferred length is fixed by an `Upload-Length` that is not below the offset or above the maximum and that never changes an already fixed length (`422 UPLOAD_LENGTH_MISMATCH`, `413 FILE_TOO_LARGE`); no second reservation is made;
7. the loop of `append.rs`.

### Checkpoints and the lease

A checkpoint is `fdatasync` of the staging file, then one short transaction that persists the offset, renews the lease and extends the expiry. It happens every 8 MiB or 2 s, whichever comes first. The 2 s trigger is a timer in the loop's `select!`, not a check made when a frame arrives, so a small buffered tail becomes durable even if the client sends nothing more. While a connected client is idle the same timer renews the lease every `lease_ttl / 3`, so a live writer never loses its lease inside the 60 s idle timeout. Every write of an offset is `WHERE state IN ('created','in_progress') AND locked_by = <holder>`: after a termination or a lost lease a writer matches no row, persists nothing and ends `410` or `423`.

### `Upload-Checksum`

The digest is accumulated over the bytes of the request body only, never over the file. A request that carries one persists no offset at its checkpoints: the staging file is synced and the lease renewed, but `upload_offset` stays where it was before the request and advances only after the whole body matched. On a mismatch, an idle timeout, a disconnect, a shutdown or any other early end the staging file is truncated back to the offset the request started from and the response never claims more. **Recovery exception.** The 8 MiB / 2 s recovery bound applies to a `PATCH` without `Upload-Checksum`. A checksummed request is one verification unit: if it ends before verification (crash, disconnect, idle timeout, shutdown, mismatch) the whole request body is retransmitted, so no bounded retransmission window is claimed for a single large checksummed request. A client gets a smaller window by sending smaller checksummed requests; the server imposes no size limit on a valid request and persists no SHA-256 midstate. Crash recovery is unchanged because `min(recorded, fstat)` removes the unverified tail. A per-request digest is never stored as a whole-file digest (`client_checksum` stays `NULL`, and finalization leaves `storage_objects.checksum` `NULL` because no whole-file digest is verified).

### Shutdown

The application cancels one token before it stops accepting connections. Each streaming request stops at its next safe boundary (after the current bounded write), drains its buffer, syncs, persists the exact offset, releases its lease and answers `204` with that offset; a new `PATCH` is refused with `503 SERVICE_UNAVAILABLE`. A grace expiry or a kill leaves the last checkpoint, the unproven tail is removed by the next reconciliation, and the lease expires on its own.

### Startup recovery

`tus.release_orphan_leases` is registered in `ReconcileRegistry::production()` (`app/reconcile.rs`). It runs after the exclusive instance/data-dir lock is acquired and before the listener serves, in one short batched write transaction: `locked_by`/`lock_expires_at` become `NULL` on `created`/`in_progress` rows whose holder is not this instance. The lock proves the previous single-host writer is gone; a second live process on the same data directory cannot start, so it cannot run this step against a live writer (`it_tus_startup_recovery_never_runs_against_a_live_instance`). Nothing else changes: no offset, no staging byte, no session, no reservation, no state; no filesystem I/O. It is not a reaper (expiry and staging cleanup are M14-T06). Tests: `it_startup_reconcile_releases_orphan_tus_leases_and_nothing_else` (unit, pins the exact columns and idempotency), the `it_tus_startup_*` integration tests, and `it_tus_real_process_crash_recovers_without_a_lease_wait`, which `SIGKILL`s a real `palmr`, restarts it on the same data directory and resumes immediately.

Known gap: the canonical audit catalogue has no action for a downward offset repair, so the repair is a structured `WARN` diagnostic only; the audited action is left for a later convergence.

### The boundary with finalization

When the offset reaches the length the request ends exactly as before (`204`, `Upload-Offset`, no JSON body) and starts a detached finalization of the item. The same happens for a zero-byte upload, for a repeated `POST` of an already complete upload and after an item retry; a startup step covers anything a crash interrupted. `204` is a statement about bytes only: the file exists once the item is `completed` and the session view carries its `fileId`. While the item is `finalizing` or `completed`, `HEAD` reports the recorded offset (the full length) instead of reconciling it with the staging file, which has moved. Further `PATCH` requests on such an upload are `409 TRANSFER_SESSION_STATE_INVALID`; `DELETE` of a completed upload is `204` and never removes the file. The per-request `Upload-Checksum` digest is still never stored as a whole-file digest (`client_checksum` stays `NULL`).

## Termination while a body is streaming

DELETE and the streaming request coordinate only through durable state. Every offset write is `UPDATE … WHERE state IN ('created','in_progress') AND locked_by = <holder>`, so after the termination commits, a still-running writer matches no row: it persists nothing, ends `410`, and cannot revive the upload or its item. It may still write into its already-open, already-unlinked file until its next checkpoint (bounded by one flush window), which is unreachable and freed when the stream ends. Because staging is created after the row commit, creation re-reads the row after its filesystem work and removes what it created if the upload was terminated in between (`it_tus_delete_between_row_commit_and_staging_leaves_no_staging`, `it_tus_delete_during_creation_stream_stops_writer`).

## Errors

Every failure carries the TUS status, `Tus-Resumable`, `Cache-Control: no-store`, `X-Request-Id` and the Palmr envelope (a `HEAD` response has no body by HTTP). Unknown and foreign uploads are the same `404 NOT_FOUND`; an owned upload that expired or was terminated is `410 UPLOAD_SESSION_EXPIRED`. `Upload-Concat` is `501 TUS_EXTENSION_UNSUPPORTED` before anything is parsed. A header problem is `400 UPLOAD_METADATA_INVALID` with `details.key`.

## Running the third-party client locally

```sh
cargo build --package palmr-server --bin palmr
PALMR_BIN="$PWD/target/debug/palmr" pnpm --filter @palmr/tus-conformance test
```

The suite drives `tus-js-client` (chunked `PATCH`, one long `PATCH`, a streamed body of deferred length, resume after an interrupted request, the checksum header through `onBeforeRequest`, termination) and raw requests for the cases a client library hides (`409`, `423`, `460`, `413`, header validation, method override). No test is skipped or recorded as a todo.

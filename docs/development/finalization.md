# Upload finalization (local and S3)

Introduced in M14-T05 for local TUS uploads and extended by M14-T07 for S3 multipart (see [s3-multipart.md](./s3-multipart.md)). It turns a completed upload into a durable, user-visible file and defines the one metadata transaction that every finalizer shares (the Received finalizer of M20-T06 reuses it too). It implements TRANSFER_ENGINE §3.9 to §3.10, DATABASE_SCHEMA §5.10 and §5.12, and STORAGE_DESIGN's local placement.

| Artefact | Location |
|---|---|
| Module | `apps/server/src/features/transfers/finalize/{service,local,s3,commit,repo,hooks}.rs` (`local.rs` the TUS half, `s3.rs` the S3 half) |
| Storage primitives | `StagingStorage::{measure, inspect, staged_matches, place}` (`apps/server/src/storage/staging.rs`, `storage/local/{inspect,finalize,staging}.rs`) |
| Audit action | `FILE_UPLOADED` (`features/audit`), recorded inside the commit transaction |
| Startup step | `transfers.finalize_redrive`, registered last in `ReconcileRegistry::production()` |
| Flow tests | `apps/server/src/features/auth/flow_tests/finalize*.rs` |
| Process tests | `apps/server/tests/it_finalize.rs`, `it_finalize_process.rs` (feature `e2e-fixture`) |

No job kind and no migration was added. There is no public finalize endpoint: local finalization starts from the TUS request, S3 finalization from `POST …/s3/multipart/complete`, and both are recovered by the same re-drive.

## What starts a finalization

| Trigger | Where |
|---|---|
| The body of a `PATCH` or of `POST` with `creation-with-upload` leaves `upload_offset = upload_length` | `tus/patch.rs`, `tus/service.rs` |
| A zero-byte upload is created (`Upload-Length: 0`), with or without a body | `tus/service.rs` |
| A repeated `POST` for an item whose upload is already complete | `tus/service.rs` |
| `POST /transfers/sessions/{id}/files/{itemId}/retry` | `transfers/service.rs` |
| Startup, for every `finalizing` item and every `uploading` item whose bytes are complete | `app/reconcile.rs`, `Finalizer::redrive` |

A trigger spawns a detached task and the HTTP response is not delayed: the `PATCH` answers `204` with `Upload-Offset` equal to the length, and the item is `uploading`, then `finalizing`, then `completed` in the session view. `204` never means that file metadata exists; `GET /transfers/sessions/{id}` carries the resulting `fileId`. Background work is bounded (4 concurrent finalizations), tracked, cancelled by the shutdown token and drained during shutdown. A transient failure is retried in-task with a short backoff; anything beyond that is picked up by the next startup or an explicit retry.

The closed job-kind set has no finalization kind, so no job is enqueued: the durable item state is the queue.

### Dispatch durability

The detached task is only a latency optimization; losing it never loses an upload. The durable fact is `tus_uploads.upload_offset = upload_length` on an `in_progress` row of an `uploading` item, persisted (after `fdatasync`) before the request ends. Three recoveries read exactly that fact:

1. **Startup**: `transfers.finalize_redrive` runs the paginated candidate query (`repo::redrive_page`, 100 rows, keyset on the item id) and finalizes each candidate through the normal claim. It selects `finalizing` items and `uploading` items whose bytes are complete, only for `upload_kind = 'tus'`, only while the TUS row is `created`/`in_progress`, the session is `created|uploading|finalizing|failed`, is not `cancel_requested` and has not passed its `expires_at` (an expired session belongs to the reaper). Partial uploads, uploads of unknown final length, terminated or expired TUS rows, canceled, failed and completed items never match; a checksummed `PATCH` persists its offset only after verification, so unverified bytes never make an upload complete.
2. **While running**: a periodic sweep (every 5 minutes, `FinalizeLimits::sweep_interval`, one pass at a time, started after startup reconciliation and stopped by the shutdown token) runs the same query when it counts a candidate. It exists for a live process whose detached task gave up after its bounded retries.
3. **Explicit retry**: `POST …/retry` triggers the finalizer again.

**S3 items** have their own candidate query (`repo::s3_redrive_page`/`s3_redrive_count`): `upload_kind IN ('s3_multipart','s3_single')`, item `finalizing`, session `created|uploading|finalizing|failed`, not `cancel_requested`, not past `expires_at`. An S3 item that is only `uploading` is never finalized by the server: the browser's `complete` call is the trigger, exactly because the server cannot know the parts are final (`ListParts` is the truth). The same startup step and the same sweep drive both sets, TUS first.

Concurrent finalizers are safe because every path goes through the same claim (guarded item transition plus the TUS-row lease). `it_finalize_crash_before_detached_task_claim` proves the first recovery with a real process: the hook point `dispatched` parks the task before its claim, the process is `SIGKILL`ed, and the restart finalizes the upload exactly once. Removing the `uploading` branch of the query makes that test fail.

## Ownership

Exactly one logical finalization may act on an item. Three layers enforce it, and none of them is only in memory:

1. the durable item transition `uploading → finalizing` (guarded `UPDATE`, one row);
2. a lease on the item's `tus_uploads` row, acquired in the same transaction with the holder `<instance id>:finalize` and renewed by a heartbeat every 20 s while the work runs; a PATCH writer uses a different holder, so a finalizer can never take a writer's lease, and a startup crash is cleared by the existing orphan-lease step;
3. an in-process set that keeps a second finalizer in the same process from even trying.

A finalizer that finds the lease held waits for the owner's result (bounded) instead of acting. The lease is released on every error exit.

The session moves to `finalizing` only when no other item is still `pending` or `uploading`; otherwise sibling items could not start or resume.

## Order

```text
claim (one tx)          item finalizing, TUS lease, session maybe finalizing
fsync(staging)          sync_all
fstat                   authoritative size
validate                == upload_length, == declared, <= effective maximum
MIME                    <= 8 KiB prefix: sniff, extension, advisory filetype, fallback
pre-checks              not canceled/expired, account active, destination resolves
intent (one tx)         finalize_stage = placing        -- the key already belongs to the item
place                   rename, or the bounded EXDEV copy; fsync the leaf directory
commit (one tx)         shared transaction, below
cleanup                 remove staging, best effort
```

No `fsync`, `fstat`, `rename`, copy, unlink, directory walk or MIME read runs while a write transaction is open (`TestStaging` probes the writer from inside every storage call).

The destination is `transfer_session_files.final_object_key`, allocated at admission. No key is derived from a name, no `storage_objects` row exists before the commit, and a replay never allocates a new identity.

## Durable stages

The schema has `finalize_stage ∈ {none, placing, committed}`; the finer crash boundaries are recovered from the filesystem, as TRANSFER_ENGINE §3.9.10 describes.

| Crash point | Durable state | Recovery |
|---|---|---|
| claim … MIME | `finalizing` / `none`, staging intact | re-measure and continue |
| intent … before place | `finalizing` / `placing`, staging intact | place again; a half EXDEV temp is discarded |
| after place … before commit | `finalizing` / `placing`, blob at the key, no object row | measure the blob at the key, run the commit |
| after commit | `completed` / `committed` | nothing |

If both staging and the key exist, the bytes are compared with a bounded streaming comparison (never size alone); identical bytes are adopted, different bytes are left untouched and the item fails.

## Authoritative size, MIME, checksum

The committed size is the `fstat` of the staged (or placed) file. `Upload-Length`, `declared_size_bytes`, `Content-Length` and browser progress are only compared. A mismatch is `UPLOAD_LENGTH_MISMATCH`; above the effective maximum is `FILE_TOO_LARGE`. Bytes that are missing entirely are a mismatch too: content is never invented. MIME comes from `domain::mime::resolve_mime` over at most 8 KiB. `storage_objects.checksum` stays `NULL`: the per-request `Upload-Checksum` digest is not a whole-file digest and no whole-file digest is computed.

## The shared commit transaction

`commit.rs` takes a provider-neutral `FinalizationProof` built only from server-side measurements (never deserialized) and runs one `BEGIN IMMEDIATE`:

1. re-read the item, session and protocol rows and bind the proof to the recorded identity;
2. a `completed` item returns its existing file id (idempotent re-entry);
3. re-check cancel, expiry, account, the effective maximum, the length and the destination folder chain (never created, never redirected to the root);
4. insert the `storage_objects` row (`id = final_object_id`, `active`, `refcount 1`, measured size);
5. insert `files` with keep-both naming against the real unique index;
6. `QuotaService::settle_item`: the item's own `reserved_bytes` share leaves the held reservation, the measured size enters `users.used_bytes`, margin and quota are checked;
7. complete the item and its TUS row, advance the session counters and close the session (and commit its reservation) when no item is live and none failed;
8. record `FILE_UPLOADED`.

A check that fails rolls everything back and a second transaction takes the rejection branch of DATABASE_SCHEMA §5.12: terminal causes tombstone-insert the placed object (when `finalize_stage = placing`), queue one `file_deletion_queue` row and one deduplicated `storage.delete_blob` job, terminate the TUS row, release and zero the item's share and fail the item; retryable causes (storage I/O) keep the staged bytes, the share and the TUS row so a retry costs no retransmission.

| Cause | Code | Retryable |
|---|---|---|
| size mismatch, missing bytes | `UPLOAD_LENGTH_MISMATCH` | no |
| above the maximum | `FILE_TOO_LARGE` | no |
| quota or margin | `QUOTA_EXCEEDED` | no |
| session expired | `TRANSFER_SESSION_EXPIRED` | no |
| account deactivated | `AUTH_ACCOUNT_INACTIVE` | no |
| folder missing or being deleted | `FOLDER_NOT_FOUND`, `FOLDER_DELETING` | no |
| ENOSPC / EDQUOT | `STORAGE_WRITE_FAILED` | yes |
| other storage failure | `STORAGE_UNAVAILABLE` | yes |
| foreign bytes at the key | `INTERNAL_ERROR` | yes (never overwritten) |

## Cancellation

The cancel transaction and the commit transaction serialize on the write lock, so the commit is the linearization point. Cancel before it: the cancel transaction already tombstone-inserted the key when the stage was `placing`, the finalizer finds the item `canceled` and does nothing; the pre-placement check skips the move altogether. Cancel after it: the file is completed content and is never removed by a cancel. Because the commit re-checks `cancel_requested` under the writer, TRANSFER_ENGINE §3.9.9's post-commit compensation has no state to act on and is not implemented. The EXDEV copy polls the item and its cancellation token every 64 MiB and stops at that boundary, leaving no temp object.

## Test seams

`FinalizeHooks` is an injectable observer called at ten points (`Point`); the flow tests use it for barrier-controlled races and simulated process death (`Flow::Crash` abandons the task with its durable state untouched). In a build with the `e2e-fixture` feature the application reads `PALMR_E2E_FINALIZE_PAUSE_AT` and `PALMR_E2E_FINALIZE_PAUSE_FILE` and parks at that point, which lets `it_finalize_process.rs` SIGKILL the real binary at a controlled boundary. Release builds contain neither.

## Known deferrals

- A crash between the commit and the staging cleanup leaves an empty staging directory; the staging sweep reclaims it.
- Expiry reapers, the periodic `tus.expire_stale` and the session reaper are implemented in M14-T06 ([transfer-reaper.md](./transfer-reaper.md)); they never expire an item that is `finalizing`, or `uploading` with every byte received, before its session's 7-day bound.
- A permanent database fault during the commit leaves the item `finalizing` until the next sweep, start or retry.

## S3 multipart (M14-T07)

`finalize/s3.rs` implements the provider-specific half and calls the same `Committer` as the local path; only "measure the object", "read the MIME prefix" and "place the object" differ.

| Step | Local | S3 multipart | S3 zero-byte |
|---|---|---|---|
| ownership | TUS-row lease | in-process active set + durable claim/intent transitions | same |
| claim tx | `uploading → finalizing` | `uploading → finalizing` after a clean `ListParts` | `pending → finalizing` |
| intent tx | `finalize_stage = placing` | `placing` and multipart `completing` in one statement pair | `placing` |
| place | `rename` / EXDEV copy | `CompleteMultipartUpload` (strictly ascending manifest from `ListParts`) | server `PutObject` of an empty body |
| measure | `fstat` | `HeadObject` | `HeadObject` |
| MIME | ≤ 8 KiB file prefix | ≤ 8 KiB ranged `GetObject` | empty prefix, extension decides |
| commit | shared | shared; also `s3_multipart_uploads → completed` and `final_etag` | shared |

`check_size` in the commit takes the TUS length for local items and the declared size for S3 items; the measured size is always the proof's. A terminal rejection also moves the multipart row to `abandoned`; a rejection that happens before anything was placed additionally aborts the still-open provider upload at once (best effort; the abandoned-upload cleanup is the guarantee). `RejectCause::MultipartLost` (`MULTIPART_NOT_FOUND`) covers an upload the provider no longer knows without an object at the key.

Crash windows: before the intent the item is `finalizing/none` or `uploading` and the multipart `in_progress` (nothing was placed); after the intent and before the provider answer the multipart is `completing` and the object may or may not exist; after the provider answer and before the commit the object exists and the item is `finalizing/placing` (the `before_commit` pause point); after the commit nothing is owed. The re-drive probes `HeadObject` of the recorded key first, so each window converges to exactly one file.

Test seams: the existing hook points apply (`dispatched`, `claimed`, `intent_committed`, `before_place`, `after_place`, `measured`, `mime_resolved`, `before_commit`, `after_commit`); `PALMR_E2E_FINALIZE_PAUSE_AT=before_commit` pauses after the provider completed and before the metadata commit, which `it_s3_finalize_process.rs` uses to `SIGKILL` a real `palmr` against a real MinIO.

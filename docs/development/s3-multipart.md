# S3 multipart control plane

Introduced in M14-T07. It implements TRANSFER_ENGINE §4.1 to §4.13, API_DESIGN §2.15.2 and DATABASE_SCHEMA §5.11 for authenticated My Files uploads: Palmr authorizes, allocates identity, plans parts, signs, reconciles, completes and commits. The upload bytes travel **browser → S3**; Palmr moves none of them, except in the degraded mode of [Server-proxied parts](#server-proxied-parts-m14-t08) (M14-T08), where the profile requires a checksum a browser cannot compute. The abandoned-upload reapers and the provider orphan sweep (M14-T09), directory trees (M14-T10) and the public Reverse Share mount (M20) are not part of it.

| Artefact | Location |
|---|---|
| Module | `apps/server/src/features/transfers/s3/{routes,service,reconcile,repo,abort,model}.rs` |
| Finalization (S3 half) | `apps/server/src/features/transfers/finalize/s3.rs`, shared commit transaction in `finalize/commit.rs` |
| Storage seam | `TransferStorage::{multipart, provider_handle, bucket, multipart_ttl}` over the existing `MultipartStorage` / `PresignStorage` capabilities of `storage/s3` |
| Routes | `POST …/s3/multipart`, `POST …/presign`, `POST` and `GET …/parts`, `POST …/complete`, `POST …/abort`, all under `/api/v1/transfers/sessions/{sid}/files/{itemId}`; plus the degraded-mode `PUT …/parts/{partNumber}` |
| Policy | `authenticated`, CSRF, `rl.transfer.control`, `X-Request-Id`, control-plane deadline and body limit (the `PUT` part route is a streamed byte path with `rl.transfer.data`, see below) |
| In-process tests | `apps/server/src/features/auth/flow_tests/s3_multipart*.rs` (real MinIO container, real presigned `PUT`s) |
| Process test | `apps/server/tests/it_s3_finalize_process.rs` (feature `e2e-fixture`: real `palmr`, real MinIO, `SIGKILL`) |

## What the browser can and cannot say

There is no key, bucket, upload id or path in any request. Every route is addressed by `(sessionId, itemId)`; the handler resolves the item, checks that the caller owns it (otherwise `404`, indistinguishable from an unknown id) and derives everything else from the row: the object key allocated at session admission, the bucket from the operator configuration, the provider upload id from `s3_multipart_uploads`. A part number, an `ETag` and a size are the only caller-supplied values. `multipartId` in the create response is Palmr's handle, not the provider's upload id; no response, error or log carries a key, bucket, upload id or credential (a presigned URL naturally carries the key in its path and the access key id in its credential; it is a short-lived scoped capability, never stored or logged).

Only an admitted S3 session can use these routes. A local item answers `409 TRANSFER_SESSION_STATE_INVALID` on create and `404 MULTIPART_NOT_FOUND` elsewhere; a zero-byte item (`s3_single`) answers the same, because it has no multipart upload.

## Create

`POST …/s3/multipart`, in this order, with no write transaction open while S3 is called:

1. read-only authorization and lifecycle checks (provider, kind, item state, session state and expiry, an existing row is `409 MULTIPART_ALREADY_CREATED`);
2. the part plan from `plan_parts` through the existing planner (`TransferStorage::part_layout`), re-checked against the current effective maximum file size and provider limits (`413 FILE_TOO_LARGE`), account active and destination folder resolvable;
3. `CreateMultipartUpload` on the **internal** client against the key recorded on the item;
4. one write transaction that re-validates, inserts the `s3_multipart_uploads` row, inserts every part row in one recursive-CTE statement, moves the item `pending → uploading` and the session `created → uploading`;
5. the response: `multipartId`, `partSizeBytes`, `partCount`, `maxPresignBatch = 16`, `presignTtlSeconds = 900`, `checksumHeadersRequired`.

Concurrent creates race on `ux_s3mp_tsf`: the winner commits, every loser aborts its own provider upload and answers `409 MULTIPART_ALREADY_CREATED`. If the provider call succeeds and the database step fails, the upload is aborted immediately (best effort) and the error is returned; no success is claimed. **A crash between the provider call and the insert leaves a provider-side orphan that is never adopted.** Reclaiming it is the `ListMultipartUploads` sweep of M14-T09 (STORAGE_DESIGN: MinIO lists incomplete uploads only for an exact object key, which that task has to account for).

An unknown declared length cannot be planned: create answers `422 MULTIPART_PART_INVALID` with `details.reason = unknown_length_requires_server_mediated_upload`. When the storage profile requires checksum headers (`UploadPlanner::is_proxied`, from the profile or the structured self-test, never from an error string) create succeeds with `checksumHeadersRequired: true` and the pinned 8 MiB plan, presign refuses (below) and parts go through the proxied route of M14-T08. The unknown-length case stays refused: nothing pretends a presigned plan can serve it.

## Presign

`POST …/presign {"partNumbers":[…]}` signs one `UploadPart` per number with the **public** signer client (`PALMR_S3_PUBLIC_ENDPOINT`, else the internal endpoint). Server-side calls never use that client.

* at most 16 numbers (`422 PRESIGN_BATCH_TOO_LARGE`, nothing is signed); empty or malformed bodies are `422 VALIDATION_ERROR`; a number outside `1..=partCount`, a repeated, non-integer, negative or oversized number is `422 MULTIPART_PART_INVALID`;
* TTL 900 s, never longer; `expiresAt` is the earliest expiry the provider signed;
* the URL signs only `host`: no `x-amz-checksum-*`, no `x-amz-sdk-checksum-algorithm`, no `Content-Type` (asserted by the provider on every URL and again by `it_s3mp_presign_batch_limits`);
* any number can be signed again at any time, any number of times. A URL is never stored: the database keeps only `signed_at`, `signed_expires_at` and an attempt counter, and nothing is cached (`regression_R091_presigned_url_lifetime` also proves that the provider rejects an expired URL and accepts the re-signed one);
* with `checksumHeadersRequired` the answer is `422 MULTIPART_PART_INVALID` with `details.reason = server_proxied_part_upload` and no URL is signed.

## Part reports are hints

`POST …/parts` stores up to 1 000 `{partNumber, etag, sizeBytes}` entries as `uploaded` hints and answers `204`. The part number must belong to the plan, the size must equal the planned size of that part, the `ETag` is 1 to 128 non-control characters kept verbatim (quotes included). A repeated report is safe; a conflicting duplicate in one request is `422`. A hint finalizes nothing: completion never reads it.

## ListParts is the truth

`GET …/parts` calls `ListParts` through every provider page (the provider caps pages at 1 000 and 10 000 parts overall; the memory bound is one metadata vector of at most 10 000 entries), analyses the result against the plan, writes the outcome back to the cache and answers from the **provider's** data: `completedParts` (with the provider's sizes), `missingPartNumbers`, `uploadedBytes = Σ provider sizes`, `partSizeBytes`, `partCount`. A held part of the wrong size is reported as held with its real size; completion refuses it.

The cache write is one short transaction that runs after the network call and is guarded by `state IN ('created','in_progress')` on the multipart row, so a snapshot can never resurrect an aborted, abandoned, completing or completed upload (the caller then gets the error that describes the current state). Present parts become `verified` with the provider's `ETag` and size; recorded parts the provider does not hold return to `planned` (planned size restored) **only if** their hint is not newer than the moment the snapshot was requested. The multipart expiry is extended by `PALMR_S3_MULTIPART_TTL_HOURS` on every part-level request.

## Completion

`POST …/complete` has no body and is crash-safe. The work runs in a tracked detached task (`Finalizer::complete_s3`) so a client that disconnects or a proxy that times out cannot interrupt it half way; the handler awaits a channel.

```text
authorize, gate           item owned, S3 kind, not canceled or expired
ListParts                 every page (network, no transaction)
analyze                   missing -> 409 MULTIPART_INCOMPLETE {missingPartNumbers}
                          wrong size -> 422 MULTIPART_PART_INVALID
cache write               short guarded transaction
claim (one tx)            item finalizing, session maybe finalizing
pre-checks                size policy, account, expiry, destination folder
intent (one tx)           item finalize_stage = placing, multipart completing
CompleteMultipartUpload   strictly ascending ETags built from ListParts, never from the client
HeadObject                authoritative size
ranged GET 0-8191         MIME, one bounded read
commit (one tx)           shared M14-T05 transaction
```

The intent transaction is the durable ownership record: once it commits, a restart can tell "still uploading" (`in_progress`) from "completion intended" (`completing` + `placing`) from "committed" (`completed` + `committed`). No S3 call, and no filesystem call, ever runs inside a transaction; `it_s3mp_*` observe this with a probe that needs the writer from inside every provider call.

### Provider answers

| Provider outcome | Result |
|---|---|
| success | `HeadObject`, MIME, commit |
| `InvalidPart`, `InvalidPartOrder`, `EntityTooSmall` | the intent is reverted (item back to `uploading`, multipart `in_progress`), `422 MULTIPART_PART_INVALID`; nothing was placed |
| `NoSuchUpload` and the final key **exists** | the completion already happened; continue with `HeadObject` |
| `NoSuchUpload` and no object | the item fails with `MULTIPART_NOT_FOUND`, the key is tombstone-inserted, the share released |
| timeout, 5xx, connection error, an XML error inside `200` | `503 STORAGE_UNAVAILABLE` or `502 STORAGE_UPSTREAM_ERROR`; durable state is kept and a retry resumes |

The SDK-level parse of a `200` response with an `<Error>` body is the M07-T07 behaviour and is classified like any other service error; Palmr never reads HTTP 200 alone as success.

### Measured size

The committed size is the `HeadObject` content length of the key recorded on the item. The declared size, the part plan and the client progress are only compared. A mismatch is `422 UPLOAD_LENGTH_MISMATCH`, above the effective maximum `413 FILE_TOO_LARGE`, over quota `507 QUOTA_EXCEEDED`; each takes the DATABASE_SCHEMA §5.12 rejection branch: the placed object is inserted as `tombstoned`, one deletion job is queued, the share is released and no `files` row exists. The `ETag` of the completed object is stored in `final_etag` only for diagnostics and is never treated as a digest; `storage_objects.checksum` stays `NULL`.

### Re-drive

`Finalizer::redrive` (startup step `transfers.finalize_redrive` and the 5-minute sweep) now also pages `s3_multipart` and `s3_single` items in `finalizing`. A re-drive of an item whose multipart is `completing` first asks `HeadObject(final_object_key)`: an existing object is adopted and committed without a second `CompleteMultipartUpload` and without listing parts; a missing object is completed again from `ListParts`. A browser retry is not needed. The recorded `final_object_id` and key are reused; no replacement is ever allocated. Re-driving an item whose parts vanished fails it with `MULTIPART_NOT_FOUND` rather than adopting anything.

Startup never aborts a live multipart upload: no abort call exists on any startup path (T09 owns the age-guarded cleanup).

### Zero-byte items

A zero-byte file is `upload_kind = s3_single` and has no multipart upload. `POST …/complete` claims the item (`pending → finalizing`), records the intent, writes the empty object with a server-side `PutObject` (skipped when `HeadObject` finds it already placed) and then runs the same measurement, MIME and commit as every other item: exactly one file, one active object of size 0, nothing charged.

## Abort

`POST …/abort` first runs the existing item cancellation transaction (`TransferService::cancel_item`): the item becomes `canceled`, its quota share is released, the multipart row becomes `abandoned` (the durable cleanup obligation) and a `TRANSFER_CANCELED` audit event is written, and a placed-but-uncommitted object is tombstoned. Only then does it call `AbortMultipartUpload`; `NoSuchUpload` counts as success and flips the row to `aborted`. If the provider call fails the row stays `abandoned`, the error is returned (`502`/`503`) and calling again retries; M14-T09's worker is the other consumer of `abandoned` rows. A committed item answers `409 MULTIPART_ALREADY_COMPLETED` and is never touched; aborting twice is `204`.

The two linearizations are decided by the guarded transitions of the commit and cancel transactions: abort first leaves no file, no object, no charge and a durable cleanup; completion first keeps the file and refuses the abort (`it_s3mp_abort_during_completion_leaves_no_file`, `it_s3mp_completion_wins_before_abort`).

## Error summary

`TRANSFER_SESSION_NOT_FOUND` (create) and `MULTIPART_NOT_FOUND` (every other route) for unknown, foreign and aborted uploads; `MULTIPART_ALREADY_CREATED`, `MULTIPART_ALREADY_COMPLETED`, `MULTIPART_INCOMPLETE`, `MULTIPART_PART_INVALID`, `PRESIGN_BATCH_TOO_LARGE`, `FILE_TOO_LARGE`, `QUOTA_EXCEEDED`, `STORAGE_UNAVAILABLE`, `STORAGE_UPSTREAM_ERROR`, `UPLOAD_LENGTH_MISMATCH`, `FILE_NAME_CONFLICT` (name exhaustion only), `TRANSFER_SESSION_STATE_INVALID`, `TRANSFER_SESSION_EXPIRED`. A completion that another request is still running answers `503 TRANSFER_BACKPRESSURE`.

## Server-proxied parts (M14-T08)

Implemented in `features/transfers/s3/proxied.rs`; design record in TRANSFER_ENGINE §4.7.1.

```text
PUT /api/v1/transfers/sessions/{sid}/files/{itemId}/s3/multipart/parts/{partNumber}
Content-Type: application/octet-stream      body: exactly the bytes of this part
-> 200 {"partNumber": 2, "etag": "\"…\"", "sizeBytes": 8388608}
```

| Property | Value |
|---|---|
| When | the capability `requires_checksum_headers` resolves to true. Precedence: the operator's `PALMR_S3_REQUIRE_PART_CHECKSUMS` (`true`/`false`, unset = no opinion) > the profile default (`r2` = true, others false) > a self-test relaxation, which only applies to a profile-sourced `true`. An explicit value is never overridden by a probe; nothing raises the flag automatically. No hostname, bucket, header or error text is ever consulted |
| Part size | pinned 8 MiB; every part but the last is exactly that. The **persisted** `part_size_bytes` decides what an upload may do: an upload planned with larger parts (created in direct mode before a profile change) answers `422 part_size_exceeds_proxy_buffer` and has to be aborted |
| Ceiling | `min(provider max_object, 8 MiB × (max_parts − safety margin), file-size policy)`; **83 047 219 200 bytes** by default (79 200 MiB = 77.34375 GiB). Refused with `FILE_TOO_LARGE`, `reason = proxy_part_count`, at session creation and again at multipart create, before any provider call. The earlier "79.2 GiB" was a unit slip |
| Middleware | authenticated, CSRF + Origin, `rl.transfer.data`, `RequestContent::OctetStream`; opts out of the JSON body cap, the whole-request deadline and compression (a `BytePath(Streamed, Identity, IdleOnly)` policy). only a 60 s idle gap ends the request (`TRANSFER_IDLE_TIMEOUT`); there is no total-duration limit |
| Concurrency | one `Semaphore(S3_PROXY_CONCURRENCY = 8)` per process, held from before the buffer is allocated until the part is recorded; a ninth request waits 5 s without reading its body, then `503 TRANSFER_BACKPRESSURE` + `Retry-After` |
| Memory | one `BytesMut::with_capacity(expected)` per admitted request, never grown; a frame that would pass the expected length ends the request before any byte reaches the provider. The buffer is moved into the provider call as an owned `std::io::Cursor`, so the SDK reads it in `PALMR_UPLOAD_BUFFER_BYTES` chunks (256 KiB by default) and no second full-size copy exists. 8 buffers = 64 MiB of application-owned part memory (not the process RSS) |
| Checksum | SHA-256 of exactly the received bytes, computed while receiving, sent as `x-amz-checksum-sha256` by `MultipartStorage::upload_part_checksummed` on the internal client. Presigned parts never sign a checksum |
| Part state | `uploaded` (ETag verbatim, size, `attempts + 1`) after the provider answered and only while the multipart is `created`/`in_progress`; never before. A lost answer leaves the row `planned` and the part present in `ListParts`, which stays the truth |
| Same part twice | exclusive: the second request answers `423 UPLOAD_LOCKED` while the first is in flight; a later request replaces the part |
| Provider errors | `NoSuchUpload` → `404`; checksum refusal (structured code: `BadDigest`, `InvalidDigest`, `XAmzContentSHA256Mismatch`, `XAmzContentChecksumMismatch`) → `502` with `details.reason = checksum_rejected`; timeout/5xx → `503`. No total-duration deadline: inactivity guards only (10 s connect, SDK minimum-throughput guard with 60 s grace on the request body, response wait = 60 s + the time the upload took, socket send buffer 256 KiB) |

Races: an abort that wins makes the in-flight part fail with `404` and leaves the row alone (a part the provider accepted after the abort is reclaimed by the M14-T09 sweep); a completion that starts first refuses the part with `409`, and completion only ever uses `ListParts`, so an in-flight part cannot enter the manifest half-written.

Observability: one startup `WARN` (`data_plane=proxied`, `part_size_bytes`, `max_concurrent_parts`, `max_object_bytes`); `/health/ready` adds `dataPlane` and, when proxied, `proxyCeilingBytes`. The Admin storage endpoint and panel are M22-T04 / M22-T07: until they land, `GET /health/ready` and the log are the only places the mode is visible. The storage self-test sends its server-side probe parts with the checksum while the profile still requires it.

Tests: `regression_381_provider_checksum_rejection` (real MinIO behind a layer that rejects a part without `x-amz-checksum-sha256`), `it_s3_proxied_part_memory_bounded`, `it_s3_proxied_ceiling_preflight`, `it_s3_proxied_part_http_semantics`, `it_s3_proxied_part_route_is_authenticated_scoped_and_stateful`, `it_s3_proxied_parts_retry_and_reconcile_with_provider_truth`, `it_s3_proxied_parts_race_abort_and_completion_safely`, `it_s3_proxied_part_plan_is_pinned_for_the_life_of_the_upload`, the real-process `it_s3_proxied_process_uploads_through_a_checksum_demanding_provider`, and the unit tests in `proxied.rs`.

## Deferrals

* Streaming of unknown-length uploads through the proxied path: with Reverse Share (M20).
* `s3.abort_abandoned_multipart`, `s3.reconcile_multipart`, the TTL reaper and the `ListMultipartUploads` orphan sweep: M14-T09. T07 writes `abandoned` and aborts directly; it enqueues no job.
* The public Reverse Share mount of the same operations and the Received target: M20.
* The Transfer Center client: M16.

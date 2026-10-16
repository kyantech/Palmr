# Authenticated single-file download

Introduced in M15-T01. It implements API_DESIGN §2.16 (`GET /api/v1/files/{id}/content`), STORAGE_DESIGN §3.6 and §4.5, ADR 0004 §7, ADR 0007 and ADR 0008 for My Files. It is the streaming foundation that the preview endpoint (M15-T02) and the archive stream (M15-T04) reuse.

| Artefact | Location |
|---|---|
| Module | `apps/server/src/features/downloads/{routes,service,model,error,stream}.rs` |
| Range and validator logic | `apps/server/src/domain/http_range.rs` (M07-T12, unchanged except `format_http_date`) |
| Header builder | `apps/server/src/domain/content_disposition.rs` (M07-T12, reused) |
| Sealed grant | `GrantContext::after_authorization()` in `apps/server/src/storage/provider/grant.rs` |
| Unit tests | `features/downloads/tests.rs`, `domain/http_range.rs` |
| Process tests (real `palmr`, TCP) | `tests/it_download_local_process.rs`; `tests/it_download_s3_process.rs` (real MinIO); `tests/it_download_s3_proxy_process.rs` (feature `e2e-fixture`, real MinIO) |

## Order of operations

1. Authenticate (`authenticated`, `rl.transfer.data`).
2. Parse the id. A malformed id is `404 FILE_NOT_FOUND`. Parse `disposition` (`attachment` default, `inline`); anything else is `422 VALIDATION_ERROR`. Unknown query members are ignored and never reach storage.
3. Resolve the file through one owner-scoped read: `files` joined to `storage_objects` where `owner_id` is the caller, the folder is not hidden by a deleting ancestor and `storage_objects.state = 'active'`. No row is `404 FILE_NOT_FOUND`, identical for unknown, foreign, deleted and mid-deletion files.
4. Check that `storage_objects.provider` equals the configured provider (`500 STORAGE_PROVIDER_MISMATCH`) and parse the stored key through `ObjectKey::parse`. The browser never supplies a key.
5. Only now touch storage.

Every conditional header is evaluated after step 3, so `Range`, `If-Range` and `If-None-Match` cannot probe another owner's file: a foreign id answers `404` whatever the headers say.

## Providers

| Capability | Behaviour |
|---|---|
| `as_presign()` is present (S3 default) | `302 Found` (unless a condition below needs Palmr) to a presigned `GET` signed against `PALMR_S3_PUBLIC_ENDPOINT`, TTL 300 s, `response-content-disposition` and `response-content-type` pinned into the signature, `Cache-Control: no-store`, the `Location` header marked sensitive. No `HeadObject` is issued for a plain request: the control plane stays O(0). |
| no presign capability (local, or an S3 profile that forbids presigned `GET`) | Palmr streams. `stat` first (authoritative size), then `open_read` or `open_range(start, len)`. Local uses `openat(O_NOFOLLOW)` + `seek` + `take`; S3 issues a real `GetObject` with `Range: bytes=start-end`. |

No profile forbids presigned `GET` today. The proxy path is therefore exercised by the `e2e-fixture` seam `PALMR_E2E_S3_PRESIGNED_GET=off`, which is read only from the environment of a binary built with that feature and is absent from the release build.

## Representation, validators and conditional requests

* `Content-Length` is the size the provider measures (`stat` and the opened handle), never a client-declared size. A disagreement between the recorded size, the measured size and the size reported by the opened handle is `500 STORAGE_SIZE_MISMATCH` and no file byte is written.
* `ETag` is strong. It is `"<sha256>"` when the finalizer recorded and verified a checksum, otherwise `"<size hex>-<mtime micros hex>"`. It never derives from the file name and never repeats the provider's own ETag.
* `Last-Modified` is the provider-reported modification time of the immutable object.
* `If-None-Match` is evaluated by weak comparison, after authorization and before `Range`; a match is `304` with `ETag`, `Last-Modified` and `Cache-Control`, no body, no object read. API_DESIGN §1.14 lists `304` for immutable assets; §1.16 names authenticated file content explicitly. §1.16 is the more specific statement and §1.14 is amended to match.
* `If-Range` follows RFC 9110 §13.1.5: strong ETag equality, or an exact `Last-Modified` date; a weak, stale, malformed or empty validator restarts the download with `200`. Several `Range` headers, or an unusable one, are ignored (`200`).
* Cache policy is always `private, no-cache`. No `public`, no shared-cache eligibility, no `Content-Encoding`, no compression layer.

## Range

One parser serves local and proxied S3 responses (`domain::http_range`):

* `bytes=a-b`, `a-`, `-n`, `0-`, `0-0`, with checked arithmetic. An end past EOF is clamped; a suffix larger than the object is the whole object; an overflowing number makes the header unusable (`200`); a start at or past EOF, a zero suffix, or any range of an empty object is `416` with `Content-Range: bytes */<size>`.
* **Multiple ranges.** The documents agree once read in full: API_DESIGN §2.16, STORAGE_DESIGN §3.6, plan M07-T12 and TEST_STRATEGY §2.1 all say the first byte-range-spec is authoritative, later ones are ignored, never coalesced, and `multipart/byteranges` is never emitted. No correction was needed; the task text that described a coalescing rule did not match the documents. The tests that pin it are `unit_range_header_decision_table` (`multi_*` cases) and `it_download_local_range_exact`.
* The `206` carries `Content-Range: bytes first-last/size`, `Content-Length: last-first+1` and `Accept-Ranges: bytes`. The `416` is the canonical envelope (`RANGE_NOT_SATISFIABLE`, `requestId` equal to `X-Request-Id`) plus `Content-Range: bytes */<size>`; no object byte is read.

## Content type, disposition and inline safety

* `Content-Type` is the type stored by the finalizer (extension, sniffed prefix and the hybrid policy of M07-T11). A value that is not a parseable media type is served as `application/octet-stream`. `X-Content-Type-Options: nosniff` is always present.
* `Content-Disposition` comes only from `domain::content_disposition` (RFC 6266 with an ASCII `filename` fallback and an RFC 5987 `filename*`). The fallback is `[A-Za-z0-9._ -]` with everything else `_`, so `Relatório Anual (2026).pdf` becomes `Relatorio Anual _2026_.pdf`.
* `disposition=inline` is honoured only for raster images, video, audio, PDF and `text/plain`. SVG, HTML, XHTML, any `+xml` or `xml` subtype, JavaScript, CSS, JSON and every unknown type are served as `attachment` whatever was requested (R-025). The content-security policy is not relaxed. The preview allowlist and the text-size cap belong to M15-T02.

## Concurrency, cancellation and timeouts

* `DOWNLOAD_CONCURRENCY = 128`. The permit is taken after the `304`/`416`/redirect decisions and before storage is opened, waits at most 5 s, then answers `503 TRANSFER_BACKPRESSURE` with `Retry-After: 5`. It lives inside the response body (`ContentStream`), so it is returned when the body completes, fails, or is dropped by a client disconnect. A zero-length body holds no permit.
* `DOWNLOAD_IDLE_TIMEOUT = 60 s` (TRANSFER_ENGINE §1.7): a per-stream watchdog measures the time since a chunk was handed to the connection. When the client has not consumed for 60 s the watchdog drops the provider reader and the permit, so the slot is free at once, and the connection is aborted with `TimedOut` the next time it is polled. A stream that is waiting for the provider, or that keeps consuming, is never timed out; there is no total deadline and no size-dependent deadline. Covered by the paused-clock unit tests `unit_download_idle_client_is_released_after_the_idle_timeout`, `unit_download_progress_resets_the_idle_clock` and `unit_download_slow_provider_is_not_an_idle_client`.
* `ContentStream` wraps the provider reader in a `ReaderStream` with 256 KiB chunks, truncates at the declared length, turns a short source into `UnexpectedEof` (the connection is aborted, never silently shortened) and logs `download_stream_failed` with the request id and byte counts. The URL, key and file name are never logged. A client disconnect logs `download_stream_abandoned` at debug.
* The route is `Transport::BytePath` with identity response encoding and an idle-only deadline: no response compression, no 30-second control-plane deadline, no body collection.
* No SQLite transaction is held across the stream. The object key is immutable and never reused, so a rename, move or concurrent delete cannot make the stream serve a different file. If storage reports the object missing, the file is looked up again: gone means `404 FILE_NOT_FOUND` (a delete won the race), still present means `502 STORAGE_OBJECT_MISSING`.

## Error mapping

| Condition | Code | Status |
|---|---|---|
| unknown, foreign, deleted, malformed id | `FILE_NOT_FOUND` | 404 |
| unsatisfiable range | `RANGE_NOT_SATISFIABLE` (+ `Content-Range`) | 416 |
| bad `disposition` | `VALIDATION_ERROR` | 422 |
| object absent in storage | `STORAGE_OBJECT_MISSING` | 502 |
| provider 5xx, timeout | `STORAGE_UPSTREAM_ERROR` | 502 |
| provider unreachable, local I/O | `STORAGE_UNAVAILABLE` | 503 |
| every download slot busy | `TRANSFER_BACKPRESSURE` (+ `Retry-After`) | 503 |
| permission refused | `STORAGE_PERMISSION_DENIED` | 500 |
| provider differs | `STORAGE_PROVIDER_MISMATCH` | 500 |
| size disagreement | `STORAGE_SIZE_MISMATCH` | 500 |

`STORAGE_OBJECT_MISSING` (502, retryable) existed in API_DESIGN §3 and is now in the Rust catalogue, the error snapshot and the web retry and presentation tables. Mapping reuses `transfers::classify::classify_storage`; no provider string reaches a client.

## S3 delivery decision and conditional requests

The first M15-T01 implementation returned `302` as soon as ownership was proven, so `Range`, `If-Range` and `If-None-Match` were never evaluated by Palmr and went to the provider. Measured against the pinned MinIO, the provider ignores `If-Range` entirely (a stale validator still gets `206`) and its `ETag` is a different string from Palmr's strong ETag. That broke the accepted contract (a stale or weak `If-Range` must restart with `200`) and was corrected:

| Request | Result on S3 (presign available) |
|---|---|
| plain `GET` | `302`, no storage call (O(0) control plane) |
| `Range`, no `If-Range`, satisfiable against the recorded size | `302`; the provider answers the `Range` |
| `Range`, no `If-Range`, unsatisfiable against the recorded size | `416` from Palmr, canonical envelope, no storage call |
| `If-None-Match` | one internal `HeadObject`; a match (Palmr's ETag or the provider's) is `304`, otherwise the rows above apply |
| `Range` **and** `If-Range` | one `HeadObject`, then Palmr decides with `domain::http_range` and streams through the bounded relay: `206` (match), `200` (stale, weak, malformed, empty, repeated or non-matching date) or `416` |

Validators. A client that downloaded directly holds the provider's strong ETag and its `Last-Modified`; a client that was served through Palmr holds Palmr's ETag. Both identify the same immutable object, so `If-Range` and `If-None-Match` accept either. Weak forms are never honoured for `If-Range`; `If-None-Match` uses weak comparison. Palmr's own responses (`200`, `206`, `304`) always carry Palmr's ETag. Dates are compared exactly at one-second precision against the provider's `Last-Modified`.

Redirected `Range` requests are not rewritten: a client that follows the redirect re-sends `Range` (reqwest does; clients differ) and the provider answers it. Nothing is signed except `host`, so no client header is bound into the URL. The relay uses the single internal S3 client; storage keys are unchanged. Tests: `it_download_s3_conditional_range_real_minio` (real MinIO, byte and SHA-256 checks for every case above, foreign owner, 140 abandoned conditional streams followed by 128 concurrent ones).

### Missing object behind a redirect

API_DESIGN lists `STORAGE_OBJECT_MISSING` (502) for this route but does not require a preflight. Palmr produces it wherever Palmr itself reads the object: local files, the proxy relay, and every conditional S3 request (the `HeadObject` fails). For a plain or ranged redirect Palmr has issued `302` before anything touches the provider, so the failure is the provider's own `404 NoSuchKey` XML at the target, not a Palmr envelope and not a `FILE_NOT_FOUND`. A `HeadObject` on every download would add a provider round trip to the O(0) path and would still leave a check-to-use window, so it is not added. Characterized by `it_download_s3_missing_object_redirect_boundary`.

## Known limits

* The 5 GiB Firefox run, the multi-proxy browser matrix and the RSS size sweep belong to M24 and M15-T05. The integration tests here use 2 to 24 MiB of generated data and never claim those.
* The plain redirect path does not verify that the object exists (see above).
* A `Range` + `If-Range` request on S3 moves its bytes through Palmr (bounded relay, 128-permit limit); that is the price of exact conditional semantics.

# Authenticated single-file download

Introduced in M15-T01. It implements API_DESIGN §2.16 (`GET /api/v1/files/{id}/content`), STORAGE_DESIGN §3.6 and §4.5, ADR 0004 §7, ADR 0007 and ADR 0008 for My Files. It is the streaming foundation that the preview endpoint (M15-T02, below) and the archive stream (M15-T04) reuse.

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
* `disposition=inline` is honoured only for raster images, video, audio, PDF and `text/plain`. SVG, HTML, XHTML, any `+xml` or `xml` subtype, JavaScript, CSS, JSON and every unknown type are served as `attachment` whatever was requested (R-025). The content-security policy is not relaxed. The preview allowlist and the text limit are in the next section.

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

# Authenticated file preview

Introduced in M15-T02. `GET /api/v1/files/{id}/preview` (API_DESIGN §2.16, SPEC §37, FRONTEND_ARCHITECTURE §7) is the same delivery as `/content` with a stricter type policy, an `inline` disposition and a bound on text. It adds no storage protocol, no second semaphore and no second streaming engine: `DownloadService::serve` (the former `deliver` body) takes a `Plan` — served type, disposition, whether a redirect is allowed, an optional response-length limit — and both routes call it.

| Artefact | Location |
|---|---|
| Policy, limits, route | `apps/server/src/features/downloads/preview.rs` |
| Shared engine | `features/downloads/service.rs` (`serve`, `Plan`) |
| New catalogue codes | `PREVIEW_UNSUPPORTED` 415, `PREVIEW_TOO_LARGE` 413 (`domain/error_code.rs`, snapshot, web tables, 23 locales) |
| Unit tests | `features/downloads/tests.rs` (`unit_preview_*`) |
| Process tests | `tests/it_preview_local_process.rs`; `tests/it_preview_s3_process.rs` (real MinIO behind a recording tap, `tests/support/tap.rs`; the `*_e2e_fixture` tests need feature `e2e-fixture`) |

## Order of operations

1. Authenticate; parse the id (malformed is `404 FILE_NOT_FOUND`); resolve the file with the same owner-scoped read as `/content`. Nothing about the file's type, size, validators or range is decided before this succeeds, so a foreign, deleted or unknown id is an identical `404` for every header combination, including for a type that would be `415` or a size that would be `413` for its owner.
2. Provider and key checks (`500` operator errors), then **type policy** from the stored MIME only: `415 PREVIEW_UNSUPPORTED` before any storage access, redirect or conditional evaluation.
3. If the provider can presign and the type is not text: the `/content` redirect rules (plain or ranged request is `302`; `If-None-Match` or `Range` with `If-Range` is decided by Palmr after one `HeadObject`), with `response-content-disposition: inline` and the allowlisted type pinned into the signature.
4. Otherwise `stat`, size-integrity check, the `Range`/`If-Range` decision, then for text **the limit** (`413 PREVIEW_TOO_LARGE`), then `If-None-Match` (`304`), then `416`, then the stream. The limit is checked before `304` so a validator cannot launder a refused request; `304` still precedes `416`, as for downloads.

## Type policy

Decided by `preview::classify` from `files.mime_type` (the type the finalizer stored: sniffed prefix, extension, hybrid policy of M07-T11). The file name, the client-declared type, request headers and query members play no part, and there is no re-detection (no object byte is read to decide). Matching is by exact essence against explicit lists; there are no `image/*` or `text/*` wildcards.

| Class | Types | `Content-Type` sent | Redirect on S3 |
|---|---|---|---|
| image | png, apng, jpeg, gif, webp, avif, bmp, x-icon, vnd.microsoft.icon, heic, heif, tiff, jxl | the stored essence | yes |
| video | mp4, webm, ogg, quicktime, x-matroska, x-m4v, mpeg, x-msvideo, 3gpp, 3gpp2, mp2t | the stored essence | yes |
| audio | mpeg, mp3, mp4, aac, x-m4a, ogg, opus, wav (x-wav, wave, vnd.wave), webm, flac, x-flac | the stored essence | yes |
| PDF | `application/pdf` | the stored essence | yes |
| text and code | `text/plain`, markdown, csv, tsv, css, yaml, toml, log, diff/patch, common source types (`text/x-*` from an explicit list), JavaScript and TypeScript, JSON, NDJSON, SQL, shell, and any `+json` type | always `text/plain; charset=utf-8` | **never** |
| unsafe | any subtype containing `xml`, `html` or `svg`, any `+xml` suffix | — (`415`, `details.reason = unsafe_content`) | no |
| unsupported | everything else: octet-stream, archives, executables, unknown or unparsable types | — (`415`, `details.reason = unsupported_type`) | no |

Why text is forced to `text/plain`: source is previewed as text, so a stored `text/javascript`, `text/css` or `application/x-sh` is never emitted under an executable or stylesheet type. With `X-Content-Type-Options: nosniff` and the unchanged content-security policy, no preview response is an active document on the application origin. HTML, XHTML, XML and SVG are refused outright; the user downloads them (`/content` still serves them as `attachment`). A supported type does not promise the browser has the codec; falling back to download on a media error is the frontend's job (FRONTEND_ARCHITECTURE §7.3). MIME detection quirks that misname a file (for example `.ts` is detected as MPEG-TS video) are M07-T11 behaviour and are not corrected here.

A `415` carries the canonical envelope, `X-Request-Id` equal to `error.requestId`, `details.reason`, no `Location`, no `Content-Disposition`, no validator and no file byte.

## The text limit

| Quantity | Value | Meaning |
|---|---|---|
| `TEXT_PREVIEW_PREFIX_BYTES` | 524 288 (512 KiB) | what the SPA requests: `Range: bytes=0-524287` (FRONTEND_ARCHITECTURE §7.5) |
| `TEXT_PREVIEW_MAX_BYTES` | 5 242 880 (5 MiB) | the most any text or code response may carry (API_DESIGN §2.16) |

Source documents and why they agree. SPEC §37 requires "a safe size limit". API_DESIGN §2.16 refuses text above 5 MiB with `PREVIEW_TOO_LARGE`. FRONTEND_ARCHITECTURE §7.5 fetches a 512 KiB prefix by `Range`, and R-024 expects the prefix of a 500 MiB log to render with a "download to view all" fallback. Read literally, "refused above 5 MiB" and "a prefix of a 500 MiB file renders" collide; both are statements about the **response**, not about the object, so the rule is stated on the response: *the length that would be sent, after `Range` and `If-Range` are resolved, must not exceed 5 MiB*. The plan's acceptance text ("oversized text is truncated at the documented limit") is superseded by the higher-ranked API_DESIGN: the server never silently truncates, the client truncates by asking for a `Range`. All three documents were updated narrowly (API_DESIGN §2.16 and the `PREVIEW_*` catalogue rows, FRONTEND_ARCHITECTURE §7.5 step 1, the plan's M15-T02 notes and acceptance).

| Request | Object ≤ 5 MiB | Object > 5 MiB |
|---|---|---|
| no `Range` (or a malformed, repeated or stale-`If-Range` one) | `200`, whole | `413 PREVIEW_TOO_LARGE` |
| `bytes=0-524287` | `206`, `min(size, 512 KiB)` bytes | `206`, 512 KiB |
| `bytes=0-`, `bytes=n-` | `206` | `413` unless `size - n ≤ 5 MiB` (then `206`) |
| `bytes=-n` | `206`, `min(n, size)` | `206` if `n ≤ 5 MiB`, else `413` |
| `bytes=a-b` | `206`, clamped to the object | `206` if the clamped length ≤ 5 MiB, else `413` |
| start at or past the end, `bytes=-0`, any range of an empty object | `416` with `Content-Range: bytes */size` | `416` |
| matching `If-None-Match` | `304` | `304` for a request that would be `206`; `413` for one that would be refused |

The `413` envelope carries `details.limitBytes = 5242880` and `details.prefixBytes = 524288` and no body byte. A zero-byte text file is `200` with `Content-Length: 0`. Paging an arbitrarily large object in pieces of at most 5 MiB stays possible by design: `Range` is a bound on each response, not an access control, and the object is owned by the caller.

How the bound is enforced, not assumed:

* The comparison uses the size measured by `stat`/`HeadObject` (after the recorded-size check), and `ContentStream` still truncates at the declared length and treats a short source as an error, so a provider that sends more cannot enlarge the response.
* Local: `open_range(start, length)` seeks and reads `length` bytes through 256 KiB chunks. S3: text is never redirected, because a presigned `GET` grants the whole object for its TTL whatever `Range` the browser first sent. Palmr issues its own `GetObject` with `Range: bytes=start-end` through the internal client and relays it. The real-MinIO tests record the provider's side with a tap and assert exactly one object read of at most the prefix (or 5 MiB) bytes, and zero object reads for every `413`.
* Palmr never builds, collects or buffers the text: the unit test `unit_download_sources_never_materialise_a_whole_object` now scans `preview.rs` as well.

## Headers

`200` and `206` served by Palmr: `Content-Type` (table above), `Content-Disposition: inline; filename="…"; filename*=UTF-8''…` from the M07-T12 builder, `X-Content-Type-Options: nosniff`, `Cache-Control: private, no-cache`, `Accept-Ranges: bytes`, `ETag` (strong, as for downloads), `Last-Modified`, the measured `Content-Length`, and `Content-Range` for `206`. No `Content-Encoding`. The `302` is `Cache-Control: no-store` with a sensitive `Location` and carries Palmr's default security headers (including `nosniff`), which the browser does **not** carry over to the response it receives after following the redirect.

## Concurrency, cancellation, timeouts

Unchanged and shared with downloads: the same `DownloadPermits` (128, 5 s wait, `503 TRANSFER_BACKPRESSURE`), the same `ContentStream` (256 KiB chunks, 60 s idle watchdog, permit and reader released on completion, failure, idle expiry or disconnect). `it_preview_shares_the_download_permits` stalls 128 mixed preview and download streams and observes the 129th preview answer `503`. The route is `Transport::BytePath` with identity encoding and an idle-only deadline (`unit_preview_route_declares_its_byte_path_policy`).

## Downloads are unchanged

`/content` keeps its semantics: no text limit, no type refusal, `attachment` by default, `inline` only for the types of `inline_is_safe`. The tests assert that the files `/preview` refuses (HTML, SVG, XML, archives, a 12 MiB log) still download completely.

## Not in this task

Thumbnails, a media viewer, pdf.js, transcoding, Share, Received and Embed preview routes (M19, M20, M21), and the browser-level R-024 and R-025 runs. This task holds the server half of R-024 (ranged seeks, bounded text prefix, `Content-Type`) and R-025 (stored type, `nosniff`, SVG never inline) as `regression_R024_preview_server_half_streams_ranges_and_bounds_text` and `regression_358_preview_mime_behind_proxy`; the Nginx, Traefik, Caddy and Cloudflare runs and the browser playback assertions belong to the E2E and proxy-matrix milestones. A future frontend that frames a PDF needs `object-src`/`frame-src 'self'` coordinated with the security model (FRONTEND_ARCHITECTURE §7.4); this task changes no content-security policy.

## Who controls which header on a redirected S3 preview (M15-T02 corrective)

After `302` the browser talks to the S3 provider, not to Palmr. The final `200`/`206` is the provider's, measured against the pinned MinIO target by `it_preview_s3_post_redirect_response_contract_real_minio`:

| Header on the final response | Controlled by | Observed (MinIO) | Guarantee |
|---|---|---|---|
| `Content-Type` | Palmr, signed (`response-content-type`) | the stored allowlisted type, for png, webp, mp4, webm, mp3, pdf | enforced: the provider rejects a changed, added or removed value with `403` |
| `Content-Disposition` | Palmr, signed (`response-content-disposition`) | `inline; filename="…"; filename*=UTF-8''…` | enforced, same `403` rule |
| `Content-Length`, `Content-Range`, `ETag`, `Last-Modified`, `Accept-Ranges` | provider | correct on `200` and on every `206` tested | provider behaviour; bytes and framing verified |
| `X-Content-Type-Options: nosniff` | **provider** | present on `200` and `206` | **not signable**: S3 `GetObject` response overrides cover only `Cache-Control`, `Content-Disposition`, `Content-Encoding`, `Content-Language`, `Content-Type` and `Expires`. MinIO sent it on every response observed; the S3 API does not require it and other providers need not send it |
| `Cache-Control`, CSP, CORP, COOP, `Referrer-Policy` | provider (or its proxy) | MinIO sends none of the CSP family | not Palmr's to enforce on the provider origin |

What is enforced and what is a provider property, stated without extension:

* Palmr enforces, before any URL exists: ownership, the type allowlist (so no SVG, HTML, XHTML, XML or unknown type ever receives a `Location`; text is never redirected), the signed type and disposition, a 300 s TTL and a signature over `host` only. A URL whose pinned parameters or expiry are altered is refused by the provider.
* Palmr cannot make a third-party provider send `nosniff`. API_DESIGN §1.17 and R-025 state `nosniff` for "download and preview responses" in the context of Palmr's own response headers; STORAGE_DESIGN §4.5 promises only that the signed type and disposition are correct "even though Palmr is not serving the bytes". The tests therefore assert `nosniff` on the provider response as a **verified property of the MinIO test target**, and on Palmr's `302` and every Palmr-streamed response. For other providers it is an operator requirement (add `X-Content-Type-Options: nosniff` at the provider or at the proxy fronting the public endpoint) and is not verified by this repository.
* Why the residual risk is bounded: only raster images, video, audio and PDF are ever signed, each with a trusted `Content-Type`. By the MIME Sniffing Standard a browser does not promote a supplied image, audio or video type to HTML and does not sniff a supplied `application/pdf` (this is the standard's rule, not re-verified in a browser here). Active content, which is what `nosniff` guards, is not in the allowlist.
* Same-origin condition. `PALMR_S3_PUBLIC_ENDPOINT` is not validated against `PALMR_BASE_URL`. If an operator publishes the object store on the same origin as Palmr (for example through a path on the same host), user-owned bytes are served from the authenticated application origin and origin isolation no longer limits them. The allowlist and the signed type still apply, but the deployment should use a distinct origin (host or port). Recommendation for a later task: warn or refuse at config parse when the two origins are equal.
* Presigned material is not logged: the same test reads the server log after every request and finds none of the signatures, the `X-Amz-Signature` or `X-Amz-Credential` names, or the S3 access and secret keys.
* The CSP sent with the application (ARCHITECTURE §11.4) adds the S3 public origin to `img-src`, `media-src` and `connect-src` only. `object-src` and `frame-src` stay `'self'`, so a PDF framed from the application URL that redirects to the S3 origin is a CSP question for the frontend milestone (FRONTEND_ARCHITECTURE §7.4). Browser behaviour was not tested here and this task changes no CSP.

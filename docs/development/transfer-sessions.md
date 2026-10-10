# Transfer sessions (control plane)

Introduced in M14-T02. It implements the authenticated My Files control plane of TRANSFER_ENGINE §2, §6 and §7 and API_DESIGN §2.15: sessions, admission, the durable state machine, cancel, close and retry. It moves no bytes. TUS (M14-T03 to T05, see `tus.md` for the protocol surface), S3 multipart (M14-T07) and the finalization engine (M14-T05) attach to it later.

| Artefact | Location |
|---|---|
| Module | `apps/server/src/features/transfers` (`routes`, `service`, `repo`, `model`, `state`, `admission`, `presentation`, `error`) and `apps/server/src/storage/planning.rs` |
| Routes | `POST/GET /api/v1/transfers/sessions`, `GET/DELETE …/{id}`, `POST …/{id}/complete`, `POST …/{id}/files/{itemId}/retry`, `DELETE …/{id}/files/{itemId}` |
| Flow tests | `apps/server/src/features/auth/flow_tests/transfer*.rs` |
| Statement-shape proof | `apps/server/tests/it_transfer_session_statement_shape.rs` |
| Replay-envelope migration | `apps/server/migrations/0007_idempotency_transfer_envelope.sql` |

## Admission is one transaction

`POST` runs a single `BEGIN IMMEDIATE`: account active, storage health, target folder, per-file size policy and provider plan, directory depth, the checked reservation total, folder-chain creation, quota admission, then the session, its items in batches of 200 rows, the one `held` reservation and the idempotency completion. Any failure rolls back all of it, including folders created earlier in the same transaction. Error precedence follows that order and is asserted by `it_transfer_session_error_precedence_is_fixed`.

The statement count does not grow with the file count: items are inserted 200 per statement and a directory shared by many files is created once (the chain is memoized inside the transaction).

## Provider differences stay in `storage`

`features/transfers` never names a provider kind, a storage configuration variant or a capability flag (`unit_caps_branch_points` enforces this). `storage/planning.rs` owns every provider-specific decision for a new item: whether it is a resumable local upload, an S3 multipart upload or a zero-byte S3 put, whether the profile is proxied (read live from the provider's capabilities), the provider's maximum object size, and the part layout. `features/transfers/admission.rs` only combines that answer with the admin size policy, storage health and the quota arithmetic.

## State machine

`state.rs` is the single authority. `session_transition` and `item_transition` encode T1 to T13 of TRANSFER_ENGINE §2.4, including which repeats are idempotent (`Unchanged`) and which transitions are refused. A refusal is `TRANSFER_SESSION_STATE_INVALID` (409). `TRANSFER_SESSION_EXPIRED` (410) is returned only for a session still in a live state whose `expires_at` has passed; a session the reaper already expired is terminal. Reading never changes a session.

An item's durable `pending` is spelled `created` on the wire. Client-only states (`queued`, `preparing`, `paused`, `resumable`) are rejected as filters.

## Storage identity

Each item receives `final_object_id` and `final_object_key` at admission. No `storage_objects` row and no blob exist until finalization, and neither value appears in any response, error or log. A session id is not a storage capability: every route proves `transfer_sessions.user_id = caller` and that the item belongs to that session.

## Session TTL

A session lives `created_at + 7 days` (TRANSFER_ENGINE §3.11) and its hold carries the same expiry. The 24 h TUS upload TTL is a different clock owned by the TUS adapter. The control plane never extends a session; activity refresh belongs to the adapters and may not pass the 7-day bound.

## Reservation shares

| Item | `reserved_bytes` |
|---|---|
| Known size | the size |
| Unknown size, finite maximum | `min(admin maximum, provider maximum)` |
| Unknown size, Unlimited maximum | `0` (the running check of TRANSFER_ENGINE §7.4 guards it) |

The session hold equals the sum of the shares of items that are `pending`, `uploading`, `finalizing` or `failed`. Totals are checked 64-bit sums limited to `9007199254740991`.

## Cancel keeps completed items

Cancel marks unfinished items `canceled`, marks any live `tus_uploads` row `terminated` and any live `s3_multipart_uploads` row `abandoned`, tombstone-inserts an object whose `finalize_stage` is `placing`, and settles the reservation: `committed` for the bytes of completed items, otherwise `released`. It performs no storage I/O and enqueues no abort job.

### Cleanup contract for the adapters

- A placed object goes through the existing deletion lifecycle: `storage_objects` (tombstoned) plus `file_deletion_queue` plus a `storage.delete_blob` job, which a registered handler runs after the one-hour grace. A resource that never reached `placing` gets no object row.
- No `s3.abort_abandoned_multipart` handler exists before M14-T09, so no such job is enqueued. The durable contract is the row: the provider identity is untouched, the state stays `abandoned` (multipart) or `terminated` (TUS) until cleanup succeeds, and the foreign keys from `transfer_session_files` are `RESTRICT`, so retention cannot drop a session that still awaits cleanup.
- `features/transfers/cleanup.rs` is the discovery contract: `abandoned_multipart_page` and `terminated_tus_page` page by `id` (at most 1 000 rows), served by the partial indexes of migration `0008`. M14-T09 aborts what the first returns; M14-T06 removes staging for what the second returns.
- Repeating a cancel changes zero rows. The rows, their identity and the tombstone survive a restart (`it_transfer_cancel_keeps_every_protocol_row_discoverable_for_cleanup`).

## Replay envelope

`POST /api/v1/transfers/sessions` lists up to 2 000 files, so its replay payload cannot fit the 16 KiB bound of every other idempotent route. The bound is per route (`SUPPORTED_ROUTES` in `infra/http/idempotency.rs`, mirrored by the `CASE route_template` in the `response_json` CHECK) and counted in UTF-8 bytes in both places.

| Route | Bound |
|---|---|
| every idempotent route except the one below | 16 384 bytes |
| `POST /api/v1/transfers/sessions` | 6 291 456 bytes (6 MiB) |

The 6 MiB figure comes from the worst legal response, not a round number. Per item: a 128-character `clientId`, a 255-byte `name` made of JSON-escaped quotes (510 bytes), a 1 023-byte `relativePath` of four 255-byte quote segments (2 043 bytes), a 5 TiB `sizeBytes` and a full S3 plan block, about 3 KiB. Times 2 000 items plus the envelope that is 5 994 322 bytes (5.72 MiB), asserted by `unit_maximum_create_response_fits_the_replay_bound_with_headroom`, which also fails if the bound drifts more than 10 % above that worst case. The 2 MiB control-plane request limit keeps real traffic well below it: a request that fits in 2 MiB and uses the worst Unicode normalization expansion (a 4-byte character that normalizes to 8 bytes) produces about 3.4 MB (`it_transfer_session_unicode_expansion_response_stays_inside_the_envelope_bound`).

Storage cost: a keyed 2 000-file create holds one payload in `idempotency_records` for 24 hours plus one `tokens.prune` interval: 849 307 bytes for 2 000 ordinary files (a 483 056-byte request), 3 372 322 bytes for the worst request that fits the 2 MiB limit, and at most 5.72 MiB by the field caps. That is next to the roughly 1 MiB its item rows already cost. Replay reads and decodes that payload once per request, so transient memory is a small multiple of the bound regardless of file sizes.

The first response and every replay are rendered by the same `ReplayEnvelope`, so they are byte-identical, and a replay returns the original body even after the session has changed.

## Advisory metadata and audit

`declaredContentType` is validated for shape and not stored. The TUS adapter receives the same hint as `Upload-Metadata` `filetype`, and S3 finalization decides the MIME by bounded sniffing, so durable persistence is not needed. No transfer audit action exists yet; the task that adds the first one (FILE_UPLOADED in M14-T05, then the cancel and expiry events) must enqueue it in the same transaction as the state change.

## Client keys

`clientId` is an opaque string of 1 to 128 characters with no control character. It is not narrowed further. When it appears in `FILE_TOO_LARGE` details it is carried as a validated owned string and serialized by the JSON encoder, which escapes every unsafe character.

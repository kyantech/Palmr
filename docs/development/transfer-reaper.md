# Expiry, cancellation and reaping (local uploads)

M14-T06. TRANSFER_ENGINE §3.11 and §3.11.1 are the design; this page is the map for people changing the code.

## Where things live

| File | Role |
|---|---|
| `features/transfers/cancel.rs` | `Ending` (`Canceled` / `Expired`), `end_items`, `tombstone_placed`, `settle_session`, `settle_if_held`: the one wind-down used by user cancel and by the reaper |
| `features/transfers/reaper.rs` | `ReaperContext`, `run_pass`, `ReapReport`, the windows and limits |
| `features/transfers/reaper/schedule.rs` | the `tus.expire_stale` handler, the startup `ensure_scheduled`, continuation jobs |
| `features/transfers/reaper/sessions.rs` | session expiry (T12) |
| `features/transfers/reaper/uploads.rs` | upload expiry (guards, item and quota settlement) |
| `features/transfers/reaper/staging.rs` | physical staging removal and the `staging_removed_at` marker |
| `features/transfers/reaper/retention.rs` | 7-day and 30-day row retention |
| `features/transfers/cleanup.rs` | keyset discovery pages |

## Windows

| Clock | Value | Source |
|---|---|---|
| TUS inactivity | 24 h from the last byte (`PATCH` renews it, never past the session) | `tus_uploads.expires_at` |
| Transfer session | 7 days from creation, never extended | `transfer_sessions.expires_at` |
| Sweep cadence | 15 minutes, bucketed `dedup_key` | `EXPIRE_STALE_PERIOD` |
| Terminal TUS rows | 7 days after `staging_removed_at` | `TERMINAL_UPLOAD_RETENTION` |
| Terminal sessions | 30 days after `completed_at` | `TERMINAL_SESSION_RETENTION` |

Expiry is `now >= expires_at` on the injected `Clock`.

## A pass

`sessions → uploads → staging → retention`. Each phase reads keyset pages of 200 and stops at 1 000 candidates (`PASS_LIMIT`); a capped pass enqueues a continuation job (`<kind>:<bucket>:more:<n>`, at most `MAX_CONTINUATIONS` per bucket). One failing candidate is counted in `ReapReport::errors` and does not stop the pass; the handler returns an error afterwards so the job runtime retries it. The successor job is enqueued before the pass starts, so a failing run cannot end the chain.

## Guards (why a live upload is never destroyed)

An upload is expired only when, inside its own write transaction, it is still `created`/`in_progress`, `now >= expires_at`, its lease is absent or lapsed, and its item is not awaiting finalization (`finalizing`, or `uploading` with `upload_offset = upload_length`, while the session is within its 7 days). The outcomes are `Expired`, `Gone`, `NotDue`, `Leased`, `AwaitingFinalization`. A guarded transition that changes nothing performs no cleanup.

Races are resolved by SQLite's single writer and the `from`-state guards, never by in-process locks: the finalizer's claim, intent and commit statements all require `state = 'finalizing'` (and a live TUS row), so after the reaper moved the item to `expired` they match nothing, and when the finalizer committed first the upload is `completed` and is not a candidate.

## Quota

One reservation per session; each item owns `reserved_bytes` of it. Expiring an item releases that share once (the item leaves `pending`/`uploading`/`finalizing`/`failed`, which is the guard). A session settles `committed` for completed bytes, otherwise `released` with `release_reason = 'expired'`. A reservation that is not `held` any more (for example reaped by `quota.reconcile`) is left alone.

## Staging

Removal runs outside every transaction and only through `UploadId::parse(<tus_uploads.id>)`. `ENOENT` is success; anything else is logged by kind and retried next pass. A row is marked `staging_removed_at` only after the removal returned. Rows with a live lease are skipped. A symlinked or non-canonical directory is refused by the storage layer and never marked.

## S3

Session expiry marks live multipart rows `abandoned` and leaves `bucket`, `object_key` and `s3_upload_id` untouched. No abort job exists before M14-T09, so none is enqueued, and retention keeps a session that still has a multipart row.

## Audit

| Action | Write path | Actor | Target | When |
|---|---|---|---|---|
| `TRANSFER_CANCELED` | in the cancel transaction | the owner | `session` = transfer session id | session `DELETE`, item `DELETE`, TUS `DELETE`, only when the transition moved |
| `TRANSFER_EXPIRED` | enqueued after the commit (activity class, bounded loss on a hard crash) | `system` | `session` for a session expiry, `tus_upload` for one expired upload | once per transition |

`metadata_json` is built by `actions::transfer_canceled` / `transfer_expired` and holds only `scope` (`session`, `item`, `upload`), `transfer_session_id`, `item_id`, `provider`, `items_ended`, `completed_items_kept`, `session_closed` and (expiry) `owner_id`. A 2,000-item session is one event. Repeats, no-ops, lost races, retries, cleanup attempts and retention write nothing, and finished files keep their `FILE_UPLOADED` row without gaining `FILE_DELETED`. `target_type = 'session'` is shared with authentication sessions; the action name disambiguates.

## Tests

`flow_tests/transfer_reaper.rs` (mandatory), `transfer_reaper_races.rs` (guards, races, staging failures, restart), `transfer_audit.rs` (the audit contract above), `transfer_reaper_scale.rs` (backlog, retention order, quota shares, 2 000 items, S3 identity, query plans), and the structural unit tests in `reaper.rs`.

```sh
cargo nextest run -p palmr-server -E 'test(transfer_reaper) or test(it_tus_expire_stale_idempotent) or test(it_active_upload_never_expires) or test(it_cancel_releases_reservation) or test(it_startup_reconcile_releases_expired_holds) or test(it_abandoned_placed_object_tombstone_inserted) or test(it_cancel_keeps_completed_items)'
```

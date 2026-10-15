# Schema and migrations

## Freeze

The v4.0.0 schema is frozen.

| | |
|---|---|
| Freeze date | `2026-09-24` |
| Source commit | `79870f9d960df31b6f3c36a5cf2ec2b44551d169` |
| Frozen migration | `apps/server/migrations/0001_initial_schema.sql` |
| Schema shape | 40 domain tables + 2 FTS5 virtual tables |

`0001_initial_schema.sql` is immutable. It must not be edited, renamed, reordered, squashed or deleted, and it has no down migration. The migrator records its checksum, so a single changed byte makes every existing database refuse to start with a checksum mismatch.

Every schema change after the freeze is a new forward-only migration with a four-digit, zero-padded, monotonically increasing name and a `snake_case` description:

```text
0002_*.sql
0003_*.sql
...
```

Post-freeze migrations so far:

| Migration | Change |
|---|---|
| `0002_add_identity_provider_email_linking.sql` | `identity_providers.allow_email_linking` (ADR 0012 rule 3): existing `oidc` rows `1`, `oauth2` rows `0`, column default `0` |
| `0003_authorization_request_contract.sql` | `oauth_auth_requests` purpose vocabulary (`login`, `link`, `reauth`) and the `auth_providers_enabled` setting row |
| `0004_session_revoked_reason_identity_unlink.sql` | `sessions.revoked_reason` gains `identity_provider_unlinked` |
| `0005_password_login_enabled.sql` | `app_settings.password_login_enabled` (`security`, `boolean`, `true`; Decision 52): written only by `PUT /api/v1/admin/auth/password-login` |
| `0006_deletion_lifecycle.sql` | `folders.deleting`, `folder_deletions`, `deletion_receipts`, `transfer_sessions.deleted_target_folder_id` and its immutability trigger (ADR 0018) |
| `0007_idempotency_transfer_envelope.sql` | Rebuilds `idempotency_records`: the `response_json` CHECK counts UTF-8 bytes and allows 6 MiB for `/api/v1/transfers/sessions` (16 KiB elsewhere). Rows are copied unchanged |
| `0008_cancellation_cleanup_discovery.sql` | Keyset discovery indexes `ix_s3mp_abandoned` and `ix_tus_uploads_terminated` for the cancellation cleanup contract |
| `0009_tus_staging_cleanup_and_expiry_indexes.sql` | `tus_uploads.staging_removed_at` (proof that physical staging cleanup finished), the cleanup and retention indexes `ix_tus_uploads_cleanup` / `ix_tus_uploads_retention`, and the session indexes `ix_transfer_sessions_failed_expiry` / `ix_transfer_sessions_retention` that the `tus.expire_stale` reaper needs (M14-T06) |
| `0010_relative_path_exact_dot_segments.sql` | Rebuilds `transfer_session_files` with `relative_path` refusing a `..` segment instead of any `..` substring, together with the three RESTRICT/CASCADE children `tus_uploads`, `s3_multipart_uploads` and `s3_multipart_parts` (copied through `*_held` tables inside the migration transaction; rows, keys and indexes are preserved). SQLite cannot alter a CHECK in place, and the RESTRICT keys forbid dropping the parent while children hold rows (M14-T10) |

A new migration must keep both of these producing the same normalized schema:

```text
empty                     -> head
every released v4 schema  -> head
```

There is no `db push`, no runtime drift repair, no `CREATE TABLE IF NOT EXISTS` reconciliation and no force-reset flag. A migration that fails aborts startup, leaves the database at its last applied migration, and the HTTP listener is never bound.

## Local checks

Run from `v4/`:

```sh
cargo nextest run -E 'test(it_schema_golden_matches) or test(it_schema_table_count) or test(it_schema_frozen_migration_matches_baseline)'
```

- `it_schema_golden_matches` migrates a fresh database to head and compares the normalized `sqlite_master` against `apps/server/tests/snapshots/schema.sql`.
- `it_schema_table_count` requires exactly 40 domain tables and the 2 FTS5 virtual tables `files_fts` and `received_files_fts`.
- `it_schema_frozen_migration_matches_baseline` compares the bytes of `0001_initial_schema.sql` with the committed fingerprint `apps/server/tests/fixtures/0001_initial_schema.sha256`.

Regenerating the golden snapshot is an explicit developer action, never automatic:

```sh
PALMR_UPDATE_SCHEMA_SNAPSHOT=1 cargo nextest run -E 'test(it_schema_golden_matches)'
```

Regenerate only for an intended schema change or a new migration that changes the normalized schema, and review the resulting diff before committing. The frozen fingerprint is never updated to accept an edit to `0001_initial_schema.sql`; an intended schema change is a new `0002_*.sql` migration.

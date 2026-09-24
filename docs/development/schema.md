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

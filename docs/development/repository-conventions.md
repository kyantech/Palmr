# Repository conventions

Palmr keeps one SQLite database with two pools: a single write connection and a bounded read pool. `DbPools` (`apps/server/src/infra/db`) exposes them as two executor forms, and every repository function declares which one it takes.

| Access | Executor | Obtained from |
|---|---|---|
| Read outside a mutation | `&ReadPool`, queried through `ReadPool::executor()` | `DbPools::reader()` |
| Mutation, and any read that guards it | `&mut WriteTx<'_>`, queried through `WriteTx::executor()` | the closure argument of `DbPools::write_tx` |

There is no public accessor for the write pool. The only way to write is to open a write transaction.

## Repository signatures

```rust
pub async fn find_share(db: &ReadPool, alias: &str) -> Result<Option<Share>, DbError>;
pub async fn insert_share(tx: &mut WriteTx<'_>, share: &NewShare) -> Result<(), DbError>;
pub async fn count_active_admins(tx: &mut WriteTx<'_>) -> Result<i64, DbError>;
```

- A read-only handler takes `&ReadPool`. It never opens a write transaction just to read.
- A read whose result decides a write takes `&mut WriteTx<'_>`, even if it only reads. Quota admission, last-admin checks, job claims and any read-compare-write sequence read inside the same transaction that writes. Reading on the read pool and then writing is a check-then-act race.
- Repositories return `DbError`. Services decide what a constraint failure means.

## Write transactions

```rust
pools
    .write_tx(clock, "shares.create", async |tx| {
        let taken = shares::repo::alias_exists(tx, &alias).await?;
        if taken {
            return Err(ShareError::AliasTaken);
        }
        shares::repo::insert_share(tx, &share).await?;
        Ok(share.id)
    })
    .await
```

`write_tx` waits in the write pool's FIFO queue, issues `BEGIN IMMEDIATE`, runs the closure, then commits on `Ok` or rolls back on `Err`. If the future is dropped or panics mid-transaction, sqlx rolls the connection back before reuse. The closure's error type must implement `From<DbError>`.

- The name is a `&'static str` in the form `feature.action`. It appears in logs, so it must never contain runtime data.
- `BEGIN IMMEDIATE` takes the write lock up front, so every read inside the closure runs under that lock.
- Never call `write_tx` inside another `write_tx` closure. The inner call waits for the connection the outer call holds, so it never returns.
- Every write transaction logs `transaction`, `pool_wait_ms`, `elapsed_ms` and `outcome` at `DEBUG`. `elapsed_ms` measures from `BEGIN IMMEDIATE` to commit or rollback. It excludes pool wait, which is logged separately as `pool_wait_ms`. A transaction that takes more than 250 ms logs at `WARN` with `threshold_ms`. Treat that warning as a defect. SQL parameters are never logged.

## Keep transactions short

A closure contains database statements only. Never do any of the following inside it:

- storage I/O: `rename`, `fsync`, copy, `stat` or directory listing;
- network I/O: S3, HTTP, OIDC or SMTP;
- sending e-mail (insert the outbox and job rows instead);
- waiting on a channel, lock, semaphore or another task;
- statements over an unbounded number of rows. Process large sets in batches of at most 1 000 rows per transaction, after the first transaction hides the parent.

When an operation mixes I/O and state, use three steps: a short transaction that records intent and reserves resources, then the I/O outside any transaction, then a second short transaction that records the result.

## Errors

`DbError` classifies SQLite extended result codes. It never reads error message text.

| Variant | SQLite result | Public mapping when unhandled |
|---|---|---|
| `Busy` | `SQLITE_BUSY` and its extended codes; a write-queue acquire timeout | `DATABASE_BUSY`, 503, retryable |
| `UniqueViolation` | `SQLITE_CONSTRAINT_UNIQUE`, `SQLITE_CONSTRAINT_PRIMARYKEY` | `INTERNAL_ERROR` |
| `CheckViolation` | `SQLITE_CONSTRAINT_CHECK` | `INTERNAL_ERROR` |
| `ForeignKeyViolation` | `SQLITE_CONSTRAINT_FOREIGNKEY` | `INTERNAL_ERROR` |
| `Other` | everything else | `INTERNAL_ERROR` |

- Services match constraint variants on purpose. For example, a duplicate name retries with the next keep-both candidate, and each attempt runs in its own write transaction. Any constraint failure a service does not handle is a bug and reaches clients as `INTERNAL_ERROR`.
- Never retry `SQLITE_BUSY` in a loop. Palmr writers cannot cause it, because they queue for the single write connection. It only occurs when an external process holds the lock past the 5 s busy timeout. Return `DATABASE_BUSY` and let the client retry.

## Shared e-mail translations

The `emails` namespace is shared between the SPA and the transactional e-mail subsystem (FRONTEND_ARCHITECTURE §12.8). Its canonical source is `apps/web/src/app/i18n/locales/<locale>/emails.json`; there is no second copy. The Rust e-mail renderer embeds the 23 files at build time with `include_str!` from `apps/server/src/features/email/render.rs`, which is why the container's Rust build stage copies `apps/web/src/app/i18n`. Every locale file must carry the same key set as `en-US`, asserted by `unit_email_locale_key_parity`.


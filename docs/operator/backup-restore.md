# Backup and restore

## A complete backup is three artifacts

Back up all three together. None of them is useful alone.

1. **The database backup** written by `palmr db backup`.
2. **`instance.key`** from the data directory (`/data/instance.key`). It seals every recoverable secret stored in the database: TOTP secrets, the SMTP password and identity-provider client secrets. Keep it with every database backup. A database restored without its `instance.key` keeps all users and files, but SMTP and external login stop working until their secrets are re-entered, and every user must re-enroll TOTP.
3. **The file storage**: the storage root (`/data/storage/`) for local storage, or the S3 bucket for S3 storage. The database describes files; it does not contain their bytes.

The database backup alone is **not** a complete backup.

`/data/uploads/` holds unfinished uploads and is not part of a backup.

## Backing up the database

```sh
palmr db backup --out /data/backup
```

The command writes a single, consistent, compacted SQLite file with SQLite's `VACUUM INTO` and prints its path:

```text
/data/backup/palmr-20260922T031500Z.db
```

- The file name is `palmr-<UTC timestamp>.db`, so backups sort by time.
- The output directory must already exist.
- An existing file is never overwritten; the command fails instead.
- The file is written under a temporary name and renamed only once it is complete and flushed to disk, so a failed or interrupted backup never leaves a file that looks like a finished backup.
- The file has no `-wal` or `-shm` companion files and can be copied anywhere.

By default the command takes ownership of the data directory and refuses to run while a Palmr server is using it. To back up a running instance, add `--allow-concurrent`:

```sh
palmr db backup --out /data/backup --allow-concurrent
```

With `--allow-concurrent` the database is only read. The backup is a snapshot of the database at one instant, and the server keeps serving and writing while it is taken.

In a container, run the command inside the running container so it uses the same `PALMR_DATA_DIR`:

```sh
docker exec palmr palmr db backup --out /data/backup --allow-concurrent
```

Then copy the backup file, `instance.key` and the storage root (or take an S3 bucket snapshot) to your backup destination.

### Never copy a live `palmr.db`

Do not back up a running instance by copying `palmr.db`. Palmr uses SQLite's write-ahead log: recent changes live in `palmr.db-wal` until they are checkpointed, so copying `palmr.db` alone produces a torn, inconsistent snapshot. Copying `palmr.db` together with its `-wal` file is also unsafe while the server writes to them. Use `palmr db backup`.

## Checking the database

```sh
palmr db check
```

Runs SQLite's integrity check and prints its result. An intact database prints `ok` and the command exits `0`. Otherwise every reported problem is printed and the command exits with a non-zero status. The check opens the database read-only; it never migrates or modifies it.

Like `db backup`, it refuses to run while a Palmr server is using the data directory unless `--allow-concurrent` is given.

## Applying migrations

```sh
palmr migrate
```

Applies the database migrations built into this Palmr binary and exits. Running it again on a current database changes nothing. The server applies the same migrations when it starts, so this command is only needed to migrate at a chosen moment. It always refuses to run while a Palmr server is using the data directory.

## Restore

Restore in this order:

1. Stop Palmr.
2. Restore the storage root (or the S3 bucket).
3. Restore `instance.key` to the data directory with mode `0600`, owned by the user Palmr runs as.
4. Restore the database: copy the backup file to `palmr.db` in the data directory and delete any `palmr.db-wal` and `palmr.db-shm` files left there.
5. Start Palmr.

Restore the three artifacts from the same backup. A database newer than the storage has rows pointing at missing files; storage newer than the database has files that no row refers to.

Before starting Palmr, you can verify the restored database with `palmr db check`.

## Exit status

| Status | Meaning |
|---|---|
| `0` | success |
| `1` | the command failed while running (for example, SQLite reported an error during the backup) |
| `65` | the integrity check found problems |
| `66` | there is no database in the data directory |
| `73` | the backup destination is not usable, or the backup file already exists |
| `78` | the data directory is in use by a running Palmr server, or the configuration, data directory or database cannot be used |

Every failure prints one line to standard error that starts with `FATAL:` and a stable error code, such as `STARTUP_DATA_DIR_IN_USE` or `CLI_BACKUP_EXISTS`. The commands never print secrets.

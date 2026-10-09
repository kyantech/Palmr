ALTER TABLE folders ADD COLUMN deleting INTEGER NOT NULL DEFAULT 0 CHECK (deleting IN (0,1));

CREATE INDEX ix_folders_deleting ON folders(owner_id) WHERE deleting = 1;

CREATE TABLE folder_deletions (
    folder_id       TEXT    NOT NULL PRIMARY KEY,
    owner_id        TEXT    NOT NULL,
    claimed_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    files_deleted   INTEGER NOT NULL DEFAULT 0 CHECK (files_deleted >= 0),
    folders_deleted INTEGER NOT NULL DEFAULT 0 CHECK (folders_deleted >= 0),
    bytes_released  INTEGER NOT NULL DEFAULT 0 CHECK (bytes_released >= 0),

    FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX ix_folder_deletions_owner ON folder_deletions(owner_id);

CREATE TABLE deletion_receipts (
    id            TEXT NOT NULL PRIMARY KEY,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('file','folder')),
    owner_id      TEXT NOT NULL,
    deleted_at    TEXT NOT NULL,

    FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX ix_deletion_receipts_owner ON deletion_receipts(owner_id);

ALTER TABLE transfer_sessions ADD COLUMN deleted_target_folder_id TEXT NULL
    CHECK (deleted_target_folder_id IS NULL
           OR (target_folder_id IS NULL AND context = 'my_files'
               AND state IN ('completed','canceled','expired')));

CREATE TRIGGER transfer_sessions_deleted_target_immutable
BEFORE UPDATE OF deleted_target_folder_id ON transfer_sessions
WHEN OLD.deleted_target_folder_id IS NOT NULL
 AND NEW.deleted_target_folder_id IS NOT OLD.deleted_target_folder_id
BEGIN
    SELECT RAISE(ABORT, 'transfer_sessions.deleted_target_folder_id is immutable');
END;

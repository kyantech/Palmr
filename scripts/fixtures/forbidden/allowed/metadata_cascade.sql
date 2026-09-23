CREATE TABLE share_items (
    id        TEXT NOT NULL PRIMARY KEY,
    share_id  TEXT NOT NULL,
    file_id   TEXT NULL,
    FOREIGN KEY (share_id) REFERENCES shares(id) ON DELETE CASCADE,
    FOREIGN KEY (file_id)  REFERENCES files(id)  ON DELETE CASCADE
) STRICT;

CREATE TABLE files (
    id                TEXT NOT NULL PRIMARY KEY,
    owner_id          TEXT NOT NULL,
    storage_object_id TEXT NOT NULL UNIQUE,
    -- never ON DELETE CASCADE here: this row owns bytes
    FOREIGN KEY (owner_id)          REFERENCES users(id)           ON DELETE RESTRICT,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE users (
    id               TEXT NOT NULL PRIMARY KEY,
    email            TEXT NOT NULL,
    email_normalized TEXT NOT NULL
) STRICT;

-- canonical lowercase replaces COLLATE NOCASE (ADR 0001)
CREATE UNIQUE INDEX ux_users_email_normalized ON users (email_normalized);

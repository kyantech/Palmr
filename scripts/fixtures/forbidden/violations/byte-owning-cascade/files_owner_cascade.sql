CREATE TABLE files (
    id                TEXT NOT NULL PRIMARY KEY,
    owner_id          TEXT NOT NULL,
    storage_object_id TEXT NOT NULL UNIQUE,
    FOREIGN KEY (owner_id)          REFERENCES users(id)           ON DELETE CASCADE,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
) STRICT;

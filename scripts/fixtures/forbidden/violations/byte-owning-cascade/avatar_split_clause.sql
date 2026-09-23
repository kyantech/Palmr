ALTER TABLE users ADD COLUMN avatar_storage_object_id TEXT NULL
    REFERENCES storage_objects(id) ON DELETE
    CASCADE;

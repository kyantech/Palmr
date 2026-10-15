CREATE TABLE transfer_session_files_next (
    id                        TEXT    NOT NULL PRIMARY KEY,
    transfer_session_id       TEXT    NOT NULL,
    ordinal                   INTEGER NOT NULL CHECK (ordinal >= 0),
    client_file_key           TEXT    NOT NULL CHECK (length(client_file_key) BETWEEN 1 AND 128),
    display_name              TEXT    NOT NULL CHECK (length(display_name) BETWEEN 1 AND 255
                                                      AND display_name NOT LIKE '%/%'),
    relative_path             TEXT    NOT NULL DEFAULT '' CHECK (
                                          length(relative_path) <= 1024
                                          AND relative_path <> '..'
                                          AND relative_path NOT LIKE '../%'
                                          AND relative_path NOT LIKE '%/..'
                                          AND relative_path NOT LIKE '%/../%'
                                          AND relative_path NOT GLOB '/*'),
    declared_size_bytes       INTEGER NULL CHECK (declared_size_bytes IS NULL OR declared_size_bytes >= 0),
    reserved_bytes            INTEGER NOT NULL DEFAULT 0 CHECK (reserved_bytes >= 0),
    upload_kind               TEXT    NOT NULL CHECK (upload_kind IN ('tus','s3_multipart','s3_single')),
    state                     TEXT    NOT NULL DEFAULT 'pending' CHECK (state IN (
                                          'pending','uploading','finalizing','completed',
                                          'failed','canceled','expired','skipped')),
    finalize_stage            TEXT    NOT NULL DEFAULT 'none'
                                      CHECK (finalize_stage IN ('none','placing','committed')),
    final_object_id           TEXT    NOT NULL CHECK (length(final_object_id) = 36),
    final_object_key          TEXT    NOT NULL CHECK (
                                          length(final_object_key) = 46
                                          AND final_object_key GLOB 'objects/[0-9a-f][0-9a-f]/[0-9a-f][0-9a-f]/[0-9a-f]*'
                                          AND substr(final_object_key, -32) NOT GLOB '*[^0-9a-f]*'),
    attempts                  INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    resulting_file_id         TEXT    NULL,
    resulting_received_file_id TEXT   NULL,
    error_code                TEXT    NULL CHECK (error_code IS NULL OR length(error_code) <= 64),
    error_request_id          TEXT    NULL CHECK (error_request_id IS NULL OR length(error_request_id) <= 128),
    created_at                TEXT    NOT NULL,
    updated_at                TEXT    NOT NULL,
    completed_at              TEXT    NULL,

    CHECK ( resulting_file_id IS NULL OR resulting_received_file_id IS NULL ),
    CHECK ( (state = 'completed' AND finalize_stage = 'committed')
         OR (state <> 'completed' AND finalize_stage <> 'committed') ),

    FOREIGN KEY (transfer_session_id)        REFERENCES transfer_sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (resulting_file_id)          REFERENCES files(id)             ON DELETE SET NULL,
    FOREIGN KEY (resulting_received_file_id) REFERENCES received_files(id)    ON DELETE SET NULL
);

INSERT INTO transfer_session_files_next
    (id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
     declared_size_bytes, reserved_bytes, upload_kind, state, finalize_stage, final_object_id,
     final_object_key, attempts, resulting_file_id, resulting_received_file_id, error_code,
     error_request_id, created_at, updated_at, completed_at)
SELECT id, transfer_session_id, ordinal, client_file_key, display_name, relative_path,
       declared_size_bytes, reserved_bytes, upload_kind, state, finalize_stage, final_object_id,
       final_object_key, attempts, resulting_file_id, resulting_received_file_id, error_code,
       error_request_id, created_at, updated_at, completed_at
  FROM transfer_session_files;

CREATE TABLE tus_uploads_held AS SELECT * FROM tus_uploads;
CREATE TABLE s3_multipart_uploads_held AS SELECT * FROM s3_multipart_uploads;
CREATE TABLE s3_multipart_parts_held AS SELECT * FROM s3_multipart_parts;

DROP TABLE s3_multipart_parts;
DROP TABLE s3_multipart_uploads;
DROP TABLE tus_uploads;
DROP TABLE transfer_session_files;

ALTER TABLE transfer_session_files_next RENAME TO transfer_session_files;

CREATE UNIQUE INDEX ux_tsf_session_client_key ON transfer_session_files(transfer_session_id, client_file_key);
CREATE INDEX        ix_tsf_session_ordinal    ON transfer_session_files(transfer_session_id, ordinal);
CREATE INDEX        ix_tsf_live               ON transfer_session_files(transfer_session_id)
    WHERE state IN ('pending','uploading','finalizing');
CREATE UNIQUE INDEX ux_tsf_final_object_id    ON transfer_session_files(final_object_id);
CREATE UNIQUE INDEX ux_tsf_final_object_key   ON transfer_session_files(final_object_key);
CREATE INDEX        ix_tsf_finalizing         ON transfer_session_files(updated_at)
    WHERE state = 'finalizing';

CREATE TABLE tus_uploads (
    id                              TEXT    NOT NULL PRIMARY KEY,
    transfer_session_file_id        TEXT    NOT NULL,
    owner_user_id                   TEXT    NULL,
    reverse_share_upload_session_id TEXT    NULL,
    upload_length                   INTEGER NULL CHECK (upload_length IS NULL OR upload_length >= 0),
    upload_defer_length             INTEGER NOT NULL DEFAULT 0 CHECK (upload_defer_length IN (0,1)),
    upload_offset                   INTEGER NOT NULL DEFAULT 0 CHECK (upload_offset >= 0),
    staging_path                    TEXT    NOT NULL CHECK (staging_path GLOB 'uploads/*/blob'),
    metadata_json                   TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)
                                                                         AND length(metadata_json) <= 4096),
    checksum_algo                   TEXT    NULL CHECK (checksum_algo IS NULL OR checksum_algo = 'sha256'),
    client_checksum                 TEXT    NULL CHECK (client_checksum IS NULL OR length(client_checksum) = 64),
    state                           TEXT    NOT NULL DEFAULT 'created' CHECK (state IN (
                                        'created','in_progress','completed','terminated','expired')),
    locked_by                       TEXT    NULL CHECK (locked_by IS NULL OR length(locked_by) <= 64),
    lock_expires_at                 TEXT    NULL,
    created_at                      TEXT    NOT NULL,
    updated_at                      TEXT    NOT NULL,
    last_patch_at                   TEXT    NULL,
    expires_at                      TEXT    NOT NULL,
    completed_at                    TEXT    NULL,
    staging_removed_at              TEXT    NULL,

    CHECK ( (upload_defer_length = 1 AND upload_length IS NULL)
         OR (upload_defer_length = 0 AND upload_length IS NOT NULL) ),
    CHECK ( upload_length IS NULL OR upload_offset <= upload_length ),
    CHECK ( (locked_by IS NULL AND lock_expires_at IS NULL)
         OR (locked_by IS NOT NULL AND lock_expires_at IS NOT NULL) ),
    CHECK ( (owner_user_id IS NOT NULL AND reverse_share_upload_session_id IS NULL)
         OR (owner_user_id IS NULL     AND reverse_share_upload_session_id IS NOT NULL) ),

    FOREIGN KEY (transfer_session_file_id) REFERENCES transfer_session_files(id) ON DELETE RESTRICT,
    FOREIGN KEY (owner_user_id)            REFERENCES users(id)                  ON DELETE CASCADE,
    FOREIGN KEY (reverse_share_upload_session_id)
        REFERENCES reverse_share_upload_sessions(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_tus_uploads_tsf     ON tus_uploads(transfer_session_file_id);
CREATE INDEX        ix_tus_uploads_expiry  ON tus_uploads(expires_at) WHERE state IN ('created','in_progress');
CREATE INDEX        ix_tus_uploads_locks   ON tus_uploads(lock_expires_at) WHERE locked_by IS NOT NULL;
CREATE INDEX        ix_tus_uploads_owner   ON tus_uploads(owner_user_id, created_at DESC) WHERE owner_user_id IS NOT NULL;
CREATE INDEX ix_tus_uploads_terminated ON tus_uploads(id) WHERE state = 'terminated';
CREATE INDEX ix_tus_uploads_cleanup ON tus_uploads(id)
    WHERE state IN ('completed','terminated','expired') AND staging_removed_at IS NULL;
CREATE INDEX ix_tus_uploads_retention ON tus_uploads(staging_removed_at)
    WHERE staging_removed_at IS NOT NULL;

INSERT INTO tus_uploads
    (id, transfer_session_file_id, owner_user_id, reverse_share_upload_session_id, upload_length,
     upload_defer_length, upload_offset, staging_path, metadata_json, checksum_algo,
     client_checksum, state, locked_by, lock_expires_at, created_at, updated_at, last_patch_at,
     expires_at, completed_at, staging_removed_at)
SELECT id, transfer_session_file_id, owner_user_id, reverse_share_upload_session_id, upload_length,
       upload_defer_length, upload_offset, staging_path, metadata_json, checksum_algo,
       client_checksum, state, locked_by, lock_expires_at, created_at, updated_at, last_patch_at,
       expires_at, completed_at, staging_removed_at
  FROM tus_uploads_held;

CREATE TABLE s3_multipart_uploads (
    id                              TEXT    NOT NULL PRIMARY KEY,
    transfer_session_file_id        TEXT    NOT NULL,
    s3_upload_id                    TEXT    NOT NULL CHECK (length(s3_upload_id) BETWEEN 1 AND 1024),
    bucket                          TEXT    NOT NULL CHECK (length(bucket) BETWEEN 1 AND 255),
    object_key                      TEXT    NOT NULL CHECK (
                                        length(object_key) = 46
                                        AND object_key GLOB 'objects/[0-9a-f][0-9a-f]/[0-9a-f][0-9a-f]/[0-9a-f]*'
                                        AND substr(object_key, -32) NOT GLOB '*[^0-9a-f]*'),
    owner_user_id                   TEXT    NULL,
    reverse_share_upload_session_id TEXT    NULL,
    total_size_bytes                INTEGER NULL CHECK (total_size_bytes IS NULL OR total_size_bytes >= 0),
    part_size_bytes                 INTEGER NOT NULL CHECK (part_size_bytes >= 5242880),
    part_count                      INTEGER NOT NULL CHECK (part_count BETWEEN 1 AND 10000),
    state                           TEXT    NOT NULL DEFAULT 'created' CHECK (state IN (
                                        'created','in_progress','completing','completed','aborted','abandoned')),
    final_etag                      TEXT    NULL CHECK (final_etag IS NULL OR length(final_etag) <= 128),
    created_at                      TEXT    NOT NULL,
    updated_at                      TEXT    NOT NULL,
    expires_at                      TEXT    NOT NULL,
    completed_at                    TEXT    NULL,
    last_reconciled_at              TEXT    NULL,

    CHECK ( (owner_user_id IS NOT NULL AND reverse_share_upload_session_id IS NULL)
         OR (owner_user_id IS NULL     AND reverse_share_upload_session_id IS NOT NULL) ),

    FOREIGN KEY (transfer_session_file_id) REFERENCES transfer_session_files(id) ON DELETE RESTRICT,
    FOREIGN KEY (owner_user_id)            REFERENCES users(id)                  ON DELETE CASCADE,
    FOREIGN KEY (reverse_share_upload_session_id)
        REFERENCES reverse_share_upload_sessions(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_s3mp_tsf       ON s3_multipart_uploads(transfer_session_file_id);
CREATE UNIQUE INDEX ux_s3mp_object    ON s3_multipart_uploads(object_key);
CREATE UNIQUE INDEX ux_s3mp_provider  ON s3_multipart_uploads(bucket, object_key, s3_upload_id);
CREATE INDEX        ix_s3mp_expiry    ON s3_multipart_uploads(expires_at) WHERE state IN ('created','in_progress');
CREATE INDEX        ix_s3mp_reconcile ON s3_multipart_uploads(last_reconciled_at) WHERE state = 'in_progress';
CREATE INDEX ix_s3mp_abandoned ON s3_multipart_uploads(id) WHERE state = 'abandoned';

INSERT INTO s3_multipart_uploads
    (id, transfer_session_file_id, s3_upload_id, bucket, object_key, owner_user_id,
     reverse_share_upload_session_id, total_size_bytes, part_size_bytes, part_count, state,
     final_etag, created_at, updated_at, expires_at, completed_at, last_reconciled_at)
SELECT id, transfer_session_file_id, s3_upload_id, bucket, object_key, owner_user_id,
       reverse_share_upload_session_id, total_size_bytes, part_size_bytes, part_count, state,
       final_etag, created_at, updated_at, expires_at, completed_at, last_reconciled_at
  FROM s3_multipart_uploads_held;

CREATE TABLE s3_multipart_parts (
    s3_multipart_upload_id TEXT    NOT NULL,
    part_number            INTEGER NOT NULL CHECK (part_number BETWEEN 1 AND 10000),
    size_bytes             INTEGER NOT NULL CHECK (size_bytes > 0),
    etag                   TEXT    NULL CHECK (etag IS NULL OR length(etag) <= 128),
    state                  TEXT    NOT NULL DEFAULT 'planned' CHECK (state IN (
                               'planned','signed','uploaded','verified','failed')),
    attempts               INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    signed_at              TEXT    NULL,
    signed_expires_at      TEXT    NULL,
    uploaded_at            TEXT    NULL,

    CHECK ( state NOT IN ('uploaded','verified') OR etag IS NOT NULL ),

    PRIMARY KEY (s3_multipart_upload_id, part_number),
    FOREIGN KEY (s3_multipart_upload_id) REFERENCES s3_multipart_uploads(id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX ix_s3mp_parts_outstanding
    ON s3_multipart_parts(s3_multipart_upload_id, part_number) WHERE state <> 'uploaded';

INSERT INTO s3_multipart_parts
    (s3_multipart_upload_id, part_number, size_bytes, etag, state, attempts, signed_at,
     signed_expires_at, uploaded_at)
SELECT s3_multipart_upload_id, part_number, size_bytes, etag, state, attempts, signed_at,
       signed_expires_at, uploaded_at
  FROM s3_multipart_parts_held;

DROP TABLE s3_multipart_parts_held;
DROP TABLE s3_multipart_uploads_held;
DROP TABLE tus_uploads_held;

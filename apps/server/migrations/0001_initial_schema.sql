-- Palmr v4.0.0 initial schema

CREATE TABLE app_settings (
    key               TEXT    NOT NULL PRIMARY KEY,
    group_name        TEXT    NOT NULL CHECK (group_name IN (
                                 'general','branding','smtp','security','quotas',
                                 'public_links','retention','audit')),
    value_type        TEXT    NOT NULL CHECK (value_type IN ('string','integer','boolean','json','secret')),
    value_json        TEXT    NULL CHECK (value_json IS NULL
                                          OR (json_valid(value_json) AND length(value_json) <= 65536)),
    secret_ciphertext BLOB    NULL,
    secret_nonce      BLOB    NULL CHECK (secret_nonce IS NULL OR length(secret_nonce) = 24),
    key_version       INTEGER NOT NULL DEFAULT 1 CHECK (key_version >= 1),
    is_secret         INTEGER NOT NULL DEFAULT 0 CHECK (is_secret IN (0,1)),
    updated_at        TEXT    NOT NULL,
    updated_by        TEXT    NULL,

    CHECK ( (is_secret = 0 AND value_json IS NOT NULL
                             AND secret_ciphertext IS NULL AND secret_nonce IS NULL)
         OR (is_secret = 1 AND value_json IS NULL
                             AND secret_ciphertext IS NOT NULL AND secret_nonce IS NOT NULL) ),
    CHECK ( (value_type = 'secret') = (is_secret = 1) ),

    FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL
);

CREATE INDEX ix_app_settings_group ON app_settings(group_name);

CREATE TABLE jobs (
    id                TEXT    NOT NULL PRIMARY KEY,
    kind              TEXT    NOT NULL CHECK (length(kind) BETWEEN 1 AND 64
                                              AND kind NOT GLOB '*[^a-z0-9_.]*'),
    payload_json      TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(payload_json)
                                                            AND length(payload_json) <= 16384),
    state             TEXT    NOT NULL DEFAULT 'pending'
                              CHECK (state IN ('pending','claimed','succeeded','failed','dead')),
    priority          INTEGER NOT NULL DEFAULT 100 CHECK (priority BETWEEN 0 AND 1000),
    run_at            TEXT    NOT NULL,
    attempts          INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts      INTEGER NOT NULL DEFAULT 10 CHECK (max_attempts >= 1),
    claimed_by        TEXT    NULL CHECK (claimed_by IS NULL OR length(claimed_by) <= 64),
    lease_expires_at  TEXT    NULL,
    last_error        TEXT    NULL CHECK (last_error IS NULL OR length(last_error) <= 2048),
    dedup_key         TEXT    NULL CHECK (dedup_key IS NULL OR length(dedup_key) <= 256),
    created_at        TEXT    NOT NULL,
    updated_at        TEXT    NOT NULL,

    CHECK ( (state IN ('claimed') AND claimed_by IS NOT NULL AND lease_expires_at IS NOT NULL)
         OR (state <> 'claimed'  AND claimed_by IS NULL     AND lease_expires_at IS NULL) ),
    CHECK ( state <> 'failed' OR last_error IS NOT NULL )
);

CREATE UNIQUE INDEX ux_jobs_dedup_key ON jobs(dedup_key) WHERE dedup_key IS NOT NULL;
CREATE INDEX ix_jobs_claim        ON jobs(state, run_at, priority) WHERE state = 'pending';
CREATE INDEX ix_jobs_lease        ON jobs(lease_expires_at) WHERE state = 'claimed';
CREATE INDEX ix_jobs_kind_created ON jobs(kind, created_at DESC);

CREATE TABLE email_outbox (
    id            TEXT    NOT NULL PRIMARY KEY,
    kind          TEXT    NOT NULL CHECK (length(kind) BETWEEN 1 AND 64
                                          AND kind NOT GLOB '*[^a-z0-9_.]*'),
    to_email      TEXT    NOT NULL CHECK (length(to_email) BETWEEN 3 AND 254),
    to_name       TEXT    NULL CHECK (to_name IS NULL OR length(to_name) <= 100),
    locale        TEXT    NOT NULL DEFAULT 'en-US' CHECK (locale IN (
                      'ar-SA','de-DE','el-GR','en-US','es-ES','fa-IR','fr-FR','he-IL','hi-IN',
                      'id-ID','it-IT','ja-JP','ko-KR','nl-NL','pl-PL','pt-BR','ru-RU','sv-SE',
                      'th-TH','tr-TR','uk-UA','vi-VN','zh-CN')),
    params_json   TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(params_json)
                                                        AND length(params_json) <= 16384),
    state         TEXT    NOT NULL DEFAULT 'pending'
                          CHECK (state IN ('pending','sending','sent','failed','canceled')),
    batch_key     TEXT    NULL CHECK (batch_key IS NULL OR length(batch_key) <= 256),
    dedup_key     TEXT    NULL CHECK (dedup_key IS NULL OR length(dedup_key) <= 256),
    attempts      INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts  INTEGER NOT NULL DEFAULT 10 CHECK (max_attempts >= 1),
    last_error    TEXT    NULL CHECK (last_error IS NULL OR length(last_error) <= 2048),
    scheduled_at  TEXT    NOT NULL,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL,
    sent_at       TEXT    NULL,
    token_ciphertext BLOB NULL,
    token_nonce      BLOB NULL CHECK (token_nonce IS NULL OR length(token_nonce) = 24),
    key_version      INTEGER NULL CHECK (key_version IS NULL OR key_version >= 1),

    CHECK ( state <> 'sent' OR sent_at IS NOT NULL ),
    CHECK ( (token_ciphertext IS NULL AND token_nonce IS NULL AND key_version IS NULL)
         OR (token_ciphertext IS NOT NULL AND token_nonce IS NOT NULL AND key_version IS NOT NULL) ),
    CHECK ( state IN ('pending','sending') OR token_ciphertext IS NULL )
);

CREATE UNIQUE INDEX ux_email_outbox_dedup_key ON email_outbox(dedup_key) WHERE dedup_key IS NOT NULL;
CREATE INDEX ix_email_outbox_deliver    ON email_outbox(scheduled_at) WHERE state IN ('pending','sending');
CREATE INDEX ix_email_outbox_batch      ON email_outbox(batch_key, created_at DESC) WHERE batch_key IS NOT NULL;
CREATE INDEX ix_email_outbox_kind_state ON email_outbox(kind, state, created_at DESC);

CREATE TABLE audit_events (
    id            TEXT    NOT NULL PRIMARY KEY,
    occurred_at   TEXT    NOT NULL,
    action        TEXT    NOT NULL CHECK (length(action) BETWEEN 1 AND 64
                                           AND action NOT GLOB '*[^A-Z0-9_]*'),
    actor_type    TEXT    NOT NULL CHECK (actor_type IN ('user','anonymous','system','operator_cli')),
    actor_user_id TEXT    NULL,
    actor_label   TEXT    NULL CHECK (actor_label IS NULL OR length(actor_label) <= 320),
    target_type   TEXT    NULL CHECK (target_type IS NULL OR target_type IN (
                              'user','file','folder','share','share_item','reverse_share',
                              'received_file','embed_grant','setting','provider','identity_link',
                              'session','trusted_device','invite','job','branding_asset',
                              'storage_object','tus_upload','s3_multipart_upload','system')),
    target_id     TEXT    NULL CHECK (target_id IS NULL OR length(target_id) <= 128),
    target_label  TEXT    NULL CHECK (target_label IS NULL OR length(target_label) <= 320),
    result        TEXT    NOT NULL CHECK (result IN ('success','failure','denied')),
    error_code    TEXT    NULL CHECK (error_code IS NULL OR length(error_code) <= 64),
    request_id    TEXT    NULL CHECK (request_id IS NULL OR length(request_id) <= 64),
    client_ip     TEXT    NULL CHECK (client_ip IS NULL OR length(client_ip) <= 45),
    user_agent    TEXT    NULL CHECK (user_agent IS NULL OR length(user_agent) <= 256),
    metadata_json TEXT    NULL CHECK (metadata_json IS NULL
                                       OR (json_valid(metadata_json) AND length(metadata_json) <= 4096)),

    CHECK ( (result = 'success') OR error_code IS NOT NULL ),
    CHECK ( actor_type <> 'user' OR actor_user_id IS NOT NULL OR actor_label IS NOT NULL ),

    FOREIGN KEY (actor_user_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE INDEX ix_audit_events_occurred   ON audit_events(occurred_at DESC, id DESC);
CREATE INDEX ix_audit_events_actor      ON audit_events(actor_user_id, occurred_at DESC)
    WHERE actor_user_id IS NOT NULL;
CREATE INDEX ix_audit_events_target     ON audit_events(target_type, target_id, occurred_at DESC)
    WHERE target_id IS NOT NULL;
CREATE INDEX ix_audit_events_action     ON audit_events(action, occurred_at DESC);
CREATE INDEX ix_audit_events_result     ON audit_events(result, occurred_at DESC) WHERE result <> 'success';

CREATE TABLE storage_objects (
    id             TEXT    NOT NULL PRIMARY KEY,
    object_key     TEXT    NOT NULL CHECK (
                       substr(object_key, -32) NOT GLOB '*[^0-9a-f]*'
                       AND (
                            (length(object_key) = 46
                             AND object_key GLOB 'objects/[0-9a-f][0-9a-f]/[0-9a-f][0-9a-f]/[0-9a-f]*')
                         OR (length(object_key) = 46 AND object_key GLOB 'branding/logo/*')
                         OR (length(object_key) = 49 AND object_key GLOB 'branding/favicon/*')
                         OR (length(object_key) = 58 AND object_key GLOB 'branding/login_background/*')
                         OR (length(object_key) = 52 AND object_key GLOB 'branding/email_logo/*')
                         OR (length(object_key) = 58 AND object_key GLOB 'branding/og_default_image/*')
                         OR (length(object_key) = 48 AND object_key GLOB 'branding/avatar/*')
                         OR (length(object_key) = 46 AND object_key GLOB 'branding/hero/*')
                       ) ),
    provider       TEXT    NOT NULL CHECK (provider IN ('local','s3')),
    size_bytes     INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    checksum       TEXT    NULL CHECK (checksum IS NULL OR length(checksum) = 64),
    checksum_algo  TEXT    NULL CHECK (checksum_algo IS NULL OR checksum_algo = 'sha256'),
    state          TEXT    NOT NULL DEFAULT 'active'
                           CHECK (state IN ('active','tombstoned','deleted')),
    refcount       INTEGER NOT NULL DEFAULT 0 CHECK (refcount >= 0 AND refcount <= 1),
    created_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL,
    finalized_at   TEXT    NULL,
    tombstoned_at  TEXT    NULL,
    deleted_at     TEXT    NULL,

    CHECK ( (checksum IS NULL AND checksum_algo IS NULL)
         OR (checksum IS NOT NULL AND checksum_algo IS NOT NULL) ),
    CHECK ( (state = 'active'     AND tombstoned_at IS NULL AND deleted_at IS NULL)
         OR (state = 'tombstoned' AND tombstoned_at IS NOT NULL AND deleted_at IS NULL)
         OR (state = 'deleted'    AND tombstoned_at IS NOT NULL AND deleted_at IS NOT NULL) ),
    CHECK ( (state = 'active' AND refcount = 1 AND finalized_at IS NOT NULL)
         OR (state <> 'active' AND refcount = 0) )
);

CREATE UNIQUE INDEX ux_storage_objects_key    ON storage_objects(object_key);
CREATE INDEX        ix_storage_objects_state  ON storage_objects(state, tombstoned_at) WHERE state <> 'active';
CREATE INDEX        ix_storage_objects_deleted ON storage_objects(deleted_at) WHERE state = 'deleted';

CREATE TABLE file_deletion_queue (
    id                TEXT    NOT NULL PRIMARY KEY,
    storage_object_id TEXT    NOT NULL,
    reason            TEXT    NOT NULL CHECK (reason IN (
                          'file_deleted','folder_deleted','received_deleted','reverse_share_deleted',
                          'received_retention_expired','user_deleted','upload_abandoned','upload_rejected',
                          'quota_rejected','branding_replaced','avatar_replaced','copy_failed','orphan_sweep')),
    state             TEXT    NOT NULL DEFAULT 'pending'
                              CHECK (state IN ('pending','deleting','done','failed')),
    attempts          INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    requested_at      TEXT    NOT NULL,
    not_before        TEXT    NOT NULL,
    last_attempt_at   TEXT    NULL,
    completed_at      TEXT    NULL,
    last_error        TEXT    NULL CHECK (last_error IS NULL OR length(last_error) <= 512),

    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_file_deletion_queue_object ON file_deletion_queue(storage_object_id);
CREATE INDEX        ix_file_deletion_queue_pending
    ON file_deletion_queue(not_before) WHERE state IN ('pending','failed');

CREATE TABLE idempotency_records (
    id                  TEXT    NOT NULL PRIMARY KEY,
    scope_kind          TEXT    NOT NULL CHECK (scope_kind IN ('user','reverse_share_grant','reverse_share_link')),
    scope_id            TEXT    NOT NULL CHECK (length(scope_id) BETWEEN 1 AND 64),
    http_method         TEXT    NOT NULL CHECK (http_method IN ('POST','PUT','PATCH','DELETE')),
    route_template      TEXT    NOT NULL CHECK (length(route_template) BETWEEN 8 AND 256
                                                AND route_template GLOB '/api/v1/*'),
    key_hash            TEXT    NOT NULL CHECK (length(key_hash) = 64),
    request_hash        TEXT    NOT NULL CHECK (length(request_hash) = 64),
    state               TEXT    NOT NULL DEFAULT 'in_progress'
                                CHECK (state IN ('in_progress','completed')),
    lease_expires_at    TEXT    NULL,
    response_status     INTEGER NULL CHECK (response_status IS NULL OR response_status BETWEEN 200 AND 599),
    response_json       TEXT    NULL CHECK (response_json IS NULL
                                            OR (json_valid(response_json) AND length(response_json) <= 16384)),
    response_ciphertext BLOB    NULL CHECK (response_ciphertext IS NULL OR length(response_ciphertext) <= 16400),
    response_nonce      BLOB    NULL CHECK (response_nonce IS NULL OR length(response_nonce) = 24),
    key_version         INTEGER NULL CHECK (key_version IS NULL OR key_version >= 1),
    created_at          TEXT    NOT NULL,
    completed_at        TEXT    NULL,
    expires_at          TEXT    NOT NULL,

    CHECK ( (state = 'in_progress'
             AND lease_expires_at IS NOT NULL AND completed_at IS NULL AND response_status IS NULL
             AND response_json IS NULL AND response_ciphertext IS NULL
             AND response_nonce IS NULL AND key_version IS NULL)
         OR (state = 'completed'
             AND lease_expires_at IS NULL AND completed_at IS NOT NULL AND response_status IS NOT NULL
             AND ( (response_json IS NOT NULL AND response_ciphertext IS NULL
                    AND response_nonce IS NULL AND key_version IS NULL)
                OR (response_json IS NULL AND response_ciphertext IS NOT NULL
                    AND response_nonce IS NOT NULL AND key_version IS NOT NULL) )) )
);

CREATE UNIQUE INDEX ux_idempotency_scope
    ON idempotency_records(scope_kind, scope_id, http_method, route_template, key_hash);
CREATE INDEX        ix_idempotency_expiry ON idempotency_records(expires_at);

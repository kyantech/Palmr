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

CREATE TABLE users (
    id                        TEXT    NOT NULL PRIMARY KEY,
    email                     TEXT    NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    email_normalized          TEXT    NOT NULL,
    email_verified_at         TEXT    NULL,
    pending_email             TEXT    NULL CHECK (pending_email IS NULL OR length(pending_email) BETWEEN 3 AND 254),
    pending_email_normalized  TEXT    NULL,
    username                  TEXT    NOT NULL CHECK (length(username) BETWEEN 3 AND 64),
    username_normalized       TEXT    NOT NULL,
    first_name                TEXT    NOT NULL DEFAULT '' CHECK (length(first_name) <= 100),
    last_name                 TEXT    NOT NULL DEFAULT '' CHECK (length(last_name)  <= 100),
    password_hash             TEXT    NULL,
    password_updated_at       TEXT    NULL,
    must_change_password      INTEGER NOT NULL DEFAULT 0 CHECK (must_change_password IN (0,1)),
    role                      TEXT    NOT NULL DEFAULT 'user' CHECK (role IN ('admin','user')),
    is_active                 INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
    deactivated_at            TEXT    NULL,
    deactivated_by            TEXT    NULL,
    totp_enabled              INTEGER NOT NULL DEFAULT 0 CHECK (totp_enabled IN (0,1)),
    quota_override_mode       TEXT    NOT NULL DEFAULT 'inherit'
                                      CHECK (quota_override_mode IN ('inherit','unlimited','bytes')),
    quota_bytes               INTEGER NULL CHECK (quota_bytes IS NULL OR quota_bytes >= 0),
    used_bytes                INTEGER NOT NULL DEFAULT 0 CHECK (used_bytes >= 0),
    avatar_storage_object_id  TEXT    NULL,
    last_login_at             TEXT    NULL,
    created_at                TEXT    NOT NULL,
    updated_at                TEXT    NOT NULL,
    created_by                TEXT    NULL,

    CHECK ( (quota_override_mode = 'bytes' AND quota_bytes IS NOT NULL)
         OR (quota_override_mode <> 'bytes' AND quota_bytes IS NULL) ),
    CHECK ( (pending_email IS NULL     AND pending_email_normalized IS NULL)
         OR (pending_email IS NOT NULL AND pending_email_normalized IS NOT NULL) ),
    CHECK ( (is_active = 1 AND deactivated_at IS NULL)
         OR (is_active = 0 AND deactivated_at IS NOT NULL) ),

    FOREIGN KEY (avatar_storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by)               REFERENCES users(id)           ON DELETE SET NULL,
    FOREIGN KEY (deactivated_by)           REFERENCES users(id)           ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_users_email_normalized    ON users(email_normalized);
CREATE UNIQUE INDEX ux_users_username_normalized ON users(username_normalized);
CREATE UNIQUE INDEX ux_users_pending_email_normalized
    ON users(pending_email_normalized) WHERE pending_email_normalized IS NOT NULL;
CREATE INDEX        ix_users_active_admins ON users(id) WHERE role = 'admin' AND is_active = 1;
CREATE INDEX        ix_users_created_at    ON users(created_at DESC);
CREATE INDEX        ix_users_used_bytes    ON users(used_bytes DESC);
CREATE INDEX        ix_users_avatar_object ON users(avatar_storage_object_id) WHERE avatar_storage_object_id IS NOT NULL;

CREATE TABLE user_preferences (
    user_id         TEXT NOT NULL PRIMARY KEY,
    locale          TEXT NOT NULL DEFAULT 'en-US' CHECK (locale IN (
                        'ar-SA','de-DE','el-GR','en-US','es-ES','fa-IR','fr-FR','he-IL','hi-IN',
                        'id-ID','it-IT','ja-JP','ko-KR','nl-NL','pl-PL','pt-BR','ru-RU','sv-SE',
                        'th-TH','tr-TR','uk-UA','vi-VN','zh-CN')),
    theme           TEXT NOT NULL DEFAULT 'system' CHECK (theme IN ('light','dark','system')),
    accent          TEXT NOT NULL DEFAULT 'default'
                         CHECK (accent IN ('default','blue','violet','emerald','amber','rose','slate')),
    files_view_mode TEXT NOT NULL DEFAULT 'table' CHECK (files_view_mode IN ('table','grid')),
    files_sort_key  TEXT NOT NULL DEFAULT 'name'
                         CHECK (files_sort_key IN ('name','size','created_at','updated_at','type')),
    files_sort_dir  TEXT NOT NULL DEFAULT 'asc' CHECK (files_sort_dir IN ('asc','desc')),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE sessions (
    id                  TEXT    NOT NULL PRIMARY KEY,
    user_id             TEXT    NOT NULL,
    token_hash          TEXT    NOT NULL CHECK (length(token_hash) = 64),
    csrf_token_hash     TEXT    NOT NULL CHECK (length(csrf_token_hash) = 64),
    state               TEXT    NOT NULL DEFAULT 'active'
                                CHECK (state IN ('mfa_pending','active','revoked','expired')),
    auth_method         TEXT    NOT NULL
                                CHECK (auth_method IN ('password','password_totp','password_backup_code',
                                                       'password_trusted_device','external','invite','reset')),
    mfa_token_hash      TEXT    NULL CHECK (mfa_token_hash IS NULL OR length(mfa_token_hash) = 64),
    mfa_expires_at      TEXT    NULL,
    mfa_attempts        INTEGER NOT NULL DEFAULT 0 CHECK (mfa_attempts >= 0),
    trusted_device_id   TEXT    NULL,
    identity_link_id    TEXT    NULL,
    created_at          TEXT    NOT NULL,
    last_seen_at        TEXT    NOT NULL,
    last_auth_at        TEXT    NOT NULL,
    idle_expires_at     TEXT    NOT NULL,
    absolute_expires_at TEXT    NOT NULL,
    revoked_at          TEXT    NULL,
    revoked_reason      TEXT    NULL CHECK (revoked_reason IS NULL OR revoked_reason IN (
                                    'logout','user_request','admin_request','password_changed',
                                    'password_reset','role_changed','deactivated','deleted',
                                    'mfa_abandoned','rotated','policy_changed','trusted_device_revoked')),
    ip                  TEXT    NULL CHECK (ip IS NULL OR length(ip) <= 45),
    user_agent          TEXT    NULL CHECK (user_agent IS NULL OR length(user_agent) <= 512),

    CHECK ( (state = 'mfa_pending' AND mfa_token_hash IS NOT NULL AND mfa_expires_at IS NOT NULL)
         OR (state <> 'mfa_pending' AND mfa_token_hash IS NULL) ),
    CHECK ( (state = 'revoked' AND revoked_at IS NOT NULL AND revoked_reason IS NOT NULL)
         OR (state <> 'revoked' AND revoked_at IS NULL) ),

    FOREIGN KEY (user_id)           REFERENCES users(id)            ON DELETE CASCADE,
    FOREIGN KEY (trusted_device_id) REFERENCES trusted_devices(id)  ON DELETE SET NULL,
    FOREIGN KEY (identity_link_id)  REFERENCES identity_links(id)   ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_sessions_token_hash ON sessions(token_hash);
CREATE UNIQUE INDEX ux_sessions_mfa_token_hash ON sessions(mfa_token_hash) WHERE mfa_token_hash IS NOT NULL;
CREATE INDEX        ix_sessions_user_active   ON sessions(user_id, created_at DESC) WHERE state = 'active';
CREATE INDEX        ix_sessions_absolute_exp  ON sessions(absolute_expires_at) WHERE state IN ('active','mfa_pending');
CREATE INDEX        ix_sessions_idle_exp      ON sessions(idle_expires_at)     WHERE state = 'active';
CREATE INDEX        ix_sessions_prune         ON sessions(absolute_expires_at) WHERE state IN ('revoked','expired');

CREATE TABLE trusted_devices (
    id           TEXT NOT NULL PRIMARY KEY,
    user_id      TEXT NOT NULL,
    token_hash   TEXT NOT NULL CHECK (length(token_hash) = 64),
    label        TEXT NULL CHECK (label IS NULL OR length(label) <= 100),
    created_at   TEXT NOT NULL,
    last_used_at TEXT NULL,
    expires_at   TEXT NOT NULL,
    revoked_at   TEXT NULL,
    ip           TEXT NULL CHECK (ip IS NULL OR length(ip) <= 45),
    user_agent   TEXT NULL CHECK (user_agent IS NULL OR length(user_agent) <= 512),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_trusted_devices_token_hash ON trusted_devices(token_hash);
CREATE INDEX        ix_trusted_devices_user       ON trusted_devices(user_id, created_at DESC);
CREATE INDEX        ix_trusted_devices_expiry     ON trusted_devices(expires_at) WHERE revoked_at IS NULL;

CREATE TABLE totp_secrets (
    user_id           TEXT    NOT NULL PRIMARY KEY,
    secret_ciphertext BLOB    NOT NULL,
    secret_nonce      BLOB    NOT NULL CHECK (length(secret_nonce) = 24),
    key_version       INTEGER NOT NULL DEFAULT 1 CHECK (key_version >= 1),
    algorithm         TEXT    NOT NULL DEFAULT 'sha1'   CHECK (algorithm IN ('sha1','sha256','sha512')),
    digits            INTEGER NOT NULL DEFAULT 6        CHECK (digits IN (6,8)),
    period_seconds    INTEGER NOT NULL DEFAULT 30       CHECK (period_seconds BETWEEN 15 AND 120),
    state             TEXT    NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','active')),
    last_used_step    INTEGER NULL CHECK (last_used_step IS NULL OR last_used_step >= 0),
    confirmed_at      TEXT    NULL,
    created_at        TEXT    NOT NULL,
    updated_at        TEXT    NOT NULL,

    CHECK ( (state = 'active' AND confirmed_at IS NOT NULL)
         OR (state = 'pending' AND confirmed_at IS NULL) ),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE totp_backup_codes (
    id         TEXT NOT NULL PRIMARY KEY,
    user_id    TEXT NOT NULL,
    batch_id   TEXT NOT NULL,
    code_hash  TEXT NOT NULL CHECK (length(code_hash) = 64),
    created_at TEXT NOT NULL,
    used_at    TEXT NULL,
    used_ip    TEXT NULL CHECK (used_ip IS NULL OR length(used_ip) <= 45),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_totp_backup_codes_user_code ON totp_backup_codes(user_id, code_hash);
CREATE INDEX        ix_totp_backup_codes_unused    ON totp_backup_codes(user_id) WHERE used_at IS NULL;

CREATE TABLE password_reset_tokens (
    id             TEXT NOT NULL PRIMARY KEY,
    user_id        TEXT NOT NULL,
    token_hash     TEXT NOT NULL CHECK (length(token_hash) = 64),
    created_at     TEXT NOT NULL,
    expires_at     TEXT NOT NULL,
    used_at        TEXT NULL,
    invalidated_at TEXT NULL,
    requested_ip   TEXT NULL CHECK (requested_ip IS NULL OR length(requested_ip) <= 45),

    CHECK (used_at IS NULL OR invalidated_at IS NULL),

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_password_reset_token_hash ON password_reset_tokens(token_hash);
CREATE INDEX        ix_password_reset_live       ON password_reset_tokens(user_id)
    WHERE used_at IS NULL AND invalidated_at IS NULL;
CREATE INDEX        ix_password_reset_expiry     ON password_reset_tokens(expires_at);

CREATE TABLE email_verifications (
    id               TEXT NOT NULL PRIMARY KEY,
    user_id          TEXT NOT NULL,
    purpose          TEXT NOT NULL CHECK (purpose IN ('email_change','invite_email','initial_admin')),
    email            TEXT NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    email_normalized TEXT NOT NULL,
    token_hash       TEXT NOT NULL CHECK (length(token_hash) = 64),
    created_at       TEXT NOT NULL,
    expires_at       TEXT NOT NULL,
    consumed_at      TEXT NULL,
    invalidated_at   TEXT NULL,
    requested_by     TEXT NULL,

    CHECK (consumed_at IS NULL OR invalidated_at IS NULL),

    FOREIGN KEY (user_id)      REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (requested_by) REFERENCES users(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_email_verifications_token_hash ON email_verifications(token_hash);
CREATE UNIQUE INDEX ux_email_verifications_live
    ON email_verifications(user_id, purpose) WHERE consumed_at IS NULL AND invalidated_at IS NULL;
CREATE INDEX        ix_email_verifications_expiry ON email_verifications(expires_at);

CREATE TABLE invites (
    id               TEXT NOT NULL PRIMARY KEY,
    token_hash       TEXT NOT NULL CHECK (length(token_hash) = 64),
    email            TEXT NULL CHECK (email IS NULL OR length(email) BETWEEN 3 AND 254),
    email_normalized TEXT NULL,
    role             TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin','user')),
    state            TEXT NOT NULL DEFAULT 'pending'
                          CHECK (state IN ('pending','accepted','revoked','expired')),
    created_by       TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    expires_at       TEXT NOT NULL,
    accepted_at      TEXT NULL,
    accepted_user_id TEXT NULL,
    revoked_at       TEXT NULL,
    revoked_by       TEXT NULL,
    token_ciphertext BLOB NULL,
    token_nonce      BLOB NULL CHECK (token_nonce IS NULL OR length(token_nonce) = 24),
    key_version      INTEGER NULL CHECK (key_version IS NULL OR key_version >= 1),

    CHECK ( (token_ciphertext IS NULL AND token_nonce IS NULL AND key_version IS NULL)
         OR (token_ciphertext IS NOT NULL AND token_nonce IS NOT NULL AND key_version IS NOT NULL) ),
    CHECK ( state = 'pending' OR token_ciphertext IS NULL ),
    CHECK ( (email IS NULL AND email_normalized IS NULL)
         OR (email IS NOT NULL AND email_normalized IS NOT NULL) ),
    CHECK ( (state = 'accepted' AND accepted_at IS NOT NULL AND accepted_user_id IS NOT NULL)
         OR (state <> 'accepted' AND accepted_at IS NULL AND accepted_user_id IS NULL) ),
    CHECK ( (state = 'revoked' AND revoked_at IS NOT NULL)
         OR (state <> 'revoked' AND revoked_at IS NULL) ),

    FOREIGN KEY (created_by)       REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (accepted_user_id) REFERENCES users(id) ON DELETE SET NULL,
    FOREIGN KEY (revoked_by)       REFERENCES users(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_invites_token_hash ON invites(token_hash);
CREATE UNIQUE INDEX ux_invites_pending_email
    ON invites(email_normalized) WHERE state = 'pending' AND email_normalized IS NOT NULL;
CREATE INDEX        ix_invites_state_created ON invites(state, created_at DESC);
CREATE INDEX        ix_invites_expiry        ON invites(expires_at) WHERE state = 'pending';

CREATE TABLE login_attempts (
    id                    TEXT NOT NULL PRIMARY KEY,
    at                    TEXT NOT NULL,
    identifier_normalized TEXT NOT NULL CHECK (length(identifier_normalized) <= 254),
    user_id               TEXT NULL,
    provider_id           TEXT NULL,
    method                TEXT NOT NULL CHECK (method IN ('password','totp','backup_code','trusted_device','external')),
    result                TEXT NOT NULL CHECK (result IN (
                              'success','bad_credentials','unknown_identifier','inactive','locked_out',
                              'totp_required','totp_failed','password_auth_disabled','rate_limited',
                              'must_change_password','provider_denied')),
    ip                    TEXT NULL CHECK (ip IS NULL OR length(ip) <= 45),
    user_agent            TEXT NULL CHECK (user_agent IS NULL OR length(user_agent) <= 512),
    request_id            TEXT NULL CHECK (request_id IS NULL OR length(request_id) <= 64),

    FOREIGN KEY (user_id)     REFERENCES users(id)              ON DELETE CASCADE,
    FOREIGN KEY (provider_id) REFERENCES identity_providers(id) ON DELETE SET NULL
);

CREATE INDEX ix_login_attempts_identifier ON login_attempts(identifier_normalized, at DESC);
CREATE INDEX ix_login_attempts_ip         ON login_attempts(ip, at DESC) WHERE ip IS NOT NULL;
CREATE INDEX ix_login_attempts_at         ON login_attempts(at);
CREATE INDEX ix_login_attempts_user       ON login_attempts(user_id, at DESC) WHERE user_id IS NOT NULL;

CREATE TABLE account_lockouts (
    user_id        TEXT    NOT NULL PRIMARY KEY,
    failed_count   INTEGER NOT NULL DEFAULT 0 CHECK (failed_count >= 0),
    first_failed_at TEXT   NULL,
    last_failed_at TEXT    NULL,
    locked_until   TEXT    NULL,
    lock_count     INTEGER NOT NULL DEFAULT 0 CHECK (lock_count >= 0),
    cleared_at     TEXT    NULL,
    cleared_by     TEXT    NULL,
    updated_at     TEXT    NOT NULL,

    FOREIGN KEY (user_id)    REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (cleared_by) REFERENCES users(id) ON DELETE SET NULL
);

CREATE INDEX ix_account_lockouts_locked ON account_lockouts(locked_until) WHERE locked_until IS NOT NULL;

CREATE TABLE identity_providers (
    id                      TEXT    NOT NULL PRIMARY KEY,
    key                     TEXT    NOT NULL CHECK (length(key) BETWEEN 2 AND 40
                                                    AND key NOT GLOB '*[^a-z0-9_-]*'),
    display_name            TEXT    NOT NULL CHECK (length(display_name) BETWEEN 1 AND 64),
    kind                    TEXT    NOT NULL CHECK (kind IN ('oidc','oauth2')),
    preset                  TEXT    NULL CHECK (preset IS NULL OR preset IN (
                                        'google','github','discord','auth0','kinde','zitadel',
                                        'authentik','frontegg','pocket_id','generic')),
    issuer                  TEXT    NULL CHECK (issuer IS NULL OR length(issuer) <= 512),
    discovery_url           TEXT    NULL CHECK (discovery_url IS NULL OR length(discovery_url) <= 512),
    authorization_endpoint  TEXT    NULL CHECK (authorization_endpoint IS NULL OR length(authorization_endpoint) <= 512),
    token_endpoint          TEXT    NULL CHECK (token_endpoint  IS NULL OR length(token_endpoint)  <= 512),
    userinfo_endpoint       TEXT    NULL CHECK (userinfo_endpoint IS NULL OR length(userinfo_endpoint) <= 512),
    jwks_uri                TEXT    NULL CHECK (jwks_uri IS NULL OR length(jwks_uri) <= 512),
    scopes                  TEXT    NOT NULL DEFAULT 'openid profile email' CHECK (length(scopes) <= 512),
    client_id               TEXT    NOT NULL CHECK (length(client_id) BETWEEN 1 AND 512),
    client_secret_ciphertext BLOB   NULL,
    client_secret_nonce     BLOB    NULL CHECK (client_secret_nonce IS NULL OR length(client_secret_nonce) = 24),
    key_version             INTEGER NOT NULL DEFAULT 1 CHECK (key_version >= 1),
    token_auth_method       TEXT    NOT NULL DEFAULT 'client_secret_post'
                                    CHECK (token_auth_method IN ('client_secret_basic','client_secret_post','none')),
    claim_subject           TEXT    NOT NULL DEFAULT 'sub'            CHECK (length(claim_subject) <= 64),
    claim_email             TEXT    NOT NULL DEFAULT 'email'          CHECK (length(claim_email) <= 64),
    claim_email_verified    TEXT    NOT NULL DEFAULT 'email_verified' CHECK (length(claim_email_verified) <= 64),
    claim_username          TEXT    NOT NULL DEFAULT 'preferred_username' CHECK (length(claim_username) <= 64),
    claim_name              TEXT    NOT NULL DEFAULT 'name'           CHECK (length(claim_name) <= 64),
    claim_avatar            TEXT    NOT NULL DEFAULT 'picture'        CHECK (length(claim_avatar) <= 64),
    is_enabled              INTEGER NOT NULL DEFAULT 0 CHECK (is_enabled IN (0,1)),
    auto_provision          INTEGER NOT NULL DEFAULT 0 CHECK (auto_provision IN (0,1)),
    sort_order              INTEGER NOT NULL DEFAULT 0,
    validated_at            TEXT    NULL,
    validation_error        TEXT    NULL CHECK (validation_error IS NULL OR length(validation_error) <= 512),
    created_at              TEXT    NOT NULL,
    updated_at              TEXT    NOT NULL,
    updated_by              TEXT    NULL,

    CHECK ( (client_secret_ciphertext IS NULL     AND client_secret_nonce IS NULL)
         OR (client_secret_ciphertext IS NOT NULL AND client_secret_nonce IS NOT NULL) ),
    CHECK ( kind = 'oidc' OR (authorization_endpoint IS NOT NULL AND token_endpoint IS NOT NULL) ),

    FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_identity_providers_key ON identity_providers(key);
CREATE INDEX        ix_identity_providers_enabled ON identity_providers(sort_order, key) WHERE is_enabled = 1;

CREATE TABLE identity_links (
    id                     TEXT    NOT NULL PRIMARY KEY,
    user_id                TEXT    NOT NULL,
    provider_id            TEXT    NOT NULL,
    subject                TEXT    NOT NULL CHECK (length(subject) BETWEEN 1 AND 255),
    email_at_link          TEXT    NULL CHECK (email_at_link IS NULL OR length(email_at_link) <= 254),
    email_verified_at_link INTEGER NOT NULL DEFAULT 0 CHECK (email_verified_at_link IN (0,1)),
    link_method            TEXT    NOT NULL CHECK (link_method IN ('auto_verified_email','manual','auto_provision')),
    state                  TEXT    NOT NULL DEFAULT 'active' CHECK (state IN ('active','suspended')),
    created_at             TEXT    NOT NULL,
    last_login_at          TEXT    NULL,
    suspended_at           TEXT    NULL,
    avatar_fetched_at      TEXT    NULL,

    CHECK ( (state = 'suspended' AND suspended_at IS NOT NULL)
         OR (state = 'active'    AND suspended_at IS NULL) ),

    FOREIGN KEY (user_id)     REFERENCES users(id)              ON DELETE CASCADE,
    FOREIGN KEY (provider_id) REFERENCES identity_providers(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_identity_links_provider_subject ON identity_links(provider_id, subject);
CREATE UNIQUE INDEX ux_identity_links_user_provider    ON identity_links(user_id, provider_id);
CREATE INDEX        ix_identity_links_user             ON identity_links(user_id);

CREATE TABLE oauth_auth_requests (
    id                        TEXT    NOT NULL PRIMARY KEY,
    provider_id               TEXT    NOT NULL,
    state_hash                TEXT    NOT NULL CHECK (length(state_hash) = 64),
    binding_cookie_hash       TEXT    NOT NULL CHECK (length(binding_cookie_hash) = 64),
    pkce_verifier_ciphertext  BLOB    NOT NULL,
    pkce_verifier_nonce       BLOB    NOT NULL CHECK (length(pkce_verifier_nonce) = 24),
    key_version               INTEGER NOT NULL DEFAULT 1 CHECK (key_version >= 1),
    nonce                     TEXT    NOT NULL CHECK (length(nonce) BETWEEN 16 AND 128),
    redirect_uri              TEXT    NOT NULL CHECK (length(redirect_uri) <= 512),
    post_auth_path            TEXT    NULL CHECK (post_auth_path IS NULL OR
                                        (post_auth_path GLOB '/*' AND post_auth_path NOT GLOB '//*'
                                         AND post_auth_path NOT LIKE '%..%' AND length(post_auth_path) <= 256)),
    purpose                   TEXT    NOT NULL CHECK (purpose IN ('login','link','recent_auth')),
    link_user_id              TEXT    NULL,
    created_at                TEXT    NOT NULL,
    expires_at                TEXT    NOT NULL,
    consumed_at               TEXT    NULL,
    ip                        TEXT    NULL CHECK (ip IS NULL OR length(ip) <= 45),

    CHECK ( (purpose = 'login' AND link_user_id IS NULL)
         OR (purpose <> 'login' AND link_user_id IS NOT NULL) ),

    FOREIGN KEY (provider_id)  REFERENCES identity_providers(id) ON DELETE CASCADE,
    FOREIGN KEY (link_user_id) REFERENCES users(id)              ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_oauth_auth_requests_state ON oauth_auth_requests(state_hash);
CREATE INDEX        ix_oauth_auth_requests_expiry ON oauth_auth_requests(expires_at) WHERE consumed_at IS NULL;

CREATE TABLE folders (
    id              TEXT    NOT NULL PRIMARY KEY,
    owner_id        TEXT    NOT NULL,
    parent_id       TEXT    NULL,
    name            TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 255
                                            AND name NOT LIKE '%/%'
                                            AND name NOT IN ('.','..')),
    name_normalized TEXT    NOT NULL CHECK (length(name_normalized) BETWEEN 1 AND 255),
    description     TEXT    NULL CHECK (description IS NULL OR length(description) <= 2000),
    depth           INTEGER NOT NULL CHECK (depth BETWEEN 0 AND 64),
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,

    FOREIGN KEY (owner_id)  REFERENCES users(id)   ON DELETE RESTRICT,
    FOREIGN KEY (parent_id) REFERENCES folders(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_folders_sibling_name
    ON folders(owner_id, parent_id, name_normalized) WHERE parent_id IS NOT NULL;
CREATE UNIQUE INDEX ux_folders_root_name
    ON folders(owner_id, name_normalized)            WHERE parent_id IS NULL;
CREATE INDEX        ix_folders_parent   ON folders(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX        ix_folders_owner    ON folders(owner_id, name_normalized);

CREATE TABLE files (
    id                 TEXT    NOT NULL PRIMARY KEY,
    owner_id           TEXT    NOT NULL,
    folder_id          TEXT    NULL,
    storage_object_id  TEXT    NOT NULL,
    name               TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 255
                                               AND name NOT LIKE '%/%'
                                               AND name NOT IN ('.','..')),
    name_normalized    TEXT    NOT NULL CHECK (length(name_normalized) BETWEEN 1 AND 255),
    extension          TEXT    NOT NULL DEFAULT '' CHECK (length(extension) <= 32),
    description        TEXT    NULL CHECK (description IS NULL OR length(description) <= 2000),
    size_bytes         INTEGER NOT NULL CHECK (size_bytes >= 0),
    mime_type          TEXT    NOT NULL DEFAULT 'application/octet-stream' CHECK (length(mime_type) <= 255),
    mime_source        TEXT    NOT NULL DEFAULT 'fallback'
                               CHECK (mime_source IN ('sniffed','extension','client_hint','fallback')),
    created_at         TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL,

    FOREIGN KEY (owner_id)          REFERENCES users(id)           ON DELETE RESTRICT,
    FOREIGN KEY (folder_id)         REFERENCES folders(id)         ON DELETE RESTRICT,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_files_sibling_name
    ON files(owner_id, folder_id, name_normalized) WHERE folder_id IS NOT NULL;
CREATE UNIQUE INDEX ux_files_root_name
    ON files(owner_id, name_normalized)             WHERE folder_id IS NULL;
CREATE UNIQUE INDEX ux_files_storage_object ON files(storage_object_id);
CREATE INDEX        ix_files_folder         ON files(folder_id) WHERE folder_id IS NOT NULL;
CREATE INDEX        ix_files_owner_created  ON files(owner_id, created_at DESC);
CREATE INDEX        ix_files_owner_updated  ON files(owner_id, updated_at DESC);
CREATE INDEX        ix_files_owner_size     ON files(owner_id, size_bytes DESC);

CREATE VIRTUAL TABLE files_fts USING fts5(
    name,
    description,
    content       = 'files',
    content_rowid = 'rowid',
    tokenize      = 'unicode61 remove_diacritics 2',
    prefix        = '2 3 4'
);

CREATE TRIGGER files_fts_ai AFTER INSERT ON files BEGIN
    INSERT INTO files_fts(rowid, name, description) VALUES (new.rowid, new.name, new.description);
END;
CREATE TRIGGER files_fts_ad AFTER DELETE ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name, description) VALUES ('delete', old.rowid, old.name, old.description);
END;
CREATE TRIGGER files_fts_au AFTER UPDATE OF name, description ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name, description) VALUES ('delete', old.rowid, old.name, old.description);
    INSERT INTO files_fts(rowid, name, description) VALUES (new.rowid, new.name, new.description);
END;

CREATE TABLE shares (
    id               TEXT    NOT NULL PRIMARY KEY,
    owner_id         TEXT    NOT NULL,
    public_id        TEXT    NOT NULL CHECK (length(public_id) BETWEEN 16 AND 32),
    alias            TEXT    NOT NULL CHECK (length(alias) BETWEEN 3 AND 64
                                             AND alias NOT GLOB '*[^a-z0-9_-]*'),
    name             TEXT    NULL CHECK (name IS NULL OR length(name) <= 255),
    description      TEXT    NULL CHECK (description IS NULL OR length(description) <= 2000),
    password_hash    TEXT    NULL,
    expires_at       TEXT    NULL,
    max_views        INTEGER NULL CHECK (max_views IS NULL OR max_views > 0),
    max_downloads    INTEGER NULL CHECK (max_downloads IS NULL OR max_downloads > 0),
    view_count       INTEGER NOT NULL DEFAULT 0 CHECK (view_count >= 0),
    download_count   INTEGER NOT NULL DEFAULT 0 CHECK (download_count >= 0),
    is_active        INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
    suspended_at     TEXT    NULL,
    suspended_reason TEXT    NULL CHECK (suspended_reason IS NULL OR
                                         suspended_reason IN ('owner_deactivated','admin_action','deleting')),
    notify_recipients INTEGER NOT NULL DEFAULT 1 CHECK (notify_recipients IN (0,1)),
    show_owner       INTEGER NOT NULL DEFAULT 0 CHECK (show_owner IN (0,1)),
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL,
    last_accessed_at TEXT    NULL,

    CHECK ( (suspended_at IS NULL AND suspended_reason IS NULL)
         OR (suspended_at IS NOT NULL AND suspended_reason IS NOT NULL) ),

    FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_shares_alias     ON shares(alias);
CREATE UNIQUE INDEX ux_shares_public_id ON shares(public_id);
CREATE INDEX        ix_shares_owner     ON shares(owner_id, created_at DESC);
CREATE INDEX        ix_shares_expiry    ON shares(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX        ix_shares_owner_active ON shares(owner_id) WHERE is_active = 1 AND suspended_at IS NULL;

CREATE TABLE share_items (
    id        TEXT NOT NULL PRIMARY KEY,
    share_id  TEXT NOT NULL,
    item_type TEXT NOT NULL CHECK (item_type IN ('file','folder')),
    file_id   TEXT NULL,
    folder_id TEXT NULL,
    added_at  TEXT NOT NULL,

    CHECK ( (item_type = 'file'   AND file_id IS NOT NULL AND folder_id IS NULL)
         OR (item_type = 'folder' AND folder_id IS NOT NULL AND file_id IS NULL) ),

    FOREIGN KEY (share_id)  REFERENCES shares(id)  ON DELETE CASCADE,
    FOREIGN KEY (file_id)   REFERENCES files(id)   ON DELETE CASCADE,
    FOREIGN KEY (folder_id) REFERENCES folders(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_share_items_file   ON share_items(share_id, file_id)   WHERE file_id   IS NOT NULL;
CREATE UNIQUE INDEX ux_share_items_folder ON share_items(share_id, folder_id) WHERE folder_id IS NOT NULL;
CREATE INDEX        ix_share_items_share  ON share_items(share_id);
CREATE INDEX        ix_share_items_file_rev   ON share_items(file_id)   WHERE file_id   IS NOT NULL;
CREATE INDEX        ix_share_items_folder_rev ON share_items(folder_id) WHERE folder_id IS NOT NULL;

CREATE TABLE share_recipients (
    id                 TEXT    NOT NULL PRIMARY KEY,
    share_id           TEXT    NOT NULL,
    email              TEXT    NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    email_normalized   TEXT    NOT NULL,
    added_at           TEXT    NOT NULL,
    last_notified_at   TEXT    NULL,
    notify_count       INTEGER NOT NULL DEFAULT 0 CHECK (notify_count >= 0),
    last_notify_state  TEXT    NULL CHECK (last_notify_state IS NULL OR
                                           last_notify_state IN ('pending','sent','failed','skipped_no_smtp')),

    FOREIGN KEY (share_id) REFERENCES shares(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_share_recipients_email ON share_recipients(share_id, email_normalized);
CREATE INDEX        ix_share_recipients_share ON share_recipients(share_id, added_at);

CREATE TABLE share_grants (
    id           TEXT NOT NULL PRIMARY KEY,
    share_id     TEXT NOT NULL,
    token_hash   TEXT NOT NULL CHECK (length(token_hash) = 64),
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    last_used_at TEXT NULL,
    revoked_at   TEXT NULL,
    ip           TEXT NULL CHECK (ip IS NULL OR length(ip) <= 45),
    user_agent   TEXT NULL CHECK (user_agent IS NULL OR length(user_agent) <= 512),

    FOREIGN KEY (share_id) REFERENCES shares(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX ux_share_grants_token  ON share_grants(token_hash);
CREATE INDEX        ix_share_grants_share  ON share_grants(share_id, expires_at);
CREATE INDEX        ix_share_grants_expiry ON share_grants(expires_at);

CREATE TABLE share_access_events (
    id           TEXT    NOT NULL PRIMARY KEY,
    share_id     TEXT    NOT NULL,
    at           TEXT    NOT NULL,
    kind         TEXT    NOT NULL CHECK (kind IN (
                     'view','password_ok','password_fail','preview','range',
                     'download_single','download_selected','download_all','presign_issued')),
    counted_as   TEXT    NOT NULL CHECK (counted_as IN ('view','download','none')),
    grant_id     TEXT    NULL,
    file_id      TEXT    NULL,
    item_count   INTEGER NULL CHECK (item_count IS NULL OR item_count >= 0),
    bytes_authorized INTEGER NULL CHECK (bytes_authorized IS NULL OR bytes_authorized >= 0),
    ip           TEXT    NULL CHECK (ip IS NULL OR length(ip) <= 45),
    user_agent   TEXT    NULL CHECK (user_agent IS NULL OR length(user_agent) <= 512),
    request_id   TEXT    NULL CHECK (request_id IS NULL OR length(request_id) <= 64),

    FOREIGN KEY (share_id) REFERENCES shares(id)       ON DELETE CASCADE,
    FOREIGN KEY (grant_id) REFERENCES share_grants(id) ON DELETE SET NULL,
    FOREIGN KEY (file_id)  REFERENCES files(id)        ON DELETE SET NULL
);

CREATE INDEX ix_share_access_events_share ON share_access_events(share_id, at DESC);
CREATE INDEX ix_share_access_events_at    ON share_access_events(at);
CREATE INDEX ix_share_access_events_dedup
    ON share_access_events(share_id, grant_id, at DESC) WHERE counted_as = 'view';

CREATE TABLE embed_grants (
    id               TEXT    NOT NULL PRIMARY KEY,
    file_id          TEXT    NOT NULL,
    owner_id         TEXT    NOT NULL,
    public_id        TEXT    NOT NULL CHECK (length(public_id) BETWEEN 16 AND 32),
    token_hash       TEXT    NOT NULL CHECK (length(token_hash) = 64),
    label            TEXT    NULL CHECK (label IS NULL OR length(label) <= 100),
    created_at       TEXT    NOT NULL,
    expires_at       TEXT    NULL,
    revoked_at       TEXT    NULL,
    last_accessed_at TEXT    NULL,
    access_count     INTEGER NOT NULL DEFAULT 0 CHECK (access_count >= 0),

    FOREIGN KEY (file_id)  REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_embed_grants_token     ON embed_grants(token_hash);
CREATE UNIQUE INDEX ux_embed_grants_public_id ON embed_grants(public_id);
CREATE INDEX        ix_embed_grants_file      ON embed_grants(file_id);
CREATE INDEX        ix_embed_grants_owner     ON embed_grants(owner_id, created_at DESC);
CREATE INDEX        ix_embed_grants_expiry    ON embed_grants(expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE reverse_shares (
    id                     TEXT    NOT NULL PRIMARY KEY,
    owner_id               TEXT    NOT NULL,
    public_id              TEXT    NOT NULL CHECK (length(public_id) BETWEEN 16 AND 32),
    alias                  TEXT    NOT NULL CHECK (length(alias) BETWEEN 3 AND 64
                                                   AND alias NOT GLOB '*[^a-z0-9_-]*'),
    name                   TEXT    NULL CHECK (name IS NULL OR length(name) <= 255),
    description            TEXT    NULL CHECK (description IS NULL OR length(description) <= 2000),
    password_hash          TEXT    NULL,
    is_active              INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
    suspended_at           TEXT    NULL,
    suspended_reason       TEXT    NULL CHECK (suspended_reason IS NULL OR
                                               suspended_reason IN ('owner_deactivated','admin_action','deleting')),
    expires_at             TEXT    NULL,
    max_files              INTEGER NULL CHECK (max_files IS NULL OR max_files > 0),
    max_file_size_bytes    INTEGER NULL CHECK (max_file_size_bytes IS NULL OR max_file_size_bytes > 0),
    allowed_extensions     TEXT    NULL CHECK (allowed_extensions IS NULL OR
                                               (json_valid(allowed_extensions)
                                                AND json_type(allowed_extensions) = 'array'
                                                AND length(allowed_extensions) <= 2048)),
    name_field             TEXT    NOT NULL DEFAULT 'optional' CHECK (name_field  IN ('hidden','optional','required')),
    email_field            TEXT    NOT NULL DEFAULT 'optional' CHECK (email_field IN ('hidden','optional','required')),
    description_field      TEXT    NOT NULL DEFAULT 'optional' CHECK (description_field IN ('hidden','optional','required')),
    layout                 TEXT    NOT NULL DEFAULT 'standard' CHECK (layout IN ('standard','hero')),
    hero_background_kind   TEXT    NOT NULL DEFAULT 'brand'
                                   CHECK (hero_background_kind IN ('brand','gradient','image')),
    hero_gradient_preset   TEXT    NULL CHECK (hero_gradient_preset IS NULL OR length(hero_gradient_preset) <= 40),
    hero_asset_id          TEXT    NULL,
    notify_owner           INTEGER NOT NULL DEFAULT 1 CHECK (notify_owner IN (0,1)),
    received_retention_days INTEGER NULL CHECK (received_retention_days IS NULL OR received_retention_days > 0),
    file_count             INTEGER NOT NULL DEFAULT 0 CHECK (file_count >= 0),
    total_bytes            INTEGER NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
    created_at             TEXT    NOT NULL,
    updated_at             TEXT    NOT NULL,

    CHECK ( (suspended_at IS NULL AND suspended_reason IS NULL)
         OR (suspended_at IS NOT NULL AND suspended_reason IS NOT NULL) ),
    CHECK ( hero_background_kind <> 'image'    OR hero_asset_id IS NOT NULL ),
    CHECK ( hero_background_kind <> 'gradient' OR hero_gradient_preset IS NOT NULL ),

    FOREIGN KEY (owner_id)      REFERENCES users(id)                 ON DELETE RESTRICT,
    FOREIGN KEY (hero_asset_id) REFERENCES reverse_share_assets(id)  ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_reverse_shares_alias     ON reverse_shares(alias);
CREATE UNIQUE INDEX ux_reverse_shares_public_id ON reverse_shares(public_id);
CREATE INDEX        ix_reverse_shares_owner     ON reverse_shares(owner_id, created_at DESC);
CREATE INDEX        ix_reverse_shares_expiry    ON reverse_shares(expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE reverse_share_upload_sessions (
    id                     TEXT    NOT NULL PRIMARY KEY,
    reverse_share_id       TEXT    NOT NULL,
    token_hash             TEXT    NOT NULL CHECK (length(token_hash) = 64),
    state                  TEXT    NOT NULL DEFAULT 'active'
                                   CHECK (state IN ('active','completed','canceled','expired','invalidated')),
    uploader_name          TEXT    NULL CHECK (uploader_name  IS NULL OR length(uploader_name)  <= 100),
    uploader_email         TEXT    NULL CHECK (uploader_email IS NULL OR length(uploader_email) <= 254),
    uploader_email_normalized TEXT NULL,
    submission_description TEXT NULL CHECK (submission_description IS NULL OR length(submission_description) <= 2000),
    locale                 TEXT    NOT NULL DEFAULT 'en-US' CHECK (locale IN (
                               'ar-SA','de-DE','el-GR','en-US','es-ES','fa-IR','fr-FR','he-IL','hi-IN',
                               'id-ID','it-IT','ja-JP','ko-KR','nl-NL','pl-PL','pt-BR','ru-RU','sv-SE',
                               'th-TH','tr-TR','uk-UA','vi-VN','zh-CN')),
    send_confirmation      INTEGER NOT NULL DEFAULT 0 CHECK (send_confirmation IN (0,1)),
    password_verified_at   TEXT    NULL,
    files_uploaded         INTEGER NOT NULL DEFAULT 0 CHECK (files_uploaded >= 0),
    bytes_uploaded         INTEGER NOT NULL DEFAULT 0 CHECK (bytes_uploaded >= 0),
    created_at             TEXT    NOT NULL,
    expires_at             TEXT    NOT NULL,
    last_activity_at       TEXT    NOT NULL,
    completed_at           TEXT    NULL,
    ip                     TEXT    NULL CHECK (ip IS NULL OR length(ip) <= 45),
    user_agent             TEXT    NULL CHECK (user_agent IS NULL OR length(user_agent) <= 512),

    CHECK ( (uploader_email IS NULL AND uploader_email_normalized IS NULL)
         OR (uploader_email IS NOT NULL AND uploader_email_normalized IS NOT NULL) ),

    FOREIGN KEY (reverse_share_id) REFERENCES reverse_shares(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_rs_upload_sessions_token  ON reverse_share_upload_sessions(token_hash);
CREATE INDEX        ix_rs_upload_sessions_share  ON reverse_share_upload_sessions(reverse_share_id, created_at DESC);
CREATE INDEX        ix_rs_upload_sessions_expiry ON reverse_share_upload_sessions(expires_at) WHERE state = 'active';
CREATE INDEX        ix_rs_upload_sessions_batch
    ON reverse_share_upload_sessions(reverse_share_id, uploader_email_normalized) WHERE state IN ('active','completed');

CREATE TABLE reverse_share_assets (
    id                TEXT    NOT NULL PRIMARY KEY,
    reverse_share_id  TEXT    NOT NULL,
    kind              TEXT    NOT NULL DEFAULT 'hero_background' CHECK (kind = 'hero_background'),
    storage_object_id TEXT    NOT NULL,
    mime_type         TEXT    NOT NULL CHECK (mime_type = 'image/webp'),
    size_bytes        INTEGER NOT NULL CHECK (size_bytes > 0),
    width             INTEGER NOT NULL CHECK (width  BETWEEN 1 AND 8192),
    height            INTEGER NOT NULL CHECK (height BETWEEN 1 AND 8192),
    created_at        TEXT    NOT NULL,
    created_by        TEXT    NULL,

    FOREIGN KEY (reverse_share_id)  REFERENCES reverse_shares(id)  ON DELETE RESTRICT,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by)        REFERENCES users(id)           ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_rs_assets_storage_object ON reverse_share_assets(storage_object_id);
CREATE INDEX        ix_rs_assets_share          ON reverse_share_assets(reverse_share_id, created_at DESC);

CREATE TABLE received_files (
    id                 TEXT    NOT NULL PRIMARY KEY,
    owner_id           TEXT    NOT NULL,
    reverse_share_id   TEXT    NOT NULL,
    upload_session_id  TEXT    NULL,
    storage_object_id  TEXT    NOT NULL,
    name               TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 255
                                               AND name NOT LIKE '%/%' AND name NOT IN ('.','..')),
    name_normalized    TEXT    NOT NULL,
    extension          TEXT    NOT NULL DEFAULT '' CHECK (length(extension) <= 32),
    description        TEXT    NULL CHECK (description IS NULL OR length(description) <= 2000),
    size_bytes         INTEGER NOT NULL CHECK (size_bytes >= 0),
    mime_type          TEXT    NOT NULL DEFAULT 'application/octet-stream' CHECK (length(mime_type) <= 255),
    mime_source        TEXT    NOT NULL DEFAULT 'fallback'
                               CHECK (mime_source IN ('sniffed','extension','client_hint','fallback')),
    uploader_name      TEXT    NULL CHECK (uploader_name  IS NULL OR length(uploader_name)  <= 100),
    uploader_email     TEXT    NULL CHECK (uploader_email IS NULL OR length(uploader_email) <= 254),
    uploader_ip        TEXT    NULL CHECK (uploader_ip IS NULL OR length(uploader_ip) <= 45),
    received_at        TEXT    NOT NULL,
    updated_at         TEXT    NOT NULL,
    expires_at         TEXT    NULL,

    FOREIGN KEY (owner_id)          REFERENCES users(id)                          ON DELETE RESTRICT,
    FOREIGN KEY (reverse_share_id)  REFERENCES reverse_shares(id)                 ON DELETE RESTRICT,
    FOREIGN KEY (upload_session_id) REFERENCES reverse_share_upload_sessions(id)  ON DELETE SET NULL,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id)                ON DELETE RESTRICT
);

CREATE UNIQUE INDEX ux_received_files_name    ON received_files(reverse_share_id, name_normalized);
CREATE UNIQUE INDEX ux_received_storage_object ON received_files(storage_object_id);
CREATE INDEX        ix_received_owner         ON received_files(owner_id, received_at DESC);
CREATE INDEX        ix_received_share         ON received_files(reverse_share_id, received_at DESC);
CREATE INDEX        ix_received_expiry        ON received_files(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX        ix_received_owner_size    ON received_files(owner_id, size_bytes DESC);

CREATE VIRTUAL TABLE received_files_fts USING fts5(
    name,
    description,
    content       = 'received_files',
    content_rowid = 'rowid',
    tokenize      = 'unicode61 remove_diacritics 2',
    prefix        = '2 3 4'
);

CREATE TRIGGER received_files_fts_ai AFTER INSERT ON received_files BEGIN
    INSERT INTO received_files_fts(rowid, name, description) VALUES (new.rowid, new.name, new.description);
END;
CREATE TRIGGER received_files_fts_ad AFTER DELETE ON received_files BEGIN
    INSERT INTO received_files_fts(received_files_fts, rowid, name, description) VALUES ('delete', old.rowid, old.name, old.description);
END;
CREATE TRIGGER received_files_fts_au AFTER UPDATE OF name, description ON received_files BEGIN
    INSERT INTO received_files_fts(received_files_fts, rowid, name, description) VALUES ('delete', old.rowid, old.name, old.description);
    INSERT INTO received_files_fts(rowid, name, description) VALUES (new.rowid, new.name, new.description);
END;

CREATE TABLE branding_assets (
    id                TEXT    NOT NULL PRIMARY KEY,
    kind              TEXT    NOT NULL CHECK (kind IN (
                          'logo','favicon','login_background','email_logo','og_default_image')),
    storage_object_id TEXT    NOT NULL,
    mime_type         TEXT    NOT NULL CHECK (mime_type IN ('image/webp','image/png')),
    size_bytes        INTEGER NOT NULL CHECK (size_bytes > 0),
    width             INTEGER NULL CHECK (width  IS NULL OR width  BETWEEN 1 AND 8192),
    height            INTEGER NULL CHECK (height IS NULL OR height BETWEEN 1 AND 8192),
    is_current        INTEGER NOT NULL DEFAULT 0 CHECK (is_current IN (0,1)),
    created_at        TEXT    NOT NULL,
    created_by        TEXT    NULL,

    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by)        REFERENCES users(id)           ON DELETE SET NULL
);

CREATE UNIQUE INDEX ux_branding_assets_storage_object ON branding_assets(storage_object_id);
CREATE UNIQUE INDEX ux_branding_assets_current        ON branding_assets(kind) WHERE is_current = 1;
CREATE INDEX        ix_branding_assets_kind           ON branding_assets(kind, created_at DESC);

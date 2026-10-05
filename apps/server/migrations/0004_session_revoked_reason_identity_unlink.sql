-- Extend the session revocation vocabulary with `identity_provider_unlinked`,
-- the reason recorded when removing an external identity link revokes every
-- session of the affected account. Existing rows are preserved verbatim.
-- SQLite requires a table rebuild to change a CHECK constraint.

CREATE TABLE sessions_new (
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
                                    'mfa_abandoned','rotated','policy_changed','trusted_device_revoked',
                                    'identity_provider_unlinked')),
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

INSERT INTO sessions_new
    (id, user_id, token_hash, csrf_token_hash, state, auth_method, mfa_token_hash,
     mfa_expires_at, mfa_attempts, trusted_device_id, identity_link_id, created_at,
     last_seen_at, last_auth_at, idle_expires_at, absolute_expires_at, revoked_at,
     revoked_reason, ip, user_agent)
SELECT id, user_id, token_hash, csrf_token_hash, state, auth_method, mfa_token_hash,
       mfa_expires_at, mfa_attempts, trusted_device_id, identity_link_id, created_at,
       last_seen_at, last_auth_at, idle_expires_at, absolute_expires_at, revoked_at,
       revoked_reason, ip, user_agent
FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE UNIQUE INDEX ux_sessions_token_hash ON sessions(token_hash);
CREATE UNIQUE INDEX ux_sessions_mfa_token_hash ON sessions(mfa_token_hash) WHERE mfa_token_hash IS NOT NULL;
CREATE INDEX        ix_sessions_user_active   ON sessions(user_id, created_at DESC) WHERE state = 'active';
CREATE INDEX        ix_sessions_absolute_exp  ON sessions(absolute_expires_at) WHERE state IN ('active','mfa_pending');
CREATE INDEX        ix_sessions_idle_exp      ON sessions(idle_expires_at)     WHERE state = 'active';
CREATE INDEX        ix_sessions_prune         ON sessions(absolute_expires_at) WHERE state IN ('revoked','expired');

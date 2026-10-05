-- Align oauth_auth_requests with the accepted external-identity contract:
-- the purpose vocabulary becomes ('login','link','reauth') and the validated
-- post-authentication relative path is bounded at 512 characters. Existing
-- rows are preserved verbatim, with the retired `recent_auth` label mapped to
-- `reauth`. SQLite requires a table rebuild to change a CHECK constraint.

CREATE TABLE oauth_auth_requests_new (
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
                                         AND post_auth_path NOT LIKE '%..%' AND length(post_auth_path) <= 512)),
    purpose                   TEXT    NOT NULL CHECK (purpose IN ('login','link','reauth')),
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

INSERT INTO oauth_auth_requests_new
    (id, provider_id, state_hash, binding_cookie_hash, pkce_verifier_ciphertext,
     pkce_verifier_nonce, key_version, nonce, redirect_uri, post_auth_path, purpose,
     link_user_id, created_at, expires_at, consumed_at, ip)
SELECT id, provider_id, state_hash, binding_cookie_hash, pkce_verifier_ciphertext,
       pkce_verifier_nonce, key_version, nonce, redirect_uri, post_auth_path,
       CASE purpose WHEN 'recent_auth' THEN 'reauth' ELSE purpose END,
       link_user_id, created_at, expires_at, consumed_at, ip
FROM oauth_auth_requests;

DROP TABLE oauth_auth_requests;
ALTER TABLE oauth_auth_requests_new RENAME TO oauth_auth_requests;

CREATE UNIQUE INDEX ux_oauth_auth_requests_state ON oauth_auth_requests(state_hash);
CREATE INDEX        ix_oauth_auth_requests_expiry ON oauth_auth_requests(expires_at) WHERE consumed_at IS NULL;

-- F-AUTH-16 requires a global provider toggle that is actually enforced
-- server-side. Materialize it inside the existing `security` settings group so
-- the current settings framework (snapshot, PATCH, audit, recent-auth) owns it.
INSERT INTO app_settings
    (key, group_name, value_type, value_json, is_secret, key_version, updated_at)
VALUES
    ('auth_providers_enabled', 'security', 'boolean', 'true', 0, 1, '2026-10-02T00:00:00.000Z');

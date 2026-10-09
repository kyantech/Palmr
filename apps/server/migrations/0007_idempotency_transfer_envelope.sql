CREATE TABLE idempotency_records_next (
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
                                            OR (json_valid(response_json)
                                                AND length(CAST(response_json AS BLOB)) <=
                                                    CASE route_template
                                                        WHEN '/api/v1/transfers/sessions' THEN 6291456
                                                        ELSE 16384
                                                    END)),
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

INSERT INTO idempotency_records_next
    (id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash, state,
     lease_expires_at, response_status, response_json, response_ciphertext, response_nonce,
     key_version, created_at, completed_at, expires_at)
SELECT id, scope_kind, scope_id, http_method, route_template, key_hash, request_hash, state,
       lease_expires_at, response_status, response_json, response_ciphertext, response_nonce,
       key_version, created_at, completed_at, expires_at
  FROM idempotency_records;

DROP TABLE idempotency_records;

ALTER TABLE idempotency_records_next RENAME TO idempotency_records;

CREATE UNIQUE INDEX ux_idempotency_scope
    ON idempotency_records(scope_kind, scope_id, http_method, route_template, key_hash);
CREATE INDEX        ix_idempotency_expiry ON idempotency_records(expires_at);

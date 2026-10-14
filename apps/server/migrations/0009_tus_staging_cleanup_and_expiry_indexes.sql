ALTER TABLE tus_uploads ADD COLUMN staging_removed_at TEXT NULL;

CREATE INDEX ix_tus_uploads_cleanup ON tus_uploads(id)
    WHERE state IN ('completed','terminated','expired') AND staging_removed_at IS NULL;

CREATE INDEX ix_tus_uploads_retention ON tus_uploads(staging_removed_at)
    WHERE staging_removed_at IS NOT NULL;

CREATE INDEX ix_transfer_sessions_failed_expiry ON transfer_sessions(expires_at)
    WHERE state = 'failed';

CREATE INDEX ix_transfer_sessions_retention ON transfer_sessions(completed_at)
    WHERE state IN ('completed','canceled','expired') AND completed_at IS NOT NULL;

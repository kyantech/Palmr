CREATE INDEX ix_s3mp_abandoned ON s3_multipart_uploads(id) WHERE state = 'abandoned';

CREATE INDEX ix_tus_uploads_terminated ON tus_uploads(id) WHERE state = 'terminated';

-- Per-provider e-mail linking policy (ADR 0012 rule 3): OIDC providers default to enabled, OAuth2 providers to disabled.

ALTER TABLE identity_providers
    ADD COLUMN allow_email_linking INTEGER NOT NULL DEFAULT 0 CHECK (allow_email_linking IN (0,1));

UPDATE identity_providers SET allow_email_linking = 1 WHERE kind = 'oidc';

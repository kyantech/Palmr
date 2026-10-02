# External identity providers: control plane

Implemented by `apps/server/src/features/identity_providers/` (M12-T01). Login, callback, token validation and linking arrive in M12-T02 onward.

## Model

One `IdentityProvider` concept over the `identity_providers` table, with `ProviderVariant::Oidc(OidcProvider)` and `ProviderVariant::OAuth2(OAuth2Provider)`. The types make the protocol rules structural:

| | `oidc` | `oauth2` |
|---|---|---|
| Issuer | required, exact-equal to the discovery document | absent |
| Endpoints | authorization, token and JWKS required (from discovery or explicit); userinfo optional | authorization, token and userinfo required and explicit; no JWKS |
| Discovery | `<issuer>/.well-known/openid-configuration` only | never |
| `allowEmailLinking` default | `true` | `false` |

`slug` is the `key` column. It is immutable and appears in the derived callback URI `PALMR_BASE_URL + /api/v1/auth/providers/{slug}/callback`, which no request can supply and no request header can influence.

`allow_email_linking` was added by the post-freeze migration `0002_add_identity_provider_email_linking.sql`. Existing `oidc` rows became `1`, existing `oauth2` rows `0`, and the column default is `0`. The service writes the protocol default explicitly on creation and the stored value is returned as is.

## Secrets

The client secret is sealed with `SealPurpose::Idp` (`palmr:v1:idp`) and the AAD `idp \0 <provider id>`. It is write-only: absent leaves it unchanged, a string replaces it, `null` clears it (only together with `tokenAuthMethod: none`). Responses, audit metadata, errors and logs carry only `clientSecretConfigured`; audit records presence transitions (`set → set`, `set → unset`).

## Presets

`presets.rs` is static data. Fixed issuers and endpoints (Google, GitHub, Discord) and the Zitadel `client_secret_basic` method are the values Palmr v3 shipped; every other provider is instance-specific and carries no issuer. A preset only fills members a create request omits. The two Custom entries both persist as `generic` and differ by protocol.

## Outbound HTTP

`ProviderHttpClient` owns its own `rustls::ClientConfig` and `RootCertStore` (webpki roots). It never uses or installs a process default crypto provider and shares nothing with the S3, SMTP or avatar clients.

- Provider URLs are `https`; plain `http` is accepted only for a loopback host.
- 10 s total timeout per fetch, 256 KiB response cap (enforced from `Content-Length` and while streaming), no cookies, no `Authorization`, no caller-controlled headers.
- At most 2 redirects. A redirect to the same origin is followed. A redirect to another origin must have a publicly routable address: literal addresses are checked against the SECURITY_MODEL §3.8 deny list and names are resolved by a resolver that refuses the whole answer if any address is denied, so the checked address is the dialled address.
- The initial URL is the admin's own configuration and may be a private address, since self-hosted providers on a LAN are the common case.

## Test endpoint

The checks run concurrently and each is bounded by the fetch limits. A check reports a stable detail code (`unreachable`, `timeout`, `tls_error`, `upstream_error`, `unexpected_status`, `response_too_large`, `malformed_document`, `empty_key_set`, `issuer_mismatch`, `blocked_address`, …); upstream bodies are never copied into a response or into `validation_error`. Success stamps `validated_at`; failure clears it and stores the sanitized `validation_error`. A test that changes either persisted value records `IDENTITY_PROVIDER_UPDATED` with `{"fields":[…]}` (only the changed fields) in the same transaction; an unchanged state records nothing. The stamp is conditional on `updated_at`, so a test that raced a configuration change returns `DATABASE_BUSY` instead of validating the new configuration.

## Deletion

`identity_links.provider_id` is `ON DELETE RESTRICT`. The service checks for links inside the same `BEGIN IMMEDIATE` transaction and refuses with `HasLinks`; the foreign key remains the backstop. SQLite reports a RESTRICT violation as extended code 1811, not 787, so the shared `DbError` classifier does not map it to `ForeignKeyViolation`. The route answers `PROVIDER_HAS_LINKS` (409). The service does not depend on the SQLite code.

## Ordering audit

`PUT /order` records `IDENTITY_PROVIDER_UPDATED` with `{"fields":["sortOrder"]}` for each provider whose persisted `sort_order` changed, in the same transaction as the update. A reorder that changes nothing records nothing, and an audit failure rolls the whole order back.

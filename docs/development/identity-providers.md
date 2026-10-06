# External identity providers: control plane

Implemented by `apps/server/src/features/identity_providers/` (M12-T01 to M12-T05). The safe SSO-only mode arrives in M12-T06.

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

## Callback

`GET /api/v1/auth/providers/{slug}/callback` (`public`, `rl.auth.login`) is a top-level browser navigation, so every outcome is a `303 See Other` whose landing is chosen by the `purpose` stored in the authorization request (never by a request parameter):

| `purpose` | Success | Failure |
|---|---|---|
| `login` | `PALMR_BASE_URL` plus the validated `post_auth_path` stored by the authorize call (default `/overview`) | `/login?error=<CODE>&requestId=<ID>` |
| `link` | `/settings/security` | `/settings/security?error=<CODE>&requestId=<ID>` |
| `reauth` | `/auth/reauth-complete?status=success` | `/auth/reauth-complete?status=error&error=<CODE>&requestId=<ID>` |

`/auth/reauth-complete` is the SPA completion route of the popup SSO re-authentication (FRONTEND_ARCHITECTURE §4.6), not an API route. `requestId` is the callback request's own `X-Request-Id`, read from the request-ID extension (`RequestId::of`); no second id is generated. Only a stable `ErrorCode` and that id cross into the redirect; provider text (`error`, `error_description`) is never read, logged, audited or echoed. The purpose for the landing comes from `ExternalLoginService::run` setting it once the consumed row's binding digest and provider match (`CallbackFailure.purpose`), or from the read-only `recover_purpose` lookup for a provider denial, which consumes nothing; when it cannot be established the landing is the `login` failure form. It is routing context only and changes no validation. Every outcome clears `palmr_oauth` with the original `Path=/api/v1/auth/providers`.

The order is normative and fail-closed (`callback.rs`):

1. an `error` parameter is `PROVIDER_AUTH_DENIED`; nothing is exchanged and no row is touched;
2. a missing `palmr_oauth` cookie, a missing or duplicated `state` or `code` is `PROVIDER_STATE_INVALID` with no row consumed;
3. one short `BEGIN IMMEDIATE` runs the conditional `UPDATE oauth_auth_requests SET consumed_at … WHERE state_hash = ? AND consumed_at IS NULL AND expires_at > ? RETURNING …`; zero rows (unknown, expired, replayed) is `PROVIDER_STATE_INVALID`;
4. from here the row is consumed and stays consumed: the binding cookie digest must equal `binding_cookie_hash` (constant time), the row's provider must be the provider in the path, and `redirect_uri` must equal the URI derived from `PALMR_BASE_URL` exactly. Any mismatch is `PROVIDER_STATE_INVALID` and nothing leaves the process. A disabled provider or a disabled global toggle is `PROVIDER_DISABLED`;
5. the row purpose selects exactly one flow (`Flow::Login | Link | Reauth`, no fall-through). `link` and `reauth` additionally require the presented `palmr_session` to authenticate as the row's `link_user_id` (`PROVIDER_STATE_INVALID` otherwise) *before* anything is exchanged, and an OAuth2 `reauth` row older than the recent-authentication window is `AUTH_RECENT_AUTH_REQUIRED` without contacting the provider;
6. the PKCE verifier is opened with `palmr:v1:oidc` and the request-row AAD (a failure is `PROVIDER_STATE_INVALID`), and the code is exchanged at the token endpoint honouring `token_auth_method` (`client_secret_basic` sends only an `Authorization` header, `client_secret_post` sends `client_id` and `client_secret` in the form, `none` sends `client_id` only). Redirects are never followed. Any transport, status or body failure is `PROVIDER_CODE_EXCHANGE_FAILED`. Access and ID tokens live only for the request;
7. `oidc` runs the T02 `IdTokenValidator` with the stored nonce and the flow's `ValidationPurpose` (`Reauth` makes a missing, stale or future `auth_time` an `AUTH_RECENT_AUTH_REQUIRED`) (`PROVIDER_ID_TOKEN_INVALID`, `PROVIDER_SUBJECT_MISSING`, `PROVIDER_DISCOVERY_FAILED`); a missing `id_token` is `PROVIDER_ID_TOKEN_INVALID`. When the signed token lacks a username, name or picture and the provider has a userinfo endpoint, one best-effort userinfo request may fill only those display fields, and only when its `sub` equals the signed `sub`. `oauth2` fetches userinfo (`PROVIDER_USERINFO_FAILED`);
8. a subject that is empty, longer than 255 characters or contains control characters is `PROVIDER_SUBJECT_MISSING`.

No network request is made while a SQLite write transaction is open. The `login` sign-in itself is a single transaction (`resolve.rs`, `provision.rs`, the shared `AuthService::issue_session_in_tx`): resolution, link or account creation, their audit rows, the session, `identity_links.last_login_at` and `sessions.identity_link_id` commit together, and a refusal by the account-state gate rolls the whole transaction back. The `LOGIN_SUCCEEDED` event (`method = external`) is enqueued after the commit.

## Link, unlink and SSO re-authentication (M12-T05)

`login` is the only purpose the public `POST /authorize` accepts. The other two enter through authenticated routes that call the same `IdentityProviderService::authorize` with a bound user id:

| Purpose | Entry | Class |
|---|---|---|
| `link` | `POST /api/v1/auth/providers/{slug}/link` (`link_routes.rs`, `rl.write`) | `authenticated+recent-auth` |
| `reauth` | SSO branch of `POST /api/v1/auth/reauthenticate` (empty body, `password_hash IS NULL`) | `authenticated` |

`link` stores `purpose = link`, `link_user_id` and `post_auth_path = /settings/security`; it is refused before a row is minted with `PROVIDER_IDENTITY_ALREADY_LINKED` when the caller already holds an identity from that provider. `reauth` resolves the provider from `sessions.identity_link_id` (never an arbitrary link), requires the link to belong to the caller and be `active`, and answers `202 { accepted, externalReauthUrl }`, where `externalReauthUrl` is the real provider URL carrying `prompt=login` (and `max_age=0` for OIDC only).

The callback (`link.rs`, `reauth.rs`) never enters `resolve::resolve`:

* **link** re-reads the session inside one short write transaction (live, same user, still inside the recent-authentication window, otherwise `AUTH_RECENT_AUTH_REQUIRED`) and inserts `identity_links(link_method = 'manual')` with `IDENTITY_LINK_CREATED {via: manual, provider_id}`. The provider e-mail only fills `email_at_link`/`email_verified_at_link`; it never decides. The exact subject already bound to the same user is a no-op success; a subject bound to another user, or another subject for the same provider, is `PROVIDER_IDENTITY_ALREADY_LINKED`. No session is issued, rotated or stamped.
* **reauth** proves the same user, provider and subject as the session's link inside one short write transaction and sets `sessions.last_auth_at` for that session only; a mismatch is `AUTH_RECENT_AUTH_REQUIRED`. The token is not rotated, nothing else changes and Palmr TOTP is never applied.

`GET /api/v1/identity-links` and `GET /api/v1/admin/users/{id}/identity-links` return `{ id, providerSlug, providerDisplayName, externalSubject, emailAtLink, linkedAt, lastUsedAt }` (keyset-paged, oldest first). `DELETE /api/v1/identity-links/{id}` and `DELETE /api/v1/admin/users/{id}/identity-links/{linkId}` share `ExternalLoginService::unlink`: one write transaction scoped by `(user_id, link id)` deletes the link, revokes **every** session of the target (`revoked_reason = identity_provider_unlinked`, migration `0004`) and every trusted device, and writes `IDENTITY_LINK_REMOVED`. `assert_unlink_allowed` is the single guard seam: the self route refuses the only login path of a passwordless account (`IDENTITY_LINK_LAST_LOGIN_PATH`), followed by the SSO-only standing invariant (`PASSWORD_LOGIN_DISABLE_UNSAFE`, below), so a refused unlink changes, revokes and audits nothing.

## Safe SSO-only mode (M12-T06)

`app_settings.password_login_enabled` (migration `0005`, `security` group, default `true`) is the instance-wide local password login switch (Decision 52). `AppSettings.security.password_login_enabled` feeds `auth::login::password_login_enabled`, which login, password reset, profile password change, admin password reset and `GET /api/v1/bootstrap` already consult. The key is registered in `SETTINGS_REGISTRY` but is deliberately not a `Field` of the `security` group, so `PATCH /api/v1/admin/settings/security` answers `SETTING_UNKNOWN` for `passwordLoginEnabled` and the generic path cannot bypass the guard; `PUT /api/v1/admin/auth/password-login` (`password_login_routes.rs`, `PasswordLoginService`) is the only writer.

**One structural path predicate.** `password_login.rs` owns the SQL for a usable Admin external login path and every caller shares it:

```text
auth_providers_enabled = true
identity_providers.is_enabled = 1
identity_links.state = 'active'
users.role = 'admin' AND users.is_active = 1
```

Disable admission adds, on top of that structural path, `validated_at IS NOT NULL AND validation_error IS NULL AND now - validated_at <= 24 h`.

`validated_at` / `validation_error` are the persisted result of `POST /admin/providers/{id}/test`; the older `last_validated_at` / `last_validation_ok` names in earlier prose never existed in the schema.

* **Admission (disable).** `evaluate` reads the setting flags, the structural path list and the enabled providers on one connection (the write transaction for `PUT`, a read-pool connection for the advisory `GET`) and produces the `{ passwordLoginEnabled, canDisable, blockers, safeAdminLoginPaths }` body. A provider is *recent* when `validation_error IS NULL` and `now - validated_at <= 24 h` (inclusive, injected `Clock`, not configurable); `providerValidated` in the body reports exactly that. Blockers are `NO_VALIDATED_PROVIDER` (no provider enabled, or none recently tested) or `PASSWORD_LOGIN_DISABLE_UNSAFE` (providers globally disabled, no active Admin linked to a recent provider, or the acting Admin is not). `PUT` runs inside `SettingsService::update_group`: write transaction, `evaluate`, setting write, `PASSWORD_LOGIN_DISABLED` audit, commit, then the snapshot reload. Any failure, including the audit, rolls everything back before the swap. Enabling skips admission; a request for the current state returns it without writing or auditing.
* **Standing check.** `assert_safe_sso_after_change(tx, Projection)` reads `password_login_enabled` and `auth_providers_enabled` from `app_settings` in the caller's own write transaction and, only when password login is off, requires the global toggle to stay on and at least one path to remain once the projection (a removed link, Admin, provider, or the global toggle) is excluded. It is a projection, never mutate-then-validate, and it performs no network I/O. It uses the structural path only and never reads validation state of providers that are not being changed, so neither time passing nor a later failed provider test can make an operating SSO-only instance spontaneously invalid; a provider edit that clears validation is projected as unavailable.
* **Callers.** Provider `PATCH` (`enabled: false` or a validation-clearing edit: issuer, client id, client secret, endpoints, protocol, token auth method) and `DELETE` (guard first, then the `RESTRICT`/`PROVIDER_HAS_LINKS` check); `AdminSettingsService::patch` for `authProvidersEnabled: true → false`; `assert_unlink_allowed` for self and Admin unlink; `AdminUserService::change_role` (demotion) and `deactivate`, each after `assert_active_admin_remains`, so `LAST_ADMIN_PROTECTED` wins. User deletion (M22-T06) must call the same guard with `Projection::removing_admin` at the transition that makes the Admin unusable.
* **Provider test.** `POST /admin/providers/{id}/test` is diagnostic and not a standing-invariant trigger: it persists the ordinary M12-T01 state in every instance mode (success: `validated_at` set, `validation_error` NULL; failure: `validated_at` NULL, `validation_error` set), with `IDENTITY_PROVIDER_UPDATED` in the same transaction.
* **Login.** `AuthService::login` checks the settings snapshot first and answers `AUTH_PASSWORD_LOGIN_DISABLED` with no user lookup, no password verification, no lockout accounting, no `login_attempts` row and no MFA state. `POST /auth/password/reset` answers the same code and leaves the token unconsumed.
* **Recovery.** `palmr admin recover <user>` re-enables the setting through `password_login::reenable_in_tx` inside its own transaction (offline, instance lock held); nothing else re-enables it without the recent-auth `PUT`. There is no HTTP break-glass route.

## Account resolution

1. **Existing link.** `(provider_id, subject)` is authoritative. A changed provider e-mail moves nothing and rewrites neither `email_at_link` nor `email_verified_at_link`, which are link-time evidence. A `suspended` link, an inactive user and a locked user are refused (`AUTH_ACCOUNT_INACTIVE`, `AUTH_LOCKED`) through the shared gate. Palmr TOTP is not applied to external login; the shared gate still restricts hybrid accounts (`must_change_password`, mandatory local 2FA enrolment).
2. **Verified-e-mail auto-link.** Only when the provider has `allowEmailLinking`, the e-mail parses and is explicitly verified, exactly one active user has that normalized e-mail, and that user has no link to the provider yet (`PROVIDER_IDENTITY_ALREADY_LINKED` otherwise). It writes `link_method = auto_verified_email` and `IDENTITY_LINK_CREATED {via: verified_email, provider_id}` in the same transaction. A unique-index conflict is re-resolved by reading the row that won; a subject is never bound twice.
3. **Auto-provision.** Only when `autoProvision` is on, the e-mail is verified and no account has it. The account is role `user`, active, `password_hash NULL`, `must_change_password 0`, quota `inherit`, with its link (`auto_provision`), `USER_CREATED` and `IDENTITY_LINK_CREATED` in one transaction. Usernames come from the e-mail local part (NFKC, lowercase, `[a-z0-9._-]`, separator runs collapsed to their first character, trimmed, at most 28 characters, `user` when shorter than 3) and are inserted directly; a unique violation on the normalized username advances to `base-2` … `base-50`, then one attempt with a 20-character base and 8 random base32 characters, then `AUTH_EXTERNAL_USERNAME_UNAVAILABLE`.
4. **Refuse.** `PROVIDER_EMAIL_UNVERIFIED` when the e-mail is not explicitly verified, `PROVIDER_AUTO_PROVISION_DISABLED` otherwise. The code never depends on whether an account exists. More than one matching account fails closed with `AUTH_EXTERNAL_AMBIGUOUS_IDENTITY`.

An e-mail is verified only when the mapped claim is JSON `true` or the string `"true"`. GitHub never proves an e-mail through `/user`: the secondary endpoint (`<userinfo endpoint>/emails`) is read and only an address with `primary` and `verified` both `true` is eligible. That rule is keyed on the `github` preset and is not applied to any other provider.

## Avatars

A usable avatar is an absolute `https` URL (for Discord, the `avatar` hash is turned into its CDN URL). The callback never fetches it and never stores it: it enqueues `avatar.fetch_external` with `{userId, identityLinkId, providerId, url}` and the dedup key `avatar.fetch_external:<link id>`, in its own short transaction after the session commits. A failure to enqueue is logged and never fails the login. The handler lands with M18-T05.


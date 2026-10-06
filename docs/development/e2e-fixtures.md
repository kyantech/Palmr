# E2E instance fixtures

Governing contract: `TEST_STRATEGY.md` §2.4.1. This page documents the one fixture that exists today.

The release-container E2E suite (`tests/e2e`) exercises Palmr only through its browser and HTTP surface. Two M10-T08 acceptance scenarios need Admin/database settings that have no supported product surface until the M11 Admin management API exists: outbound SMTP (password reset through a real SMTP sink) and the mandatory local two-factor policy (forced password change followed by 2FA enrollment). These are not operator configuration, so no environment variable exists for them and none is added for tests. `palmr-e2e-fixture` prepares exactly those two preconditions and nothing else.

## Contract

`palmr-e2e-fixture` is a second binary of `palmr-server`.

- It exists only under the `e2e-fixture` Cargo feature, which is off by default. The binary is declared with `required-features`, so `cargo build`, `cargo test` and `cargo clippy` with default features neither build nor require it.
- `pnpm build:release` and the Dockerfile build `--bin palmr` only. The fixture is not in the release build, not copied into the production image, not a `palmr` subcommand and not an HTTP route.
- It runs only while the release container is stopped, against a copy of `/data`. It acquires the same exclusive data-directory lock as the operator commands, so it refuses a directory that a server or another command owns.
- It requires an existing migrated database and loads the real `instance.key`.
- It writes through `SettingsService::write_setting` inside `SettingsService::update_group`. Setting keys are checked against the settings registry, value types are checked, secrets would be sealed with the instance key, and the settings snapshot is rebuilt by the same code the server runs. The fixture does not duplicate any validation.
- It contains no raw SQL.
- It fails when the SQLite write-ahead log cannot be fully checkpointed, so a copy handed back to the release container is always complete.
- It prints no secret. The SMTP sink is unauthenticated and stores no credential.

The interface is two typed subcommands. The setting keys each one writes are fixed in the source; nothing is caller-supplied.

| Subcommand | Effect |
| --- | --- |
| `smtp-sink --host H --port P --from-email E` | `smtp_enabled = true`, `smtp_security = none`, `smtp_no_auth = true`, host, port and sender address. |
| `two-factor-required true\|false` | `two_factor_required`. |

There is no `set-setting`, no SQL, no table or column names and no generic key/value input, and none may be added. A new precondition needs a new typed subcommand and must satisfy every condition of `TEST_STRATEGY.md` §2.4.1, including that no product surface for it exists yet.

## Lifecycle

The fixture is not a permanent substitute for the product surface. When M11 ships the Admin SMTP and security-policy surface, new E2E tests use it whenever practical.

## Applying it to the release container

`tests/e2e/support/instance.ts` (`applyInstanceFixture`):

1. stops the release container;
2. copies `/data` out with `docker compose cp`;
3. runs the fixture on the copy through `PALMR_DATA_DIR`;
4. empties the volume and copies the prepared directory back through the `data-seed` helper service;
5. restores ownership to `10001:10001`;
6. starts the same release image again.

The release container and the fixture never own the same database at the same time. `instance.key` and storage state travel with the copy.

## Building and host requirements

`tests/e2e/run.sh` compiles the fixture on the host with `cargo build --locked --package palmr-server --features e2e-fixture --bin palmr-e2e-fixture` and exports `PALMR_E2E_FIXTURE_BIN`. The machine that runs the E2E suite therefore needs the Rust toolchain from `rust-toolchain.toml`, in addition to Node.js 24, Docker and Docker Compose. CI jobs that run `test:container` install it. Rust is never added to the release image.

## Topology

`tests/e2e/compose.yml` adds two E2E-only services next to `palmr`: `smtp-sink` (Mailpit, pinned by `PALMR_E2E_SINK_IMAGE`, plain SMTP on `smtp-sink:1025`, HTTP API published on `127.0.0.1:${PALMR_E2E_SINK_PORT:-8025}`) and `data-seed` (busybox, used only to replace and chown the data volume). Neither belongs to any production topology. `run.sh` uses a unique compose project per run and `down --volumes` on exit, so every run starts with an empty sink and a fresh data volume; the reset test also clears the sink before it starts.

## Black-box rule and token delivery

The fixture prepares preconditions and is never an assertion oracle. After the container starts, Playwright uses the browser, the real HTTP API and the SMTP sink only. It never reads capability material from the instance.

The reset test drains the durable outbox with the shipped `palmr jobs run-once --kind email.send` (server stopped for exclusive access), which sends through the real `SmtpTransport`. The test reads the message from the sink and follows the delivered link. Reset and invite tokens are never taken from SQLite, the outbox, fixture output or logs.

## Release artifact purity

`tests/container/smoke.sh` asserts that the image contains no `e2e-fixture` file, that `palmr --help` lists no fixture command and that `/openapi.json` mentions no fixture. `tests/snapshots/public_routes.txt` is the route snapshot and contains no fixture route.

## Mock identity provider

M12-T07 browser tests need a real OIDC round trip without a real provider. `tests/e2e/support/mock-idp/server.mjs` is a dependency-free Node server (`PALMR_E2E_IDP_IMAGE`, default `node:24-bookworm-slim`; `run.sh` copies the script into the created container with `compose cp`, like the fixture data, so no bind mount of the checkout is required) that serves discovery, JWKS, `/authorize`, `/token` and `/userinfo`. It signs RS256 ID tokens, checks S256 PKCE and the client secret, and approves every authorization request immediately for the identity it was last told about. A small control surface (`/__control/identity`, `/deny-next`, `/authorizations`, `/reset`) lets the test choose the identity, force one denial and read back what the authorization requests carried (`prompt`, `max_age`, PKCE method, nonce).

The `mock-idp` compose service shares the `palmr` network namespace (`network_mode: service:palmr`) and port 9100 is published next to 5487. The issuer is therefore `http://127.0.0.1:9100` for the browser and for Palmr's own outbound HTTP client alike, which satisfies the loopback-only plain-HTTP rule for provider URLs without any production bypass. `withPalmrStopped` restarts the service after Palmr restarts because its namespace is tied to the Palmr container.

Providers, the lowered recent-auth window and the second provider are created through the supported admin HTTP API with a logged-in admin session; no database access and no new fixture subcommand is involved. The mock never ships in the release image.


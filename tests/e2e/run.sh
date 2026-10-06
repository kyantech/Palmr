#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$ROOT/../.." && pwd)
export COMPOSE_PROJECT_NAME=${COMPOSE_PROJECT_NAME:-palmr-e2e-$$}
export PALMR_E2E_SINK_IMAGE=${PALMR_E2E_SINK_IMAGE:-axllent/mailpit:v1.29.1}
export PALMR_E2E_SEED_IMAGE=${PALMR_E2E_SEED_IMAGE:-busybox:1.37.0-musl}
export PALMR_E2E_IDP_IMAGE=${PALMR_E2E_IDP_IMAGE:-node:24-bookworm-slim}
export PALMR_E2E_SINK_URL=${PALMR_E2E_SINK_URL:-http://127.0.0.1:${PALMR_E2E_SINK_PORT:-8025}}

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose --file "$ROOT/compose.yml")
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose --file "$ROOT/compose.yml")
else
  printf 'E2E requires Docker Compose\n' >&2
  exit 1
fi

for image in "$PALMR_E2E_SINK_IMAGE" "$PALMR_E2E_SEED_IMAGE" "$PALMR_E2E_IDP_IMAGE"; do
  docker image inspect "$image" >/dev/null 2>&1 || docker pull "$image" >/dev/null
done

cargo build --locked --manifest-path "$REPO/Cargo.toml" --package palmr-server --features e2e-fixture --bin palmr-e2e-fixture
export PALMR_E2E_FIXTURE_BIN=$REPO/target/debug/palmr-e2e-fixture

cleanup() {
  "${COMPOSE[@]}" down --volumes --remove-orphans
}

trap cleanup EXIT
"${COMPOSE[@]}" create --no-build --pull never
"${COMPOSE[@]}" cp "$ROOT/support/mock-idp/server.mjs" mock-idp:/tmp/mock-idp-server.mjs
"${COMPOSE[@]}" up --detach --no-build --pull never

for _ in {1..100}; do
  if curl --fail --silent --show-error "${PALMR_E2E_BASE_URL:-http://127.0.0.1:5487}/health/live" >/dev/null 2>&1; then
    pnpm exec playwright test "$@"
    exit 0
  fi
  if [[ $("${COMPOSE[@]}" ps --status running --quiet | wc -l | tr -d ' ') == 0 ]]; then
    "${COMPOSE[@]}" logs
    exit 1
  fi
  sleep 0.1
done

"${COMPOSE[@]}" logs
exit 1

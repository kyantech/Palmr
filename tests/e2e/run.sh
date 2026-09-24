#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose --file "$ROOT/compose.yml")
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose --file "$ROOT/compose.yml")
else
  printf 'E2E requires Docker Compose\n' >&2
  exit 1
fi

cleanup() {
  "${COMPOSE[@]}" down --volumes --remove-orphans
}

trap cleanup EXIT
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

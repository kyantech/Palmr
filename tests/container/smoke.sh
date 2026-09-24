#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IMAGE=${PALMR_CONTAINER_IMAGE:-palmr-container-smoke:local}
INSPECTOR_IMAGE=${PALMR_CONTAINER_INSPECTOR_IMAGE:-busybox:1.37.0-musl}
REQUIRED=${PALMR_CONTAINER_REQUIRED:-0}
containers=()
volumes=()
temp_dirs=()

cleanup() {
  if ((${#containers[@]})); then
    docker rm -f "${containers[@]}" >/dev/null 2>&1 || true
  fi
  if ((${#volumes[@]})); then
    docker volume rm -f "${volumes[@]}" >/dev/null 2>&1 || true
  fi
  if ((${#temp_dirs[@]})); then
    rm -rf "${temp_dirs[@]}"
  fi
}

trap cleanup EXIT

skip_or_fail() {
  if [[ $REQUIRED == 1 ]]; then
    printf 'container smoke requires Docker: %s\n' "$1" >&2
    exit 1
  fi
  printf 'container smoke skipped: %s\n' "$1"
  exit 0
}

if ! command -v docker >/dev/null 2>&1; then
  skip_or_fail 'docker command not found'
fi
if ! docker info >/dev/null 2>&1; then
  skip_or_fail 'docker daemon unavailable'
fi

docker build --tag "$IMAGE" "$ROOT"
docker pull "$INSPECTOR_IMAGE" >/dev/null

new_volume() {
  NEW_VOLUME="palmr-smoke-$RANDOM-$RANDOM"
  docker volume create "$NEW_VOLUME" >/dev/null
  volumes+=("$NEW_VOLUME")
}

start_container() {
  local volume=$1
  shift
  STARTED_CONTAINER=$(docker run --detach --publish 127.0.0.1::5487 --mount "type=volume,src=$volume,dst=/data" "$@" "$IMAGE")
  containers+=("$STARTED_CONTAINER")
}

host_port() {
  docker port "$1" 5487/tcp | sed -n '1s/.*://p'
}

wait_for_live() {
  local container=$1
  local port
  port=$(host_port "$container")
  for _ in {1..100}; do
    if curl --fail --silent --show-error "http://127.0.0.1:$port/health/live" >/dev/null 2>&1; then
      printf '%s' "$port"
      return 0
    fi
    if [[ $(docker inspect --format '{{.State.Running}}' "$container") != true ]]; then
      docker logs "$container" >&2
      return 1
    fi
    sleep 0.1
  done
  docker logs "$container" >&2
  return 1
}

assert_log_field() {
  local logs=$1
  local field=$2
  local value=$3
  grep -Fq "\"$field\":\"$value\"" <<<"$logs"
}

container_runs_as_10001() {
  local volume container port rows
  new_volume
  volume=$NEW_VOLUME
  start_container "$volume"
  container=$STARTED_CONTAINER
  port=$(wait_for_live "$container")
  rows=$(docker top "$container" -eo pid,uid,gid,comm | tail -n +2)
  [[ $(sed '/^[[:space:]]*$/d' <<<"$rows" | wc -l | tr -d ' ') == 1 ]]
  grep -Eq '^[[:space:]]*[0-9]+[[:space:]]+10001[[:space:]]+10001[[:space:]]+palmr[[:space:]]*$' <<<"$rows"
  [[ $(docker inspect --format '{{.Config.User}}' "$container") == 10001:10001 ]]
  [[ $(docker inspect --format '{{.Path}}' "$container") == /usr/local/bin/palmr ]]
  [[ -n $port ]]
}

container_single_process_single_port() {
  local volume container port processes sockets files root_headers root_body openapi docs api
  new_volume
  volume=$NEW_VOLUME
  start_container "$volume"
  container=$STARTED_CONTAINER
  port=$(wait_for_live "$container")
  processes=$(docker top "$container" -eo pid,comm | tail -n +2)
  [[ $(sed '/^[[:space:]]*$/d' <<<"$processes" | wc -l | tr -d ' ') == 1 ]]
  grep -Eq '^[[:space:]]*[0-9]+[[:space:]]+palmr[[:space:]]*$' <<<"$processes"
  docker run --rm --pid "container:$container" "$INSPECTOR_IMAGE" sh -c 'test "$(tr -d "\000" </proc/1/cmdline)" = /usr/local/bin/palmr' >/dev/null
  sockets=$(docker run --rm --network "container:$container" "$INSPECTOR_IMAGE" netstat -lnt)
  [[ $(grep -c LISTEN <<<"$sockets") == 1 ]]
  grep -Eq '(^|[.:])5487[[:space:]].*LISTEN' <<<"$sockets"
  [[ $(docker inspect --format '{{json .Config.ExposedPorts}}' "$container") == '{"5487/tcp":{}}' ]]
  files=$(docker export "$container" | tar -tf -)
  grep -Eq '^usr/local/bin/palmr$' <<<"$files"
  grep -Eq '^etc/ssl/certs/ca-certificates\.crt$' <<<"$files"
  if grep -Eiq '(^|/)(node|npm|pnpm|minio|supervisord|tini|s6)(/|$)' <<<"$files"; then
    return 1
  fi
  root_headers=$(mktemp)
  root_body=$(mktemp)
  temp_dirs+=("$root_headers" "$root_body")
  curl --fail --silent --show-error --header 'Accept: text/html' --dump-header "$root_headers" --output "$root_body" "http://127.0.0.1:$port/"
  grep -Eiq '^content-type: text/html; charset=utf-8' "$root_headers"
  grep -Eiq '^content-security-policy: ' "$root_headers"
  grep -q '<title>Palmr</title>' "$root_body"
  grep -q '<div id="root"></div>' "$root_body"
  grep -Eq '<script[^>]+src="\./assets/[^\"]+\.js"' "$root_body"
  openapi=$(curl --fail --silent --show-error "http://127.0.0.1:$port/openapi.json")
  grep -Eq '"openapi":"3\.[01]\.' <<<"$openapi"
  docs=$(curl --fail --silent --show-error "http://127.0.0.1:$port/docs")
  grep -Eiq 'scalar|api-reference' <<<"$docs"
  api=$(curl --silent --show-error "http://127.0.0.1:$port/api/v1/not-real")
  grep -q '"code":"NOT_FOUND"' <<<"$api"
}

container_readonly_data_fails_explicitly() {
  local volume logs status
  new_volume
  volume=$NEW_VOLUME
  set +e
  logs=$(docker run --rm --mount "type=volume,src=$volume,dst=/data,readonly" "$IMAGE" 2>&1)
  status=$?
  set -e
  [[ $status == 78 ]]
  assert_log_field "$logs" startup_error STARTUP_DATA_DIR_NOT_WRITABLE
}

it_base_url_default_localhost_with_warning() {
  local volume container port logs explicit_volume explicit_container
  new_volume
  volume=$NEW_VOLUME
  start_container "$volume"
  container=$STARTED_CONTAINER
  port=$(wait_for_live "$container")
  logs=$(docker logs "$container" 2>&1)
  assert_log_field "$logs" startup_warning STARTUP_BASE_URL_DEFAULTED
  assert_log_field "$logs" base_url http://localhost:5487/
  curl --fail --silent --show-error "http://127.0.0.1:$port/health/live" >/dev/null
  new_volume
  explicit_volume=$NEW_VOLUME
  start_container "$explicit_volume" --env PALMR_BASE_URL=http://127.0.0.1:5487
  explicit_container=$STARTED_CONTAINER
  wait_for_live "$explicit_container" >/dev/null
  logs=$(docker logs "$explicit_container" 2>&1)
  if grep -q '"startup_warning":"STARTUP_BASE_URL_DEFAULTED"' <<<"$logs"; then
    return 1
  fi
}

run_test() {
  printf 'running %s\n' "$1"
  "$1"
  printf 'passed %s\n' "$1"
}

run_test container_runs_as_10001
run_test container_single_process_single_port
run_test container_readonly_data_fails_explicitly
run_test it_base_url_default_localhost_with_warning

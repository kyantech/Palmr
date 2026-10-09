#!/usr/bin/env bash
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO_ROOT
readonly RUST_TEST="unit_chacha8_stream_matches_manifest"
readonly NODE_TEST="node_chacha8_stream_matches_manifest"

cd "$REPO_ROOT"

report="$(mktemp)"
trap 'rm -f "$report"' EXIT
failed=0

echo "G7 generator manifest: Rust ($RUST_TEST)"
cargo nextest run --locked --profile ci --package palmr-stress --no-tests=fail \
  -E "test(=$RUST_TEST)" || failed=1

echo "G7 generator manifest: Node ($NODE_TEST)"
if pnpm --filter @palmr/engine-harness exec vitest run \
  "test/$NODE_TEST.test.ts" --reporter=json --outputFile="$report" >/dev/null; then
  jq -e --arg name "$NODE_TEST" '
    .numFailedTests == 0
    and ([.testResults[].assertionResults[] | select(.title == $name and .status == "passed")] | length) == 1
  ' "$report" >/dev/null || {
    echo "G7 failed: vitest did not report $NODE_TEST as passed" >&2
    failed=1
  }
else
  jq -r '.testResults[].assertionResults[] | select(.status == "failed") | .failureMessages[]' "$report" 2>/dev/null >&2
  failed=1
fi

if ((failed)); then
  echo "G7 generator manifest: FAILED" >&2
  exit 1
fi
echo "G7 generator manifest: ok"

#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO_ROOT
readonly SCHEMA="src/shared/api/schema.d.ts"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

cd "$REPO_ROOT"
cargo run --locked --quiet --package palmr-server --features openapi-export -- openapi \
  >"$workdir/openapi.json"

cd "$REPO_ROOT/apps/web"
pnpm exec openapi-typescript "$workdir/openapi.json" --output "$SCHEMA"

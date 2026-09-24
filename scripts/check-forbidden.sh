#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO_ROOT
readonly FIXTURE_DIR="scripts/fixtures/forbidden"
readonly IDENT_END='([^A-Za-z0-9_]|$)'
readonly SIZE_TERM='(size|content_?length|upload_?length|total_?bytes)'
readonly COMMENT_LINE='^[[:space:]]*(//|/\*|\*([[:space:]]|/|$)|--)'

RULE_IDS=()
RULE_SCOPES=()
RULE_CASES=()
RULE_PATTERNS=()
RULE_UNLESS=()
RULE_MESSAGES=()
RULE_REFS=()

rule() {
  RULE_IDS+=("$1")
  RULE_SCOPES+=("$2")
  RULE_CASES+=("$3")
  RULE_PATTERNS+=("$4")
  RULE_UNLESS+=("$5")
  RULE_MESSAGES+=("$6")
  RULE_REFS+=("$7")
}

rule body-to-bytes rust sensitive \
  "body::to_bytes$IDENT_END" "" \
  "body::to_bytes collects a whole HTTP body in memory; stream it through a bounded buffer" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §7"

rule bytestream-collect rust sensitive \
  "ByteStream::collect$IDENT_END|\.body\.collect[[:space:]]*\(" "" \
  "ByteStream::collect() reads a whole object into memory; relay it through the bounded 256 KiB stream" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §4, §7"

rule whole-file-read rust sensitive \
  "(^|[^A-Za-z0-9_])fs::read[[:space:]]*[(;]" "" \
  "std::fs::read / tokio::fs::read loads a whole file into memory; stream it with a bounded buffer" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §7"

rule whole-object-accumulation rust sensitive \
  "\.read_to_end[[:space:]]*\(|\.read_to_string[[:space:]]*\(|fs::read_to_string[[:space:]]*\(|\.collect\(\)[[:space:]]*\.await[^;]*\.to_bytes[[:space:]]*\(" \
  "\.take[[:space:]]*\(|Limited" \
  "accumulates a whole object into a Vec<u8>/String/Bytes; use a fixed-capacity buffer or bound the read with .take()/Limited" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §2, §7"

rule wall-clock rust sensitive \
  "SystemTime::now$IDENT_END|OffsetDateTime::now_(utc|local)$IDENT_END|UtcDateTime::now$IDENT_END|UNIX_EPOCH\.elapsed[[:space:]]*\(" "" \
  "reads the wall clock directly; obtain time from the injected Clock (only SystemClock may read the OS clock)" \
  "TEST_STRATEGY §10.4; IMPLEMENTATION_PLAN M02-T03"

rule spa-jszip web sensitive \
  "(from|import|require)[[:space:]]*\(?[[:space:]]*[\"']jszip[\"']|new[[:space:]]+JSZip$IDENT_END|JSZip\.|\.generateAsync[[:space:]]*\(" "" \
  "JSZip / generateAsync builds a whole archive in the tab; use the server's streaming ZIP64 download" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §7"

rule spa-jszip manifest insensitive \
  "\"jszip\"[[:space:]]*:" "" \
  "JSZip dependency: archives are built by the server's streaming ZIP64 download, never in the tab" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §7"

rule spa-response-blob web sensitive \
  "\.blob[[:space:]]*\([[:space:]]*\)" "" \
  "response.blob() buffers a whole media/archive response in the tab; let the browser stream or download it" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §7"

rule forced-gc "web manifest" sensitive \
  "(global|globalThis|window|self)\.gc$IDENT_END|performance\.memory$IDENT_END|usedJSHeapSize|jsHeapSizeLimit|--expose-gc" "" \
  "forced GC or heap-threshold throttling compensates for an unbounded design; bound the path instead" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §7"

rule timeout-by-file-size "rust web" insensitive \
  "(timeout|deadline)[A-Za-z0-9_]*(for|by|from)_?[A-Za-z0-9_]*size|(timeout|deadline)[A-Za-z0-9_]*[[:space:]]*[=:][^=:;,][^;,]*$SIZE_TERM|Duration::from_[a-z0-9_]+[[:space:]]*\([^)]*$SIZE_TERM|AbortSignal\.timeout[[:space:]]*\([^)]*$SIZE_TERM" "" \
  "timeout or deadline derived from file size; timeouts are per idle frame, never per transfer" \
  "TRANSFER_ENGINE §1.7; ADR 0008 §5, §7"

rule collate-nocase "sql rust" insensitive \
  "COLLATE[[:space:]]+NOCASE" "" \
  "COLLATE NOCASE is ASCII-only; compare application-written canonical lowercase/normalized columns" \
  "ADR 0001 §7; DATABASE_SCHEMA §6.3"

rule byte-owning-cascade sql-statement insensitive "" "" \
  "ON DELETE CASCADE in a table that owns bytes (storage_object_id); use RESTRICT and the tombstone lifecycle" \
  "ADR 0018; DATABASE_SCHEMA §5.14, §7"

ALLOWLIST=(
  "wall-clock apps/server/src/domain/clock/system.rs SystemClock is the only production reader of the OS wall clock"
  "spa-response-blob apps/web/eslint.config.js the M01-T04 ESLint rule message that names the banned response.blob() call"
  "spa-jszip apps/web/src/test/lint-fixtures/transfer-engine/ImportsJsZip.ts M01-T04 ESLint fixture proving the transfer-engine JSZip import ban"
  "spa-response-blob apps/web/src/test/lint-fixtures/transfer-engine/ReadsWholeBody.ts M01-T04 ESLint fixture proving the transfer-engine response.blob() ban"
  "collate-nocase apps/server/tests/it_schema_rules.rs M05-T01 scratch-schema fixtures proving the SQLite-aware NOCASE gate rejects each spelling"
  "timeout-by-file-size apps/server/vendor/scalar-api-reference-1.71.0/standalone.js vendored, digest-pinned Scalar API reference runtime; its minified editor code is not a Palmr transfer path"
)

VIOLATIONS=()
RUST_FILES=()
WEB_FILES=()
SQL_FILES=()
MANIFEST_FILES=()
WORK_DIR=""

cleanup() {
  if [[ -n "$WORK_DIR" ]]; then
    rm -rf "$WORK_DIR"
  fi
}

die() {
  echo "check-forbidden: $*" >&2
  exit 2
}

usage() {
  cat <<'EOF'
Usage: scripts/check-forbidden.sh [--self-test]

  (no argument)  G2 gate: scan the repository for forbidden constructions
  --self-test    verify the rules against scripts/fixtures/forbidden/
EOF
}

byte_owning_cascade_awk() {
  cat <<'AWK'
function flush(   i, lower) {
  lower = tolower(statement)
  if (lower ~ /storage_object_id/ || lower ~ /references[[:space:]]+"?storage_objects"?[[:space:]]*\(/) {
    reported = 0
    for (i = 1; i <= count; i++) {
      if (tolower(code[i]) ~ /on[[:space:]]+delete[[:space:]]+cascade/) {
        print FILENAME "\t" lines[i] "\t" text[i]
        reported = 1
      }
    }
    if (!reported && lower ~ /on[[:space:]]+delete[[:space:]]+cascade/) {
      print FILENAME "\t" lines[1] "\t" text[1]
    }
  }
  statement = ""
  count = 0
}
{
  stripped = $0
  sub(/--.*/, "", stripped)
  if (count == 0 && stripped ~ /^[[:space:]]*$/) next
  count++
  lines[count] = FNR
  text[count] = $0
  code[count] = stripped
  statement = statement " " stripped
  if (index(stripped, ";") > 0) flush()
}
END { if (count > 0) flush() }
AWK
}

unique_rule_ids() {
  printf '%s\n' "${RULE_IDS[@]}" | sort -u
}

rule_exists() {
  local id
  for id in "${RULE_IDS[@]}"; do
    [[ "$id" == "$1" ]] && return 0
  done
  return 1
}

classify_files() {
  RUST_FILES=()
  WEB_FILES=()
  SQL_FILES=()
  MANIFEST_FILES=()
  local path
  for path in "$@"; do
    case "$path" in
      *.rs) RUST_FILES+=("$path") ;;
      *.ts | *.tsx | *.mts | *.cts | *.js | *.jsx | *.mjs | *.cjs) WEB_FILES+=("$path") ;;
      *.sql) SQL_FILES+=("$path") ;;
      package.json | */package.json) MANIFEST_FILES+=("$path") ;;
    esac
  done
}

files_in_scope() {
  case "$1" in
    rust) ((${#RUST_FILES[@]})) && printf '%s\0' "${RUST_FILES[@]}" ;;
    web) ((${#WEB_FILES[@]})) && printf '%s\0' "${WEB_FILES[@]}" ;;
    sql | sql-statement) ((${#SQL_FILES[@]})) && printf '%s\0' "${SQL_FILES[@]}" ;;
    manifest) ((${#MANIFEST_FILES[@]})) && printf '%s\0' "${MANIFEST_FILES[@]}" ;;
    *) die "unknown scope '$1'" ;;
  esac
  return 0
}

is_allowlisted() {
  local rule_id="$1" path="$2" entry
  ((${#ALLOWLIST[@]})) || return 1
  for entry in "${ALLOWLIST[@]}"; do
    [[ "${entry%% *}" == "$rule_id" ]] || continue
    local rest="${entry#* }"
    [[ "${rest%% *}" == "$path" ]] && return 0
  done
  return 1
}

record() {
  local index="$1" path="$2" line="$3" text="$4"
  if [[ "$APPLY_ALLOWLIST" == yes ]] && is_allowlisted "${RULE_IDS[index]}" "$path"; then
    return
  fi
  VIOLATIONS+=("$index"$'\t'"$path"$'\t'"$line"$'\t'"$text")
}

grep_rule() {
  local index="$1" scope="$2"
  local -a files=()
  local path
  while IFS= read -r -d '' path; do
    files+=("$path")
  done < <(files_in_scope "$scope")
  ((${#files[@]})) || return 0

  local -a flags=(-HnE --null)
  [[ "${RULE_CASES[index]}" == insensitive ]] && flags+=(-i)
  local matches="$WORK_DIR/matches" status=0
  grep "${flags[@]}" -e "${RULE_PATTERNS[index]}" -- "${files[@]}" >"$matches" || status=$?
  ((status <= 1)) || die "grep failed for rule ${RULE_IDS[index]} (exit $status)"

  local unless="${RULE_UNLESS[index]}" rest line text
  while IFS= read -r -d '' path && IFS= read -r rest; do
    line="${rest%%:*}"
    text="${rest#*:}"
    [[ "$text" =~ $COMMENT_LINE ]] && continue
    [[ -n "$unless" && "$text" =~ $unless ]] && continue
    record "$index" "$path" "$line" "$text"
  done <"$matches"
}

statement_rule() {
  local index="$1" path line text
  local -a files=()
  while IFS= read -r -d '' path; do
    files+=("$path")
  done < <(files_in_scope sql-statement)
  ((${#files[@]})) || return 0

  for path in "${files[@]}"; do
    while IFS=$'\t' read -r _ line text; do
      record "$index" "$path" "$line" "$text"
    done < <(awk "$(byte_owning_cascade_awk)" "$path")
  done
}

scan() {
  VIOLATIONS=()
  classify_files "$@"
  local index scope
  for index in "${!RULE_IDS[@]}"; do
    for scope in ${RULE_SCOPES[index]}; do
      if [[ "$scope" == sql-statement ]]; then
        statement_rule "$index"
      else
        grep_rule "$index" "$scope"
      fi
    done
  done
}

report_violations() {
  local entry index path line text
  ((${#VIOLATIONS[@]})) || return 0
  for entry in "${VIOLATIONS[@]}"; do
    IFS=$'\t' read -r index path line text <<<"$entry"
    echo "G2 [${RULE_IDS[index]}] $path:$line"
    echo "    ${RULE_MESSAGES[index]} (${RULE_REFS[index]})"
    echo "    > ${text#"${text%%[![:space:]]*}"}"
    if [[ "${GITHUB_ACTIONS:-}" == true ]]; then
      echo "::error file=$path,line=$line,title=G2 ${RULE_IDS[index]}::${RULE_MESSAGES[index]} (${RULE_REFS[index]})"
    fi
  done
}

validate_allowlist() {
  local entry rule_id rest path reason errors=0
  ((${#ALLOWLIST[@]})) || return 0
  for entry in "${ALLOWLIST[@]}"; do
    rule_id="${entry%% *}"
    rest="${entry#* }"
    path="${rest%% *}"
    reason="${rest#* }"
    if ! rule_exists "$rule_id"; then
      echo "check-forbidden: allow-list names unknown rule '$rule_id'" >&2
      errors=$((errors + 1))
    fi
    if [[ ! -f "$path" || "$path" == *[\*\?\[]* ]]; then
      echo "check-forbidden: allow-list path '$path' is not an existing file" >&2
      errors=$((errors + 1))
    fi
    if [[ "$reason" == "$rest" || -z "$reason" ]]; then
      echo "check-forbidden: allow-list entry for '$path' has no reason" >&2
      errors=$((errors + 1))
    fi
  done
  ((errors == 0)) || exit 2
}

check_repository() {
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "must run inside the git work tree"
  validate_allowlist

  local -a files=()
  local path
  while IFS= read -r -d '' path; do
    [[ "$path" == "$FIXTURE_DIR/"* ]] && continue
    [[ -f "$path" ]] || continue
    files+=("$path")
  done < <(git ls-files -z --cached --others --exclude-standard)

  APPLY_ALLOWLIST=yes
  scan "${files[@]}"

  if ((${#VIOLATIONS[@]})); then
    report_violations
    echo "check-forbidden: G2 failed with ${#VIOLATIONS[@]} violation(s)" >&2
    exit 1
  fi
  echo "check-forbidden: G2 ok (${#files[@]} files, $(unique_rule_ids | wc -l | tr -d ' ') rules)"
}

reported_rule_ids() {
  local entry
  ((${#VIOLATIONS[@]})) || return 0
  for entry in "${VIOLATIONS[@]}"; do
    echo "${RULE_IDS[${entry%%$'\t'*}]}"
  done | sort -u
}

self_test() {
  local violations_dir="$FIXTURE_DIR/violations" allowed_dir="$FIXTURE_DIR/allowed"
  [[ -d "$violations_dir" && -d "$allowed_dir" ]] || die "missing fixtures under $FIXTURE_DIR"

  APPLY_ALLOWLIST=no
  local failures=0 checked=0 covered=" " dir expected file reported

  for dir in "$violations_dir"/*/; do
    expected="$(basename "$dir")"
    if ! rule_exists "$expected"; then
      echo "FAIL $dir: directory does not name a rule"
      failures=$((failures + 1))
      continue
    fi
    for file in "$dir"*; do
      [[ -f "$file" ]] || continue
      checked=$((checked + 1))
      scan "$file"
      reported="$(reported_rule_ids | tr '\n' ' ')"
      if [[ "$reported" == "$expected " ]]; then
        echo "ok   $file -> $expected"
        covered="$covered$expected "
      else
        echo "FAIL $file: expected [$expected], reported [${reported% }]"
        failures=$((failures + 1))
      fi
    done
  done

  for file in "$allowed_dir"/*; do
    [[ -f "$file" ]] || continue
    checked=$((checked + 1))
    scan "$file"
    if ((${#VIOLATIONS[@]})); then
      echo "FAIL $file: legal code reported"
      report_violations
      failures=$((failures + 1))
    else
      echo "ok   $file -> clean"
    fi
  done

  local id
  for id in $(unique_rule_ids); do
    if [[ "$covered" != *" $id "* ]]; then
      echo "FAIL rule $id has no detecting fixture under $violations_dir/$id/"
      failures=$((failures + 1))
    fi
  done

  if ((failures)); then
    echo "check-forbidden: self-test failed ($failures failure(s), $checked fixture(s))" >&2
    exit 1
  fi
  echo "check-forbidden: self-test ok ($checked fixtures)"
}

main() {
  cd "$REPO_ROOT"
  WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/check-forbidden.XXXXXX")"
  trap cleanup EXIT
  case "${1:-}" in
    "") check_repository ;;
    --self-test) self_test ;;
    -h | --help) usage ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
}

main "$@"

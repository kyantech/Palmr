# G2 — forbidden constructions

`scripts/check-forbidden.sh` is the CI grep gate of TEST_STRATEGY §9.1 G2. It rejects the forbidden constructions of TRANSFER_ENGINE §1.7 and ADR 0008 §7, plus the TEST_STRATEGY §10.4 clock rule. It complements the clippy `disallowed-methods` list in `clippy.toml` (see `quality-gates.md`). The two overlap on purpose: clippy resolves paths, and the grep also covers TypeScript, `package.json` and SQL.

Run from `v4/`:

```sh
./scripts/check-forbidden.sh --self-test
./scripts/check-forbidden.sh
```

Exit codes: `0` clean, `1` violations (every one is listed with rule, `file:line`, reason and governing reference), `2` usage or allow-list error. The PR workflow runs both commands in the `G2 / forbidden constructions` job.

## Rules

| Rule | Files | Rejects |
|---|---|---|
| `body-to-bytes` | `*.rs` | `body::to_bytes` (axum/hyper) |
| `bytestream-collect` | `*.rs` | `ByteStream::collect`, `.body.collect()` |
| `whole-file-read` | `*.rs` | `std::fs::read`, `tokio::fs::read` |
| `whole-object-accumulation` | `*.rs` | `.read_to_end(`, `.read_to_string(`, `fs::read_to_string(`, `.collect().await…to_bytes()`, except when the same line bounds the read with `.take(` or `Limited` |
| `wall-clock` | `*.rs` | `SystemTime::now`, `OffsetDateTime::now_utc`/`now_local`, `UtcDateTime::now`, `UNIX_EPOCH.elapsed()` |
| `spa-jszip` | JS/TS, `package.json` | `jszip` imports, `new JSZip`, `JSZip.`, `.generateAsync(`, a `jszip` dependency |
| `spa-response-blob` | JS/TS | `.blob()` |
| `forced-gc` | JS/TS, `package.json` | `global.gc`/`globalThis.gc`/`window.gc`/`self.gc`, `performance.memory`, `usedJSHeapSize`, `jsHeapSizeLimit`, `--expose-gc` |
| `timeout-by-file-size` | `*.rs`, JS/TS | `timeout…For…Size`-style helpers; a `timeout*`/`deadline*` assigned from a size term; `Duration::from_*(…size…)`; `AbortSignal.timeout(…size…)` |
| `collate-nocase` | `*.sql`, `*.rs` | `COLLATE NOCASE` (ADR 0001 §7, DATABASE_SCHEMA §6.3) |
| `byte-owning-cascade` | `*.sql` | `ON DELETE CASCADE` inside a statement that has a `*storage_object_id` column or references `storage_objects` (ADR 0018) |

Size terms are `size`, `content_length`, `upload_length` and `total_bytes`, matched case-insensitively. Using file size for progress, quota, part planning, validation or display is legal. Only a timeout or deadline computed from it is rejected.

The patterns are deliberately narrow markers. They are not a Rust or TypeScript analyzer. A `Vec<u8>` or `BytesMut` holding bounded protocol data is legal. Unbounded accumulation that no marker catches is still review-blocking under ADR 0008. It is also caught by the bounded-RSS suites.

`byte-owning-cascade` works on whole SQL statements. The metadata cascades approved in DATABASE_SCHEMA (`share_items`, `embed_grants`, `s3_multipart_parts`, per-user rows …) pass. Every `CREATE TABLE` in DATABASE_SCHEMA.md was checked and none is reported. M05-T01 may extend this check.

## What is scanned

All files that `git ls-files --cached --others --exclude-standard` lists: tracked files plus untracked ones that are not ignored. Build outputs and `node_modules/` are skipped because `.gitignore` ignores them. Lines that start with a comment marker (`//`, `/*`, `*`, `--`) are skipped, so a comment that names a forbidden call is not a violation.

The only excluded directory is `scripts/fixtures/forbidden/`, the gate's own fixture corpus. It is checked by `--self-test` instead.

## Allow-list

`ALLOWLIST` in the script holds entries of the form `<rule-id> <exact file path> <reason>`. An entry exempts one file from one rule. The gate exits `2` when an entry names an unknown rule, a path that is not an existing file (directories and globs included), or has no reason. Stale entries therefore fail too.

| Rule | File | Reason |
|---|---|---|
| `spa-response-blob` | `apps/web/eslint.config.js` | The M01-T04 ESLint message names the banned call. |
| `spa-jszip` | `apps/web/src/test/lint-fixtures/transfer-engine/ImportsJsZip.ts` | M01-T04 ESLint fixture. |
| `spa-response-blob` | `apps/web/src/test/lint-fixtures/transfer-engine/ReadsWholeBody.ts` | M01-T04 ESLint fixture. |

The only production exception the architecture allows is the `wall-clock` entry for the `SystemClock` file, which M02-T03 adds (exact file path, never a directory). Every new entry needs review.

## Self-test

- Each file under `scripts/fixtures/forbidden/violations/<rule-id>/` must report exactly `<rule-id>`.
- Each file under `allowed/` must report nothing.
- Each rule must have at least one violation fixture.

A new rule without a fixture fails the self-test.

## Negative check

Last run on 2026-09-23, with BSD grep and bash 3.2 (macOS) and with GNU grep 3.11, mawk and bash 5.2 (Ubuntu 24.04). The checks ran on a scratch copy of the repository and were never committed:

| Change | Result |
|---|---|
| `std::time::SystemTime::now()` and `Duration::from_secs(file_size)` in `apps/server/src/main.rs`, `x.blob()` in `App.tsx`, `COLLATE NOCASE` in a new migration | four violations, each naming its rule and reference; exit 1 |
| Violation fixtures copied outside the excluded directory | every forbidden line reported; exit 1 |
| `files.folder_id` in DATABASE_SCHEMA flipped from `RESTRICT` to `CASCADE` | `byte-owning-cascade` on that line |
| `wall-clock` allow-list entry for one file, same code in a sibling file | the sibling is reported; the allow-listed file is not |
| Allow-list entry with an unknown rule, a directory or a missing file | exit 2 |

# Rust quality gates

The Rust quality baseline introduced in M01-T02. It implements the Rust half of TEST_STRATEGY §9.1 G1 (lint), seeds the clippy half of G2 (forbidden constructions, ADR 0008 §7) and enforces the dependency decisions of ARCHITECTURE §3.1–§3.3 and ADR 0020.

Run from `v4/`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check
```

## Configuration

| File | Purpose |
|---|---|
| `rustfmt.toml` | Formatting (edition 2021, Unix newlines). |
| `Cargo.toml` `[workspace.lints]` | `unsafe_code = "forbid"`; `clippy::all` deny; `dbg_macro` deny; `unwrap_used` / `expect_used` warn. Each crate inherits them with `[lints] workspace = true`. |
| `clippy.toml` | `unwrap`/`expect` allowed in tests; `disallowed-methods`: `std::time::SystemTime::now`, `axum::body::to_bytes`, `tokio::fs::read`, `std::fs::read`. |
| `deny.toml` | Advisories (including yanked crates), AGPL-3.0-only-compatible license allow-list, crate bans, crates.io as the only source. |

`unwrap_used` and `expect_used` are warnings so that tests may use them. Clippy runs with `-D warnings`, so an occurrence outside test code still fails the gate.

`disallowed_methods` belongs to `clippy::all` and is therefore denied. Exceptions are allowed only in `SystemClock` (M02-T03) and in test code. Each one is a narrowly scoped `#[allow(clippy::disallowed_methods)]` with a reason. Nothing else may be exempted.

## Crate bans

**Graph-wide:** `openssl`, `openssl-sys`, `bcrypt`, `diesel`, `sea-orm`, `rusqlite`, `actix-web`, `rocket`, `rust-s3`, `redis`. These may not appear anywhere in the dependency graph, including transitively. No exception exists for them.

**Direct-only:** `chrono`, `eyre`, `axum-login`, `aide`. ARCHITECTURE §3.3 rejects these as direct choices. They are declared with cargo-deny `wrappers`: a banned crate is accepted only when every crate that depends on it directly is listed in its `wrappers`. The lists start empty.

- A workspace crate must never be added to `wrappers`, so a direct dependency always fails.
- If a permitted library brings one of these crates in transitively, review it and then add that library by name to the crate's `wrappers`.

## Negative check

Verifies that the policy rejects prohibited dependencies. Run it on a scratch copy of the workspace or a throwaway branch. Never commit it.

| Step | Change | Expected `cargo deny check bans` |
|---|---|---|
| A | `cargo add -p palmr-server bcrypt` | `error[banned]: crate 'bcrypt …' is explicitly banned`, `bans FAILED` |
| B | `cargo add -p palmr-server chrono` | `error[banned]: crate 'chrono …'`, `bans FAILED` |
| C | `cargo add -p palmr-server chrono-tz` (brings in `chrono` transitively), `wrappers` unchanged | `bans FAILED` (the transitive parent has not been reviewed) |
| D | as C, with `wrappers = ["chrono-tz"]` for `chrono` | `bans ok` |
| E | as D, plus `cargo add -p palmr-server chrono` | `bans FAILED` (a workspace crate depends on it directly) |

For the lints, add the following to a non-test module and run `cargo clippy --all-targets -- -D warnings`. It must report `unsafe`, disallowed-method, `unwrap()`, `expect()` and `dbg!` errors:

```rust
unsafe fn u() {}
fn f() {
    let _ = std::time::SystemTime::now();
    let _ = std::fs::read("x");
    let _ = "1".parse::<u8>().unwrap();
    let _ = "1".parse::<u8>().expect("x");
    dbg!(1);
}
```

Last run with cargo-deny 0.20.2 and Rust 1.97.1: every step produced the expected result.

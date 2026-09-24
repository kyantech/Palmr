pub mod support;

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use support::schema::{canonical_schema, migrated_schema, schema_diff, Schema, Table};

const FROZEN_MIGRATION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/migrations/0001_initial_schema.sql"
);
const FROZEN_FINGERPRINT: &str = include_str!("fixtures/0001_initial_schema.sha256");
const SNAPSHOT: &str = include_str!("snapshots/schema.sql");
const UPDATE_SNAPSHOT: &str = "PALMR_UPDATE_SCHEMA_SNAPSHOT";
const DOMAIN_TABLES: usize = 40;
const FTS5_TABLES: [&str; 2] = ["files_fts", "received_files_fts"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableClass {
    Domain,
    FtsVirtual,
    FtsInternal,
    SqlxInternal,
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_golden_matches() -> Result<()> {
    let schema = migrated_schema("it_schema_golden_matches").await?;
    let actual = canonical_schema(&schema);

    if std::env::var_os(UPDATE_SNAPSHOT).is_some() {
        let path = snapshot_path();
        std::fs::write(&path, &actual)
            .with_context(|| format!("write golden snapshot {}", path.display()))?;
        return Ok(());
    }

    if SNAPSHOT != actual {
        bail!(
            "the migrated schema differs from the committed golden snapshot in apps/server/tests/snapshots/schema.sql; \
             review the diff and, only if the change is intended, regenerate with {UPDATE_SNAPSHOT}=1\n{}",
            schema_diff(SNAPSHOT, &actual)
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_table_count() -> Result<()> {
    let schema = migrated_schema("it_schema_table_count").await?;
    assert_table_count(&schema);
    Ok(())
}

#[test]
fn it_schema_frozen_migration_matches_baseline() -> Result<()> {
    let digest = file_sha256(Path::new(FROZEN_MIGRATION))?;
    assert_eq!(
        digest,
        FROZEN_FINGERPRINT.trim(),
        "apps/server/migrations/0001_initial_schema.sql changed after the schema freeze; add a new forward migration instead of editing 0001"
    );
    Ok(())
}

fn assert_table_count(schema: &Schema) {
    let fts: Vec<&str> = schema
        .tables
        .iter()
        .filter(|table| is_fts5_virtual(&table.sql))
        .map(|table| table.name.as_str())
        .collect();
    assert_eq!(
        fts, FTS5_TABLES,
        "the product FTS5 virtual tables must be exactly files_fts and received_files_fts"
    );

    let mut domain = Vec::new();
    let mut fts_internal = Vec::new();
    let mut sqlx_internal = Vec::new();
    for table in &schema.tables {
        match classify(table, &fts) {
            TableClass::Domain => domain.push(table.name.as_str()),
            TableClass::FtsVirtual => {}
            TableClass::FtsInternal => fts_internal.push(table.name.as_str()),
            TableClass::SqlxInternal => sqlx_internal.push(table.name.as_str()),
        }
    }

    assert_eq!(
        domain.len(),
        DOMAIN_TABLES,
        "expected {DOMAIN_TABLES} domain tables, found {}: {domain:?}",
        domain.len()
    );
    assert!(
        domain.contains(&"idempotency_records"),
        "idempotency_records must be counted as a domain table"
    );
    assert!(
        sqlx_internal.is_empty(),
        "SQLite and sqlx internals must never be classified as domain tables: {sqlx_internal:?}"
    );
    assert_eq!(
        domain.len() + fts.len() + fts_internal.len(),
        schema.tables.len(),
        "every migrated table must classify as a domain table, an FTS5 virtual table or an FTS5 internal table"
    );
}

fn classify(table: &Table, fts: &[&str]) -> TableClass {
    if table.name.starts_with("sqlite_") || table.name == "_sqlx_migrations" {
        return TableClass::SqlxInternal;
    }
    if is_fts5_virtual(&table.sql) {
        return TableClass::FtsVirtual;
    }
    if fts
        .iter()
        .any(|name| table.name.starts_with(&format!("{name}_")))
    {
        return TableClass::FtsInternal;
    }
    TableClass::Domain
}

fn is_fts5_virtual(sql: &str) -> bool {
    let upper = sql.trim_start().to_ascii_uppercase();
    upper.starts_with("CREATE VIRTUAL TABLE") && upper.contains("USING FTS5")
}

fn snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/schema.sql")
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

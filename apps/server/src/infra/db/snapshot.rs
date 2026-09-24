use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use super::pool::{connect, DbOpenError, PoolRole, DATABASE_FILE};
use super::pragmas::connect_options;
use crate::config::SqliteSynchronous;

const INTEGRITY_OK: &str = "ok";

#[derive(Debug)]
pub struct SnapshotReader {
    path: PathBuf,
    pool: SqlitePool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    rows: Vec<String>,
}

impl IntegrityReport {
    pub fn is_ok(&self) -> bool {
        matches!(self.rows.as_slice(), [only] if only == INTEGRITY_OK)
    }

    pub fn problems(&self) -> &[String] {
        &self.rows
    }
}

impl SnapshotReader {
    pub async fn open(
        data_root: &Path,
        synchronous: SqliteSynchronous,
    ) -> Result<Self, DbOpenError> {
        let path = data_root.join(DATABASE_FILE);
        let options = connect_options(&path, synchronous)
            .create_if_missing(false)
            .read_only(true);
        let pool = connect(PoolRole::Read, 1, options)
            .await
            .map_err(|cause| DbOpenError {
                path: path.clone(),
                cause,
            })?;
        Ok(Self { path, pool })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn integrity_check(&self) -> Result<IntegrityReport, sqlx::Error> {
        let rows = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_all(&self.pool)
            .await?;
        Ok(IntegrityReport { rows })
    }

    pub async fn vacuum_into(&self, destination: &str) -> Result<(), sqlx::Error> {
        sqlx::query("VACUUM INTO ?1")
            .bind(destination)
            .execute(&self.pool)
            .await
            .map(drop)
    }

    pub async fn close(self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::IntegrityReport;

    fn report(rows: &[&str]) -> IntegrityReport {
        IntegrityReport {
            rows: rows.iter().map(|row| (*row).to_owned()).collect(),
        }
    }

    #[test]
    fn unit_integrity_report_requires_the_single_ok_row() {
        assert!(report(&["ok"]).is_ok());
        assert!(!report(&[]).is_ok());
        assert!(!report(&["OK"]).is_ok());
        assert!(!report(&["ok", "ok"]).is_ok());
        assert!(!report(&["row 3 missing from index idx_example"]).is_ok());
    }
}

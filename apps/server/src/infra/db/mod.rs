mod error;
mod migrate;
mod pool;
mod pragmas;
mod tx;

pub use error::{DbError, DbErrorKind};
pub use migrate::{
    MigrationError, MigrationStatus, DB_SCHEMA_AHEAD_OF_BINARY, MIGRATOR,
    STARTUP_MIGRATION_CHECKSUM_MISMATCH, STARTUP_MIGRATION_FAILED,
};
pub use pool::{DbOpenError, DbPools, DbShutdown, WriterCheck, DATABASE_FILE};
pub use tx::{ReadPool, WriteTx, SLOW_WRITE_TX_THRESHOLD};

#[cfg(test)]
mod error_tests;
#[cfg(test)]
pub(crate) mod migrate_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tx_tests;

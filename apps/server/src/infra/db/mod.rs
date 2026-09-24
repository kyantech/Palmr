mod error;
mod pool;
mod pragmas;
mod tx;

pub use error::{DbError, DbErrorKind};
pub use pool::{DbOpenError, DbPools, DbShutdown, WriterCheck, DATABASE_FILE};
pub use tx::{ReadPool, WriteTx, SLOW_WRITE_TX_THRESHOLD};

#[cfg(test)]
mod error_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tx_tests;

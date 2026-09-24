use super::error::CliError;
use super::ownership::{Access, DataAccess};
use crate::app::lifecycle::StartupError;
use crate::config::OperatorConfig;
use crate::domain::clock::Clock;
use crate::infra::db::{DbPools, MigrationStatus, MIGRATOR};

pub async fn migrate(
    config: &OperatorConfig,
    clock: &dyn Clock,
) -> Result<MigrationStatus, CliError> {
    let access = DataAccess::claim(config, Access::Exclusive, clock)?;
    let outcome = apply(config, &access).await;
    access.release();
    outcome
}

async fn apply(config: &OperatorConfig, access: &DataAccess) -> Result<MigrationStatus, CliError> {
    let pools = DbPools::open(
        access.root(),
        config.db_read_connections,
        config.db_synchronous,
    )
    .await
    .map_err(StartupError::from)?;
    let status = pools.migrate(&MIGRATOR).await;
    let _ = pools.shutdown().await;
    Ok(status.map_err(StartupError::from)?)
}

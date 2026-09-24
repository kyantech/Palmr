use std::sync::Arc;

use super::error::CliError;
use super::ownership::{Access, DataAccess};
use crate::app::lifecycle::StartupError;
use crate::config::OperatorConfig;
use crate::domain::clock::Clock;
use crate::infra::db::DbPools;
use crate::infra::jobs::cli::{run_once, RunOnceReport};
use crate::infra::jobs::{
    Claimant, Dispatcher, Jitter, JobAudit, JobKind, Registry, RuntimeTiming,
};

pub async fn run_once_command(
    config: &OperatorConfig,
    kind: JobKind,
    clock: Arc<dyn Clock>,
) -> Result<RunOnceReport, CliError> {
    let access = DataAccess::claim(config, Access::Exclusive, clock.as_ref())?;
    let outcome = execute(config, &access, kind, clock).await;
    access.release();
    outcome
}

async fn execute(
    config: &OperatorConfig,
    access: &DataAccess,
    kind: JobKind,
    clock: Arc<dyn Clock>,
) -> Result<RunOnceReport, CliError> {
    access.existing_database()?;
    let pools = DbPools::open(
        access.root(),
        config.db_read_connections,
        config.db_synchronous,
    )
    .await
    .map_err(StartupError::from)?;
    let dispatcher = Dispatcher::new(
        pools.clone(),
        Arc::clone(&clock),
        Registry::production(),
        Jitter::os(),
        JobAudit::detached(),
        RuntimeTiming::DEFAULT.lease_renewal,
    );
    let claimant = Claimant::worker(access.instance_id(), 0);
    let report = run_once(&dispatcher, &claimant, kind).await;
    let _ = pools.shutdown().await;
    report.map_err(|source| CliError::Jobs { source })
}

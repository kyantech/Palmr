mod args;
mod db;
mod error;
mod migrate;
mod ownership;
mod serve;

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

pub use self::args::{Cli, Command, DbCommand};
use self::error::CliError;
use crate::app::lifecycle::StartupError;
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::{Clock, SystemClock};
use crate::infra::telemetry::write_startup_failure;

pub fn main() -> ExitCode {
    execute(Cli::parse().command.unwrap_or_default())
}

pub fn execute(command: Command) -> ExitCode {
    match command {
        Command::Serve => serve::serve(),
        Command::Migrate => operate(Operation::Migrate),
        Command::Db { command } => operate(Operation::Db(command)),
        #[cfg(feature = "openapi-export")]
        Command::Openapi => serve::export_openapi(),
    }
}

enum Operation {
    Migrate,
    Db(DbCommand),
}

fn operate(operation: Operation) -> ExitCode {
    let outcome = OperatorConfig::load(&EnvironmentSource::from_process())
        .map_err(|error| CliError::from(StartupError::from(error)))
        .and_then(|loaded| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(CliError::Runtime)?
                .block_on(run(operation, &loaded.config, &SystemClock))
        });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = io::stdout().lock().flush();
            let _ = write_startup_failure(&mut io::stderr().lock(), &error);
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run(
    operation: Operation,
    config: &OperatorConfig,
    clock: &dyn Clock,
) -> Result<(), CliError> {
    match operation {
        Operation::Migrate => {
            let status = migrate::migrate(config, clock).await?;
            let version = status
                .version
                .map_or_else(|| "none".to_owned(), |version| version.to_string());
            report(&format_args!(
                "database migrations current: applied {}, schema version {version}",
                status.applied
            ));
            Ok(())
        }
        Operation::Db(DbCommand::Check { allow_concurrent }) => {
            db::check(config, allow_concurrent, clock).await
        }
        Operation::Db(DbCommand::Backup {
            out,
            allow_concurrent,
        }) => {
            let path = db::backup(config, &out, allow_concurrent, clock).await?;
            report(&path.display());
            let _ = writeln!(
                io::stderr().lock(),
                "The database file alone is not a complete backup: keep instance.key and the storage root (or the S3 bucket) with it."
            );
            Ok(())
        }
    }
}

fn report(line: &dyn std::fmt::Display) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

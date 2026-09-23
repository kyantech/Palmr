#[cfg(not(unix))]
compile_error!("Palmr runs on Unix-like operating systems only");

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "route policy primitives are consumed as feature routes are registered"
    )
)]
mod app;
#[expect(
    dead_code,
    unused_imports,
    reason = "configuration fields are consumed by later startup steps"
)]
mod config;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "domain primitives are consumed by feature modules"
    )
)]
mod domain;
mod features;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        unused_imports,
        reason = "infrastructure primitives are consumed by feature modules"
    )
)]
mod infra;

use std::io;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use app::lifecycle::{self, ShutdownSignals};
use config::EnvironmentSource;
use infra::telemetry::write_startup_failure;

/// Palmr — self-hosted file sharing.
#[derive(Debug, Parser)]
#[command(name = "palmr", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Subcommand)]
enum Command {
    /// Run the Palmr server (the default when no command is given).
    #[default]
    Serve,
}

fn main() -> ExitCode {
    match Cli::parse().command.unwrap_or_default() {
        Command::Serve => serve(),
    }
}

fn serve() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return bootstrap_failed("the async runtime could not be started", &error),
    };
    runtime.block_on(async {
        let signals = match ShutdownSignals::install() {
            Ok(signals) => signals,
            Err(error) => {
                return bootstrap_failed(
                    "the shutdown signal handlers could not be installed",
                    &error,
                )
            }
        };
        lifecycle::run(&EnvironmentSource::from_process(), signals).await
    })
}

fn bootstrap_failed(context: &str, error: &io::Error) -> ExitCode {
    let _ = write_startup_failure(
        &mut io::stderr().lock(),
        &format_args!("{context}: {error}"),
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::{error::ErrorKind, CommandFactory, Parser};

    #[test]
    fn unit_cli_version_flag() {
        Cli::command().debug_assert();

        let err = Cli::try_parse_from(["palmr", "--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert_eq!(err.exit_code(), 0);
        assert_eq!(
            err.render().to_string(),
            format!("palmr {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn unit_cli_serve_is_the_default_command() {
        let bare = Cli::try_parse_from(["palmr"]).unwrap();
        assert_eq!(bare.command, None);
        assert_eq!(bare.command.unwrap_or_default(), Command::Serve);

        let explicit = Cli::try_parse_from(["palmr", "serve"]).unwrap();
        assert_eq!(explicit.command, Some(Command::Serve));
    }

    #[test]
    fn unit_cli_has_no_other_commands() {
        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|command| command.get_name().to_owned())
            .collect();
        assert_eq!(names, ["serve"]);

        let err = Cli::try_parse_from(["palmr", "admin"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }
}

#[expect(
    dead_code,
    unused_imports,
    reason = "OperatorConfig is consumed by the startup pipeline (ARCHITECTURE §8.1) wired in by later M02 tasks"
)]
mod config;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        unused_imports,
        reason = "domain primitives are consumed once the startup pipeline is wired in"
    )
)]
mod domain;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        unused_imports,
        reason = "telemetry is installed by the startup pipeline (ARCHITECTURE §8.1 step 2) wired in by M02-T09"
    )
)]
mod infra;

use clap::Parser;

/// Palmr — self-hosted file sharing.
#[derive(Debug, Parser)]
#[command(name = "palmr", version)]
struct Cli {}

fn main() {
    Cli::parse();
}

#[cfg(test)]
mod tests {
    use super::Cli;
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
}

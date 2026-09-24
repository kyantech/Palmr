use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::infra::jobs::cli::JobsCommand;

/// Palmr — self-hosted file sharing.
#[derive(Debug, Parser)]
#[command(name = "palmr", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Subcommand)]
pub enum Command {
    /// Run the Palmr server (the default when no command is given).
    #[default]
    Serve,
    /// Apply the database migrations built into this binary, then exit.
    ///
    /// Refuses to run while a Palmr server is using the data directory.
    Migrate,
    /// Check or back up the database.
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Advance durable background work.
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Write this build's OpenAPI document to standard output.
    #[cfg(feature = "openapi-export")]
    #[command(hide = true)]
    Openapi,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DbCommand {
    /// Run SQLite's integrity check on the database and print its result.
    ///
    /// Prints `ok` and exits 0 when the database is intact. The database is
    /// opened read-only and is never migrated or modified.
    Check {
        /// Run while a Palmr server is using the data directory.
        #[arg(long)]
        allow_concurrent: bool,
    },
    /// Write a consistent, compacted copy of the database into a directory.
    ///
    /// Creates `palmr-<UTC timestamp>.db` in the given directory and prints its
    /// path. An existing file is never overwritten. The database file alone is
    /// not a complete backup: keep `instance.key` and the storage root (or the
    /// S3 bucket) with it.
    Backup {
        /// Existing directory that receives the backup file.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Run while a Palmr server is using the data directory.
        #[arg(long)]
        allow_concurrent: bool,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Cli, Command, DbCommand};
    use crate::infra::jobs::cli::JobsCommand;
    use crate::infra::jobs::JobKind;
    use clap::{error::ErrorKind, CommandFactory, Parser};

    fn subcommand_names(command: &clap::Command) -> Vec<String> {
        command
            .get_subcommands()
            .map(|command| command.get_name().to_owned())
            .collect()
    }

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
        let root = Cli::command();
        let expected: &[&str] = if cfg!(feature = "openapi-export") {
            &["serve", "migrate", "db", "jobs", "openapi"]
        } else {
            &["serve", "migrate", "db", "jobs"]
        };
        assert_eq!(subcommand_names(&root), expected);

        let db = root
            .find_subcommand("db")
            .expect("db command is registered");
        assert_eq!(subcommand_names(db), ["check", "backup"]);

        let jobs = root
            .find_subcommand("jobs")
            .expect("jobs command is registered");
        assert_eq!(subcommand_names(jobs), ["run-once"]);

        for unknown in [
            &["palmr", "admin"][..],
            &["palmr", "user"],
            &["palmr", "storage"],
            &["palmr", "db", "restore"],
        ] {
            let err = Cli::try_parse_from(unknown).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidSubcommand, "{unknown:?}");
        }
        assert!(Cli::try_parse_from(["palmr", "jobs"]).is_err());
    }

    #[test]
    fn unit_cli_jobs_run_once_parses() {
        let run =
            Cli::try_parse_from(["palmr", "jobs", "run-once", "--kind", "tokens.prune"]).unwrap();
        assert_eq!(
            run.command,
            Some(Command::Jobs {
                command: JobsCommand::RunOnce {
                    kind: JobKind::TokensPrune
                }
            })
        );

        let err = Cli::try_parse_from(["palmr", "jobs", "run-once"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);

        let err = Cli::try_parse_from(["palmr", "jobs", "run-once", "--kind", "run"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ValueValidation);

        let err =
            Cli::try_parse_from(["palmr", "jobs", "run-once", "--kind", "EMAIL.SEND"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn unit_cli_db_commands_parse() {
        let check = Cli::try_parse_from(["palmr", "db", "check"]).unwrap();
        assert_eq!(
            check.command,
            Some(Command::Db {
                command: DbCommand::Check {
                    allow_concurrent: false
                }
            })
        );

        let concurrent =
            Cli::try_parse_from(["palmr", "db", "check", "--allow-concurrent"]).unwrap();
        assert_eq!(
            concurrent.command,
            Some(Command::Db {
                command: DbCommand::Check {
                    allow_concurrent: true
                }
            })
        );

        let backup =
            Cli::try_parse_from(["palmr", "db", "backup", "--out", "/srv/backup"]).unwrap();
        assert_eq!(
            backup.command,
            Some(Command::Db {
                command: DbCommand::Backup {
                    out: PathBuf::from("/srv/backup"),
                    allow_concurrent: false,
                }
            })
        );

        let err = Cli::try_parse_from(["palmr", "db", "backup"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);

        let err = Cli::try_parse_from(["palmr", "migrate", "--allow-concurrent"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[cfg(not(feature = "openapi-export"))]
    #[test]
    fn unit_cli_default_build_has_no_openapi_export() {
        let err = Cli::try_parse_from(["palmr", "openapi"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[cfg(feature = "openapi-export")]
    #[test]
    fn unit_cli_openapi_export_is_hidden() {
        let parsed = Cli::try_parse_from(["palmr", "openapi"]).unwrap();
        assert_eq!(parsed.command, Some(Command::Openapi));

        let export = Cli::command()
            .find_subcommand("openapi")
            .map(clap::Command::is_hide_set);
        assert_eq!(export, Some(true));
    }
}

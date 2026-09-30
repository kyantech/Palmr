use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};

use super::ownership::{Access, DataAccess};
use crate::app::lifecycle::StartupError;
use crate::config::{EnvironmentSource, OperatorConfig};
use crate::domain::clock::{Clock, SystemClock};
use crate::features::settings::service::{SettingValueInput, SETTINGS_UPDATE_GROUP};
use crate::features::settings::{SettingsError, SettingsService};
use crate::infra::crypto::instance_key::InstanceKey;
use crate::infra::db::DbPools;

#[derive(Debug, Parser)]
#[command(
    name = "palmr-e2e-fixture",
    version,
    about = "Prepare an isolated E2E data directory through the real settings service"
)]
struct Args {
    #[command(subcommand)]
    fixture: Fixture,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Fixture {
    /// Point outbound e-mail at an unauthenticated, unencrypted SMTP sink.
    SmtpSink {
        #[arg(long)]
        host: String,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        from_email: String,
    },
    /// Turn the mandatory local two-factor policy on or off.
    TwoFactorRequired {
        #[arg(action = clap::ArgAction::Set)]
        required: bool,
    },
}

impl Fixture {
    fn writes(&self) -> Vec<(&'static str, SettingValueInput<'_>)> {
        match self {
            Self::SmtpSink {
                host,
                port,
                from_email,
            } => vec![
                ("smtp_enabled", SettingValueInput::Boolean(true)),
                ("smtp_host", SettingValueInput::String(host)),
                ("smtp_port", SettingValueInput::Integer(i64::from(*port))),
                ("smtp_security", SettingValueInput::String("none")),
                ("smtp_no_auth", SettingValueInput::Boolean(true)),
                ("smtp_from_email", SettingValueInput::String(from_email)),
            ],
            Self::TwoFactorRequired { required } => {
                vec![("two_factor_required", SettingValueInput::Boolean(*required))]
            }
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::SmtpSink { host, port, .. } => format!("smtp sink {host}:{port}"),
            Self::TwoFactorRequired { required } => format!("two_factor_required={required}"),
        }
    }
}

pub fn main() -> ExitCode {
    let fixture = Args::parse().fixture;
    match run(&fixture) {
        Ok(()) => {
            println!("palmr-e2e-fixture: applied {}", fixture.summary());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("palmr-e2e-fixture: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(fixture: &Fixture) -> anyhow::Result<()> {
    let loaded = OperatorConfig::load(&EnvironmentSource::from_process())
        .map_err(StartupError::from)
        .context("operator configuration")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(apply(&loaded.config, Arc::new(SystemClock), fixture))
}

pub(crate) async fn apply(
    config: &OperatorConfig,
    clock: Arc<dyn Clock>,
    fixture: &Fixture,
) -> anyhow::Result<()> {
    let access = DataAccess::claim(config, Access::Exclusive, clock.as_ref())?;
    let outcome = write(config, &access, clock, fixture).await;
    access.release();
    outcome
}

async fn write(
    config: &OperatorConfig,
    access: &DataAccess,
    clock: Arc<dyn Clock>,
    fixture: &Fixture,
) -> anyhow::Result<()> {
    access.existing_database()?;
    let pools = DbPools::open(
        access.root(),
        config.db_read_connections,
        config.db_synchronous,
    )
    .await
    .map_err(StartupError::from)?;
    let outcome = commit(access, &pools, clock, fixture).await;
    let checkpoint = pools.shutdown().await.checkpoint;
    outcome?;
    let checkpoint = checkpoint.context("the write-ahead log could not be checkpointed")?;
    anyhow::ensure!(
        checkpoint.complete,
        "the write-ahead log was not fully checkpointed"
    );
    Ok(())
}

async fn commit(
    access: &DataAccess,
    pools: &DbPools,
    clock: Arc<dyn Clock>,
    fixture: &Fixture,
) -> anyhow::Result<()> {
    let (instance_key, _) = InstanceKey::load_or_create(access.root())?;
    let settings = SettingsService::load(pools, clock, &instance_key).await?;
    settings
        .update_group::<(), SettingsError, _>(SETTINGS_UPDATE_GROUP, async |tx| {
            for (key, value) in fixture.writes() {
                settings.write_setting(tx, key, value, None).await?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clap::Parser;
    use tempfile::TempDir;

    use super::{apply, Args, Fixture};
    use crate::cli::error::CliError;
    use crate::cli::migrate;
    use crate::cli::ownership::{Access, DataAccess};
    use crate::config::{EnvironmentSource, OperatorConfig};
    use crate::domain::clock::{Clock, SystemClock};
    use crate::features::settings::model::SmtpSecurity;
    use crate::features::settings::SettingsService;
    use crate::infra::crypto::instance_key::InstanceKey;
    use crate::infra::db::DbPools;

    fn config(dir: &TempDir) -> OperatorConfig {
        let source = EnvironmentSource::from_vars([("PALMR_DATA_DIR", dir.path().as_os_str())]);
        OperatorConfig::load(&source).unwrap().config
    }

    async fn snapshot(
        config: &OperatorConfig,
    ) -> Arc<crate::features::settings::model::AppSettings> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let pools = DbPools::open(&config.data_dir, 1, config.db_synchronous)
            .await
            .unwrap();
        let (key, _) = InstanceKey::load_or_create(&config.data_dir).unwrap();
        let settings = SettingsService::load(&pools, clock, &key).await.unwrap();
        let current = settings.current();
        pools.shutdown().await.checkpoint.unwrap();
        current
    }

    #[tokio::test]
    async fn unit_e2e_fixture_applies_smtp_and_policy_through_the_settings_service() {
        let dir = TempDir::new().unwrap();
        let config = config(&dir);
        migrate::migrate(&config, &SystemClock).await.unwrap();
        let before = snapshot(&config).await;
        assert!(!before.smtp.enabled);
        assert!(!before.security.two_factor_required);

        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        apply(
            &config,
            Arc::clone(&clock),
            &Fixture::SmtpSink {
                host: "smtp-sink".to_owned(),
                port: 1025,
                from_email: "palmr@example.test".to_owned(),
            },
        )
        .await
        .unwrap();
        apply(
            &config,
            Arc::clone(&clock),
            &Fixture::TwoFactorRequired { required: true },
        )
        .await
        .unwrap();

        let after = snapshot(&config).await;
        assert!(after.smtp.enabled);
        assert_eq!(after.smtp.host.as_deref(), Some("smtp-sink"));
        assert_eq!(after.smtp.port, 1025);
        assert_eq!(after.smtp.security, SmtpSecurity::None);
        assert!(after.smtp.no_auth);
        assert!(after.smtp.password.is_none());
        assert_eq!(after.smtp.from_email.as_deref(), Some("palmr@example.test"));
        assert!(after.security.two_factor_required);

        apply(
            &config,
            clock,
            &Fixture::TwoFactorRequired { required: false },
        )
        .await
        .unwrap();
        assert!(!snapshot(&config).await.security.two_factor_required);
    }

    #[tokio::test]
    async fn unit_e2e_fixture_refuses_a_data_dir_without_a_database() {
        let dir = TempDir::new().unwrap();
        let config = config(&dir);

        let error = apply(
            &config,
            Arc::new(SystemClock),
            &Fixture::TwoFactorRequired { required: true },
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("database"), "{error}");
    }

    #[tokio::test]
    async fn unit_e2e_fixture_refuses_a_data_dir_owned_by_another_process() {
        let dir = TempDir::new().unwrap();
        let config = config(&dir);
        migrate::migrate(&config, &SystemClock).await.unwrap();
        let server = DataAccess::claim(&config, Access::Exclusive, &SystemClock).unwrap();

        let error = apply(
            &config,
            Arc::new(SystemClock),
            &Fixture::TwoFactorRequired { required: true },
        )
        .await
        .unwrap_err();
        server.release();

        assert!(
            matches!(
                error.downcast_ref::<CliError>(),
                Some(CliError::DataDirInUse { .. })
            ),
            "{error:?}"
        );
        assert!(!snapshot(&config).await.security.two_factor_required);
    }

    #[test]
    fn unit_e2e_fixture_accepts_only_narrow_typed_commands() {
        let parse = |args: &[&str]| {
            Args::try_parse_from(std::iter::once("palmr-e2e-fixture").chain(args.iter().copied()))
        };

        assert!(parse(&[
            "smtp-sink",
            "--host",
            "smtp-sink",
            "--port",
            "1025",
            "--from-email",
            "palmr@example.test",
        ])
        .is_ok());
        assert!(parse(&["two-factor-required", "true"]).is_ok());

        assert!(parse(&["two-factor-required", "maybe"]).is_err());
        assert!(parse(&[
            "smtp-sink",
            "--host",
            "h",
            "--port",
            "70000",
            "--from-email",
            "e"
        ])
        .is_err());
        assert!(parse(&["smtp-sink", "--host", "h", "--from-email", "e"]).is_err());
        for generic in ["set-setting", "execute-sql", "sql", "write"] {
            assert!(parse(&[generic, "key", "value"]).is_err(), "{generic}");
        }
    }
}

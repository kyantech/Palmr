use clap::Subcommand;

use super::kinds::JobKind;
use super::runtime::Dispatcher;
use super::{Claimant, JobsError};

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum JobsCommand {
    /// Claim and run every runnable pending job of one kind, then exit.
    RunOnce {
        /// Closed v4 job kind to drain.
        #[arg(long, value_name = "KIND", value_parser = parse_kind)]
        kind: JobKind,
    },
}

fn parse_kind(text: &str) -> Result<JobKind, String> {
    text.parse::<JobKind>().map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOnceReport {
    pub kind: JobKind,
    pub executed: u64,
}

pub async fn run_once(
    dispatcher: &Dispatcher,
    claimant: &Claimant,
    kind: JobKind,
) -> Result<RunOnceReport, JobsError> {
    let mut executed = 0_u64;
    while dispatcher
        .run_next_kind(claimant, kind, || true)
        .await?
        .is_some()
    {
        executed = executed.saturating_add(1);
    }
    Ok(RunOnceReport { kind, executed })
}

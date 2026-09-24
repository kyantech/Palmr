use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::infra::db::{DbPools, InstanceId};
use crate::infra::jobs::claim::MAX_ROWS_PER_TX;
use crate::infra::jobs::JobsError;

pub const JOBS_STEP: &str = "jobs";

const REQUEUE_STALE_CLAIMS: &str = "UPDATE jobs
        SET state = 'pending', claimed_by = NULL, lease_expires_at = NULL, updated_at = ?1
      WHERE id IN (SELECT id FROM jobs
                    WHERE state = 'claimed'
                      AND (lease_expires_at <= ?1
                           OR substr(claimed_by, 1, length(?2)) <> ?2)
                    LIMIT ?3)";

type StepFuture = Pin<Box<dyn Future<Output = Result<u64, ReconcileError>> + Send>>;
type StepFn = Arc<dyn Fn(ReconcileContext) -> StepFuture + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileError {
    code: &'static str,
    detail: String,
}

impl ReconcileError {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ReconcileError {}

#[derive(Clone)]
pub struct ReconcileContext {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    instance: InstanceId,
}

impl ReconcileContext {
    pub const fn new(pools: DbPools, clock: Arc<dyn Clock>, instance: InstanceId) -> Self {
        Self {
            pools,
            clock,
            instance,
        }
    }
}

#[derive(Clone)]
struct Step {
    id: &'static str,
    run: StepFn,
}

#[derive(Clone, Default)]
pub struct ReconcileRegistry {
    steps: Vec<Step>,
}

impl ReconcileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn production() -> Self {
        Self::new().register(JOBS_STEP, jobs_step)
    }

    #[must_use]
    pub fn register(
        mut self,
        id: &'static str,
        step: impl Fn(ReconcileContext) -> StepFuture + Send + Sync + 'static,
    ) -> Self {
        assert!(
            self.steps.iter().all(|existing| existing.id != id),
            "reconcile step {id} is registered more than once"
        );
        self.steps.push(Step {
            id,
            run: Arc::new(step),
        });
        self
    }

    pub async fn run(&self, context: &ReconcileContext) -> ReconcileReport {
        let mut outcomes = Vec::with_capacity(self.steps.len());
        for step in &self.steps {
            let result = (step.run)(context.clone()).await;
            match &result {
                Ok(rows) => tracing::info!(
                    step = step.id,
                    rows,
                    "startup reconciliation step completed"
                ),
                Err(error) => tracing::warn!(
                    step = step.id,
                    error_code = error.code(),
                    "startup reconciliation step failed"
                ),
            }
            outcomes.push(StepOutcome {
                id: step.id,
                result,
            });
        }
        ReconcileReport { outcomes }
    }
}

impl fmt::Debug for ReconcileRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.steps.iter().map(|step| step.id))
            .finish()
    }
}

#[derive(Debug)]
pub struct StepOutcome {
    id: &'static str,
    result: Result<u64, ReconcileError>,
}

impl StepOutcome {
    pub const fn result(&self) -> &Result<u64, ReconcileError> {
        &self.result
    }
}

#[derive(Debug)]
pub struct ReconcileReport {
    outcomes: Vec<StepOutcome>,
}

impl ReconcileReport {
    pub fn steps(&self) -> usize {
        self.outcomes.len()
    }

    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.result.is_err())
            .count()
    }

    pub fn requeued(&self) -> u64 {
        self.outcomes
            .iter()
            .filter_map(|outcome| outcome.result.as_ref().ok().copied())
            .sum()
    }

    pub fn outcome(&self, id: &str) -> Option<&Result<u64, ReconcileError>> {
        self.outcomes
            .iter()
            .find(|outcome| outcome.id == id)
            .map(StepOutcome::result)
    }
}

fn jobs_step(context: ReconcileContext) -> StepFuture {
    Box::pin(async move {
        requeue_stale_claims(&context)
            .await
            .map_err(|error| ReconcileError::new(error.kind(), error.to_string()))
    })
}

async fn requeue_stale_claims(context: &ReconcileContext) -> Result<u64, JobsError> {
    let now = Timestamp::try_from(context.clock.now())?;
    let instance = format!("{}#", context.instance);
    let mut requeued = 0_u64;
    loop {
        let batch = context
            .pools
            .write_tx(
                context.clock.as_ref(),
                "jobs.reconcile_stale_claims",
                async |tx| {
                    let updated = sqlx::query(REQUEUE_STALE_CLAIMS)
                        .bind(now.to_string())
                        .bind(instance.as_str())
                        .bind(i64::from(MAX_ROWS_PER_TX))
                        .execute(tx.executor())
                        .await?;
                    Ok::<_, JobsError>(updated.rows_affected())
                },
            )
            .await?;
        requeued = requeued.saturating_add(batch);
        if batch < u64::from(MAX_ROWS_PER_TX) {
            return Ok(requeued);
        }
    }
}

#[cfg(test)]
mod tests;

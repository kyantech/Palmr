use std::collections::BTreeMap;
use std::time::Duration;

use super::report::Diagnosis;
use super::routine::diagnose;
use crate::infra::db::{DbError, ReadPool};
use crate::storage::error::StorageError;
use crate::storage::key::ObjectKey;
use crate::storage::provider::StorageProvider;
use crate::storage::ProviderKind;

pub const SAMPLE_LIMIT: usize = 50;
pub const MISSING_PERCENT_THRESHOLD: usize = 20;

const KEY_STRATA: usize = SAMPLE_LIMIT / 2;
const FAN_OUT: usize = 256;
const EXISTS_TIMEOUT: Duration = Duration::from_secs(15);

const BY_KEY: &str = "SELECT id, object_key, provider FROM storage_objects
                      WHERE state = 'active' AND object_key >= ?1
                      ORDER BY object_key LIMIT 1";
const BY_ID: &str = "SELECT id, object_key, provider FROM storage_objects
                     WHERE state = 'active' AND id >= ?1
                     ORDER BY id LIMIT 1";
const ID_SPAN: &str = "SELECT MIN(id), MAX(id) FROM storage_objects WHERE state = 'active'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampledRow {
    pub id: String,
    pub object_key: String,
    pub provider: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsistencyOutcome {
    Empty,
    Consistent,
    StorageReplaced,
    ProviderMismatch,
    Inconclusive(Diagnosis),
}

impl ConsistencyOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Consistent => "consistent",
            Self::StorageReplaced => "storage_replaced",
            Self::ProviderMismatch => "provider_mismatch",
            Self::Inconclusive(_) => "inconclusive",
        }
    }

    pub const fn degradation(self) -> Option<Diagnosis> {
        match self {
            Self::StorageReplaced => Some(Diagnosis::StorageReplaced),
            Self::ProviderMismatch => Some(Diagnosis::ProviderMismatch),
            Self::Empty | Self::Consistent | Self::Inconclusive(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsistencyReport {
    pub sampled: usize,
    pub present: usize,
    pub missing: usize,
    pub mismatched: usize,
    pub unprobed: usize,
    pub outcome: ConsistencyOutcome,
}

pub async fn sample(reader: &ReadPool) -> Result<Vec<SampledRow>, DbError> {
    let mut rows: BTreeMap<String, SampledRow> = BTreeMap::new();
    for stratum in 0..KEY_STRATA {
        let shard = stratum * FAN_OUT / KEY_STRATA;
        let floor = format!("objects/{shard:02x}/");
        if let Some(row) = first_row(reader, BY_KEY, &floor).await? {
            rows.entry(row.id.clone()).or_insert(row);
        }
    }
    let span: (Option<String>, Option<String>) =
        sqlx::query_as(ID_SPAN).fetch_one(reader.executor()).await?;
    if let (Some(first), Some(last)) = span {
        for floor in id_floors(&first, &last, SAMPLE_LIMIT - rows.len()) {
            if rows.len() >= SAMPLE_LIMIT {
                break;
            }
            if let Some(row) = first_row(reader, BY_ID, &floor).await? {
                rows.entry(row.id.clone()).or_insert(row);
            }
        }
    }
    Ok(rows.into_values().take(SAMPLE_LIMIT).collect())
}

pub async fn probe(provider: &dyn StorageProvider, rows: &[SampledRow]) -> ConsistencyReport {
    let configured = provider.describe().provider;
    let mut report = ConsistencyReport {
        sampled: rows.len().min(SAMPLE_LIMIT),
        present: 0,
        missing: 0,
        mismatched: 0,
        unprobed: 0,
        outcome: ConsistencyOutcome::Empty,
    };
    let mut outage = None;
    for row in rows.iter().take(SAMPLE_LIMIT) {
        if outage.is_some() {
            report.unprobed += 1;
            continue;
        }
        if provider_kind(&row.provider) != Some(configured) {
            report.mismatched += 1;
            continue;
        }
        let Ok(key) = ObjectKey::parse(&row.object_key) else {
            report.unprobed += 1;
            continue;
        };
        match tokio::time::timeout(EXISTS_TIMEOUT, provider.exists(&key)).await {
            Ok(Ok(true)) => report.present += 1,
            Ok(Ok(false) | Err(StorageError::NotFound)) => report.missing += 1,
            Ok(Err(error)) => {
                outage = Some(diagnose(&error));
                report.unprobed += 1;
            }
            Err(_) => {
                outage = Some(Diagnosis::Unreachable);
                report.unprobed += 1;
            }
        }
    }
    let meaningful = report.present + report.missing;
    report.outcome = if report.mismatched > 0 {
        ConsistencyOutcome::ProviderMismatch
    } else if let Some(diagnosis) = outage {
        ConsistencyOutcome::Inconclusive(diagnosis)
    } else if meaningful == 0 {
        ConsistencyOutcome::Empty
    } else if report.missing * 100 > meaningful * MISSING_PERCENT_THRESHOLD {
        ConsistencyOutcome::StorageReplaced
    } else {
        ConsistencyOutcome::Consistent
    };
    report
}

async fn first_row(
    reader: &ReadPool,
    query: &str,
    floor: &str,
) -> Result<Option<SampledRow>, DbError> {
    let row: Option<(String, String, String)> = sqlx::query_as(query)
        .bind(floor)
        .fetch_optional(reader.executor())
        .await?;
    Ok(row.map(|(id, object_key, provider)| SampledRow {
        id,
        object_key,
        provider,
    }))
}

fn provider_kind(text: &str) -> Option<ProviderKind> {
    match text {
        "local" => Some(ProviderKind::Local),
        "s3" => Some(ProviderKind::S3),
        _ => None,
    }
}

fn id_floors(first: &str, last: &str, strata: usize) -> Vec<String> {
    let (Some(start), Some(end)) = (id_millis(first), id_millis(last)) else {
        return Vec::new();
    };
    let span = end.saturating_sub(start);
    let strata = u64::try_from(strata).unwrap_or(1).max(1);
    (0..strata)
        .map(|stratum| start + span * stratum / strata)
        .map(|millis| {
            format!(
                "{:08x}-{:04x}-0000-0000-000000000000",
                millis >> 16,
                millis & 0xffff
            )
        })
        .collect()
}

fn id_millis(id: &str) -> Option<u64> {
    let high = id.get(..8)?;
    let low = id.get(9..13)?;
    let millis = u64::from_str_radix(&format!("{high}{low}"), 16).ok()?;
    (id.as_bytes().get(8) == Some(&b'-')).then_some(millis)
}

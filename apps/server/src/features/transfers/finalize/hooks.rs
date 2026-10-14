use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Point {
    Dispatched,
    Claimed,
    StagingSynced,
    Measured,
    MimeResolved,
    IntentCommitted,
    BeforePlace,
    AfterPlace,
    BeforeCommit,
    AfterCommit,
    BeforeCleanup,
}

#[cfg(any(test, feature = "e2e-fixture"))]
impl Point {
    pub const ALL: [Self; 11] = [
        Self::Dispatched,
        Self::Claimed,
        Self::StagingSynced,
        Self::Measured,
        Self::MimeResolved,
        Self::IntentCommitted,
        Self::BeforePlace,
        Self::AfterPlace,
        Self::BeforeCommit,
        Self::AfterCommit,
        Self::BeforeCleanup,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::Claimed => "claimed",
            Self::StagingSynced => "staging_synced",
            Self::Measured => "measured",
            Self::MimeResolved => "mime_resolved",
            Self::IntentCommitted => "intent_committed",
            Self::BeforePlace => "before_place",
            Self::AfterPlace => "after_place",
            Self::BeforeCommit => "before_commit",
            Self::AfterCommit => "after_commit",
            Self::BeforeCleanup => "before_cleanup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "only the crash-recovery tests simulate process death at a point"
        )
    )]
    Crash,
}

#[async_trait]
pub trait FinalizeHooks: Send + Sync {
    async fn reached(&self, point: Point) -> Flow;
}

pub struct NoHooks;

#[async_trait]
impl FinalizeHooks for NoHooks {
    async fn reached(&self, _: Point) -> Flow {
        Flow::Continue
    }
}

#[cfg(feature = "e2e-fixture")]
pub struct PauseHook {
    point: Point,
    marker: std::path::PathBuf,
}

#[cfg(feature = "e2e-fixture")]
impl PauseHook {
    pub fn from_environment() -> Option<Self> {
        let point = std::env::var("PALMR_E2E_FINALIZE_PAUSE_AT").ok()?;
        let marker = std::env::var_os("PALMR_E2E_FINALIZE_PAUSE_FILE")?;
        let point = Point::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == point)?;
        Some(Self {
            point,
            marker: marker.into(),
        })
    }
}

#[cfg(feature = "e2e-fixture")]
#[async_trait]
impl FinalizeHooks for PauseHook {
    async fn reached(&self, point: Point) -> Flow {
        if point == self.point {
            let marked = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.marker)
                .and_then(|mut marker| {
                    std::io::Write::write_all(
                        &mut marker,
                        format!("{}\n", point.as_str()).as_bytes(),
                    )
                });
            if marked.is_err() {
                return Flow::Continue;
            }
            std::future::pending::<()>().await;
        }
        Flow::Continue
    }
}

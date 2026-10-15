use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Point {
    ExpireDiscovered,
    BeforeProviderAbort,
    OrphanBeforeAbort,
    ScanPageRead,
}

#[cfg(feature = "e2e-fixture")]
impl Point {
    pub const ALL: [Self; 4] = [
        Self::ExpireDiscovered,
        Self::BeforeProviderAbort,
        Self::OrphanBeforeAbort,
        Self::ScanPageRead,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExpireDiscovered => "expire_discovered",
            Self::BeforeProviderAbort => "before_provider_abort",
            Self::OrphanBeforeAbort => "orphan_before_abort",
            Self::ScanPageRead => "scan_page_read",
        }
    }
}

#[async_trait]
pub trait ReaperHooks: Send + Sync {
    async fn proceeds(&self, point: Point) -> bool;
}

#[cfg(feature = "e2e-fixture")]
pub struct PauseHook {
    point: Point,
    marker: std::path::PathBuf,
}

#[cfg(feature = "e2e-fixture")]
impl PauseHook {
    pub fn from_environment() -> Option<Self> {
        let point = std::env::var("PALMR_E2E_S3_REAPER_PAUSE_AT").ok()?;
        let marker = std::env::var_os("PALMR_E2E_S3_REAPER_PAUSE_FILE")?;
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
impl ReaperHooks for PauseHook {
    async fn proceeds(&self, point: Point) -> bool {
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
                return true;
            }
            std::future::pending::<()>().await;
        }
        true
    }
}

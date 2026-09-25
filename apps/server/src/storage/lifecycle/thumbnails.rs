use std::io;
use std::path::{Path, PathBuf};

use crate::storage::key::{KeyNamespace, ObjectKey};

pub const THUMBNAILS_DIR: &str = "thumbnails";
const THUMBNAIL_EXTENSION: &str = "webp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailRemoval {
    Removed,
    Absent,
    NotApplicable,
    Failed(io::ErrorKind),
}

#[derive(Debug, Clone)]
pub struct ThumbnailCache {
    root: PathBuf,
}

impl ThumbnailCache {
    pub fn under(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(THUMBNAILS_DIR),
        }
    }

    pub fn path_for(&self, key: &ObjectKey) -> Option<PathBuf> {
        if key.namespace() != KeyNamespace::Objects {
            return None;
        }
        let mut segments = key.as_str().rsplit('/');
        let oid = segments.next()?;
        let cd = segments.next()?;
        let ab = segments.next()?;
        Some(
            self.root
                .join(ab)
                .join(cd)
                .join(format!("{oid}.{THUMBNAIL_EXTENSION}")),
        )
    }

    pub async fn remove(&self, key: &ObjectKey) -> ThumbnailRemoval {
        let Some(path) = self.path_for(key) else {
            return ThumbnailRemoval::NotApplicable;
        };
        let removed = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
        match removed {
            Ok(Ok(())) => ThumbnailRemoval::Removed,
            Ok(Err(error)) if error.kind() == io::ErrorKind::NotFound => ThumbnailRemoval::Absent,
            Ok(Err(error)) => ThumbnailRemoval::Failed(error.kind()),
            Err(_) => ThumbnailRemoval::Failed(io::ErrorKind::Other),
        }
    }
}

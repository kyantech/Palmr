use std::collections::HashMap;

use crate::domain::clock::Clock;
use crate::domain::naming::NameCandidate;
use crate::domain::time::Timestamp;
use crate::features::folders::{ensure_child_in_tx, FolderError, FolderId, MAX_FOLDER_DEPTH};
use crate::features::users::model::UserId;
use crate::infra::db::WriteTx;

use super::error::TransferError;
use super::model::PlannedFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Node<'a> {
    parent: Option<usize>,
    segment: &'a NameCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreePlan<'a> {
    nodes: Vec<Node<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Materialized {
    pub created: usize,
    pub reused: usize,
}

impl<'a> TreePlan<'a> {
    pub fn of(files: &'a [PlannedFile]) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut nodes: Vec<Node<'a>> = Vec::new();
        for file in files {
            let mut key = String::new();
            let mut parent = None;
            for segment in file.directory.segments() {
                if !key.is_empty() {
                    key.push('/');
                }
                key.push_str(segment.normalized());
                let at = *index.entry(key.clone()).or_insert_with(|| {
                    nodes.push(Node { parent, segment });
                    nodes.len() - 1
                });
                parent = Some(at);
            }
        }
        Self { nodes }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[cfg(test)]
    pub fn display_names(&self) -> Vec<&str> {
        self.nodes
            .iter()
            .map(|node| node.segment.display())
            .collect()
    }
}

pub fn exceeds_depth(base_depth: i64, files: &[PlannedFile]) -> bool {
    files.iter().any(|file| {
        let segments = i64::try_from(file.directory.len()).unwrap_or(i64::MAX);
        segments > 0 && base_depth.saturating_add(segments - 1) > MAX_FOLDER_DEPTH
    })
}

pub async fn materialize(
    tx: &mut WriteTx<'_>,
    clock: &dyn Clock,
    owner: UserId,
    target: Option<FolderId>,
    base_depth: i64,
    plan: &TreePlan<'_>,
    now: Timestamp,
) -> Result<Materialized, TransferError> {
    let mut resolved: Vec<(FolderId, i64)> = Vec::with_capacity(plan.nodes.len());
    let mut summary = Materialized::default();
    for node in &plan.nodes {
        let (parent, depth) = match node.parent {
            None => (target, base_depth),
            Some(at) => {
                let (folder, depth) = resolved
                    .get(at)
                    .copied()
                    .ok_or(TransferError::Invariant { what: "tree_plan" })?;
                (Some(folder), depth + 1)
            }
        };
        if depth > MAX_FOLDER_DEPTH {
            return Err(TransferError::Folder(FolderError::DepthExceeded));
        }
        let step = ensure_child_in_tx(tx, clock, owner, parent, depth, node.segment, now).await?;
        if step.created {
            summary.created += 1;
        } else {
            summary.reused += 1;
        }
        resolved.push((step.id, depth));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{exceeds_depth, TreePlan};
    use crate::features::transfers::model::{PlannedFile, ValidatedSession};

    fn planned(paths: &[&str]) -> Vec<PlannedFile> {
        let files: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                json!({
                    "clientId": format!("c{index}"),
                    "name": path.rsplit(['/', '\\']).next().unwrap(),
                    "sizeBytes": 1,
                    "relativePath": path,
                })
            })
            .collect();
        ValidatedSession::parse(
            serde_json::from_value(json!({ "target": { "kind": "my_files" }, "files": files }))
                .unwrap(),
        )
        .unwrap()
        .files
    }

    #[test]
    fn unit_directory_tree_wire_path_splits_into_prefix_and_leaf() {
        let files = planned(&[
            "Photos/2026/Iceland/photo.jpg",
            "README.md",
            "Photos\\2026\\Iceland\\other.jpg",
        ]);
        assert_eq!(files[0].relative_path(), "Photos/2026/Iceland");
        assert_eq!(files[0].name.display(), "photo.jpg");
        assert_eq!(files[1].relative_path(), "");
        assert_eq!(files[1].name.display(), "README.md");
        assert!(files[1].directory.is_empty());
        assert_eq!(files[2].relative_path(), "Photos/2026/Iceland");
        assert_eq!(files[2].name.display(), "other.jpg");
        let reserved = planned(&["CON/NUL/COM1"]);
        assert_eq!(reserved[0].relative_path(), "CON/NUL");
        assert_eq!(reserved[0].name.display(), "COM1");
    }

    #[test]
    fn unit_directory_tree_plan_resolves_each_distinct_prefix_once() {
        let files = planned(&[
            "Photos/2026/a.jpg",
            "Photos/2026/b.jpg",
            "Photos/2025/c.jpg",
            "root.txt",
        ]);
        let plan = TreePlan::of(&files);
        assert_eq!(plan.display_names(), ["Photos", "2026", "2025"]);
        assert_eq!(plan.len(), 3);
        assert!(TreePlan::of(&planned(&["a.txt"])).is_empty());
    }

    #[test]
    fn unit_directory_tree_plan_merges_normalized_duplicates() {
        let files = planned(&[
            "Photos/a.jpg",
            "photos/b.jpg",
            "PHOTOS/sub/c.jpg",
            "cafe\u{301}/d.jpg",
            "caf\u{e9}/e.jpg",
        ]);
        let plan = TreePlan::of(&files);
        assert_eq!(
            plan.display_names(),
            ["Photos", "sub", "caf\u{e9}"],
            "the first spelling is the folder that is created"
        );
    }

    #[test]
    fn unit_directory_tree_same_leaf_name_in_different_paths_stays_distinct() {
        let files = planned(&["a/same.txt", "b/same.txt", "a/b/same.txt"]);
        let plan = TreePlan::of(&files);
        assert_eq!(plan.display_names(), ["a", "b", "b"]);
        assert_eq!(plan.len(), 3);
    }

    #[test]
    fn unit_directory_tree_depth_counts_the_target_ancestry() {
        let files = planned(&["a/b/c/f"]);
        assert!(!exceeds_depth(61, &files));
        assert!(exceeds_depth(63, &files));
        let flat = planned(&["f"]);
        assert!(!exceeds_depth(1_000, &flat));
    }
}

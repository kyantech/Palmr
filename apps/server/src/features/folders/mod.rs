mod browse;
pub mod delete;
mod ensure_path;
mod error;
pub mod impact;
mod model;
mod r#move;
mod repo;
pub mod routes;
mod service;
pub(crate) mod visibility;

pub use browse::{count_child_folders, folder_paths, list_child_folders, ChildFolder};
pub use delete::{claim_folder_deletion_in_tx, ClaimOutcome};
pub use error::FolderError;
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "the folder flow tests assert on the read-only folder validation seam"
    )
)]
pub use model::OwnedFolder;
pub use model::{FolderId, FolderItem, FolderPathItem};
pub use r#move::move_folder_in_tx;
pub use service::FolderService;
pub use service::{resolve_owned_folder, resolve_writable_folder};

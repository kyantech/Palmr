mod browse;
mod ensure_path;
mod error;
mod model;
mod r#move;
mod repo;
pub mod routes;
mod service;

pub use browse::{count_child_folders, list_child_folders, ChildFolder};
pub use error::FolderError;
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "the folder flow tests assert on the read-only folder validation seam"
    )
)]
pub use model::OwnedFolder;
pub use model::{FolderId, FolderItem};
pub use r#move::move_folder_in_tx;
pub use service::resolve_owned_folder;
pub use service::FolderService;

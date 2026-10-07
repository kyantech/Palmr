mod error;
mod model;
mod repo;
pub mod routes;
mod service;

#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "the files feature consumes the read-only folder validation seam when file placement is wired"
    )
)]
pub use error::FolderError;
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "the files feature consumes the read-only folder validation seam when file placement is wired"
    )
)]
pub use model::{FolderId, OwnedFolder};
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "the files feature consumes the read-only folder validation seam when file placement is wired"
    )
)]
pub use service::resolve_owned_folder;
pub use service::FolderService;

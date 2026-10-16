mod error;
mod model;
pub mod preview;
pub mod routes;
mod service;
mod stream;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        unused_imports,
        reason = "the pure ZIP64 writer is consumed by the archive endpoint task"
    )
)]
pub mod zip;

#[cfg(test)]
mod tests;

pub use service::DownloadService;

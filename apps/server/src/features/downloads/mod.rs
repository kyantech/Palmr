mod error;
mod model;
pub mod routes;
mod service;
mod stream;

#[cfg(test)]
mod tests;

pub use service::DownloadService;

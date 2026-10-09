pub mod delete;
mod error;
mod model;
pub mod naming_insert;
mod repo;
pub mod routes;
mod search;
mod service;

#[cfg(test)]
mod naming_insert_tests;
#[cfg(test)]
mod search_tests;

#[cfg(test)]
pub(crate) use search::{push_indexed, push_scanned, Probe, SCAN_WINDOW, SEARCH_SORT};

pub use model::FileId;
pub use service::FileService;

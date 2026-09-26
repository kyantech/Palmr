pub mod error;
pub mod model;
pub mod preferences;
pub mod profile;
pub mod repo;
pub mod routes;
pub mod service;

pub use profile::ProfileService;

#[cfg(test)]
mod tests;

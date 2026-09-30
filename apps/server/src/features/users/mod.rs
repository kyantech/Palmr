pub mod admin_model;
pub mod admin_repo;
pub mod admin_routes;
pub mod admin_service;
pub mod error;
pub mod model;
pub mod preferences;
pub mod profile;
pub mod repo;
pub mod routes;
pub mod service;

pub use admin_service::AdminUserService;
pub use profile::ProfileService;

#[cfg(test)]
mod admin_tests;
#[cfg(test)]
mod tests;

pub mod authorize;
pub mod callback;
pub mod claims;
pub mod discovery;
pub mod draft;
pub mod error;
pub mod exchange;
pub mod http_client;
pub mod input;
pub mod jwks;
pub mod model;
pub mod oauth2;
pub mod oidc;
pub mod presets;
pub mod provider_test;
pub mod provision;
pub mod repo;
pub mod resolve;
pub mod routes;
pub mod service;

pub use http_client::ProviderHttpClient;
pub use service::IdentityProviderService;

#[cfg(test)]
mod client_tests;
#[cfg(test)]
pub(crate) mod test_support;

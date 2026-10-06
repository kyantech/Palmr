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
pub mod link;
mod link_repo;
pub mod link_routes;
pub mod model;
pub mod oauth2;
pub mod oidc;
pub mod password_login;
pub mod password_login_routes;
pub mod presets;
pub mod provider_test;
pub mod provision;
pub mod reauth;
pub mod repo;
pub mod resolve;
pub mod routes;
pub mod service;

pub use http_client::ProviderHttpClient;
pub use password_login::PasswordLoginService;
pub use service::IdentityProviderService;

#[cfg(test)]
mod client_tests;
#[cfg(test)]
pub(crate) mod test_support;

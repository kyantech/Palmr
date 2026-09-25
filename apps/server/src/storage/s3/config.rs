use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use url::{Origin, Url};

use crate::config::{S3Config, S3TlsVerification, StorageConfig};
use crate::domain::secret::Secret;

use super::profile::ProviderProfile;
use super::tls::S3TlsPolicy;

pub const STORAGE_CONFIG_INVALID: &str = "STORAGE_CONFIG_INVALID";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3SetupError {
    CaFileUnreadable { path: PathBuf },
    CaFileEmpty { path: PathBuf },
    CaFileMalformed { path: PathBuf },
    TlsConfiguration,
    PresigningConfiguration,
}

impl S3SetupError {
    pub const fn code(&self) -> &'static str {
        STORAGE_CONFIG_INVALID
    }
}

impl fmt::Display for S3SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CaFileUnreadable { path } => write!(
                f,
                "{STORAGE_CONFIG_INVALID}: the S3 CA file {path:?} could not be read"
            ),
            Self::CaFileEmpty { path } => write!(
                f,
                "{STORAGE_CONFIG_INVALID}: the S3 CA file {path:?} contains no certificates"
            ),
            Self::CaFileMalformed { path } => write!(
                f,
                "{STORAGE_CONFIG_INVALID}: the S3 CA file {path:?} is not a valid PEM certificate bundle"
            ),
            Self::TlsConfiguration => {
                f.write_str("STORAGE_CONFIG_INVALID: the S3 TLS configuration is invalid")
            }
            Self::PresigningConfiguration => f.write_str(
                "STORAGE_CONFIG_INVALID: the S3 presigning configuration is invalid",
            ),
        }
    }
}

impl std::error::Error for S3SetupError {}

pub fn effective_public_endpoint(storage: &StorageConfig) -> Option<&Url> {
    match storage {
        StorageConfig::Local => None,
        StorageConfig::S3(s3) => Some(s3.public_endpoint.as_ref().unwrap_or(&s3.endpoint)),
    }
}

pub fn public_origin(endpoint: &Url) -> Option<String> {
    let origin @ Origin::Tuple(..) = endpoint.origin() else {
        return None;
    };
    let serialized = origin.ascii_serialization();
    serialized
        .bytes()
        .all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'/' | b'.' | b'-' | b'[' | b']')
        })
        .then_some(serialized)
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedS3Config {
    pub profile: ProviderProfile,
    pub endpoint: Url,
    pub public_endpoint: Url,
    pub region: String,
    pub bucket: String,
    pub access_key: Secret<String>,
    pub secret_key: Secret<String>,
    pub force_path_style: bool,
    pub ca_file: Option<PathBuf>,
    pub verify_certificates: bool,
    pub multipart_ttl: Duration,
}

impl ResolvedS3Config {
    pub fn from_config(s3: &S3Config) -> Self {
        Self {
            profile: ProviderProfile::from_config(s3.profile),
            endpoint: s3.endpoint.clone(),
            public_endpoint: s3
                .public_endpoint
                .clone()
                .unwrap_or_else(|| s3.endpoint.clone()),
            region: s3.region.clone(),
            bucket: s3.bucket.clone(),
            access_key: s3.access_key.clone(),
            secret_key: s3.secret_key.clone(),
            force_path_style: s3.force_path_style,
            ca_file: s3.ca_file.clone(),
            verify_certificates: s3.tls_verification == S3TlsVerification::Enabled,
            multipart_ttl: s3.multipart_ttl,
        }
    }

    pub const fn tls_policy(&self) -> S3TlsPolicy {
        S3TlsPolicy {
            verify_certificates: self.verify_certificates,
            additional_ca: self.ca_file.is_some(),
        }
    }
}

impl fmt::Debug for ResolvedS3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedS3Config")
            .field("profile", &self.profile)
            .field("endpoint", &self.endpoint)
            .field("public_endpoint", &self.public_endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &crate::domain::secret::REDACTED)
            .field("secret_key", &crate::domain::secret::REDACTED)
            .field("force_path_style", &self.force_path_style)
            .field("ca_file", &self.ca_file)
            .field("verify_certificates", &self.verify_certificates)
            .field("multipart_ttl", &self.multipart_ttl)
            .finish()
    }
}

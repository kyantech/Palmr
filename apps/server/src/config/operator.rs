use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use figment::value::{Dict, Map, Value};
use figment::{Figment, Metadata, Profile, Provider};
use ipnet::IpNet;
use url::Url;

use super::validate::{self, ConfigError};
use crate::domain::secret::Secret;

pub const STARTUP_BASE_URL_DEFAULTED: &str = "STARTUP_BASE_URL_DEFAULTED";

const VARIABLE_PREFIX: &str = "PALMR_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Variable {
    Host,
    Port,
    BaseUrl,
    DataDir,
    TrustProxy,
    LogLevel,
    LogFormat,
    StorageProvider,
    S3Profile,
    S3Endpoint,
    S3PublicEndpoint,
    S3Region,
    S3Bucket,
    S3AccessKey,
    S3SecretKey,
    S3ForcePathStyle,
    S3CaFile,
    S3RejectUnauthorized,
    S3MultipartTtlHours,
    DbReadConnections,
    DbSynchronous,
    ZipMaxEntries,
    JobWorkers,
    ShutdownGraceSecs,
    UploadBufferBytes,
    MaxConcurrentTransfers,
    StorageOrphanReap,
    DefaultLanguage,
}

impl Variable {
    pub const ALL: [Self; 28] = [
        Self::Host,
        Self::Port,
        Self::BaseUrl,
        Self::DataDir,
        Self::TrustProxy,
        Self::LogLevel,
        Self::LogFormat,
        Self::StorageProvider,
        Self::S3Profile,
        Self::S3Endpoint,
        Self::S3PublicEndpoint,
        Self::S3Region,
        Self::S3Bucket,
        Self::S3AccessKey,
        Self::S3SecretKey,
        Self::S3ForcePathStyle,
        Self::S3CaFile,
        Self::S3RejectUnauthorized,
        Self::S3MultipartTtlHours,
        Self::DbReadConnections,
        Self::DbSynchronous,
        Self::ZipMaxEntries,
        Self::JobWorkers,
        Self::ShutdownGraceSecs,
        Self::UploadBufferBytes,
        Self::MaxConcurrentTransfers,
        Self::StorageOrphanReap,
        Self::DefaultLanguage,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Host => "PALMR_HOST",
            Self::Port => "PALMR_PORT",
            Self::BaseUrl => "PALMR_BASE_URL",
            Self::DataDir => "PALMR_DATA_DIR",
            Self::TrustProxy => "PALMR_TRUST_PROXY",
            Self::LogLevel => "PALMR_LOG_LEVEL",
            Self::LogFormat => "PALMR_LOG_FORMAT",
            Self::StorageProvider => "PALMR_STORAGE_PROVIDER",
            Self::S3Profile => "PALMR_S3_PROFILE",
            Self::S3Endpoint => "PALMR_S3_ENDPOINT",
            Self::S3PublicEndpoint => "PALMR_S3_PUBLIC_ENDPOINT",
            Self::S3Region => "PALMR_S3_REGION",
            Self::S3Bucket => "PALMR_S3_BUCKET",
            Self::S3AccessKey => "PALMR_S3_ACCESS_KEY",
            Self::S3SecretKey => "PALMR_S3_SECRET_KEY",
            Self::S3ForcePathStyle => "PALMR_S3_FORCE_PATH_STYLE",
            Self::S3CaFile => "PALMR_S3_CA_FILE",
            Self::S3RejectUnauthorized => "PALMR_S3_REJECT_UNAUTHORIZED",
            Self::S3MultipartTtlHours => "PALMR_S3_MULTIPART_TTL_HOURS",
            Self::DbReadConnections => "PALMR_DB_READ_CONNECTIONS",
            Self::DbSynchronous => "PALMR_DB_SYNCHRONOUS",
            Self::ZipMaxEntries => "PALMR_ZIP_MAX_ENTRIES",
            Self::JobWorkers => "PALMR_JOB_WORKERS",
            Self::ShutdownGraceSecs => "PALMR_SHUTDOWN_GRACE_SECS",
            Self::UploadBufferBytes => "PALMR_UPLOAD_BUFFER_BYTES",
            Self::MaxConcurrentTransfers => "PALMR_MAX_CONCURRENT_TRANSFERS",
            Self::StorageOrphanReap => "PALMR_STORAGE_ORPHAN_REAP",
            Self::DefaultLanguage => "PALMR_DEFAULT_LANGUAGE",
        }
    }

    pub const fn is_secret(self) -> bool {
        matches!(self, Self::S3AccessKey | Self::S3SecretKey)
    }

    pub const fn is_s3_only(self) -> bool {
        matches!(
            self,
            Self::S3Profile
                | Self::S3Endpoint
                | Self::S3PublicEndpoint
                | Self::S3Region
                | Self::S3Bucket
                | Self::S3AccessKey
                | Self::S3SecretKey
                | Self::S3ForcePathStyle
                | Self::S3CaFile
                | Self::S3RejectUnauthorized
                | Self::S3MultipartTtlHours
        )
    }

    pub const fn is_url(self) -> bool {
        matches!(
            self,
            Self::BaseUrl | Self::S3Endpoint | Self::S3PublicEndpoint
        )
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variable| variable.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorConfig {
    pub host: IpAddr,
    pub port: u16,
    pub base_url: PublicBaseUrl,
    pub data_dir: PathBuf,
    pub trust_proxy: TrustProxy,
    pub log_level: String,
    pub log_format: LogFormat,
    pub storage: StorageConfig,
    pub storage_orphan_reap: bool,
    pub db_read_connections: u8,
    pub db_synchronous: SqliteSynchronous,
    pub zip_max_entries: u32,
    pub job_workers: u16,
    pub shutdown_grace: Duration,
    pub upload_buffer_bytes: u32,
    pub max_concurrent_transfers: u16,
    pub setup_default_language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBaseUrl {
    url: Url,
    source: PublicBaseUrlSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicBaseUrlSource {
    Configured,
    Defaulted,
}

impl PublicBaseUrl {
    pub(super) const fn configured(url: Url) -> Self {
        Self {
            url,
            source: PublicBaseUrlSource::Configured,
        }
    }

    pub(super) fn defaulted(port: u16) -> Result<Self, url::ParseError> {
        Ok(Self {
            url: Url::parse(&format!("http://localhost:{port}"))?,
            source: PublicBaseUrlSource::Defaulted,
        })
    }

    pub const fn url(&self) -> &Url {
        &self.url
    }

    pub const fn source(&self) -> PublicBaseUrlSource {
        self.source
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustProxy {
    Off,
    AllowList(Vec<IpNet>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Pretty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteSynchronous {
    Full,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageConfig {
    Local,
    S3(Box<S3Config>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Config {
    pub profile: S3Profile,
    pub endpoint: Url,
    pub public_endpoint: Option<Url>,
    pub region: String,
    pub bucket: String,
    pub access_key: Secret<String>,
    pub secret_key: Secret<String>,
    pub force_path_style: bool,
    pub ca_file: Option<PathBuf>,
    pub tls_verification: S3TlsVerification,
    pub multipart_ttl: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3Profile {
    Generic,
    Aws,
    Minio,
    R2,
    Rustfs,
    B2,
    Gcs,
    Wasabi,
    Garage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3TlsVerification {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigWarning {
    BaseUrlDefaulted { effective: Url },
    S3VariablesIgnored { variables: Vec<Variable> },
    S3TlsVerificationDisabled { endpoint: Url },
    UnknownVariable { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: OperatorConfig,
    pub warnings: Vec<ConfigWarning>,
}

impl OperatorConfig {
    pub fn load(source: &EnvironmentSource) -> Result<LoadedConfig, ConfigError> {
        let figment = Figment::from(source);
        let raw = RawEnvironment {
            values: Variable::ALL
                .into_iter()
                .filter_map(|variable| {
                    figment
                        .extract_inner::<String>(variable.name())
                        .ok()
                        .map(|value| (variable, value))
                })
                .collect(),
            not_unicode: source.not_unicode.clone(),
        };
        validate::validate(&raw, &source.unknown)
    }
}

pub struct EnvironmentSource {
    values: Dict,
    not_unicode: Vec<Variable>,
    unknown: Vec<String>,
}

impl EnvironmentSource {
    pub fn from_process() -> Self {
        Self::from_vars(std::env::vars_os())
    }

    pub fn from_vars<I, K, V>(vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        let mut source = Self {
            values: Dict::new(),
            not_unicode: Vec::new(),
            unknown: Vec::new(),
        };
        for (key, value) in vars {
            let key = key.into();
            let key = key.to_string_lossy();
            if !key.starts_with(VARIABLE_PREFIX) {
                continue;
            }
            let Some(variable) = Variable::from_name(&key) else {
                source.unknown.push(key.into_owned());
                continue;
            };
            match value.into().into_string() {
                Ok(text) if text.is_empty() => {}
                Ok(text) => {
                    source
                        .values
                        .insert(variable.name().to_owned(), Value::from(text));
                }
                Err(_) => source.not_unicode.push(variable),
            }
        }
        source.unknown.sort();
        source.unknown.dedup();
        source
    }
}

impl Provider for EnvironmentSource {
    fn metadata(&self) -> Metadata {
        Metadata::named("PALMR_* environment variables")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        Ok(Profile::Default.collect(self.values.clone()))
    }
}

pub(super) struct RawEnvironment {
    values: BTreeMap<Variable, String>,
    not_unicode: Vec<Variable>,
}

pub(super) enum RawValue<'a> {
    Absent,
    NotUnicode,
    Text(&'a str),
}

impl RawEnvironment {
    pub(super) fn get(&self, variable: Variable) -> RawValue<'_> {
        if self.not_unicode.contains(&variable) {
            return RawValue::NotUnicode;
        }
        self.values
            .get(&variable)
            .map_or(RawValue::Absent, |text| RawValue::Text(text))
    }

    pub(super) fn is_present(&self, variable: Variable) -> bool {
        !matches!(self.get(variable), RawValue::Absent)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        ConfigWarning, EnvironmentSource, LoadedConfig, LogFormat, OperatorConfig,
        PublicBaseUrlSource, S3Profile, S3TlsVerification, SqliteSynchronous, StorageConfig,
        TrustProxy, Variable,
    };
    use crate::config::ConfigError;

    pub(in crate::config) const ACCESS_KEY: &str = "AKIA-access-key-sentinel";
    pub(in crate::config) const SECRET_KEY: &str = "secret-key-sentinel-9f2c";

    pub(in crate::config) const COMPLETE_S3: &[(&str, &str)] = &[
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_ENDPOINT", "http://minio:9000"),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", "palmr"),
        ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
        ("PALMR_S3_SECRET_KEY", SECRET_KEY),
    ];

    pub(in crate::config) fn load(vars: &[(&str, &str)]) -> Result<LoadedConfig, ConfigError> {
        OperatorConfig::load(&EnvironmentSource::from_vars(vars.iter().copied()))
    }

    pub(in crate::config) fn with(
        base: &[(&str, &str)],
        extra: &[(&'static str, &'static str)],
    ) -> Vec<(String, String)> {
        let mut vars: Vec<(String, String)> = base
            .iter()
            .filter(|(key, _)| !extra.iter().any(|(extra_key, _)| extra_key == key))
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        vars.extend(
            extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        vars
    }

    pub(in crate::config) fn load_owned(
        vars: Vec<(String, String)>,
    ) -> Result<LoadedConfig, ConfigError> {
        OperatorConfig::load(&EnvironmentSource::from_vars(vars))
    }

    #[test]
    fn unit_config_defaults() {
        let LoadedConfig { config, warnings } = load(&[]).unwrap();

        assert_eq!(config.host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(config.port, 5487);
        assert_eq!(config.base_url.url().as_str(), "http://localhost:5487/");
        assert_eq!(config.base_url.source(), PublicBaseUrlSource::Defaulted);
        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert_eq!(config.trust_proxy, TrustProxy::Off);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.storage, StorageConfig::Local);
        assert!(!config.storage_orphan_reap);
        assert_eq!(config.db_read_connections, 4);
        assert_eq!(config.db_synchronous, SqliteSynchronous::Full);
        assert_eq!(config.zip_max_entries, 100_000);
        assert_eq!(config.job_workers, 2);
        assert_eq!(config.shutdown_grace, Duration::from_secs(30));
        assert_eq!(config.upload_buffer_bytes, 262_144);
        assert_eq!(config.max_concurrent_transfers, 3);
        assert_eq!(config.setup_default_language, None);
        assert_eq!(
            warnings,
            vec![ConfigWarning::BaseUrlDefaulted {
                effective: config.base_url.url().clone()
            }]
        );
    }

    #[test]
    fn unit_config_s3_defaults() {
        let config = load(COMPLETE_S3).unwrap().config;
        let StorageConfig::S3(s3) = config.storage else {
            panic!("expected S3 storage");
        };

        assert_eq!(s3.profile, S3Profile::Generic);
        assert_eq!(s3.endpoint.as_str(), "http://minio:9000/");
        assert_eq!(s3.public_endpoint, None);
        assert_eq!(s3.region, "us-east-1");
        assert_eq!(s3.bucket, "palmr");
        assert_eq!(s3.access_key.expose(), ACCESS_KEY);
        assert_eq!(s3.secret_key.expose(), SECRET_KEY);
        assert!(s3.force_path_style);
        assert_eq!(s3.ca_file, None);
        assert_eq!(s3.tls_verification, S3TlsVerification::Enabled);
        assert_eq!(s3.multipart_ttl, Duration::from_secs(24 * 3_600));
    }

    #[test]
    fn unit_config_accepts_every_explicit_value() {
        let vars = with(
            COMPLETE_S3,
            &[
                ("PALMR_HOST", "::1"),
                ("PALMR_PORT", "8080"),
                ("PALMR_BASE_URL", "https://files.example.com/palmr"),
                ("PALMR_DATA_DIR", "/srv/palmr"),
                ("PALMR_TRUST_PROXY", "10.0.0.0/8, 192.168.1.10"),
                ("PALMR_LOG_LEVEL", "palmr=debug,info"),
                ("PALMR_LOG_FORMAT", "pretty"),
                ("PALMR_S3_PROFILE", "minio"),
                ("PALMR_S3_PUBLIC_ENDPOINT", "https://s3.example.com"),
                ("PALMR_S3_FORCE_PATH_STYLE", "false"),
                ("PALMR_S3_CA_FILE", "/data/ca.pem"),
                ("PALMR_S3_MULTIPART_TTL_HOURS", "48"),
                ("PALMR_DB_READ_CONNECTIONS", "16"),
                ("PALMR_DB_SYNCHRONOUS", "normal"),
                ("PALMR_ZIP_MAX_ENTRIES", "250000"),
                ("PALMR_JOB_WORKERS", "4"),
                ("PALMR_SHUTDOWN_GRACE_SECS", "60"),
                ("PALMR_UPLOAD_BUFFER_BYTES", "524288"),
                ("PALMR_MAX_CONCURRENT_TRANSFERS", "2"),
                ("PALMR_STORAGE_ORPHAN_REAP", "true"),
                ("PALMR_DEFAULT_LANGUAGE", "pt-BR"),
            ],
        );
        let LoadedConfig { config, warnings } = load_owned(vars).unwrap();

        assert_eq!(config.host, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(config.port, 8080);
        assert_eq!(
            config.base_url.url().as_str(),
            "https://files.example.com/palmr"
        );
        assert_eq!(config.base_url.source(), PublicBaseUrlSource::Configured);
        assert_eq!(config.data_dir, PathBuf::from("/srv/palmr"));
        assert_eq!(
            config.trust_proxy,
            TrustProxy::AllowList(vec![
                "10.0.0.0/8".parse().unwrap(),
                "192.168.1.10/32".parse().unwrap()
            ])
        );
        assert_eq!(config.log_level, "palmr=debug,info");
        assert_eq!(config.log_format, LogFormat::Pretty);
        assert!(config.storage_orphan_reap);
        assert_eq!(config.db_read_connections, 16);
        assert_eq!(config.db_synchronous, SqliteSynchronous::Normal);
        assert_eq!(config.zip_max_entries, 250_000);
        assert_eq!(config.job_workers, 4);
        assert_eq!(config.shutdown_grace, Duration::from_secs(60));
        assert_eq!(config.upload_buffer_bytes, 524_288);
        assert_eq!(config.max_concurrent_transfers, 2);
        assert_eq!(config.setup_default_language.as_deref(), Some("pt-BR"));

        let StorageConfig::S3(s3) = config.storage else {
            panic!("expected S3 storage");
        };
        assert_eq!(s3.profile, S3Profile::Minio);
        assert_eq!(
            s3.public_endpoint.as_ref().map(url::Url::as_str),
            Some("https://s3.example.com/")
        );
        assert!(!s3.force_path_style);
        assert_eq!(s3.ca_file, Some(PathBuf::from("/data/ca.pem")));
        assert_eq!(s3.multipart_ttl, Duration::from_secs(48 * 3_600));
        assert!(warnings.is_empty());
    }

    #[test]
    fn unit_config_choices_ignore_ascii_case() {
        let config = load(&[
            ("PALMR_LOG_FORMAT", "PRETTY"),
            ("PALMR_DB_SYNCHRONOUS", "Normal"),
            ("PALMR_STORAGE_ORPHAN_REAP", "TRUE"),
            ("PALMR_TRUST_PROXY", "OFF"),
        ])
        .unwrap()
        .config;

        assert_eq!(config.log_format, LogFormat::Pretty);
        assert_eq!(config.db_synchronous, SqliteSynchronous::Normal);
        assert!(config.storage_orphan_reap);
        assert_eq!(config.trust_proxy, TrustProxy::Off);
    }

    #[test]
    fn unit_config_empty_value_is_unset() {
        let LoadedConfig { config, warnings } =
            load(&[("PALMR_BASE_URL", ""), ("PALMR_PORT", "")]).unwrap();

        assert_eq!(config.port, 5487);
        assert_eq!(config.base_url.source(), PublicBaseUrlSource::Defaulted);
        assert!(matches!(
            warnings.as_slice(),
            [ConfigWarning::BaseUrlDefaulted { .. }]
        ));
    }

    #[test]
    fn it_base_url_default_localhost_with_warning() {
        let default_port = load(&[]).unwrap();
        assert_eq!(
            default_port.config.base_url.url().as_str(),
            "http://localhost:5487/"
        );
        assert_eq!(
            default_port.warnings,
            vec![ConfigWarning::BaseUrlDefaulted {
                effective: "http://localhost:5487/".parse().unwrap()
            }]
        );

        let custom_port = load(&[("PALMR_PORT", "8080")]).unwrap();
        assert_eq!(
            custom_port.config.base_url.url().as_str(),
            "http://localhost:8080/"
        );
        assert_eq!(
            custom_port.config.base_url.source(),
            PublicBaseUrlSource::Defaulted
        );
        assert!(custom_port
            .warnings
            .contains(&ConfigWarning::BaseUrlDefaulted {
                effective: "http://localhost:8080/".parse().unwrap()
            }));

        let configured = load(&[("PALMR_BASE_URL", "https://files.example.com")]).unwrap();
        assert_eq!(
            configured.config.base_url.source(),
            PublicBaseUrlSource::Configured
        );
        assert!(!configured
            .warnings
            .iter()
            .any(|warning| matches!(warning, ConfigWarning::BaseUrlDefaulted { .. })));
        assert_eq!(
            super::STARTUP_BASE_URL_DEFAULTED,
            "STARTUP_BASE_URL_DEFAULTED"
        );
    }

    #[test]
    fn unit_config_unknown_palmr_variables_warned() {
        let LoadedConfig { warnings, .. } = load(&[
            ("PALMR_UID", "1000"),
            ("PALMR_GID", "1000"),
            ("PALMR_PROT", "8080"),
            ("PALMR_S3_SECRETKEY", SECRET_KEY),
            ("PALMR_port", "8080"),
            ("palmr_port", "8080"),
            ("HOME", "/root"),
            ("S3_SECRET_KEY", SECRET_KEY),
        ])
        .unwrap();

        let unknown: Vec<&str> = warnings
            .iter()
            .filter_map(|warning| match warning {
                ConfigWarning::UnknownVariable { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            unknown,
            [
                "PALMR_GID",
                "PALMR_PROT",
                "PALMR_S3_SECRETKEY",
                "PALMR_UID",
                "PALMR_port"
            ]
        );
        assert!(!format!("{warnings:?}").contains(SECRET_KEY));
    }

    #[test]
    fn unit_config_uid_gid_are_not_configuration() {
        let config = load(&[("PALMR_UID", "1000"), ("PALMR_GID", "1000")])
            .unwrap()
            .config;

        assert_eq!(config, load(&[]).unwrap().config);
        assert_eq!(Variable::from_name("PALMR_UID"), None);
        assert_eq!(Variable::from_name("PALMR_GID"), None);
    }

    #[test]
    fn unit_config_s3_variables_ignored_for_local_storage() {
        let LoadedConfig { config, warnings } = load(&[
            ("PALMR_S3_BUCKET", "palmr"),
            ("PALMR_S3_SECRET_KEY", SECRET_KEY),
            ("PALMR_S3_PROFILE", "not-a-profile"),
        ])
        .unwrap();

        assert_eq!(config.storage, StorageConfig::Local);
        assert!(warnings.contains(&ConfigWarning::S3VariablesIgnored {
            variables: vec![
                Variable::S3Profile,
                Variable::S3Bucket,
                Variable::S3SecretKey
            ],
        }));
    }

    #[test]
    fn unit_config_s3_tls_verification_disabled_is_warned() {
        let vars = with(COMPLETE_S3, &[("PALMR_S3_REJECT_UNAUTHORIZED", "false")]);
        let LoadedConfig { config, warnings } = load_owned(vars).unwrap();

        let StorageConfig::S3(s3) = &config.storage else {
            panic!("expected S3 storage");
        };
        assert_eq!(s3.tls_verification, S3TlsVerification::Disabled);
        assert!(
            warnings.contains(&ConfigWarning::S3TlsVerificationDisabled {
                endpoint: "http://minio:9000".parse().unwrap()
            })
        );
    }

    #[test]
    fn unit_config_never_prints_secrets() {
        let loaded = load(COMPLETE_S3).unwrap();
        let StorageConfig::S3(s3) = &loaded.config.storage else {
            panic!("expected S3 storage");
        };
        let rendered = [
            format!("{loaded:?}"),
            format!("{loaded:#?}"),
            format!("{s3:?}"),
            format!("{:?}", s3.access_key),
            format!("{}", s3.secret_key),
        ];
        for text in &rendered {
            assert!(!text.contains(ACCESS_KEY), "{text}");
            assert!(!text.contains(SECRET_KEY), "{text}");
        }
        assert!(rendered[2].contains("<redacted>"));

        let invalid = with(
            COMPLETE_S3,
            &[
                ("PALMR_PORT", "0"),
                (
                    "PALMR_S3_ENDPOINT",
                    "https://AKIA-access-key-sentinel:secret-key-sentinel-9f2c@minio:9000?x=1",
                ),
                (
                    "PALMR_S3_PUBLIC_ENDPOINT",
                    "https://AKIA-access-key-sentinel@",
                ),
                (
                    "PALMR_BASE_URL",
                    "ftp://secret-key-sentinel-9f2c@example.com",
                ),
            ],
        );
        let error = load_owned(invalid).unwrap_err();
        assert_eq!(error.issues().len(), 4);
        for text in [
            format!("{error}"),
            format!("{error:?}"),
            format!("{error:#?}"),
        ] {
            assert!(!text.contains(ACCESS_KEY), "{text}");
            assert!(!text.contains(SECRET_KEY), "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unit_config_never_prints_non_unicode_secrets() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let mut vars: Vec<(OsString, OsString)> = COMPLETE_S3
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();
        vars.push((
            OsString::from("PALMR_S3_SECRET_KEY"),
            OsString::from_vec(b"secret-key-sentinel-9f2c\xff".to_vec()),
        ));
        vars.retain(|(key, value)| key != "PALMR_S3_SECRET_KEY" || value.to_str().is_none());

        let error = OperatorConfig::load(&EnvironmentSource::from_vars(vars)).unwrap_err();
        let text = format!("{error} {error:?}");
        assert!(text.contains("PALMR_S3_SECRET_KEY (non-UTF-8 value): must be valid UTF-8"));
        assert!(!text.contains(SECRET_KEY));
    }

    #[test]
    fn unit_config_variable_catalog_is_canonical() {
        let names: Vec<&str> = Variable::ALL
            .iter()
            .map(|variable| variable.name())
            .collect();
        assert_eq!(
            names,
            [
                "PALMR_HOST",
                "PALMR_PORT",
                "PALMR_BASE_URL",
                "PALMR_DATA_DIR",
                "PALMR_TRUST_PROXY",
                "PALMR_LOG_LEVEL",
                "PALMR_LOG_FORMAT",
                "PALMR_STORAGE_PROVIDER",
                "PALMR_S3_PROFILE",
                "PALMR_S3_ENDPOINT",
                "PALMR_S3_PUBLIC_ENDPOINT",
                "PALMR_S3_REGION",
                "PALMR_S3_BUCKET",
                "PALMR_S3_ACCESS_KEY",
                "PALMR_S3_SECRET_KEY",
                "PALMR_S3_FORCE_PATH_STYLE",
                "PALMR_S3_CA_FILE",
                "PALMR_S3_REJECT_UNAUTHORIZED",
                "PALMR_S3_MULTIPART_TTL_HOURS",
                "PALMR_DB_READ_CONNECTIONS",
                "PALMR_DB_SYNCHRONOUS",
                "PALMR_ZIP_MAX_ENTRIES",
                "PALMR_JOB_WORKERS",
                "PALMR_SHUTDOWN_GRACE_SECS",
                "PALMR_UPLOAD_BUFFER_BYTES",
                "PALMR_MAX_CONCURRENT_TRANSFERS",
                "PALMR_STORAGE_ORPHAN_REAP",
                "PALMR_DEFAULT_LANGUAGE",
            ]
        );
        for variable in Variable::ALL {
            assert_eq!(Variable::from_name(variable.name()), Some(variable));
        }
        let secrets: Vec<Variable> = Variable::ALL
            .into_iter()
            .filter(|variable| variable.is_secret())
            .collect();
        assert_eq!(secrets, [Variable::S3AccessKey, Variable::S3SecretKey]);
    }
}

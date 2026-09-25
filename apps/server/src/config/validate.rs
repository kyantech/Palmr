use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use ipnet::IpNet;
use url::Url;

use super::operator::{
    ConfigWarning, LoadedConfig, LogFormat, OperatorConfig, PublicBaseUrl, PublicBaseUrlSource,
    RawEnvironment, RawValue, S3Config, S3Profile, S3TlsVerification, SqliteSynchronous,
    StorageConfig, TrustProxy, Variable,
};
use crate::domain::secret::Secret;

pub const STARTUP_CONFIG_INVALID: &str = "STARTUP_CONFIG_INVALID";

const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
const DEFAULT_PORT: u16 = 5487;
const DEFAULT_DATA_DIR: &str = "/data";
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_DB_READ_CONNECTIONS: u8 = 4;
const DEFAULT_ZIP_MAX_ENTRIES: u32 = 100_000;
const DEFAULT_JOB_WORKERS: u16 = 2;
const DEFAULT_SHUTDOWN_GRACE_SECS: u16 = 30;
const DEFAULT_UPLOAD_BUFFER_BYTES: u32 = 262_144;
const DEFAULT_MAX_CONCURRENT_TRANSFERS: u16 = 3;
const DEFAULT_S3_MULTIPART_TTL_HOURS: u16 = 24;

const PORT_RANGE: RangeInclusive<u16> = 1..=u16::MAX;
const DB_READ_CONNECTIONS_RANGE: RangeInclusive<u8> = 1..=16;
const ZIP_MAX_ENTRIES_RANGE: RangeInclusive<u32> = 1..=10_000_000;
const JOB_WORKERS_RANGE: RangeInclusive<u16> = 1..=64;
const SHUTDOWN_GRACE_SECS_RANGE: RangeInclusive<u16> = 1..=3_600;
const UPLOAD_BUFFER_BYTES_RANGE: RangeInclusive<u32> = 4_096..=16_777_216;
const MAX_CONCURRENT_TRANSFERS_RANGE: RangeInclusive<u16> = 1..=64;
const S3_MULTIPART_TTL_HOURS_RANGE: RangeInclusive<u16> = 1..=720;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    issues: Vec<ConfigIssue>,
}

impl ConfigError {
    pub const fn code(&self) -> &'static str {
        STARTUP_CONFIG_INVALID
    }

    pub fn issues(&self) -> &[ConfigIssue] {
        &self.issues
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{STARTUP_CONFIG_INVALID}: invalid operator configuration"
        )?;
        for issue in &self.issues {
            write!(f, "; {issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub variable: Variable,
    pub received: Received,
    pub constraint: Constraint,
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.variable.name();
        match &self.received {
            Received::NotSet => write!(f, "{name} is not set")?,
            Received::Value(value) => write!(f, "{name}={value:?}")?,
            Received::Withheld => write!(f, "{name} (value withheld)")?,
            Received::NotUnicode => write!(f, "{name} (non-UTF-8 value)")?,
        }
        write!(f, ": {}", self.constraint)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    NotSet,
    Value(String),
    Withheld,
    NotUnicode,
}

impl Received {
    fn text(variable: Variable, text: &str) -> Self {
        if variable.is_secret() || (variable.is_url() && text.contains('@')) {
            Self::Withheld
        } else {
            Self::Value(text.to_owned())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    Unicode,
    Integer { min: u64, max: u64 },
    Boolean,
    OneOf(Vec<&'static str>),
    IpAddress,
    AbsoluteUrl,
    HttpScheme,
    UrlHost,
    UrlWithoutQuery,
    UrlWithoutFragment,
    UrlWithoutCredentials,
    UrlWithoutPath,
    TrustProxyEntry,
    TrustProxyAllMode,
    RequiredForS3,
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unicode => f.write_str("must be valid UTF-8"),
            Self::Integer { min, max } => write!(f, "must be an integer from {min} to {max}"),
            Self::Boolean => f.write_str("must be `true` or `false`"),
            Self::OneOf(choices) => write!(f, "must be one of: {}", choices.join(", ")),
            Self::IpAddress => f.write_str("must be an IP address"),
            Self::AbsoluteUrl => f.write_str("must be an absolute URL"),
            Self::HttpScheme => f.write_str("must use the `http` or `https` scheme"),
            Self::UrlHost => f.write_str("must include a host"),
            Self::UrlWithoutQuery => f.write_str("must not include a query"),
            Self::UrlWithoutFragment => f.write_str("must not include a fragment"),
            Self::UrlWithoutCredentials => f.write_str("must not include credentials"),
            Self::UrlWithoutPath => f.write_str("must not include a path"),
            Self::TrustProxyEntry => f.write_str(
                "must be `off` or a comma-separated list of IP addresses or CIDR ranges",
            ),
            Self::TrustProxyAllMode => f.write_str(
                "trusting every proxy is not supported; list the trusted proxy IP addresses or CIDR ranges explicitly",
            ),
            Self::RequiredForS3 => f.write_str("required when PALMR_STORAGE_PROVIDER=s3"),
        }
    }
}

#[derive(Clone, Copy)]
enum StorageProvider {
    Local,
    S3,
}

trait Choice: Copy + 'static {
    const CHOICES: &'static [(&'static str, Self)];
}

impl Choice for LogFormat {
    const CHOICES: &'static [(&'static str, Self)] =
        &[("json", Self::Json), ("pretty", Self::Pretty)];
}

impl Choice for StorageProvider {
    const CHOICES: &'static [(&'static str, Self)] = &[("local", Self::Local), ("s3", Self::S3)];
}

impl Choice for SqliteSynchronous {
    const CHOICES: &'static [(&'static str, Self)] =
        &[("full", Self::Full), ("normal", Self::Normal)];
}

impl Choice for S3Profile {
    const CHOICES: &'static [(&'static str, Self)] = &[
        ("generic", Self::Generic),
        ("aws", Self::Aws),
        ("minio", Self::Minio),
        ("r2", Self::R2),
        ("rustfs", Self::Rustfs),
        ("b2", Self::B2),
        ("gcs", Self::Gcs),
        ("wasabi", Self::Wasabi),
        ("garage", Self::Garage),
    ];
}

pub(super) fn validate(
    raw: &RawEnvironment,
    unknown: &[String],
) -> Result<LoadedConfig, ConfigError> {
    let mut validator = Validator {
        raw,
        issues: Vec::new(),
    };
    let v = &mut validator;

    let host = v.optional(Variable::Host, |text| {
        text.parse().map_err(|_| Constraint::IpAddress)
    });
    let port = v.optional(Variable::Port, integer(PORT_RANGE));
    let port = port.unwrap_or(DEFAULT_PORT);
    let configured_base_url = v.optional(Variable::BaseUrl, public_base_url);
    let data_dir = v.optional(Variable::DataDir, path);
    let trust_proxy = v.optional(Variable::TrustProxy, trust_proxy);
    let log_level = v.optional(Variable::LogLevel, |text| Ok(text.to_owned()));
    let log_format = v.optional(Variable::LogFormat, choice);
    let provider = v.optional(Variable::StorageProvider, choice);
    let storage_orphan_reap = v.optional(Variable::StorageOrphanReap, boolean);
    let db_read_connections = v.optional(
        Variable::DbReadConnections,
        integer(DB_READ_CONNECTIONS_RANGE),
    );
    let db_synchronous = v.optional(Variable::DbSynchronous, choice);
    let zip_max_entries = v.optional(Variable::ZipMaxEntries, integer(ZIP_MAX_ENTRIES_RANGE));
    let job_workers = v.optional(Variable::JobWorkers, integer(JOB_WORKERS_RANGE));
    let shutdown_grace_secs = v.optional(
        Variable::ShutdownGraceSecs,
        integer(SHUTDOWN_GRACE_SECS_RANGE),
    );
    let upload_buffer_bytes = v.optional(
        Variable::UploadBufferBytes,
        integer(UPLOAD_BUFFER_BYTES_RANGE),
    );
    let max_concurrent_transfers = v.optional(
        Variable::MaxConcurrentTransfers,
        integer(MAX_CONCURRENT_TRANSFERS_RANGE),
    );
    let setup_default_language = v.optional(Variable::DefaultLanguage, |text| Ok(text.to_owned()));

    let base_url = match configured_base_url {
        Some(url) => Some(PublicBaseUrl::configured(url)),
        None => match PublicBaseUrl::defaulted(port) {
            Ok(url) => Some(url),
            Err(_) => {
                v.reject(Variable::BaseUrl, Received::NotSet, Constraint::AbsoluteUrl);
                None
            }
        },
    };

    let mut ignored_s3_variables = Vec::new();
    let storage = match provider.unwrap_or(StorageProvider::Local) {
        StorageProvider::Local => {
            ignored_s3_variables = Variable::ALL
                .into_iter()
                .filter(|variable| variable.is_s3_only() && raw.is_present(*variable))
                .collect();
            Some(StorageConfig::Local)
        }
        StorageProvider::S3 => v.s3().map(|s3| StorageConfig::S3(Box::new(s3))),
    };

    let (Some(base_url), Some(storage), true) = (base_url, storage, validator.issues.is_empty())
    else {
        return Err(ConfigError {
            issues: validator.issues,
        });
    };

    let mut warnings = Vec::new();
    if base_url.source() == PublicBaseUrlSource::Defaulted {
        warnings.push(ConfigWarning::BaseUrlDefaulted {
            effective: base_url.url().clone(),
        });
    }
    if !ignored_s3_variables.is_empty() {
        warnings.push(ConfigWarning::S3VariablesIgnored {
            variables: ignored_s3_variables,
        });
    }
    if let StorageConfig::S3(s3) = &storage {
        if s3.tls_verification == S3TlsVerification::Disabled {
            warnings.push(ConfigWarning::S3TlsVerificationDisabled {
                endpoint: s3.endpoint.clone(),
            });
        }
    }
    warnings.extend(
        unknown
            .iter()
            .map(|name| ConfigWarning::UnknownVariable { name: name.clone() }),
    );

    let config = OperatorConfig {
        host: host.unwrap_or(DEFAULT_HOST),
        port,
        base_url,
        data_dir: data_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR)),
        trust_proxy: trust_proxy.unwrap_or(TrustProxy::Off),
        log_level: log_level.unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_owned()),
        log_format: log_format.unwrap_or(LogFormat::Json),
        storage,
        storage_orphan_reap: storage_orphan_reap.unwrap_or(false),
        db_read_connections: db_read_connections.unwrap_or(DEFAULT_DB_READ_CONNECTIONS),
        db_synchronous: db_synchronous.unwrap_or(SqliteSynchronous::Full),
        zip_max_entries: zip_max_entries.unwrap_or(DEFAULT_ZIP_MAX_ENTRIES),
        job_workers: job_workers.unwrap_or(DEFAULT_JOB_WORKERS),
        shutdown_grace: Duration::from_secs(
            shutdown_grace_secs
                .unwrap_or(DEFAULT_SHUTDOWN_GRACE_SECS)
                .into(),
        ),
        upload_buffer_bytes: upload_buffer_bytes.unwrap_or(DEFAULT_UPLOAD_BUFFER_BYTES),
        max_concurrent_transfers: max_concurrent_transfers
            .unwrap_or(DEFAULT_MAX_CONCURRENT_TRANSFERS),
        setup_default_language,
    };
    Ok(LoadedConfig { config, warnings })
}

struct Validator<'a> {
    raw: &'a RawEnvironment,
    issues: Vec<ConfigIssue>,
}

impl<'a> Validator<'a> {
    fn reject(&mut self, variable: Variable, received: Received, constraint: Constraint) {
        self.issues.push(ConfigIssue {
            variable,
            received,
            constraint,
        });
    }

    fn optional<T>(
        &mut self,
        variable: Variable,
        parse: impl FnOnce(&'a str) -> Result<T, Constraint>,
    ) -> Option<T> {
        match self.raw.get(variable) {
            RawValue::Absent => None,
            RawValue::NotUnicode => {
                self.reject(variable, Received::NotUnicode, Constraint::Unicode);
                None
            }
            RawValue::Text(text) => match parse(text) {
                Ok(value) => Some(value),
                Err(constraint) => {
                    self.reject(variable, Received::text(variable, text), constraint);
                    None
                }
            },
        }
    }

    fn required_for_s3<T>(
        &mut self,
        variable: Variable,
        parse: impl FnOnce(&'a str) -> Result<T, Constraint>,
    ) -> Option<T> {
        if let RawValue::Absent = self.raw.get(variable) {
            self.reject(variable, Received::NotSet, Constraint::RequiredForS3);
            return None;
        }
        self.optional(variable, parse)
    }

    fn s3(&mut self) -> Option<S3Config> {
        let profile = self.optional(Variable::S3Profile, choice);
        let endpoint = self.required_for_s3(Variable::S3Endpoint, s3_endpoint);
        let public_endpoint = self.optional(Variable::S3PublicEndpoint, s3_endpoint);
        let region = self.required_for_s3(Variable::S3Region, |text| Ok(text.to_owned()));
        let bucket = self.required_for_s3(Variable::S3Bucket, |text| Ok(text.to_owned()));
        let access_key = self.required_for_s3(Variable::S3AccessKey, secret);
        let secret_key = self.required_for_s3(Variable::S3SecretKey, secret);
        let force_path_style = self.optional(Variable::S3ForcePathStyle, boolean);
        let ca_file = self.optional(Variable::S3CaFile, path);
        let reject_unauthorized = self.optional(Variable::S3RejectUnauthorized, boolean);
        let multipart_ttl_hours = self.optional(
            Variable::S3MultipartTtlHours,
            integer(S3_MULTIPART_TTL_HOURS_RANGE),
        );

        Some(S3Config {
            profile: profile.unwrap_or(S3Profile::Generic),
            endpoint: endpoint?,
            public_endpoint,
            region: region?,
            bucket: bucket?,
            access_key: access_key?,
            secret_key: secret_key?,
            force_path_style: force_path_style.unwrap_or(true),
            ca_file,
            tls_verification: if reject_unauthorized.unwrap_or(true) {
                S3TlsVerification::Enabled
            } else {
                S3TlsVerification::Disabled
            },
            multipart_ttl: Duration::from_secs(
                u64::from(multipart_ttl_hours.unwrap_or(DEFAULT_S3_MULTIPART_TTL_HOURS)) * 3_600,
            ),
        })
    }
}

fn integer<T>(range: RangeInclusive<T>) -> impl FnOnce(&str) -> Result<T, Constraint>
where
    T: FromStr + PartialOrd + Copy + Into<u64>,
{
    move |text| {
        text.parse::<T>()
            .ok()
            .filter(|value| range.contains(value))
            .ok_or(Constraint::Integer {
                min: (*range.start()).into(),
                max: (*range.end()).into(),
            })
    }
}

fn boolean(text: &str) -> Result<bool, Constraint> {
    if text.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if text.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(Constraint::Boolean)
    }
}

fn choice<T: Choice>(text: &str) -> Result<T, Constraint> {
    T::CHOICES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(text))
        .map(|(_, value)| *value)
        .ok_or_else(|| Constraint::OneOf(T::CHOICES.iter().map(|(name, _)| *name).collect()))
}

fn path(text: &str) -> Result<PathBuf, Constraint> {
    Ok(PathBuf::from(text))
}

fn secret(text: &str) -> Result<Secret<String>, Constraint> {
    Ok(Secret::new(text.to_owned()))
}

fn trust_proxy(text: &str) -> Result<TrustProxy, Constraint> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("off") {
        return Ok(TrustProxy::Off);
    }
    let entries: Vec<&str> = text.split(',').map(str::trim).collect();
    if entries
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case("all") || *entry == "*")
    {
        return Err(Constraint::TrustProxyAllMode);
    }
    let mut networks = Vec::with_capacity(entries.len());
    for entry in entries {
        let network = entry
            .parse::<IpNet>()
            .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
            .map_err(|_| Constraint::TrustProxyEntry)?;
        // A /0 prefix matches every peer, which is the forbidden `all` mode under another spelling (Decision 75).
        if network.prefix_len() == 0 {
            return Err(Constraint::TrustProxyAllMode);
        }
        networks.push(network);
    }
    Ok(TrustProxy::AllowList(networks))
}

fn http_url(text: &str) -> Result<Url, Constraint> {
    let url = Url::parse(text).map_err(|error| match error {
        url::ParseError::EmptyHost => Constraint::UrlHost,
        _ => Constraint::AbsoluteUrl,
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Constraint::HttpScheme);
    }
    if url.host().is_none() {
        return Err(Constraint::UrlHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Constraint::UrlWithoutCredentials);
    }
    if url.query().is_some() {
        return Err(Constraint::UrlWithoutQuery);
    }
    if url.fragment().is_some() {
        return Err(Constraint::UrlWithoutFragment);
    }
    Ok(url)
}

fn public_base_url(text: &str) -> Result<Url, Constraint> {
    http_url(text)
}

fn s3_endpoint(text: &str) -> Result<Url, Constraint> {
    let url = http_url(text)?;
    if url.path() != "/" {
        return Err(Constraint::UrlWithoutPath);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{Constraint, Received, STARTUP_CONFIG_INVALID};
    use crate::config::operator::tests::{load, load_owned, with, COMPLETE_S3};
    use crate::config::{S3Profile, StorageConfig, TrustProxy, Variable};

    fn one_of(choices: &[&'static str]) -> Constraint {
        Constraint::OneOf(choices.to_vec())
    }

    fn integer(min: u64, max: u64) -> Constraint {
        Constraint::Integer { min, max }
    }

    const PROFILES: &[&str] = &[
        "generic", "aws", "minio", "r2", "rustfs", "b2", "gcs", "wasabi", "garage",
    ];

    #[rstest]
    #[case::port_zero("PALMR_PORT", "0", integer(1, 65_535))]
    #[case::port_too_large("PALMR_PORT", "65536", integer(1, 65_535))]
    #[case::port_not_numeric("PALMR_PORT", "http", integer(1, 65_535))]
    #[case::port_negative("PALMR_PORT", "-1", integer(1, 65_535))]
    #[case::host_name("PALMR_HOST", "localhost", Constraint::IpAddress)]
    #[case::host_with_port("PALMR_HOST", "0.0.0.0:5487", Constraint::IpAddress)]
    #[case::base_url_malformed("PALMR_BASE_URL", "not a url", Constraint::AbsoluteUrl)]
    #[case::base_url_relative("PALMR_BASE_URL", "/palmr", Constraint::AbsoluteUrl)]
    #[case::base_url_scheme("PALMR_BASE_URL", "ftp://files.example.com", Constraint::HttpScheme)]
    #[case::base_url_host_as_scheme(
        "PALMR_BASE_URL",
        "files.example.com:443",
        Constraint::HttpScheme
    )]
    #[case::base_url_without_host("PALMR_BASE_URL", "https://", Constraint::UrlHost)]
    #[case::base_url_query(
        "PALMR_BASE_URL",
        "https://files.example.com/?a=b",
        Constraint::UrlWithoutQuery
    )]
    #[case::base_url_fragment(
        "PALMR_BASE_URL",
        "https://files.example.com/#top",
        Constraint::UrlWithoutFragment
    )]
    #[case::trust_proxy_all("PALMR_TRUST_PROXY", "all", Constraint::TrustProxyAllMode)]
    #[case::trust_proxy_invalid_entry(
        "PALMR_TRUST_PROXY",
        "10.0.0.300",
        Constraint::TrustProxyEntry
    )]
    #[case::trust_proxy_empty_entry("PALMR_TRUST_PROXY", "10.0.0.1,", Constraint::TrustProxyEntry)]
    #[case::trust_proxy_off_in_list(
        "PALMR_TRUST_PROXY",
        "off,10.0.0.1",
        Constraint::TrustProxyEntry
    )]
    #[case::trust_proxy_hostname(
        "PALMR_TRUST_PROXY",
        "proxy.internal",
        Constraint::TrustProxyEntry
    )]
    #[case::log_format("PALMR_LOG_FORMAT", "xml", one_of(&["json", "pretty"]))]
    #[case::storage_provider("PALMR_STORAGE_PROVIDER", "minio", one_of(&["local", "s3"]))]
    #[case::db_read_connections_zero("PALMR_DB_READ_CONNECTIONS", "0", integer(1, 16))]
    #[case::db_read_connections_too_many("PALMR_DB_READ_CONNECTIONS", "17", integer(1, 16))]
    #[case::db_read_connections_not_numeric("PALMR_DB_READ_CONNECTIONS", "four", integer(1, 16))]
    #[case::db_synchronous_off("PALMR_DB_SYNCHRONOUS", "off", one_of(&["full", "normal"]))]
    #[case::db_synchronous_extra("PALMR_DB_SYNCHRONOUS", "extra", one_of(&["full", "normal"]))]
    #[case::zip_max_entries_zero("PALMR_ZIP_MAX_ENTRIES", "0", integer(1, 10_000_000))]
    #[case::zip_max_entries_too_many("PALMR_ZIP_MAX_ENTRIES", "10000001", integer(1, 10_000_000))]
    #[case::zip_max_entries_negative("PALMR_ZIP_MAX_ENTRIES", "-5", integer(1, 10_000_000))]
    #[case::zip_max_entries_fractional("PALMR_ZIP_MAX_ENTRIES", "1.5", integer(1, 10_000_000))]
    #[case::job_workers_zero("PALMR_JOB_WORKERS", "0", integer(1, 64))]
    #[case::shutdown_grace_zero("PALMR_SHUTDOWN_GRACE_SECS", "0", integer(1, 3_600))]
    #[case::upload_buffer_too_small(
        "PALMR_UPLOAD_BUFFER_BYTES",
        "1024",
        integer(4_096, 16_777_216)
    )]
    #[case::upload_buffer_too_large(
        "PALMR_UPLOAD_BUFFER_BYTES",
        "16777217",
        integer(4_096, 16_777_216)
    )]
    #[case::max_concurrent_transfers_zero("PALMR_MAX_CONCURRENT_TRANSFERS", "0", integer(1, 64))]
    #[case::orphan_reap_yes("PALMR_STORAGE_ORPHAN_REAP", "yes", Constraint::Boolean)]
    #[case::orphan_reap_numeric("PALMR_STORAGE_ORPHAN_REAP", "1", Constraint::Boolean)]
    fn unit_config_rejects_invalid_value(
        #[case] name: &'static str,
        #[case] value: &'static str,
        #[case] constraint: Constraint,
    ) {
        let error = load(&[(name, value)]).unwrap_err();

        assert_eq!(error.code(), STARTUP_CONFIG_INVALID);
        let [issue] = error.issues() else {
            panic!("expected exactly one issue, got {error}");
        };
        assert_eq!(issue.variable.name(), name);
        assert_eq!(issue.received, Received::Value(value.to_owned()));
        assert_eq!(issue.constraint, constraint);
        assert!(error.to_string().starts_with(STARTUP_CONFIG_INVALID));
        assert!(error.to_string().contains(name));
    }

    #[rstest]
    #[case::profile("PALMR_S3_PROFILE", "digitalocean", one_of(PROFILES))]
    #[case::endpoint_scheme("PALMR_S3_ENDPOINT", "minio:9000", Constraint::HttpScheme)]
    #[case::endpoint_path(
        "PALMR_S3_ENDPOINT",
        "http://minio:9000/palmr",
        Constraint::UrlWithoutPath
    )]
    #[case::endpoint_query(
        "PALMR_S3_ENDPOINT",
        "http://minio:9000/?region=x",
        Constraint::UrlWithoutQuery
    )]
    #[case::public_endpoint_malformed(
        "PALMR_S3_PUBLIC_ENDPOINT",
        "s3 example com",
        Constraint::AbsoluteUrl
    )]
    #[case::public_endpoint_fragment(
        "PALMR_S3_PUBLIC_ENDPOINT",
        "https://s3.example.com/#x",
        Constraint::UrlWithoutFragment
    )]
    #[case::force_path_style("PALMR_S3_FORCE_PATH_STYLE", "maybe", Constraint::Boolean)]
    #[case::reject_unauthorized("PALMR_S3_REJECT_UNAUTHORIZED", "0", Constraint::Boolean)]
    #[case::multipart_ttl_zero("PALMR_S3_MULTIPART_TTL_HOURS", "0", integer(1, 720))]
    #[case::multipart_ttl_too_long("PALMR_S3_MULTIPART_TTL_HOURS", "721", integer(1, 720))]
    fn unit_config_rejects_invalid_s3_value(
        #[case] name: &'static str,
        #[case] value: &'static str,
        #[case] constraint: Constraint,
    ) {
        let error = load_owned(with(COMPLETE_S3, &[(name, value)])).unwrap_err();

        let [issue] = error.issues() else {
            panic!("expected exactly one issue, got {error}");
        };
        assert_eq!(issue.variable.name(), name);
        assert_eq!(issue.received, Received::Value(value.to_owned()));
        assert_eq!(issue.constraint, constraint);
    }

    #[rstest]
    #[case::endpoint("PALMR_S3_ENDPOINT")]
    #[case::region("PALMR_S3_REGION")]
    #[case::bucket("PALMR_S3_BUCKET")]
    #[case::access_key("PALMR_S3_ACCESS_KEY")]
    #[case::secret_key("PALMR_S3_SECRET_KEY")]
    fn unit_config_rejects_incomplete_s3(#[case] missing: &'static str) {
        let vars: Vec<(&str, &str)> = COMPLETE_S3
            .iter()
            .copied()
            .filter(|(key, _)| *key != missing)
            .collect();
        let error = load(&vars).unwrap_err();

        let [issue] = error.issues() else {
            panic!("expected exactly one issue, got {error}");
        };
        assert_eq!(issue.variable.name(), missing);
        assert_eq!(issue.received, Received::NotSet);
        assert_eq!(issue.constraint, Constraint::RequiredForS3);
        assert!(error.to_string().contains(&format!(
            "{missing} is not set: required when PALMR_STORAGE_PROVIDER=s3"
        )));
    }

    #[test]
    fn unit_config_rejects_s3_provider_alone_naming_every_missing_variable() {
        let error = load(&[("PALMR_STORAGE_PROVIDER", "s3")]).unwrap_err();

        let missing: Vec<Variable> = error.issues().iter().map(|issue| issue.variable).collect();
        assert_eq!(
            missing,
            [
                Variable::S3Endpoint,
                Variable::S3Region,
                Variable::S3Bucket,
                Variable::S3AccessKey,
                Variable::S3SecretKey,
            ]
        );
        assert!(error
            .issues()
            .iter()
            .all(|issue| issue.constraint == Constraint::RequiredForS3));
    }

    #[test]
    fn unit_config_rejects_reports_every_invalid_variable() {
        let error = load(&[
            ("PALMR_PORT", "0"),
            ("PALMR_TRUST_PROXY", "all"),
            ("PALMR_DB_READ_CONNECTIONS", "32"),
            ("PALMR_ZIP_MAX_ENTRIES", "0"),
        ])
        .unwrap_err();

        let variables: Vec<Variable> = error.issues().iter().map(|issue| issue.variable).collect();
        assert_eq!(
            variables,
            [
                Variable::Port,
                Variable::TrustProxy,
                Variable::DbReadConnections,
                Variable::ZipMaxEntries,
            ]
        );
        assert_eq!(
            error.to_string(),
            "STARTUP_CONFIG_INVALID: invalid operator configuration; \
             PALMR_PORT=\"0\": must be an integer from 1 to 65535; \
             PALMR_TRUST_PROXY=\"all\": trusting every proxy is not supported; list the trusted proxy IP addresses or CIDR ranges explicitly; \
             PALMR_DB_READ_CONNECTIONS=\"32\": must be an integer from 1 to 16; \
             PALMR_ZIP_MAX_ENTRIES=\"0\": must be an integer from 1 to 10000000"
        );
    }

    #[test]
    fn unit_config_rejects_credentials_in_urls_without_echoing_them() {
        let error = load_owned(with(
            COMPLETE_S3,
            &[("PALMR_S3_ENDPOINT", "http://user:password@minio:9000")],
        ))
        .unwrap_err();

        let [issue] = error.issues() else {
            panic!("expected exactly one issue, got {error}");
        };
        assert_eq!(issue.variable, Variable::S3Endpoint);
        assert_eq!(issue.received, Received::Withheld);
        assert_eq!(issue.constraint, Constraint::UrlWithoutCredentials);
        assert!(!error.to_string().contains("password"));
    }

    #[rstest]
    #[case::all_lowercase("all")]
    #[case::all_uppercase("ALL")]
    #[case::all_padded("  all  ")]
    #[case::wildcard("*")]
    #[case::all_inside_list("10.0.0.1, all")]
    #[case::every_ipv4("0.0.0.0/0")]
    #[case::every_ipv6("::/0")]
    #[case::every_address_inside_list("10.0.0.0/8,::/0")]
    fn it_trust_proxy_has_no_all_mode(#[case] value: &str) {
        let error = load(&[("PALMR_TRUST_PROXY", value)]).unwrap_err();

        let [issue] = error.issues() else {
            panic!("expected exactly one issue, got {error}");
        };
        assert_eq!(issue.variable, Variable::TrustProxy);
        assert_eq!(issue.constraint, Constraint::TrustProxyAllMode);
    }

    #[rstest]
    #[case::unset(None, TrustProxy::Off)]
    #[case::off(Some("off"), TrustProxy::Off)]
    #[case::single_ipv4(Some("172.18.0.2"), TrustProxy::AllowList(vec!["172.18.0.2/32".parse().unwrap()]))]
    #[case::single_ipv6(Some("fd00::1"), TrustProxy::AllowList(vec!["fd00::1/128".parse().unwrap()]))]
    #[case::cidr_list(
        Some("10.0.0.0/8,fd00::/8"),
        TrustProxy::AllowList(vec!["10.0.0.0/8".parse().unwrap(), "fd00::/8".parse().unwrap()])
    )]
    fn unit_config_trust_proxy_accepts_off_or_explicit_list(
        #[case] value: Option<&str>,
        #[case] expected: TrustProxy,
    ) {
        let vars: Vec<(&str, &str)> = value
            .map(|v| ("PALMR_TRUST_PROXY", v))
            .into_iter()
            .collect();
        assert_eq!(load(&vars).unwrap().config.trust_proxy, expected);
    }

    #[rstest]
    #[case::aws_hostname("https://s3.us-east-1.amazonaws.com")]
    #[case::r2_hostname("https://0123456789abcdef.r2.cloudflarestorage.com")]
    #[case::b2_hostname("https://s3.us-west-004.backblazeb2.com")]
    #[case::gcs_hostname("https://storage.googleapis.com")]
    #[case::wasabi_hostname("https://s3.wasabisys.com")]
    #[case::minio_hostname("http://minio:9000")]
    fn it_storage_profile_is_explicit(#[case] endpoint: &'static str) {
        let omitted = load_owned(with(
            COMPLETE_S3,
            &[
                ("PALMR_S3_ENDPOINT", endpoint),
                ("PALMR_S3_PUBLIC_ENDPOINT", endpoint),
            ],
        ))
        .unwrap();
        let StorageConfig::S3(s3) = omitted.config.storage else {
            panic!("expected S3 storage");
        };
        assert_eq!(s3.profile, S3Profile::Generic);

        let explicit = load_owned(with(
            COMPLETE_S3,
            &[
                ("PALMR_S3_ENDPOINT", endpoint),
                ("PALMR_S3_PROFILE", "garage"),
            ],
        ))
        .unwrap();
        let StorageConfig::S3(s3) = explicit.config.storage else {
            panic!("expected S3 storage");
        };
        assert_eq!(s3.profile, S3Profile::Garage);
    }

    #[rstest]
    #[case::aws_host_generic("https://s3.us-east-1.amazonaws.com", S3Profile::Generic)]
    #[case::aws_host_minio("https://s3.us-east-1.amazonaws.com", S3Profile::Minio)]
    #[case::minio_host_generic("http://minio:9000", S3Profile::Generic)]
    #[case::minio_host_aws("http://minio:9000", S3Profile::Aws)]
    fn it_storage_profile_ignores_endpoint_hostname(
        #[case] endpoint: &'static str,
        #[case] expected: S3Profile,
    ) {
        let name = match expected {
            S3Profile::Generic => "generic",
            S3Profile::Aws => "aws",
            S3Profile::Minio => "minio",
            S3Profile::R2 => "r2",
            S3Profile::Rustfs => "rustfs",
            S3Profile::B2 => "b2",
            S3Profile::Gcs => "gcs",
            S3Profile::Wasabi => "wasabi",
            S3Profile::Garage => "garage",
        };
        let loaded = load_owned(with(
            COMPLETE_S3,
            &[("PALMR_S3_ENDPOINT", endpoint), ("PALMR_S3_PROFILE", name)],
        ))
        .unwrap();
        let StorageConfig::S3(s3) = loaded.config.storage else {
            panic!("expected S3 storage");
        };
        assert_eq!(s3.profile, expected);
    }

    #[rstest]
    #[case::generic("generic", S3Profile::Generic)]
    #[case::aws("aws", S3Profile::Aws)]
    #[case::minio("minio", S3Profile::Minio)]
    #[case::r2("r2", S3Profile::R2)]
    #[case::rustfs("rustfs", S3Profile::Rustfs)]
    #[case::b2("b2", S3Profile::B2)]
    #[case::gcs("gcs", S3Profile::Gcs)]
    #[case::wasabi("wasabi", S3Profile::Wasabi)]
    #[case::garage("garage", S3Profile::Garage)]
    fn unit_config_accepts_every_documented_s3_profile(
        #[case] value: &'static str,
        #[case] expected: S3Profile,
    ) {
        let config = load_owned(with(COMPLETE_S3, &[("PALMR_S3_PROFILE", value)]))
            .unwrap()
            .config;
        let StorageConfig::S3(s3) = config.storage else {
            panic!("expected S3 storage");
        };
        assert_eq!(s3.profile, expected);
    }

    #[rstest]
    #[case::https_host("https://files.example.com", Ok("https://files.example.com/"))]
    #[case::http_ip_port("http://192.168.1.20:5487", Ok("http://192.168.1.20:5487/"))]
    #[case::sub_path("https://example.com/palmr/", Ok("https://example.com/palmr/"))]
    #[case::ipv6_host("http://[fd00::10]:8080", Ok("http://[fd00::10]:8080/"))]
    #[case::uppercase_scheme("HTTPS://Files.Example.com", Ok("https://files.example.com/"))]
    #[case::malformed("https://exa mple.com", Err(Constraint::AbsoluteUrl))]
    #[case::no_scheme("files.example.com", Err(Constraint::AbsoluteUrl))]
    #[case::unsupported_scheme("ws://files.example.com", Err(Constraint::HttpScheme))]
    #[case::file_scheme("file:///srv/palmr", Err(Constraint::HttpScheme))]
    #[case::no_host("http://", Err(Constraint::UrlHost))]
    #[case::query("https://files.example.com/?next=/", Err(Constraint::UrlWithoutQuery))]
    #[case::empty_query("https://files.example.com/?", Err(Constraint::UrlWithoutQuery))]
    #[case::fragment(
        "https://files.example.com/#login",
        Err(Constraint::UrlWithoutFragment)
    )]
    #[case::credentials(
        "https://admin:hunter2@files.example.com",
        Err(Constraint::UrlWithoutCredentials)
    )]
    fn it_base_url_validation(
        #[case] value: &'static str,
        #[case] expected: Result<&'static str, Constraint>,
    ) {
        let loaded = load(&[("PALMR_BASE_URL", value)]);
        match expected {
            Ok(url) => {
                let config = loaded.unwrap().config;
                assert_eq!(config.base_url.url().as_str(), url);
                assert_eq!(
                    config.base_url.source(),
                    crate::config::PublicBaseUrlSource::Configured
                );
            }
            Err(constraint) => {
                let error = loaded.unwrap_err();
                let [issue] = error.issues() else {
                    panic!("expected exactly one issue, got {error}");
                };
                assert_eq!(issue.variable, Variable::BaseUrl);
                assert_eq!(issue.constraint, constraint);
                assert!(!error.to_string().contains("hunter2"));
            }
        }
    }
}

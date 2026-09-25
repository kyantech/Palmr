use std::path::PathBuf;
use std::sync::Arc;

use aws_sdk_s3::config::RequestChecksumCalculation;
use url::Url;

use super::client::S3Clients;
use super::config::{
    effective_public_endpoint, public_origin, ResolvedS3Config, S3SetupError,
    STORAGE_CONFIG_INVALID,
};
use super::profile::{ProfileLimits, ProviderProfile, GIB, MIB, TIB};
use super::tls::S3TlsPolicy;
use crate::config::{
    ConfigError, EnvironmentSource, OperatorConfig, S3Config, S3Profile, StorageConfig, Variable,
};
use crate::storage::provider::StorageDescriptor;
use crate::storage::ProviderKind;

const ACCESS_KEY: &str = "AKIA-palmr-access-sentinel";
const SECRET_KEY: &str = "palmr-secret-sentinel-9f2c";
const DEFAULT_ENDPOINT: &str = "http://minio:9000";

const BASE: [(&str, &str); 6] = [
    ("PALMR_STORAGE_PROVIDER", "s3"),
    ("PALMR_S3_ENDPOINT", DEFAULT_ENDPOINT),
    ("PALMR_S3_REGION", "us-east-1"),
    ("PALMR_S3_BUCKET", "palmr"),
    ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
    ("PALMR_S3_SECRET_KEY", SECRET_KEY),
];

const CA_BUNDLE: &str = "-----BEGIN CERTIFICATE-----
MIIBRDCB96ADAgECAhQz0eKctTKca7+nPAzUhCIDYQyoGDAFBgMrZXAwGDEWMBQG
A1UEAwwNcGFsbXItdGVzdC1jYTAeFw0yNjA5MjQyMzUyNDNaFw0zNjA5MjEyMzUy
NDNaMBgxFjAUBgNVBAMMDXBhbG1yLXRlc3QtY2EwKjAFBgMrZXADIQCPc72qCB70
AFrRSNRwIZdfOV2ue2XmEEFogcMFjCdoj6NTMFEwHQYDVR0OBBYEFIdQdcabj3gV
Lk0oaqgrb2aZEEh/MB8GA1UdIwQYMBaAFIdQdcabj3gVLk0oaqgrb2aZEEh/MA8G
A1UdEwEB/wQFMAMBAf8wBQYDK2VwA0EAwpT190ku6Luh3UVQ5QPoPVi43y+ewNuk
MyCoAclNnu6Sbrw+ceMBAm6w5JEvBuJF6QarLoa9jiXJjS9kWI/KDg==
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
MIIBSDCB+6ADAgECAhRSSt22mxMrwIE/HQY5kew9KCCBkDAFBgMrZXAwGjEYMBYG
A1UEAwwPcGFsbXItdGVzdC1jYS0yMB4XDTI2MDkyNDIzNTI0N1oXDTM2MDkyMTIz
NTI0N1owGjEYMBYGA1UEAwwPcGFsbXItdGVzdC1jYS0yMCowBQYDK2VwAyEAXBZA
u9LbcTfnx8NV3wuF2O2VvJQ6ebloI1K7YN7wzrOjUzBRMB0GA1UdDgQWBBSDO7Yn
Ic7DLXhrfY4DQc4rgpgBrDAfBgNVHSMEGDAWgBSDO7YnIc7DLXhrfY4DQc4rgpgB
rDAPBgNVHRMBAf8EBTADAQH/MAUGAytlcANBAKGNz9xy7L5L1r53gMtgYDlsfm7D
ASSwRaTy85BElrWP5MMkhfN45As8hddywEJZ/jUiGUnJJrG5peen1lg11QU=
-----END CERTIFICATE-----
";

const CA_CERT_COUNT: usize = 2;

const PRODUCTION_SOURCES: [&str; 5] = [
    include_str!("mod.rs"),
    include_str!("profile.rs"),
    include_str!("config.rs"),
    include_str!("client.rs"),
    include_str!("tls.rs"),
];

fn vars(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = BASE
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

fn load(extra: &[(&str, &str)]) -> Result<OperatorConfig, ConfigError> {
    OperatorConfig::load(&EnvironmentSource::from_vars(vars(extra))).map(|loaded| loaded.config)
}

fn storage(extra: &[(&str, &str)]) -> StorageConfig {
    load(extra).unwrap().storage
}

fn s3_config(extra: &[(&str, &str)]) -> S3Config {
    match storage(extra) {
        StorageConfig::S3(s3) => *s3,
        StorageConfig::Local => panic!("expected s3 storage"),
    }
}

fn clients(extra: &[(&str, &str)]) -> S3Clients {
    S3Clients::build(&storage(extra)).unwrap().unwrap()
}

#[test]
fn unit_profile_table_constants() {
    let baseline = ProfileLimits::BASELINE;
    assert_eq!(baseline.min_part, 5 * MIB);
    assert_eq!(baseline.max_part, 5 * GIB);
    assert_eq!(baseline.max_parts, 10_000);
    assert_eq!(baseline.max_object, 5 * TIB);
    assert_eq!(baseline.safety_margin, 100);
    assert_eq!(baseline.usable_parts(), 9_900);
    assert!(!baseline.requires_part_checksums);
    assert!(baseline.allows_single_small_part);
    assert!(baseline.supports_presigned_get);
    assert!(baseline.supports_server_side_copy);

    let names: Vec<&str> = ProviderProfile::ALL
        .iter()
        .map(|profile| profile.as_str())
        .collect();
    assert_eq!(
        names,
        ["generic", "aws", "minio", "r2", "rustfs", "b2", "gcs", "wasabi", "garage"]
    );

    for profile in ProviderProfile::ALL {
        assert_eq!(
            ProviderProfile::from_config(profile.config_profile()),
            profile
        );
        let limits = profile.limits();
        assert_eq!(limits.min_part, 5 * MIB);
        assert_eq!(limits.max_part, 5 * GIB);
        assert_eq!(limits.max_parts, 10_000);
        assert_eq!(limits.max_object, 5 * TIB);
        assert_eq!(limits.safety_margin, 100);
        assert!(limits.supports_server_side_copy);
    }

    assert!(ProviderProfile::R2.limits().requires_part_checksums);
    assert!(!ProviderProfile::Aws.limits().requires_part_checksums);
    assert!(!ProviderProfile::Generic.limits().requires_part_checksums);
}

#[test]
fn it_storage_profile_is_explicit() {
    let aws_endpoint = "https://s3.us-east-1.amazonaws.com";
    let minio_endpoint = "http://minio.internal:9000";

    for endpoint in [aws_endpoint, minio_endpoint] {
        let omitted = clients(&[
            ("PALMR_S3_ENDPOINT", endpoint),
            ("PALMR_S3_PUBLIC_ENDPOINT", endpoint),
        ]);
        assert_eq!(omitted.shared().profile(), ProviderProfile::Generic);
    }

    let aws_looking_generic = clients(&[("PALMR_S3_ENDPOINT", aws_endpoint)]);
    assert_eq!(
        aws_looking_generic.shared().profile(),
        ProviderProfile::Generic
    );

    let minio_looking_aws = clients(&[
        ("PALMR_S3_ENDPOINT", minio_endpoint),
        ("PALMR_S3_PROFILE", "aws"),
    ]);
    assert_eq!(minio_looking_aws.shared().profile(), ProviderProfile::Aws);

    let aws_looking_minio = clients(&[
        ("PALMR_S3_ENDPOINT", aws_endpoint),
        ("PALMR_S3_PROFILE", "minio"),
    ]);
    assert_eq!(aws_looking_minio.shared().profile(), ProviderProfile::Minio);

    for (name, expected) in [
        ("generic", ProviderProfile::Generic),
        ("aws", ProviderProfile::Aws),
        ("minio", ProviderProfile::Minio),
        ("r2", ProviderProfile::R2),
        ("rustfs", ProviderProfile::Rustfs),
        ("b2", ProviderProfile::B2),
        ("gcs", ProviderProfile::Gcs),
        ("wasabi", ProviderProfile::Wasabi),
        ("garage", ProviderProfile::Garage),
    ] {
        assert_eq!(
            clients(&[("PALMR_S3_PROFILE", name)]).shared().profile(),
            expected,
            "{name}"
        );
    }

    let error = load(&[("PALMR_S3_PROFILE", "digitalocean")]).unwrap_err();
    assert_eq!(error.issues()[0].variable, Variable::S3Profile);
}

#[test]
fn unit_public_endpoint_fallback() {
    let fallback = clients(&[]);
    assert_eq!(
        fallback.internal_client().endpoint().as_str(),
        "http://minio:9000/"
    );
    assert_eq!(
        fallback.public_signer().endpoint(),
        fallback.internal_client().endpoint()
    );
    assert!(Arc::ptr_eq(
        fallback.internal_client().shared(),
        fallback.public_signer().shared()
    ));
    assert_eq!(
        fallback.internal_client().shared().region(),
        fallback.public_signer().shared().region()
    );
    assert_eq!(
        fallback.internal_client().shared().bucket(),
        fallback.public_signer().shared().bucket()
    );
    assert_eq!(
        fallback.internal_client().shared().force_path_style(),
        fallback.public_signer().shared().force_path_style()
    );
    assert_eq!(
        fallback.internal_tls().policy(),
        fallback.public_tls().policy()
    );
    assert_eq!(
        fallback.public_origin().as_deref(),
        Some("http://minio:9000")
    );
    assert_eq!(
        fallback.internal_client().shared().tls_policy(),
        S3TlsPolicy {
            verify_certificates: true,
            additional_ca: false
        }
    );
    assert_eq!(
        fallback.internal_client().shared().multipart_ttl(),
        std::time::Duration::from_secs(24 * 3_600)
    );
    assert_eq!(
        fallback.base_config().request_checksum_calculation(),
        Some(&RequestChecksumCalculation::WhenRequired)
    );

    let split = clients(&[("PALMR_S3_PUBLIC_ENDPOINT", "https://s3.example.com")]);
    assert_eq!(
        split.internal_client().endpoint().as_str(),
        "http://minio:9000/"
    );
    assert_eq!(
        split.public_signer().endpoint().as_str(),
        "https://s3.example.com/"
    );
    assert_ne!(
        split.internal_client().endpoint(),
        split.public_signer().endpoint()
    );
    assert_eq!(
        split.public_origin().as_deref(),
        Some("https://s3.example.com")
    );
    assert!(Arc::ptr_eq(
        split.internal_client().shared(),
        split.public_signer().shared()
    ));
    assert_eq!(split.internal_tls().policy(), split.public_tls().policy());

    let signer = split.public_signer();
    assert!(signer
        .presigning_config(std::time::Duration::from_secs(900))
        .is_ok());
    assert!(matches!(
        signer.presigning_config(std::time::Duration::from_secs(30 * 24 * 3_600)),
        Err(S3SetupError::PresigningConfiguration)
    ));
}

#[test]
fn unit_public_origin_is_scheme_host_port_only() {
    for (url, expected) in [
        (
            "https://files.example.com/some/prefix?X-Amz-Signature=abc#part",
            "https://files.example.com",
        ),
        (
            "https://key:secret@files.example.com/",
            "https://files.example.com",
        ),
        (
            "https://files.example.com:9443/bucket",
            "https://files.example.com:9443",
        ),
        ("http://[::1]:9000/", "http://[::1]:9000"),
    ] {
        let parsed = Url::parse(url).unwrap();
        let origin = public_origin(&parsed).unwrap();
        assert_eq!(origin, expected);
        assert!(!origin.contains('?'));
        assert!(!origin.contains('@'));
        assert_eq!(origin.matches('/').count(), 2, "{origin}");
    }
    assert_eq!(
        public_origin(&Url::parse("data:text/plain,hi").unwrap()),
        None
    );
}

#[test]
#[allow(non_snake_case)]
fn regression_R052_s3_tls_options_scoped_to_client() {
    let provider_before = rustls::crypto::CryptoProvider::get_default()
        .map(|provider| Arc::as_ptr(provider) as usize);

    let strict = clients(&[]);
    let default_roots = webpki_roots::TLS_SERVER_ROOTS.len();
    assert!(strict.internal_tls().verifies_certificates());
    assert!(strict.public_tls().verifies_certificates());
    assert_eq!(strict.internal_tls().root_count(), default_roots);
    assert_eq!(strict.public_tls().root_count(), default_roots);
    assert!(!strict.internal_tls().policy().additional_ca);
    assert!(!Arc::ptr_eq(
        strict.internal_tls().config(),
        strict.public_tls().config()
    ));

    let ca = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ca.path(), CA_BUNDLE).unwrap();
    let with_ca = clients(&[("PALMR_S3_CA_FILE", ca.path().to_str().unwrap())]);
    assert!(with_ca.internal_tls().verifies_certificates());
    assert!(with_ca.internal_tls().policy().additional_ca);
    assert_eq!(
        with_ca.internal_tls().root_count(),
        default_roots + CA_CERT_COUNT
    );
    assert_eq!(
        with_ca.public_tls().root_count(),
        default_roots + CA_CERT_COUNT
    );
    assert_eq!(
        with_ca.internal_tls().policy(),
        S3TlsPolicy {
            verify_certificates: true,
            additional_ca: true
        }
    );

    let insecure = clients(&[("PALMR_S3_REJECT_UNAUTHORIZED", "false")]);
    assert!(!insecure.internal_tls().verifies_certificates());
    assert!(!insecure.public_tls().verifies_certificates());
    assert_eq!(
        insecure.internal_tls().policy(),
        insecure.public_tls().policy()
    );
    assert_eq!(insecure.internal_tls().root_count(), default_roots);
    assert!(strict.internal_tls().verifies_certificates());

    let provider_after = rustls::crypto::CryptoProvider::get_default()
        .map(|provider| Arc::as_ptr(provider) as usize);
    assert_eq!(provider_before, provider_after);

    for source in [include_str!("tls.rs"), include_str!("client.rs")] {
        for forbidden in [
            "install_default",
            "set_default",
            "lettre",
            "TlsParameters",
            "oauth",
            "oidc",
            "dangerous_accept_invalid",
        ] {
            assert!(!source.contains(forbidden), "{forbidden}");
        }
    }
}

#[test]
fn it_s3_ca_file_failure_is_config_error() {
    let missing = PathBuf::from("/nonexistent/palmr/ca.pem");
    let error =
        S3Clients::build(&storage(&[("PALMR_S3_CA_FILE", missing.to_str().unwrap())])).unwrap_err();
    assert!(matches!(error, S3SetupError::CaFileUnreadable { .. }));
    assert_eq!(error.code(), STORAGE_CONFIG_INVALID);
    assert!(error.to_string().contains("STORAGE_CONFIG_INVALID"));
    assert!(!error.to_string().contains(SECRET_KEY));

    let empty = tempfile::NamedTempFile::new().unwrap();
    let error = S3Clients::build(&storage(&[(
        "PALMR_S3_CA_FILE",
        empty.path().to_str().unwrap(),
    )]))
    .unwrap_err();
    assert!(matches!(error, S3SetupError::CaFileEmpty { .. }));

    let malformed = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        malformed.path(),
        b"-----BEGIN CERTIFICATE-----\nnot-base64\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let error = S3Clients::build(&storage(&[(
        "PALMR_S3_CA_FILE",
        malformed.path().to_str().unwrap(),
    )]))
    .unwrap_err();
    assert!(matches!(
        error,
        S3SetupError::CaFileMalformed { .. } | S3SetupError::CaFileEmpty { .. }
    ));
}

#[test]
fn unit_presign_client_cannot_execute() {
    const OPERATIONS: [&str; 12] = [
        "head_object",
        "get_object",
        "put_object",
        "delete_object",
        "copy_object",
        "list_objects_v2",
        "create_multipart_upload",
        "complete_multipart_upload",
        "abort_multipart_upload",
        "upload_part",
        "list_parts",
        "list_multipart_uploads",
    ];

    for source in PRODUCTION_SOURCES {
        for operation in OPERATIONS {
            assert!(!source.contains(operation), "{operation}");
        }
    }

    let production = include_str!("client.rs");
    assert!(production.contains("client: aws_sdk_s3::Client,"));
    assert!(!production.contains("impl Deref for PublicSigner"));
    assert!(!production.contains("AsRef<aws_sdk_s3::Client>"));

    let signer_impl = production
        .split("impl PublicSigner {")
        .nth(1)
        .unwrap()
        .split("impl fmt::Debug for PublicSigner")
        .next()
        .unwrap();
    let mut methods: Vec<&str> = signer_impl
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub fn ")
                .map(|rest| rest.split('(').next().unwrap())
        })
        .collect();
    methods.sort_unstable();
    assert_eq!(methods, ["endpoint", "presigning_config"]);
    for line in signer_impl.lines() {
        let line = line.trim();
        if line.starts_with("pub fn ") {
            assert!(
                !line.contains("aws_sdk_s3::Client"),
                "public raw client accessor: {line}"
            );
        }
    }

    let clients = clients(&[]);
    assert!(!std::ptr::eq(
        clients.public_signer().signing_client(),
        clients.internal_client().client()
    ));
}

#[test]
fn regression_s3_credentials_are_redacted() {
    let clients = clients(&[]);
    let rendered = [
        format!("{clients:?}"),
        format!("{:?}", clients.shared()),
        format!("{:?}", clients.internal_client()),
        format!("{:?}", clients.public_signer()),
        format!("{:?}", ResolvedS3Config::from_config(&s3_config(&[]))),
    ];
    for text in &rendered {
        assert!(!text.contains(ACCESS_KEY), "{text}");
        assert!(!text.contains(SECRET_KEY), "{text}");
    }
    assert!(rendered[1].contains("<redacted>"));
    assert_eq!(format!("{:?}", clients.shared().secret_key()), "<redacted>");
    assert_eq!(clients.shared().access_key().to_string(), "<redacted>");
    assert_eq!(clients.shared().secret_key().expose_secret(), SECRET_KEY);

    let descriptor = StorageDescriptor {
        provider: ProviderKind::S3,
        local: None,
    };
    let debug = format!("{descriptor:?}");
    assert!(!debug.contains(ACCESS_KEY));
    assert!(!debug.contains(SECRET_KEY));
}

#[test]
fn regression_no_default_aws_credential_chain() {
    for source in PRODUCTION_SOURCES {
        for forbidden in [
            "aws_config",
            "aws-config",
            "DefaultCredentialsChain",
            "InstanceMetadata",
            "imds",
            "ProfileFile",
            "credential_process",
            "web_identity",
            "shared_credentials",
            "AWS_PROFILE",
            "from_env",
        ] {
            assert!(!source.contains(forbidden), "{forbidden}");
        }
    }

    let manifest = include_str!("../../../Cargo.toml");
    assert!(manifest.contains("aws-sdk-s3"));
    assert!(!manifest.contains("aws-config"));
    assert!(manifest.contains("default-features = false"));
}

#[test]
fn regression_force_path_style_is_operator_configuration_only() {
    let virtual_hosted = clients(&[
        ("PALMR_S3_ENDPOINT", "https://s3.us-east-1.amazonaws.com"),
        ("PALMR_S3_FORCE_PATH_STYLE", "false"),
    ]);
    assert!(!virtual_hosted.shared().force_path_style());

    let path_style = clients(&[("PALMR_S3_ENDPOINT", "http://minio:9000")]);
    assert!(path_style.shared().force_path_style());

    let aws_profile_default_style = clients(&[
        ("PALMR_S3_ENDPOINT", "https://s3.us-east-1.amazonaws.com"),
        ("PALMR_S3_PROFILE", "aws"),
    ]);
    assert!(aws_profile_default_style.shared().force_path_style());

    let aws_profile_virtual_hosted = clients(&[
        ("PALMR_S3_ENDPOINT", "https://s3.us-east-1.amazonaws.com"),
        ("PALMR_S3_PROFILE", "aws"),
        ("PALMR_S3_FORCE_PATH_STYLE", "false"),
    ]);
    assert!(!aws_profile_virtual_hosted.shared().force_path_style());

    for source in PRODUCTION_SOURCES {
        for forbidden in [
            "ends_with",
            "starts_with",
            "host_str",
            "amazonaws",
            "cloudflarestorage",
            "backblazeb2",
            "wasabisys",
            "storage.googleapis",
            "domain()",
        ] {
            assert!(!source.contains(forbidden), "{forbidden}");
        }
    }
    assert!(!include_str!("client.rs").contains("public_endpoint.is_some"));
}

#[test]
fn regression_s3_clients_are_constructed_once() {
    let production = include_str!("client.rs");
    assert_eq!(
        production.matches("aws_sdk_s3::Client::from_conf").count(),
        1
    );
    assert_eq!(production.matches("client_for(").count(), 3);

    let signer_impl = production
        .split("impl PublicSigner {")
        .nth(1)
        .unwrap()
        .split("impl fmt::Debug for PublicSigner")
        .next()
        .unwrap();
    assert!(!signer_impl.contains("from_conf"));
    assert!(!signer_impl.contains("Client::"));
}

#[test]
fn unit_local_storage_builds_no_clients() {
    assert!(S3Clients::build(&StorageConfig::Local).unwrap().is_none());
    assert_eq!(effective_public_endpoint(&StorageConfig::Local), None);
    let configured = storage(&[("PALMR_S3_PUBLIC_ENDPOINT", "https://s3.example.com")]);
    assert_eq!(
        effective_public_endpoint(&configured).map(Url::as_str),
        Some("https://s3.example.com/")
    );
    assert_eq!(
        S3Profile::Generic,
        ProviderProfile::Generic.config_profile()
    );
}

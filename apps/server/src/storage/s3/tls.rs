use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

use super::config::S3SetupError;

pub const CA_FILE_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S3TlsPolicy {
    pub verify_certificates: bool,
    pub additional_ca: bool,
}

#[derive(Clone)]
pub struct S3TlsConfig {
    config: Arc<rustls::ClientConfig>,
    policy: S3TlsPolicy,
    root_count: usize,
}

impl S3TlsConfig {
    pub fn config(&self) -> &Arc<rustls::ClientConfig> {
        &self.config
    }

    pub const fn policy(&self) -> S3TlsPolicy {
        self.policy
    }

    pub const fn root_count(&self) -> usize {
        self.root_count
    }

    pub const fn verifies_certificates(&self) -> bool {
        self.policy.verify_certificates
    }
}

impl fmt::Debug for S3TlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3TlsConfig")
            .field("policy", &self.policy)
            .field("root_count", &self.root_count)
            .finish()
    }
}

pub fn build(
    ca_file: Option<&Path>,
    verify_certificates: bool,
) -> Result<S3TlsConfig, S3SetupError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|_| S3SetupError::TlsConfiguration)?;

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Some(path) = ca_file {
        let bundle = read_ca_file(path)?;
        let mut added = 0_usize;
        for certificate in CertificateDer::pem_slice_iter(&bundle) {
            let certificate = certificate.map_err(|_| S3SetupError::CaFileMalformed {
                path: path.to_path_buf(),
            })?;
            roots
                .add(certificate)
                .map_err(|_| S3SetupError::CaFileMalformed {
                    path: path.to_path_buf(),
                })?;
            added += 1;
        }
        if added == 0 {
            return Err(S3SetupError::CaFileEmpty {
                path: path.to_path_buf(),
            });
        }
    }

    let root_count = roots.len();
    let config = if verify_certificates {
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification { provider }))
            .with_no_client_auth()
    };

    Ok(S3TlsConfig {
        config: Arc::new(config),
        policy: S3TlsPolicy {
            verify_certificates,
            additional_ca: ca_file.is_some(),
        },
        root_count,
    })
}

fn read_ca_file(path: &Path) -> Result<Vec<u8>, S3SetupError> {
    let mut bundle = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(CA_FILE_MAX_BYTES).read_to_end(&mut bundle))
        .map_err(|_| S3SetupError::CaFileUnreadable {
            path: path.to_path_buf(),
        })?;
    Ok(bundle)
}

#[derive(Debug)]
struct NoCertificateVerification {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _certificate: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _certificate: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

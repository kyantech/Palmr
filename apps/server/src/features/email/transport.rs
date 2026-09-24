use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{CertificateStore, Tls, TlsParameters, TlsParametersBuilder};
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::error::{
    EMAIL_CONFIG_INVALID, EMAIL_DELIVERY_FAILED, EMAIL_NOT_CONFIGURED, EMAIL_REJECTED,
};
use crate::domain::email::Email;
use crate::domain::secret::Secret;
use crate::features::settings::model::{SmtpSecurity, SmtpSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    NotConfigured,
    InvalidConfiguration,
    Delivery,
    Rejected,
}

impl TransportError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotConfigured => EMAIL_NOT_CONFIGURED,
            Self::InvalidConfiguration => EMAIL_CONFIG_INVALID,
            Self::Delivery => EMAIL_DELIVERY_FAILED,
            Self::Rejected => EMAIL_REJECTED,
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotConfigured => "outbound e-mail is not configured",
            Self::InvalidConfiguration => "outbound e-mail configuration is invalid",
            Self::Delivery => "the SMTP server could not be reached",
            Self::Rejected => "the SMTP server rejected the message",
        })
    }
}

impl std::error::Error for TransportError {}

#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password: Option<Secret<String>>,
    pub from_name: Option<String>,
    pub from_email: Email,
    pub allow_self_signed_certificate: bool,
    pub no_auth: bool,
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .field("password", &self.password)
            .field("from_name", &self.from_name)
            .field("from_email", &self.from_email)
            .field(
                "allow_self_signed_certificate",
                &self.allow_self_signed_certificate,
            )
            .field("no_auth", &self.no_auth)
            .finish()
    }
}

impl SmtpConfig {
    pub fn from_settings(settings: &SmtpSettings) -> Result<Self, TransportError> {
        if !settings.enabled {
            return Err(TransportError::NotConfigured);
        }
        let host = settings
            .host
            .as_deref()
            .filter(|host| !host.trim().is_empty())
            .ok_or(TransportError::InvalidConfiguration)?
            .to_owned();
        let from_email = settings
            .from_email
            .as_deref()
            .ok_or(TransportError::InvalidConfiguration)
            .and_then(|address| {
                Email::parse(address).map_err(|_| TransportError::InvalidConfiguration)
            })?;
        let credentials = if settings.no_auth {
            None
        } else {
            let username = settings
                .username
                .as_deref()
                .filter(|username| !username.is_empty())
                .ok_or(TransportError::InvalidConfiguration)?;
            let password = settings
                .password
                .as_ref()
                .ok_or(TransportError::InvalidConfiguration)?;
            Some((username.to_owned(), password.clone()))
        };
        Ok(Self {
            host,
            port: settings.port,
            security: settings.security,
            username: credentials.as_ref().map(|(username, _)| username.clone()),
            password: credentials.map(|(_, password)| password),
            from_name: settings.from_name.clone(),
            from_email,
            allow_self_signed_certificate: settings.allow_self_signed_certificate,
            no_auth: settings.no_auth,
        })
    }
}

#[derive(Debug, Clone)]
pub struct OutboundMailbox {
    pub name: Option<String>,
    pub email: Email,
}

#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub from: OutboundMailbox,
    pub to: OutboundMailbox,
    pub subject: String,
    pub html: String,
    pub text: String,
}

pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;

pub trait EmailTransport: Send + Sync + 'static {
    fn send<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
    ) -> TransportFuture<'a>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SmtpTransport;

impl EmailTransport for SmtpTransport {
    fn send<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
    ) -> TransportFuture<'a> {
        Box::pin(async move {
            let email = build_message(config, message)?;
            let client = build_client(config)?;
            client
                .send(email)
                .await
                .map(drop)
                .map_err(|_| TransportError::Delivery)
        })
    }
}

fn mailbox(name: Option<String>, email: &Email) -> Result<Mailbox, TransportError> {
    let address: Address = email
        .as_str()
        .parse()
        .map_err(|_| TransportError::Rejected)?;
    Ok(Mailbox::new(name, address))
}

fn build_message(
    config: &SmtpConfig,
    message: &OutboundMessage,
) -> Result<Message, TransportError> {
    let from = mailbox(config.from_name.clone(), &config.from_email)?;
    let to = mailbox(message.to.name.clone(), &message.to.email)?;
    Message::builder()
        .from(from)
        .to(to)
        .subject(message.subject.clone())
        .multipart(MultiPart::alternative_plain_html(
            message.text.clone(),
            message.html.clone(),
        ))
        .map_err(|_| TransportError::InvalidConfiguration)
}

fn tls_parameters(config: &SmtpConfig) -> Result<TlsParameters, TransportError> {
    let builder =
        TlsParametersBuilder::new(config.host.clone()).certificate_store(CertificateStore::Default);
    let builder = if config.allow_self_signed_certificate {
        builder
            .dangerous_accept_invalid_certs(true)
            .dangerous_accept_invalid_hostnames(true)
    } else {
        builder
    };
    builder
        .build_rustls()
        .map_err(|_| TransportError::InvalidConfiguration)
}

fn build_client(config: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, TransportError> {
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(config.host.clone())
        .port(config.port);
    builder = match config.security {
        SmtpSecurity::Starttls => builder.tls(Tls::Required(tls_parameters(config)?)),
        SmtpSecurity::Implicit => builder.tls(Tls::Wrapper(tls_parameters(config)?)),
        SmtpSecurity::None => builder.tls(Tls::None),
    };
    if !config.no_auth {
        if let (Some(username), Some(password)) = (&config.username, &config.password) {
            builder = builder.credentials(Credentials::new(
                username.clone(),
                password.expose_secret().clone(),
            ));
        }
    }
    Ok(builder.build())
}

#[derive(Debug, Clone)]
pub struct CapturedMessage {
    pub config: SmtpConfig,
    pub message: OutboundMessage,
}

#[derive(Debug, Default)]
pub struct CapturingTransport {
    sent: Mutex<Vec<CapturedMessage>>,
}

impl CapturingTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn captured(&self) -> Vec<CapturedMessage> {
        self.lock().clone()
    }

    pub fn count(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<CapturedMessage>> {
        match self.sent.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl EmailTransport for CapturingTransport {
    fn send<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
    ) -> TransportFuture<'a> {
        let captured = CapturedMessage {
            config: config.clone(),
            message: message.clone(),
        };
        self.lock().push(captured);
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::secret::Secret;
    use crate::features::settings::model::SmtpSettings;

    const PASSWORD: &str = "palmr-smtp-password-sentinel-4b1c";

    fn settings(enabled: bool, security: SmtpSecurity) -> SmtpSettings {
        SmtpSettings {
            enabled,
            host: Some("smtp.example.test".to_owned()),
            port: 587,
            security,
            username: Some("palmr".to_owned()),
            password: Some(Secret::new(PASSWORD.to_owned())),
            from_name: Some("Palmr".to_owned()),
            from_email: Some("palmr@example.test".to_owned()),
            allow_self_signed_certificate: false,
            no_auth: false,
        }
    }

    fn message() -> OutboundMessage {
        OutboundMessage {
            from: OutboundMailbox {
                name: Some("Palmr".to_owned()),
                email: Email::parse("palmr@example.test").unwrap(),
            },
            to: OutboundMailbox {
                name: Some("Ada".to_owned()),
                email: Email::parse("ada@example.test").unwrap(),
            },
            subject: "Subject".to_owned(),
            html: "<p>html</p>".to_owned(),
            text: "text".to_owned(),
        }
    }

    #[test]
    fn unit_smtp_config_requires_enabled_settings() {
        assert_eq!(
            SmtpConfig::from_settings(&settings(false, SmtpSecurity::Starttls)).unwrap_err(),
            TransportError::NotConfigured
        );
        let mut no_host = settings(true, SmtpSecurity::Starttls);
        no_host.host = None;
        assert_eq!(
            SmtpConfig::from_settings(&no_host).unwrap_err(),
            TransportError::InvalidConfiguration
        );
        let mut no_from = settings(true, SmtpSecurity::Starttls);
        no_from.from_email = None;
        assert_eq!(
            SmtpConfig::from_settings(&no_from).unwrap_err(),
            TransportError::InvalidConfiguration
        );
        let mut no_password = settings(true, SmtpSecurity::Starttls);
        no_password.password = None;
        assert_eq!(
            SmtpConfig::from_settings(&no_password).unwrap_err(),
            TransportError::InvalidConfiguration
        );
        let mut no_auth = settings(true, SmtpSecurity::None);
        no_auth.no_auth = true;
        no_auth.username = None;
        no_auth.password = None;
        let config = SmtpConfig::from_settings(&no_auth).unwrap();
        assert!(config.no_auth);
        assert!(config.password.is_none());
    }

    #[test]
    fn unit_transport_error_codes_are_distinguishable() {
        assert_eq!(TransportError::NotConfigured.code(), EMAIL_NOT_CONFIGURED);
        assert_eq!(
            TransportError::InvalidConfiguration.code(),
            EMAIL_CONFIG_INVALID
        );
        assert_eq!(TransportError::Delivery.code(), EMAIL_DELIVERY_FAILED);
        assert_eq!(TransportError::Rejected.code(), EMAIL_REJECTED);
        for error in [
            TransportError::NotConfigured,
            TransportError::InvalidConfiguration,
            TransportError::Delivery,
            TransportError::Rejected,
        ] {
            assert!(!error.to_string().contains(PASSWORD));
        }
    }

    #[test]
    fn unit_smtp_password_never_formatted() {
        let config = SmtpConfig::from_settings(&settings(true, SmtpSecurity::Starttls)).unwrap();
        let rendered = format!("{config:?} | {config:#?}");
        assert!(!rendered.contains(PASSWORD), "{rendered}");
        assert!(rendered.contains("<redacted>"));
        assert!(!TransportError::InvalidConfiguration
            .to_string()
            .contains(PASSWORD));
    }

    #[test]
    fn unit_smtp_tls_modes_build_own_client() {
        for security in [
            SmtpSecurity::Starttls,
            SmtpSecurity::Implicit,
            SmtpSecurity::None,
        ] {
            let config = SmtpConfig::from_settings(&settings(true, security)).unwrap();
            build_client(&config).unwrap();
        }

        let mut permissive = settings(true, SmtpSecurity::Implicit);
        permissive.allow_self_signed_certificate = true;
        let permissive = SmtpConfig::from_settings(&permissive).unwrap();
        let strict = SmtpConfig::from_settings(&settings(true, SmtpSecurity::Implicit)).unwrap();
        assert!(permissive.allow_self_signed_certificate);
        assert!(!strict.allow_self_signed_certificate);
        build_client(&permissive).unwrap();
        build_client(&strict).unwrap();
    }

    #[tokio::test]
    async fn it_capturing_transport_records_without_smtp() {
        let transport = Arc::new(CapturingTransport::new());
        let config = SmtpConfig::from_settings(&settings(true, SmtpSecurity::Starttls)).unwrap();
        transport.send(&config, &message()).await.unwrap();
        assert_eq!(transport.count(), 1);
        let captured = transport.captured();
        assert_eq!(captured[0].message.subject, "Subject");
        assert_eq!(captured[0].config.host, "smtp.example.test");
        assert!(!format!("{captured:?}").contains(PASSWORD));
    }

    #[test]
    fn unit_mailbox_builds_display_name() {
        let email = Email::parse("ada@example.test").unwrap();
        let built = mailbox(Some("Ada".to_owned()), &email).unwrap();
        assert_eq!(built.to_string(), "Ada <ada@example.test>");
        let bare = mailbox(None, &email).unwrap();
        assert_eq!(bare.to_string(), "ada@example.test");
    }

    #[test]
    fn unit_smtp_tls_settings_do_not_touch_global_state() {
        let provider_before = rustls::crypto::CryptoProvider::get_default().is_none();

        let strict = SmtpConfig::from_settings(&settings(true, SmtpSecurity::Implicit)).unwrap();
        drop(tls_parameters(&strict).unwrap());

        let mut permissive = settings(true, SmtpSecurity::Implicit);
        permissive.allow_self_signed_certificate = true;
        let permissive = SmtpConfig::from_settings(&permissive).unwrap();
        drop(tls_parameters(&permissive).unwrap());
        drop(build_client(&permissive).unwrap());

        let provider_after = rustls::crypto::CryptoProvider::get_default().is_none();
        assert_eq!(
            provider_after, provider_before,
            "SMTP TLS must not install or replace a process-wide rustls provider"
        );
    }
}

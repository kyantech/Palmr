use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::{Credentials, DEFAULT_MECHANISMS};
use lettre::transport::smtp::client::{
    AsyncSmtpConnection, CertificateStore, Tls, TlsParameters, TlsParametersBuilder,
};
use lettre::transport::smtp::extension::ClientId;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpGap {
    Host,
    FromEmail,
    Username,
    Password,
}

impl SmtpGap {
    pub const fn field(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::FromEmail => "fromEmail",
            Self::Username => "username",
            Self::Password => "password",
        }
    }
}

struct Resolved<'a> {
    host: &'a str,
    from_email: Email,
    credentials: Option<(&'a str, &'a Secret<String>)>,
}

impl SmtpConfig {
    pub fn from_settings(settings: &SmtpSettings) -> Result<Self, TransportError> {
        if !settings.enabled {
            return Err(TransportError::NotConfigured);
        }
        Self::resolve(settings).map_err(|_| TransportError::InvalidConfiguration)
    }

    pub fn resolve(settings: &SmtpSettings) -> Result<Self, SmtpGap> {
        let resolved = Self::parts(settings)?;
        Ok(Self {
            host: resolved.host.to_owned(),
            port: settings.port,
            security: settings.security,
            username: resolved
                .credentials
                .map(|(username, _)| username.to_owned()),
            password: resolved.credentials.map(|(_, password)| password.clone()),
            from_name: settings.from_name.clone(),
            from_email: resolved.from_email,
            allow_self_signed_certificate: settings.allow_self_signed_certificate,
            no_auth: settings.no_auth,
        })
    }

    pub fn gap(settings: &SmtpSettings) -> Option<SmtpGap> {
        Self::parts(settings).err()
    }

    fn parts(settings: &SmtpSettings) -> Result<Resolved<'_>, SmtpGap> {
        let host = settings
            .host
            .as_deref()
            .filter(|host| !host.trim().is_empty())
            .ok_or(SmtpGap::Host)?;
        let from_email = settings
            .from_email
            .as_deref()
            .and_then(|address| Email::parse(address).ok())
            .filter(|email| email.as_str().parse::<Address>().is_ok())
            .ok_or(SmtpGap::FromEmail)?;
        let credentials = if settings.no_auth {
            None
        } else {
            let username = settings
                .username
                .as_deref()
                .filter(|username| !username.is_empty())
                .ok_or(SmtpGap::Username)?;
            let password = settings.password.as_ref().ok_or(SmtpGap::Password)?;
            Some((username, password))
        };
        Ok(Resolved {
            host,
            from_email,
            credentials,
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

pub type ProbeFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ProbeFailure>> + Send + 'a>>;

pub trait EmailTransport: Send + Sync + 'static {
    fn send<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
    ) -> TransportFuture<'a>;

    fn probe<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
        progress: &'a StageTracker,
    ) -> ProbeFuture<'a>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Connect,
    Starttls,
    Auth,
    Send,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Starttls => "starttls",
            Self::Auth => "auth",
            Self::Send => "send",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    Unreachable,
    TimedOut,
    Tls,
    StarttlsUnsupported,
    NoAuthMechanism,
    CredentialsRejected,
    AuthDeferred,
    MessageRejected,
    MessageDeferred,
    Protocol,
}

impl FailureReason {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Unreachable => "Could not connect to the SMTP server.",
            Self::TimedOut => "The SMTP server did not respond in time.",
            Self::Tls => "The TLS handshake failed. Check the certificate and the security mode.",
            Self::StarttlsUnsupported => "The SMTP server does not offer STARTTLS.",
            Self::NoAuthMechanism => {
                "The SMTP server offers no authentication mechanism Palmr supports."
            }
            Self::CredentialsRejected => "The SMTP server rejected the credentials.",
            Self::AuthDeferred => "The SMTP server could not process the credentials right now.",
            Self::MessageRejected => "The SMTP server rejected the message.",
            Self::MessageDeferred => "The SMTP server deferred the message.",
            Self::Protocol => "The SMTP server sent an unexpected response.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeFailure {
    pub stage: Stage,
    pub reason: FailureReason,
}

impl ProbeFailure {
    pub const fn new(stage: Stage, reason: FailureReason) -> Self {
        Self { stage, reason }
    }
}

#[derive(Debug)]
pub struct StageTracker {
    state: Mutex<TrackerState>,
}

#[derive(Debug)]
struct TrackerState {
    current: Stage,
    completed: Vec<Stage>,
}

impl Default for StageTracker {
    fn default() -> Self {
        Self {
            state: Mutex::new(TrackerState {
                current: Stage::Connect,
                completed: Vec::new(),
            }),
        }
    }
}

impl StageTracker {
    pub fn enter(&self, stage: Stage) {
        self.lock().current = stage;
    }

    pub fn pass(&self) {
        let mut state = self.lock();
        let current = state.current;
        state.completed.push(current);
    }

    pub fn current(&self) -> Stage {
        self.lock().current
    }

    pub fn completed(&self) -> Vec<Stage> {
        self.lock().completed.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TrackerState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
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

    fn probe<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
        progress: &'a StageTracker,
    ) -> ProbeFuture<'a> {
        Box::pin(run_probe(config, message, progress))
    }
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

async fn run_probe(
    config: &SmtpConfig,
    message: &OutboundMessage,
    progress: &StageTracker,
) -> Result<(), ProbeFailure> {
    let email = build_message(config, message)
        .map_err(|_| ProbeFailure::new(Stage::Send, FailureReason::MessageRejected))?;
    let hello = ClientId::default();
    let tls = match config.security {
        SmtpSecurity::None => None,
        SmtpSecurity::Starttls | SmtpSecurity::Implicit => Some(
            tls_parameters(config)
                .map_err(|_| ProbeFailure::new(Stage::Connect, FailureReason::Tls))?,
        ),
    };

    progress.enter(Stage::Connect);
    let wrapper = tls
        .clone()
        .filter(|_| config.security == SmtpSecurity::Implicit);
    let mut connection = AsyncSmtpConnection::connect_tokio1(
        (config.host.as_str(), config.port),
        Some(CONNECT_TIMEOUT),
        &hello,
        wrapper,
        None,
    )
    .await
    .map_err(|error| failure(Stage::Connect, &error))?;
    progress.pass();

    if let Some(parameters) = tls.filter(|_| config.security == SmtpSecurity::Starttls) {
        progress.enter(Stage::Starttls);
        connection
            .starttls(parameters, &hello)
            .await
            .map_err(|error| failure(Stage::Starttls, &error))?;
        progress.pass();
    }

    if let (false, Some(username), Some(password)) =
        (config.no_auth, &config.username, &config.password)
    {
        progress.enter(Stage::Auth);
        let credentials = Credentials::new(username.clone(), password.expose_secret().clone());
        connection
            .auth(DEFAULT_MECHANISMS, &credentials)
            .await
            .map_err(|error| failure(Stage::Auth, &error))?;
        progress.pass();
    }

    progress.enter(Stage::Send);
    connection
        .send(email.envelope(), &email.formatted())
        .await
        .map_err(|error| failure(Stage::Send, &error))?;
    progress.pass();
    drop(connection.quit().await);
    Ok(())
}

fn failure(stage: Stage, error: &lettre::transport::smtp::Error) -> ProbeFailure {
    let reason = if error.is_timeout() {
        FailureReason::TimedOut
    } else if error.is_tls() || caused_by_tls(error) {
        FailureReason::Tls
    } else {
        match stage {
            Stage::Connect => FailureReason::Unreachable,
            Stage::Starttls if error.is_client() => FailureReason::StarttlsUnsupported,
            Stage::Starttls => FailureReason::Tls,
            Stage::Auth if error.is_client() => FailureReason::NoAuthMechanism,
            Stage::Auth if error.is_permanent() => FailureReason::CredentialsRejected,
            Stage::Auth if error.is_transient() => FailureReason::AuthDeferred,
            Stage::Send if error.is_permanent() => FailureReason::MessageRejected,
            Stage::Send if error.is_transient() => FailureReason::MessageDeferred,
            Stage::Auth | Stage::Send => FailureReason::Protocol,
        }
    };
    ProbeFailure::new(stage, reason)
}

fn caused_by_tls(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(cause) = current {
        let wrapped = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref);
        if cause.is::<rustls::Error>() || wrapped.is_some_and(|inner| inner.is::<rustls::Error>()) {
            return true;
        }
        current = cause.source();
    }
    false
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ProbeScript {
    #[default]
    Succeed,
    Fail(ProbeFailure),
    Hang,
}

#[derive(Debug, Default)]
pub struct CapturingTransport {
    sent: Mutex<Vec<CapturedMessage>>,
    script: Mutex<ProbeScript>,
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

    pub fn script_probe(&self, script: ProbeScript) {
        let mut guard = match self.script.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = script;
    }

    fn scripted(&self) -> ProbeScript {
        match self.script.lock() {
            Ok(guard) => *guard,
            Err(poisoned) => *poisoned.into_inner(),
        }
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

    fn probe<'a>(
        &'a self,
        config: &'a SmtpConfig,
        message: &'a OutboundMessage,
        progress: &'a StageTracker,
    ) -> ProbeFuture<'a> {
        let captured = CapturedMessage {
            config: config.clone(),
            message: message.clone(),
        };
        self.lock().push(captured);
        let script = self.scripted();
        let applicable = [
            Some(Stage::Connect),
            (config.security == SmtpSecurity::Starttls).then_some(Stage::Starttls),
            (!config.no_auth).then_some(Stage::Auth),
            Some(Stage::Send),
        ];
        for stage in applicable.into_iter().flatten() {
            progress.enter(stage);
            match script {
                ProbeScript::Fail(failure) if failure.stage == stage => {
                    return Box::pin(async move { Err(failure) });
                }
                ProbeScript::Hang if stage == Stage::Send => {
                    return Box::pin(std::future::pending());
                }
                ProbeScript::Succeed | ProbeScript::Fail(_) | ProbeScript::Hang => {}
            }
            progress.pass();
        }
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
    fn unit_smtp_capability_gap_names_the_missing_requirement() {
        assert!(settings(true, SmtpSecurity::Starttls).is_available());
        assert!(!settings(false, SmtpSecurity::Starttls).is_available());
        assert_eq!(SmtpConfig::gap(&settings(false, SmtpSecurity::None)), None);

        let mut blank_host = settings(true, SmtpSecurity::None);
        blank_host.host = Some("   ".to_owned());
        assert_eq!(SmtpConfig::gap(&blank_host), Some(SmtpGap::Host));
        let mut unusable_sender = settings(true, SmtpSecurity::None);
        unusable_sender.from_email = Some("a,b@example.test".to_owned());
        assert!(Email::parse("a,b@example.test").is_ok());
        assert_eq!(SmtpConfig::gap(&unusable_sender), Some(SmtpGap::FromEmail));
        assert!(!unusable_sender.is_available());
        let mut empty_username = settings(true, SmtpSecurity::None);
        empty_username.username = Some(String::new());
        assert_eq!(SmtpConfig::gap(&empty_username), Some(SmtpGap::Username));
        let mut no_password = settings(true, SmtpSecurity::None);
        no_password.password = None;
        assert_eq!(SmtpConfig::gap(&no_password), Some(SmtpGap::Password));
        no_password.no_auth = true;
        assert_eq!(SmtpConfig::gap(&no_password), None);

        let mut gaps = SmtpSettings::clone(&settings(true, SmtpSecurity::None));
        gaps.host = None;
        gaps.from_email = None;
        gaps.username = None;
        gaps.password = None;
        let order: Vec<&str> = [
            SmtpConfig::gap(&gaps),
            {
                gaps.host = Some("smtp.example.test".to_owned());
                SmtpConfig::gap(&gaps)
            },
            {
                gaps.from_email = Some("palmr@example.test".to_owned());
                SmtpConfig::gap(&gaps)
            },
            {
                gaps.username = Some("palmr".to_owned());
                SmtpConfig::gap(&gaps)
            },
        ]
        .into_iter()
        .flatten()
        .map(SmtpGap::field)
        .collect();
        assert_eq!(order, ["host", "fromEmail", "username", "password"]);
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

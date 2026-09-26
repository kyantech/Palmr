use crate::domain::time::Timestamp;
use crate::infra::http::proxy::ResolvedClient;
use crate::infra::http::request_id::RequestId;
use crate::infra::jobs::runtime::FailureClass;

use super::actions::{ActionSpec, Metadata};

pub const MAX_ACTOR_ID: usize = 64;
pub const MAX_ACTOR_LABEL: usize = 320;
pub const MAX_TARGET_ID: usize = 128;
pub const MAX_TARGET_LABEL: usize = 320;
pub const MAX_REQUEST_ID: usize = 64;
pub const MAX_USER_AGENT: usize = 256;

fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorType {
    User,
    Anonymous,
    System,
    OperatorCli,
}

impl ActorType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Anonymous => "anonymous",
            Self::System => "system",
            Self::OperatorCli => "operator_cli",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    kind: ActorType,
    user_id: Option<String>,
    label: Option<String>,
}

impl Actor {
    pub fn user(user_id: &str, label: &str) -> Self {
        Self {
            kind: ActorType::User,
            user_id: Some(bounded(user_id, MAX_ACTOR_ID)),
            label: Some(bounded(label, MAX_ACTOR_LABEL)),
        }
    }

    pub fn anonymous(label: &str) -> Self {
        Self {
            kind: ActorType::Anonymous,
            user_id: None,
            label: Some(bounded(label, MAX_ACTOR_LABEL)),
        }
    }

    pub const fn system() -> Self {
        Self {
            kind: ActorType::System,
            user_id: None,
            label: None,
        }
    }

    pub fn operator_cli(label: &str) -> Self {
        Self {
            kind: ActorType::OperatorCli,
            user_id: None,
            label: Some(bounded(label, MAX_ACTOR_LABEL)),
        }
    }

    pub const fn kind(&self) -> ActorType {
        self.kind
    }

    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    User,
    File,
    Folder,
    Share,
    ShareItem,
    ReverseShare,
    ReceivedFile,
    EmbedGrant,
    Setting,
    Provider,
    IdentityLink,
    Session,
    TrustedDevice,
    Invite,
    Job,
    BrandingAsset,
    StorageObject,
    TusUpload,
    S3MultipartUpload,
    System,
}

impl TargetType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::File => "file",
            Self::Folder => "folder",
            Self::Share => "share",
            Self::ShareItem => "share_item",
            Self::ReverseShare => "reverse_share",
            Self::ReceivedFile => "received_file",
            Self::EmbedGrant => "embed_grant",
            Self::Setting => "setting",
            Self::Provider => "provider",
            Self::IdentityLink => "identity_link",
            Self::Session => "session",
            Self::TrustedDevice => "trusted_device",
            Self::Invite => "invite",
            Self::Job => "job",
            Self::BrandingAsset => "branding_asset",
            Self::StorageObject => "storage_object",
            Self::TusUpload => "tus_upload",
            Self::S3MultipartUpload => "s3_multipart_upload",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    kind: TargetType,
    id: Option<String>,
    label: Option<String>,
}

impl Target {
    pub const fn new(kind: TargetType) -> Self {
        Self {
            kind,
            id: None,
            label: None,
        }
    }

    #[must_use]
    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(bounded(id, MAX_TARGET_ID));
        self
    }

    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(bounded(label, MAX_TARGET_LABEL));
        self
    }

    pub const fn kind(&self) -> TargetType {
        self.kind
    }

    pub fn id_value(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn label_value(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditCode {
    HandlerFailed,
    HandlerRejected,
    HandlerPanicked,
    NoHandler,
    AuthInvalidCredentials,
    AuthLocked,
    AuthPasswordLoginDisabled,
}

impl AuditCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HandlerFailed => "JOB_HANDLER_FAILED",
            Self::HandlerRejected => "JOB_HANDLER_REJECTED",
            Self::HandlerPanicked => "JOB_HANDLER_PANICKED",
            Self::NoHandler => "JOB_NO_HANDLER",
            Self::AuthInvalidCredentials => "AUTH_INVALID_CREDENTIALS",
            Self::AuthLocked => "AUTH_LOCKED",
            Self::AuthPasswordLoginDisabled => "AUTH_PASSWORD_LOGIN_DISABLED",
        }
    }
}

impl From<FailureClass> for AuditCode {
    fn from(failure: FailureClass) -> Self {
        match failure {
            FailureClass::HandlerFailed => Self::HandlerFailed,
            FailureClass::HandlerRejected => Self::HandlerRejected,
            FailureClass::HandlerPanicked => Self::HandlerPanicked,
            FailureClass::NoHandler => Self::NoHandler,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure(AuditCode),
    Denied(AuditCode),
}

impl Outcome {
    pub const fn result_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure(_) => "failure",
            Self::Denied(_) => "denied",
        }
    }

    pub const fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Success => None,
            Self::Failure(code) | Self::Denied(code) => Some(code.as_str()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePath {
    InTransaction,
    Enqueued,
}

impl WritePath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InTransaction => "in_transaction",
            Self::Enqueued => "enqueued",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    JobDeadLettered,
    SettingChanged,
    StorageOrphanDetected,
    SessionRevoked,
    AllSessionsRevoked,
    SetupCompleted,
    LoginSucceeded,
    LoginFailed,
    LoginLockedOut,
    Logout,
    PasswordChanged,
}

impl AuditAction {
    pub const ALL: &'static [Self] = &[
        Self::JobDeadLettered,
        Self::SettingChanged,
        Self::StorageOrphanDetected,
        Self::SessionRevoked,
        Self::AllSessionsRevoked,
        Self::SetupCompleted,
        Self::LoginSucceeded,
        Self::LoginFailed,
        Self::LoginLockedOut,
        Self::Logout,
        Self::PasswordChanged,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JobDeadLettered => "JOB_DEAD_LETTERED",
            Self::SettingChanged => "SETTING_CHANGED",
            Self::StorageOrphanDetected => "STORAGE_ORPHAN_DETECTED",
            Self::SessionRevoked => "SESSION_REVOKED",
            Self::AllSessionsRevoked => "ALL_SESSIONS_REVOKED",
            Self::SetupCompleted => "SETUP_COMPLETED",
            Self::LoginSucceeded => "LOGIN_SUCCEEDED",
            Self::LoginFailed => "LOGIN_FAILED",
            Self::LoginLockedOut => "LOGIN_LOCKED_OUT",
            Self::Logout => "LOGOUT",
            Self::PasswordChanged => "PASSWORD_CHANGED",
        }
    }

    pub const fn write_path(self) -> WritePath {
        match self {
            Self::JobDeadLettered
            | Self::StorageOrphanDetected
            | Self::LoginSucceeded
            | Self::LoginFailed
            | Self::LoginLockedOut
            | Self::Logout => WritePath::Enqueued,
            Self::SettingChanged
            | Self::SessionRevoked
            | Self::AllSessionsRevoked
            | Self::SetupCompleted
            | Self::PasswordChanged => WritePath::InTransaction,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientMetadata {
    request_id: Option<String>,
    client_ip: Option<String>,
    user_agent: Option<String>,
}

impl ClientMetadata {
    pub const fn none() -> Self {
        Self {
            request_id: None,
            client_ip: None,
            user_agent: None,
        }
    }

    pub fn from_request(
        client: &ResolvedClient,
        request_id: Option<&RequestId>,
        user_agent: Option<&str>,
    ) -> Self {
        Self {
            request_id: request_id.map(|id| bounded(id.as_str(), MAX_REQUEST_ID)),
            client_ip: Some(client.ip().to_string()),
            user_agent: user_agent.map(|agent| bounded(agent, MAX_USER_AGENT)),
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub fn client_ip(&self) -> Option<&str> {
        self.client_ip.as_deref()
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }
}

#[derive(Debug, Clone)]
pub struct AuditEvent {
    spec: ActionSpec,
    actor: Actor,
    target: Option<Target>,
    outcome: Outcome,
    occurred_at: Timestamp,
    client: ClientMetadata,
}

impl AuditEvent {
    pub fn new(spec: ActionSpec, actor: Actor, outcome: Outcome, occurred_at: Timestamp) -> Self {
        Self {
            spec,
            actor,
            target: None,
            outcome,
            occurred_at,
            client: ClientMetadata::none(),
        }
    }

    #[must_use]
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }

    #[must_use]
    pub fn with_client(mut self, client: ClientMetadata) -> Self {
        self.client = client;
        self
    }

    pub fn action(&self) -> AuditAction {
        self.spec.action()
    }

    pub fn metadata(&self) -> &Metadata {
        self.spec.metadata()
    }

    pub const fn actor(&self) -> &Actor {
        &self.actor
    }

    pub const fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    pub const fn outcome(&self) -> Outcome {
        self.outcome
    }

    pub const fn occurred_at(&self) -> Timestamp {
        self.occurred_at
    }

    pub const fn client(&self) -> &ClientMetadata {
        &self.client
    }
}

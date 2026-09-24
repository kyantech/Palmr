use std::sync::Arc;

use serde_json::Value;

use super::error::{EmailError, EMAIL_TEMPLATE_FAILED, EMAIL_TOKEN_UNREADABLE};
use super::model::{
    link_from, outbox_token_aad, public_link, resolve_locale, NewMail, OutboxId, OutboxRow,
    ParamsError,
};
use super::render::{MailRenderer, RenderRequest};
use super::repo::{self, NewOutboxRow};
use super::transport::{EmailTransport, OutboundMailbox, OutboundMessage, SmtpConfig};
use crate::config::PublicBaseUrl;
use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::settings::SettingsHandle;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::db::{DbPools, WriteTx};
use crate::infra::jobs::claim::enqueue;
use crate::infra::jobs::{
    ClaimedJob, DedupKey, Idempotency, JobKind, JobPayload, JobsError, NewJob, Registry,
};

const EMAIL_SEND_IDEMPOTENCY: &str = "email_outbox id";
const RENDER_FAILURE_CODE: &str = EMAIL_TEMPLATE_FAILED;

#[derive(Clone)]
pub struct EmailService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    keys: Arc<KeyRing>,
    settings: SettingsHandle,
    base_url: PublicBaseUrl,
    renderer: MailRenderer,
    transport: Arc<dyn EmailTransport>,
}

impl std::fmt::Debug for EmailService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailService").finish_non_exhaustive()
    }
}

impl EmailService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        keys: Arc<KeyRing>,
        settings: SettingsHandle,
        base_url: PublicBaseUrl,
        transport: Arc<dyn EmailTransport>,
    ) -> Self {
        Self {
            pools,
            clock,
            keys,
            settings,
            base_url,
            renderer: MailRenderer::new(),
            transport,
        }
    }

    pub async fn enqueue(
        &self,
        tx: &mut WriteTx<'_>,
        mail: NewMail,
    ) -> Result<OutboxId, EmailError> {
        if mail.params.kind() != mail.kind {
            return Err(EmailError::KindMismatch);
        }
        if mail.token.is_some() && !mail.kind.carries_sealed_token() {
            return Err(EmailError::TokenNotAllowed);
        }
        if let Some(batch_key) = &mail.batch_key {
            if batch_key.chars().count() > super::model::MAX_BATCH_KEY_CHARS {
                return Err(EmailError::InvalidBatchKey);
            }
        }
        link_from(mail.kind, &mail.params)?;

        let id = OutboxId::generate(self.clock.as_ref());
        let now = now(self.clock.as_ref())?;
        let locale = resolve_locale(
            mail.locale_preference,
            Some(self.settings.load().default_locale()),
        );
        let sealed_token = match &mail.token {
            Some(token) => Some(self.keys.seal(
                SealPurpose::OutboxToken,
                &outbox_token_aad(id, mail.kind),
                token.expose_secret().as_bytes(),
            )?),
            None => None,
        };
        let params_json = serde_json::to_string(&mail.params).map_err(ParamsError::from)?;
        if params_json.len() > super::model::MAX_PARAMS_BYTES {
            return Err(EmailError::ParamsTooLarge);
        }

        repo::insert(
            tx,
            &NewOutboxRow {
                id,
                kind: mail.kind,
                to_email: mail.recipient.email(),
                to_name: mail.recipient.name(),
                locale,
                params_json: &params_json,
                batch_key: mail.batch_key.as_deref(),
                dedup_key: mail.dedup_key.as_ref().map(DedupKey::as_str),
                scheduled_at: mail.scheduled_at.unwrap_or(now),
                created_at: now,
                sealed_token: sealed_token.as_ref(),
            },
        )
        .await?;

        let payload = JobPayload::new(&serde_json::json!({ "outbox_id": id.to_string() }))?;
        let dedup_key = DedupKey::new(format!("email_outbox:{id}"))?;
        enqueue(
            tx,
            self.clock.as_ref(),
            &NewJob::new(JobKind::EmailSend, payload).dedup_key(dedup_key),
        )
        .await?;
        Ok(id)
    }

    pub async fn cancel_in_tx(
        &self,
        tx: &mut WriteTx<'_>,
        id: OutboxId,
    ) -> Result<bool, EmailError> {
        let now = now(self.clock.as_ref())?;
        Ok(repo::cancel(tx, id, now).await?)
    }

    async fn deliver(&self, id: OutboxId, job: &ClaimedJob) -> anyhow::Result<()> {
        let Some(row) = repo::load(self.pools.reader(), id).await? else {
            return Ok(());
        };
        if row.state.is_terminal() {
            return Ok(());
        }

        let settings = self.settings.load();
        let config = match SmtpConfig::from_settings(&settings.smtp) {
            Ok(config) => config,
            Err(error) => {
                self.settle_failed(id, error.code()).await?;
                return Ok(());
            }
        };

        let next_attempt = job.attempts().saturating_add(1);
        let marked_at = now(self.clock.as_ref())?;
        let moved = self
            .pools
            .write_tx(self.clock.as_ref(), "email.mark_sending", async |tx| {
                repo::mark_sending(tx, id, next_attempt, marked_at).await
            })
            .await?;
        if !moved {
            return Ok(());
        }

        let token = match self.open_token(&row) {
            Ok(token) => token,
            Err(()) => {
                self.settle_failed(id, EMAIL_TOKEN_UNREADABLE).await?;
                return Ok(());
            }
        };
        let message = match self.render(&row, &settings, token.as_deref()) {
            Ok(message) => message,
            Err(()) => {
                self.settle_failed(id, RENDER_FAILURE_CODE).await?;
                return Ok(());
            }
        };

        match self.transport.send(&config, &message).await {
            Ok(()) => {
                self.settle_sent(id).await?;
                Ok(())
            }
            Err(error) => {
                if next_attempt >= job.max_attempts() {
                    self.settle_failed(id, error.code()).await?;
                } else {
                    self.record_retry_error(id, error.code()).await?;
                }
                anyhow::bail!("email delivery failed: {}", error.code())
            }
        }
    }

    fn open_token(&self, row: &OutboxRow) -> Result<Option<String>, ()> {
        if !row.kind.carries_sealed_token() {
            return Ok(None);
        }
        let sealed = row.sealed_token.as_ref().ok_or(())?;
        let opened = self
            .keys
            .open(
                SealPurpose::OutboxToken,
                &outbox_token_aad(row.id, row.kind),
                sealed,
            )
            .map_err(|_| ())?;
        String::from_utf8(opened.expose_secret().clone())
            .map(Some)
            .map_err(|_| ())
    }

    fn render(
        &self,
        row: &OutboxRow,
        settings: &crate::features::settings::model::AppSettings,
        token: Option<&str>,
    ) -> Result<OutboundMessage, ()> {
        let link = link_from(row.kind, &row.params).map_err(|_| ())?;
        let action_url = public_link(self.base_url.url(), &link, token);
        let rendered = self
            .renderer
            .render(&RenderRequest {
                kind: row.kind,
                locale: row.locale,
                recipient_name: row.to_name.as_ref().map(|name| name.as_str()),
                app_name: settings.app_name(),
                logo_url: branding_logo_url(),
                action_url: &action_url,
                params: &row.params,
            })
            .map_err(|_| ())?;
        Ok(OutboundMessage {
            from: OutboundMailbox {
                name: settings.smtp.from_name.clone(),
                email: settings
                    .smtp
                    .from_email
                    .as_deref()
                    .and_then(|address| crate::domain::email::Email::parse(address).ok())
                    .ok_or(())?,
            },
            to: OutboundMailbox {
                name: row.to_name.as_ref().map(|name| name.as_str().to_owned()),
                email: row.to_email.clone(),
            },
            subject: rendered.subject,
            html: rendered.html,
            text: rendered.text,
        })
    }

    async fn settle_sent(&self, id: OutboxId) -> Result<(), EmailError> {
        let settled_at = now(self.clock.as_ref())?;
        self.pools
            .write_tx(self.clock.as_ref(), "email.mark_sent", async |tx| {
                repo::mark_sent(tx, id, settled_at).await
            })
            .await?;
        Ok(())
    }

    async fn settle_failed(&self, id: OutboxId, code: &'static str) -> Result<(), EmailError> {
        let settled_at = now(self.clock.as_ref())?;
        self.pools
            .write_tx(self.clock.as_ref(), "email.mark_failed", async |tx| {
                repo::mark_failed(tx, id, code, settled_at).await
            })
            .await?;
        Ok(())
    }

    async fn record_retry_error(&self, id: OutboxId, code: &'static str) -> Result<(), EmailError> {
        let recorded_at = now(self.clock.as_ref())?;
        self.pools
            .write_tx(self.clock.as_ref(), "email.record_retry", async |tx| {
                repo::mark_retry_error(tx, id, code, recorded_at).await
            })
            .await?;
        Ok(())
    }
}

pub fn register_jobs(registry: Registry, service: EmailService) -> Registry {
    registry.register(
        JobKind::EmailSend,
        Idempotency::key(EMAIL_SEND_IDEMPOTENCY),
        move |job: ClaimedJob| {
            let service = service.clone();
            async move {
                let id = outbox_id(job.payload())
                    .ok_or_else(|| anyhow::anyhow!("email.send payload has no outbox id"))?;
                service.deliver(id, &job).await
            }
        },
    )
}

pub fn outbox_id(payload: &Value) -> Option<OutboxId> {
    payload
        .get("outbox_id")
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
}

fn now(clock: &dyn Clock) -> Result<Timestamp, JobsError> {
    Ok(Timestamp::try_from(clock.now())?)
}

fn branding_logo_url() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::Value;
    use sqlx::Row;
    use tempfile::TempDir;
    use time::macros::datetime;

    use crate::config::{EnvironmentSource, OperatorConfig, PublicBaseUrl, SqliteSynchronous};
    use crate::domain::clock::{Clock, TestClock};
    use crate::domain::locale::LocaleCode;
    use crate::domain::secret::Secret;
    use crate::domain::time::Timestamp;
    use crate::features::email::error::{EmailError, EMAIL_NOT_CONFIGURED};
    use crate::features::email::model::{
        outbox_token_aad, LocalePreference, MailKind, MailParams, NewMail, OutboxId, Recipient,
        SecurityEvent,
    };
    use crate::features::email::service::{register_jobs, EmailService};
    use crate::features::email::transport::CapturingTransport;
    use crate::features::settings::model::{AppSettings, SmtpSecurity};
    use crate::features::settings::snapshot::SettingsHandle;
    use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
    use crate::infra::crypto::instance_key::InstanceKey;
    use crate::infra::db::{DbError, DbPools, InstanceId, WriteTx, MIGRATOR};
    use crate::infra::jobs::backoff::Jitter;
    use crate::infra::jobs::claim::enqueue as enqueue_job;
    use crate::infra::jobs::cli::run_once;
    use crate::infra::jobs::runtime::{Dispatcher, RuntimeTiming};
    use crate::infra::jobs::{Claimant, DedupKey, JobAudit, JobKind, JobPayload, NewJob, Registry};

    const START: time::OffsetDateTime = datetime!(2026-09-24 12:00 UTC);
    const USER_ID: &str = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d";
    const TOKEN: &str = "palmr-outbox-token-sentinel-7f3a91c2";
    const BASE: &str = "https://palmr.example";

    struct Harness {
        _root: TempDir,
        root_path: PathBuf,
        pools: DbPools,
        clock: TestClock,
        keys: Arc<KeyRing>,
        service: EmailService,
        transport: Arc<CapturingTransport>,
    }

    impl Harness {
        async fn open() -> Self {
            Self::open_with_settings(SettingsHandle::documented_defaults()).await
        }

        async fn open_with_smtp() -> Self {
            let mut settings = AppSettings::defaults();
            settings.smtp.enabled = true;
            settings.smtp.host = Some("smtp.example.test".to_owned());
            settings.smtp.port = 587;
            settings.smtp.security = SmtpSecurity::Starttls;
            settings.smtp.username = Some("palmr".to_owned());
            settings.smtp.password = Some(Secret::new("palmr-smtp-secret".to_owned()));
            settings.smtp.from_name = Some("Palmr".to_owned());
            settings.smtp.from_email = Some("palmr@example.test".to_owned());
            Self::open_with_settings(SettingsHandle::new(settings)).await
        }

        async fn open_with_settings(settings: SettingsHandle) -> Self {
            let root = TempDir::new().unwrap();
            let root_path = root.path().to_path_buf();
            let pools = DbPools::open(&root_path, 4, SqliteSynchronous::Full)
                .await
                .unwrap();
            pools.migrate(&MIGRATOR).await.unwrap();
            let clock = TestClock::new(START);
            let (instance_key, _) = InstanceKey::load_or_create(&root_path).unwrap();
            let keys = Arc::new(KeyRing::new(&instance_key));
            let transport = Arc::new(CapturingTransport::new());
            let base_url = base_url();
            let service = EmailService::new(
                pools.clone(),
                Arc::new(clock.clone()),
                Arc::clone(&keys),
                settings,
                base_url,
                transport.clone(),
            );
            Self {
                _root: root,
                root_path,
                pools,
                clock,
                keys,
                service,
                transport,
            }
        }

        fn shared_clock(&self) -> Arc<dyn Clock> {
            Arc::new(self.clock.clone())
        }

        fn dispatcher(&self) -> Dispatcher {
            let registry = register_jobs(Registry::production(), self.service.clone());
            Dispatcher::new(
                self.pools.clone(),
                self.shared_clock(),
                registry,
                Jitter::from_fn(|| 0),
                JobAudit::detached(),
                RuntimeTiming::DEFAULT.lease_renewal,
            )
        }

        fn claimant(&self) -> Claimant {
            Claimant::worker(InstanceId::generate(&self.clock), 0)
        }

        async fn run_email(&self) {
            let dispatcher = self.dispatcher();
            run_once(&dispatcher, &self.claimant(), JobKind::EmailSend)
                .await
                .unwrap();
        }

        async fn count(&self, table: &str) -> i64 {
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(self.pools.reader().executor())
                .await
                .unwrap()
        }

        async fn outbox(&self, id: OutboxId) -> OutboxRecord {
            let row = sqlx::query(
                "SELECT state, last_error, sent_at, attempts, params_json, token_ciphertext,
                    token_nonce, key_version
               FROM email_outbox WHERE id = ?1",
            )
            .bind(id.to_string())
            .fetch_one(self.pools.reader().executor())
            .await
            .unwrap();
            OutboxRecord {
                state: row.get("state"),
                last_error: row.get("last_error"),
                sent_at: row.get("sent_at"),
                attempts: row.get("attempts"),
                params_json: row.get("params_json"),
                token: (
                    row.get("token_ciphertext"),
                    row.get("token_nonce"),
                    row.get("key_version"),
                ),
            }
        }

        async fn job_payloads(&self) -> Vec<String> {
            sqlx::query_scalar("SELECT payload_json FROM jobs WHERE kind = 'email.send'")
                .fetch_all(self.pools.reader().executor())
                .await
                .unwrap()
        }

        async fn seed_user(&self) -> Result<(), DbError> {
            self.pools
                .write_tx(&self.clock, "test.seed_user", async |tx| {
                    insert_user(tx, &self.clock).await
                })
                .await
        }

        async fn requeue(&self, id: OutboxId, tag: &str) {
            self.pools
                .write_tx(&self.clock, "test.requeue_email", async |tx| {
                    let payload =
                        JobPayload::new(&serde_json::json!({ "outbox_id": id.to_string() }))
                            .unwrap();
                    let job = NewJob::new(JobKind::EmailSend, payload)
                        .dedup_key(DedupKey::new(format!("retry:{tag}")).unwrap());
                    enqueue_job(tx, &self.clock, &job).await
                })
                .await
                .unwrap();
        }
    }

    struct OutboxRecord {
        state: String,
        last_error: Option<String>,
        sent_at: Option<String>,
        attempts: i64,
        params_json: String,
        token: (Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>),
    }

    fn base_url() -> PublicBaseUrl {
        OperatorConfig::load(&EnvironmentSource::from_vars([("PALMR_BASE_URL", BASE)]))
            .unwrap()
            .config
            .base_url
    }

    async fn insert_user(tx: &mut WriteTx<'_>, clock: &TestClock) -> Result<(), DbError> {
        let at = Timestamp::try_from(clock.now()).unwrap().to_string();
        sqlx::query(
            "INSERT INTO users (id, email, email_normalized, username, username_normalized,
                            created_at, updated_at)
         VALUES (?1, 'ada@example.test', 'ada@example.test', 'ada', 'ada', ?2, ?2)",
        )
        .bind(USER_ID)
        .bind(at)
        .execute(tx.executor())
        .await?;
        Ok(())
    }

    fn password_reset() -> NewMail {
        NewMail::new(
            MailKind::PasswordReset,
            Recipient::try_from(("ada@example.test", Some("Ada"))).unwrap(),
            LocalePreference::Account(LocaleCode::EnUs),
            MailParams::PasswordReset { expiry_minutes: 60 },
        )
        .with_token(Secret::new(TOKEN.to_owned()))
    }

    fn invite(recipient: &str) -> NewMail {
        NewMail::new(
            MailKind::Invite,
            Recipient::try_from((recipient, Some("Bo"))).unwrap(),
            LocalePreference::InstanceDefault,
            MailParams::Invite {
                inviter_name: crate::features::email::model::DisplayText::new("Ada"),
                expiry_hours: 24,
            },
        )
        .with_token(Secret::new(TOKEN.to_owned()))
    }

    #[tokio::test]
    async fn it_email_outbox_same_tx_as_business_change() {
        let harness = Harness::open().await;

        let rolled_back: Result<(), EmailError> = harness
            .pools
            .write_tx(&harness.clock, "test.business_rollback", async |tx| {
                insert_user(tx, &harness.clock).await?;
                harness.service.enqueue(tx, password_reset()).await?;
                Err(EmailError::InvalidBatchKey)
            })
            .await;
        assert!(rolled_back.is_err());
        assert_eq!(harness.count("users").await, 0);
        assert_eq!(harness.count("email_outbox").await, 0);
        assert_eq!(harness.count("jobs").await, 0);
        assert_eq!(harness.transport.count(), 0);

        harness
            .pools
            .write_tx(&harness.clock, "test.business_commit", async |tx| {
                insert_user(tx, &harness.clock).await?;
                harness.service.enqueue(tx, password_reset()).await?;
                Ok::<(), EmailError>(())
            })
            .await
            .unwrap();

        assert_eq!(harness.count("users").await, 1);
        assert_eq!(harness.count("email_outbox").await, 1);
        assert_eq!(harness.count("jobs").await, 1);
        assert_eq!(harness.transport.count(), 0);

        let payloads = harness.job_payloads().await;
        assert_eq!(payloads.len(), 1);
        let payload: Value = serde_json::from_str(&payloads[0]).unwrap();
        let keys: BTreeSet<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(["outbox_id"]));
    }

    #[tokio::test]
    async fn it_email_not_configured_marks_failed_action_succeeds() {
        let harness = Harness::open().await;

        let id = harness
            .pools
            .write_tx(&harness.clock, "test.business_commit", async |tx| {
                insert_user(tx, &harness.clock).await?;
                harness.service.enqueue(tx, password_reset()).await
            })
            .await
            .unwrap();

        let stored = harness.outbox(id).await;
        assert_eq!(stored.state, "pending");
        assert!(stored.token.0.is_some());

        harness.run_email().await;

        let stored = harness.outbox(id).await;
        assert_eq!(stored.state, "failed");
        assert_eq!(stored.last_error.as_deref(), Some(EMAIL_NOT_CONFIGURED));
        assert_eq!(stored.token, (None, None, None));
        assert!(stored.sent_at.is_none());
        assert_eq!(harness.transport.count(), 0);
        assert_eq!(harness.count("users").await, 1);

        assert!(!stored.params_json.contains(TOKEN));
    }

    #[tokio::test]
    async fn it_email_send_idempotent() {
        let harness = Harness::open_with_smtp().await;
        harness.seed_user().await.unwrap();

        let id = harness
            .pools
            .write_tx(&harness.clock, "test.enqueue_reset", async |tx| {
                harness.service.enqueue(tx, password_reset()).await
            })
            .await
            .unwrap();

        harness.run_email().await;
        assert_eq!(harness.transport.count(), 1);
        let stored = harness.outbox(id).await;
        assert_eq!(stored.state, "sent");
        assert!(stored.sent_at.is_some());
        assert_eq!(stored.token, (None, None, None));

        harness.requeue(id, "duplicate").await;
        harness.run_email().await;
        assert_eq!(
            harness.transport.count(),
            1,
            "a sent outbox row must not be delivered twice"
        );

        let canceled = harness
            .pools
            .write_tx(&harness.clock, "test.enqueue_invite", async |tx| {
                harness.service.enqueue(tx, invite("bo@example.test")).await
            })
            .await
            .unwrap();
        harness
            .pools
            .write_tx(&harness.clock, "test.cancel_invite", async |tx| {
                harness.service.cancel_in_tx(tx, canceled).await
            })
            .await
            .unwrap();
        let canceled_row = harness.outbox(canceled).await;
        assert_eq!(canceled_row.state, "canceled");
        assert_eq!(canceled_row.token, (None, None, None));

        harness.run_email().await;
        assert_eq!(
            harness.transport.count(),
            1,
            "a canceled outbox row must not be delivered"
        );
        assert_eq!(harness.outbox(id).await.attempts, 1);
    }

    #[tokio::test]
    async fn it_email_params_and_job_never_carry_the_token() {
        let harness = Harness::open_with_smtp().await;
        let id = harness
            .pools
            .write_tx(&harness.clock, "test.enqueue_reset", async |tx| {
                harness.service.enqueue(tx, password_reset()).await
            })
            .await
            .unwrap();

        let stored = harness.outbox(id).await;
        assert!(!stored.params_json.contains(TOKEN));
        assert!(stored.token.0.is_some());
        assert_ne!(stored.token.0.as_deref(), Some(TOKEN.as_bytes()));

        let payloads = harness.job_payloads().await;
        assert_eq!(payloads.len(), 1);
        assert!(!payloads[0].contains(TOKEN));
        assert!(!payloads[0].contains("template"));
        assert_eq!(payloads[0], format!("{{\"outbox_id\":\"{id}\"}}"));
    }

    #[tokio::test]
    async fn it_email_server_derived_link_ignores_host() {
        let harness = Harness::open_with_smtp().await;
        harness.seed_user().await.unwrap();
        harness
            .pools
            .write_tx(&harness.clock, "test.enqueue_reset", async |tx| {
                harness.service.enqueue(tx, password_reset()).await
            })
            .await
            .unwrap();

        harness.run_email().await;
        assert_eq!(harness.transport.count(), 1);
        let captured = harness.transport.captured();
        let message = &captured[0].message;
        let expected = format!("{BASE}/reset-password/{TOKEN}");
        assert!(message.text.contains(&expected), "{}", message.text);
        let html_link = message.html.replace("&#x2f;", "/").replace("&#47;", "/");
        assert!(html_link.contains(&expected), "{}", message.html);
        for hostile in ["evil.example", "Host:", "Origin:", "forwarded"] {
            assert!(!message.text.contains(hostile));
            assert!(!message.html.contains(hostile));
        }
        assert!(!message.subject.contains('\r'));
        assert!(!message.subject.contains('\n'));
    }

    #[tokio::test]
    async fn it_email_token_wrong_row_aad_fails() {
        let harness = Harness::open_with_smtp().await;
        let first = OutboxId::generate(&harness.clock);
        let second = OutboxId::generate(&harness.clock);

        let sealed = harness
            .keys
            .seal(
                SealPurpose::OutboxToken,
                &outbox_token_aad(first, MailKind::PasswordReset),
                TOKEN.as_bytes(),
            )
            .unwrap();

        let opened = harness
            .keys
            .open(
                SealPurpose::OutboxToken,
                &outbox_token_aad(first, MailKind::PasswordReset),
                &sealed,
            )
            .unwrap();
        assert_eq!(opened.expose_secret(), TOKEN.as_bytes());

        for wrong_aad in [
            outbox_token_aad(second, MailKind::PasswordReset),
            outbox_token_aad(first, MailKind::Invite),
        ] {
            assert!(harness
                .keys
                .open(SealPurpose::OutboxToken, &wrong_aad, &sealed)
                .is_err());
        }
    }

    #[tokio::test]
    async fn it_email_security_notification_renders_locales() {
        let harness = Harness::open_with_smtp().await;
        let mail = NewMail::new(
            MailKind::SecurityNotification,
            Recipient::try_from(("ada@example.test", Some("Ada"))).unwrap(),
            LocalePreference::Explicit(LocaleCode::ArSa),
            MailParams::SecurityNotification {
                event: SecurityEvent::SessionsRevoked,
            },
        );
        let id = harness
            .pools
            .write_tx(&harness.clock, "test.enqueue_security", async |tx| {
                harness.service.enqueue(tx, mail).await
            })
            .await
            .unwrap();
        harness.run_email().await;
        assert_eq!(harness.transport.count(), 1);
        assert_eq!(harness.outbox(id).await.state, "sent");
        let captured = harness.transport.captured();
        assert!(captured[0].message.html.contains("dir=\"rtl\""));
        assert!(captured[0]
            .message
            .text
            .contains("https://palmr.example/settings/security"));
    }

    #[test]
    fn unit_outbox_id_round_trips_through_job_payload() {
        let clock = TestClock::new(START);
        let id = OutboxId::generate(&clock);
        let payload = serde_json::json!({ "outbox_id": id.to_string() });
        assert_eq!(
            crate::features::email::service::outbox_id(&payload),
            Some(id)
        );
        assert_eq!(
            crate::features::email::service::outbox_id(&serde_json::json!({})),
            None
        );
        assert_eq!(
            crate::features::email::service::outbox_id(
                &serde_json::json!({ "outbox_id": "not-an-id" })
            ),
            None
        );
    }
}

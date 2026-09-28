use std::sync::Arc;
use std::time::Duration;

use crate::domain::clock::Clock;
use crate::domain::time::Timestamp;
use crate::features::audit::actions::{self, ActionSpec};
use crate::features::audit::model::{
    Actor, AuditEvent, ClientMetadata, Outcome, Target, TargetType,
};
use crate::features::audit::service::AuditService;
use crate::features::auth::sessions::{
    AuthenticatedPrincipal, MintedSession, RevokedReason, SessionError,
};
use crate::features::auth::trusted_devices::repo as trusted_devices;
use crate::features::auth::AuthService;
use crate::features::settings::SettingsHandle;
use crate::features::users::model::{User, UserId};
use crate::features::users::repo as users;
use crate::infra::crypto::aead::SealedSecret;
use crate::infra::crypto::hkdf::{KeyRing, SealPurpose};
use crate::infra::crypto::totp::{time_step, BackupCode, TotpCode, TotpSecret};
use crate::infra::db::{DbPools, WriteTx};

use super::error::TotpError;
use super::model::{
    enrollment_id, enrollment_matches, provisioning_uri, secret_aad, BackupCodesResponse,
    CodeCheck, EnrollmentResponse, EnrollmentVerifyRequest, TotpRow, TotpState, TwoFactorStatus,
};
use super::repo;

pub const ENROLL_TRANSACTION: &str = "auth.totp.enroll";
pub const VERIFY_TRANSACTION: &str = "auth.totp.verify";
pub const DISABLE_TRANSACTION: &str = "auth.totp.disable";
pub const REGENERATE_TRANSACTION: &str = "auth.totp.regenerate_backup_codes";

pub const PENDING_ENROLLMENT_TTL: Duration = Duration::from_secs(10 * 60);

pub struct Enabled {
    pub response: BackupCodesResponse,
    pub session: MintedSession,
}

#[derive(Clone)]
pub struct TotpService {
    pools: DbPools,
    clock: Arc<dyn Clock>,
    settings: SettingsHandle,
    keys: Arc<KeyRing>,
    auth: AuthService,
    audit: AuditService,
}

impl TotpService {
    pub fn new(
        pools: DbPools,
        clock: Arc<dyn Clock>,
        settings: SettingsHandle,
        keys: Arc<KeyRing>,
        auth: AuthService,
        audit: AuditService,
    ) -> Self {
        Self {
            pools,
            clock,
            settings,
            keys,
            auth,
            audit,
        }
    }

    pub const fn auth(&self) -> &AuthService {
        &self.auth
    }

    pub async fn status(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<TwoFactorStatus, TotpError> {
        let user = self.active_user(principal.user_id).await?;
        let row = repo::find(self.pools.reader(), user.id).await?;
        let enrolled_at = row
            .as_ref()
            .filter(|row| row.state == TotpState::Active)
            .and_then(|row| row.confirmed_at);
        let enabled = user.totp_enabled && enrolled_at.is_some();
        let backup_codes_remaining = if enabled {
            repo::count_unused_backup_codes(self.pools.reader(), user.id).await?
        } else {
            0
        };
        let required_by_policy = self.required_by_policy(&user);
        Ok(TwoFactorStatus {
            enabled,
            enrolled_at: enrolled_at.filter(|_| enabled).map(|at| at.to_string()),
            backup_codes_remaining,
            required_by_policy,
            can_disable: enabled && !required_by_policy,
        })
    }

    pub async fn enroll(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<EnrollmentResponse, TotpError> {
        let secret = TotpSecret::mint()?;
        let sealed = self.keys.seal(
            SealPurpose::Totp,
            &secret_aad(principal.user_id),
            secret.expose_secret(),
        )?;
        let (user, created_at) = self
            .pools
            .write_tx(self.clock.as_ref(), ENROLL_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                let user = active_user_in_tx(tx, principal.user_id).await?;
                if user.totp_enabled || !repo::upsert_pending(tx, user.id, &sealed, now).await? {
                    return Err(TotpError::AlreadyEnabled);
                }
                Ok((user, now))
            })
            .await?;
        let expires_at = Timestamp::try_from(created_at.get() + PENDING_ENROLLMENT_TTL)?;
        Ok(EnrollmentResponse {
            enrollment_id: enrollment_id(sealed.nonce()),
            otpauth_uri: provisioning_uri(&user.email, &secret),
            secret_base32: secret.base32().expose_secret().clone(),
            expires_at: expires_at.to_string(),
        })
    }

    pub async fn verify(
        &self,
        principal: &AuthenticatedPrincipal,
        request: EnrollmentVerifyRequest,
        client: &ClientMetadata,
    ) -> Result<Enabled, TotpError> {
        if request.enrollment_id.is_empty() {
            return Err(TotpError::Invalid {
                fields: vec!["enrollmentId"],
            });
        }
        let code = TotpCode::parse(&request.code);
        let codes = BackupCode::mint_set()?;
        let credentials = self.auth.sessions().prepare_credentials()?;
        let (session, generated_at) = self
            .pools
            .write_tx(self.clock.as_ref(), VERIFY_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                let user = active_user_in_tx(tx, principal.user_id).await?;
                let created_after = Timestamp::try_from(now.get() - PENDING_ENROLLMENT_TTL)?;
                let pending = repo::find_in_tx(tx, user.id)
                    .await?
                    .filter(|row| {
                        row.state == TotpState::Pending
                            && row.created_at > created_after
                            && enrollment_matches(&row.secret_nonce, &request.enrollment_id)
                    })
                    .ok_or(TotpError::EnrollmentPendingMissing)?;
                let code = code.as_ref().ok_or(TotpError::CodeInvalid)?;
                let secret = self.open(user.id, &pending)?;
                let step = matching_step(&secret, code, now)?.ok_or(TotpError::CodeInvalid)?;
                if !repo::activate(tx, user.id, &pending.secret_nonce, step, now, created_after)
                    .await?
                {
                    return Err(TotpError::EnrollmentPendingMissing);
                }
                if !repo::set_enabled(tx, user.id, true, now).await? {
                    return Err(TotpError::AlreadyEnabled);
                }
                let digests: Vec<_> = codes.iter().map(BackupCode::digest).collect();
                repo::replace_backup_codes(tx, self.clock.as_ref(), user.id, &digests, now).await?;
                let sessions = self.auth.sessions();
                let revoked = sessions
                    .revoke_all_others_in_tx(
                        tx,
                        user.id,
                        principal.session_id,
                        RevokedReason::PolicyChanged,
                    )
                    .await?;
                let session = sessions
                    .rotate_in_tx(tx, principal.session_id, &credentials)
                    .await?;
                self.record(
                    tx,
                    &user,
                    actions::two_factor_enabled(revoked, codes.len()),
                    client,
                    now,
                )
                .await?;
                Ok((session, now))
            })
            .await?;
        Ok(Enabled {
            response: backup_codes_response(&codes, generated_at),
            session,
        })
    }

    pub async fn disable(
        &self,
        principal: &AuthenticatedPrincipal,
        client: &ClientMetadata,
    ) -> Result<(), TotpError> {
        self.pools
            .write_tx(self.clock.as_ref(), DISABLE_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                let user = active_user_in_tx(tx, principal.user_id).await?;
                let active = repo::find_in_tx(tx, user.id)
                    .await?
                    .is_some_and(|row| row.state == TotpState::Active);
                if !active || !user.totp_enabled {
                    return Err(TotpError::NotEnrolled);
                }
                if self.required_by_policy(&user) {
                    return Err(TotpError::RequiredByPolicy);
                }
                repo::delete_secret(tx, user.id).await?;
                repo::set_enabled(tx, user.id, false, now).await?;
                let codes_deleted = repo::delete_backup_codes(tx, user.id).await?;
                let sessions_revoked = self
                    .auth
                    .sessions()
                    .revoke_all_in_tx(tx, user.id, RevokedReason::PolicyChanged)
                    .await?;
                let devices_revoked = trusted_devices::revoke_all_in_tx(tx, user.id, now).await?;
                self.record(
                    tx,
                    &user,
                    actions::two_factor_disabled(sessions_revoked, devices_revoked, codes_deleted),
                    client,
                    now,
                )
                .await
            })
            .await
    }

    pub async fn regenerate_backup_codes(
        &self,
        principal: &AuthenticatedPrincipal,
        client: &ClientMetadata,
    ) -> Result<BackupCodesResponse, TotpError> {
        let codes = BackupCode::mint_set()?;
        let generated_at = self
            .pools
            .write_tx(self.clock.as_ref(), REGENERATE_TRANSACTION, async |tx| {
                let now = Timestamp::try_from(self.clock.now())?;
                let user = active_user_in_tx(tx, principal.user_id).await?;
                let active = repo::find_in_tx(tx, user.id)
                    .await?
                    .is_some_and(|row| row.state == TotpState::Active);
                if !active || !user.totp_enabled {
                    return Err(TotpError::NotEnrolled);
                }
                let digests: Vec<_> = codes.iter().map(BackupCode::digest).collect();
                let deleted =
                    repo::replace_backup_codes(tx, self.clock.as_ref(), user.id, &digests, now)
                        .await?;
                self.record(
                    tx,
                    &user,
                    actions::two_factor_backup_codes_regenerated(deleted, codes.len()),
                    client,
                    now,
                )
                .await?;
                Ok(now)
            })
            .await?;
        Ok(backup_codes_response(&codes, generated_at))
    }

    fn required_by_policy(&self, user: &User) -> bool {
        user.password_hash.is_some() && self.settings.load().security.two_factor_required
    }

    fn open(&self, user_id: UserId, row: &TotpRow) -> Result<TotpSecret, TotpError> {
        open_secret(&self.keys, user_id, row)
    }

    async fn record(
        &self,
        tx: &mut WriteTx<'_>,
        user: &User,
        spec: ActionSpec,
        client: &ClientMetadata,
        now: Timestamp,
    ) -> Result<(), TotpError> {
        let id = user.id.to_string();
        let event = AuditEvent::new(
            spec,
            Actor::user(&id, &user.username),
            Outcome::Success,
            now,
        )
        .with_target(Target::new(TargetType::User).id(&id).label(&user.username))
        .with_client(client.clone());
        self.audit.record_in_tx(tx, &event).await?;
        Ok(())
    }

    async fn active_user(&self, user_id: UserId) -> Result<User, TotpError> {
        users::find_by_id(self.pools.reader(), user_id)
            .await?
            .filter(|user| user.is_active)
            .ok_or(TotpError::Session(SessionError::AuthRequired))
    }
}

pub(crate) async fn consume_active_code_in_tx(
    tx: &mut WriteTx<'_>,
    keys: &KeyRing,
    user_id: UserId,
    code: &TotpCode,
    now: Timestamp,
) -> Result<CodeCheck, TotpError> {
    let Some(row) = repo::find_in_tx(tx, user_id)
        .await?
        .filter(|row| row.state == TotpState::Active)
    else {
        return Ok(CodeCheck::NotEnrolled);
    };
    let secret = open_secret(keys, user_id, &row)?;
    let Some(step) = matching_step(&secret, code, now)? else {
        return Ok(CodeCheck::Invalid);
    };
    if row.last_used_step.is_some_and(|last| step <= last)
        || !repo::consume_step(tx, user_id, step, now).await?
    {
        return Ok(CodeCheck::Replayed);
    }
    Ok(CodeCheck::Accepted)
}

fn open_secret(keys: &KeyRing, user_id: UserId, row: &TotpRow) -> Result<TotpSecret, TotpError> {
    let sealed = SealedSecret::from_parts(
        row.secret_ciphertext.clone(),
        &row.secret_nonce,
        row.key_version,
    )?;
    let opened = keys.open(SealPurpose::Totp, &secret_aad(user_id), &sealed)?;
    Ok(TotpSecret::from_opened(&opened)?)
}

fn matching_step(
    secret: &TotpSecret,
    code: &TotpCode,
    now: Timestamp,
) -> Result<Option<u64>, TotpError> {
    let step = time_step(now.get().unix_timestamp()).ok_or(TotpError::CodeInvalid)?;
    Ok(secret.matching_step(code, step))
}

async fn active_user_in_tx(tx: &mut WriteTx<'_>, user_id: UserId) -> Result<User, TotpError> {
    users::find_by_id_in_tx(tx, user_id)
        .await?
        .filter(|user| user.is_active)
        .ok_or(TotpError::Session(SessionError::AuthRequired))
}

fn backup_codes_response(codes: &[BackupCode], generated_at: Timestamp) -> BackupCodesResponse {
    BackupCodesResponse {
        backup_codes: codes
            .iter()
            .map(|code| code.display().expose_secret().clone())
            .collect(),
        generated_at: generated_at.to_string(),
    }
}

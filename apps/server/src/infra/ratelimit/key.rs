use std::fmt;
use std::net::IpAddr;

use sha2::{Digest, Sha256};

use super::class::Dimension;
use crate::domain::normalize::normalize;

const IDENTITY_LEN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdentityDigest([u8; IDENTITY_LEN]);

impl IdentityDigest {
    fn derive(label: &[u8], value: &[u8]) -> Self {
        let digest = Sha256::new()
            .chain_update(label)
            .chain_update([0])
            .chain_update(value)
            .finalize();
        let mut truncated = [0; IDENTITY_LEN];
        truncated.copy_from_slice(&digest[..IDENTITY_LEN]);
        Self(truncated)
    }
}

impl fmt::Debug for IdentityDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdentityDigest(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublicScope {
    Alias(IdentityDigest),
    Capability(IdentityDigest),
}

impl PublicScope {
    pub fn alias(alias: &str) -> Self {
        Self::Alias(IdentityDigest::derive(
            b"palmr:ratelimit:alias",
            alias.to_ascii_lowercase().as_bytes(),
        ))
    }

    pub fn capability(token: &str) -> Self {
        Self::Capability(IdentityDigest::derive(
            b"palmr:ratelimit:capability",
            token.as_bytes(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitPrincipal {
    Session(IdentityDigest),
    ReverseShareGrant(IdentityDigest),
}

impl RateLimitPrincipal {
    pub fn session(stable_id: &[u8]) -> Self {
        Self::Session(IdentityDigest::derive(
            b"palmr:ratelimit:session",
            stable_id,
        ))
    }

    pub fn reverse_share_grant(stable_id: &[u8]) -> Self {
        Self::ReverseShareGrant(IdentityDigest::derive(
            b"palmr:ratelimit:reverse-share-grant",
            stable_id,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalizedAccount(IdentityDigest);

impl NormalizedAccount {
    pub fn new(identifier: &str) -> Self {
        Self(IdentityDigest::derive(
            b"palmr:ratelimit:account",
            normalize(identifier).as_bytes(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MfaPendingToken(IdentityDigest);

impl MfaPendingToken {
    pub fn new(raw_token: &[u8]) -> Self {
        Self(IdentityDigest::derive(
            b"palmr:ratelimit:mfa-pending",
            raw_token,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    ResolvedIp(IpAddr),
    SessionId(IdentityDigest),
    ReverseShareGrant(IdentityDigest),
    MfaPendingToken(IdentityDigest),
    NormalizedAccount(IdentityDigest),
    IpAndPublicScope { ip: IpAddr, scope: PublicScope },
    InstanceWide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subject {
    ip: IpAddr,
    principal: Option<RateLimitPrincipal>,
    scope: Option<PublicScope>,
    account: Option<NormalizedAccount>,
    mfa_pending: Option<MfaPendingToken>,
}

impl Subject {
    pub fn new(ip: IpAddr) -> Self {
        Self {
            ip: ip.to_canonical(),
            principal: None,
            scope: None,
            account: None,
            mfa_pending: None,
        }
    }

    #[must_use]
    pub const fn with_principal(mut self, principal: Option<RateLimitPrincipal>) -> Self {
        self.principal = principal;
        self
    }

    #[must_use]
    pub const fn with_scope(mut self, scope: Option<PublicScope>) -> Self {
        self.scope = scope;
        self
    }

    #[must_use]
    pub const fn with_account(mut self, account: NormalizedAccount) -> Self {
        self.account = Some(account);
        self
    }

    #[must_use]
    pub const fn with_mfa_pending(mut self, token: MfaPendingToken) -> Self {
        self.mfa_pending = Some(token);
        self
    }

    pub(super) fn key_for(&self, dimension: Dimension) -> Option<RateLimitKey> {
        let ip = RateLimitKey::ResolvedIp(self.ip);
        match dimension {
            Dimension::ResolvedIp => Some(ip),
            Dimension::SessionOrIp => Some(match self.principal {
                Some(RateLimitPrincipal::Session(session)) => RateLimitKey::SessionId(session),
                Some(RateLimitPrincipal::ReverseShareGrant(_)) | None => ip,
            }),
            Dimension::SessionOrGrantOrIp => Some(match self.principal {
                Some(RateLimitPrincipal::Session(session)) => RateLimitKey::SessionId(session),
                Some(RateLimitPrincipal::ReverseShareGrant(grant)) => {
                    RateLimitKey::ReverseShareGrant(grant)
                }
                None => ip,
            }),
            Dimension::IpAndPublicScope => Some(match self.scope {
                Some(scope) => RateLimitKey::IpAndPublicScope { ip: self.ip, scope },
                None => ip,
            }),
            Dimension::MfaPendingOrSessionOrIp => Some(match (self.mfa_pending, self.principal) {
                (Some(MfaPendingToken(token)), _) => RateLimitKey::MfaPendingToken(token),
                (None, Some(RateLimitPrincipal::Session(session))) => {
                    RateLimitKey::SessionId(session)
                }
                (None, Some(RateLimitPrincipal::ReverseShareGrant(_)) | None) => ip,
            }),
            Dimension::NormalizedAccount => self
                .account
                .map(|NormalizedAccount(account)| RateLimitKey::NormalizedAccount(account)),
            Dimension::InstanceWide => Some(RateLimitKey::InstanceWide),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{
        MfaPendingToken, NormalizedAccount, PublicScope, RateLimitKey, RateLimitPrincipal, Subject,
    };
    use crate::infra::ratelimit::class::Dimension;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn unit_public_alias_scope_is_case_insensitive_and_distinct_from_capabilities() {
        assert_eq!(
            PublicScope::alias("Summer-Photos"),
            PublicScope::alias("summer-photos")
        );
        assert_ne!(PublicScope::alias("summer"), PublicScope::alias("winter"));
        assert_ne!(PublicScope::alias("abc"), PublicScope::capability("abc"));
        assert_ne!(
            PublicScope::capability("Token"),
            PublicScope::capability("token")
        );
    }

    #[test]
    fn unit_account_identity_uses_domain_normalization() {
        assert_eq!(
            NormalizedAccount::new("  Alice@Example.COM "),
            NormalizedAccount::new("alice@example.com")
        );
        assert_ne!(
            NormalizedAccount::new("alice@example.com"),
            NormalizedAccount::new("bob@example.com")
        );
    }

    #[test]
    fn unit_principal_identities_are_domain_separated() {
        let session = RateLimitPrincipal::session(b"0192f3a7");
        let grant = RateLimitPrincipal::reverse_share_grant(b"0192f3a7");
        let (RateLimitPrincipal::Session(left), RateLimitPrincipal::ReverseShareGrant(right)) =
            (session, grant)
        else {
            panic!("constructors produce their own variants");
        };
        assert_ne!(left, right);
    }

    #[test]
    fn unit_subject_keys_follow_dimension_fallbacks() {
        let client = ip("198.51.100.7");
        let session = RateLimitPrincipal::session(b"session-a");
        let grant = RateLimitPrincipal::reverse_share_grant(b"grant-a");
        let anonymous = Subject::new(client);
        let signed_in = anonymous.with_principal(Some(session));
        let uploader = anonymous.with_principal(Some(grant));

        assert_eq!(
            anonymous.key_for(Dimension::SessionOrIp),
            Some(RateLimitKey::ResolvedIp(client))
        );
        assert!(matches!(
            signed_in.key_for(Dimension::SessionOrIp),
            Some(RateLimitKey::SessionId(_))
        ));
        assert_eq!(
            uploader.key_for(Dimension::SessionOrIp),
            Some(RateLimitKey::ResolvedIp(client))
        );
        assert!(matches!(
            uploader.key_for(Dimension::SessionOrGrantOrIp),
            Some(RateLimitKey::ReverseShareGrant(_))
        ));
        assert_eq!(
            anonymous.key_for(Dimension::IpAndPublicScope),
            Some(RateLimitKey::ResolvedIp(client))
        );
        assert_eq!(
            anonymous
                .with_scope(Some(PublicScope::alias("abc")))
                .key_for(Dimension::IpAndPublicScope),
            Some(RateLimitKey::IpAndPublicScope {
                ip: client,
                scope: PublicScope::alias("abc"),
            })
        );
        assert_eq!(anonymous.key_for(Dimension::NormalizedAccount), None);
        assert!(matches!(
            anonymous
                .with_account(NormalizedAccount::new("alice"))
                .key_for(Dimension::NormalizedAccount),
            Some(RateLimitKey::NormalizedAccount(_))
        ));
        assert!(matches!(
            signed_in
                .with_mfa_pending(MfaPendingToken::new(b"challenge"))
                .key_for(Dimension::MfaPendingOrSessionOrIp),
            Some(RateLimitKey::MfaPendingToken(_))
        ));
        assert!(matches!(
            signed_in.key_for(Dimension::MfaPendingOrSessionOrIp),
            Some(RateLimitKey::SessionId(_))
        ));
        assert_eq!(
            anonymous.key_for(Dimension::InstanceWide),
            Some(RateLimitKey::InstanceWide)
        );
    }

    #[test]
    fn unit_ipv4_mapped_ipv6_shares_the_ipv4_bucket() {
        assert_eq!(
            Subject::new(ip("::ffff:198.51.100.7")).key_for(Dimension::ResolvedIp),
            Some(RateLimitKey::ResolvedIp(ip("198.51.100.7")))
        );
    }

    #[test]
    fn unit_key_debug_never_prints_identity_material() {
        let key = Subject::new(ip("198.51.100.7"))
            .with_account(NormalizedAccount::new("alice@example.com"))
            .key_for(Dimension::NormalizedAccount)
            .unwrap();
        let rendered = format!("{key:?}");
        assert_eq!(rendered, "NormalizedAccount(IdentityDigest(..))");
    }
}

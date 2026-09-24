use std::fmt;
use std::num::NonZeroU32;

use governor::Quota;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RateLimitClass {
    None,
    Read,
    Write,
    AdminWrite,
    AuthLogin,
    AuthTotp,
    AuthReset,
    AuthToken,
    PublicRead,
    PublicPassword,
    PublicSession,
    TransferControl,
    TransferData,
    EmailTest,
    ProviderTest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    ResolvedIp,
    SessionOrIp,
    SessionOrGrantOrIp,
    IpAndPublicScope,
    MfaPendingOrSessionOrIp,
    NormalizedAccount,
    InstanceWide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Edge,
    Deferred,
}

impl Dimension {
    pub const fn stage(self) -> Stage {
        match self {
            Self::MfaPendingOrSessionOrIp | Self::NormalizedAccount => Stage::Deferred,
            Self::ResolvedIp
            | Self::SessionOrIp
            | Self::SessionOrGrantOrIp
            | Self::IpAndPublicScope
            | Self::InstanceWide => Stage::Edge,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketSpec {
    dimension: Dimension,
    quota: Quota,
}

impl BucketSpec {
    const fn per_minute(dimension: Dimension, burst: u32) -> Self {
        Self {
            dimension,
            quota: Quota::per_minute(nonzero(burst)),
        }
    }

    const fn per_hour(dimension: Dimension, burst: u32) -> Self {
        Self {
            dimension,
            quota: Quota::per_hour(nonzero(burst)),
        }
    }

    pub const fn dimension(&self) -> Dimension {
        self.dimension
    }

    pub const fn quota(&self) -> Quota {
        self.quota
    }
}

const fn nonzero(value: u32) -> NonZeroU32 {
    match NonZeroU32::new(value) {
        Some(value) => value,
        None => panic!("a rate-limit burst must be positive"),
    }
}

const READ: [BucketSpec; 1] = [BucketSpec::per_minute(Dimension::SessionOrIp, 300)];
const WRITE: [BucketSpec; 1] = [BucketSpec::per_minute(Dimension::SessionOrIp, 120)];
const ADMIN_WRITE: [BucketSpec; 1] = [BucketSpec::per_minute(Dimension::SessionOrIp, 60)];
const AUTH_LOGIN: [BucketSpec; 1] = [BucketSpec::per_minute(Dimension::ResolvedIp, 10)];
const AUTH_TOTP: [BucketSpec; 1] = [BucketSpec::per_minute(
    Dimension::MfaPendingOrSessionOrIp,
    10,
)];
const AUTH_RESET: [BucketSpec; 2] = [
    BucketSpec::per_hour(Dimension::ResolvedIp, 3),
    BucketSpec::per_hour(Dimension::NormalizedAccount, 3),
];
const AUTH_TOKEN: [BucketSpec; 1] = [BucketSpec::per_hour(Dimension::ResolvedIp, 20)];
const PUBLIC_READ: [BucketSpec; 1] = [BucketSpec::per_minute(Dimension::IpAndPublicScope, 120)];
const PUBLIC_PASSWORD: [BucketSpec; 1] = [BucketSpec::per_minute(Dimension::IpAndPublicScope, 10)];
const PUBLIC_SESSION: [BucketSpec; 1] = [BucketSpec::per_hour(Dimension::ResolvedIp, 20)];
const TRANSFER_CONTROL: [BucketSpec; 1] =
    [BucketSpec::per_minute(Dimension::SessionOrGrantOrIp, 600)];
const EMAIL_TEST: [BucketSpec; 1] = [BucketSpec::per_hour(Dimension::InstanceWide, 5)];
const PROVIDER_TEST: [BucketSpec; 1] = [BucketSpec::per_hour(Dimension::InstanceWide, 30)];

impl RateLimitClass {
    pub const ALL: [Self; 15] = [
        Self::None,
        Self::Read,
        Self::Write,
        Self::AdminWrite,
        Self::AuthLogin,
        Self::AuthTotp,
        Self::AuthReset,
        Self::AuthToken,
        Self::PublicRead,
        Self::PublicPassword,
        Self::PublicSession,
        Self::TransferControl,
        Self::TransferData,
        Self::EmailTest,
        Self::ProviderTest,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "rl.none",
            Self::Read => "rl.read",
            Self::Write => "rl.write",
            Self::AdminWrite => "rl.admin.write",
            Self::AuthLogin => "rl.auth.login",
            Self::AuthTotp => "rl.auth.totp",
            Self::AuthReset => "rl.auth.reset",
            Self::AuthToken => "rl.auth.token",
            Self::PublicRead => "rl.public.read",
            Self::PublicPassword => "rl.public.password",
            Self::PublicSession => "rl.public.session",
            Self::TransferControl => "rl.transfer.control",
            Self::TransferData => "rl.transfer.data",
            Self::EmailTest => "rl.email.test",
            Self::ProviderTest => "rl.provider.test",
        }
    }

    pub const fn buckets(self) -> &'static [BucketSpec] {
        match self {
            Self::None | Self::TransferData => &[],
            Self::Read => &READ,
            Self::Write => &WRITE,
            Self::AdminWrite => &ADMIN_WRITE,
            Self::AuthLogin => &AUTH_LOGIN,
            Self::AuthTotp => &AUTH_TOTP,
            Self::AuthReset => &AUTH_RESET,
            Self::AuthToken => &AUTH_TOKEN,
            Self::PublicRead => &PUBLIC_READ,
            Self::PublicPassword => &PUBLIC_PASSWORD,
            Self::PublicSession => &PUBLIC_SESSION,
            Self::TransferControl => &TRANSFER_CONTROL,
            Self::EmailTest => &EMAIL_TEST,
            Self::ProviderTest => &PROVIDER_TEST,
        }
    }

    pub const fn counts_requests(self) -> bool {
        !self.buckets().is_empty()
    }

    pub fn has_deferred_buckets(self) -> bool {
        self.buckets()
            .iter()
            .any(|bucket| bucket.dimension.stage() == Stage::Deferred)
    }

    pub(super) const fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for RateLimitClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::num::NonZeroU32;

    use governor::Quota;
    use rstest::rstest;

    use super::{Dimension, RateLimitClass, Stage};

    fn burst(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    #[rstest]
    #[case::read(RateLimitClass::Read, &[(Dimension::SessionOrIp, Quota::per_minute(burst(300)))])]
    #[case::write(RateLimitClass::Write, &[(Dimension::SessionOrIp, Quota::per_minute(burst(120)))])]
    #[case::admin_write(RateLimitClass::AdminWrite, &[(Dimension::SessionOrIp, Quota::per_minute(burst(60)))])]
    #[case::auth_login(RateLimitClass::AuthLogin, &[(Dimension::ResolvedIp, Quota::per_minute(burst(10)))])]
    #[case::auth_totp(RateLimitClass::AuthTotp, &[(Dimension::MfaPendingOrSessionOrIp, Quota::per_minute(burst(10)))])]
    #[case::auth_reset(
        RateLimitClass::AuthReset,
        &[
            (Dimension::ResolvedIp, Quota::per_hour(burst(3))),
            (Dimension::NormalizedAccount, Quota::per_hour(burst(3))),
        ]
    )]
    #[case::auth_token(RateLimitClass::AuthToken, &[(Dimension::ResolvedIp, Quota::per_hour(burst(20)))])]
    #[case::public_read(RateLimitClass::PublicRead, &[(Dimension::IpAndPublicScope, Quota::per_minute(burst(120)))])]
    #[case::public_password(RateLimitClass::PublicPassword, &[(Dimension::IpAndPublicScope, Quota::per_minute(burst(10)))])]
    #[case::public_session(RateLimitClass::PublicSession, &[(Dimension::ResolvedIp, Quota::per_hour(burst(20)))])]
    #[case::transfer_control(RateLimitClass::TransferControl, &[(Dimension::SessionOrGrantOrIp, Quota::per_minute(burst(600)))])]
    #[case::email_test(RateLimitClass::EmailTest, &[(Dimension::InstanceWide, Quota::per_hour(burst(5)))])]
    #[case::provider_test(RateLimitClass::ProviderTest, &[(Dimension::InstanceWide, Quota::per_hour(burst(30)))])]
    #[case::none(RateLimitClass::None, &[])]
    #[case::transfer_data(RateLimitClass::TransferData, &[])]
    fn unit_rate_limit_catalogue_matches_api_design(
        #[case] class: RateLimitClass,
        #[case] expected: &[(Dimension, Quota)],
    ) {
        let declared: Vec<(Dimension, Quota)> = class
            .buckets()
            .iter()
            .map(|bucket| (bucket.dimension(), bucket.quota()))
            .collect();
        assert_eq!(declared, expected);
        assert_eq!(class.counts_requests(), !expected.is_empty());
    }

    #[test]
    fn unit_rate_limit_class_all_is_unique_and_indexed() {
        let labels: BTreeSet<&str> = RateLimitClass::ALL
            .iter()
            .map(|class| class.as_str())
            .collect();
        assert_eq!(labels.len(), RateLimitClass::ALL.len());
        for (position, class) in RateLimitClass::ALL.into_iter().enumerate() {
            assert_eq!(class.index(), position);
            assert_eq!(class.to_string(), class.as_str());
        }
    }

    #[test]
    fn unit_only_body_derived_dimensions_are_deferred() {
        let deferred: Vec<RateLimitClass> = RateLimitClass::ALL
            .into_iter()
            .filter(|class| class.has_deferred_buckets())
            .collect();
        assert_eq!(
            deferred,
            [RateLimitClass::AuthTotp, RateLimitClass::AuthReset]
        );
        assert_eq!(Dimension::NormalizedAccount.stage(), Stage::Deferred);
        assert_eq!(Dimension::ResolvedIp.stage(), Stage::Edge);
    }
}

use crate::features::auth::sessions::{AuthMethod, AuthenticatedPrincipal, SessionRestriction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSecurity {
    pub must_change_password: bool,
    pub has_local_password: bool,
    pub totp_enabled: bool,
}

impl AccountSecurity {
    pub const fn restriction(self, two_factor_required: bool) -> SessionRestriction {
        if self.must_change_password {
            SessionRestriction::MustChangePassword
        } else if two_factor_required && self.has_local_password && !self.totp_enabled {
            SessionRestriction::MustEnrollTotp
        } else {
            SessionRestriction::None
        }
    }
}

pub const fn proved_temporary_password(principal: &AuthenticatedPrincipal) -> bool {
    matches!(
        principal.restriction,
        SessionRestriction::MustChangePassword
    ) && matches!(
        principal.auth_method,
        AuthMethod::Password
            | AuthMethod::PasswordTotp
            | AuthMethod::PasswordBackupCode
            | AuthMethod::PasswordTrustedDevice
    )
}

#[cfg(test)]
mod tests {
    use super::{proved_temporary_password, AccountSecurity};
    use crate::domain::clock::{Clock, TestClock};
    use crate::domain::id::Id;
    use crate::domain::role::Role;
    use crate::domain::time::Timestamp;
    use crate::features::auth::sessions::{AuthMethod, AuthenticatedPrincipal, SessionRestriction};

    const fn account(
        must_change_password: bool,
        has_local_password: bool,
        totp_enabled: bool,
    ) -> AccountSecurity {
        AccountSecurity {
            must_change_password,
            has_local_password,
            totp_enabled,
        }
    }

    #[test]
    fn unit_restriction_precedence_and_scope() {
        use SessionRestriction::{MustChangePassword, MustEnrollTotp, None};
        let cases = [
            (account(true, true, false), true, MustChangePassword),
            (account(true, true, false), false, MustChangePassword),
            (account(true, true, true), true, MustChangePassword),
            (account(true, false, false), true, MustChangePassword),
            (account(false, true, false), true, MustEnrollTotp),
            (account(false, true, false), false, None),
            (account(false, true, true), true, None),
            (account(false, false, false), true, None),
            (account(false, false, true), true, None),
            (account(false, false, false), false, None),
        ];
        for (account, required, expected) in cases {
            assert_eq!(
                account.restriction(required),
                expected,
                "{account:?} with policy {required}"
            );
        }
    }

    #[test]
    fn unit_temporary_password_proof_requires_password_session() {
        let clock = TestClock::new(time::macros::datetime!(2026-09-25 12:00 UTC));
        let principal = |restriction, auth_method| AuthenticatedPrincipal {
            user_id: Id::generate(&clock),
            username: "ada".to_owned(),
            session_id: Id::generate(&clock),
            role: Role::User,
            restriction,
            last_auth_at: Timestamp::try_from(clock.now()).unwrap(),
            recent_auth: false,
            auth_method,
        };
        for method in [
            AuthMethod::Password,
            AuthMethod::PasswordTotp,
            AuthMethod::PasswordBackupCode,
            AuthMethod::PasswordTrustedDevice,
        ] {
            assert!(proved_temporary_password(&principal(
                SessionRestriction::MustChangePassword,
                method
            )));
            for restriction in [SessionRestriction::None, SessionRestriction::MustEnrollTotp] {
                assert!(!proved_temporary_password(&principal(restriction, method)));
            }
        }
        for method in [AuthMethod::External, AuthMethod::Invite, AuthMethod::Reset] {
            assert!(!proved_temporary_password(&principal(
                SessionRestriction::MustChangePassword,
                method
            )));
        }
    }
}

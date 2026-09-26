use std::fmt;

use http::Method;

use crate::features::auth::sessions::SessionRestriction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AuthClass {
    Public,
    PublicGrant,
    Setup,
    Authenticated,
    AuthenticatedRecentAuth,
    Admin,
    AdminRecentAuth,
}

impl AuthClass {
    pub const ALL: [Self; 7] = [
        Self::Public,
        Self::PublicGrant,
        Self::Setup,
        Self::Authenticated,
        Self::AuthenticatedRecentAuth,
        Self::Admin,
        Self::AdminRecentAuth,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::PublicGrant => "public+grant",
            Self::Setup => "setup",
            Self::Authenticated => "authenticated",
            Self::AuthenticatedRecentAuth => "authenticated+recent-auth",
            Self::Admin => "admin",
            Self::AdminRecentAuth => "admin+recent-auth",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsentSession {
    Reject,
    AlreadySignedOut,
}

impl AbsentSession {
    pub fn permitted_for(self, class: AuthClass, method: &Method) -> bool {
        match self {
            Self::Reject => true,
            Self::AlreadySignedOut => class == AuthClass::Authenticated && method == Method::POST,
        }
    }
}

pub const FORCED_PASSWORD_CHANGE_PATH: &str = "/api/v1/profile/password";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentAuthWaiver {
    None,
    ForcedPasswordChange,
}

impl RecentAuthWaiver {
    pub fn permitted_for(self, class: AuthClass, method: &Method, path: &str) -> bool {
        match self {
            Self::None => true,
            Self::ForcedPasswordChange => {
                class == AuthClass::AuthenticatedRecentAuth
                    && method == Method::POST
                    && path == FORCED_PASSWORD_CHANGE_PATH
            }
        }
    }

    pub const fn waives(self, restriction: SessionRestriction) -> bool {
        matches!(
            (self, restriction),
            (
                Self::ForcedPasswordChange,
                SessionRestriction::MustChangePassword
            )
        )
    }
}

impl fmt::Display for AuthClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::AuthClass;

    #[test]
    fn unit_auth_class_labels_are_the_accepted_set() {
        let labels: Vec<&str> = AuthClass::ALL.iter().map(|class| class.as_str()).collect();
        assert_eq!(
            labels,
            [
                "public",
                "public+grant",
                "setup",
                "authenticated",
                "authenticated+recent-auth",
                "admin",
                "admin+recent-auth",
            ]
        );
    }

    #[test]
    fn unit_auth_class_all_is_unique() {
        let classes: BTreeSet<AuthClass> = AuthClass::ALL.into_iter().collect();
        let labels: BTreeSet<&str> = AuthClass::ALL.iter().map(|class| class.as_str()).collect();
        assert_eq!(classes.len(), AuthClass::ALL.len());
        assert_eq!(labels.len(), AuthClass::ALL.len());
    }

    #[test]
    fn unit_auth_class_display_matches_label() {
        for class in AuthClass::ALL {
            assert_eq!(class.to_string(), class.as_str());
        }
    }
}

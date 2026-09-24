use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Admin,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidRole;

impl Role {
    pub const ALL: [Self; 2] = [Self::Admin, Self::User];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }
}

impl FromStr for Role {
    type Err = InvalidRole;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "admin" => Ok(Self::Admin),
            "user" => Ok(Self::User),
            _ => Err(InvalidRole),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for InvalidRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("role must be 'admin' or 'user'")
    }
}

impl std::error::Error for InvalidRole {}

#[cfg(test)]
mod tests {
    use super::{InvalidRole, Role};

    #[test]
    fn unit_role_closed_set() {
        assert_eq!("admin".parse(), Ok(Role::Admin));
        assert_eq!("user".parse(), Ok(Role::User));
        for role in Role::ALL {
            assert_eq!(role.as_str().parse(), Ok(role));
            assert_eq!(role.to_string(), role.as_str());
        }
        for input in [
            "",
            "Admin",
            "ADMIN",
            "User",
            " user",
            "user ",
            "owner",
            "superadmin",
            "guest",
            "0",
        ] {
            assert_eq!(input.parse::<Role>(), Err(InvalidRole), "{input:?}");
        }
        assert!(!InvalidRole.to_string().is_empty());
    }
}

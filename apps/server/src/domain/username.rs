use std::{fmt, str::FromStr};

use super::normalize::normalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Username {
    display: String,
    normalized: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUsername;

impl Username {
    pub const MIN_CHARS: usize = 3;
    pub const MAX_CHARS: usize = 64;

    pub fn parse(input: &str) -> Result<Self, InvalidUsername> {
        if (Self::MIN_CHARS..=Self::MAX_CHARS).contains(&input.chars().count()) {
            Ok(Self {
                display: input.to_owned(),
                normalized: normalize(input),
            })
        } else {
            Err(InvalidUsername)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.display
    }

    pub fn normalized(&self) -> &str {
        &self.normalized
    }
}

impl FromStr for Username {
    type Err = InvalidUsername;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

impl fmt::Display for InvalidUsername {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("username must be 3-64 characters")
    }
}

impl std::error::Error for InvalidUsername {}

#[cfg(test)]
mod tests {
    use super::{InvalidUsername, Username};

    #[test]
    fn unit_username_grammar() {
        for input in [
            "abc",
            "Daniel",
            "d.alves-96_x",
            "user name",
            "Jos\u{00e9}",
            "\u{ff24}\u{ff41}\u{ff4e}",
            "\u{1f600}\u{1f600}\u{1f600}",
        ] {
            let username = Username::parse(input).unwrap();
            assert_eq!(username.as_str(), input);
            assert_eq!(username.to_string(), input);
        }

        let longest = "\u{00e9}".repeat(64);
        assert_eq!(Username::parse(&longest).unwrap().as_str(), longest);
        assert!(Username::parse(&"a".repeat(64)).is_ok());

        for input in ["", "a", "ab", "\u{00e9}\u{00e9}"] {
            assert_eq!(Username::parse(input), Err(InvalidUsername), "{input:?}");
        }
        assert_eq!(Username::parse(&"a".repeat(65)), Err(InvalidUsername));
        assert_eq!(
            Username::parse(&"\u{00e9}".repeat(65)),
            Err(InvalidUsername)
        );
        assert!(!InvalidUsername.to_string().is_empty());

        for (input, normalized) in [
            ("Daniel", "daniel"),
            ("DANIEL", "daniel"),
            ("\u{ff24}\u{ff41}\u{ff4e}", "dan"),
            ("STRA\u{1e9e}E", "stra\u{00df}e"),
            ("STRASSE", "strasse"),
            ("J\u{030c}x", "\u{01f0}x"),
        ] {
            let username: Username = input.parse().unwrap();
            assert_eq!(username.as_str(), input);
            assert_eq!(username.normalized(), normalized, "{input:?}");
        }
    }
}

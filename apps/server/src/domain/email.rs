use std::{fmt, str::FromStr};

use super::normalize::normalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    display: String,
    normalized: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidEmail;

impl Email {
    pub const MIN_CHARS: usize = 3;
    pub const MAX_CHARS: usize = 254;

    pub fn parse(input: &str) -> Result<Self, InvalidEmail> {
        let length = input.chars().count();
        let well_formed = (Self::MIN_CHARS..=Self::MAX_CHARS).contains(&length)
            && !input.chars().any(|c| c.is_whitespace() || c.is_control())
            && matches!(
                input.split_once('@'),
                Some((local, domain)) if !local.is_empty() && !domain.is_empty() && !domain.contains('@')
            );
        if well_formed {
            Ok(Self {
                display: input.to_owned(),
                normalized: normalize(input),
            })
        } else {
            Err(InvalidEmail)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.display
    }

    pub fn normalized(&self) -> &str {
        &self.normalized
    }
}

impl FromStr for Email {
    type Err = InvalidEmail;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl fmt::Display for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

impl fmt::Display for InvalidEmail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("e-mail must be 3-254 characters with exactly one '@', text on both sides and no whitespace or control characters")
    }
}

impl std::error::Error for InvalidEmail {}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{Email, InvalidEmail};
    use crate::domain::normalize::{normalize, tests::unicode_text};

    #[test]
    fn unit_email_accepts_contract_and_preserves_display() {
        let email = Email::parse("Daniel@Example.COM").unwrap();
        assert_eq!(email.as_str(), "Daniel@Example.COM");
        assert_eq!(email.to_string(), "Daniel@Example.COM");
        assert_eq!(email.normalized(), "daniel@example.com");

        let fullwidth = Email::parse("\u{ff24}\u{ff41}\u{ff4e}@example.com").unwrap();
        assert_eq!(fullwidth.normalized(), "dan@example.com");

        let eszett = Email::parse("STRA\u{1e9e}E@example.com").unwrap();
        assert_eq!(eszett.normalized(), "stra\u{00df}e@example.com");
        assert_ne!(
            Email::parse("STRASSE@example.com").unwrap().normalized(),
            eszett.normalized()
        );

        assert_eq!(Email::parse("a@b").unwrap().as_str(), "a@b");
        let longest = format!("{}@{}", "a".repeat(126), "\u{00e9}".repeat(127));
        assert_eq!(longest.chars().count(), 254);
        assert!(Email::parse(&longest).is_ok());
        assert_eq!("X@Y".parse::<Email>().unwrap().normalized(), "x@y");
    }

    #[test]
    fn unit_email_rejects_invalid_shapes() {
        let too_long = format!("{}@{}", "a".repeat(126), "b".repeat(128));
        for input in [
            "",
            "@",
            "a@",
            "ab",
            "@ab",
            "ab@",
            "abc",
            "a@@b",
            "a@b@c",
            "a b@c",
            "a@b c",
            " a@b",
            "a@b ",
            "a@b\u{00a0}",
            "a\u{3000}@b",
            "a\t@b",
            "a@b\n",
            "a\u{0000}@b",
            "a@b\u{007f}",
            "a\u{0085}@b",
            too_long.as_str(),
        ] {
            assert_eq!(Email::parse(input), Err(InvalidEmail), "{input:?}");
        }
        assert!(!InvalidEmail.to_string().is_empty());
    }

    proptest! {
        #[test]
        fn prop_email_normalization_idempotent(
            text in unicode_text(),
            local in unicode_text(),
            domain in unicode_text(),
        ) {
            let once = normalize(&text);
            prop_assert_eq!(normalize(&once), once);

            let keep = |c: &char| !c.is_whitespace() && !c.is_control() && *c != '@';
            let local: String = format!("x{local}").chars().filter(keep).collect();
            let domain: String = format!("{domain}y").chars().filter(keep).collect();
            let candidate = format!("{local}@{domain}");
            let email = Email::parse(&candidate).unwrap();
            prop_assert_eq!(email.as_str(), candidate.as_str());
            prop_assert_eq!(email.normalized(), normalize(&candidate));
            prop_assert_eq!(normalize(email.normalized()), email.normalized());
        }
    }
}

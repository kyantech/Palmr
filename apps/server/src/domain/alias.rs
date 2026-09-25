use std::{fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Alias(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAlias;

const ALPHABET: &[u8; 38] = b"abcdefghijklmnopqrstuvwxyz0123456789_-";
const UNBIASED_LIMIT: u8 = 228;

impl Alias {
    pub const MIN_LEN: usize = 3;
    pub const MAX_LEN: usize = 64;
    pub const GENERATED_LEN: usize = 10;
    pub const PATTERN: &'static str = "^[A-Za-z0-9_-]{3,64}$";

    pub fn parse(input: &str) -> Result<Self, InvalidAlias> {
        let well_formed = (Self::MIN_LEN..=Self::MAX_LEN).contains(&input.len())
            && input
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if well_formed {
            Ok(Self(input.to_ascii_lowercase()))
        } else {
            Err(InvalidAlias)
        }
    }

    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut alias = String::with_capacity(Self::GENERATED_LEN);
        let mut entropy = [0_u8; 16];
        while alias.len() < Self::GENERATED_LEN {
            getrandom::fill(&mut entropy)?;
            alias.extend(
                entropy
                    .iter()
                    .filter_map(|&byte| symbol(byte))
                    .take(Self::GENERATED_LEN - alias.len()),
            );
        }
        Ok(Self(alias))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn symbol(byte: u8) -> Option<char> {
    (byte < UNBIASED_LIMIT).then(|| char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]))
}

impl FromStr for Alias {
    type Err = InvalidAlias;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl fmt::Display for Alias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for InvalidAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("alias must be 3-64 characters of A-Z, a-z, 0-9, '_' or '-'")
    }
}

impl std::error::Error for InvalidAlias {}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashSet};

    use proptest::prelude::*;

    use super::{symbol, Alias, InvalidAlias, ALPHABET, UNBIASED_LIMIT};

    fn is_canonical(text: &str) -> bool {
        (Alias::MIN_LEN..=Alias::MAX_LEN).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    }

    #[test]
    fn unit_alias_parse_boundaries_and_rejections() {
        assert_eq!(Alias::parse("ClienteX").unwrap().as_str(), "clientex");
        assert_eq!("MeuLink".parse::<Alias>().unwrap().to_string(), "meulink");
        assert_eq!(
            Alias::parse("CLIENTEX").unwrap(),
            Alias::parse("clientex").unwrap()
        );
        assert_eq!(Alias::parse("a_-").unwrap().as_str(), "a_-");
        assert_eq!(
            Alias::parse(&"Z".repeat(64)).unwrap().as_str(),
            "z".repeat(64)
        );

        let too_long = "a".repeat(65);
        for input in [
            "",
            "a",
            "ab",
            too_long.as_str(),
            "abc def",
            " abc",
            "abc ",
            "abc.def",
            "abc/def",
            "..%2f",
            "abc%20",
            "ab+c",
            "caf\u{00e9}",
            "\u{ff21}\u{ff22}\u{ff23}",
            "\u{0130}stanbul",
            "\u{212a}elvin",
            "ab\u{0000}",
            "abc\n",
        ] {
            assert_eq!(Alias::parse(input), Err(InvalidAlias), "{input:?}");
        }
        assert!(!InvalidAlias.to_string().is_empty());
    }

    #[test]
    fn unit_alias_pattern_matches_the_parse_rule() {
        assert_eq!(
            Alias::PATTERN,
            format!("^[A-Za-z0-9_-]{{{},{}}}$", Alias::MIN_LEN, Alias::MAX_LEN)
        );
    }

    #[test]
    fn unit_alias_generator_valid() {
        let mut seen = HashSet::new();
        let mut symbols = BTreeSet::new();
        for _ in 0..2_000 {
            let alias = Alias::generate().unwrap();
            let text = alias.as_str();
            assert_eq!(text.len(), Alias::GENERATED_LEN);
            assert!(is_canonical(text), "{text:?}");
            assert_eq!(Alias::parse(text), Ok(alias.clone()));
            symbols.extend(text.bytes());
            assert!(seen.insert(text.to_owned()), "{text:?}");
        }
        assert_eq!(symbols, ALPHABET.iter().copied().collect());

        let mut counts = BTreeMap::new();
        for byte in 0..=u8::MAX {
            match symbol(byte) {
                Some(c) => *counts.entry(c).or_insert(0) += 1,
                None => assert!(byte >= UNBIASED_LIMIT),
            }
        }
        assert_eq!(counts.len(), ALPHABET.len());
        assert!(counts.values().all(|&count| count == 6));
        assert_eq!(usize::from(UNBIASED_LIMIT), ALPHABET.len() * 6);
    }

    proptest! {
        #[test]
        fn prop_alias_canonical_lowercase_and_grammar(
            input in "[A-Za-z0-9_-]{3,64}",
            noise in any::<String>(),
        ) {
            let alias = Alias::parse(&input).unwrap();
            prop_assert!(is_canonical(alias.as_str()));
            prop_assert_eq!(alias.as_str(), input.to_ascii_lowercase());
            prop_assert!(alias.as_str().eq_ignore_ascii_case(&input));
            prop_assert_eq!(Alias::parse(alias.as_str()), Ok(alias.clone()));
            prop_assert_eq!(Alias::parse(&input.to_ascii_uppercase()), Ok(alias.clone()));

            match Alias::parse(&noise) {
                Ok(parsed) => {
                    prop_assert!(is_canonical(parsed.as_str()));
                    prop_assert_eq!(parsed.as_str(), noise.to_ascii_lowercase());
                }
                Err(InvalidAlias) => prop_assert!(!is_canonical(&noise.to_ascii_lowercase())),
            }
        }
    }
}

use std::fmt;

pub const REDACTED: &str = "<redacted>";

// Plaintext leaves only through `expose_secret`: no `Deref`, `From`/`Into`,
// `AsRef` or `Serialize` impl exists, so formatting, logging and serializing
// a `Secret` can never emit the inner value (SECURITY_MODEL §6.4).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    pub const fn expose_secret(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::{Secret, REDACTED};

    const SENTINEL: &str = "palmr-secret-sentinel-7c1e";

    #[derive(Debug)]
    #[expect(dead_code, reason = "fields are read only through Debug")]
    struct Holder {
        name: &'static str,
        token: Secret<String>,
        optional: Option<Secret<String>>,
        list: Vec<Secret<&'static str>>,
    }

    #[test]
    fn unit_secret_debug_redacted() {
        let secret = Secret::new(SENTINEL.to_owned());
        let holder = Holder {
            name: "holder",
            token: secret.clone(),
            optional: Some(secret.clone()),
            list: vec![Secret::new(SENTINEL)],
        };

        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(format!("{secret:#?}"), REDACTED);
        for rendered in [format!("{holder:?}"), format!("{holder:#?}")] {
            assert!(!rendered.contains(SENTINEL));
            assert_eq!(rendered.matches(REDACTED).count(), 3);
        }
    }

    #[test]
    fn unit_secret_display_redacted() {
        let secret = Secret::new(SENTINEL.to_owned());

        assert_eq!(format!("{secret}"), REDACTED);
        assert_eq!(secret.to_string(), REDACTED);
        assert_eq!(format!("{secret:>40}"), REDACTED);
        assert!(!format!("{:?}", Some(&secret)).contains(SENTINEL));
    }

    #[test]
    fn unit_secret_expose_secret_returns_inner_value() {
        let secret = Secret::new(SENTINEL.to_owned());

        assert_eq!(secret.expose_secret(), SENTINEL);
        assert_eq!(secret.clone(), secret);
        assert_ne!(secret, Secret::new(String::new()));
    }
}

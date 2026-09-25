use std::fmt;

use http::header::{COOKIE, SET_COOKIE};
use http::{HeaderMap, HeaderValue};

use crate::config::PublicBaseUrl;
use crate::domain::secret::Secret;

pub const SESSION_COOKIE: &str = "palmr_session";
pub const CSRF_COOKIE: &str = "palmr_csrf";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookiePolicy {
    secure: bool,
}

impl CookiePolicy {
    pub fn from_base_url(base_url: &PublicBaseUrl) -> Self {
        Self {
            secure: base_url.url().scheme() == "https",
        }
    }

    pub fn append_session_pair(
        self,
        headers: &mut HeaderMap,
        session: &Secret<String>,
        csrf: &Secret<String>,
        max_age_seconds: u64,
    ) -> Result<(), CookieError> {
        append_cookie(
            headers,
            SESSION_COOKIE,
            session.expose_secret(),
            true,
            self.secure,
            Some(max_age_seconds),
        )?;
        append_cookie(
            headers,
            CSRF_COOKIE,
            csrf.expose_secret(),
            false,
            self.secure,
            Some(max_age_seconds),
        )
    }

    pub fn expire_session_pair(self, headers: &mut HeaderMap) -> Result<(), CookieError> {
        append_cookie(headers, SESSION_COOKIE, "", true, self.secure, Some(0))?;
        append_cookie(headers, CSRF_COOKIE, "", false, self.secure, Some(0))
    }

    pub fn append_anonymous_csrf(
        self,
        headers: &mut HeaderMap,
        csrf: &Secret<String>,
    ) -> Result<(), CookieError> {
        append_cookie(
            headers,
            CSRF_COOKIE,
            csrf.expose_secret(),
            false,
            self.secure,
            None,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieError {
    InvalidValue,
    Duplicate,
}

impl fmt::Display for CookieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidValue => "cookie value cannot be represented as an HTTP header",
            Self::Duplicate => "request carries the same cookie more than once",
        })
    }
}

impl std::error::Error for CookieError {}

pub fn read(headers: &HeaderMap, name: &str) -> Result<Option<String>, CookieError> {
    let mut found = None;
    for header in headers.get_all(COOKIE) {
        let line = header.to_str().map_err(|_| CookieError::InvalidValue)?;
        for pair in line.split(';') {
            let Some((candidate, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if candidate.trim() != name {
                continue;
            }
            if found.is_some() {
                return Err(CookieError::Duplicate);
            }
            found = Some(value.trim().to_owned());
        }
    }
    Ok(found)
}

pub fn presents(headers: &HeaderMap, name: &str) -> bool {
    !matches!(read(headers, name), Ok(None))
}

pub fn sets(headers: &HeaderMap, name: &str) -> bool {
    headers.get_all(SET_COOKIE).iter().any(|value| {
        value
            .as_bytes()
            .split(|byte| *byte == b'=')
            .next()
            .is_some_and(|candidate| candidate == name.as_bytes())
    })
}

pub fn append_header_value(headers: &mut HeaderMap, value: HeaderValue) {
    headers.append(SET_COOKIE, value);
}

fn append_cookie(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
    http_only: bool,
    secure: bool,
    max_age_seconds: Option<u64>,
) -> Result<(), CookieError> {
    let mut rendered = format!("{name}={value}; Path=/; SameSite=Lax");
    if let Some(max_age) = max_age_seconds {
        rendered.push_str("; Max-Age=");
        rendered.push_str(&max_age.to_string());
    }
    if secure {
        rendered.push_str("; Secure");
    }
    if http_only {
        rendered.push_str("; HttpOnly");
    }
    let value = HeaderValue::from_str(&rendered).map_err(|_| CookieError::InvalidValue)?;
    append_header_value(headers, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use http::header::{HeaderValue, COOKIE, SET_COOKIE};
    use http::HeaderMap;

    use super::{read, CookieError, CookiePolicy, CSRF_COOKIE, SESSION_COOKIE};
    use crate::config::{EnvironmentSource, OperatorConfig};
    use crate::domain::secret::Secret;

    fn policy(base_url: &str) -> CookiePolicy {
        let config = OperatorConfig::load(&EnvironmentSource::from_vars([(
            "PALMR_BASE_URL",
            base_url,
        )]))
        .unwrap()
        .config;
        CookiePolicy::from_base_url(&config.base_url)
    }

    #[test]
    #[allow(non_snake_case, reason = "the accepted regression identifier is R-092")]
    fn regression_R092_multiple_set_cookie_headers() {
        let mut headers = HeaderMap::new();
        policy("https://files.example.test")
            .append_session_pair(
                &mut headers,
                &Secret::new("session-token".to_owned()),
                &Secret::new("csrf-token".to_owned()),
                600,
            )
            .unwrap();

        let values: Vec<&str> = headers
            .get_all(SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(values.len(), 2);
        assert!(values[0].starts_with("palmr_session=session-token;"));
        assert!(values[0].contains("; HttpOnly"));
        assert!(values[0].contains("; Secure"));
        assert!(values[1].starts_with("palmr_csrf=csrf-token;"));
        assert!(!values[1].contains("HttpOnly"));
        assert!(values[1].contains("; Secure"));
        assert!(!values.iter().any(|value| value.contains(", palmr_")));
    }

    #[test]
    fn unit_cookie_secure_comes_only_from_base_url() {
        let mut https = HeaderMap::new();
        policy("https://files.example.test")
            .append_session_pair(
                &mut https,
                &Secret::new("one".to_owned()),
                &Secret::new("two".to_owned()),
                1,
            )
            .unwrap();
        assert!(https
            .get_all(SET_COOKIE)
            .iter()
            .all(|value| value.to_str().unwrap().contains("; Secure")));

        let mut http = HeaderMap::new();
        policy("http://localhost:5487")
            .append_session_pair(
                &mut http,
                &Secret::new("one".to_owned()),
                &Secret::new("two".to_owned()),
                1,
            )
            .unwrap();
        assert!(http
            .get_all(SET_COOKIE)
            .iter()
            .all(|value| !value.to_str().unwrap().contains("; Secure")));
    }

    #[test]
    fn unit_cookie_reader_fails_closed_on_duplicates() {
        let mut headers = HeaderMap::new();
        headers.append(
            COOKIE,
            HeaderValue::from_static("other=x; palmr_session=one"),
        );
        assert_eq!(
            read(&headers, SESSION_COOKIE).unwrap().as_deref(),
            Some("one")
        );
        assert_eq!(read(&headers, CSRF_COOKIE).unwrap(), None);

        headers.append(COOKIE, HeaderValue::from_static("palmr_session=two"));
        assert_eq!(read(&headers, SESSION_COOKIE), Err(CookieError::Duplicate));
    }
}

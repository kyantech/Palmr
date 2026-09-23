use std::fmt;

use url::Url;

// The query string of a presigned URL is the capability itself, so only the
// host and path are ever rendered. Input that does not parse as a URL with a
// host renders a fixed marker, never the raw text (ARCHITECTURE §14.3).
pub struct RedactedPresignedUrl<'a>(&'a str);

impl<'a> RedactedPresignedUrl<'a> {
    pub const fn new(url: &'a str) -> Self {
        Self(url)
    }
}

impl fmt::Display for RedactedPresignedUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Ok(url) = Url::parse(self.0) else {
            return f.write_str("<presigned:invalid>");
        };
        let Some(host) = url.host_str() else {
            return f.write_str("<presigned:invalid>");
        };
        match url.port() {
            Some(port) => write!(f, "<presigned:{host}:{port}{}>", url.path()),
            None => write!(f, "<presigned:{host}{}>", url.path()),
        }
    }
}

impl fmt::Debug for RedactedPresignedUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::RedactedPresignedUrl;

    const SIGNATURE: &str = "f00dfacecafe5ea1ed5160a7e2e";

    #[rstest]
    #[case::virtual_host(
        "https://storage.example.com/bucket/object?X-Amz-Signature=f00dfacecafe5ea1ed5160a7e2e&X-Amz-Credential=AKIA%2F20260101&X-Amz-Expires=900",
        "<presigned:storage.example.com/bucket/object>"
    )]
    #[case::explicit_port(
        "http://minio:9000/palmr/objects/ab/cd?X-Amz-Signature=f00dfacecafe5ea1ed5160a7e2e",
        "<presigned:minio:9000/palmr/objects/ab/cd>"
    )]
    #[case::userinfo_and_fragment(
        "https://AKIA:f00dfacecafe5ea1ed5160a7e2e@storage.example.com/b/o#f00dfacecafe5ea1ed5160a7e2e",
        "<presigned:storage.example.com/b/o>"
    )]
    #[case::ipv6_host(
        "https://[::1]:8443/b/o?sig=f00dfacecafe5ea1ed5160a7e2e",
        "<presigned:[::1]:8443/b/o>"
    )]
    #[case::percent_encoded_path(
        "https://s3.example.com/b/a%20b%0Ac?sig=f00dfacecafe5ea1ed5160a7e2e",
        "<presigned:s3.example.com/b/a%20b%0Ac>"
    )]
    fn unit_presigned_url_renders_host_and_path_only(#[case] url: &str, #[case] expected: &str) {
        let redacted = RedactedPresignedUrl::new(url);

        assert_eq!(redacted.to_string(), expected);
        assert_eq!(format!("{redacted:?}"), expected);
        assert!(!redacted.to_string().contains(SIGNATURE));
    }

    #[rstest]
    #[case::no_scheme("storage.example.com/b/o?X-Amz-Signature=f00dfacecafe5ea1ed5160a7e2e")]
    #[case::no_host("mailto:f00dfacecafe5ea1ed5160a7e2e@example.com")]
    #[case::garbage("f00dfacecafe5ea1ed5160a7e2e")]
    #[case::empty("")]
    #[case::bad_port("https://example.com:99999/b?sig=f00dfacecafe5ea1ed5160a7e2e")]
    fn unit_presigned_url_malformed_never_echoes_input(#[case] url: &str) {
        let redacted = RedactedPresignedUrl::new(url);

        assert_eq!(redacted.to_string(), "<presigned:invalid>");
        assert_eq!(format!("{redacted:?}"), "<presigned:invalid>");
    }
}

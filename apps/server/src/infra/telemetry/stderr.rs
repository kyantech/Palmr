use std::fmt;
use std::io::{self, Write};

pub fn write_startup_failure(out: &mut impl Write, failure: &dyn fmt::Display) -> io::Result<()> {
    let message: String = failure
        .to_string()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    writeln!(out, "FATAL: {message}")
}

#[cfg(test)]
mod tests {
    use crate::config::{EnvironmentSource, OperatorConfig};
    use crate::domain::secret::Secret;
    use crate::infra::telemetry::write_startup_failure;

    const ACCESS_KEY: &str = "AKIA-stderr-access-sentinel";
    const SECRET_KEY: &str = "stderr-secret-sentinel-51d0";

    fn render(failure: &dyn std::fmt::Display) -> String {
        let mut out = Vec::new();
        write_startup_failure(&mut out, failure).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn unit_startup_failure_is_one_fixed_format_line() {
        let line = render(&"STARTUP_EXAMPLE: first\nsecond\r\tthird");

        assert_eq!(line, "FATAL: STARTUP_EXAMPLE: first second  third\n");
    }

    #[test]
    fn unit_startup_failure_config_error_never_prints_secrets() {
        let error = OperatorConfig::load(&EnvironmentSource::from_vars([
            ("PALMR_PORT", "0"),
            ("PALMR_STORAGE_PROVIDER", "s3"),
            (
                "PALMR_S3_ENDPOINT",
                "https://AKIA-stderr-access-sentinel:stderr-secret-sentinel-51d0@minio:9000",
            ),
            ("PALMR_S3_REGION", "us-east-1"),
            ("PALMR_S3_BUCKET", "palmr"),
            ("PALMR_S3_ACCESS_KEY", ACCESS_KEY),
            ("PALMR_S3_SECRET_KEY", SECRET_KEY),
        ]))
        .unwrap_err();
        let line = render(&error);

        assert!(line.starts_with("FATAL: STARTUP_CONFIG_INVALID: "));
        assert!(line.contains("PALMR_PORT=\"0\""));
        assert!(line.contains("PALMR_S3_ENDPOINT (value withheld)"));
        assert_eq!(line.matches('\n').count(), 1);
        assert!(!line.contains(ACCESS_KEY));
        assert!(!line.contains(SECRET_KEY));
    }

    #[test]
    fn unit_startup_failure_secret_renders_redacted() {
        let line = render(&Secret::new(SECRET_KEY));

        assert_eq!(line, "FATAL: <redacted>\n");
    }
}

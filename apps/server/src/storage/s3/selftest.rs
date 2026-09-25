use std::time::Duration;

use async_trait::async_trait;
use http::header::{
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, DATE,
    ETAG,
};
use http::{HeaderMap, HeaderValue, Method};
use time::format_description::well_known::Rfc2822;
use time::OffsetDateTime;

use super::multipart::{provider_page, UploadTarget};
use super::probe_http::{ProbeFailure, ProbeResponse};
use super::{classify, service_code, Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::health::{
    compare, diagnose, remove_and_verify, skip_removal, sweep_stale, write_and_verify, CheckName,
    CheckStatus, Comparison, Diagnosis, Fact, FactReport, ListedProbe, ProbeDepth, ProbeKey,
    ProbePattern, ProbeRun, ProbeStore, SelfTestReport, PROBE_BYTES, PROBE_PREFIX,
};
use crate::storage::provider::{
    ETag, ObjectBody, ObjectStat, PartPlanEntry, PresignedRequest, PutHint, UploadedPart,
};
use crate::storage::s3::profile::MIB;
use crate::storage::ProviderKind;

pub const PROBE_PART_BYTES: u64 = 5 * MIB;
pub const PROBE_TAIL_BYTES: u64 = 1024;
pub const PROBE_MULTIPART_BYTES: u64 = 2 * PROBE_PART_BYTES + PROBE_TAIL_BYTES;

const PRESIGN_TTL: Duration = Duration::from_secs(60);
const MULTIPART_TIMEOUT: Duration = Duration::from_secs(180);
const PROBE_CONTENT_TYPE: &str = "application/octet-stream";
const PROBE_DISPOSITION: &str = "attachment";
const CLOCK_TOLERANCE: time::Duration = time::Duration::minutes(15);
const MAX_ERROR_CODE_LEN: usize = 64;

const CAPABILITY_CHECKS: [CheckName; 5] = [
    CheckName::PresignedGet,
    CheckName::PresignedPart,
    CheckName::CorsPreflight,
    CheckName::Multipart,
    CheckName::ListMultipartUploads,
];

pub(super) struct Preflights {
    pub(super) put: ProbeResponse,
    pub(super) get: ProbeResponse,
}

enum Direct {
    Accepted(UploadedPart, HeaderMap),
    Rejected(Diagnosis),
    Unreachable,
    Unsigned(Diagnosis),
}

impl S3Provider {
    pub(super) async fn run_self_test(&self, depth: ProbeDepth) -> SelfTestReport {
        let mut run = ProbeRun::new(self.clock.as_ref(), ProviderKind::S3, depth);
        if depth == ProbeDepth::Full {
            run.run(CheckName::HeadBucket, async {
                self.head_bucket()
                    .await
                    .map_err(|error| bucket_diagnosis(&error))
            })
            .await;
        }
        let key = ProbeKey::generate();
        let written = write_and_verify(&mut run, self, &key).await;
        let mut multipart_key = None;
        if depth == ProbeDepth::Full {
            if written && run.core_intact() {
                self.presigned_get_probe(&mut run, &key).await;
                multipart_key = Some(self.multipart_probe(&mut run).await);
            } else {
                for name in CAPABILITY_CHECKS {
                    run.skip(name);
                }
            }
        }
        if written {
            remove_and_verify(&mut run, self, &key).await;
        } else {
            skip_removal(&mut run);
        }
        if depth == ProbeDepth::Full {
            let mut in_use = vec![&key];
            in_use.extend(multipart_key.as_ref());
            sweep_stale(&mut run, self, &in_use).await;
        }
        run.finish()
    }

    async fn head_bucket(&self) -> Result<(), StorageError> {
        self.internal()
            .head_bucket()
            .bucket(self.bucket())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| classify(Operation::HeadBucket, error))
    }

    async fn presigned_get_probe(&self, run: &mut ProbeRun<'_>, key: &ProbeKey) {
        let started = run.started();
        let signed_at = self.clock.now();
        let outcome = match self
            .sign_get(
                key,
                PRESIGN_TTL,
                &HeaderValue::from_static(PROBE_DISPOSITION),
                PROBE_CONTENT_TYPE,
                signed_at,
            )
            .await
        {
            Ok(signed) => match self
                .public_probe
                .execute(&signed, &self.browser_origin, None)
                .await
            {
                Ok(response) if response.status.is_success() => {
                    let body: ObjectBody = Box::pin(std::io::Cursor::new(response.body));
                    match compare(body, key.seed(), 0, PROBE_BYTES).await {
                        Ok(Comparison::Equal) => (CheckStatus::Passed, None),
                        _ => (CheckStatus::Failed, Some(Diagnosis::ContentMismatch)),
                    }
                }
                Ok(response) => (
                    CheckStatus::Failed,
                    Some(presigned_failure(&response, signed_at)),
                ),
                Err(ProbeFailure::Unreachable) => (
                    CheckStatus::Info,
                    Some(Diagnosis::PublicEndpointUnreachable),
                ),
                Err(ProbeFailure::Refused) => (CheckStatus::Failed, Some(Diagnosis::Config)),
            },
            Err(error) => (CheckStatus::Failed, Some(diagnose(&error))),
        };
        let duration = run.elapsed(started);
        run.record(CheckName::PresignedGet, outcome.0, outcome.1, duration);
        run.fact(FactReport {
            fact: Fact::AddressingStyle,
            assumed: Some(true),
            verified: match outcome {
                (CheckStatus::Passed, _) => Some(true),
                (_, Some(Diagnosis::AddressingStyleMismatch)) => Some(false),
                _ => None,
            },
            applied: false,
        });
    }

    async fn multipart_probe(&self, run: &mut ProbeRun<'_>) -> ProbeKey {
        let key = ProbeKey::generate();
        let started = run.started();
        let hint = PutHint {
            declared_len: Some(PROBE_MULTIPART_BYTES),
            content_type: Some(PROBE_CONTENT_TYPE.to_owned()),
        };
        let upload_id =
            match tokio::time::timeout(MULTIPART_TIMEOUT, self.create_upload(&key, hint)).await {
                Ok(Ok(upload_id)) => upload_id,
                Ok(Err(error)) => {
                    self.multipart_failed(run, started, &error);
                    return key;
                }
                Err(_) => {
                    self.multipart_failed(run, started, &unreachable_error());
                    return key;
                }
            };
        let target = UploadTarget::probe(&key, &upload_id);
        let completed = tokio::time::timeout(
            MULTIPART_TIMEOUT,
            self.exercise_multipart(run, target, &key),
        )
        .await;
        let outcome = match completed {
            Ok(Ok(stat)) if stat.size == PROBE_MULTIPART_BYTES => Ok(()),
            Ok(Ok(_)) => Err(Diagnosis::SizeMismatch),
            Ok(Err(error)) => Err(multipart_diagnosis(&error)),
            Err(_) => Err(Diagnosis::Unreachable),
        };
        if outcome.is_err() {
            let _ = self.abort_upload(target).await;
        }
        let _ = self.delete_object(&key).await;
        let duration = run.elapsed(started);
        match outcome {
            Ok(()) => run.record(CheckName::Multipart, CheckStatus::Passed, None, duration),
            Err(diagnosis) => run.record(
                CheckName::Multipart,
                CheckStatus::Failed,
                Some(diagnosis),
                duration,
            ),
        }
        run.fact(FactReport {
            fact: Fact::MultipartCompletion,
            assumed: Some(true),
            verified: Some(outcome.is_ok()),
            applied: false,
        });
        key
    }

    async fn exercise_multipart(
        &self,
        run: &mut ProbeRun<'_>,
        target: UploadTarget<'_>,
        key: &ProbeKey,
    ) -> Result<ObjectStat, StorageError> {
        let seed = key.seed();
        let direct = self.direct_part(run, target, seed).await;
        let first = match direct {
            Some(part) => part,
            None => {
                self.upload_part_body(
                    target,
                    1,
                    PROBE_PART_BYTES,
                    ProbePattern::new(seed, 0, PROBE_PART_BYTES).body(),
                )
                .await?
            }
        };
        let second = self
            .upload_part_body(
                target,
                2,
                PROBE_PART_BYTES,
                ProbePattern::new(seed, PROBE_PART_BYTES, PROBE_PART_BYTES).body(),
            )
            .await?;
        let third = self
            .upload_part_body(
                target,
                3,
                PROBE_TAIL_BYTES,
                ProbePattern::new(seed, 2 * PROBE_PART_BYTES, PROBE_TAIL_BYTES).body(),
            )
            .await?;
        self.list_uploads_probe(run, target).await;
        self.complete_upload(target, &[first, second, third]).await
    }

    async fn direct_part(
        &self,
        run: &mut ProbeRun<'_>,
        target: UploadTarget<'_>,
        seed: u64,
    ) -> Option<UploadedPart> {
        let started = run.started();
        let signed_at = self.clock.now();
        let part = PartPlanEntry {
            part_number: 1,
            len: PROBE_PART_BYTES,
        };
        let signed = self
            .sign_parts(target, &[part], PRESIGN_TTL, signed_at)
            .await
            .ok()
            .and_then(|mut signed| signed.pop());
        let (preflights, direct) = match &signed {
            Some(request) => {
                let preflights = self.preflights(request).await;
                let body = ProbePattern::new(seed, 0, PROBE_PART_BYTES).body();
                let direct = match self
                    .public_probe
                    .execute(
                        request,
                        &self.browser_origin,
                        Some((body, PROBE_PART_BYTES)),
                    )
                    .await
                {
                    Ok(response) => accepted_part(response, signed_at),
                    Err(ProbeFailure::Unreachable) => Direct::Unreachable,
                    Err(ProbeFailure::Refused) => Direct::Rejected(Diagnosis::Config),
                };
                (preflights, direct)
            }
            None => (
                Err(ProbeFailure::Refused),
                Direct::Unsigned(Diagnosis::PresignRejected),
            ),
        };
        let duration = run.elapsed(started);
        let (status, diagnosis) = match &direct {
            Direct::Accepted(..) => (CheckStatus::Passed, None),
            Direct::Rejected(diagnosis) | Direct::Unsigned(diagnosis) => {
                (CheckStatus::Failed, Some(*diagnosis))
            }
            Direct::Unreachable => (
                CheckStatus::Info,
                Some(Diagnosis::PublicEndpointUnreachable),
            ),
        };
        run.record(CheckName::PresignedPart, status, diagnosis, duration);

        let actual = match &direct {
            Direct::Accepted(_, headers) => Some(headers),
            _ => None,
        };
        let (status, diagnosis) = match &preflights {
            Ok(preflights) => cors_verdict(&self.browser_origin, preflights, actual),
            Err(ProbeFailure::Unreachable) => (
                CheckStatus::Info,
                Some(Diagnosis::PublicEndpointUnreachable),
            ),
            Err(ProbeFailure::Refused) => (CheckStatus::Skipped, None),
        };
        run.record(CheckName::CorsPreflight, status, diagnosis, Duration::ZERO);
        run.fact(FactReport {
            fact: Fact::BucketCors,
            assumed: None,
            verified: match status {
                CheckStatus::Passed | CheckStatus::Warning => Some(true),
                CheckStatus::Failed => Some(false),
                CheckStatus::Info | CheckStatus::Skipped => None,
            },
            applied: false,
        });

        let accepted = matches!(direct, Direct::Accepted(..));
        let applied = accepted && self.relax_checksum_requirement();
        if applied {
            tracing::info!(
                profile = self.clients.shared().profile().as_str(),
                "the provider accepted a browser-shaped part upload without checksum headers; parts go directly from the browser for this process"
            );
        }
        run.fact(FactReport {
            fact: Fact::RequiresChecksumHeaders,
            assumed: Some(self.caps.requires_checksum_headers),
            verified: accepted.then_some(false),
            applied: self.effective_caps() != &self.caps,
        });
        match direct {
            Direct::Accepted(part, _) => Some(part),
            Direct::Rejected(_) | Direct::Unreachable | Direct::Unsigned(_) => None,
        }
    }

    async fn preflights(&self, request: &PresignedRequest) -> Result<Preflights, ProbeFailure> {
        let origin = &self.browser_origin;
        let put = self
            .public_probe
            .preflight(request, origin, &Method::PUT)
            .await?;
        let get = self
            .public_probe
            .preflight(request, origin, &Method::GET)
            .await?;
        Ok(Preflights { put, get })
    }

    async fn list_uploads_probe(&self, run: &mut ProbeRun<'_>, target: UploadTarget<'_>) {
        let started = run.started();
        let listed = self
            .internal()
            .list_multipart_uploads()
            .bucket(self.bucket())
            .prefix(target.key().stored_key())
            .max_uploads(provider_page())
            .send()
            .await;
        let (status, diagnosis, verified) = match listed {
            Ok(output)
                if output
                    .uploads()
                    .iter()
                    .any(|upload| upload.upload_id() == Some(target.upload_id())) =>
            {
                (CheckStatus::Passed, None, Some(true))
            }
            Ok(_) => (
                CheckStatus::Warning,
                Some(Diagnosis::ListMultipartUploadsUnverified),
                None,
            ),
            Err(_) => (
                CheckStatus::Warning,
                Some(Diagnosis::ListMultipartUploadsUnsupported),
                Some(false),
            ),
        };
        let duration = run.elapsed(started);
        run.record(CheckName::ListMultipartUploads, status, diagnosis, duration);
        run.fact(FactReport {
            fact: Fact::ListMultipartUploads,
            assumed: Some(true),
            verified,
            applied: false,
        });
    }

    fn multipart_failed(
        &self,
        run: &mut ProbeRun<'_>,
        started: std::time::Instant,
        error: &StorageError,
    ) {
        let duration = run.elapsed(started);
        for name in [
            CheckName::PresignedPart,
            CheckName::CorsPreflight,
            CheckName::ListMultipartUploads,
        ] {
            run.skip(name);
        }
        run.record(
            CheckName::Multipart,
            CheckStatus::Failed,
            Some(multipart_diagnosis(error)),
            duration,
        );
    }
}

#[async_trait]
impl ProbeStore for S3Provider {
    async fn put_probe(
        &self,
        key: &ProbeKey,
        body: ObjectBody,
        len: u64,
    ) -> Result<ObjectStat, StorageError> {
        self.put_object_single(key, body, len, Some(PROBE_CONTENT_TYPE))
            .await
    }

    async fn stat_probe(&self, key: &ProbeKey) -> Result<ObjectStat, StorageError> {
        self.head_object(key).await
    }

    async fn read_probe(&self, key: &ProbeKey) -> Result<ObjectBody, StorageError> {
        self.get_object(key).await.map(|(_, body)| body)
    }

    async fn read_probe_range(
        &self,
        key: &ProbeKey,
        start: u64,
        len: u64,
    ) -> Result<ObjectBody, StorageError> {
        self.get_object_range(key, start, len)
            .await
            .map(|(_, body)| body)
    }

    async fn delete_probe(&self, key: &ProbeKey) -> Result<(), StorageError> {
        self.delete_object(key).await
    }

    async fn probe_exists(&self, key: &ProbeKey) -> Result<bool, StorageError> {
        self.object_exists(key).await
    }

    async fn list_probes(&self, limit: u32) -> Result<Vec<ListedProbe>, StorageError> {
        let page = self.list_objects_page(PROBE_PREFIX, None, limit).await?;
        Ok(page
            .entries
            .into_iter()
            .filter_map(|entry| {
                ProbeKey::from_listed(&entry.key).map(|key| ListedProbe {
                    key,
                    modified_at: entry.modified_at,
                })
            })
            .collect())
    }

    fn diagnose(&self, error: &StorageError) -> Diagnosis {
        match service_code(error) {
            Some("RequestTimeTooSkewed") => Diagnosis::ClockSkew,
            Some("NoSuchBucket") => Diagnosis::BucketMissing,
            _ => diagnose(error),
        }
    }
}

pub(super) fn cors_verdict(
    browser_origin: &str,
    preflights: &Preflights,
    actual: Option<&HeaderMap>,
) -> (CheckStatus, Option<Diagnosis>) {
    let responses = [&preflights.put, &preflights.get];
    if responses
        .iter()
        .any(|response| !response.status.is_success())
    {
        return (CheckStatus::Failed, Some(Diagnosis::CorsPreflightRejected));
    }
    let mut wildcard = false;
    for response in responses {
        match header_text(&response.headers, &ACCESS_CONTROL_ALLOW_ORIGIN) {
            Some("*") => wildcard = true,
            Some(origin) if origin.eq_ignore_ascii_case(browser_origin) => {}
            _ => return (CheckStatus::Failed, Some(Diagnosis::CorsOriginMismatch)),
        }
    }
    let methods: Vec<String> = responses
        .iter()
        .flat_map(|response| tokens(&response.headers, &ACCESS_CONTROL_ALLOW_METHODS))
        .collect();
    let allows = |method: &str| {
        methods
            .iter()
            .any(|allowed| allowed == "*" || allowed.eq_ignore_ascii_case(method))
    };
    if !(allows("PUT") && allows("GET")) {
        return (CheckStatus::Failed, Some(Diagnosis::CorsMethodsMissing));
    }
    let exposes_etag = responses
        .iter()
        .map(|response| &response.headers)
        .chain(actual)
        .flat_map(|headers| tokens(headers, &ACCESS_CONTROL_EXPOSE_HEADERS))
        .any(|name| name == "*" || name.eq_ignore_ascii_case(ETAG.as_str()));
    if !exposes_etag {
        return (CheckStatus::Failed, Some(Diagnosis::CorsMissingEtag));
    }
    if wildcard {
        (CheckStatus::Warning, Some(Diagnosis::CorsWildcardOrigin))
    } else {
        (CheckStatus::Passed, None)
    }
}

pub(super) fn presigned_failure(response: &ProbeResponse, signed_at: OffsetDateTime) -> Diagnosis {
    match error_code(&response.body).as_deref() {
        Some("RequestTimeTooSkewed") => Diagnosis::ClockSkew,
        Some("SignatureDoesNotMatch") => Diagnosis::AddressingStyleMismatch,
        _ if provider_clock_skewed(&response.headers, signed_at) => Diagnosis::ClockSkew,
        _ => Diagnosis::PresignRejected,
    }
}

fn accepted_part(response: ProbeResponse, signed_at: OffsetDateTime) -> Direct {
    if !response.status.is_success() {
        return Direct::Rejected(presigned_failure(&response, signed_at));
    }
    match header_text(&response.headers, &ETAG) {
        Some(etag) if !etag.is_empty() => Direct::Accepted(
            UploadedPart {
                part_number: 1,
                etag: ETag::new(etag),
                size: PROBE_PART_BYTES,
            },
            response.headers,
        ),
        _ => Direct::Rejected(Diagnosis::PresignRejected),
    }
}

fn provider_clock_skewed(headers: &HeaderMap, signed_at: OffsetDateTime) -> bool {
    header_text(headers, &DATE)
        .and_then(|date| OffsetDateTime::parse(date, &Rfc2822).ok())
        .is_some_and(|provider_now| (provider_now - signed_at).abs() > CLOCK_TOLERANCE)
}

fn error_code(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let start = text.find("<Code>")? + "<Code>".len();
    let end = start + text[start..].find("</Code>")?;
    let code: String = text[start..end]
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_ERROR_CODE_LEN)
        .collect();
    (!code.is_empty()).then_some(code)
}

fn header_text<'h>(headers: &'h HeaderMap, name: &http::HeaderName) -> Option<&'h str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}

fn tokens(headers: &HeaderMap, name: &http::HeaderName) -> Vec<String> {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
        .collect()
}

fn bucket_diagnosis(error: &StorageError) -> Diagnosis {
    match error {
        StorageError::NotFound => Diagnosis::BucketMissing,
        _ => match service_code(error) {
            Some("NoSuchBucket") => Diagnosis::BucketMissing,
            Some("RequestTimeTooSkewed") => Diagnosis::ClockSkew,
            _ => diagnose(error),
        },
    }
}

fn multipart_diagnosis(error: &StorageError) -> Diagnosis {
    match diagnose(error) {
        Diagnosis::ProviderError | Diagnosis::SizeMismatch => Diagnosis::MultipartRejected,
        other => other,
    }
}

fn unreachable_error() -> StorageError {
    StorageError::ProviderUnavailable(crate::storage::error::Retryable::new(std::io::Error::from(
        std::io::ErrorKind::TimedOut,
    )))
}

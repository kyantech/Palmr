use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_METHOD,
    CONTENT_LENGTH, CONTENT_RANGE, DATE, ETAG, LAST_MODIFIED, ORIGIN, RANGE,
};
use http::{HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt as _, Limited};
use time::format_description::well_known::Rfc3339;
use time::macros::{datetime, format_description};
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub(crate) const BUCKET: &str = "palmr";
const BODY_LIMIT: usize = 16 * 1024 * 1024;
const RECOMMENDED_EXPOSE: &str = "ETag, Content-Length, Content-Range, Accept-Ranges";
const EXPOSE_WITHOUT_ETAG: &str = "Content-Length, Content-Range, Accept-Ranges";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorsMode {
    Recommended,
    MissingEtag,
    WildcardOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresignFault {
    None,
    ClockSkew,
    SignatureMismatch,
}

#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
}

impl Seen {
    pub(crate) fn presigned(&self) -> bool {
        self.query.contains("X-Amz-Signature=")
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn query_value(&self, name: &str) -> Option<String> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }

    pub(crate) fn is(&self, method: &str, operation_query: &str) -> bool {
        self.method == method && self.query.contains(operation_query)
    }
}

struct Object {
    bytes: Vec<u8>,
    modified: OffsetDateTime,
}

struct Upload {
    key: String,
    parts: BTreeMap<u32, Vec<u8>>,
}

struct FakeState {
    objects: BTreeMap<String, Object>,
    uploads: BTreeMap<String, Upload>,
    next_upload: u64,
    seen: Vec<Seen>,
    cors: CorsMode,
    fault: PresignFault,
    now: OffsetDateTime,
}

type Shared = Arc<Mutex<FakeState>>;

pub(crate) struct FakeS3 {
    state: Shared,
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl FakeS3 {
    pub(crate) async fn start() -> Self {
        Self::serve(TcpListener::bind("127.0.0.1:0").await.unwrap())
    }

    pub(crate) async fn start_on(port: u16) -> Self {
        Self::serve(TcpListener::bind(("127.0.0.1", port)).await.unwrap())
    }

    fn serve(listener: TcpListener) -> Self {
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(FakeState {
            objects: BTreeMap::new(),
            uploads: BTreeMap::new(),
            next_upload: 1,
            seen: Vec::new(),
            cors: CorsMode::Recommended,
            fault: PresignFault::None,
            now: datetime!(2026-09-25 12:00 UTC),
        }));
        let router = axum::Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            state,
            address,
            task,
        }
    }

    pub(crate) fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    pub(crate) fn set_cors(&self, cors: CorsMode) {
        lock(&self.state).cors = cors;
    }

    pub(crate) fn set_fault(&self, fault: PresignFault) {
        lock(&self.state).fault = fault;
    }

    pub(crate) fn insert(&self, key: &str, bytes: &[u8], modified: OffsetDateTime) {
        lock(&self.state).objects.insert(
            key.to_owned(),
            Object {
                bytes: bytes.to_vec(),
                modified,
            },
        );
    }

    pub(crate) fn keys(&self) -> Vec<String> {
        lock(&self.state).objects.keys().cloned().collect()
    }

    pub(crate) fn open_uploads(&self) -> usize {
        lock(&self.state).uploads.len()
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        lock(&self.state).seen.clone()
    }
}

impl Drop for FakeS3 {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock(state: &Shared) -> MutexGuard<'_, FakeState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn handle(State(state): State<Shared>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let collected = Limited::new(body, BODY_LIMIT).collect().await;
    let body = collected
        .map(|collected| collected.to_bytes().to_vec())
        .unwrap_or_default();
    let seen = Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().unwrap_or_default().to_owned(),
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
    };
    let mut state = lock(&state);
    state.seen.push(seen.clone());
    let origin = seen.header(ORIGIN.as_str()).map(str::to_owned);
    let mut response = if parts.method == Method::OPTIONS {
        preflight(&state, &seen)
    } else if seen.presigned() && state.fault != PresignFault::None {
        let code = match state.fault {
            PresignFault::ClockSkew => "RequestTimeTooSkewed",
            PresignFault::SignatureMismatch | PresignFault::None => "SignatureDoesNotMatch",
        };
        error(StatusCode::FORBIDDEN, code)
    } else {
        dispatch(&mut state, &parts.method, &seen, body)
    };
    if let Some(origin) = origin.filter(|_| parts.method != Method::OPTIONS) {
        decorate(&mut response, state.cors, &origin);
    }
    let date = http_date(state.now);
    response
        .headers_mut()
        .insert(DATE, HeaderValue::from_str(&date).unwrap());
    response
}

fn preflight(state: &FakeState, seen: &Seen) -> Response {
    let origin = seen.header(ORIGIN.as_str()).unwrap_or_default().to_owned();
    let method = seen
        .header(ACCESS_CONTROL_REQUEST_METHOD.as_str())
        .unwrap_or_default()
        .to_owned();
    let mut response = Response::new(Body::empty());
    let headers = response.headers_mut();
    let allowed_origin = match state.cors {
        CorsMode::WildcardOrigin => "*".to_owned(),
        CorsMode::Recommended | CorsMode::MissingEtag => origin,
    };
    headers.insert(
        ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(&allowed_origin).unwrap(),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_str(&method).unwrap(),
    );
    headers.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("*"));
    headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("3000"));
    response
}

fn decorate(response: &mut Response, cors: CorsMode, origin: &str) {
    let headers = response.headers_mut();
    let (allowed, expose) = match cors {
        CorsMode::Recommended => (origin, RECOMMENDED_EXPOSE),
        CorsMode::MissingEtag => (origin, EXPOSE_WITHOUT_ETAG),
        CorsMode::WildcardOrigin => ("*", RECOMMENDED_EXPOSE),
    };
    headers.insert(
        ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(allowed).unwrap(),
    );
    headers.insert(
        ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(expose),
    );
}

fn dispatch(state: &mut FakeState, method: &Method, seen: &Seen, body: Vec<u8>) -> Response {
    let path = seen.path.trim_start_matches('/');
    let (bucket, key) = path.split_once('/').unwrap_or((path, ""));
    if bucket != BUCKET {
        return error(StatusCode::NOT_FOUND, "NoSuchBucket");
    }
    let key = key.to_owned();
    let upload_id = seen.query_value("uploadId");
    match (method.as_str(), key.is_empty()) {
        ("HEAD", true) => Response::new(Body::empty()),
        ("GET", true) if seen.query.contains("uploads") => list_uploads(state, seen),
        ("GET", true) => list_objects(state, seen),
        ("POST", false) if seen.query.contains("uploads") => create_upload(state, &key),
        ("PUT", false) if upload_id.is_some() => upload_part(state, seen, body),
        ("POST", false) if upload_id.is_some() => complete(state, seen, &key),
        ("DELETE", false) if upload_id.is_some() => {
            state.uploads.remove(&upload_id.unwrap_or_default());
            status(StatusCode::NO_CONTENT)
        }
        ("PUT", false) => {
            let now = state.now;
            state.objects.insert(
                key,
                Object {
                    bytes: body,
                    modified: now,
                },
            );
            with_etag(Response::new(Body::empty()), "\"put-etag\"")
        }
        ("HEAD", false) => match state.objects.get(&key) {
            Some(object) => head(object),
            None => status(StatusCode::NOT_FOUND),
        },
        ("GET", false) => match state.objects.get(&key) {
            Some(object) => get(object, seen),
            None => error(StatusCode::NOT_FOUND, "NoSuchKey"),
        },
        ("DELETE", false) => {
            state.objects.remove(&key);
            status(StatusCode::NO_CONTENT)
        }
        _ => error(StatusCode::NOT_IMPLEMENTED, "NotImplemented"),
    }
}

fn create_upload(state: &mut FakeState, key: &str) -> Response {
    let upload_id = format!("fake-upload-{}", state.next_upload);
    state.next_upload += 1;
    state.uploads.insert(
        upload_id.clone(),
        Upload {
            key: key.to_owned(),
            parts: BTreeMap::new(),
        },
    );
    xml(format!(
        "<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><UploadId>{upload_id}</UploadId></InitiateMultipartUploadResult>"
    ))
}

fn upload_part(state: &mut FakeState, seen: &Seen, body: Vec<u8>) -> Response {
    let upload_id = seen.query_value("uploadId").unwrap_or_default();
    let number: u32 = seen
        .query_value("partNumber")
        .and_then(|number| number.parse().ok())
        .unwrap_or_default();
    match state.uploads.get_mut(&upload_id) {
        Some(upload) => {
            upload.parts.insert(number, body);
            with_etag(
                Response::new(Body::empty()),
                &format!("\"part-{number}-etag\""),
            )
        }
        None => error(StatusCode::NOT_FOUND, "NoSuchUpload"),
    }
}

fn complete(state: &mut FakeState, seen: &Seen, key: &str) -> Response {
    let upload_id = seen.query_value("uploadId").unwrap_or_default();
    let Some(upload) = state.uploads.remove(&upload_id) else {
        return error(StatusCode::NOT_FOUND, "NoSuchUpload");
    };
    let bytes: Vec<u8> = upload.parts.into_values().flatten().collect();
    let now = state.now;
    state.objects.insert(
        upload.key,
        Object {
            bytes,
            modified: now,
        },
    );
    xml(format!(
        "<CompleteMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><ETag>\"complete-etag-3\"</ETag></CompleteMultipartUploadResult>"
    ))
}

fn list_uploads(state: &FakeState, seen: &Seen) -> Response {
    let prefix = seen.query_value("prefix").unwrap_or_default();
    let initiated = state.now.format(&Rfc3339).unwrap();
    let uploads: String = state
        .uploads
        .iter()
        .filter(|(_, upload)| upload.key.starts_with(&prefix))
        .map(|(id, upload)| {
            format!(
                "<Upload><Key>{}</Key><UploadId>{id}</UploadId><Initiated>{initiated}</Initiated></Upload>",
                upload.key
            )
        })
        .collect();
    xml(format!(
        "<ListMultipartUploadsResult><Bucket>{BUCKET}</Bucket><MaxUploads>1000</MaxUploads><IsTruncated>false</IsTruncated>{uploads}</ListMultipartUploadsResult>"
    ))
}

fn list_objects(state: &FakeState, seen: &Seen) -> Response {
    let prefix = seen.query_value("prefix").unwrap_or_default();
    let contents: Vec<String> = state
        .objects
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(key, object)| {
            format!(
                "<Contents><Key>{key}</Key><LastModified>{}</LastModified><ETag>\"e\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                object.modified.format(&Rfc3339).unwrap(),
                object.bytes.len()
            )
        })
        .collect();
    xml(format!(
        "<ListBucketResult><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><KeyCount>{}</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{}</ListBucketResult>",
        contents.len(),
        contents.concat()
    ))
}

fn head(object: &Object) -> Response {
    let mut response = with_etag(Response::new(Body::empty()), "\"object-etag\"");
    let headers = response.headers_mut();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(object.bytes.len()));
    headers.insert(
        LAST_MODIFIED,
        HeaderValue::from_str(&http_date(object.modified)).unwrap(),
    );
    response
}

fn get(object: &Object, seen: &Seen) -> Response {
    let total = object.bytes.len();
    let range = seen.header(RANGE.as_str()).and_then(|range| {
        let (start, end) = range.strip_prefix("bytes=")?.split_once('-')?;
        let start: usize = start.parse().ok()?;
        let end: usize = end.parse::<usize>().ok()?.min(total.checked_sub(1)?);
        (start <= end).then_some((start, end))
    });
    let (status, bytes, content_range) = match range {
        Some((start, end)) => (
            StatusCode::PARTIAL_CONTENT,
            object.bytes[start..=end].to_vec(),
            Some(format!("bytes {start}-{end}/{total}")),
        ),
        None => (StatusCode::OK, object.bytes.clone(), None),
    };
    let len = bytes.len();
    let mut response = with_etag(Response::new(Body::from(bytes)), "\"object-etag\"");
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(
        LAST_MODIFIED,
        HeaderValue::from_str(&http_date(object.modified)).unwrap(),
    );
    if let Some(content_range) = content_range {
        headers.insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&content_range).unwrap(),
        );
    }
    response
}

fn with_etag(mut response: Response, etag: &str) -> Response {
    response
        .headers_mut()
        .insert(ETAG, HeaderValue::from_str(etag).unwrap());
    response
}

fn status(code: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = code;
    response
}

fn error(code: StatusCode, name: &str) -> Response {
    let mut response = Response::new(Body::from(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{name}</Code><Message>fake provider refusal</Message><RequestId>fake</RequestId></Error>"
    )));
    *response.status_mut() = code;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml"),
    );
    response
}

fn xml(body: String) -> Response {
    let mut response = Response::new(Body::from(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{body}"
    )));
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml"),
    );
    response
}

pub(crate) fn http_date(at: OffsetDateTime) -> String {
    at.format(format_description!(
        "[weekday repr:short], [day] [month repr:short] [year] [hour]:[minute]:[second] GMT"
    ))
    .unwrap()
}

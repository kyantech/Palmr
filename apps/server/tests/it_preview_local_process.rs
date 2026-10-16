pub mod support;

use anyhow::{ensure, Context, Result};
use percent_encoding::percent_decode_str;
use reqwest::Method;
use tempfile::TempDir;
use tokio::net::TcpStream;
use url::Url;
use uuid::Uuid;

use support::client::{Creds, Db, Http};
use support::download::{
    content_path, expected_bytes, preview_path, raw_get_path, read_head, seed_local_file, user_id,
    Fetch, Fetcher,
};
use support::tus::{generated_digest, TusWorld, MIB};
use support::ServerProcess;

const PREFIX: u64 = 524_288;
const CAP: u64 = 5 * MIB;
const SIZE: u64 = 2 * MIB + 12_345;
const TEXT_TYPE: &str = "text/plain; charset=utf-8";

struct Rig {
    _root: TempDir,
    _process: ServerProcess,
    owner: TusWorld,
    other: TusWorld,
    fetch: Fetcher,
    base: Url,
    owner_id: String,
}

async fn rig() -> Result<Rig> {
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let mut process = ServerProcess::spawn(&data_dir)?;
    process.wait_ready().await?;
    let base = Url::parse(&format!("http://127.0.0.1:{}/", process.port()))?;
    let admin_http = Http::new(base.clone())?;
    let admin = admin_http.setup_admin().await?;
    let other = admin_http.invite_and_accept(&admin, "bob").await?;
    let db = Db::new(&data_dir);
    let owner_id = user_id(&db, "ada").await?;
    Ok(Rig {
        owner: TusWorld::new(base.clone(), data_dir.clone(), admin_http, admin),
        other: TusWorld::new(base.clone(), data_dir, Http::new(base.clone())?, other),
        fetch: Fetcher::new(&base)?,
        base,
        owner_id,
        _root: root,
        _process: process,
    })
}

impl Rig {
    fn creds(&self) -> &Creds {
        &self.owner.creds
    }

    async fn preview(&self, file: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        self.fetch
            .get(&preview_path(file), Some(self.creds()), headers)
            .await
    }

    async fn content(&self, file: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        self.fetch
            .get(&content_path(file), Some(self.creds()), headers)
            .await
    }

    async fn upload(&self, name: &str, size: u64) -> Result<String> {
        support::download::upload_local(&self.owner, name, size).await
    }

    async fn seed(
        &self,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<support::download::Seeded> {
        seed_local_file(
            &self.owner.db,
            &self.owner.data_dir,
            &self.owner_id,
            name,
            mime,
            bytes,
        )
        .await
    }

    async fn raw_preview(&self, file: &str, extra: &str) -> Result<TcpStream> {
        raw_get_path(&self.base, &preview_path(file), self.creds(), extra).await
    }
}

fn disposition_kind(value: &str) -> Result<String> {
    let (kind, rest) = value.split_once("; filename=\"").context("filename")?;
    let (_, encoded) = rest.split_once("filename*=UTF-8''").context("filename*")?;
    ensure!(!value.contains(['\r', '\n', '\0']), "control characters");
    percent_decode_str(encoded).decode_utf8()?;
    Ok(kind.to_owned())
}

fn text_bytes(from: u64, to: u64) -> Vec<u8> {
    const ALPHABET: &[u8] = b"0123456789abcdef\n";
    (from..to)
        .map(|offset| ALPHABET[usize::try_from(offset % 17).unwrap_or(0)])
        .collect()
}

fn assert_inline_headers(reply: &Fetch, content_type: &str, length: u64, what: &str) -> Result<()> {
    assert_eq!(reply.require("content-type")?, content_type, "{what}");
    assert_eq!(
        reply.require("x-content-type-options")?,
        "nosniff",
        "{what}"
    );
    assert_eq!(
        reply.require("cache-control")?,
        "private, no-cache",
        "{what}"
    );
    assert_eq!(reply.require("accept-ranges")?, "bytes", "{what}");
    assert_eq!(
        reply.require("content-length")?,
        length.to_string(),
        "{what}"
    );
    assert_eq!(
        disposition_kind(reply.require("content-disposition")?)?,
        "inline",
        "{what}"
    );
    assert!(reply.header("content-encoding").is_none(), "{what}");
    assert!(reply.header("location").is_none(), "{what}");
    let etag = reply.require("etag")?;
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "{what}: {etag}"
    );
    assert_eq!(reply.body.len() as u64, length, "{what}");
    Ok(())
}

fn assert_envelope(reply: &Fetch, status: u16, code: &str, what: &str) -> Result<()> {
    assert_eq!(reply.status, status, "{what}: {}", reply.text());
    assert!(
        reply
            .require("content-type")?
            .starts_with("application/json"),
        "{what}"
    );
    assert_eq!(reply.error_code()?, code, "{what}");
    let envelope = reply.envelope()?;
    let request_id = envelope["error"]["requestId"]
        .as_str()
        .context("request id")?;
    assert!(!request_id.is_empty(), "{what}");
    assert_eq!(reply.require("x-request-id")?, request_id, "{what}");
    assert!(envelope["error"]["message"].is_string(), "{what}");
    assert!(envelope["error"]["details"].is_object(), "{what}");
    for leaked in ["location", "content-disposition", "etag", "last-modified"] {
        assert!(reply.header(leaked).is_none(), "{what}: {leaked}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_inline_range_supported() -> Result<()> {
    let rig = rig().await?;
    let cases = [
        ("clip.mp4", "video/mp4"),
        ("clip.webm", "video/webm"),
        ("song.mp3", "audio/mpeg"),
        ("song.ogg", "audio/ogg"),
        ("paper.pdf", "application/pdf"),
        ("photo.png", "image/png"),
        ("photo.jpg", "image/jpeg"),
        ("photo.webp", "image/webp"),
        ("anim.gif", "image/gif"),
    ];
    let last = SIZE - 1;
    let middle = SIZE / 2;
    for (name, mime) in cases {
        let file = rig.upload(name, SIZE).await?;
        let stored = rig
            .owner
            .db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await?;
        assert_eq!(stored, mime, "{name}: the type stored at upload");

        let full = rig
            .preview(&file, &[("accept-encoding", "gzip, br, zstd")])
            .await?;
        assert_eq!(full.status, 200, "{name}: {}", full.text());
        assert_inline_headers(&full, mime, SIZE, name)?;
        assert_eq!(full.sha256(), generated_digest(SIZE)?, "{name}");
        assert!(full.header("content-range").is_none());
        assert!(full.require("last-modified")?.ends_with(" GMT"));

        for (header, from, to) in [
            (format!("bytes=0-{}", 1_023), 0, 1_024),
            (
                format!("bytes={middle}-{}", middle + 4_095),
                middle,
                middle + 4_096,
            ),
            (format!("bytes={}-{last}", last - 700), last - 700, SIZE),
            (format!("bytes={middle}-"), middle, SIZE),
            ("bytes=-512".to_owned(), SIZE - 512, SIZE),
            ("bytes=0-0".to_owned(), 0, 1),
            (format!("bytes={last}-{last}"), last, SIZE),
        ] {
            let part = rig.preview(&file, &[("range", &header)]).await?;
            assert_eq!(part.status, 206, "{name} {header}: {}", part.text());
            assert_inline_headers(&part, mime, to - from, &format!("{name} {header}"))?;
            assert_eq!(
                part.require("content-range")?,
                format!("bytes {from}-{}/{SIZE}", to - 1),
                "{name} {header}"
            );
            assert_eq!(
                part.body,
                expected_bytes(SIZE, from, to)?,
                "{name} {header}"
            );
        }

        for header in [
            format!("bytes={SIZE}-"),
            format!("bytes={}-", SIZE + 1_000),
            "bytes=99999999999-".to_owned(),
            "bytes=-0".to_owned(),
        ] {
            let refused = rig.preview(&file, &[("range", &header)]).await?;
            assert_envelope(&refused, 416, "RANGE_NOT_SATISFIABLE", &header)?;
            assert_eq!(
                refused.require("content-range")?,
                format!("bytes */{SIZE}"),
                "{name} {header}"
            );
        }

        let etag = full.require("etag")?.to_owned();
        let matching = rig
            .preview(&file, &[("range", "bytes=10-19"), ("if-range", &etag)])
            .await?;
        assert_eq!(matching.status, 206, "{name}");
        assert_eq!(matching.body, expected_bytes(SIZE, 10, 20)?);
        let stale = rig
            .preview(
                &file,
                &[("range", "bytes=10-19"), ("if-range", "\"stale\"")],
            )
            .await?;
        assert_eq!(stale.status, 200, "{name}: a stale If-Range restarts");
        assert_eq!(stale.body.len() as u64, SIZE);
        let weak = rig
            .preview(
                &file,
                &[("range", "bytes=10-19"), ("if-range", &format!("W/{etag}"))],
            )
            .await?;
        assert_eq!(weak.status, 200, "{name}: a weak If-Range never matches");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Spec {
    None,
    Malformed,
    Bounded(u64, u64),
    From(u64),
    Suffix(u64),
}

impl Spec {
    fn header(self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Malformed => Some("bytes=abc".to_owned()),
            Self::Bounded(first, last) => Some(format!("bytes={first}-{last}")),
            Self::From(first) => Some(format!("bytes={first}-")),
            Self::Suffix(length) => Some(format!("bytes=-{length}")),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Whole,
    Part(u64, u64),
    TooLarge,
    Unsatisfiable,
}

fn model(size: u64, spec: Spec) -> Outcome {
    let (outcome, length) = match spec {
        Spec::None | Spec::Malformed => (Outcome::Whole, size),
        Spec::Bounded(first, _) | Spec::From(first) if first >= size => {
            return Outcome::Unsatisfiable
        }
        Spec::Suffix(0) => return Outcome::Unsatisfiable,
        Spec::Suffix(_) if size == 0 => return Outcome::Whole,
        Spec::Bounded(first, last) => {
            let last = last.min(size - 1);
            (Outcome::Part(first, last + 1), last + 1 - first)
        }
        Spec::From(first) => (Outcome::Part(first, size), size - first),
        Spec::Suffix(length) => {
            let length = length.min(size);
            (Outcome::Part(size - length, size), length)
        }
    };
    if length > CAP {
        Outcome::TooLarge
    } else {
        outcome
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_text_size_limit() -> Result<()> {
    let rig = rig().await?;
    let sizes = [
        0,
        1,
        PREFIX - 1,
        PREFIX,
        PREFIX + 1,
        CAP,
        CAP + 1,
        24 * MIB + 77,
    ];
    let specs = [
        Spec::None,
        Spec::Malformed,
        Spec::Bounded(0, PREFIX - 1),
        Spec::Bounded(0, 0),
        Spec::Bounded(0, CAP - 1),
        Spec::Bounded(0, CAP),
        Spec::Bounded(0, 99_999_999_999),
        Spec::Bounded(1, CAP),
        Spec::Bounded(PREFIX, 2 * PREFIX - 1),
        Spec::From(0),
        Spec::From(1),
        Spec::From(PREFIX),
        Spec::From(1_000_000_000),
        Spec::Suffix(1),
        Spec::Suffix(PREFIX),
        Spec::Suffix(CAP),
        Spec::Suffix(CAP + 1),
        Spec::Suffix(0),
    ];
    for size in sizes {
        let body = text_bytes(0, size);
        let seeded = rig
            .seed(&format!("log-{size}.txt"), "text/plain", &body)
            .await?;
        for spec in specs {
            let what = format!("size {size} {spec:?}");
            let headers: Vec<(&str, String)> = spec
                .header()
                .map(|value| ("range", value))
                .into_iter()
                .collect();
            let borrowed: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            let reply = rig.preview(&seeded.file_id, &borrowed).await?;
            match model(size, spec) {
                Outcome::Whole => {
                    assert_eq!(reply.status, 200, "{what}: {}", reply.text());
                    assert_inline_headers(&reply, TEXT_TYPE, size, &what)?;
                    assert!(reply.header("content-range").is_none(), "{what}");
                    assert_eq!(reply.body, body, "{what}");
                }
                Outcome::Part(from, to) => {
                    assert_eq!(reply.status, 206, "{what}: {}", reply.text());
                    assert_inline_headers(&reply, TEXT_TYPE, to - from, &what)?;
                    assert_eq!(
                        reply.require("content-range")?,
                        format!("bytes {from}-{}/{size}", to - 1),
                        "{what}"
                    );
                    assert_eq!(reply.body, text_bytes(from, to), "{what}");
                    assert!(reply.body.len() as u64 <= CAP, "{what}");
                }
                Outcome::TooLarge => {
                    assert_envelope(&reply, 413, "PREVIEW_TOO_LARGE", &what)?;
                    assert!(reply.header("content-range").is_none(), "{what}");
                    let details = &reply.envelope()?["error"]["details"];
                    assert_eq!(details["limitBytes"], CAP, "{what}");
                    assert_eq!(details["prefixBytes"], PREFIX, "{what}");
                    assert!(reply.body.len() < 1_024, "{what}: no file bytes");
                    assert!(
                        !reply.body.windows(16).any(|w| w == &text_bytes(0, 16)[..]),
                        "{what}: no file bytes"
                    );
                }
                Outcome::Unsatisfiable => {
                    assert_envelope(&reply, 416, "RANGE_NOT_SATISFIABLE", &what)?;
                    assert_eq!(
                        reply.require("content-range")?,
                        format!("bytes */{size}"),
                        "{what}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_text_cap_cannot_be_bypassed() -> Result<()> {
    let rig = rig().await?;
    let size = 12 * MIB;
    let seeded = rig
        .seed("huge.log", "text/plain", &text_bytes(0, size))
        .await?;
    let file = seeded.file_id;
    let etag = rig
        .content(&file, &[("range", "bytes=0-0")])
        .await?
        .require("etag")?
        .to_owned();

    let prefix = rig.preview(&file, &[("range", "bytes=0-524287")]).await?;
    assert_eq!(prefix.status, 206);
    assert_eq!(prefix.body, text_bytes(0, PREFIX));
    assert_eq!(
        prefix.require("content-range")?,
        format!("bytes 0-{}/{size}", PREFIX - 1)
    );

    for (what, headers) in [
        ("no range", vec![]),
        ("open range", vec![("range", "bytes=0-")]),
        ("huge bounded", vec![("range", "bytes=0-99999999999")]),
        ("suffix", vec![("range", "bytes=-9999999999")]),
        (
            "stale if-range",
            vec![("range", "bytes=0-9"), ("if-range", "\"old\"")],
        ),
        ("garbage range", vec![("range", "lines=0-9")]),
        (
            "multi range",
            vec![("range", "bytes=0-9"), ("range", "bytes=20-29")],
        ),
        (
            "if-none-match mismatch",
            vec![("if-none-match", "\"other\"")],
        ),
        (
            "if-none-match match",
            vec![("if-none-match", etag.as_str())],
        ),
        ("if-none-match star", vec![("if-none-match", "*")]),
        ("accept-encoding", vec![("accept-encoding", "gzip, br")]),
    ] {
        let reply = rig.preview(&file, &headers).await?;
        assert_envelope(&reply, 413, "PREVIEW_TOO_LARGE", what)?;
    }

    for query in [
        "?mime=text/plain",
        "?disposition=attachment",
        "?limit=99999999999",
        "?objectKey=objects/aa/bb/x&storagePath=/etc/passwd&providerUrl=http://x",
        "?download=1",
    ] {
        let reply = rig
            .fetch
            .get(
                &format!("{}{query}", preview_path(&file)),
                Some(rig.creds()),
                &[],
            )
            .await?;
        assert_envelope(&reply, 413, "PREVIEW_TOO_LARGE", query)?;
    }

    let conditional = rig
        .preview(
            &file,
            &[("range", "bytes=0-524287"), ("if-none-match", &etag)],
        )
        .await?;
    assert_eq!(conditional.status, 304, "a bounded request may be a 304");
    assert!(conditional.body.is_empty());

    let download = rig.content(&file, &[]).await?;
    assert_eq!(download.status, 200, "the download route has no text limit");
    assert_eq!(download.require("content-length")?, size.to_string());
    assert_eq!(download.body.len() as u64, size);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_unsafe_type_refused() -> Result<()> {
    let rig = rig().await?;
    let marker = b"PALMR-SECRET-MARKER-0123456789";
    let cases = [
        ("page.html", "text/html", "unsafe_content"),
        ("page.htm", "text/html; charset=utf-8", "unsafe_content"),
        ("page.xhtml", "application/xhtml+xml", "unsafe_content"),
        ("drawing.svg", "image/svg+xml", "unsafe_content"),
        ("data.xml", "text/xml", "unsafe_content"),
        ("data2.xml", "application/xml", "unsafe_content"),
        ("feed.atom", "application/atom+xml", "unsafe_content"),
        ("blob.bin", "application/octet-stream", "unsupported_type"),
        ("archive.zip", "application/zip", "unsupported_type"),
        ("tool.exe", "application/x-msdownload", "unsupported_type"),
        ("odd.bin", "application/x-palmr-unknown", "unsupported_type"),
        ("weird.bin", "not a mime", "unsupported_type"),
        ("empty-mime.bin", "", "unsupported_type"),
        ("wild.txt", "text/*", "unsupported_type"),
    ];
    for (name, mime, reason) in cases {
        let mut bytes = marker.to_vec();
        bytes.extend_from_slice(&[b'.'; 4_096]);
        let seeded = rig.seed(name, mime, &bytes).await?;
        let file = &seeded.file_id;
        let download = rig.content(file, &[]).await?;
        assert_eq!(download.status, 200, "{name}: the download still works");
        assert_eq!(download.body, bytes, "{name}");
        assert_eq!(
            disposition_kind(download.require("content-disposition")?)?,
            "attachment",
            "{name}"
        );
        let etag = download.require("etag")?.to_owned();

        for headers in [
            vec![],
            vec![("range", "bytes=0-9")],
            vec![("range", "bytes=99999999-")],
            vec![("if-none-match", etag.as_str())],
            vec![("if-none-match", "*")],
            vec![("range", "bytes=0-9"), ("if-range", etag.as_str())],
            vec![("accept", "text/html")],
        ] {
            let refused = rig.preview(file, &headers).await?;
            let what = format!("{name} {headers:?}");
            assert_envelope(&refused, 415, "PREVIEW_UNSUPPORTED", &what)?;
            assert_eq!(
                refused.envelope()?["error"]["details"]["reason"],
                reason,
                "{what}"
            );
            assert!(
                !refused.body.windows(marker.len()).any(|w| w == marker),
                "{what}: file bytes leaked"
            );
            assert!(refused.header("content-range").is_none(), "{what}");
            assert!(refused.header("accept-ranges").is_none(), "{what}");
            assert!(!refused.text().contains(name), "{what}: name leaked");
        }
        let asked_inline = rig
            .fetch
            .get(
                &format!("{}?mime=image/png&disposition=inline", preview_path(file)),
                Some(rig.creds()),
                &[],
            )
            .await?;
        assert_envelope(&asked_inline, 415, "PREVIEW_UNSUPPORTED", name)?;
    }

    let uploaded = [
        ("real.html", "text/html"),
        ("real.svg", "image/svg+xml"),
        ("real.xml", "text/xml"),
        ("real.zip", "application/zip"),
        ("real.exe", "application/x-msdos-program"),
    ];
    for (name, expected) in uploaded {
        let file = rig.upload(name, 20_000).await?;
        let stored = rig
            .owner
            .db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await?;
        assert!(
            expected == stored || name == "real.exe",
            "{name}: stored {stored}"
        );
        let refused = rig.preview(&file, &[]).await?;
        assert_envelope(&refused, 415, "PREVIEW_UNSUPPORTED", name)?;
        let download = rig.content(&file, &[]).await?;
        assert_eq!(download.status, 200, "{name}");
        assert_eq!(download.body.len(), 20_000);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_other_owner_404() -> Result<()> {
    let rig = rig().await?;
    let text = rig
        .seed("secret.txt", "text/plain", &text_bytes(0, 6 * MIB))
        .await?
        .file_id;
    let html = rig
        .seed("secret.html", "text/html", b"<script>1</script>")
        .await?
        .file_id;
    let video = rig.upload("secret.mp4", SIZE).await?;
    let etag = rig.content(&video, &[]).await?.require("etag")?.to_owned();

    let unknown = Uuid::now_v7().to_string();
    let reference = rig
        .fetch
        .get(&preview_path(&unknown), Some(&rig.other.creds), &[])
        .await?;
    assert_eq!(reference.status, 404);
    assert_eq!(reference.error_code()?, "FILE_NOT_FOUND");

    for (file, what) in [
        (&text, "oversized text"),
        (&html, "unsafe type"),
        (&video, "video"),
    ] {
        let attempts: Vec<Vec<(&str, &str)>> = vec![
            vec![],
            vec![("range", "bytes=0-9")],
            vec![("range", "bytes=99999999999-")],
            vec![("if-none-match", &etag)],
            vec![("if-none-match", "*")],
            vec![("range", "bytes=0-9"), ("if-range", &etag)],
        ];
        for headers in attempts {
            let foreign = rig
                .fetch
                .get(&preview_path(file), Some(&rig.other.creds), &headers)
                .await?;
            assert_eq!(foreign.status, 404, "{what} {headers:?}");
            assert_eq!(foreign.error_code()?, "FILE_NOT_FOUND");
            let (mut left, mut right) = (foreign.envelope()?, reference.envelope()?);
            left["error"]["requestId"] = "".into();
            right["error"]["requestId"] = "".into();
            assert_eq!(left, right, "{what}: indistinguishable from an unknown id");
            for leaked in [
                "etag",
                "last-modified",
                "content-disposition",
                "content-range",
                "accept-ranges",
                "location",
            ] {
                assert!(foreign.header(leaked).is_none(), "{what}: {leaked}");
            }
            let body = foreign.text();
            for leaked in ["secret", "objects/", etag.as_str()] {
                assert!(!body.contains(leaked), "{what}: {leaked}");
            }
        }
    }

    let anonymous = rig.fetch.get(&preview_path(&video), None, &[]).await?;
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.error_code()?, "AUTH_REQUIRED");

    let malformed = rig
        .fetch
        .get(&preview_path("not-a-uuid"), Some(rig.creds()), &[])
        .await?;
    assert_eq!(
        (malformed.status, malformed.error_code()?.as_str()),
        (404, "FILE_NOT_FOUND")
    );

    assert_eq!(rig.preview(&video, &[]).await?.status, 200, "the owner");
    let gone = rig
        .owner
        .http
        .send(
            Method::DELETE,
            &format!("/api/v1/files/{video}"),
            Some(rig.creds()),
            None,
        )
        .await?;
    assert!(gone.status.is_success());
    let deleted = rig.preview(&video, &[]).await?;
    assert_eq!(deleted.status, 404);
    assert_eq!(deleted.error_code()?, "FILE_NOT_FOUND");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_zero_byte_and_small_text() -> Result<()> {
    let rig = rig().await?;
    let empty = rig.seed("empty.txt", "text/plain", b"").await?.file_id;
    let reply = rig.preview(&empty, &[]).await?;
    assert_eq!(reply.status, 200);
    assert_inline_headers(&reply, TEXT_TYPE, 0, "empty")?;
    let ranged = rig.preview(&empty, &[("range", "bytes=0-524287")]).await?;
    assert_envelope(&ranged, 416, "RANGE_NOT_SATISFIABLE", "empty ranged")?;
    assert_eq!(ranged.require("content-range")?, "bytes */0");

    let small = rig
        .seed("notes.json", "application/json", br#"{"a":[1,2,3]}"#)
        .await?
        .file_id;
    let reply = rig.preview(&small, &[]).await?;
    assert_eq!(reply.status, 200);
    assert_inline_headers(&reply, TEXT_TYPE, 13, "json")?;
    assert_eq!(reply.body, br#"{"a":[1,2,3]}"#);
    let ranged = rig.preview(&small, &[("range", "bytes=0-524287")]).await?;
    assert_eq!(ranged.status, 206);
    assert_eq!(ranged.require("content-range")?, "bytes 0-12/13");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_mime_derived_from_stored_metadata() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("photo.png", 30_000).await?;
    let own = rig.preview(&file, &[]).await?;
    assert_eq!(own.require("content-type")?, "image/png");

    for header in [
        ("content-type", "text/html"),
        ("accept", "text/html"),
        ("x-content-type", "text/html"),
        ("x-forwarded-proto", "https"),
    ] {
        let reply = rig.preview(&file, &[header]).await?;
        assert_eq!(reply.require("content-type")?, "image/png", "{header:?}");
    }
    for query in ["?mime=text/html", "?contentType=text/html", "?type=html"] {
        let reply = rig
            .fetch
            .get(
                &format!("{}{query}", preview_path(&file)),
                Some(rig.creds()),
                &[],
            )
            .await?;
        assert_eq!(reply.require("content-type")?, "image/png", "{query}");
    }

    let retag = |mime: &'static str| {
        let db = &rig.owner.db;
        let file = file.clone();
        async move {
            db.execute_bound(
                "UPDATE files SET mime_type = ?1 WHERE id = ?2",
                &[mime, file.as_str()],
            )
            .await
        }
    };
    retag("text/html").await?;
    assert_envelope(
        &rig.preview(&file, &[]).await?,
        415,
        "PREVIEW_UNSUPPORTED",
        "html",
    )?;
    retag("application/pdf").await?;
    assert_eq!(
        rig.preview(&file, &[]).await?.require("content-type")?,
        "application/pdf"
    );
    retag("text/markdown").await?;
    assert_eq!(
        rig.preview(&file, &[]).await?.require("content-type")?,
        TEXT_TYPE
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_conditional_requests() -> Result<()> {
    let rig = rig().await?;
    for (name, mime) in [("a.mp4", "video/mp4"), ("a.txt", "text/plain")] {
        let file = if mime == "text/plain" {
            rig.seed(name, mime, &text_bytes(0, 100_000)).await?.file_id
        } else {
            rig.upload(name, SIZE).await?
        };
        let full = rig.preview(&file, &[]).await?;
        assert_eq!(full.status, 200, "{name}");
        let etag = full.require("etag")?.to_owned();
        let modified = full.require("last-modified")?.to_owned();

        let unchanged = rig.preview(&file, &[("if-none-match", &etag)]).await?;
        assert_eq!(unchanged.status, 304, "{name}");
        assert!(unchanged.body.is_empty());
        assert_eq!(unchanged.require("etag")?, etag);
        assert_eq!(unchanged.require("cache-control")?, "private, no-cache");
        assert_eq!(unchanged.require("x-content-type-options")?, "nosniff");
        let changed = rig
            .preview(&file, &[("if-none-match", "\"other\"")])
            .await?;
        assert_eq!(changed.status, 200, "{name}");
        let beats_range = rig
            .preview(&file, &[("if-none-match", &etag), ("range", "bytes=0-9")])
            .await?;
        assert_eq!(beats_range.status, 304, "{name}");

        let by_date = rig
            .preview(&file, &[("range", "bytes=0-9"), ("if-range", &modified)])
            .await?;
        assert_eq!(by_date.status, 206, "{name}");
        let stale_date = rig
            .preview(
                &file,
                &[
                    ("range", "bytes=0-9"),
                    ("if-range", "Mon, 01 Jan 2001 00:00:00 GMT"),
                ],
            )
            .await?;
        assert_eq!(stale_date.status, 200, "{name}");
        let unsatisfiable_with_match = rig
            .preview(
                &file,
                &[("range", "bytes=999999999-"), ("if-none-match", &etag)],
            )
            .await?;
        assert_eq!(unsatisfiable_with_match.status, 304, "{name}: T01 ordering");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_shares_the_download_permits() -> Result<()> {
    let rig = rig().await?;
    let big = rig
        .seed("slots.mp4", "video/mp4", &vec![0x33_u8; 40 * MIB as usize])
        .await?;
    let small = rig.seed("small.txt", "text/plain", b"tiny").await?;

    let mut stalled = Vec::new();
    for index in 0..128 {
        let mut stream = if index % 2 == 0 {
            rig.raw_preview(&big.file_id, "").await?
        } else {
            support::download::raw_get(&rig.base, &big.file_id, rig.creds(), "").await?
        };
        let (head, _) = read_head(&mut stream).await?;
        assert_eq!(head.status, 200);
        stalled.push(stream);
    }
    let started = std::time::Instant::now();
    let busy = rig.preview(&small.file_id, &[]).await?;
    assert_eq!(busy.status, 503, "previews and downloads share 128 slots");
    assert_eq!(busy.error_code()?, "TRANSFER_BACKPRESSURE");
    assert_eq!(busy.require("retry-after")?, "5");
    assert!(started.elapsed() >= std::time::Duration::from_secs(4));

    drop(stalled);
    let mut served = false;
    for _ in 0..200 {
        if rig.preview(&small.file_id, &[]).await?.status == 200 {
            served = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(served, "closing the connections returns every permit");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_disconnect_stops_stream() -> Result<()> {
    let rig = rig().await?;
    let seeded = rig
        .seed("big.mp4", "video/mp4", &vec![0x5a_u8; 48 * MIB as usize])
        .await?;
    for headers in [vec![], vec![("range", "bytes=1000000-40000000")]] {
        let mut response = rig
            .fetch
            .open(&preview_path(&seeded.file_id), Some(rig.creds()), &headers)
            .await?;
        let mut read = 0_usize;
        while read < 512 * 1024 {
            read += response.chunk().await?.context("an early end")?.len();
        }
        drop(response);
    }
    let again = rig
        .preview(&seeded.file_id, &[("range", "bytes=0-99")])
        .await?;
    assert_eq!(again.status, 206);
    assert_eq!(std::fs::metadata(&seeded.path)?.len(), 48 * MIB);
    Ok(())
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-024")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_R024_preview_server_half_streams_ranges_and_bounds_text() -> Result<()> {
    let rig = rig().await?;
    let video_size = 32 * MIB + 7;
    let video = rig.upload("movie.mp4", video_size).await?;
    let started = std::time::Instant::now();
    for fraction in [0, video_size / 2, video_size - 70_000] {
        let reply = rig
            .preview(
                &video,
                &[("range", &format!("bytes={fraction}-{}", fraction + 65_535))],
            )
            .await?;
        assert_eq!(reply.status, 206);
        assert_eq!(
            reply.body,
            expected_bytes(video_size, fraction, fraction + 65_536)?
        );
        assert_eq!(
            reply.require("content-range")?,
            format!("bytes {fraction}-{}/{video_size}", fraction + 65_535)
        );
        assert_eq!(reply.require("content-type")?, "video/mp4");
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(5));

    let log = rig
        .seed("big.log", "text/plain", &text_bytes(0, 24 * MIB))
        .await?
        .file_id;
    let prefix = rig.preview(&log, &[("range", "bytes=0-524287")]).await?;
    assert_eq!(prefix.status, 206);
    assert_eq!(prefix.body.len() as u64, PREFIX);
    assert_eq!(prefix.body, text_bytes(0, PREFIX));
    let whole = rig.preview(&log, &[]).await?;
    assert_envelope(&whole, 413, "PREVIEW_TOO_LARGE", "whole log")?;
    Ok(())
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-025")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_358_preview_mime_behind_proxy() -> Result<()> {
    let rig = rig().await?;
    let cases = [
        ("clip.mp4", "video/mp4", Some("video/mp4")),
        ("clip.webm", "video/webm", Some("video/webm")),
        ("song.mp3", "audio/mpeg", Some("audio/mpeg")),
        ("song.ogg", "audio/ogg", Some("audio/ogg")),
        ("paper.pdf", "application/pdf", Some("application/pdf")),
        ("photo.png", "image/png", Some("image/png")),
        ("drawing.svg", "image/svg+xml", None),
        ("notes.txt", "text/plain", Some(TEXT_TYPE)),
    ];
    let behind_proxy = [
        ("x-forwarded-for", "198.51.100.4"),
        ("x-forwarded-proto", "https"),
        ("x-forwarded-host", "palmr.example.test"),
        ("via", "1.1 nginx"),
    ];
    for (name, stored_mime, served) in cases {
        let file = rig.upload(name, 20_000).await?;
        let stored = rig
            .owner
            .db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await?;
        assert_eq!(stored, stored_mime, "{name}");
        let reply = rig.preview(&file, &behind_proxy).await?;
        match served {
            Some(content_type) => {
                assert_eq!(reply.status, 200, "{name}");
                assert_ne!(reply.require("content-type")?, "application/octet-stream");
                assert_inline_headers(&reply, content_type, 20_000, name)?;
                let ranged = rig
                    .preview(&file, &[("range", "bytes=0-99"), behind_proxy[0]])
                    .await?;
                assert_eq!(ranged.status, 206, "{name}");
                assert_eq!(ranged.require("content-type")?, content_type);
                assert_eq!(ranged.require("x-content-type-options")?, "nosniff");
            }
            None => {
                assert_envelope(&reply, 415, "PREVIEW_UNSUPPORTED", name)?;
                assert!(reply
                    .require("content-type")?
                    .starts_with("application/json"));
            }
        }
    }
    Ok(())
}

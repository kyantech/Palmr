pub mod support;

use std::ffi::OsString;
use std::time::Duration;

#[cfg(feature = "e2e-fixture")]
use anyhow::ensure;
use anyhow::{Context, Result};
use tempfile::TempDir;
use url::Url;
use uuid::Uuid;

use support::client::{Creds, Db, Http};
use support::download::{content_path, expected_bytes, preview_path, upload_s3, Fetch, Fetcher};
#[cfg(feature = "e2e-fixture")]
use support::download::{raw_get_path, read_head};
use support::minio::MinioServer;
use support::tap::Tap;
use support::tus::{generated_digest, MIB};
use support::ServerProcess;

const PREFIX: u64 = 524_288;
const CAP: u64 = 5 * MIB;
const MEDIA: u64 = 3 * MIB + 999;
const TEXT_TYPE: &str = "text/plain; charset=utf-8";

struct Rig {
    _root: TempDir,
    _process: ServerProcess,
    _minio: MinioServer,
    tap: Tap,
    http: Http,
    db: Db,
    fetch: Fetcher,
    owner: Creds,
    other: Creds,
    public: Url,
    bucket: String,
    #[cfg_attr(
        not(feature = "e2e-fixture"),
        allow(dead_code, reason = "only the fixture tests open raw connections")
    )]
    base: Url,
}

async fn rig(presigned_get: bool) -> Result<Rig> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("preview").await?;
    let public = Url::parse(minio.endpoint())?;
    let internal = format!("http://127.0.0.1:{}", public.port().context("port")?);
    let tap = Tap::start(&internal).await?;
    let tapped = tap.endpoint();
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let mut env: Vec<(String, OsString)> = [
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_PROFILE", "minio"),
        ("PALMR_S3_ENDPOINT", tapped.as_str()),
        ("PALMR_S3_PUBLIC_ENDPOINT", minio.endpoint()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", bucket.as_str()),
        ("PALMR_S3_ACCESS_KEY", minio.access_key()),
        ("PALMR_S3_SECRET_KEY", minio.secret_key()),
        ("PALMR_S3_FORCE_PATH_STYLE", "true"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), OsString::from(value)))
    .collect();
    if !presigned_get {
        env.push((
            "PALMR_E2E_S3_PRESIGNED_GET".to_owned(),
            OsString::from("off"),
        ));
    }
    let borrowed: Vec<(&str, &std::ffi::OsStr)> = env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_os_str()))
        .collect();
    let mut process = ServerProcess::spawn_with(&data_dir, &borrowed)?;
    process.wait_ready().await?;
    let base = Url::parse(&format!("http://127.0.0.1:{}/", process.port()))?;
    let http = Http::new(base.clone())?;
    let owner = http.setup_admin().await?;
    let other = http.invite_and_accept(&owner, "bob").await?;
    Ok(Rig {
        fetch: Fetcher::new(&base)?,
        db: Db::new(&data_dir),
        _root: root,
        _process: process,
        _minio: minio,
        tap,
        http,
        owner,
        other,
        public,
        bucket,
        base,
    })
}

impl Rig {
    async fn upload(&self, name: &str, size: u64) -> Result<String> {
        upload_s3(&self.http, &self.db, &self.owner, name, size).await
    }

    async fn preview(&self, file: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        self.fetch
            .get(&preview_path(file), Some(&self.owner), headers)
            .await
    }

    async fn stored_mime(&self, file: &str) -> Result<String> {
        self.db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await
    }
}

fn pair(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.into_owned())
}

async fn fetch_url(url: &Url, headers: &[(&str, &str)]) -> Result<Fetch> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()?;
    let mut request = client.get(url.clone());
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    support::download::collect(request.send().await?).await
}

fn assert_envelope(reply: &Fetch, status: u16, code: &str, what: &str) -> Result<()> {
    assert_eq!(reply.status, status, "{what}: {}", reply.text());
    assert_eq!(reply.error_code()?, code, "{what}");
    let envelope = reply.envelope()?;
    assert_eq!(
        reply.require("x-request-id")?,
        envelope["error"]["requestId"].as_str().context("id")?,
        "{what}"
    );
    for leaked in ["location", "content-disposition", "etag"] {
        assert!(reply.header(leaked).is_none(), "{what}: {leaked}");
    }
    Ok(())
}

fn with_pair(url: &Url, name: &str, value: Option<&str>) -> Url {
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !key.eq_ignore_ascii_case(name))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut tampered = url.clone();
    {
        let mut query = tampered.query_pairs_mut();
        query.clear();
        for (key, value) in &kept {
            query.append_pair(key, value);
        }
        if let Some(value) = value {
            query.append_pair(name, value);
        }
    }
    tampered
}

fn disposition_kind(value: &str) -> Result<&str> {
    value
        .split_once("; filename=\"")
        .map(|(kind, _)| kind)
        .context("filename")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_media_direct_real_minio() -> Result<()> {
    let rig = rig(true).await?;
    let last = MEDIA - 1;
    for (name, mime) in [
        ("clip.mp4", "video/mp4"),
        ("clip.webm", "video/webm"),
        ("song.mp3", "audio/mpeg"),
        ("song.ogg", "audio/ogg"),
        ("paper.pdf", "application/pdf"),
        ("photo.png", "image/png"),
    ] {
        let file = rig.upload(name, MEDIA).await?;
        assert_eq!(rig.stored_mime(&file).await?, mime, "{name}");

        rig.tap.clear();
        let issued = rig.preview(&file, &[]).await?;
        assert_eq!(issued.status, 302, "{name}: {}", issued.text());
        assert!(issued.body.is_empty());
        assert_eq!(issued.require("cache-control")?, "no-store");
        assert_eq!(issued.require("x-content-type-options")?, "nosniff");
        assert!(
            rig.tap.all().is_empty(),
            "{name}: a plain redirect touches no storage"
        );
        let location = Url::parse(issued.require("location")?)?;
        assert_eq!(location.host_str(), rig.public.host_str(), "{name}");
        assert_eq!(location.port(), rig.public.port());
        assert!(location
            .path()
            .starts_with(&format!("/{}/objects/", rig.bucket)));
        assert_eq!(pair(&location, "X-Amz-Expires").as_deref(), Some("300"));
        assert_eq!(
            pair(&location, "X-Amz-SignedHeaders").as_deref(),
            Some("host")
        );
        assert_eq!(
            pair(&location, "response-content-type").as_deref(),
            Some(mime)
        );
        let pinned =
            pair(&location, "response-content-disposition").context("pinned disposition")?;
        assert_eq!(disposition_kind(&pinned)?, "inline", "{name}");
        assert!(pinned.contains(&format!("filename=\"{name}\"")), "{name}");

        let followed = fetch_url(&location, &[]).await?;
        assert_eq!(followed.status, 200, "{name}");
        assert_eq!(followed.require("content-type")?, mime, "{name}");
        assert_eq!(
            disposition_kind(followed.require("content-disposition")?)?,
            "inline",
            "{name}"
        );
        assert_eq!(followed.require("content-length")?, MEDIA.to_string());
        assert_eq!(followed.sha256(), generated_digest(MEDIA)?, "{name}");

        for (header, from, to) in [
            ("bytes=0-1023".to_owned(), 0, 1_024),
            (
                format!("bytes={}-{}", MEDIA / 2, MEDIA / 2 + 4_095),
                MEDIA / 2,
                MEDIA / 2 + 4_096,
            ),
            (format!("bytes={}-", MEDIA - 700), MEDIA - 700, MEDIA),
            ("bytes=-512".to_owned(), MEDIA - 512, MEDIA),
        ] {
            rig.tap.clear();
            let redirected = rig.preview(&file, &[("range", &header)]).await?;
            assert_eq!(redirected.status, 302, "{name} {header}");
            assert!(rig.tap.all().is_empty());
            let target = Url::parse(redirected.require("location")?)?;
            let part = fetch_url(&target, &[("range", &header)]).await?;
            assert_eq!(part.status, 206, "{name} {header}");
            assert_eq!(part.require("content-type")?, mime);
            assert_eq!(
                part.require("content-range")?,
                format!("bytes {from}-{}/{MEDIA}", to - 1)
            );
            assert_eq!(
                part.body,
                expected_bytes(MEDIA, from, to)?,
                "{name} {header}"
            );
        }

        for header in [
            format!("bytes={MEDIA}-"),
            "bytes=99999999999-".to_owned(),
            "bytes=-0".to_owned(),
        ] {
            rig.tap.clear();
            let refused = rig.preview(&file, &[("range", &header)]).await?;
            assert_envelope(&refused, 416, "RANGE_NOT_SATISFIABLE", &header)?;
            assert_eq!(
                refused.require("content-range")?,
                format!("bytes */{MEDIA}")
            );
            assert!(
                rig.tap.all().is_empty(),
                "{header}: decided without storage"
            );
        }

        let foreign = rig
            .fetch
            .get(&preview_path(&file), Some(&rig.other), &[])
            .await?;
        assert_envelope(&foreign, 404, "FILE_NOT_FOUND", name)?;
        assert!(!foreign.text().contains("objects/"));
        let _ = last;
    }
    let unknown = rig
        .fetch
        .get(
            &preview_path(&Uuid::now_v7().to_string()),
            Some(&rig.owner),
            &[],
        )
        .await?;
    assert_envelope(&unknown, 404, "FILE_NOT_FOUND", "unknown")?;
    let anonymous = rig
        .fetch
        .get(&preview_path(&Uuid::now_v7().to_string()), None, &[])
        .await?;
    assert_eq!(anonymous.status, 401);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_conditional_range_real_minio() -> Result<()> {
    let rig = rig(true).await?;
    let file = rig.upload("clip.mp4", MEDIA).await?;
    let issued = rig.preview(&file, &[]).await?;
    let direct = fetch_url(&Url::parse(issued.require("location")?)?, &[]).await?;
    let provider_etag = direct.require("etag")?.to_owned();
    assert!(provider_etag.starts_with('"'), "{provider_etag}");

    rig.tap.clear();
    let unchanged = rig
        .preview(&file, &[("if-none-match", &provider_etag)])
        .await?;
    assert_eq!(unchanged.status, 304);
    assert!(unchanged.body.is_empty());
    assert_ne!(
        unchanged.require("etag")?,
        provider_etag,
        "Palmr's own validator"
    );
    assert_eq!(unchanged.require("cache-control")?, "private, no-cache");
    assert!(rig.tap.object_reads().is_empty());
    let own = unchanged.require("etag")?.to_owned();
    let again = rig.preview(&file, &[("if-none-match", &own)]).await?;
    assert_eq!(again.status, 304);

    rig.tap.clear();
    let matching = rig
        .preview(
            &file,
            &[("range", "bytes=100-199"), ("if-range", &provider_etag)],
        )
        .await?;
    assert_eq!(matching.status, 206, "{}", matching.text());
    assert_eq!(matching.body, expected_bytes(MEDIA, 100, 200)?);
    assert_eq!(matching.require("content-type")?, "video/mp4");
    assert_eq!(matching.require("x-content-type-options")?, "nosniff");
    assert_eq!(
        disposition_kind(matching.require("content-disposition")?)?,
        "inline"
    );
    assert_eq!(matching.require("etag")?, own);
    let reads = rig.tap.object_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].range.as_deref(), Some("bytes=100-199"));

    for stale in ["\"0000\"".to_owned(), format!("W/{provider_etag}")] {
        let reply = rig
            .preview(&file, &[("range", "bytes=100-199"), ("if-range", &stale)])
            .await?;
        assert_eq!(reply.status, 200, "{stale}: a stale validator restarts");
        assert_eq!(reply.sha256(), generated_digest(MEDIA)?);
        assert_eq!(reply.require("content-length")?, MEDIA.to_string());
    }
    let by_palmr = rig
        .preview(&file, &[("range", "bytes=0-9"), ("if-range", &own)])
        .await?;
    assert_eq!(by_palmr.status, 206);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_unsafe_refused_never_signs() -> Result<()> {
    let rig = rig(true).await?;
    for (name, mime) in [
        ("drawing.svg", "image/svg+xml"),
        ("page.html", "text/html"),
        ("data.xml", "text/xml"),
        ("archive.zip", "application/zip"),
    ] {
        let file = rig.upload(name, 30_000).await?;
        assert_eq!(rig.stored_mime(&file).await?, mime, "{name}");
        for headers in [
            vec![],
            vec![("range", "bytes=0-9")],
            vec![("range", "bytes=99999999-")],
            vec![("if-none-match", "*")],
            vec![("range", "bytes=0-9"), ("if-range", "\"x\"")],
        ] {
            rig.tap.clear();
            let refused = rig.preview(&file, &headers).await?;
            let what = format!("{name} {headers:?}");
            assert_envelope(&refused, 415, "PREVIEW_UNSUPPORTED", &what)?;
            assert!(rig.tap.all().is_empty(), "{what}: storage was not touched");
            assert!(!refused.text().contains("X-Amz"), "{what}");
        }
        let download = rig
            .fetch
            .get(&content_path(&file), Some(&rig.owner), &[])
            .await?;
        assert_eq!(
            download.status, 302,
            "{name}: the download route still redirects"
        );
        let target = Url::parse(download.require("location")?)?;
        assert_eq!(
            disposition_kind(&pair(&target, "response-content-disposition").context("pin")?)?
                .to_owned(),
            "attachment"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_text_bound_real_minio() -> Result<()> {
    let rig = rig(true).await?;
    let small = rig.upload("small.txt", 20_000).await?;
    let mid = rig.upload("mid.log", 600 * 1024).await?;
    let edge_size = CAP + 1;
    let edge = rig.upload("edge.txt", edge_size).await?;
    let huge_size = 12 * MIB + 4_321;
    let huge = rig.upload("huge.log", huge_size).await?;
    for file in [&small, &mid, &edge, &huge] {
        assert_eq!(rig.stored_mime(file).await?, "text/plain");
    }

    rig.tap.clear();
    let whole = rig.preview(&small, &[]).await?;
    assert_eq!(whole.status, 200);
    assert_eq!(whole.require("content-type")?, TEXT_TYPE);
    assert_eq!(whole.require("x-content-type-options")?, "nosniff");
    assert!(
        whole.header("location").is_none(),
        "text is never redirected"
    );
    assert_eq!(whole.body, expected_bytes(20_000, 0, 20_000)?);
    assert_eq!(
        disposition_kind(whole.require("content-disposition")?)?,
        "inline"
    );

    rig.tap.clear();
    let middle = rig.preview(&mid, &[]).await?;
    assert_eq!(middle.status, 200, "between the prefix and the 5 MiB limit");
    assert_eq!(middle.body, expected_bytes(600 * 1024, 0, 600 * 1024)?);
    let reads = rig.tap.object_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].served, Some(600 * 1024));

    rig.tap.clear();
    let prefix = rig.preview(&huge, &[("range", "bytes=0-524287")]).await?;
    assert_eq!(prefix.status, 206);
    assert!(prefix.header("location").is_none());
    assert_eq!(prefix.require("content-type")?, TEXT_TYPE);
    assert_eq!(
        prefix.require("content-range")?,
        format!("bytes 0-{}/{huge_size}", PREFIX - 1)
    );
    assert_eq!(prefix.body, expected_bytes(huge_size, 0, PREFIX)?);
    let reads = rig.tap.object_reads();
    assert_eq!(reads.len(), 1, "exactly one provider read");
    assert_eq!(reads[0].range.as_deref(), Some("bytes=0-524287"));
    assert_eq!(
        reads[0].served,
        Some(PREFIX),
        "the provider sent only the prefix"
    );

    rig.tap.clear();
    let tail = rig.preview(&huge, &[("range", "bytes=-524288")]).await?;
    assert_eq!(tail.status, 206);
    assert_eq!(
        tail.body,
        expected_bytes(huge_size, huge_size - PREFIX, huge_size)?
    );
    let reads = rig.tap.object_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].served, Some(PREFIX));

    for (what, file, headers) in [
        ("whole", &huge, vec![]),
        ("open", &huge, vec![("range", "bytes=0-")]),
        ("open from offset", &huge, vec![("range", "bytes=1000-")]),
        (
            "huge bounded",
            &huge,
            vec![("range", "bytes=0-99999999999")],
        ),
        ("suffix", &huge, vec![("range", "bytes=-99999999999")]),
        (
            "stale if-range",
            &huge,
            vec![("range", "bytes=0-9"), ("if-range", "\"x\"")],
        ),
        ("garbage range", &huge, vec![("range", "pages=1-2")]),
        ("edge whole", &edge, vec![]),
        ("edge open", &edge, vec![("range", "bytes=0-")]),
        ("edge one over", &edge, vec![("range", "bytes=0-5242880")]),
    ] {
        rig.tap.clear();
        let refused = rig.preview(file, &headers).await?;
        assert_envelope(&refused, 413, "PREVIEW_TOO_LARGE", what)?;
        assert!(refused.body.len() < 1_024, "{what}");
        assert!(
            rig.tap.object_reads().is_empty(),
            "{what}: no object byte left the provider"
        );
    }

    rig.tap.clear();
    let exactly = rig.preview(&edge, &[("range", "bytes=1-")]).await?;
    assert_eq!(exactly.status, 206, "5 MiB resolved is allowed");
    assert_eq!(exactly.body.len() as u64, CAP);
    assert_eq!(exactly.body, expected_bytes(edge_size, 1, edge_size)?);
    let reads = rig.tap.object_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].served, Some(CAP));

    for (_, file) in [("etag", &huge)] {
        let head = rig.preview(file, &[("range", "bytes=0-9")]).await?;
        let own = head.require("etag")?.to_owned();
        let conditional = rig
            .preview(
                file,
                &[("range", "bytes=0-524287"), ("if-none-match", &own)],
            )
            .await?;
        assert_eq!(conditional.status, 304);
        let whole_with_match = rig.preview(file, &[("if-none-match", &own)]).await?;
        assert_envelope(&whole_with_match, 413, "PREVIEW_TOO_LARGE", "no 304 bypass")?;
        let matched = rig
            .preview(file, &[("range", "bytes=0-9"), ("if-range", &own)])
            .await?;
        assert_eq!(matched.status, 206);
    }

    let foreign = rig
        .fetch
        .get(&preview_path(&huge), Some(&rig.other), &[])
        .await?;
    assert_envelope(&foreign, 404, "FILE_NOT_FOUND", "foreign oversized text")?;
    let download = rig
        .fetch
        .get(&content_path(&huge), Some(&rig.owner), &[])
        .await?;
    assert_eq!(download.status, 302, "downloads have no text limit");
    Ok(())
}

#[cfg(feature = "e2e-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_proxy_fallback_real_minio_e2e_fixture() -> Result<()> {
    let rig = rig(false).await?;
    let size = 12 * MIB + 4_321;
    let last = size - 1;
    let file = rig.upload("movie.mp4", size).await?;
    let digest = generated_digest(size)?;

    rig.tap.clear();
    let full = rig
        .preview(&file, &[("accept-encoding", "gzip, br, zstd")])
        .await?;
    assert_eq!(full.status, 200, "no redirect when presigned GET is off");
    assert!(full.header("location").is_none());
    assert_eq!(full.require("content-length")?, size.to_string());
    assert_eq!(full.sha256(), digest);
    assert_eq!(full.require("accept-ranges")?, "bytes");
    assert_eq!(full.require("content-type")?, "video/mp4");
    assert_eq!(full.require("x-content-type-options")?, "nosniff");
    assert_eq!(full.require("cache-control")?, "private, no-cache");
    assert!(full.header("content-encoding").is_none());
    assert_eq!(
        disposition_kind(full.require("content-disposition")?)?,
        "inline"
    );
    let etag = full.require("etag")?.to_owned();
    assert_eq!(rig.tap.object_reads().len(), 1);

    for (header, first, end) in [
        ("bytes=0-1023".to_owned(), 0, 1_023),
        ("bytes=5000000-5000099".to_owned(), 5_000_000, 5_000_099),
        ("bytes=-512".to_owned(), size - 512, last),
        (format!("bytes={}-", size - 10), size - 10, last),
    ] {
        rig.tap.clear();
        let reply = rig.preview(&file, &[("range", &header)]).await?;
        assert_eq!(reply.status, 206, "{header}");
        assert_eq!(
            reply.require("content-range")?,
            format!("bytes {first}-{end}/{size}")
        );
        assert_eq!(reply.body, expected_bytes(size, first, end + 1)?);
        assert_eq!(
            disposition_kind(reply.require("content-disposition")?)?,
            "inline"
        );
        let reads = rig.tap.object_reads();
        assert_eq!(reads.len(), 1, "{header}");
        assert_eq!(reads[0].served, Some(end - first + 1), "{header}");
    }
    for header in [format!("bytes={size}-"), "bytes=-0".to_owned()] {
        rig.tap.clear();
        let reply = rig.preview(&file, &[("range", &header)]).await?;
        assert_envelope(&reply, 416, "RANGE_NOT_SATISFIABLE", &header)?;
        assert_eq!(reply.require("content-range")?, format!("bytes */{size}"));
        assert!(rig.tap.object_reads().is_empty());
    }
    let matching = rig
        .preview(&file, &[("range", "bytes=0-9"), ("if-range", &etag)])
        .await?;
    assert_eq!(matching.status, 206);
    let stale = rig
        .preview(&file, &[("range", "bytes=0-9"), ("if-range", "\"0000\"")])
        .await?;
    assert_eq!(stale.status, 200);
    assert_eq!(stale.body.len() as u64, size);
    rig.tap.clear();
    let unchanged = rig.preview(&file, &[("if-none-match", &etag)]).await?;
    assert_eq!(unchanged.status, 304);
    assert!(rig.tap.object_reads().is_empty());

    let text = rig.upload("notes.txt", 700 * 1024).await?;
    rig.tap.clear();
    let prefix = rig.preview(&text, &[("range", "bytes=0-524287")]).await?;
    assert_eq!(prefix.status, 206);
    assert_eq!(prefix.require("content-type")?, TEXT_TYPE);
    assert_eq!(prefix.body, expected_bytes(700 * 1024, 0, PREFIX)?);
    assert_eq!(rig.tap.object_reads()[0].served, Some(PREFIX));

    let big_text = rig.upload("big.log", 8 * MIB).await?;
    rig.tap.clear();
    let refused = rig.preview(&big_text, &[]).await?;
    assert_envelope(&refused, 413, "PREVIEW_TOO_LARGE", "proxied text")?;
    assert!(rig.tap.object_reads().is_empty());

    let svg = rig.upload("drawing.svg", 20_000).await?;
    rig.tap.clear();
    let unsafe_type = rig.preview(&svg, &[]).await?;
    assert_envelope(&unsafe_type, 415, "PREVIEW_UNSUPPORTED", "svg")?;
    assert!(rig.tap.all().is_empty());

    let foreign = rig
        .fetch
        .get(
            &preview_path(&file),
            Some(&rig.other),
            &[("range", "bytes=0-9")],
        )
        .await?;
    assert_envelope(&foreign, 404, "FILE_NOT_FOUND", "foreign")?;

    for extra in ["", "Range: bytes=7-\r\n", "Range: bytes=-1048577\r\n"] {
        let mut stream = raw_get_path(&rig.base, &preview_path(&file), &rig.owner, extra).await?;
        let (head, _) = read_head(&mut stream).await?;
        assert!(head.header("transfer-encoding").is_none(), "{extra:?}");
        let declared: u64 = head
            .header("content-length")
            .context("Content-Length")?
            .parse()?;
        let mut received = 0_u64;
        let mut buffer = vec![0_u8; 256 * 1024];
        while received < declared {
            let read = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer).await?;
            ensure!(read > 0, "closed after {received} of {declared}");
            received += read as u64;
        }
        assert_eq!(received, declared, "{extra:?}");
    }
    Ok(())
}

#[cfg(feature = "e2e-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_proxy_disconnect_releases_permits_e2e_fixture() -> Result<()> {
    let rig = rig(false).await?;
    let size = 12 * MIB + 4_321;
    let file = rig.upload("abandoned.mp4", size).await?;
    for round in 0..140 {
        let headers: Vec<(&str, &str)> = if round % 2 == 0 {
            vec![]
        } else {
            vec![("range", "bytes=1000000-")]
        };
        let mut response = rig
            .fetch
            .open(&preview_path(&file), Some(&rig.owner), &headers)
            .await?;
        let mut read = 0;
        while read < 300_000 {
            read += response.chunk().await?.context("an early end")?.len();
        }
        drop(response);
    }
    let mut tasks = Vec::new();
    for _ in 0..128 {
        let fetch = rig.fetch.clone();
        let creds = rig.owner.clone();
        let path = preview_path(&file);
        tasks.push(tokio::spawn(async move {
            fetch
                .get(&path, Some(&creds), &[("range", "bytes=0-99")])
                .await
                .map(|reply| reply.status)
        }));
    }
    for task in tasks {
        assert_eq!(task.await??, 206, "no permit leaked by abandoned previews");
    }
    let whole = rig.preview(&file, &[]).await?;
    assert_eq!(whole.sha256(), generated_digest(size)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_preview_s3_post_redirect_response_contract_real_minio() -> Result<()> {
    let rig = rig(true).await?;
    let mut secrets = vec![
        rig._minio.access_key().to_owned(),
        rig._minio.secret_key().to_owned(),
    ];
    for (name, mime) in [
        ("photo.png", "image/png"),
        ("photo.webp", "image/webp"),
        ("clip.mp4", "video/mp4"),
        ("clip.webm", "video/webm"),
        ("song.mp3", "audio/mpeg"),
        ("paper.pdf", "application/pdf"),
    ] {
        let file = rig.upload(name, MEDIA).await?;
        assert_eq!(rig.stored_mime(&file).await?, mime, "{name}");

        let issued = rig.preview(&file, &[]).await?;
        assert_eq!(issued.status, 302, "{name}");
        assert_eq!(issued.require("x-content-type-options")?, "nosniff");
        assert_eq!(issued.require("cache-control")?, "no-store");
        assert!(issued.header("content-type").is_none(), "{name}");
        let location = Url::parse(issued.require("location")?)?;
        if let Some(signature) = pair(&location, "X-Amz-Signature") {
            secrets.push(signature);
        }

        let full = fetch_url(&location, &[]).await?;
        assert_eq!(full.status, 200, "{name}");
        assert_eq!(
            full.require("content-type")?,
            mime,
            "{name}: the signed type"
        );
        assert_eq!(
            full.require("content-disposition")?,
            format!("inline; filename=\"{name}\"; filename*=UTF-8\'\'{name}"),
            "{name}: the signed disposition"
        );
        assert_eq!(full.require("content-length")?, MEDIA.to_string());
        assert!(full.header("content-range").is_none());
        assert!(full.require("etag")?.starts_with('"'));
        assert_eq!(full.sha256(), generated_digest(MEDIA)?, "{name}");
        assert_eq!(
            full.require("x-content-type-options")?,
            "nosniff",
            "{name}: emitted by the pinned MinIO target on the final 200; not signable, so a provider property"
        );

        for (header, from, to) in [
            ("bytes=10-19".to_owned(), 10, 20),
            (format!("bytes={}-", MEDIA - 100), MEDIA - 100, MEDIA),
            ("bytes=-64".to_owned(), MEDIA - 64, MEDIA),
        ] {
            let part = fetch_url(&location, &[("range", &header)]).await?;
            assert_eq!(part.status, 206, "{name} {header}");
            assert_eq!(part.require("content-type")?, mime, "{name} {header}");
            assert_eq!(
                disposition_kind(part.require("content-disposition")?)?,
                "inline"
            );
            assert_eq!(part.require("content-length")?, (to - from).to_string());
            assert_eq!(
                part.require("content-range")?,
                format!("bytes {from}-{}/{MEDIA}", to - 1)
            );
            assert!(part.require("etag")?.starts_with('"'));
            assert_eq!(
                part.body,
                expected_bytes(MEDIA, from, to)?,
                "{name} {header}"
            );
            assert_eq!(
                part.require("x-content-type-options")?,
                "nosniff",
                "{name} {header}: provider property on the final 206"
            );
        }

        let flipped_type = with_pair(&location, "response-content-type", Some("text/html"));
        let flipped_disposition = with_pair(
            &location,
            "response-content-disposition",
            Some("attachment; filename=\"x.html\""),
        );
        let stripped_type = with_pair(&location, "response-content-type", None);
        let stripped_disposition = with_pair(&location, "response-content-disposition", None);
        let longer = with_pair(&location, "X-Amz-Expires", Some("604800"));
        for (what, tampered) in [
            ("content-type overridden", flipped_type),
            ("disposition overridden", flipped_disposition),
            ("content-type stripped", stripped_type),
            ("disposition stripped", stripped_disposition),
            ("expiry extended", longer),
        ] {
            let refused = fetch_url(&tampered, &[]).await?;
            assert_eq!(refused.status, 403, "{name}: {what}");
            assert!(
                !refused
                    .body
                    .windows(32)
                    .any(|w| w == &expected_bytes(MEDIA, 0, 32).unwrap()[..]),
                "{name}: {what}: no object bytes"
            );
        }
    }

    for (name, mime) in [
        ("drawing.svg", "image/svg+xml"),
        ("page.html", "text/html"),
        ("data.xml", "text/xml"),
        ("archive.zip", "application/zip"),
    ] {
        let file = rig.upload(name, 30_000).await?;
        assert_eq!(rig.stored_mime(&file).await?, mime);
        rig.tap.clear();
        let refused = rig.preview(&file, &[]).await?;
        assert_envelope(&refused, 415, "PREVIEW_UNSUPPORTED", name)?;
        assert!(refused.header("location").is_none());
        assert!(
            rig.tap.all().is_empty(),
            "{name}: no provider request of any kind"
        );
    }

    let text = rig.upload("notes.txt", 20_000).await?;
    let proxied = rig.preview(&text, &[]).await?;
    assert_eq!(proxied.status, 200);
    assert!(proxied.header("location").is_none());
    assert_eq!(proxied.require("content-type")?, TEXT_TYPE);
    assert_eq!(proxied.require("x-content-type-options")?, "nosniff");

    let logs = rig._process.log_text()?;
    for secret in secrets
        .iter()
        .map(String::as_str)
        .chain(["X-Amz-Signature", "X-Amz-Credential"])
    {
        assert!(!logs.contains(secret), "the server log holds {secret}");
    }
    Ok(())
}

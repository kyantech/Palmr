#![cfg(feature = "e2e-fixture")]

pub mod support;

use std::ffi::OsString;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use percent_encoding::percent_decode_str;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use unicode_normalization::UnicodeNormalization;
use url::Url;
use uuid::Uuid;

use support::client::{Creds, Db, Http};
use support::download::{
    content_path, expected_bytes, raw_get, read_head, upload_s3, Fetch, Fetcher,
};
use support::minio::MinioServer;
use support::tap::Tap;
use support::tus::{generated_digest, MIB};
use support::ServerProcess;

const NAME: &str = "Relatório Anual (2026).pdf";
const SIZE: u64 = 12 * MIB + 4_321;

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
    base: Url,
}

async fn rig() -> Result<Rig> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("proxy").await?;
    let public = Url::parse(minio.endpoint())?;
    let internal = format!("http://127.0.0.1:{}", public.port().context("port")?);
    let tap = Tap::start(&internal).await?;
    let tapped = tap.endpoint();
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let env: Vec<(String, OsString)> = [
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_PROFILE", "minio"),
        ("PALMR_S3_ENDPOINT", tapped.as_str()),
        ("PALMR_S3_PUBLIC_ENDPOINT", minio.endpoint()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", bucket.as_str()),
        ("PALMR_S3_ACCESS_KEY", minio.access_key()),
        ("PALMR_S3_SECRET_KEY", minio.secret_key()),
        ("PALMR_S3_FORCE_PATH_STYLE", "true"),
        ("PALMR_E2E_S3_PRESIGNED_GET", "off"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), OsString::from(value)))
    .collect();
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
        base,
    })
}

impl Rig {
    async fn upload(&self, name: &str, size: u64) -> Result<String> {
        upload_s3(&self.http, &self.db, &self.owner, name, size).await
    }

    async fn get(&self, file: &str, query: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        let path = if query.is_empty() {
            content_path(file)
        } else {
            format!("{}?{query}", content_path(file))
        };
        self.fetch.get(&path, Some(&self.owner), headers).await
    }
}

fn disposition_name(value: &str) -> Result<(String, String)> {
    let (kind, rest) = value.split_once("; filename=\"").context("filename")?;
    let (_, encoded) = rest.split_once("filename*=UTF-8''").context("filename*")?;
    ensure!(!value.contains(['\r', '\n']), "control characters");
    Ok((
        kind.to_owned(),
        percent_decode_str(encoded).decode_utf8()?.into_owned(),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_proxy_fallback_real_minio_e2e_fixture() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload(NAME, SIZE).await?;
    let digest = generated_digest(SIZE)?;
    let last = SIZE - 1;

    rig.tap.clear();
    let full = rig
        .get(&file, "", &[("accept-encoding", "gzip, br, zstd")])
        .await?;
    assert_eq!(
        full.status, 200,
        "no redirect when the profile forbids presigned GET"
    );
    assert!(full.header("location").is_none());
    assert_eq!(full.require("content-length")?, SIZE.to_string());
    assert_eq!(full.sha256(), digest);
    assert_eq!(full.require("accept-ranges")?, "bytes");
    assert_eq!(full.require("content-type")?, "application/pdf");
    assert_eq!(full.require("x-content-type-options")?, "nosniff");
    assert_eq!(full.require("cache-control")?, "private, no-cache");
    assert!(full.header("content-encoding").is_none());
    let (kind, name) = disposition_name(full.require("content-disposition")?)?;
    assert_eq!(kind, "attachment");
    assert_eq!(name, NAME.nfc().collect::<String>());
    let etag = full.require("etag")?.to_owned();
    assert!(etag.starts_with('"'), "{etag}");
    let reads = rig.tap.object_reads();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].range, None);

    for (header, first, end, upstream) in [
        (
            "bytes=0-1023".to_owned(),
            0,
            1023,
            "bytes=0-1023".to_owned(),
        ),
        (
            "bytes=5000000-5000099".to_owned(),
            5_000_000,
            5_000_099,
            "bytes=5000000-5000099".to_owned(),
        ),
        (
            "bytes=-512".to_owned(),
            SIZE - 512,
            last,
            format!("bytes={}-{last}", SIZE - 512),
        ),
        (
            format!("bytes={}-", SIZE - 10),
            SIZE - 10,
            last,
            format!("bytes={}-{last}", SIZE - 10),
        ),
        (
            format!("bytes={}-{}", SIZE - 5, SIZE + 1_000),
            SIZE - 5,
            last,
            format!("bytes={}-{last}", SIZE - 5),
        ),
        (
            "bytes=1000000-1000009,5-9".to_owned(),
            1_000_000,
            1_000_009,
            "bytes=1000000-1000009".to_owned(),
        ),
    ] {
        rig.tap.clear();
        let reply = rig.get(&file, "", &[("range", &header)]).await?;
        assert_eq!(reply.status, 206, "{header}");
        assert_eq!(
            reply.require("content-range")?,
            format!("bytes {first}-{end}/{SIZE}")
        );
        assert_eq!(
            reply.require("content-length")?,
            (end - first + 1).to_string()
        );
        assert_eq!(reply.body, expected_bytes(SIZE, first, end + 1)?);
        let reads = rig.tap.object_reads();
        assert_eq!(reads.len(), 1, "{header}: one provider read");
        assert_eq!(
            reads[0].range.as_deref(),
            Some(upstream.as_str()),
            "{header}"
        );
        assert_eq!(
            reads[0].served,
            Some(end - first + 1),
            "{header}: the provider sent only the requested bytes"
        );
    }

    for header in [
        format!("bytes={SIZE}-"),
        "bytes=99999999999-".to_owned(),
        "bytes=-0".to_owned(),
    ] {
        rig.tap.clear();
        let reply = rig.get(&file, "", &[("range", &header)]).await?;
        assert_eq!(reply.status, 416, "{header}");
        assert_eq!(reply.require("content-range")?, format!("bytes */{SIZE}"));
        assert_eq!(reply.error_code()?, "RANGE_NOT_SATISFIABLE");
        assert!(
            rig.tap.object_reads().is_empty(),
            "{header}: no object byte was read"
        );
    }

    let matching = rig
        .get(&file, "", &[("range", "bytes=0-9"), ("if-range", &etag)])
        .await?;
    assert_eq!(matching.status, 206);
    let stale = rig
        .get(
            &file,
            "",
            &[("range", "bytes=0-9"), ("if-range", "\"0000\"")],
        )
        .await?;
    assert_eq!(
        stale.status, 200,
        "the proxied path restarts on a stale If-Range"
    );
    assert_eq!(stale.body.len() as u64, SIZE);
    let weak = rig
        .get(
            &file,
            "",
            &[("range", "bytes=0-9"), ("if-range", &format!("W/{etag}"))],
        )
        .await?;
    assert_eq!(weak.status, 200);
    rig.tap.clear();
    let unchanged = rig.get(&file, "", &[("if-none-match", &etag)]).await?;
    assert_eq!(unchanged.status, 304);
    assert!(
        rig.tap.object_reads().is_empty(),
        "a 304 reads no object bytes"
    );

    let inline = rig.get(&file, "disposition=inline", &[]).await?;
    assert!(inline
        .require("content-disposition")?
        .starts_with("inline;"));

    let foreign = rig
        .fetch
        .get(
            &content_path(&file),
            Some(&rig.other),
            &[("range", "bytes=0-9")],
        )
        .await?;
    assert_eq!(
        (foreign.status, foreign.error_code()?.as_str()),
        (404, "FILE_NOT_FOUND")
    );
    rig.tap.clear();
    let unknown = rig
        .fetch
        .get(
            &content_path(&Uuid::now_v7().to_string()),
            Some(&rig.owner),
            &[],
        )
        .await?;
    assert_eq!(unknown.status, 404);
    assert!(rig.tap.object_reads().is_empty());

    for extra in ["", "Range: bytes=7-\r\n", "Range: bytes=-1048577\r\n"] {
        let mut stream = raw_get(&rig.base, &file, &rig.owner, extra).await?;
        let (head, _) = read_head(&mut stream).await?;
        assert!(head.header("transfer-encoding").is_none(), "{extra:?}");
        let declared: u64 = head
            .header("content-length")
            .context("Content-Length")?
            .parse()?;
        let mut received = 0_u64;
        let mut buffer = vec![0_u8; 256 * 1024];
        while received < declared {
            let read = stream.read(&mut buffer).await?;
            ensure!(read > 0, "closed after {received} of {declared}");
            received += read as u64;
        }
        assert_eq!(received, declared, "{extra:?}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_proxy_mime_and_disposition_e2e_fixture() -> Result<()> {
    let rig = rig().await?;
    for (name, mime, safe) in [
        ("clip.mp4", "video/mp4", true),
        ("clip.webm", "video/webm", true),
        ("song.mp3", "audio/mpeg", true),
        ("song.ogg", "audio/ogg", true),
        ("paper.pdf", "application/pdf", true),
        ("photo.png", "image/png", true),
        ("drawing.svg", "image/svg+xml", false),
        ("notes.txt", "text/plain", true),
    ] {
        let file = rig.upload(name, 20_000).await?;
        let stored = rig
            .db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await?;
        assert_eq!(stored, mime, "{name}");
        let inline = rig.get(&file, "disposition=inline", &[]).await?;
        assert_eq!(inline.status, 200, "{name}");
        assert_eq!(inline.require("content-type")?, mime);
        assert_eq!(inline.require("x-content-type-options")?, "nosniff");
        let (kind, _) = disposition_name(inline.require("content-disposition")?)?;
        assert_eq!(kind, if safe { "inline" } else { "attachment" }, "{name}");
        assert_eq!(inline.body, expected_bytes(20_000, 0, 20_000)?);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_proxy_disconnect_releases_permits_e2e_fixture() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("abandoned.bin", SIZE).await?;
    for round in 0..140 {
        let headers: Vec<(&str, &str)> = if round % 2 == 0 {
            vec![]
        } else {
            vec![("range", "bytes=1000000-")]
        };
        let mut response = rig
            .fetch
            .open(&content_path(&file), Some(&rig.owner), &headers)
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
        let path = content_path(&file);
        tasks.push(tokio::spawn(async move {
            fetch
                .get(&path, Some(&creds), &[("range", "bytes=0-99")])
                .await
                .map(|reply| reply.status)
        }));
    }
    for task in tasks {
        assert_eq!(
            task.await??,
            206,
            "no permit leaked by abandoned proxied reads"
        );
    }
    let whole = rig.get(&file, "", &[]).await?;
    assert_eq!(whole.sha256(), generated_digest(SIZE)?);
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(())
}

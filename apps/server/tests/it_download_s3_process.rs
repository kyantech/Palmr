pub mod support;

use std::ffi::OsString;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use percent_encoding::percent_decode_str;
use tempfile::TempDir;
use unicode_normalization::UnicodeNormalization;
use url::Url;
use uuid::Uuid;

use support::client::{Creds, Db, Http};
use support::download::{content_path, expected_bytes, upload_s3, Fetch, Fetcher};
use support::minio::MinioServer;
use support::tus::{generated_digest, MIB};
use support::ServerProcess;

const NAME: &str = "Relatório Anual (2026).pdf";
const SIZE: u64 = 12 * MIB + 4_321;

struct Rig {
    _root: TempDir,
    _process: ServerProcess,
    minio: MinioServer,
    bucket: String,
    http: Http,
    db: Db,
    fetch: Fetcher,
    owner: Creds,
    other: Creds,
    public: Url,
    base: Url,
}

fn storage_env(minio: &MinioServer, bucket: &str) -> Result<Vec<(String, OsString)>> {
    let public = Url::parse(minio.endpoint())?;
    let internal = format!("http://127.0.0.1:{}", public.port().context("port")?);
    Ok([
        ("PALMR_STORAGE_PROVIDER", "s3"),
        ("PALMR_S3_PROFILE", "minio"),
        ("PALMR_S3_ENDPOINT", internal.as_str()),
        ("PALMR_S3_PUBLIC_ENDPOINT", minio.endpoint()),
        ("PALMR_S3_REGION", "us-east-1"),
        ("PALMR_S3_BUCKET", bucket),
        ("PALMR_S3_ACCESS_KEY", minio.access_key()),
        ("PALMR_S3_SECRET_KEY", minio.secret_key()),
        ("PALMR_S3_FORCE_PATH_STYLE", "true"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), OsString::from(value)))
    .collect())
}

async fn rig() -> Result<Rig> {
    let minio = MinioServer::start().await?;
    let bucket = minio.create_bucket("download").await?;
    let root = TempDir::new()?;
    let data_dir = root.path().join("data");
    std::fs::create_dir(&data_dir)?;
    let env = storage_env(&minio, &bucket)?;
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
        public: Url::parse(minio.endpoint())?,
        base: base.clone(),
        fetch: Fetcher::new(&base)?,
        db: Db::new(&data_dir),
        _root: root,
        _process: process,
        minio,
        bucket,
        http,
        owner,
        other,
    })
}

impl Rig {
    async fn upload(&self, name: &str, size: u64) -> Result<String> {
        upload_s3(&self.http, &self.db, &self.owner, name, size).await
    }

    async fn redirect(&self, file: &str, query: &str) -> Result<Fetch> {
        let path = if query.is_empty() {
            content_path(file)
        } else {
            format!("{}?{query}", content_path(file))
        };
        self.fetch.get(&path, Some(&self.owner), &[]).await
    }

    async fn location(&self, file: &str, query: &str) -> Result<Url> {
        let reply = self.redirect(file, query).await?;
        ensure!(reply.status == 302, "{}: {}", reply.status, reply.text());
        Ok(Url::parse(reply.require("location")?)?)
    }
}

fn pair(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.into_owned())
}

fn with_pair(url: &Url, name: &str, value: Option<&str>) -> Url {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !key.eq_ignore_ascii_case(name))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut tampered = url.clone();
    {
        let mut query = tampered.query_pairs_mut();
        query.clear();
        for (key, value) in &pairs {
            query.append_pair(key, value);
        }
        if let Some(value) = value {
            query.append_pair(name, value);
        }
    }
    tampered
}

fn direct() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()?)
}

async fn fetch_url(url: &Url, headers: &[(&str, &str)]) -> Result<Fetch> {
    let mut request = direct()?.get(url.clone());
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    support::download::collect(request.send().await?).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_302_presigned_pinned_headers() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload(NAME, SIZE).await?;
    let digest = generated_digest(SIZE)?;
    let stored_mime = rig
        .db
        .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
        .await?;
    assert_eq!(stored_mime, "application/pdf");

    let issued = rig.redirect(&file, "").await?;
    assert_eq!(issued.status, 302, "{}", issued.text());
    assert!(issued.body.is_empty());
    assert_eq!(issued.require("cache-control")?, "no-store");
    assert!(issued.header("etag").is_none() && issued.header("content-type").is_none());
    let location = Url::parse(issued.require("location")?)?;

    assert_eq!(location.scheme(), rig.public.scheme());
    assert_eq!(
        location.host_str(),
        rig.public.host_str(),
        "the public endpoint signs the URL"
    );
    assert_eq!(location.port(), rig.public.port());
    assert_ne!(
        location.host_str(),
        Some("127.0.0.1"),
        "never the internal endpoint"
    );
    assert!(location
        .path()
        .starts_with(&format!("/{}/objects/", rig.bucket)));
    assert_eq!(pair(&location, "X-Amz-Expires").as_deref(), Some("300"));
    assert_eq!(
        pair(&location, "X-Amz-SignedHeaders").as_deref(),
        Some("host")
    );
    assert!(pair(&location, "X-Amz-Signature").is_some());
    assert_eq!(
        pair(&location, "response-content-type").as_deref(),
        Some("application/pdf")
    );
    let pinned = pair(&location, "response-content-disposition").context("pinned disposition")?;
    assert!(pinned
        .starts_with("attachment; filename=\"Relatorio Anual _2026_.pdf\"; filename*=UTF-8''"));

    let inline = rig.location(&file, "disposition=inline").await?;
    assert!(pair(&inline, "response-content-disposition")
        .context("inline disposition")?
        .starts_with("inline; filename="));
    let unlike = rig
        .redirect(&file, "disposition=inline&x=../../etc")
        .await?;
    assert_eq!(
        unlike.status, 302,
        "unknown query members never become keys or paths"
    );

    let fetched = fetch_url(&location, &[]).await?;
    assert_eq!(fetched.status, 200, "{}", fetched.text());
    assert_eq!(fetched.body.len() as u64, SIZE);
    assert_eq!(fetched.sha256(), digest, "the actual bytes come from MinIO");
    assert_eq!(fetched.require("content-type")?, "application/pdf");
    assert_eq!(fetched.require("content-disposition")?, pinned);
    let (_, encoded) = pinned
        .split_once("filename*=UTF-8''")
        .context("filename*")?;
    assert_eq!(
        percent_decode_str(encoded).decode_utf8()?.into_owned(),
        NAME.nfc().collect::<String>()
    );

    for (name, value) in [
        ("response-content-type", Some("text/html")),
        ("response-content-disposition", Some("inline")),
        ("response-content-type", None),
        ("response-content-disposition", None),
        ("X-Amz-Expires", Some("604800")),
        ("X-Amz-Signature", Some("00")),
    ] {
        let tampered = with_pair(&location, name, value);
        let reply = fetch_url(&tampered, &[]).await?;
        assert_eq!(
            reply.status, 403,
            "changing {name} to {value:?} must break the signature"
        );
        assert!(
            reply.text().contains("SignatureDoesNotMatch") || reply.text().contains("AccessDenied")
        );
    }

    let foreign = rig
        .fetch
        .get(&content_path(&file), Some(&rig.other), &[])
        .await?;
    assert_eq!(foreign.status, 404);
    assert_eq!(foreign.error_code()?, "FILE_NOT_FOUND");
    assert!(
        foreign.header("location").is_none(),
        "no capability for a foreign file"
    );
    assert!(!foreign.text().contains("X-Amz"));
    let ranged_foreign = rig
        .fetch
        .get(
            &content_path(&file),
            Some(&rig.other),
            &[("range", "bytes=0-9")],
        )
        .await?;
    assert_eq!(ranged_foreign.status, 404);
    let anonymous = rig.fetch.get(&content_path(&file), None, &[]).await?;
    assert_eq!(
        (anonymous.status, anonymous.error_code()?.as_str()),
        (401, "AUTH_REQUIRED")
    );
    let unknown = rig
        .fetch
        .get(
            &content_path(&Uuid::now_v7().to_string()),
            Some(&rig.owner),
            &[],
        )
        .await?;
    assert_eq!(unknown.status, 404);
    let bad_disposition = rig.redirect(&file, "disposition=bogus").await?;
    assert_eq!(
        (
            bad_disposition.status,
            bad_disposition.error_code()?.as_str()
        ),
        (422, "VALIDATION_ERROR")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_presigned_get_real_minio() -> Result<()> {
    let rig = rig().await?;
    let cases = [
        ("clip.mp4", "video/mp4", "attachment"),
        ("song.mp3", "audio/mpeg", "attachment"),
        ("photo.png", "image/png", "attachment"),
        ("drawing.svg", "image/svg+xml", "attachment"),
        ("notes.txt", "text/plain", "attachment"),
    ];
    for (name, mime, kind) in cases {
        let file = rig.upload(name, 30_000).await?;
        let stored = rig
            .db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await?;
        assert_eq!(stored, mime, "{name}");
        let location = rig.location(&file, "disposition=inline").await?;
        assert_eq!(
            pair(&location, "response-content-type").as_deref(),
            Some(mime)
        );
        let served = fetch_url(&location, &[]).await?;
        assert_eq!(served.status, 200);
        assert_eq!(served.require("content-type")?, mime, "{name}");
        let wanted = if mime == "image/svg+xml" {
            kind
        } else {
            "inline"
        };
        assert!(
            served.require("content-disposition")?.starts_with(wanted),
            "{name}: {}",
            served.require("content-disposition")?
        );
        assert_eq!(served.body, expected_bytes(30_000, 0, 30_000)?);
    }

    let file = rig.upload("fresh.bin", 20_000).await?;
    let first = rig.location(&file, "").await?;
    let second = rig.location(&file, "").await?;
    for url in [&first, &second] {
        assert_eq!(fetch_url(url, &[]).await?.status, 200);
    }
    assert!(
        rig.redirect(&file, "").await?.header("etag").is_none(),
        "a redirect carries no validator of its own"
    );

    assert!(
        rig.minio
            .object_exists(&rig.bucket, pair_key(&first, &rig.bucket)?.as_str())
            .await
    );
    Ok(())
}

fn pair_key(url: &Url, bucket: &str) -> Result<String> {
    Ok(url
        .path()
        .strip_prefix(&format!("/{bucket}/"))
        .context("the bucket prefix")?
        .to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_range_real_minio() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("ranged.bin", SIZE).await?;
    let location = rig.location(&file, "").await?;
    let last = SIZE - 1;

    let whole = fetch_url(&location, &[]).await?;
    assert_eq!(whole.status, 200);
    assert_eq!(whole.require("accept-ranges")?, "bytes");
    assert_eq!(whole.require("content-length")?, SIZE.to_string());
    let etag = whole.require("etag")?.to_owned();

    for (header, first, end) in [
        ("bytes=0-1023".to_owned(), 0, 1023),
        ("bytes=1024-".to_owned(), 1024, last),
        ("bytes=-512".to_owned(), SIZE - 512, last),
        (
            format!("bytes={}-{}", SIZE - 10, SIZE + 99),
            SIZE - 10,
            last,
        ),
    ] {
        let reply = fetch_url(&location, &[("range", &header)]).await?;
        assert_eq!(reply.status, 206, "{header}");
        assert_eq!(
            reply.require("content-range")?,
            format!("bytes {first}-{end}/{SIZE}")
        );
        assert_eq!(
            reply.body,
            expected_bytes(SIZE, first, end + 1)?,
            "{header}"
        );
    }
    let unsatisfiable = fetch_url(&location, &[("range", "bytes=99999999999-")]).await?;
    assert_eq!(unsatisfiable.status, 416);

    let matching = fetch_url(&location, &[("range", "bytes=0-9"), ("if-range", &etag)]).await?;
    assert_eq!(
        matching.status, 206,
        "a matching If-Range is honoured by the provider"
    );
    let stale = fetch_url(
        &location,
        &[("range", "bytes=0-9"), ("if-range", "\"0000\"")],
    )
    .await?;
    assert_eq!(
        stale.status, 206,
        "characterization: this MinIO ignores If-Range and answers the Range; the proxied path implements it"
    );
    let again = fetch_url(&location, &[]).await?;
    assert_eq!(
        again.require("etag")?,
        etag,
        "the object behind one key never changes"
    );
    assert_eq!(again.sha256(), generated_digest(SIZE)?);

    let followed = reqwest::Client::builder().build()?;
    let through_palmr = followed
        .get(rig.base.join(content_path(&file).trim_start_matches('/'))?)
        .header(
            "cookie",
            format!(
                "palmr_session={}; palmr_csrf={}",
                rig.owner.session, rig.owner.csrf
            ),
        )
        .header("range", "bytes=100-199")
        .send()
        .await?;
    assert_eq!(
        through_palmr.status().as_u16(),
        206,
        "a redirect-following client re-sends Range to the presigned URL (clients differ; this one does)"
    );
    assert_eq!(
        through_palmr.bytes().await?.to_vec(),
        expected_bytes(SIZE, 100, 200)?
    );

    let unconditional = rig
        .fetch
        .get(
            &content_path(&file),
            Some(&rig.owner),
            &[("range", "bytes=100-199")],
        )
        .await?;
    assert_eq!(
        unconditional.status, 302,
        "a Range without a validator is still served by the provider"
    );
    Ok(())
}

impl Rig {
    async fn conditional(&self, file: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        self.fetch
            .get(&content_path(file), Some(&self.owner), headers)
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_conditional_range_real_minio() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("conditional.bin", SIZE).await?;
    let digest = generated_digest(SIZE)?;
    let last = SIZE - 1;

    let location = rig.location(&file, "").await?;
    let provider = fetch_url(&location, &[]).await?;
    assert_eq!(provider.status, 200);
    let provider_etag = provider.require("etag")?.to_owned();
    let provider_modified = provider.require("last-modified")?.to_owned();
    assert!(provider_etag.starts_with('"'), "{provider_etag}");

    let plain = rig.conditional(&file, &[]).await?;
    assert_eq!(
        plain.status, 302,
        "1: a plain GET stays a direct presigned download"
    );
    let direct = fetch_url(&Url::parse(plain.require("location")?)?, &[]).await?;
    assert_eq!((direct.status, direct.sha256()), (200, digest.clone()));

    let bare = rig
        .conditional(&file, &[("range", "bytes=100-199")])
        .await?;
    assert_eq!(bare.status, 302, "2: a Range without If-Range stays direct");
    let slice = fetch_url(
        &Url::parse(bare.require("location")?)?,
        &[("range", "bytes=100-199")],
    )
    .await?;
    assert_eq!(slice.status, 206);
    assert_eq!(slice.body, expected_bytes(SIZE, 100, 200)?);

    let with = |validator: &str| -> Vec<(&'static str, String)> {
        vec![
            ("range", "bytes=100-199".to_owned()),
            ("if-range", validator.to_owned()),
        ]
    };
    let send = |headers: Vec<(&'static str, String)>| {
        let rig = &rig;
        let file = file.clone();
        async move {
            let borrowed: Vec<(&str, &str)> =
                headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
            rig.conditional(&file, &borrowed).await
        }
    };

    let matching = send(with(&provider_etag)).await?;
    assert_eq!(
        matching.status, 206,
        "3: the validator the client holds from S3 is honoured"
    );
    assert!(matching.header("location").is_none());
    assert_eq!(
        matching.require("content-range")?,
        format!("bytes 100-199/{SIZE}")
    );
    assert_eq!(matching.require("content-length")?, "100");
    assert_eq!(matching.body, expected_bytes(SIZE, 100, 200)?);
    let canonical = matching.require("etag")?.to_owned();
    assert_ne!(
        canonical, provider_etag,
        "the two validators are different strings"
    );
    let by_canonical = send(with(&canonical)).await?;
    assert_eq!(
        by_canonical.status, 206,
        "3: Palmr's own validator is honoured too"
    );
    assert_eq!(by_canonical.body, expected_bytes(SIZE, 100, 200)?);

    for (what, validator) in [
        ("4: stale strong", "\"0000000000\"".to_owned()),
        ("5: weak provider tag", format!("W/{provider_etag}")),
        ("5: weak canonical tag", format!("W/{canonical}")),
        ("6: invalid date", "yesterday".to_owned()),
        ("6: stale date", "Mon, 01 Jan 2001 00:00:00 GMT".to_owned()),
        ("6: empty", String::new()),
    ] {
        let reply = send(with(&validator)).await?;
        assert_eq!(
            reply.status, 200,
            "{what} must restart with the whole representation"
        );
        assert!(reply.header("location").is_none() && reply.header("content-range").is_none());
        assert_eq!(reply.require("content-length")?, SIZE.to_string(), "{what}");
        assert_eq!(reply.require("etag")?, canonical, "{what}");
        assert_eq!(reply.body.len() as u64, SIZE, "{what}");
        assert_eq!(reply.sha256(), digest, "{what}");
    }
    let by_date = send(with(&provider_modified)).await?;
    assert_eq!(
        by_date.status, 206,
        "6: the provider's Last-Modified is a valid date validator"
    );
    assert_eq!(by_date.body, expected_bytes(SIZE, 100, 200)?);

    for tag in [
        provider_etag.clone(),
        canonical.clone(),
        "*".to_owned(),
        format!("W/{provider_etag}"),
    ] {
        let reply = rig.conditional(&file, &[("if-none-match", &tag)]).await?;
        assert_eq!(reply.status, 304, "7: {tag}");
        assert!(reply.body.is_empty() && reply.header("location").is_none());
        assert_eq!(reply.require("etag")?, canonical);
        assert_eq!(reply.require("cache-control")?, "private, no-cache");
    }
    let miss = rig
        .conditional(&file, &[("if-none-match", "\"other\"")])
        .await?;
    assert_eq!(
        miss.status, 302,
        "7: a non-matching If-None-Match continues to the redirect"
    );
    let mixed = rig
        .conditional(
            &file,
            &[("if-none-match", "\"other\""), ("range", "bytes=0-9")],
        )
        .await?;
    assert_eq!(mixed.status, 302);

    let unsatisfiable = format!("bytes={SIZE}-");
    for headers in [
        vec![("range", unsatisfiable.as_str())],
        vec![
            ("range", unsatisfiable.as_str()),
            ("if-range", provider_etag.as_str()),
        ],
    ] {
        let reply = rig.conditional(&file, &headers).await?;
        assert_eq!(reply.status, 416, "8: {headers:?}");
        assert_eq!(reply.require("content-range")?, format!("bytes */{SIZE}"));
        assert_eq!(reply.error_code()?, "RANGE_NOT_SATISFIABLE");
        assert!(reply.header("location").is_none());
        assert_eq!(
            reply.envelope()?["error"]["requestId"],
            reply.require("x-request-id")?
        );
    }
    let stale_beats = rig
        .conditional(
            &file,
            &[("range", unsatisfiable.as_str()), ("if-range", "\"0000\"")],
        )
        .await?;
    assert_eq!(
        stale_beats.status, 200,
        "8: a stale validator restarts before the range is judged"
    );
    assert_eq!(stale_beats.sha256(), digest);

    let last_byte = rig
        .conditional(
            &file,
            &[
                ("range", &format!("bytes={last}-")),
                ("if-range", &provider_etag),
            ],
        )
        .await?;
    assert_eq!(last_byte.status, 206);
    assert_eq!(last_byte.body, expected_bytes(SIZE, last, SIZE)?);

    for headers in [
        vec![("range", "bytes=0-9"), ("if-range", provider_etag.as_str())],
        vec![("if-none-match", provider_etag.as_str())],
        vec![("range", "bytes=0-9"), ("if-range", canonical.as_str())],
        vec![
            ("range", "bytes=99999999999-"),
            ("if-range", provider_etag.as_str()),
        ],
    ] {
        let foreign = rig
            .fetch
            .get(&content_path(&file), Some(&rig.other), &headers)
            .await?;
        assert_eq!(foreign.status, 404, "9: {headers:?}");
        assert_eq!(foreign.error_code()?, "FILE_NOT_FOUND");
        for leaked in [
            "etag",
            "location",
            "last-modified",
            "content-range",
            "content-disposition",
        ] {
            assert!(foreign.header(leaked).is_none(), "{leaked}");
        }
    }

    for round in 0..140 {
        let start = format!("bytes={}-", 1_000_000 + round);
        let mut response = rig
            .fetch
            .open(
                &content_path(&file),
                Some(&rig.owner),
                &[
                    ("range", start.as_str()),
                    ("if-range", provider_etag.as_str()),
                ],
            )
            .await?;
        assert_eq!(response.status().as_u16(), 206);
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
        let validator = provider_etag.clone();
        tasks.push(tokio::spawn(async move {
            fetch
                .get(
                    &path,
                    Some(&creds),
                    &[("range", "bytes=0-99"), ("if-range", &validator)],
                )
                .await
                .map(|reply| reply.status)
        }));
    }
    for task in tasks {
        assert_eq!(
            task.await??,
            206,
            "10: abandoned conditional streams leaked no permit"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_s3_missing_object_redirect_boundary() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("vanishing.bin", 50_000).await?;
    let location = rig.location(&file, "").await?;
    let key = pair_key(&location, &rig.bucket)?;
    let before = fetch_url(&location, &[]).await?;
    let etag = before.require("etag")?.to_owned();
    rig.minio.remove_object(&rig.bucket, &key).await?;

    let redirected = rig.redirect(&file, "").await?;
    assert_eq!(
        redirected.status, 302,
        "Palmr does not probe storage before signing; it cannot know"
    );
    let provider_answer = fetch_url(&Url::parse(redirected.require("location")?)?, &[]).await?;
    assert_eq!(provider_answer.status, 404);
    assert!(
        provider_answer.text().contains("NoSuchKey"),
        "the provider's own error, not a Palmr envelope"
    );
    assert!(provider_answer.error_code().is_err());

    for headers in [
        vec![("if-none-match", "\"other\"")],
        vec![("range", "bytes=0-9"), ("if-range", etag.as_str())],
    ] {
        let seen = rig.conditional(&file, &headers).await?;
        assert_eq!(
            seen.status, 502,
            "{headers:?}: Palmr reads the object itself here"
        );
        assert_eq!(seen.error_code()?, "STORAGE_OBJECT_MISSING");
        assert_eq!(
            seen.envelope()?["error"]["requestId"],
            seen.require("x-request-id")?
        );
    }
    let foreign = rig
        .fetch
        .get(
            &content_path(&file),
            Some(&rig.other),
            &[("if-none-match", "\"x\"")],
        )
        .await?;
    assert_eq!(
        (foreign.status, foreign.error_code()?.as_str()),
        (404, "FILE_NOT_FOUND")
    );
    Ok(())
}

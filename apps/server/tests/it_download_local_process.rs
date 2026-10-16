pub mod support;

use std::fs::OpenOptions;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};

use anyhow::{ensure, Context, Result};
use percent_encoding::percent_decode_str;
use reqwest::Method;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use unicode_normalization::UnicodeNormalization;
use url::Url;
use uuid::Uuid;

use support::client::{Creds, Db, Http};
use support::download::{
    content_path, expected_bytes, raw_get, read_head, seed_local_file, sha256, upload_local,
    user_id, Fetch, Fetcher,
};
use support::tus::{generated_digest, TusWorld, MIB};
use support::ServerProcess;

const SIZE: u64 = 2 * MIB + 12_345;

struct Rig {
    _root: TempDir,
    _process: ServerProcess,
    owner: TusWorld,
    other: TusWorld,
    fetch: Fetcher,
    base: Url,
    owner_id: String,
    other_id: String,
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
    let other_id = user_id(&db, "bob").await?;
    Ok(Rig {
        owner: TusWorld::new(base.clone(), data_dir.clone(), admin_http, admin),
        other: TusWorld::new(base.clone(), data_dir, Http::new(base.clone())?, other),
        fetch: Fetcher::new(&base)?,
        base,
        owner_id,
        other_id,
        _root: root,
        _process: process,
    })
}

impl Rig {
    fn owner_creds(&self) -> &Creds {
        &self.owner.creds
    }

    fn other_creds(&self) -> &Creds {
        &self.other.creds
    }

    async fn get(&self, file: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        self.fetch
            .get(&content_path(file), Some(self.owner_creds()), headers)
            .await
    }

    async fn get_query(&self, file: &str, query: &str, headers: &[(&str, &str)]) -> Result<Fetch> {
        self.fetch
            .get(
                &format!("{}?{query}", content_path(file)),
                Some(self.owner_creds()),
                headers,
            )
            .await
    }

    async fn upload(&self, name: &str, size: u64) -> Result<String> {
        upload_local(&self.owner, name, size).await
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

    fn object_path(&self, key: &str) -> std::path::PathBuf {
        self.owner.data_dir.join("storage").join(key)
    }

    async fn stored_path(&self, file: &str) -> Result<std::path::PathBuf> {
        let key = self
            .owner
            .db
            .scalar_string(&format!(
                "SELECT o.object_key FROM files f JOIN storage_objects o ON o.id = f.storage_object_id
                  WHERE f.id = '{file}'"
            ))
            .await?;
        Ok(self.object_path(&key))
    }

    async fn raw_get(&self, file: &str, creds: &Creds, extra: &str) -> Result<TcpStream> {
        raw_get(&self.base, file, creds, extra).await
    }
}

struct Disposition {
    kind: String,
    ascii: String,
    decoded: String,
}

fn disposition(value: &str) -> Result<Disposition> {
    let (kind, rest) = value.split_once("; filename=\"").context("filename")?;
    let (ascii, rest) = rest
        .split_once("\"; filename*=UTF-8''")
        .context("filename*")?;
    ensure!(
        !value.contains(['\r', '\n', '\0']),
        "a control character reached the header"
    );
    ensure!(
        !rest.contains([';', '"', ',', ' ']),
        "unencoded filename*: {rest}"
    );
    Ok(Disposition {
        kind: kind.to_owned(),
        ascii: ascii.to_owned(),
        decoded: percent_decode_str(rest).decode_utf8()?.into_owned(),
    })
}

fn last_byte(path: &std::path::Path) -> Result<u8> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::End(-1))?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn ranged(first: u64, last: u64) -> String {
    format!("bytes={first}-{last}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_local_full_stream() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("full.bin", SIZE).await?;
    let reply = rig
        .get(&file, &[("accept-encoding", "gzip, br, zstd, deflate")])
        .await?;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.require("content-length")?, SIZE.to_string());
    assert_eq!(reply.body.len() as u64, SIZE);
    assert_eq!(reply.sha256(), generated_digest(SIZE)?);
    assert_eq!(reply.require("accept-ranges")?, "bytes");
    assert_eq!(reply.require("cache-control")?, "private, no-cache");
    assert_eq!(reply.require("x-content-type-options")?, "nosniff");
    assert_eq!(reply.require("content-type")?, "application/octet-stream");
    assert!(reply.header("content-encoding").is_none());
    assert!(reply.header("transfer-encoding").is_none());
    let etag = reply.require("etag")?;
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert!(reply.require("last-modified")?.ends_with(" GMT"));
    assert!(reply.header("x-request-id").is_some());
    assert!(reply
        .require("content-disposition")?
        .starts_with("attachment; filename=\"full.bin\""));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_local_range_exact() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("ranges.bin", SIZE).await?;
    let last = SIZE - 1;
    let cases: Vec<(String, u64, u64)> = vec![
        ("bytes=0-0".to_owned(), 0, 0),
        (ranged(1_000, 262_143 + 1_000), 1_000, 263_143),
        (format!("bytes={last}-{last}"), last, last),
        (format!("bytes={}-", SIZE - 10), SIZE - 10, last),
        (ranged(SIZE - 5, SIZE + 4_000_000), SIZE - 5, last),
        (format!("bytes=-{SIZE}"), 0, last),
        (format!("bytes=-{}", SIZE * 4), 0, last),
        ("bytes=-512".to_owned(), SIZE - 512, last),
        ("bytes=0-".to_owned(), 0, last),
        ("bytes=5-9,100-200".to_owned(), 5, 9),
        ("bytes=5-9,6-12".to_owned(), 5, 9),
        (format!("bytes={}-{}", SIZE - 2, u64::MAX), SIZE - 2, last),
    ];
    for (header, first, end) in cases {
        let reply = rig.get(&file, &[("range", &header)]).await?;
        assert_eq!(reply.status, 206, "{header}: {}", reply.text());
        let length = end - first + 1;
        assert_eq!(
            reply.require("content-length")?,
            length.to_string(),
            "{header}"
        );
        assert_eq!(
            reply.require("content-range")?,
            format!("bytes {first}-{end}/{SIZE}"),
            "{header}"
        );
        assert_eq!(reply.require("accept-ranges")?, "bytes");
        assert!(reply.header("content-encoding").is_none());
        assert_eq!(
            reply.body,
            expected_bytes(SIZE, first, end + 1)?,
            "{header}"
        );
    }

    for header in [
        format!("bytes={SIZE}-"),
        format!("bytes={}-", SIZE + 1),
        "bytes=99999999999-".to_owned(),
        "bytes=18446744073709551615-".to_owned(),
        "bytes=-0".to_owned(),
        format!("bytes={}-{},0-5", SIZE + 7, SIZE + 9),
    ] {
        let reply = rig.get(&file, &[("range", &header)]).await?;
        assert_eq!(reply.status, 416, "{header}");
        assert_eq!(reply.require("content-range")?, format!("bytes */{SIZE}"));
        assert_eq!(reply.error_code()?, "RANGE_NOT_SATISFIABLE");
        assert_eq!(
            reply.envelope()?["error"]["requestId"],
            reply.require("x-request-id")?,
            "the envelope correlates with the response"
        );
        assert!(reply.body.len() < 1_024, "no file data on a 416");
    }

    for header in [
        "bytes=5-2",
        "bytes=abc",
        "items=0-5",
        "bytes=",
        "bytes=1-2-3",
        "bytes=0-18446744073709551616",
    ] {
        let reply = rig.get(&file, &[("range", header)]).await?;
        assert_eq!(reply.status, 200, "{header} is ignored, not unsatisfiable");
        assert_eq!(reply.body.len() as u64, SIZE);
    }
    let twice = rig
        .get(&file, &[("range", "bytes=0-1"), ("range", "bytes=2-3")])
        .await?;
    assert_eq!(twice.status, 200, "repeated Range headers are ignored");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_local_if_range() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("conditional.bin", SIZE).await?;
    let full = rig.get(&file, &[]).await?;
    let etag = full.require("etag")?.to_owned();
    let modified = full.require("last-modified")?.to_owned();

    fn with(validator: &str) -> Vec<(&'static str, &str)> {
        vec![("range", "bytes=100-199"), ("if-range", validator)]
    }
    let matching = rig.get(&file, &with(&etag)).await?;
    assert_eq!(matching.status, 206);
    assert_eq!(matching.body, expected_bytes(SIZE, 100, 200)?);

    for stale in [
        "\"0000\"".to_owned(),
        format!("W/{etag}"),
        "garbage".to_owned(),
        String::new(),
        "Mon, 01 Jan 2001 00:00:00 GMT".to_owned(),
        modified.replace("GMT", "UTC"),
    ] {
        let reply = rig.get(&file, &with(&stale)).await?;
        assert_eq!(
            reply.status, 200,
            "If-Range {stale:?} must restart the download"
        );
        assert_eq!(reply.body.len() as u64, SIZE);
        assert!(reply.header("content-range").is_none());
    }

    let by_date = rig.get(&file, &with(&modified)).await?;
    assert_eq!(by_date.status, 206, "a matching Last-Modified is honoured");

    let unchanged = rig.get(&file, &[("if-none-match", etag.as_str())]).await?;
    assert_eq!(unchanged.status, 304);
    assert!(unchanged.body.is_empty());
    assert_eq!(unchanged.require("etag")?, etag);
    assert_eq!(unchanged.require("cache-control")?, "private, no-cache");
    for header in [
        format!("W/{etag}"),
        "*".to_owned(),
        format!("\"zzz\", {etag}"),
    ] {
        let reply = rig.get(&file, &[("if-none-match", &header)]).await?;
        assert_eq!(reply.status, 304, "{header}");
    }
    let changed = rig.get(&file, &[("if-none-match", "\"other\"")]).await?;
    assert_eq!(changed.status, 200);
    let beats_range = rig
        .get(&file, &[("if-none-match", &etag), ("range", "bytes=0-9")])
        .await?;
    assert_eq!(beats_range.status, 304);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_content_length_measured() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("measured.bin", SIZE).await?;
    let on_disk = rig.stored_path(&file).await?;
    let recorded = rig
        .owner
        .db
        .scalar_i64("SELECT size_bytes FROM storage_objects")
        .await?;
    assert_eq!(std::fs::metadata(&on_disk)?.len(), SIZE);
    assert_eq!(recorded as u64, SIZE);

    let reply = rig.get(&file, &[]).await?;
    assert_eq!(
        reply.require("content-length")?.parse::<u64>()?,
        std::fs::metadata(&on_disk)?.len()
    );
    assert_eq!(reply.body.len() as u64, SIZE);

    let tail = last_byte(&on_disk)?;
    {
        let mut file = OpenOptions::new().append(true).open(&on_disk)?;
        file.write_all(b"surplus")?;
    }
    let reply = rig.get(&file, &[]).await?;
    assert_eq!(reply.status, 500);
    assert_eq!(reply.error_code()?, "STORAGE_SIZE_MISMATCH");
    assert!(
        reply.body.len() < 1_024,
        "no file bytes after a size disagreement"
    );
    let ranged = rig.get(&file, &[("range", "bytes=0-9")]).await?;
    assert_eq!(ranged.error_code()?, "STORAGE_SIZE_MISMATCH");

    OpenOptions::new()
        .write(true)
        .open(&on_disk)?
        .set_len(SIZE - 1)?;
    let reply = rig.get(&file, &[]).await?;
    assert_eq!(reply.error_code()?, "STORAGE_SIZE_MISMATCH");

    {
        let mut file = OpenOptions::new().append(true).open(&on_disk)?;
        file.write_all(&[tail])?;
    }
    assert_eq!(std::fs::metadata(&on_disk)?.len(), SIZE);
    let healed = rig.get(&file, &[]).await?;
    assert_eq!(healed.status, 200);
    assert_eq!(healed.sha256(), generated_digest(SIZE)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_zero_byte_file() -> Result<()> {
    let rig = rig().await?;
    let seeded = rig.seed("empty.txt", "text/plain", b"").await?;
    let reply = rig.get(&seeded.file_id, &[]).await?;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.require("content-length")?, "0");
    assert!(reply.body.is_empty());
    assert_eq!(reply.require("accept-ranges")?, "bytes");
    assert_eq!(reply.require("content-type")?, "text/plain");

    for header in ["bytes=0-", "bytes=0-0", "bytes=0-99", "bytes=-0"] {
        let reply = rig.get(&seeded.file_id, &[("range", header)]).await?;
        assert_eq!(reply.status, 416, "{header}");
        assert_eq!(reply.require("content-range")?, "bytes */0");
        assert_eq!(reply.error_code()?, "RANGE_NOT_SATISFIABLE");
    }
    let suffix = rig.get(&seeded.file_id, &[("range", "bytes=-5")]).await?;
    assert_eq!(suffix.status, 200);
    assert!(suffix.body.is_empty());
    let unchanged = rig
        .get(
            &seeded.file_id,
            &[("if-none-match", reply.require("etag")?)],
        )
        .await?;
    assert_eq!(unchanged.status, 304);
    Ok(())
}

const AWKWARD_NAMES: [&str; 14] = [
    "Relatório Anual (2026).pdf",
    "日本語のファイル.txt",
    "🎉 party time.png",
    "مرحبا بالعالم.txt",
    "Ünïcödé Ñame.txt",
    "README",
    ".env",
    "say \"hello\".txt",
    "semi;colon,comma.txt",
    "back\\slash.txt",
    "100% done %41 %2F.txt",
    "x\r\nSet-Cookie: pwned=1\r\n.txt",
    "tab\tnull\u{0}bell\u{7}.txt",
    "trailing dot.",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_disposition_utf8() -> Result<()> {
    let rig = rig().await?;
    let mut pdf_id = String::new();
    for name in AWKWARD_NAMES {
        let mime = if name.ends_with(".pdf") {
            "application/pdf"
        } else {
            "application/octet-stream"
        };
        let seeded = rig.seed(name, mime, b"payload").await?;
        if name.ends_with(".pdf") {
            pdf_id = seeded.file_id.clone();
        }
        for (query, kind) in [("", "attachment"), ("disposition=attachment", "attachment")] {
            let reply = rig.get_query(&seeded.file_id, query, &[]).await?;
            assert_eq!(reply.status, 200, "{name:?}");
            assert!(
                reply.headers.get_all("content-disposition").iter().count() == 1,
                "{name:?}"
            );
            assert!(
                reply.header("set-cookie").is_none(),
                "{name:?} injected a header"
            );
            let parsed = disposition(reply.require("content-disposition")?)
                .with_context(|| format!("{name:?}"))?;
            assert_eq!(parsed.kind, kind);
            assert_eq!(parsed.decoded, name.nfc().collect::<String>(), "{name:?}");
            assert!(
                !parsed.ascii.is_empty() && parsed.ascii.len() <= 80,
                "{name:?}"
            );
            assert!(
                parsed
                    .ascii
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')),
                "{name:?} -> {}",
                parsed.ascii
            );
        }
    }

    let attachment = rig.get(&pdf_id, &[]).await?;
    let inline = rig.get_query(&pdf_id, "disposition=inline", &[]).await?;
    assert_eq!(
        attachment.require("content-disposition")?,
        "attachment; filename=\"Relatorio Anual _2026_.pdf\"; filename*=UTF-8''Relat%C3%B3rio%20Anual%20%282026%29.pdf"
    );
    assert_eq!(
        inline.require("content-disposition")?,
        "inline; filename=\"Relatorio Anual _2026_.pdf\"; filename*=UTF-8''Relat%C3%B3rio%20Anual%20%282026%29.pdf"
    );

    for bad in [
        "disposition=Inline",
        "disposition=",
        "disposition=x",
        "disposition=inline&disposition=inline",
    ] {
        let reply = rig.get_query(&pdf_id, bad, &[]).await?;
        assert_eq!(reply.status, 422, "{bad}");
        assert_eq!(reply.error_code()?, "VALIDATION_ERROR");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_mime_and_nosniff() -> Result<()> {
    let rig = rig().await?;
    let seeded = rig
        .seed("misleading.txt", "video/mp4", b"not really a video")
        .await?;
    let reply = rig.get(&seeded.file_id, &[]).await?;
    assert_eq!(reply.require("content-type")?, "video/mp4");
    assert_eq!(reply.require("x-content-type-options")?, "nosniff");

    let octet = rig
        .seed("blob.mp4", "application/octet-stream", b"x")
        .await?;
    let reply = rig.get(&octet.file_id, &[]).await?;
    assert_eq!(
        reply.require("content-type")?,
        "application/octet-stream",
        "the stored type is served, not a guess from the name"
    );

    rig.owner
        .db
        .execute(&format!(
            "UPDATE files SET mime_type = 'broken\u{1}type' WHERE id = '{}'",
            octet.file_id
        ))
        .await?;
    let reply = rig.get(&octet.file_id, &[]).await?;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.require("content-type")?, "application/octet-stream");
    assert_eq!(reply.require("x-content-type-options")?, "nosniff");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_unsafe_inline_refused() -> Result<()> {
    let rig = rig().await?;
    for (name, mime) in [
        ("drawing.svg", "image/svg+xml"),
        ("page.html", "text/html"),
        ("page.xhtml", "application/xhtml+xml"),
        ("data.xml", "text/xml"),
        ("data2.xml", "application/xml"),
        ("run.js", "application/javascript"),
        ("run2.js", "text/javascript"),
        ("style.css", "text/css"),
        ("blob.bin", "application/octet-stream"),
    ] {
        let seeded = rig.seed(name, mime, b"<svg onload=alert(1)>").await?;
        let reply = rig
            .get_query(&seeded.file_id, "disposition=inline", &[])
            .await?;
        assert_eq!(reply.status, 200, "{name}");
        assert!(
            reply
                .require("content-disposition")?
                .starts_with("attachment;"),
            "{name} ({mime}) must not render on the application origin: {}",
            reply.require("content-disposition")?
        );
        assert_eq!(reply.require("content-type")?, mime);
        assert_eq!(reply.require("x-content-type-options")?, "nosniff");
        assert!(reply.header("content-security-policy").is_some());
    }
    for (name, mime) in [
        ("photo.png", "image/png"),
        ("clip.mp4", "video/mp4"),
        ("song.mp3", "audio/mpeg"),
        ("paper.pdf", "application/pdf"),
        ("notes.txt", "text/plain"),
    ] {
        let seeded = rig.seed(name, mime, b"safe").await?;
        let reply = rig
            .get_query(&seeded.file_id, "disposition=inline", &[])
            .await?;
        assert!(
            reply.require("content-disposition")?.starts_with("inline;"),
            "{name}"
        );
        assert_eq!(reply.require("content-type")?, mime);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_storage_missing() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("vanishing.bin", SIZE).await?;
    let path = rig.stored_path(&file).await?;
    let aside = path.with_extension("aside");
    std::fs::rename(&path, &aside)?;
    for headers in [vec![], vec![("range", "bytes=0-9")]] {
        let reply = rig.get(&file, &headers).await?;
        assert_eq!(reply.status, 502);
        assert_eq!(reply.error_code()?, "STORAGE_OBJECT_MISSING");
        assert_eq!(
            reply.envelope()?["error"]["requestId"],
            reply.require("x-request-id")?
        );
        let text = reply.text();
        for leak in ["objects/", path.to_string_lossy().as_ref(), "No such file"] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
    }

    std::fs::rename(&aside, &path)?;
    assert_eq!(rig.get(&file, &[]).await?.status, 200);

    let gone = rig
        .owner
        .http
        .send(
            Method::DELETE,
            &format!("/api/v1/files/{file}"),
            Some(rig.owner_creds()),
            None,
        )
        .await?
        .status;
    assert!(gone.is_success(), "{gone}");
    let reply = rig.get(&file, &[]).await?;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.error_code()?, "FILE_NOT_FOUND");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_other_owner_404() -> Result<()> {
    let rig = rig().await?;
    let file = rig.upload("private.bin", SIZE).await?;
    let own = rig.get(&file, &[]).await?;
    assert_eq!(own.status, 200, "A: the owner is served");
    let etag = own.require("etag")?.to_owned();

    let unknown = Uuid::now_v7().to_string();
    let reference = rig
        .fetch
        .get(&content_path(&unknown), Some(rig.other_creds()), &[])
        .await?;
    assert_eq!(reference.status, 404);
    assert_eq!(reference.error_code()?, "FILE_NOT_FOUND");

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
            .get(&content_path(&file), Some(rig.other_creds()), &headers)
            .await?;
        assert_eq!(foreign.status, 404, "B/F/G {headers:?}");
        assert_eq!(foreign.error_code()?, "FILE_NOT_FOUND");
        let (mut left, mut right) = (foreign.envelope()?, reference.envelope()?);
        left["error"]["requestId"] = "".into();
        right["error"]["requestId"] = "".into();
        assert_eq!(
            left, right,
            "a foreign id is indistinguishable from an unknown one"
        );
        for leaked in [
            "etag",
            "last-modified",
            "content-disposition",
            "content-range",
            "accept-ranges",
            "location",
        ] {
            assert!(foreign.header(leaked).is_none(), "{leaked} leaked");
        }
        assert!(foreign
            .require("content-type")?
            .starts_with("application/json"));
        let text = foreign.text();
        for leaked in ["private.bin", "objects/", &etag] {
            assert!(!text.contains(leaked), "{leaked}");
        }
    }

    let anonymous = rig.fetch.get(&content_path(&file), None, &[]).await?;
    assert_eq!(anonymous.status, 401, "C");
    assert_eq!(anonymous.error_code()?, "AUTH_REQUIRED");

    let malformed = rig
        .fetch
        .get(&content_path("not-a-uuid"), Some(rig.owner_creds()), &[])
        .await?;
    assert_eq!(
        (malformed.status, malformed.error_code()?.as_str()),
        (404, "FILE_NOT_FOUND")
    );
    assert_eq!(reference.status, 404, "D");

    let bobs = upload_local(&rig.other, "bobs.bin", 4_096).await?;
    assert_eq!(
        rig.fetch
            .get(&content_path(&bobs), Some(rig.other_creds()), &[])
            .await?
            .status,
        200
    );
    assert_eq!(
        rig.get(&bobs, &[]).await?.status,
        404,
        "the owner of the other file is not Ada"
    );
    let _ = &rig.other_id;

    let gone = rig
        .owner
        .http
        .send(
            Method::DELETE,
            &format!("/api/v1/files/{file}"),
            Some(rig.owner_creds()),
            None,
        )
        .await?;
    assert!(gone.status.is_success());
    let deleted = rig.get(&file, &[]).await?;
    assert_eq!(deleted.status, 404, "E");
    assert_eq!(deleted.error_code()?, "FILE_NOT_FOUND");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_disconnect_stops_stream() -> Result<()> {
    let rig = rig().await?;
    let big = vec![0x5a_u8; 48 * MIB as usize];
    let seeded = rig
        .seed("big.bin", "application/octet-stream", &big)
        .await?;
    let before = rig
        .owner
        .db
        .scalar_string("SELECT (SELECT COUNT(*) FROM files) || ':' || (SELECT state FROM storage_objects) || ':' || (SELECT SUM(used_bytes) FROM users)")
        .await?;

    for headers in [vec![], vec![("range", "bytes=1000000-40000000")]] {
        let mut response = rig
            .fetch
            .open(
                &content_path(&seeded.file_id),
                Some(rig.owner_creds()),
                &headers,
            )
            .await?;
        let mut read = 0_usize;
        while read < 512 * 1024 {
            read += response.chunk().await?.context("an early end")?.len();
        }
        drop(response);
    }

    let again = rig.get(&seeded.file_id, &[("range", "bytes=0-99")]).await?;
    assert_eq!(again.status, 206);
    let after = rig
        .owner
        .db
        .scalar_string("SELECT (SELECT COUNT(*) FROM files) || ':' || (SELECT state FROM storage_objects) || ':' || (SELECT SUM(used_bytes) FROM users)")
        .await?;
    assert_eq!(before, after, "an abandoned download changes nothing");
    assert_eq!(std::fs::metadata(&seeded.path)?.len(), big.len() as u64);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_download_concurrent_permit_release() -> Result<()> {
    let rig = rig().await?;
    let big = vec![0x33_u8; 40 * MIB as usize];
    let seeded = rig
        .seed("slots.bin", "application/octet-stream", &big)
        .await?;
    let small = rig
        .seed("small.bin", "application/octet-stream", b"tiny")
        .await?;

    let mut stalled = Vec::new();
    for _ in 0..128 {
        let mut stream = rig.raw_get(&seeded.file_id, rig.owner_creds(), "").await?;
        let (head, _) = read_head(&mut stream).await?;
        assert_eq!(head.status, 200);
        stalled.push(stream);
    }

    let started = std::time::Instant::now();
    let busy = rig.get(&small.file_id, &[]).await?;
    assert_eq!(
        busy.status, 503,
        "the 129th download waits, then is refused"
    );
    assert_eq!(busy.error_code()?, "TRANSFER_BACKPRESSURE");
    assert_eq!(busy.require("retry-after")?, "5");
    assert!(started.elapsed() >= std::time::Duration::from_secs(4));

    drop(stalled);
    let mut served = false;
    for _ in 0..200 {
        if rig.get(&small.file_id, &[]).await?.status == 200 {
            served = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(served, "closing the connections returns every permit");

    let mut tasks = Vec::new();
    for _ in 0..128 {
        let fetch = rig.fetch.clone();
        let creds = rig.owner_creds().clone();
        let path = content_path(&small.file_id);
        tasks.push(tokio::spawn(async move {
            fetch
                .get(&path, Some(&creds), &[])
                .await
                .map(|reply| reply.status)
        }));
    }
    for task in tasks {
        assert_eq!(task.await??, 200, "no permit leaked");
    }
    Ok(())
}

#[allow(non_snake_case, reason = "the accepted regression identifier is R-020")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_R020_proxied_range_and_content_disposition() -> Result<()> {
    let rig = rig().await?;
    let name = "Relatório Anual (2026).pdf";
    let size = 3 * MIB + 77;
    let file = rig.upload(name, size).await?;
    let mime = rig
        .owner
        .db
        .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
        .await?;
    assert_eq!(mime, "application/pdf");
    let digest = generated_digest(size)?;
    let proxy = [
        ("x-forwarded-for", "203.0.113.9"),
        ("x-forwarded-proto", "https"),
        ("x-forwarded-host", "files.example.test"),
    ];

    let full = rig.get(&file, &proxy).await?;
    assert_eq!(full.status, 200);
    assert_eq!(full.require("accept-ranges")?, "bytes");
    assert_eq!(full.require("content-type")?, "application/pdf");
    assert_eq!(full.sha256(), digest);
    let parsed = disposition(full.require("content-disposition")?)?;
    assert_eq!(parsed.kind, "attachment");
    assert_eq!(parsed.ascii, "Relatorio Anual _2026_.pdf");
    assert_eq!(parsed.decoded, name);

    let expectations: [(&str, u16, u64, u64); 4] = [
        ("bytes=0-1023", 206, 0, 1023),
        ("bytes=1024-", 206, 1024, size - 1),
        ("bytes=-512", 206, size - 512, size - 1),
        ("bytes=99999999999-", 416, 0, 0),
    ];
    for (header, status, first, last) in expectations {
        let reply = rig
            .get(&file, &[("range", header), proxy[0], proxy[1], proxy[2]])
            .await?;
        assert_eq!(reply.status, status, "{header}");
        if status == 206 {
            assert_eq!(
                reply.require("content-length")?,
                (last - first + 1).to_string()
            );
            assert_eq!(
                reply.require("content-range")?,
                format!("bytes {first}-{last}/{size}")
            );
            assert_eq!(reply.body, expected_bytes(size, first, last + 1)?);
            assert_eq!(
                disposition(reply.require("content-disposition")?)?.decoded,
                name
            );
        } else {
            assert_eq!(reply.require("content-range")?, format!("bytes */{size}"));
            assert_eq!(reply.error_code()?, "RANGE_NOT_SATISFIABLE");
        }
    }

    let etag = full.require("etag")?.to_owned();
    let mut response = rig
        .fetch
        .open(&content_path(&file), Some(rig.owner_creds()), &[])
        .await?;
    let mut received = Vec::new();
    while received.len() < 900_000 {
        received.extend_from_slice(&response.chunk().await?.context("an early end")?);
    }
    drop(response);
    let resumed = rig
        .get(
            &file,
            &[
                ("range", &format!("bytes={}-", received.len())),
                ("if-range", &etag),
            ],
        )
        .await?;
    assert_eq!(resumed.status, 206);
    received.extend_from_slice(&resumed.body);
    assert_eq!(received.len() as u64, size);
    assert_eq!(
        sha256(&received),
        digest,
        "interrupt and resume reproduce the file"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_357_firefox_large_download_content_length() -> Result<()> {
    let rig = rig().await?;
    let size = 24 * MIB + 3;
    let file = rig.upload("large.bin", size).await?;
    let stored = std::fs::metadata(rig.stored_path(&file).await?)?.len();
    let recorded = rig
        .owner
        .db
        .scalar_i64("SELECT size_bytes FROM storage_objects")
        .await? as u64;
    assert_eq!((stored, recorded), (size, size));

    for (extra, expected) in [
        (String::new(), size),
        ("Range: bytes=7-\r\n".to_owned(), size - 7),
        (format!("Range: bytes=-{}\r\n", MIB + 1), MIB + 1),
        ("Range: bytes=100-4000100\r\n".to_owned(), 4_000_001),
        ("Accept-Encoding: gzip, br\r\n".to_owned(), size),
    ] {
        let mut stream = rig.raw_get(&file, rig.owner_creds(), &extra).await?;
        let (head, _) = read_head(&mut stream).await?;
        assert!(
            matches!(head.status, 200 | 206),
            "{extra:?}: {}",
            head.status
        );
        assert!(head.header("transfer-encoding").is_none(), "{extra:?}");
        assert!(head.header("content-encoding").is_none(), "{extra:?}");
        let declared: u64 = head
            .header("content-length")
            .context("Content-Length on the wire")?
            .parse()?;
        assert_eq!(declared, expected, "{extra:?}");
        let mut received = 0_u64;
        let mut buffer = vec![0_u8; 256 * 1024];
        while received < declared {
            let read = stream.read(&mut buffer).await?;
            ensure!(
                read > 0,
                "the connection closed after {received} of {declared}"
            );
            received += read as u64;
        }
        assert_eq!(received, declared, "bytes received equal Content-Length");
        stream.set_nodelay(true)?;
        let mut extra_byte = [0_u8; 1];
        let more = tokio::time::timeout(
            std::time::Duration::from_millis(150),
            stream.read(&mut extra_byte),
        )
        .await;
        assert!(more.is_err(), "no byte follows the declared body");
    }

    let whole = rig.get(&file, &[]).await?;
    assert_eq!(whole.require("content-length")?.parse::<u64>()?, stored);
    assert_eq!(whole.body.len() as u64, stored);
    assert_eq!(whole.sha256(), generated_digest(size)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regression_358_media_preview_mime_behind_proxy() -> Result<()> {
    let rig = rig().await?;
    let cases = [
        ("clip.mp4", "video/mp4", true),
        ("clip.webm", "video/webm", true),
        ("song.mp3", "audio/mpeg", true),
        ("song.ogg", "audio/ogg", true),
        ("paper.pdf", "application/pdf", true),
        ("photo.png", "image/png", true),
        ("drawing.svg", "image/svg+xml", false),
        ("notes.txt", "text/plain", true),
    ];
    let behind_proxy = [
        ("x-forwarded-for", "198.51.100.4"),
        ("x-forwarded-proto", "https"),
        ("x-forwarded-host", "palmr.example.test"),
        ("via", "1.1 nginx"),
    ];
    for (name, mime, inline_safe) in cases {
        let file = rig.upload(name, 20_000).await?;
        let stored = rig
            .owner
            .db
            .scalar_string(&format!("SELECT mime_type FROM files WHERE id = '{file}'"))
            .await?;
        assert_eq!(
            stored, mime,
            "{name}: the server detects the type at upload"
        );
        for query in ["disposition=inline", ""] {
            let reply = rig.get_query(&file, query, &behind_proxy).await?;
            assert_eq!(reply.status, 200, "{name}");
            assert_eq!(reply.require("content-type")?, mime, "{name} {query}");
            assert_ne!(reply.require("content-type")?, "application/octet-stream");
            assert_eq!(reply.require("x-content-type-options")?, "nosniff");
            let kind = disposition(reply.require("content-disposition")?)?.kind;
            let expected = if query.is_empty() || !inline_safe {
                "attachment"
            } else {
                "inline"
            };
            assert_eq!(kind, expected, "{name} {query}");
            let ranged = rig
                .get_query(&file, query, &[("range", "bytes=0-99"), behind_proxy[0]])
                .await?;
            assert_eq!(ranged.status, 206);
            assert_eq!(ranged.require("content-type")?, mime);
        }
    }
    Ok(())
}

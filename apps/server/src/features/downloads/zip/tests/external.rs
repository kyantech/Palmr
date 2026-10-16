use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;
use unicode_normalization::UnicodeNormalization;

use super::oracle::{crc32_reference, parse_archive};
use super::{build, dir, emit_all, file, sha256_hex, CountingSink, Fixture};
use crate::features::downloads::zip::{archive_size, EntryKind, EntryShape, ZipEntry};

const PYTHON_READER: &str = r#"
import hashlib, json, sys, zipfile

archive = zipfile.ZipFile(sys.argv[1])
bad = archive.testzip()
entries = []
for info in archive.infolist():
    data = b"" if info.is_dir() else archive.read(info)
    entries.append({
        "name": info.filename,
        "is_dir": info.is_dir(),
        "flag_bits": info.flag_bits,
        "compress_type": info.compress_type,
        "file_size": info.file_size,
        "compress_size": info.compress_size,
        "crc": info.CRC,
        "header_offset": info.header_offset,
        "create_version": info.create_version,
        "extract_version": info.extract_version,
        "sha256": hashlib.sha256(data).hexdigest(),
    })
json.dump({"bad": bad, "count": len(entries), "entries": entries}, sys.stdout)
"#;

fn find_tool(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("required independent ZIP reader `{name}` is not on PATH"))
}

fn try_tool(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn utf8_locale() -> &'static str {
    if cfg!(target_os = "macos") {
        "en_US.UTF-8"
    } else {
        "C.UTF-8"
    }
}

fn unzip(args: &[&str], archive: &Path) -> std::process::Output {
    Command::new(find_tool("unzip"))
        .env("LC_ALL", utf8_locale())
        .args(args)
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn python_report(tools: &Path, archive: &Path) -> Value {
    let script = tools.join("read_zip.py");
    fs::write(&script, PYTHON_READER).unwrap();
    let output = Command::new(find_tool("python3"))
        .arg("-I")
        .arg(&script)
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "python zipfile rejected the archive: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn sha256_of_file(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut reader = fs::File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn walk(root: &Path) -> BTreeMap<String, Option<String>> {
    fn visit(root: &Path, at: &Path, out: &mut BTreeMap<String, Option<String>>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            if path.is_dir() {
                out.insert(format!("{relative}/"), None);
                visit(root, &path, out);
            } else {
                out.insert(relative, Some(sha256_of_file(&path)));
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

fn showcase() -> Vec<Fixture> {
    vec![
        dir(None, "Relatórios"),
        file(Some(0), "Relatório.pdf", &pseudo_random(10_000, 1)),
        file(Some(0), "年度報告書.pdf", &pseudo_random(777, 2)),
        file(None, "تقرير.pdf", &pseudo_random(4_096, 3)),
        file(None, "📦 backup.zip", &pseudo_random(65_537, 4)),
        file(None, "Relato\u{301}rio.pdf", &pseudo_random(31, 5)),
        dir(None, "vazio"),
        file(None, "vazio.txt", b""),
        file(None, "report.pdf", &pseudo_random(100, 6)),
        file(None, "report.pdf", &pseudo_random(101, 7)),
        file(None, "report.pdf", &pseudo_random(102, 8)),
        dir(None, "a"),
        dir(Some(11), "b"),
        dir(Some(12), "c"),
        file(Some(13), "deep.bin", &pseudo_random(300_000, 9)),
        file(None, "../../etc/passwd", b"not a traversal"),
        file(None, "C:\\windows\\win.ini", b"not a drive"),
        file(None, "quote\"semi;comma,.txt", b"punctuation"),
    ]
}

fn expected_tree(fixtures: &[Fixture], paths: &[String]) -> BTreeMap<String, Option<String>> {
    fixtures
        .iter()
        .zip(paths)
        .map(|(fixture, path)| match fixture.kind {
            EntryKind::File => (path.clone(), Some(sha256_hex(&fixture.payload))),
            EntryKind::Directory => (format!("{path}/"), None),
        })
        .collect()
}

#[test]
fn it_zip_validates_with_real_unzip() {
    let fixtures = showcase();
    let built = build(&fixtures, 4093);
    assert_eq!(built.planned.total_len, built.counted);

    let scratch = TempDir::new().unwrap();
    let tools = TempDir::new().unwrap();
    let archive = scratch.path().join("palmr.zip");
    fs::write(&archive, &built.bytes).unwrap();
    assert_eq!(
        fs::metadata(&archive).unwrap().len(),
        built.planned.total_len
    );

    let tested = unzip(&["-t"], &archive);
    let report = String::from_utf8_lossy(&tested.stdout);
    assert!(tested.status.success(), "unzip -t failed: {report}");
    assert!(report.contains("No errors detected in compressed data"));
    assert_eq!(report.matches("    testing: ").count(), fixtures.len());
    assert!(!report.contains("bad CRC") && !report.contains("warning"));

    let extracted = scratch.path().join("extracted");
    fs::create_dir(&extracted).unwrap();
    let output = Command::new(find_tool("unzip"))
        .env("LC_ALL", utf8_locale())
        .args(["-q", "-d"])
        .arg(&extracted)
        .arg(&archive)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "unzip extraction failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let on_disk = walk(&extracted);
    let expected = expected_tree(&fixtures, &built.paths);
    assert_eq!(
        on_disk, expected,
        "extracted tree, names and SHA-256 digests"
    );
    for name in on_disk.keys() {
        assert!(std::str::from_utf8(name.as_bytes()).is_ok());
        assert!(!name.split('/').any(|segment| segment == ".."), "{name}");
    }
    assert!(on_disk.contains_key("Relatórios/年度報告書.pdf"));
    assert!(on_disk.contains_key("تقرير.pdf"));
    assert!(on_disk.contains_key("📦 backup.zip"));
    assert!(on_disk.contains_key("vazio/"));
    assert!(on_disk.contains_key("report (2).pdf"));
    assert!(on_disk.contains_key("a/b/c/deep.bin"));
    assert_eq!(
        fs::read_dir(scratch.path().join("extracted"))
            .unwrap()
            .count(),
        on_disk
            .keys()
            .filter(|key| !key.trim_end_matches('/').contains('/'))
            .count()
    );

    let python = python_report(tools.path(), &archive);
    assert_eq!(python["bad"], Value::Null);
    assert_eq!(python["count"].as_u64().unwrap(), fixtures.len() as u64);
    let parsed = parse_archive(&built.bytes).unwrap();
    for (index, entry) in python["entries"].as_array().unwrap().iter().enumerate() {
        let fixture = &fixtures[index];
        let name = if fixture.kind == EntryKind::Directory {
            format!("{}/", built.paths[index])
        } else {
            built.paths[index].clone()
        };
        assert_eq!(entry["name"].as_str().unwrap(), name);
        assert_eq!(
            entry["is_dir"].as_bool().unwrap(),
            fixture.kind == EntryKind::Directory
        );
        assert_eq!(
            entry["flag_bits"].as_u64().unwrap() & 0x0800,
            0x0800,
            "{name}: EFS"
        );
        assert_eq!(entry["compress_type"].as_u64().unwrap(), 0, "{name}: STORE");
        assert_eq!(
            entry["file_size"].as_u64().unwrap(),
            fixture.payload.len() as u64
        );
        assert_eq!(entry["compress_size"], entry["file_size"]);
        assert_eq!(entry["extract_version"].as_u64().unwrap(), 45);
        assert_eq!(entry["create_version"].as_u64().unwrap(), 45);
        assert_eq!(
            entry["crc"].as_u64().unwrap(),
            u64::from(crc32_reference(&fixture.payload))
        );
        assert_eq!(
            entry["header_offset"].as_u64().unwrap(),
            parsed.tail.entries[index].local_offset
        );
        assert_eq!(
            entry["sha256"].as_str().unwrap(),
            sha256_hex(&fixture.payload),
            "{name}"
        );
    }

    let zipinfo = Command::new(find_tool("zipinfo"))
        .arg("-v")
        .arg(&archive)
        .output()
        .unwrap();
    let verbose = String::from_utf8_lossy(&zipinfo.stdout);
    assert!(zipinfo.status.success());
    assert_eq!(
        verbose.matches("PKWARE 64-bit sizes").count(),
        fixtures.len(),
        "one ZIP64 extra per central directory entry"
    );
    assert!(verbose.contains("compression method:                             none (stored)"));
    assert!(!verbose.contains("deflated"));
    assert!(verbose.contains("minimum software version required to extract:   4.5"));
    assert_eq!(
        verbose
            .matches("extended local header:                          yes")
            .count(),
        fixtures
            .iter()
            .filter(|f| f.kind == EntryKind::File)
            .count()
    );
}

#[test]
fn it_zip_streams_through_a_local_header_only_reader() {
    let Some(bsdtar) = try_tool("bsdtar") else {
        eprintln!("bsdtar is not installed; the stdin streaming reader check is skipped");
        return;
    };
    let fixtures = showcase();
    let built = build(&fixtures, 1000);
    let scratch = TempDir::new().unwrap();
    let extracted = scratch.path().join("out");
    fs::create_dir(&extracted).unwrap();
    let mut child = Command::new(bsdtar)
        .env("LC_ALL", utf8_locale())
        .arg("-xf")
        .arg("-")
        .arg("-C")
        .arg(&extracted)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let bytes = built.bytes.clone();
    let feeder = std::thread::spawn(move || {
        for part in bytes.chunks(1500) {
            if stdin.write_all(part).is_err() {
                break;
            }
        }
    });
    let output = child.wait_with_output().unwrap();
    feeder.join().unwrap();
    assert!(
        output.status.success(),
        "bsdtar streaming read failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recovered: BTreeMap<String, Option<String>> = walk(&extracted)
        .into_iter()
        .map(|(name, digest)| (name.nfc().collect(), digest))
        .collect();
    assert_eq!(
        recovered,
        expected_tree(&fixtures, &built.paths),
        "a reader that never sees the central directory recovers every entry"
    );
}

#[test]
#[allow(non_snake_case)]
fn regression_R023_real_unzip_lists_every_colliding_entry() {
    let mut fixtures = vec![dir(None, "one"), dir(None, "two")];
    for parent in [0, 1] {
        fixtures.push(file(
            Some(parent),
            "report.pdf",
            format!("r{parent}").as_bytes(),
        ));
    }
    for i in 0..3 {
        fixtures.push(file(
            None,
            "received.bin",
            format!("received {i}").as_bytes(),
        ));
    }
    for name in ["../../etc/passwd", "/abs/path", "C:\\x", "d\\f", "nul\0x"] {
        fixtures.push(file(None, name, name.as_bytes()));
    }
    let built = build(&fixtures, 3);
    let scratch = TempDir::new().unwrap();
    let archive = scratch.path().join("r023.zip");
    fs::write(&archive, &built.bytes).unwrap();

    let listing = Command::new(find_tool("unzip"))
        .env("LC_ALL", "C")
        .arg("-Z1")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(listing.status.success());
    let text = String::from_utf8_lossy(&listing.stdout).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), fixtures.len());
    let file_lines = lines.iter().filter(|line| !line.ends_with('/')).count();
    assert_eq!(
        file_lines,
        fixtures
            .iter()
            .filter(|f| f.kind == EntryKind::File)
            .count()
    );
    let unique: std::collections::BTreeSet<&&str> = lines.iter().collect();
    assert_eq!(unique.len(), lines.len(), "unzip lists distinct paths");
    for line in &lines {
        assert!(!line.starts_with('/') && !line.contains('\\'), "{line}");
        assert!(!line.split('/').any(|segment| segment == ".."), "{line}");
    }
}

fn many_entries(count: usize) -> (Vec<u8>, u64) {
    let entries: Vec<ZipEntry> = (0..count)
        .map(|i| ZipEntry::file(&format!("d{}/f{i}", i % 11), 0).unwrap())
        .collect();
    let payloads: Vec<&[u8]> = vec![&[]; count];
    let mut sink = CountingSink::new();
    emit_all(&entries, &payloads, 1, 1 << 16, &mut sink);
    let planned = archive_size(entries.iter().map(EntryShape::from)).unwrap();
    assert_eq!(planned.total_len, sink.counted);
    (sink.bytes, planned.total_len)
}

#[test]
fn it_zip_entry_counts_above_sixteen_bits_are_read_by_independent_readers() {
    let scratch = TempDir::new().unwrap();
    let tools = TempDir::new().unwrap();
    for count in [65_535usize, 65_536, 70_000] {
        let (bytes, total) = many_entries(count);
        let archive = scratch.path().join(format!("{count}.zip"));
        fs::write(&archive, &bytes).unwrap();
        assert_eq!(fs::metadata(&archive).unwrap().len(), total);

        let python = python_report(tools.path(), &archive);
        assert_eq!(python["bad"], Value::Null, "{count}");
        assert_eq!(python["count"].as_u64().unwrap(), count as u64);

        let totals = Command::new(find_tool("zipinfo"))
            .arg("-t")
            .arg(&archive)
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&totals.stdout).into_owned();
        assert!(totals.status.success(), "{count}: {line}");
        assert!(
            line.contains(&format!("{count} files, 0 bytes uncompressed")),
            "{count}: {line}"
        );
        let tested = unzip(&["-tq"], &archive);
        assert!(tested.status.success(), "{count}");
    }
}

#[test]
fn it_zip_an_empty_archive_is_read_by_an_independent_reader() {
    let (bytes, total) = many_entries(0);
    assert_eq!(total, 98);
    let scratch = TempDir::new().unwrap();
    let tools = TempDir::new().unwrap();
    let archive = scratch.path().join("empty.zip");
    fs::write(&archive, &bytes).unwrap();
    let python = python_report(tools.path(), &archive);
    assert_eq!(python["count"].as_u64().unwrap(), 0);
    assert_eq!(python["bad"], Value::Null);
    if let Some(bsdtar) = try_tool("bsdtar") {
        let listing = Command::new(bsdtar)
            .arg("-tf")
            .arg(&archive)
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert!(listing.stdout.is_empty());
    }
}

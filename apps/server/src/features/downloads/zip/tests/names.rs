use std::collections::BTreeSet;
use std::fmt::Write as _;

use proptest::prelude::*;
use rstest::rstest;
use unicode_normalization::UnicodeNormalization;

use super::oracle::parse_archive;
use super::{build, dir, file, sha256_hex, zip_entries, Fixture};
use crate::features::downloads::zip::names::{
    MAX_ENTRY_DEPTH, MAX_ENTRY_PATH_BYTES, MAX_SEGMENT_BYTES,
};
use crate::features::downloads::zip::{
    resolve_names, sanitize_segment, validate_entry_path, ArchiveNamer, EntryKind, NameError,
    PathViolation,
};

fn independently_safe(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') || path.contains('\\') {
        return false;
    }
    if path.len() > 65_000 || path.contains("//") {
        return false;
    }
    path.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.contains(':')
            && !segment.chars().any(|c| c.is_control())
            && segment.len() <= 255
            && !segment.nfkc().any(|c| matches!(c, '/' | '\\' | ':'))
    })
}

fn collision_key(segment: &str) -> String {
    segment.nfkc().collect::<String>().to_lowercase()
}

fn assert_unique_per_directory(paths: &[String]) {
    let mut seen = BTreeSet::new();
    for path in paths {
        let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", path));
        assert!(
            seen.insert((collision_key(parent), collision_key(leaf))),
            "colliding entry {path:?} in {paths:#?}"
        );
    }
}

fn names_of(requests: &[(&str, EntryKind)]) -> Vec<String> {
    let requests: Vec<_> = requests
        .iter()
        .map(|(name, kind)| (None, *name, *kind))
        .collect();
    resolve_names(&requests)
        .unwrap()
        .into_iter()
        .map(|resolved| resolved.path)
        .collect()
}

fn files(names: &[&str]) -> Vec<String> {
    let requests: Vec<_> = names.iter().map(|n| (*n, EntryKind::File)).collect();
    names_of(&requests)
}

#[rstest]
#[case("../secret.txt", ".._secret.txt")]
#[case("../../etc/passwd", ".._.._etc_passwd")]
#[case("/absolute/file.txt", "_absolute_file.txt")]
#[case("C:\\windows\\file.txt", "C__windows_file.txt")]
#[case("C:/windows/file.txt", "C__windows_file.txt")]
#[case("\\\\server\\share\\file.txt", "__server_share_file.txt")]
#[case("./report.txt", "._report.txt")]
#[case("folder/../report.txt", "folder_.._report.txt")]
#[case("folder//report.txt", "folder__report.txt")]
#[case("folder\\report.txt", "folder_report.txt")]
#[case(".", "_")]
#[case("..", "_")]
#[case("...", "_")]
#[case("", "_")]
#[case("   ", "_")]
#[case("\u{a0}\u{3000}", "_")]
#[case("report.pdf.", "report.pdf")]
#[case("report.pdf. . ", "report.pdf")]
#[case("  padded name  ", "padded name")]
#[case(".hidden", ".hidden")]
#[case("trailing.dot.", "trailing.dot")]
#[case("archive.tar.gz", "archive.tar.gz")]
#[case("noextension", "noextension")]
#[case("a\0b", "a_b")]
#[case("a\r\nb", "a__b")]
#[case("a\tb", "a_b")]
#[case("a\u{7f}b", "a_b")]
#[case("a\u{85}b", "a_b")]
#[case("a\u{2028}b\u{2029}c", "a_b_c")]
#[case("\u{202e}gpj.exe", "_gpj.exe")]
#[case("a\u{2066}b\u{2069}", "a_b_")]
#[case("zero\u{200b}width\u{feff}", "zero_width_")]
#[case("fullwidth\u{ff0f}slash", "fullwidth_slash")]
#[case("fullwidth\u{ff3c}backslash", "fullwidth_backslash")]
#[case("fullwidth\u{ff1a}colon", "fullwidth_colon")]
#[case("division\u{2215}slash", "division\u{2215}slash")]
#[case("quote\"semi;comma,.txt", "quote\"semi;comma,.txt")]
#[case("Relat\u{f3}rio.pdf", "Relat\u{f3}rio.pdf")]
#[case("Relato\u{301}rio.pdf", "Relat\u{f3}rio.pdf")]
#[case("年度報告書.pdf", "年度報告書.pdf")]
#[case("تقرير.pdf", "تقرير.pdf")]
#[case("📦 backup.zip", "📦 backup.zip")]
#[case("e\u{301}\u{302}", "\u{e9}\u{302}")]
fn unit_zip_sanitize_segment_table(#[case] input: &str, #[case] expected: &str) {
    let cleaned = sanitize_segment(input);
    assert_eq!(cleaned, expected, "{input:?}");
    assert!(independently_safe(&cleaned), "{cleaned:?}");
    assert_eq!(validate_entry_path(&cleaned), Ok(()));
    assert_eq!(sanitize_segment(&cleaned), cleaned, "idempotent");
}

#[test]
fn unit_zip_names_follow_the_keep_both_convention() {
    assert_eq!(
        files(&["report.pdf", "report.pdf", "report.pdf"]),
        ["report.pdf", "report (1).pdf", "report (2).pdf"]
    );
    assert_eq!(
        files(&["report.pdf", "report (1).pdf", "report.pdf"]),
        ["report.pdf", "report (1).pdf", "report (2).pdf"]
    );
    assert_eq!(
        files(&["report (1).pdf", "report.pdf", "report.pdf"]),
        ["report (1).pdf", "report.pdf", "report (2).pdf"]
    );
    assert_eq!(
        files(&["archive.tar.gz", "archive.tar.gz"]),
        ["archive.tar.gz", "archive.tar (1).gz"]
    );
    assert_eq!(
        files(&["README", "README", ".env", ".env"]),
        ["README", "README (1)", ".env", ".env (1)"]
    );
    assert_eq!(
        files(&["photo (1).jpg", "photo (1).jpg"]),
        ["photo (1).jpg", "photo (1) (1).jpg"]
    );
}

#[test]
fn unit_zip_names_collide_case_and_normalization_insensitively() {
    assert_eq!(
        files(&["Report.pdf", "report.PDF", "REPORT.pdf"]),
        ["Report.pdf", "report (1).PDF", "REPORT (2).pdf"]
    );
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    assert_eq!(files(&[nfc, nfd]), ["caf\u{e9}.txt", "caf\u{e9} (1).txt"]);
    assert_eq!(
        files(&["\u{fb01}le.txt", "file.txt"]),
        ["\u{fb01}le.txt", "file (1).txt"]
    );
}

#[test]
fn unit_zip_names_collide_after_sanitization() {
    assert_eq!(
        files(&["a/b", "a_b", "a\\b", "a:b"]),
        ["a_b", "a_b (1)", "a_b (2)", "a_b (3)"]
    );
    assert_eq!(
        files(&["report.pdf", "report.pdf.", "report.pdf "]),
        ["report.pdf", "report (1).pdf", "report (2).pdf"]
    );
    assert_eq!(
        files(&["", ".", "..", "_"]),
        ["_", "_ (1)", "_ (2)", "_ (3)"]
    );
}

#[test]
fn unit_zip_names_share_one_namespace_for_files_and_directories() {
    let paths = names_of(&[
        ("docs", EntryKind::File),
        ("docs", EntryKind::Directory),
        ("Docs", EntryKind::Directory),
        ("docs.d", EntryKind::Directory),
        ("docs.d", EntryKind::File),
    ]);
    assert_eq!(
        paths,
        ["docs", "docs (1)", "Docs (2)", "docs.d", "docs (1).d"]
    );
    assert_unique_per_directory(&paths);
}

#[test]
fn unit_zip_directory_names_never_split_on_a_dot() {
    let paths = names_of(&[
        ("release.v1.2", EntryKind::Directory),
        ("release.v1.2", EntryKind::Directory),
    ]);
    assert_eq!(paths, ["release.v1.2", "release.v1.2 (1)"]);
}

#[test]
fn unit_zip_renamed_parents_carry_their_children() {
    let mut namer = ArchiveNamer::new();
    let first = namer.allocate(None, "docs", EntryKind::Directory).unwrap();
    let second = namer.allocate(None, "docs", EntryKind::Directory).unwrap();
    let in_first = namer
        .allocate(Some(first.id), "report.pdf", EntryKind::File)
        .unwrap();
    let in_second = namer
        .allocate(Some(second.id), "report.pdf", EntryKind::File)
        .unwrap();
    let again = namer
        .allocate(Some(second.id), "report.pdf", EntryKind::File)
        .unwrap();
    assert_eq!(first.id.index(), 0);
    assert_eq!(second.id.index(), 1);
    assert_eq!(first.path, "docs");
    assert_eq!(second.path, "docs (1)");
    assert_eq!(in_first.path, "docs/report.pdf");
    assert_eq!(in_second.path, "docs (1)/report.pdf");
    assert_eq!(again.path, "docs (1)/report (1).pdf");
    assert_eq!(namer.path_of(in_second.id), in_second.path);
}

#[test]
fn unit_zip_same_name_in_different_directories_is_not_renamed() {
    let paths = names_of(&[("a", EntryKind::Directory), ("b", EntryKind::Directory)]);
    assert_eq!(paths, ["a", "b"]);
    let mut namer = ArchiveNamer::new();
    let a = namer.allocate(None, "a", EntryKind::Directory).unwrap();
    let b = namer.allocate(None, "b", EntryKind::Directory).unwrap();
    assert_eq!(
        namer
            .allocate(Some(a.id), "r.pdf", EntryKind::File)
            .unwrap()
            .path,
        "a/r.pdf"
    );
    assert_eq!(
        namer
            .allocate(Some(b.id), "r.pdf", EntryKind::File)
            .unwrap()
            .path,
        "b/r.pdf"
    );
}

#[test]
fn unit_zip_namer_rejects_invalid_hierarchy() {
    let mut namer = ArchiveNamer::new();
    let leaf = namer.allocate(None, "leaf.txt", EntryKind::File).unwrap();
    assert_eq!(
        namer.allocate(Some(leaf.id), "child", EntryKind::File),
        Err(NameError::ParentNotDirectory { parent: leaf.id })
    );
    let mut other = ArchiveNamer::new();
    let a = other.allocate(None, "a", EntryKind::Directory).unwrap();
    let b = other
        .allocate(Some(a.id), "b", EntryKind::Directory)
        .unwrap();
    assert_eq!(
        namer.allocate(Some(b.id), "x", EntryKind::File),
        Err(NameError::UnknownParent { parent: b.id })
    );
    assert!(namer.len() == 1 && !namer.is_empty());
}

#[test]
fn unit_zip_depth_limit_is_enforced_and_fits_the_name_field() {
    let mut namer = ArchiveNamer::new();
    let mut parent = None;
    let longest = "x".repeat(MAX_SEGMENT_BYTES);
    let mut last = None;
    for _ in 0..MAX_ENTRY_DEPTH {
        let resolved = namer
            .allocate(parent, &longest, EntryKind::Directory)
            .unwrap();
        parent = Some(resolved.id);
        last = Some(resolved);
    }
    let deepest = last.unwrap();
    assert_eq!(deepest.path.len(), MAX_ENTRY_DEPTH * 256 - 1);
    assert!(deepest.path.len() < MAX_ENTRY_PATH_BYTES);
    assert!(u16::try_from(deepest.path.len() + 1).is_ok());
    assert_eq!(validate_entry_path(&deepest.path), Ok(()));
    assert_eq!(
        namer.allocate(parent, "one-too-deep", EntryKind::File),
        Err(NameError::TooDeep {
            depth: MAX_ENTRY_DEPTH + 1
        })
    );
}

#[test]
fn unit_zip_long_names_are_cut_on_character_boundaries_with_room_for_suffixes() {
    for unit in ["a", "é", "年", "📦", "e\u{301}"] {
        let long = format!("{}.txt", unit.repeat(400));
        let names = files(&[&long, &long, &long]);
        assert_eq!(names.len(), 3);
        for name in &names {
            assert!(name.len() <= MAX_SEGMENT_BYTES, "{unit}: {}", name.len());
            assert!(name.ends_with(".txt"), "{name:?}");
            assert!(std::str::from_utf8(name.as_bytes()).is_ok());
            assert!(independently_safe(name));
        }
        assert_unique_per_directory(&names);
        assert!(names[1].ends_with(" (1).txt"), "{}", names[1]);
        assert!(names[2].ends_with(" (2).txt"), "{}", names[2]);
    }
    let exact = format!("{}.txt", "b".repeat(251));
    assert_eq!(exact.len(), 255);
    assert_eq!(files(&[&exact, &exact])[0], exact);
    let second = &files(&[&exact, &exact])[1];
    assert_eq!(second.len(), 255);
    assert!(second.ends_with(" (1).txt"));
}

#[test]
fn unit_zip_an_oversized_extension_is_truncated_with_the_name() {
    let name = format!("a.{}", "z".repeat(400));
    let names = files(&[&name, &name]);
    for n in &names {
        assert!(n.len() <= MAX_SEGMENT_BYTES);
        assert!(independently_safe(n));
    }
    assert_unique_per_directory(&names);
    let dotted = format!("{}a", ".".repeat(300));
    let cleaned = sanitize_segment(&dotted);
    assert!(independently_safe(&cleaned), "{cleaned:?}");
    let series = files(&[&dotted, &dotted]);
    assert_unique_per_directory(&series);
}

#[test]
fn unit_zip_many_identical_names_resolve_without_rescanning() {
    let mut namer = ArchiveNamer::new();
    let mut seen = BTreeSet::new();
    let mut last = String::new();
    for _ in 0..5_000 {
        last = namer
            .allocate(None, "report.pdf", EntryKind::File)
            .unwrap()
            .path;
        assert!(seen.insert(last.clone()));
    }
    assert_eq!(last, "report (4999).pdf");
}

#[test]
fn unit_zip_validate_entry_path_rejects_each_unsafe_shape() {
    for (path, violation) in [
        ("", PathViolation::Empty),
        ("/etc/passwd", PathViolation::Absolute),
        ("a\\b", PathViolation::Backslash),
        ("a//b", PathViolation::EmptySegment),
        ("a/", PathViolation::EmptySegment),
        ("a/./b", PathViolation::DotSegment),
        ("a/../b", PathViolation::DotSegment),
        ("..", PathViolation::DotSegment),
        ("C:/x", PathViolation::DriveLetter),
        ("a/b\0c", PathViolation::ControlCharacter),
        ("a/b\nc", PathViolation::ControlCharacter),
        ("a/\u{202e}b", PathViolation::ReservedCharacter),
        ("a/b.", PathViolation::TrailingSpaceOrDot),
        ("a/b ", PathViolation::TrailingSpaceOrDot),
    ] {
        assert_eq!(validate_entry_path(path), Err(violation), "{path:?}");
    }
    let too_long_segment = "x".repeat(256);
    assert_eq!(
        validate_entry_path(&too_long_segment),
        Err(PathViolation::TooLong)
    );
    assert_eq!(validate_entry_path("ok/fine.txt"), Ok(()));
}

fn hostile_corpus() -> Vec<Fixture> {
    let mut f: Vec<Fixture> = vec![
        dir(None, "alpha"),
        dir(None, "beta"),
        file(Some(0), "report.pdf", b"alpha report"),
        file(Some(1), "report.pdf", b"beta report"),
        dir(None, "Received"),
    ];
    for (i, who) in ["ana", "bruno", "carla"].iter().enumerate() {
        f.push(file(
            Some(4),
            "invoice.pdf",
            format!("invoice from {who} #{i}").as_bytes(),
        ));
    }
    f.push(dir(None, "same"));
    for i in 0..3 {
        f.push(file(Some(8), "notes.txt", format!("notes {i}").as_bytes()));
    }
    f.push(file(Some(8), "report.pdf", b"r0"));
    f.push(file(Some(8), "report (1).pdf", b"r1"));
    f.push(file(Some(8), "report.pdf", b"r2"));
    f.push(dir(None, "hostile"));
    let hostile = 15;
    for (i, name) in [
        "../../etc/passwd",
        "../secret.txt",
        "/absolute/path/file.txt",
        "C:\\windows\\system32\\drivers\\etc\\hosts",
        "C:/windows/file.txt",
        "\\\\server\\share\\file.txt",
        "dir\\file.txt",
        "nul\0byte.txt",
        "line\r\nbreak.txt",
        "a/b",
        "a_b",
        "a\\b",
        "folder/../report.txt",
        "./report.txt",
        "",
        "..",
    ]
    .iter()
    .enumerate()
    {
        f.push(file(Some(hostile), name, format!("hostile {i}").as_bytes()));
    }
    let long = format!("{}.txt", "long".repeat(80));
    assert!(long.len() > 300);
    f.push(file(Some(hostile), &long, b"long one"));
    f.push(file(Some(hostile), &long, b"long two"));
    let mut parent = None;
    let mut depth_path = 0;
    let mut index = f.len();
    while depth_path < 520 {
        f.push(dir(parent, "deeper-level"));
        parent = Some(index);
        index += 1;
        depth_path += "deeper-level/".len();
    }
    f.push(file(parent, "bottom.txt", b"at the bottom"));
    f.push(file(None, "docs", b"a file named docs"));
    f.push(dir(None, "docs"));
    let docs_dir = f.len() - 1;
    f.push(file(Some(docs_dir), "inside.txt", b"inside renamed docs"));
    f.push(file(None, "Relat\u{f3}rio.pdf", b"nfc"));
    f.push(file(None, "Relato\u{301}rio.pdf", b"nfd"));
    f
}

#[test]
#[allow(non_snake_case)]
fn regression_R023_zip_entry_name_collisions() {
    let corpus = hostile_corpus();
    let first = build(&corpus, 7);
    let second = build(&corpus, 4096);

    assert_eq!(first.summary, first.planned, "emitted summary equals plan");
    assert_eq!(first.paths, second.paths, "deterministic names");
    assert_eq!(first.bytes, second.bytes, "deterministic bytes");

    for path in &first.paths {
        assert!(independently_safe(path), "unsafe entry {path:?}");
        assert!(!path.split('/').any(|s| s == ".."), "{path:?}");
    }
    assert_unique_per_directory(&first.paths);

    let file_count = corpus.iter().filter(|f| f.kind == EntryKind::File).count();
    let dir_count = corpus.len() - file_count;
    let parsed = parse_archive(&first.bytes).unwrap();
    assert_eq!(parsed.tail.entries.len(), corpus.len());
    assert_eq!(
        parsed
            .tail
            .entries
            .iter()
            .filter(|e| !e.is_directory)
            .count(),
        file_count
    );
    assert_eq!(
        parsed
            .tail
            .entries
            .iter()
            .filter(|e| e.is_directory)
            .count(),
        dir_count
    );

    let listed: Vec<String> = parsed
        .tail
        .entries
        .iter()
        .map(|e| e.name.trim_end_matches('/').to_owned())
        .collect();
    assert_eq!(listed, first.paths, "archive listing equals resolved names");
    assert_eq!(
        listed.iter().collect::<BTreeSet<_>>().len(),
        listed.len(),
        "listing has no duplicates"
    );

    for (index, fixture) in corpus.iter().enumerate() {
        assert_eq!(
            parsed.payloads[index], fixture.payload,
            "payload of input {index} ({:?}) recoverable at {:?}",
            fixture.name, first.paths[index]
        );
    }

    assert_eq!(first.paths[2], "alpha/report.pdf");
    assert_eq!(first.paths[3], "beta/report.pdf");
    assert_eq!(first.paths[5], "Received/invoice.pdf");
    assert_eq!(first.paths[6], "Received/invoice (1).pdf");
    assert_eq!(first.paths[7], "Received/invoice (2).pdf");
    assert_eq!(first.paths[9], "same/notes.txt");
    assert_eq!(first.paths[10], "same/notes (1).txt");
    assert_eq!(first.paths[11], "same/notes (2).txt");
    assert_eq!(first.paths[12], "same/report.pdf");
    assert_eq!(first.paths[13], "same/report (1).pdf");
    assert_eq!(first.paths[14], "same/report (2).pdf");
    for (index, fixture) in corpus.iter().enumerate() {
        if let Some(parent) = fixture.parent {
            assert!(
                first.paths[index].starts_with(&format!("{}/", first.paths[parent])),
                "hierarchy lost: {:?} not under {:?}",
                first.paths[index],
                first.paths[parent]
            );
        }
    }
    let deepest = first.paths.iter().map(|p| p.len()).max().unwrap();
    assert!(deepest >= 500, "corpus must include a 500+ byte path");
    assert!(
        corpus.iter().any(|f| f.name.len() > 300),
        "corpus includes a name over 300 bytes"
    );
    let docs_file = first.paths[corpus.len() - 5].as_str();
    let docs_dir = first.paths[corpus.len() - 4].as_str();
    let docs_child = first.paths[corpus.len() - 3].as_str();
    assert_eq!((docs_file, docs_dir), ("docs", "docs (1)"));
    assert_eq!(docs_child, "docs (1)/inside.txt");

    for (index, fixture) in corpus.iter().enumerate() {
        assert_eq!(
            sha256_hex(&parsed.payloads[index]),
            sha256_hex(&fixture.payload)
        );
    }
}

fn render(label: &str, fixtures: &[Fixture]) -> String {
    let (paths, entries) = zip_entries(fixtures);
    let mut out = format!("== {label}\n");
    for ((fixture, path), entry) in fixtures.iter().zip(&paths).zip(&entries) {
        let kind = match fixture.kind {
            EntryKind::File => "file",
            EntryKind::Directory => "dir ",
        };
        let parent = fixture
            .parent
            .map_or_else(|| "-".to_owned(), |index| index.to_string());
        let shown = if path.len() > 80 {
            format!(
                "{}…{} ({} bytes)",
                path.chars().take(24).collect::<String>(),
                path.chars()
                    .rev()
                    .take(24)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>(),
                path.len()
            )
        } else {
            path.clone()
        };
        let raw = if fixture.name.len() > 80 {
            format!("<{} bytes>", fixture.name.len())
        } else {
            format!("{:?}", fixture.name)
        };
        writeln!(
            out,
            "{kind} parent={parent:<2} {raw} -> {shown:?} [{}]",
            entry.name_len()
        )
        .unwrap();
    }
    out
}

#[test]
fn unit_zip_entry_name_corpus() {
    let mut snapshot = String::new();
    snapshot.push_str("== segment sanitization\n");
    for input in [
        "../secret.txt",
        "../../etc/passwd",
        "/absolute/file.txt",
        "C:\\windows\\file.txt",
        "C:/windows/file.txt",
        "\\\\server\\share\\file.txt",
        "./report.txt",
        "folder/../report.txt",
        "folder//report.txt",
        "folder\\report.txt",
        ".",
        "..",
        "...",
        "",
        "   ",
        "report.pdf.",
        "  padded  ",
        ".hidden",
        "nul\0byte",
        "cr\rlf\n",
        "tab\there",
        "\u{202e}gpj.exe",
        "full\u{ff0f}width",
        "division\u{2215}slash",
        "quote\"semi;comma,",
        "Relat\u{f3}rio.pdf",
        "Relato\u{301}rio.pdf",
        "年度報告書.pdf",
        "تقرير.pdf",
        "📦 backup.zip",
        "e\u{301}\u{302}.txt",
    ] {
        writeln!(snapshot, "{input:?} -> {:?}", sanitize_segment(input)).unwrap();
    }
    snapshot.push_str(&render(
        "same name in different folders",
        &[
            dir(None, "alpha"),
            dir(None, "beta"),
            file(Some(0), "report.pdf", b""),
            file(Some(1), "report.pdf", b""),
        ],
    ));
    snapshot.push_str(&render(
        "identical names in one folder",
        &[
            file(None, "report.pdf", b""),
            file(None, "report (1).pdf", b""),
            file(None, "report.pdf", b""),
            file(None, "report.pdf", b""),
            file(None, "archive.tar.gz", b""),
            file(None, "archive.tar.gz", b""),
            file(None, "README", b""),
            file(None, "README", b""),
            file(None, ".env", b""),
            file(None, ".env", b""),
        ],
    ));
    snapshot.push_str(&render(
        "case and normalization",
        &[
            file(None, "Report.pdf", b""),
            file(None, "report.PDF", b""),
            file(None, "caf\u{e9}.txt", b""),
            file(None, "cafe\u{301}.txt", b""),
            file(None, "\u{fb01}le.txt", b""),
            file(None, "file.txt", b""),
        ],
    ));
    snapshot.push_str(&render(
        "files and directories share a namespace",
        &[
            file(None, "docs", b""),
            dir(None, "docs"),
            file(Some(1), "report.pdf", b""),
            dir(None, "Docs"),
            file(Some(3), "report.pdf", b""),
            file(Some(3), "report.pdf", b""),
        ],
    ));
    snapshot.push_str(&render(
        "collisions after sanitization",
        &[
            file(None, "a/b", b""),
            file(None, "a_b", b""),
            file(None, "a\\b", b""),
            file(None, "a:b", b""),
            file(None, "", b""),
            file(None, "..", b""),
            file(None, "report.pdf", b""),
            file(None, "report.pdf.", b""),
        ],
    ));
    let long = format!("{}.txt", "long".repeat(80));
    let multibyte = format!("{}.txt", "年".repeat(120));
    snapshot.push_str(&render(
        "long names",
        &[
            file(None, &long, b""),
            file(None, &long, b""),
            file(None, &multibyte, b""),
            file(None, &multibyte, b""),
        ],
    ));
    insta::with_settings!({
        snapshot_path => "../../../../../tests/snapshots",
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!("zip_entry_name_corpus", snapshot);
    });
}

fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => prop::sample::select(vec![
            "report.pdf", "report (1).pdf", "Report.PDF", "notes.txt", "docs", "Docs", "a.b.c",
            "archive.tar.gz", ".env", "README", "..", ".", "", "a/b", "a_b", "a\\b", "C:\\x",
            "../../etc/passwd", "/abs", "x\0y", "tab\t", "cafe\u{301}", "caf\u{e9}", "年報.pdf",
            "تقرير", "📦", "trailing.", "trailing ", "\u{202e}rtl",
        ]).prop_map(str::to_owned),
        3 => "[a-c]{0,3}(\\.[a-c]{1,2})?",
        2 => prop::string::string_regex(".{0,40}").unwrap(),
        1 => prop::collection::vec(prop::sample::select(vec!['é', '年', '📦', 'a', '.', ' ']), 80..200)
            .prop_map(|chars| chars.into_iter().collect()),
    ]
}

fn tree_strategy() -> impl Strategy<Value = Vec<(usize, String, bool)>> {
    prop::collection::vec(
        (0usize..1000, name_strategy(), prop::bool::weighted(0.3)),
        0..40,
    )
}

fn materialize(raw: &[(usize, String, bool)]) -> Vec<Fixture> {
    let mut fixtures: Vec<Fixture> = Vec::new();
    for (pick, name, is_dir) in raw {
        let directories: Vec<usize> = fixtures
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind == EntryKind::Directory)
            .map(|(i, _)| i)
            .collect();
        let parent = if directories.is_empty() || pick % 4 == 0 {
            None
        } else {
            Some(directories[pick % directories.len()])
        };
        fixtures.push(if *is_dir {
            dir(parent, name)
        } else {
            file(parent, name, name.as_bytes())
        });
    }
    fixtures
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn prop_zip_names_are_safe_unique_hierarchical_and_deterministic(raw in tree_strategy()) {
        let fixtures = materialize(&raw);
        let (paths, entries) = zip_entries(&fixtures);
        let (again, _) = zip_entries(&fixtures);
        prop_assert_eq!(&paths, &again);
        prop_assert_eq!(paths.len(), fixtures.len());
        for (index, path) in paths.iter().enumerate() {
            prop_assert!(independently_safe(path), "{:?}", path);
            prop_assert_eq!(validate_entry_path(path), Ok(()));
            prop_assert!(entries[index].name().len() <= usize::from(u16::MAX));
            if let Some(parent) = fixtures[index].parent {
                let prefix = format!("{}/", paths[parent]);
                prop_assert!(path.starts_with(&prefix), "{:?} under {:?}", path, paths[parent]);
            }
        }
        let mut seen = BTreeSet::new();
        for path in &paths {
            let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", path));
            prop_assert!(
                seen.insert((collision_key(parent), collision_key(leaf))),
                "colliding {:?}", path
            );
        }
    }

    #[test]
    fn prop_zip_sanitize_segment_is_safe_and_idempotent(input in prop::string::string_regex("(?s).{0,400}").unwrap()) {
        let cleaned = sanitize_segment(&input);
        prop_assert!(independently_safe(&cleaned), "{:?}", cleaned);
        prop_assert!(cleaned.len() <= MAX_SEGMENT_BYTES);
        prop_assert_eq!(sanitize_segment(&cleaned), cleaned);
    }
}

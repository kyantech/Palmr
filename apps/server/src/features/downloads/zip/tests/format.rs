use proptest::prelude::*;

use super::oracle::{crc32_reference, parse_archive, parse_tail};
use super::{build, dir, emit_all, file, hex, zip_entries, CountingSink, Fixture};
use crate::features::downloads::zip::{
    archive_size, central_record_len, data_descriptor_len, local_header_len, write_central_header,
    write_data_descriptor, write_end_records, write_local_header, ArchiveSizer, ArchiveSummary,
    Crc32, EntryKind, EntryShape, FormatError, SizeError, TrailerProgress, ZipEncoder, ZipEntry,
    END_RECORDS_LEN,
};

const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

#[test]
fn unit_zip_crc32_matches_published_vectors_and_the_bitwise_reference() {
    let mut crc = Crc32::new();
    assert_eq!(crc.finalize(), 0);
    crc.update(b"123456789");
    assert_eq!(crc.finalize(), 0xCBF4_3926);
    assert_eq!(crc32_reference(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32_reference(b""), 0);
    assert_eq!(crc32_reference(b"hello"), 0x3610_A686);
    assert_eq!(
        crc32_reference(b"The quick brown fox jumps over the lazy dog"),
        0x414F_A339
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_zip_crc32_is_independent_of_chunking(
        data in prop::collection::vec(any::<u8>(), 0..2048),
        cuts in prop::collection::vec(0usize..2048, 0..12),
    ) {
        let mut points: Vec<usize> = cuts.into_iter().map(|c| c.min(data.len())).collect();
        points.sort_unstable();
        let mut crc = Crc32::new();
        let mut at = 0;
        for point in points.into_iter().chain([data.len()]) {
            crc.update(&data[at..point]);
            at = point;
        }
        prop_assert_eq!(crc.finalize(), crc32_reference(&data));
        prop_assert_eq!(crc.finalize(), crc32_reference(&data));
    }
}

#[test]
fn unit_zip_local_header_bytes_are_exact() {
    let entry = ZipEntry::file("a.txt", 5).unwrap();
    let mut out = Vec::new();
    write_local_header(&mut out, &entry);
    assert_eq!(
        out,
        hex(
            "50 4b 03 04 2d 00 08 08 00 00 00 00 21 00 00 00 00 00 ff ff ff ff ff ff ff ff \
             05 00 14 00 61 2e 74 78 74 \
             01 00 10 00 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00"
        )
    );
    assert_eq!(out.len() as u64, 55);

    let directory = ZipEntry::directory("d/e").unwrap();
    let mut out = Vec::new();
    write_local_header(&mut out, &directory);
    assert_eq!(
        out,
        hex(
            "50 4b 03 04 2d 00 00 08 00 00 00 00 21 00 00 00 00 00 ff ff ff ff ff ff ff ff \
             04 00 14 00 64 2f 65 2f \
             01 00 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"
        )
    );
}

#[test]
fn unit_zip_data_descriptor_bytes_are_exact() {
    let mut out = Vec::new();
    write_data_descriptor(&mut out, 0x3610_A686, 5);
    assert_eq!(
        out,
        hex("50 4b 07 08 86 a6 10 36 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00")
    );
    let mut out = Vec::new();
    write_data_descriptor(&mut out, 1, 6 * GIB);
    assert_eq!(
        out,
        hex("50 4b 07 08 01 00 00 00 00 00 00 80 01 00 00 00 00 00 00 80 01 00 00 00")
    );
}

#[test]
fn unit_zip_central_header_bytes_are_exact_with_and_without_the_offset_field() {
    let entry = ZipEntry::file("a.txt", 5).unwrap();
    let mut out = Vec::new();
    write_central_header(&mut out, &entry, 0x3610_A686, 0x1_0000);
    assert_eq!(
        out,
        hex(
            "50 4b 01 02 2d 03 2d 00 08 08 00 00 00 00 21 00 86 a6 10 36 ff ff ff ff ff ff ff ff \
             05 00 14 00 00 00 00 00 00 00 00 00 a4 81 00 00 01 00 \
             61 2e 74 78 74 \
             01 00 10 00 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00"
        )
    );

    let mut out = Vec::new();
    write_central_header(&mut out, &entry, 0x3610_A686, 5 * GIB + 3);
    assert_eq!(
        out,
        hex(
            "50 4b 01 02 2d 03 2d 00 08 08 00 00 00 00 21 00 86 a6 10 36 ff ff ff ff ff ff ff ff \
             05 00 1c 00 00 00 00 00 00 00 00 00 a4 81 ff ff ff ff \
             61 2e 74 78 74 \
             01 00 18 00 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00 03 00 00 40 01 00 00 00"
        )
    );

    let directory = ZipEntry::directory("d").unwrap();
    let mut out = Vec::new();
    write_central_header(&mut out, &directory, 0, 0);
    assert_eq!(&out[8..10], hex("00 08"));
    assert_eq!(&out[38..42], hex("10 00 ed 41"));
}

#[test]
fn unit_zip_end_records_for_an_empty_archive_are_exact() {
    let mut encoder = ZipEncoder::new();
    let mut out = Vec::new();
    let progress = encoder.emit_trailer(&mut out, 4096).unwrap();
    assert_eq!(
        progress,
        TrailerProgress::Complete(ArchiveSummary {
            total_len: 98,
            entry_count: 0,
            central_directory_offset: 0,
            central_directory_len: 0,
        })
    );
    assert_eq!(
        out,
        [
            hex("50 4b 06 06 2c 00 00 00 00 00 00 00 2d 03 2d 00"),
            vec![0; 40],
            hex("50 4b 06 07 00 00 00 00 00 00 00 00 00 00 00 00 01 00 00 00"),
            hex("50 4b 05 06 00 00 00 00 00 00 00 00 00 00 00 00 00 00"),
            vec![0; 4],
        ]
        .concat()
    );
    assert_eq!(out.len() as u64, END_RECORDS_LEN);
    assert_eq!(archive_size([]).unwrap().total_len, 98);
    let parsed = parse_tail(&out, 0).unwrap();
    assert!(parsed.entries.is_empty());
}

#[test]
fn unit_zip_end_records_saturate_only_the_fields_that_do_not_fit() {
    let summary = ArchiveSummary {
        total_len: 0,
        entry_count: 65_536,
        central_directory_offset: 5 * GIB,
        central_directory_len: 123,
    };
    let mut out = Vec::new();
    write_end_records(&mut out, &summary).unwrap();
    let end = &out[out.len() - 22..];
    assert_eq!(&end[8..10], hex("ff ff"));
    assert_eq!(&end[10..12], hex("ff ff"));
    assert_eq!(&end[12..16], hex("7b 00 00 00"));
    assert_eq!(&end[16..20], hex("ff ff ff ff"));
    let zip64 = &out[..56];
    assert_eq!(&zip64[24..32], 65_536u64.to_le_bytes());
    assert_eq!(&zip64[40..48], 123u64.to_le_bytes());
    assert_eq!(&zip64[48..56], (5 * GIB).to_le_bytes());
    let locator = &out[56..76];
    assert_eq!(&locator[8..16], (5 * GIB + 123).to_le_bytes());

    for (count, expected) in [(65_534u64, 65_534u16), (65_535, 0xFFFF), (65_536, 0xFFFF)] {
        let mut out = Vec::new();
        write_end_records(
            &mut out,
            &ArchiveSummary {
                total_len: 0,
                entry_count: count,
                central_directory_offset: 0xFFFF_FFFE,
                central_directory_len: 0xFFFF_FFFF,
            },
        )
        .unwrap();
        let end = &out[out.len() - 22..];
        assert_eq!(u16::from_le_bytes([end[8], end[9]]), expected);
        assert_eq!(&end[12..16], hex("ff ff ff ff"));
        assert_eq!(&end[16..20], hex("fe ff ff ff"));
    }

    let mut out = Vec::new();
    assert_eq!(
        write_end_records(
            &mut out,
            &ArchiveSummary {
                total_len: 0,
                entry_count: 0,
                central_directory_offset: u64::MAX,
                central_directory_len: 1,
            }
        ),
        Err(FormatError::OffsetOverflow)
    );
}

#[test]
fn unit_zip_hello_archive_matches_a_hand_assembled_stream() {
    let entry = ZipEntry::file("a.txt", 5).unwrap();
    let mut sink = CountingSink::new();
    let summary = emit_all(&[entry], &[b"hello"], 2, 4096, &mut sink);
    let mut expected = hex(
        "50 4b 03 04 2d 00 08 08 00 00 00 00 21 00 00 00 00 00 ff ff ff ff ff ff ff ff \
         05 00 14 00 61 2e 74 78 74 01 00 10 00 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00",
    );
    expected.extend_from_slice(b"hello");
    expected.extend(hex(
        "50 4b 07 08 86 a6 10 36 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00",
    ));
    assert_eq!(expected.len(), 55 + 5 + 24);
    expected.extend(hex(
        "50 4b 01 02 2d 03 2d 00 08 08 00 00 00 00 21 00 86 a6 10 36 ff ff ff ff ff ff ff ff \
         05 00 14 00 00 00 00 00 00 00 00 00 a4 81 00 00 00 00 \
         61 2e 74 78 74 01 00 10 00 05 00 00 00 00 00 00 00 05 00 00 00 00 00 00 00",
    ));
    expected.extend(hex(
        "50 4b 06 06 2c 00 00 00 00 00 00 00 2d 03 2d 00 00 00 00 00 00 00 00 00 \
         01 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 47 00 00 00 00 00 00 00 54 00 00 00 00 00 00 00 \
         50 4b 06 07 00 00 00 00 9b 00 00 00 00 00 00 00 01 00 00 00 \
         50 4b 05 06 00 00 00 00 01 00 01 00 47 00 00 00 54 00 00 00 00 00",
    ));
    assert_eq!(sink.bytes, expected);
    assert_eq!(summary.total_len, expected.len() as u64);
    assert_eq!(summary.central_directory_offset, 84);
    assert_eq!(summary.central_directory_len, 71);
}

fn payload_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        2 => Just(Vec::new()),
        5 => prop::collection::vec(any::<u8>(), 1..300),
        1 => prop::collection::vec(any::<u8>(), 1..2500),
    ]
}

fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => prop::sample::select(vec![
            "report.pdf", "report.pdf", "report (1).pdf", "Report.PDF", "docs", "a.b.c", "..",
            "", "a/b", "a_b", "C:\\x", "../../etc/passwd", "x\0y", "年報.pdf", "تقرير", "📦",
            "Relat\u{f3}rio.pdf", "Relato\u{301}rio.pdf",
        ]).prop_map(str::to_owned),
        3 => "[a-c]{0,3}(\\.[a-c]{1,2})?",
        1 => prop::collection::vec(prop::sample::select(vec!['é', '年', '📦', 'a', '.']), 100..400)
            .prop_map(|chars| chars.into_iter().collect()),
    ]
}

fn archive_strategy() -> impl Strategy<Value = Vec<(usize, String, bool, Vec<u8>)>> {
    prop::collection::vec(
        (
            0usize..1000,
            name_strategy(),
            prop::bool::weighted(0.3),
            payload_strategy(),
        ),
        0..24,
    )
}

fn fixtures_from(raw: &[(usize, String, bool, Vec<u8>)]) -> Vec<Fixture> {
    let mut fixtures: Vec<Fixture> = Vec::new();
    for (pick, name, is_dir, payload) in raw {
        let directories: Vec<usize> = fixtures
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind == EntryKind::Directory)
            .map(|(i, _)| i)
            .collect();
        let parent = if directories.is_empty() || pick % 3 == 0 {
            None
        } else {
            Some(directories[pick % directories.len()])
        };
        fixtures.push(if *is_dir {
            dir(parent, name)
        } else {
            file(parent, name, payload)
        });
    }
    fixtures
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(160))]

    #[test]
    fn prop_zip_total_size_matches_emitted_bytes(
        raw in archive_strategy(),
        chunk in 1usize..700,
        budget in 1usize..6000,
    ) {
        let fixtures = fixtures_from(&raw);
        let (_, entries) = zip_entries(&fixtures);
        let payloads: Vec<&[u8]> = fixtures.iter().map(|f| f.payload.as_slice()).collect();

        let planned = archive_size(entries.iter().map(EntryShape::from)).unwrap();
        let mut sink = CountingSink::new();
        let summary = emit_all(&entries, &payloads, chunk, budget, &mut sink);

        prop_assert_eq!(planned.total_len, sink.counted);
        prop_assert_eq!(planned.total_len, sink.bytes.len() as u64);
        prop_assert_eq!(planned, summary);

        let parsed = parse_archive(&sink.bytes).map_err(TestCaseError::fail)?;
        prop_assert_eq!(parsed.tail.total_len, planned.total_len);
        prop_assert_eq!(parsed.tail.central_directory_offset, planned.central_directory_offset);
        prop_assert_eq!(parsed.tail.central_directory_len, planned.central_directory_len);
        prop_assert_eq!(parsed.tail.entries.len() as u64, planned.entry_count);
        prop_assert_eq!(parsed.tail.entries.len(), fixtures.len());
        for (index, fixture) in fixtures.iter().enumerate() {
            prop_assert_eq!(&parsed.payloads[index], &fixture.payload);
            prop_assert_eq!(parsed.tail.entries[index].crc32, crc32_reference(&fixture.payload));
            prop_assert_eq!(parsed.tail.entries[index].name.as_str(), entries[index].name());
        }
        let mut names: Vec<&str> = parsed.tail.entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        prop_assert_eq!(names.len(), fixtures.len());

        let again = build(&fixtures, 1 + chunk % 13);
        prop_assert_eq!(&again.bytes, &sink.bytes);
    }
}

#[test]
fn unit_zip_archive_bytes_do_not_depend_on_chunking_or_trailer_budget() {
    let fixtures = vec![
        dir(None, "d"),
        file(Some(0), "a.bin", &[7u8; 1500]),
        file(None, "empty", b""),
        file(Some(0), "b.bin", &[9u8; 33]),
    ];
    let (_, entries) = zip_entries(&fixtures);
    let payloads: Vec<&[u8]> = fixtures.iter().map(|f| f.payload.as_slice()).collect();
    let mut reference = CountingSink::new();
    emit_all(&entries, &payloads, 4096, 1 << 20, &mut reference);
    for (chunk, budget) in [(1, 1), (7, 50), (64, 100), (1500, 1), (4096, 4096)] {
        let mut sink = CountingSink::new();
        emit_all(&entries, &payloads, chunk, budget, &mut sink);
        assert_eq!(sink.bytes, reference.bytes, "chunk {chunk} budget {budget}");
    }
}

#[test]
fn unit_zip_emission_buffers_stay_bounded_regardless_of_payload_size() {
    let entry = ZipEntry::file("big.bin", 16 * 1024 * 1024).unwrap();
    let mut encoder = ZipEncoder::new();
    let mut out = Vec::new();
    encoder.begin_entry(&entry, &mut out).unwrap();
    let mut peak = out.len();
    let chunk = vec![0xA5u8; 64 * 1024];
    for _ in 0..256 {
        encoder.observe_payload(&chunk).unwrap();
    }
    out.clear();
    encoder.end_entry(&mut out).unwrap();
    peak = peak.max(out.len());
    out.clear();
    assert!(matches!(
        encoder.emit_trailer(&mut out, 4096).unwrap(),
        TrailerProgress::Complete(_)
    ));
    peak = peak.max(out.len());
    assert!(peak < 1024, "largest record buffer {peak}");
    assert_eq!(
        encoder.bytes_emitted(),
        archive_size([EntryShape::from(&entry)]).unwrap().total_len
    );
}

struct Synthetic {
    non_payload: Vec<u8>,
    assumed: u64,
    summary: ArchiveSummary,
}

fn synthetic(entries: &[ZipEntry]) -> Synthetic {
    let mut encoder = ZipEncoder::new();
    let mut non_payload = Vec::new();
    let mut assumed = 0u64;
    for entry in entries {
        encoder.begin_entry(entry, &mut non_payload).unwrap();
        if entry.size() > 0 {
            encoder.assume_payload(entry.size(), 0xDEAD_BEEF).unwrap();
            assumed += entry.size();
        }
        encoder.end_entry(&mut non_payload).unwrap();
    }
    let summary = loop {
        match encoder.emit_trailer(&mut non_payload, 1 << 16).unwrap() {
            TrailerProgress::More => {}
            TrailerProgress::Complete(summary) => break summary,
        }
    };
    Synthetic {
        non_payload,
        assumed,
        summary,
    }
}

struct Expected {
    offsets: Vec<u128>,
    central_directory_offset: u128,
    central_directory_len: u128,
    total: u128,
}

fn expected_layout(entries: &[(u128, bool, u128)]) -> Expected {
    let mut offsets = Vec::new();
    let mut at = 0u128;
    for (name_len, is_dir, size) in entries {
        offsets.push(at);
        at += 30 + name_len + 20 + size + if *is_dir { 0 } else { 24 };
    }
    let mut central = 0u128;
    for ((name_len, _, _), offset) in entries.iter().zip(&offsets) {
        central += 46 + name_len + if *offset >= 0xFFFF_FFFF { 28 } else { 20 };
    }
    Expected {
        offsets,
        central_directory_offset: at,
        central_directory_len: central,
        total: at + central + 98,
    }
}

fn check_synthetic(entries: &[ZipEntry]) {
    let built = synthetic(entries);
    let shape: Vec<(u128, bool, u128)> = entries
        .iter()
        .map(|e| {
            (
                u128::from(e.name_len()),
                e.kind() == EntryKind::Directory,
                u128::from(e.size()),
            )
        })
        .collect();
    let expected = expected_layout(&shape);
    let planned = archive_size(entries.iter().map(EntryShape::from)).unwrap();

    assert_eq!(u128::from(planned.total_len), expected.total);
    assert_eq!(u128::from(built.summary.total_len), expected.total);
    assert_eq!(
        u128::from(built.non_payload.len() as u64) + u128::from(built.assumed),
        expected.total,
        "independent byte count of emitted records plus skipped payload"
    );
    assert_eq!(planned, built.summary);
    assert_eq!(
        u128::from(planned.central_directory_offset),
        expected.central_directory_offset
    );
    assert_eq!(
        u128::from(planned.central_directory_len),
        expected.central_directory_len
    );

    let tail_len = (planned.central_directory_len + END_RECORDS_LEN) as usize;
    let tail = &built.non_payload[built.non_payload.len() - tail_len..];
    let parsed = parse_tail(tail, planned.central_directory_offset).unwrap();
    assert_eq!(parsed.entries.len(), entries.len());
    for (index, record) in parsed.entries.iter().enumerate() {
        assert_eq!(u128::from(record.local_offset), expected.offsets[index]);
        assert_eq!(record.uncompressed, entries[index].size());
        assert_eq!(record.compressed, entries[index].size());
        assert_eq!(record.name, entries[index].name());
        if entries[index].size() > 0 {
            assert_eq!(record.crc32, 0xDEAD_BEEF);
        }
        let with_offset = expected.offsets[index] >= 0xFFFF_FFFF;
        assert_eq!(record.extra_len, if with_offset { 28 } else { 20 });
    }
}

fn sized_entries(sizes: &[u64]) -> Vec<ZipEntry> {
    sizes
        .iter()
        .enumerate()
        .map(|(i, size)| ZipEntry::file(&format!("f{i}.bin"), *size).unwrap())
        .collect()
}

#[test]
fn unit_zip_large_entry_sizes_are_exact_in_every_record() {
    let sizes = [
        0,
        1,
        1 << 16,
        (1 << 32) - 1,
        1 << 32,
        (1 << 32) + 1,
        5 * GIB,
        100 * GIB,
        TIB,
    ];
    for size in sizes {
        check_synthetic(&sized_entries(&[size]));
        check_synthetic(&sized_entries(&[1, size, 1]));
    }
    check_synthetic(&sized_entries(&sizes));
    let mut reversed = sizes;
    reversed.reverse();
    check_synthetic(&sized_entries(&reversed));
}

#[test]
fn unit_zip_a_local_header_offset_of_exactly_four_gib_minus_one_needs_the_offset_field() {
    let first_overhead = 30 + 4 + 20 + 24;
    let target = u64::from(u32::MAX);
    for (size, field_expected) in [
        (target - first_overhead - 1, false),
        (target - first_overhead, true),
        (target - first_overhead + 1, true),
    ] {
        let entries = vec![
            ZipEntry::file("f0.b", size).unwrap(),
            ZipEntry::file("f1.b", 3).unwrap(),
        ];
        check_synthetic(&entries);
        let built = synthetic(&entries);
        let tail_len = (built.summary.central_directory_len + END_RECORDS_LEN) as usize;
        let tail = &built.non_payload[built.non_payload.len() - tail_len..];
        let parsed = parse_tail(tail, built.summary.central_directory_offset).unwrap();
        assert_eq!(
            parsed.entries[1].extra_len == 28,
            field_expected,
            "second entry offset {}",
            parsed.entries[1].local_offset
        );
        assert_eq!(parsed.entries[0].extra_len, 20);
    }
}

#[test]
fn unit_zip_archive_totals_beyond_four_gib_with_only_small_entries() {
    let entries = sized_entries(&vec![16 * 1024 * 1024; 300]);
    let planned = archive_size(entries.iter().map(EntryShape::from)).unwrap();
    assert!(planned.total_len > 4 * GIB);
    check_synthetic(&entries);
    let built = synthetic(&entries);
    let tail_len = (built.summary.central_directory_len + END_RECORDS_LEN) as usize;
    let tail = &built.non_payload[built.non_payload.len() - tail_len..];
    let parsed = parse_tail(tail, built.summary.central_directory_offset).unwrap();
    let with_offset = parsed.entries.iter().filter(|e| e.extra_len == 28).count();
    assert!(with_offset > 0 && with_offset < parsed.entries.len());
    assert!(parsed
        .entries
        .iter()
        .all(|e| e.uncompressed == 16 * 1024 * 1024));
}

#[test]
fn unit_zip_size_arithmetic_rejects_overflow_and_unrepresentable_shapes() {
    let shape = |size: u64| EntryShape {
        name_len: 1,
        kind: EntryKind::File,
        size,
    };
    assert_eq!(archive_size([shape(u64::MAX)]), Err(SizeError::Overflow));
    assert_eq!(
        archive_size([shape(u64::MAX - 100)]),
        Err(SizeError::Overflow)
    );
    assert_eq!(
        archive_size([shape(1 << 63), shape(1 << 63)]),
        Err(SizeError::Overflow)
    );
    let near = u64::MAX - 98 - 51 - 24 - 47 - 20;
    assert!(archive_size([shape(near)]).is_ok());
    assert_eq!(archive_size([shape(near + 1)]), Err(SizeError::Overflow));

    assert_eq!(
        archive_size([EntryShape {
            name_len: 65_536,
            kind: EntryKind::File,
            size: 0
        }]),
        Err(SizeError::NameTooLong { bytes: 65_536 })
    );
    assert!(archive_size([EntryShape {
        name_len: 65_535,
        kind: EntryKind::File,
        size: 0
    }])
    .is_ok());
    assert_eq!(
        archive_size([EntryShape {
            name_len: 1,
            kind: EntryKind::Directory,
            size: 1
        }]),
        Err(SizeError::DirectoryHasPayload { bytes: 1 })
    );

    let mut sizer = ArchiveSizer::new();
    sizer.push(shape(1 << 63)).unwrap();
    let before = sizer.finish().unwrap();
    assert_eq!(sizer.push(shape(1 << 63)), Err(SizeError::Overflow));
    assert_eq!(
        sizer.finish().unwrap(),
        before,
        "a failed push changes nothing"
    );
}

#[test]
fn unit_zip_zip_entry_constructors_enforce_the_name_contract() {
    assert!(matches!(
        ZipEntry::file("../x", 0),
        Err(FormatError::UnsafeName(_))
    ));
    assert!(matches!(
        ZipEntry::file("/x", 0),
        Err(FormatError::UnsafeName(_))
    ));
    assert!(matches!(
        ZipEntry::directory("a\\b"),
        Err(FormatError::UnsafeName(_))
    ));
    assert_eq!(ZipEntry::directory("a/b").unwrap().name(), "a/b/");
    let max_segment = "x".repeat(255);
    let path = vec![max_segment.as_str(); 128].join("/");
    let entry = ZipEntry::directory(&path).unwrap();
    assert_eq!(usize::from(entry.name_len()), path.len() + 1);
}

fn empty_file_entries(count: usize) -> Vec<ZipEntry> {
    (0..count)
        .map(|i| ZipEntry::file(&format!("d{}/f{i}", i % 7), 0).unwrap())
        .collect()
}

#[test]
fn unit_zip_entry_counts_around_the_sixteen_bit_limit_are_valid() {
    for count in [0usize, 1, 2, 65_534, 65_535, 65_536, 70_000] {
        let entries = empty_file_entries(count);
        let payloads: Vec<&[u8]> = vec![&[]; count];
        let mut sink = CountingSink::new();
        let summary = emit_all(&entries, &payloads, 1, 1 << 16, &mut sink);
        let planned = archive_size(entries.iter().map(EntryShape::from)).unwrap();
        assert_eq!(planned, summary, "{count}");
        assert_eq!(planned.total_len, sink.counted, "{count}");
        assert_eq!(summary.entry_count, count as u64);

        let parsed = parse_archive(&sink.bytes).unwrap();
        assert_eq!(parsed.tail.entries.len(), count);
        let end = &sink.bytes[sink.bytes.len() - 22..];
        let legacy = u16::from_le_bytes([end[10], end[11]]);
        let expected = if count >= 65_535 {
            0xFFFF
        } else {
            count as u16
        };
        assert_eq!(legacy, expected, "{count}");
        let zip64_total = {
            let at = sink.bytes.len() - 22 - 20 - 56 + 32;
            u64::from_le_bytes(sink.bytes[at..at + 8].try_into().unwrap())
        };
        assert_eq!(zip64_total, count as u64);
        let unique: std::collections::BTreeSet<_> = parsed
            .tail
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(unique.len(), count);
    }
}

#[test]
fn unit_zip_encoder_rejects_out_of_order_operations() {
    let entry = ZipEntry::file("a", 2).unwrap();
    let mut encoder = ZipEncoder::new();
    let mut out = Vec::new();
    assert_eq!(encoder.observe_payload(b"x"), Err(FormatError::NoOpenEntry));
    assert_eq!(encoder.end_entry(&mut out), Err(FormatError::NoOpenEntry));
    encoder.begin_entry(&entry, &mut out).unwrap();
    assert_eq!(
        encoder.begin_entry(&entry, &mut out),
        Err(FormatError::EntryAlreadyOpen)
    );
    assert_eq!(
        encoder.emit_trailer(&mut out, 1),
        Err(FormatError::EntryAlreadyOpen)
    );
    encoder.observe_payload(b"x").unwrap();
    assert_eq!(
        encoder.end_entry(&mut out),
        Err(FormatError::PayloadShort {
            declared: 2,
            written: 1
        })
    );
    assert_eq!(
        encoder.observe_payload(b"yz"),
        Err(FormatError::PayloadOverrun { declared: 2 })
    );
    encoder.observe_payload(b"y").unwrap();
    encoder.end_entry(&mut out).unwrap();
    assert!(matches!(
        encoder.emit_trailer(&mut out, 1).unwrap(),
        TrailerProgress::Complete(_)
    ));
    assert_eq!(
        encoder.begin_entry(&entry, &mut out),
        Err(FormatError::TrailerStarted)
    );
    assert_eq!(
        encoder.emit_trailer(&mut out, 1),
        Err(FormatError::ArchiveFinished)
    );
}

#[test]
fn unit_zip_encoder_rejects_a_directory_with_payload_and_offset_overflow() {
    let mut encoder = ZipEncoder::new();
    let mut out = Vec::new();
    let directory = ZipEntry::directory("d").unwrap();
    encoder.begin_entry(&directory, &mut out).unwrap();
    assert_eq!(
        encoder.observe_payload(b"x"),
        Err(FormatError::PayloadOverrun { declared: 0 })
    );
    encoder.end_entry(&mut out).unwrap();

    let huge = ZipEntry::file("h", u64::MAX).unwrap();
    let mut encoder = ZipEncoder::new();
    encoder.begin_entry(&huge, &mut out).unwrap();
    encoder.assume_payload(u64::MAX - 100, 0).unwrap();
    assert_eq!(
        encoder.assume_payload(60, 0),
        Err(FormatError::OffsetOverflow)
    );
}

#[test]
fn unit_zip_only_the_store_method_is_ever_written() {
    let fixtures = vec![
        file(None, "a.txt", &[0x41; 4000]),
        dir(None, "d"),
        file(Some(1), "b.txt", b"b"),
    ];
    let built = build(&fixtures, 100);
    let parsed = parse_archive(&built.bytes).unwrap();
    for entry in &parsed.tail.entries {
        assert_eq!(entry.compressed, entry.uncompressed, "{}", entry.name);
    }
    assert!(built.counted > 4000);
    let mut at = 0;
    while let Some(found) = built.bytes[at..]
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x03, 0x04])
    {
        let header = at + found;
        assert_eq!(&built.bytes[header + 8..header + 10], [0, 0]);
        at = header + 4;
    }
}

#[test]
fn unit_zip_utf8_names_are_flagged_and_identical_in_local_and_central_records() {
    let names = [
        "Relatório.pdf",
        "年度報告書.pdf",
        "تقرير.pdf",
        "📦 backup.zip",
    ];
    let fixtures: Vec<Fixture> = names
        .iter()
        .map(|name| file(None, name, name.as_bytes()))
        .collect();
    let built = build(&fixtures, 5);
    let parsed = parse_archive(&built.bytes).unwrap();
    for (entry, name) in parsed.tail.entries.iter().zip(names) {
        assert_eq!(entry.name, name);
        assert_eq!(entry.raw_name, name.as_bytes());
        assert_eq!(entry.flags & (1 << 11), 1 << 11);
        let local = built
            .bytes
            .windows(entry.raw_name.len())
            .position(|w| w == entry.raw_name.as_slice())
            .unwrap();
        assert_eq!(
            u16::from_le_bytes([built.bytes[local - 24], built.bytes[local - 23]]) & (1 << 11),
            1 << 11
        );
    }
}

#[test]
fn unit_zip_length_helpers_equal_the_bytes_the_record_writers_emit() {
    let mut paths = vec!["a".to_owned(), "dir/é.txt".to_owned(), "年".repeat(80)];
    paths.push(vec!["x".repeat(255); 100].join("/"));
    for path in paths {
        for entry in [
            ZipEntry::file(&path, 9).unwrap(),
            ZipEntry::directory(&path).unwrap(),
        ] {
            let mut local = Vec::new();
            write_local_header(&mut local, &entry);
            assert_eq!(local.len() as u64, local_header_len(entry.name_len()));
            let mut descriptor = Vec::new();
            if entry.kind() == EntryKind::File {
                write_data_descriptor(&mut descriptor, 1, entry.size());
            }
            assert_eq!(descriptor.len() as u64, data_descriptor_len(entry.kind()));
            for offset in [0, 1, u64::from(u32::MAX) - 1, u64::from(u32::MAX), 7 * GIB] {
                let mut central = Vec::new();
                write_central_header(&mut central, &entry, 1, offset);
                assert_eq!(
                    central.len() as u64,
                    central_record_len(entry.name_len(), offset),
                    "{} at {offset}",
                    entry.name()
                );
            }
        }
    }
    let mut end = Vec::new();
    write_end_records(
        &mut end,
        &ArchiveSummary {
            total_len: 0,
            entry_count: 3,
            central_directory_offset: 10,
            central_directory_len: 10,
        },
    )
    .unwrap();
    assert_eq!(end.len() as u64, END_RECORDS_LEN);
}

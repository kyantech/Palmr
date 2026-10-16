# ZIP64 STORE writer (pure)

Introduced in M15-T03. It implements ADR 0028, API_DESIGN §2.16 (entry naming rules), SECURITY_MODEL §9.7 and R-023. It is the pure foundation of the archive stream (M15-T04): no HTTP, no database, no storage, no tickets, no persistent state.

| Artefact | Location |
|---|---|
| Entry-name builder | `apps/server/src/features/downloads/zip/names.rs` |
| Binary records and the sans-IO encoder | `apps/server/src/features/downloads/zip/format.rs` |
| Exact length computation | `apps/server/src/features/downloads/zip/size.rs` |
| Tests, including the independent ZIP oracle | `apps/server/src/features/downloads/zip/tests/` |
| Name snapshot (G9) | `apps/server/tests/snapshots/zip_entry_name_corpus.snap` |
| Dependency | `crc32fast` (CRC-32 only; it was already in the lock file) |

## Input model

Entries form a tree. `ArchiveNamer::allocate(parent, display_name, kind)` takes the parent allocated earlier (`None` for the selection root), the display name and `File` or `Directory`, and returns the sanitized relative path. Input order is the only ordering; the same order always gives the same names. Identity is the `NodeId` returned by `allocate`, never the name, so two folders that are both called `docs` stay two directories. Directories are explicit entries (an empty folder is preserved as `name/`). A parent that is unknown or not a directory is a typed error.

## Entry names

Each display name is one path segment. Separators inside a name never become hierarchy: hierarchy comes only from the parent chain.

| Rule | Behaviour |
|---|---|
| Control characters (Cc), U+2028/2029, bidi embeddings, overrides and isolates (U+202A–202E, U+2066–206F), zero-width and invisible formatting (U+200B–200F, U+2060–2064, U+FEFF) | replaced by `_` |
| `/`, `\`, `:` and any character whose NFKC form is one of them (for example U+FF0F, U+FF3C, U+FF1A) | replaced by `_` (so no drive prefix, UNC prefix, separator or alternate-stream marker survives) |
| Normalization | the segment is emitted in NFC |
| Leading whitespace, trailing dots and whitespace | removed |
| Empty result (`""`, `.`, `..`, `...`, blanks) | `_` |
| Ordinary punctuation (quotes, `;`, `,`) and all other Unicode (accents, CJK, RTL, emoji) | preserved |
| Segment longer than 255 bytes | cut on a character boundary, extension kept |
| Depth above 128 segments | `NameError::TooDeep` |

A path therefore never exceeds `128 × 255 + 127` bytes (the maximum accepted depth is 128 segments), far below the 65 535-byte ZIP name field. The folder feature itself caps depth at 64. The member name is checked again by `ZipEntry::file` / `ZipEntry::directory` with `validate_entry_path`, which is a separate predicate from the builder, so a caller cannot hand the encoder an unsafe name.

Collisions are resolved per directory, files and directories sharing one namespace. The collision key is `domain::normalize::normalize` (NFKC, lower case), the same key the Files feature uses, so `Report.pdf`, `report.PDF`, `café.txt` (NFC) and `café.txt` (NFD) collide and are never left to overwrite each other on a case-insensitive or normalizing extractor. The suffix convention is the Files keep-both one: `report.pdf`, `report (1).pdf`, `report (2).pdf`, `archive.tar (1).gz`, `README (1)`; directory names are never split on a dot. A candidate is only accepted if its key is free, so a pre-existing `report (1).pdf` is skipped, never duplicated. When the suffix would push a name over 255 bytes the base is cut (on a character boundary, extension kept) to leave room for it. Children are resolved against the renamed parent, so `docs (1)/report.pdf` follows `docs (1)`.

A per-series cursor remembers the next attempt number, so N identical names cost O(N log N) in total, not O(N²). All maps are `BTreeMap`/`BTreeSet`; nothing depends on hash order. The namer keeps O(entries) metadata.

## Binary layout

STORE (method 0) only. Every entry is ZIP64, whatever its size.

| Record | Layout |
|---|---|
| Local header (30 + name + 20 bytes) | signature, version needed `45`, flags, method `0`, time `00:00`, date `1980-01-01`, CRC `0`, compressed and uncompressed 32-bit fields `0xFFFFFFFF`, name length, extra length `20`, name, ZIP64 extra `0x0001/16` holding the **real** uncompressed then compressed size |
| Payload | the source bytes, unchanged |
| Data descriptor (files only, 24 bytes) | `PK\x07\x08`, CRC-32, compressed size (8), uncompressed size (8) |
| Central header (46 + name + 20 or 28 bytes) | version made by `0x032D` (Unix, 4.5), version needed `45`, the local flags, CRC, sizes `0xFFFFFFFF`, name length, extra length, no comment, disk 0, internal attributes 0, external attributes (`0100644 << 16` for files, `040755 << 16 \| 0x10` for directories), offset, name, ZIP64 extra |
| Central ZIP64 extra | uncompressed size, compressed size, and the local header offset **only when it does not fit** (`offset >= 0xFFFFFFFF`); the 32-bit offset field is then `0xFFFFFFFF`. An offset of exactly `0xFFFFFFFF` is therefore written through the extra |
| ZIP64 end of central directory (56 bytes) | size field `44`, versions, disks 0, entry counts (8), central directory size (8) and offset (8) |
| ZIP64 locator (20 bytes) | disk 0, offset of the ZIP64 record, 1 disk |
| End of central directory (22 bytes) | real values when they fit; `0xFFFF` (count) and `0xFFFFFFFF` (size, offset) when the value is `>=` the marker |

General-purpose flags are `0x0808` for files (bit 3, data descriptor; bit 11, UTF-8 names) and `0x0800` for directories, identical in the local and central headers. Local and central headers carry the same name bytes.

Why the local header carries the true sizes while bit 3 is set: the CRC is unknown when the header is written (the source is streamed), so bit 3 and a descriptor are mandatory, but the size is authoritative and known. Putting it in the local ZIP64 extra lets a reader that never sees the central directory find the end of a STORED entry. `bsdtar -xf -` reading from a pipe extracts the archive this way (see Verification). The local CRC stays `0`, as APPNOTE 4.4.4 requires.

The DOS timestamp is the fixed epoch above, so identical input gives identical bytes. No modification times are written.

## CRC-32

`Crc32` wraps `crc32fast::Hasher`: 4 bytes of state, `update(chunk)` per bounded chunk, `finalize()` once. It never holds payload bytes. Tests compare it with a bitwise reference implementation written in the test oracle, for any chunking.

## Exact length

`ArchiveSizer::push(EntryShape { name_len, kind, size })` and `finish()`, or `archive_size(iter)`, return `ArchiveSummary { total_len, entry_count, central_directory_offset, central_directory_len }`, all `u64`, all checked.

```text
per entry   local   = 30 + name + 20
            body    = size (+ 24 for a file)
per entry   central = 46 + name + (28 if offset >= 0xFFFFFFFF else 20)
archive     total   = Σ(local + body) + Σ central + 98        (56 + 20 + 22)
```

The central size depends on each entry's offset, so the sizer accumulates offsets exactly as the encoder does. Both call the same `local_header_len`, `data_descriptor_len` and `central_record_len` functions defined next to the record writers, and a test asserts each length function equals the bytes its writer emits. Errors: `Overflow`, `NameTooLong` (does not fit 16 bits), `DirectoryHasPayload`. There is no saturation and no wraparound; a failed `push` leaves the sizer unchanged.

## API for the archive stream (M15-T04)

`ZipEncoder` is sans-IO and appends to a caller-owned `Vec<u8>`:

1. `begin_entry(&ZipEntry, &mut out)` appends the local header.
2. The caller sends the payload itself and calls `observe_payload(&chunk)` for each chunk (CRC and length accounting; the encoder never copies or stores payload bytes).
3. `end_entry(&mut out)` checks the declared size, appends the data descriptor and remembers the entry for the central directory.
4. `emit_trailer(&mut out, budget)` is called until it returns `Complete(summary)`; each call appends roughly `budget` bytes of central directory records, and the final call appends the three end records.

Calls out of order, a payload longer or shorter than declared, and offset overflow are `FormatError`s. The encoder keeps one record (name, size, CRC, offset) per entry, which is the "bounded by entry count" metadata of ADR 0028. A caller that spills the plan to disk can skip the encoder and use the public record functions (`write_local_header`, `write_data_descriptor`, `write_central_header`, `write_end_records`) with the same length helpers.

## Verification

Run from `v4/`: `cargo test -p palmr-server --lib zip::`.

| Test | Proves |
|---|---|
| `regression_R023_zip_entry_name_collisions` | hostile and colliding corpus: safe, unique, hierarchical, deterministic, every payload recoverable, listing equals the input |
| `prop_zip_total_size_matches_emitted_bytes` | planned length equals the bytes counted by the sink, the length of the stream, and a strict independent parse (CRCs, payloads, offsets, fields) |
| `unit_zip_entry_name_corpus` | insta snapshot (G9) of hostile input → output |
| `it_zip_validates_with_real_unzip` | Info-ZIP `unzip` (test and extract: names, tree and per-file SHA-256), Python `zipfile` (CRC check, flags, STORE, sizes, SHA-256) and `zipinfo -v` (ZIP64 extras, stored, descriptor flag) |
| `it_zip_streams_through_a_local_header_only_reader` | `bsdtar` reading from a pipe (skipped, with a message, if `bsdtar` is absent) |
| `unit_zip_large_entry_sizes_are_exact_in_every_record` and neighbours | entries of 0 B … 1 TiB, a 5 GiB entry mid-archive, 300 small entries totalling more than 4 GiB, an offset of exactly `0xFFFFFFFF`, with no payload bytes generated (a test-only seam of the encoder skips payload) |
| `unit_zip_entry_counts_around_the_sixteen_bit_limit_are_valid`, `it_zip_entry_counts_above_sixteen_bits_are_read_by_independent_readers` | 0, 1, 2, 65 534, 65 535, 65 536 and 70 000 entries; the large ones are read by Python and `zipinfo` |

`tests/oracle.rs` is the independent oracle: a bitwise CRC-32 and a strict parser written without calling the encoder.

Reader notes. `unzip -t`/`-Z1` on macOS print non-ASCII names with `?` characters in their listing; the extracted file names are correct, so name checks use the extracted tree. `bsdtar` on macOS writes extracted names in NFD, so that test compares NFC-normalized names. Neither is an archive defect.

Not covered here: a real reader opening an archive whose central directory sits beyond 4 GiB (no multi-gigabyte fixture is created; offsets and totals beyond 4 GiB are verified through the encoder, the sizer and the oracle), and Windows or desktop extractors.

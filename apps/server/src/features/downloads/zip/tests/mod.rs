use std::io::Write;

use sha2::{Digest, Sha256};

use super::{
    archive_size, ArchiveNamer, ArchiveSummary, EntryKind, EntryShape, NodeId, ResolvedName,
    TrailerProgress, ZipEncoder, ZipEntry,
};

mod external;
mod format;
mod names;
mod oracle;

#[derive(Debug, Clone)]
pub struct Fixture {
    pub parent: Option<usize>,
    pub name: String,
    pub kind: EntryKind,
    pub payload: Vec<u8>,
}

pub fn dir(parent: Option<usize>, name: &str) -> Fixture {
    Fixture {
        parent,
        name: name.to_owned(),
        kind: EntryKind::Directory,
        payload: Vec::new(),
    }
}

pub fn file(parent: Option<usize>, name: &str, payload: &[u8]) -> Fixture {
    Fixture {
        parent,
        name: name.to_owned(),
        kind: EntryKind::File,
        payload: payload.to_vec(),
    }
}

pub struct CountingSink {
    pub bytes: Vec<u8>,
    pub counted: u64,
}

impl CountingSink {
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            counted: 0,
        }
    }
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        self.counted += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Built {
    pub paths: Vec<String>,
    pub bytes: Vec<u8>,
    pub counted: u64,
    pub summary: ArchiveSummary,
    pub planned: ArchiveSummary,
}

pub fn zip_entries(fixtures: &[Fixture]) -> (Vec<String>, Vec<ZipEntry>) {
    let mut namer = ArchiveNamer::new();
    let mut ids: Vec<NodeId> = Vec::new();
    let mut paths = Vec::new();
    let mut entries = Vec::new();
    for fixture in fixtures {
        let parent = fixture.parent.map(|index| ids[index]);
        let resolved: ResolvedName = namer.allocate(parent, &fixture.name, fixture.kind).unwrap();
        ids.push(resolved.id);
        let entry = match fixture.kind {
            EntryKind::File => ZipEntry::file(&resolved.path, fixture.payload.len() as u64),
            EntryKind::Directory => ZipEntry::directory(&resolved.path),
        }
        .unwrap();
        paths.push(resolved.path);
        entries.push(entry);
    }
    (paths, entries)
}

pub fn emit_all(
    entries: &[ZipEntry],
    payloads: &[&[u8]],
    chunk: usize,
    trailer_budget: usize,
    sink: &mut CountingSink,
) -> ArchiveSummary {
    let mut encoder = ZipEncoder::new();
    let mut buffer = Vec::new();
    for (entry, payload) in entries.iter().zip(payloads) {
        buffer.clear();
        encoder.begin_entry(entry, &mut buffer).unwrap();
        sink.write_all(&buffer).unwrap();
        for part in payload.chunks(chunk.max(1)) {
            encoder.observe_payload(part).unwrap();
            sink.write_all(part).unwrap();
        }
        buffer.clear();
        encoder.end_entry(&mut buffer).unwrap();
        sink.write_all(&buffer).unwrap();
    }
    loop {
        buffer.clear();
        let progress = encoder.emit_trailer(&mut buffer, trailer_budget).unwrap();
        sink.write_all(&buffer).unwrap();
        if let TrailerProgress::Complete(summary) = progress {
            assert_eq!(encoder.bytes_emitted(), summary.total_len);
            return summary;
        }
    }
}

pub fn build(fixtures: &[Fixture], chunk: usize) -> Built {
    let (paths, entries) = zip_entries(fixtures);
    let payloads: Vec<&[u8]> = fixtures.iter().map(|f| f.payload.as_slice()).collect();
    let mut sink = CountingSink::new();
    let summary = emit_all(&entries, &payloads, chunk, 4096, &mut sink);
    let planned = archive_size(entries.iter().map(EntryShape::from)).unwrap();
    Built {
        paths,
        bytes: sink.bytes,
        counted: sink.counted,
        summary,
        planned,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|pair| u8::from_str_radix(pair, 16).unwrap())
        .collect()
}

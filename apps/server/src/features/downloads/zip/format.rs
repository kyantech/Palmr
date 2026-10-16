use std::fmt;

use super::names::{validate_entry_path, PathViolation};
use super::EntryKind;

pub const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4b50;
pub const DATA_DESCRIPTOR_SIGNATURE: u32 = 0x0807_4b50;
pub const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
pub const ZIP64_END_SIGNATURE: u32 = 0x0606_4b50;
pub const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
pub const END_SIGNATURE: u32 = 0x0605_4b50;

pub const ZIP64_EXTRA_ID: u16 = 0x0001;
pub const VERSION_NEEDED: u16 = 45;
pub const VERSION_MADE_BY: u16 = (3 << 8) | VERSION_NEEDED;
pub const METHOD_STORE: u16 = 0;
pub const FLAG_DATA_DESCRIPTOR: u16 = 1 << 3;
pub const FLAG_UTF8_NAME: u16 = 1 << 11;
pub const DOS_TIME: u16 = 0;
pub const DOS_DATE: u16 = (1 << 5) | 1;
pub const FILE_EXTERNAL_ATTRIBUTES: u32 = 0o100_644 << 16;
pub const DIRECTORY_EXTERNAL_ATTRIBUTES: u32 = (0o040_755 << 16) | 0x10;

pub const MARKER_U16: u16 = u16::MAX;
pub const MARKER_U32: u32 = u32::MAX;

pub const LOCAL_HEADER_FIXED_LEN: u64 = 30;
pub const LOCAL_ZIP64_EXTRA_DATA_LEN: u16 = 16;
pub const LOCAL_ZIP64_EXTRA_LEN: u64 = 4 + LOCAL_ZIP64_EXTRA_DATA_LEN as u64;
pub const DATA_DESCRIPTOR_LEN: u64 = 4 + 4 + 8 + 8;
pub const CENTRAL_HEADER_FIXED_LEN: u64 = 46;
pub const CENTRAL_ZIP64_EXTRA_SIZES_DATA_LEN: u16 = 16;
pub const CENTRAL_ZIP64_EXTRA_OFFSET_DATA_LEN: u16 = 24;
pub const CENTRAL_ZIP64_EXTRA_SIZES_LEN: u64 = 4 + CENTRAL_ZIP64_EXTRA_SIZES_DATA_LEN as u64;
pub const CENTRAL_ZIP64_EXTRA_WITH_OFFSET_LEN: u64 = 4 + CENTRAL_ZIP64_EXTRA_OFFSET_DATA_LEN as u64;
pub const ZIP64_END_LEN: u64 = 56;
pub const ZIP64_LOCATOR_LEN: u64 = 20;
pub const END_LEN: u64 = 22;
pub const END_RECORDS_LEN: u64 = ZIP64_END_LEN + ZIP64_LOCATOR_LEN + END_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    UnsafeName(PathViolation),
    NameTooLong { bytes: usize },
    DirectoryHasPayload { bytes: u64 },
    EntryAlreadyOpen,
    NoOpenEntry,
    TrailerStarted,
    ArchiveFinished,
    PayloadOverrun { declared: u64 },
    PayloadShort { declared: u64, written: u64 },
    OffsetOverflow,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeName(violation) => write!(f, "unsafe entry name: {violation}"),
            Self::NameTooLong { bytes } => {
                write!(
                    f,
                    "entry name of {bytes} bytes does not fit a ZIP name field"
                )
            }
            Self::DirectoryHasPayload { bytes } => {
                write!(f, "directory entry declares {bytes} payload bytes")
            }
            Self::EntryAlreadyOpen => f.write_str("an entry is already open"),
            Self::NoOpenEntry => f.write_str("no entry is open"),
            Self::TrailerStarted => f.write_str("the central directory has been started"),
            Self::ArchiveFinished => f.write_str("the archive is complete"),
            Self::PayloadOverrun { declared } => {
                write!(f, "payload exceeds the declared {declared} bytes")
            }
            Self::PayloadShort { declared, written } => {
                write!(f, "entry declared {declared} bytes but received {written}")
            }
            Self::OffsetOverflow => f.write_str("archive offset exceeds 64 bits"),
        }
    }
}

impl std::error::Error for FormatError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    name: String,
    name_len: u16,
    kind: EntryKind,
    size: u64,
}

impl ZipEntry {
    pub fn file(path: &str, size: u64) -> Result<Self, FormatError> {
        validate_entry_path(path).map_err(FormatError::UnsafeName)?;
        Self::checked(path.to_owned(), EntryKind::File, size)
    }

    pub fn directory(path: &str) -> Result<Self, FormatError> {
        validate_entry_path(path).map_err(FormatError::UnsafeName)?;
        Self::checked(format!("{path}/"), EntryKind::Directory, 0)
    }

    fn checked(name: String, kind: EntryKind, size: u64) -> Result<Self, FormatError> {
        let name_len = u16::try_from(name.len())
            .map_err(|_| FormatError::NameTooLong { bytes: name.len() })?;
        Ok(Self {
            name,
            name_len,
            kind,
            size,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> EntryKind {
        self.kind
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

    pub const fn name_len(&self) -> u16 {
        self.name_len
    }
}

#[derive(Debug, Clone)]
pub struct Crc32(crc32fast::Hasher);

impl Crc32 {
    pub fn new() -> Self {
        Self(crc32fast::Hasher::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    pub fn finalize(&self) -> u32 {
        self.0.clone().finalize()
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn general_purpose_flags(kind: EntryKind) -> u16 {
    match kind {
        EntryKind::File => FLAG_UTF8_NAME | FLAG_DATA_DESCRIPTOR,
        EntryKind::Directory => FLAG_UTF8_NAME,
    }
}

pub const fn external_attributes(kind: EntryKind) -> u32 {
    match kind {
        EntryKind::File => FILE_EXTERNAL_ATTRIBUTES,
        EntryKind::Directory => DIRECTORY_EXTERNAL_ATTRIBUTES,
    }
}

pub const fn local_header_len(name_len: u16) -> u64 {
    LOCAL_HEADER_FIXED_LEN + name_len as u64 + LOCAL_ZIP64_EXTRA_LEN
}

pub const fn data_descriptor_len(kind: EntryKind) -> u64 {
    match kind {
        EntryKind::File => DATA_DESCRIPTOR_LEN,
        EntryKind::Directory => 0,
    }
}

pub const fn central_needs_offset_field(local_offset: u64) -> bool {
    local_offset >= MARKER_U32 as u64
}

pub const fn central_record_len(name_len: u16, local_offset: u64) -> u64 {
    let extra = if central_needs_offset_field(local_offset) {
        CENTRAL_ZIP64_EXTRA_WITH_OFFSET_LEN
    } else {
        CENTRAL_ZIP64_EXTRA_SIZES_LEN
    };
    CENTRAL_HEADER_FIXED_LEN + name_len as u64 + extra
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn write_local_header(out: &mut Vec<u8>, entry: &ZipEntry) {
    put_u32(out, LOCAL_HEADER_SIGNATURE);
    put_u16(out, VERSION_NEEDED);
    put_u16(out, general_purpose_flags(entry.kind));
    put_u16(out, METHOD_STORE);
    put_u16(out, DOS_TIME);
    put_u16(out, DOS_DATE);
    put_u32(out, 0);
    put_u32(out, MARKER_U32);
    put_u32(out, MARKER_U32);
    put_u16(out, entry.name_len());
    put_u16(out, 4 + LOCAL_ZIP64_EXTRA_DATA_LEN);
    out.extend_from_slice(entry.name.as_bytes());
    put_u16(out, ZIP64_EXTRA_ID);
    put_u16(out, LOCAL_ZIP64_EXTRA_DATA_LEN);
    put_u64(out, entry.size);
    put_u64(out, entry.size);
}

pub fn write_data_descriptor(out: &mut Vec<u8>, crc32: u32, size: u64) {
    put_u32(out, DATA_DESCRIPTOR_SIGNATURE);
    put_u32(out, crc32);
    put_u64(out, size);
    put_u64(out, size);
}

pub fn write_central_header(out: &mut Vec<u8>, entry: &ZipEntry, crc32: u32, local_offset: u64) {
    let with_offset = central_needs_offset_field(local_offset);
    let extra_data_len = if with_offset {
        CENTRAL_ZIP64_EXTRA_OFFSET_DATA_LEN
    } else {
        CENTRAL_ZIP64_EXTRA_SIZES_DATA_LEN
    };
    put_u32(out, CENTRAL_HEADER_SIGNATURE);
    put_u16(out, VERSION_MADE_BY);
    put_u16(out, VERSION_NEEDED);
    put_u16(out, general_purpose_flags(entry.kind));
    put_u16(out, METHOD_STORE);
    put_u16(out, DOS_TIME);
    put_u16(out, DOS_DATE);
    put_u32(out, crc32);
    put_u32(out, MARKER_U32);
    put_u32(out, MARKER_U32);
    put_u16(out, entry.name_len());
    put_u16(out, 4 + extra_data_len);
    put_u16(out, 0);
    put_u16(out, 0);
    put_u16(out, 0);
    put_u32(out, external_attributes(entry.kind));
    put_u32(out, saturate_u32(local_offset));
    out.extend_from_slice(entry.name.as_bytes());
    put_u16(out, ZIP64_EXTRA_ID);
    put_u16(out, extra_data_len);
    put_u64(out, entry.size);
    put_u64(out, entry.size);
    if with_offset {
        put_u64(out, local_offset);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveSummary {
    pub total_len: u64,
    pub entry_count: u64,
    pub central_directory_offset: u64,
    pub central_directory_len: u64,
}

pub fn write_end_records(out: &mut Vec<u8>, summary: &ArchiveSummary) -> Result<(), FormatError> {
    let zip64_end_offset = summary
        .central_directory_offset
        .checked_add(summary.central_directory_len)
        .ok_or(FormatError::OffsetOverflow)?;

    put_u32(out, ZIP64_END_SIGNATURE);
    put_u64(out, ZIP64_END_LEN - 12);
    put_u16(out, VERSION_MADE_BY);
    put_u16(out, VERSION_NEEDED);
    put_u32(out, 0);
    put_u32(out, 0);
    put_u64(out, summary.entry_count);
    put_u64(out, summary.entry_count);
    put_u64(out, summary.central_directory_len);
    put_u64(out, summary.central_directory_offset);

    put_u32(out, ZIP64_LOCATOR_SIGNATURE);
    put_u32(out, 0);
    put_u64(out, zip64_end_offset);
    put_u32(out, 1);

    let count16 = saturate_u16(summary.entry_count);
    put_u32(out, END_SIGNATURE);
    put_u16(out, 0);
    put_u16(out, 0);
    put_u16(out, count16);
    put_u16(out, count16);
    put_u32(out, saturate_u32(summary.central_directory_len));
    put_u32(out, saturate_u32(summary.central_directory_offset));
    put_u16(out, 0);
    Ok(())
}

fn saturate_u16(value: u64) -> u16 {
    u16::try_from(value)
        .ok()
        .filter(|fits| *fits < MARKER_U16)
        .unwrap_or(MARKER_U16)
}

fn saturate_u32(value: u64) -> u32 {
    u32::try_from(value)
        .ok()
        .filter(|fits| *fits < MARKER_U32)
        .unwrap_or(MARKER_U32)
}

#[derive(Debug)]
struct CentralEntry {
    entry: ZipEntry,
    crc32: u32,
    local_offset: u64,
}

#[derive(Debug)]
struct OpenEntry {
    entry: ZipEntry,
    local_offset: u64,
    written: u64,
    crc: Crc32,
}

#[derive(Debug)]
enum Phase {
    Entries,
    Trailer {
        next: usize,
        central_directory_offset: u64,
        central_directory_len: u64,
    },
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailerProgress {
    More,
    Complete(ArchiveSummary),
}

#[derive(Debug)]
pub struct ZipEncoder {
    offset: u64,
    central: Vec<CentralEntry>,
    open: Option<OpenEntry>,
    phase: Phase,
}

impl Default for ZipEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipEncoder {
    pub fn new() -> Self {
        Self {
            offset: 0,
            central: Vec::new(),
            open: None,
            phase: Phase::Entries,
        }
    }

    pub const fn bytes_emitted(&self) -> u64 {
        self.offset
    }

    fn advance(&mut self, bytes: u64) -> Result<(), FormatError> {
        self.offset = self
            .offset
            .checked_add(bytes)
            .ok_or(FormatError::OffsetOverflow)?;
        Ok(())
    }

    pub fn begin_entry(&mut self, entry: &ZipEntry, out: &mut Vec<u8>) -> Result<(), FormatError> {
        if !matches!(self.phase, Phase::Entries) {
            return Err(FormatError::TrailerStarted);
        }
        if self.open.is_some() {
            return Err(FormatError::EntryAlreadyOpen);
        }
        if entry.kind == EntryKind::Directory && entry.size != 0 {
            return Err(FormatError::DirectoryHasPayload { bytes: entry.size });
        }
        let local_offset = self.offset;
        let before = out.len();
        write_local_header(out, entry);
        self.advance((out.len() - before) as u64)?;
        self.open = Some(OpenEntry {
            entry: entry.clone(),
            local_offset,
            written: 0,
            crc: Crc32::new(),
        });
        Ok(())
    }

    pub fn observe_payload(&mut self, chunk: &[u8]) -> Result<(), FormatError> {
        let open = self.open.as_mut().ok_or(FormatError::NoOpenEntry)?;
        let written = open
            .written
            .checked_add(chunk.len() as u64)
            .filter(|total| *total <= open.entry.size)
            .ok_or(FormatError::PayloadOverrun {
                declared: open.entry.size,
            })?;
        open.written = written;
        open.crc.update(chunk);
        self.advance(chunk.len() as u64)
    }

    pub fn end_entry(&mut self, out: &mut Vec<u8>) -> Result<(), FormatError> {
        let open = self.open.take().ok_or(FormatError::NoOpenEntry)?;
        if open.written != open.entry.size {
            let error = FormatError::PayloadShort {
                declared: open.entry.size,
                written: open.written,
            };
            self.open = Some(open);
            return Err(error);
        }
        let crc32 = open.crc.finalize();
        if open.entry.kind == EntryKind::File {
            let before = out.len();
            write_data_descriptor(out, crc32, open.entry.size);
            self.advance((out.len() - before) as u64)?;
        }
        self.central.push(CentralEntry {
            entry: open.entry,
            crc32,
            local_offset: open.local_offset,
        });
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn assume_payload(&mut self, bytes: u64, crc32: u32) -> Result<(), FormatError> {
        let open = self.open.as_mut().ok_or(FormatError::NoOpenEntry)?;
        let written = open
            .written
            .checked_add(bytes)
            .filter(|total| *total <= open.entry.size)
            .ok_or(FormatError::PayloadOverrun {
                declared: open.entry.size,
            })?;
        open.written = written;
        open.crc = Crc32(crc32fast::Hasher::new_with_initial_len(crc32, written));
        self.advance(bytes)
    }

    pub fn emit_trailer(
        &mut self,
        out: &mut Vec<u8>,
        budget: usize,
    ) -> Result<TrailerProgress, FormatError> {
        if self.open.is_some() {
            return Err(FormatError::EntryAlreadyOpen);
        }
        if matches!(self.phase, Phase::Entries) {
            self.phase = Phase::Trailer {
                next: 0,
                central_directory_offset: self.offset,
                central_directory_len: 0,
            };
        }
        let Phase::Trailer {
            mut next,
            central_directory_offset,
            mut central_directory_len,
        } = std::mem::replace(&mut self.phase, Phase::Done)
        else {
            return Err(FormatError::ArchiveFinished);
        };
        let started = out.len();
        let budget = budget.max(1);
        while next < self.central.len() && out.len() - started < budget {
            let record = &self.central[next];
            let before = out.len();
            write_central_header(out, &record.entry, record.crc32, record.local_offset);
            let emitted = (out.len() - before) as u64;
            central_directory_len = central_directory_len
                .checked_add(emitted)
                .ok_or(FormatError::OffsetOverflow)?;
            self.advance(emitted)?;
            next += 1;
        }
        if next < self.central.len() {
            self.phase = Phase::Trailer {
                next,
                central_directory_offset,
                central_directory_len,
            };
            return Ok(TrailerProgress::More);
        }
        let mut summary = ArchiveSummary {
            total_len: 0,
            entry_count: self.central.len() as u64,
            central_directory_offset,
            central_directory_len,
        };
        let before = out.len();
        write_end_records(out, &summary)?;
        self.advance((out.len() - before) as u64)?;
        summary.total_len = self.offset;
        Ok(TrailerProgress::Complete(summary))
    }
}

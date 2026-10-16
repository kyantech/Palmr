use std::fmt;

use super::format::{
    central_record_len, data_descriptor_len, local_header_len, ArchiveSummary, ZipEntry,
    END_RECORDS_LEN,
};
use super::EntryKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeError {
    NameTooLong { bytes: usize },
    DirectoryHasPayload { bytes: u64 },
    Overflow,
}

impl fmt::Display for SizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NameTooLong { bytes } => {
                write!(
                    f,
                    "entry name of {bytes} bytes does not fit a ZIP name field"
                )
            }
            Self::DirectoryHasPayload { bytes } => {
                write!(f, "directory entry declares {bytes} payload bytes")
            }
            Self::Overflow => f.write_str("archive length exceeds 64 bits"),
        }
    }
}

impl std::error::Error for SizeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryShape {
    pub name_len: usize,
    pub kind: EntryKind,
    pub size: u64,
}

impl From<&ZipEntry> for EntryShape {
    fn from(entry: &ZipEntry) -> Self {
        Self {
            name_len: usize::from(entry.name_len()),
            kind: entry.kind(),
            size: entry.size(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ArchiveSizer {
    stream_len: u64,
    central_len: u64,
    entry_count: u64,
}

impl ArchiveSizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, shape: EntryShape) -> Result<(), SizeError> {
        let name_len = u16::try_from(shape.name_len).map_err(|_| SizeError::NameTooLong {
            bytes: shape.name_len,
        })?;
        if shape.kind == EntryKind::Directory && shape.size != 0 {
            return Err(SizeError::DirectoryHasPayload { bytes: shape.size });
        }
        let entry_stream_len = local_header_len(name_len)
            .checked_add(shape.size)
            .and_then(|len| len.checked_add(data_descriptor_len(shape.kind)))
            .ok_or(SizeError::Overflow)?;
        let next_stream_len = self
            .stream_len
            .checked_add(entry_stream_len)
            .ok_or(SizeError::Overflow)?;
        let next_central_len = self
            .central_len
            .checked_add(central_record_len(name_len, self.stream_len))
            .ok_or(SizeError::Overflow)?;
        let next_count = self.entry_count.checked_add(1).ok_or(SizeError::Overflow)?;
        self.stream_len = next_stream_len;
        self.central_len = next_central_len;
        self.entry_count = next_count;
        Ok(())
    }

    pub fn finish(&self) -> Result<ArchiveSummary, SizeError> {
        let total_len = self
            .stream_len
            .checked_add(self.central_len)
            .and_then(|len| len.checked_add(END_RECORDS_LEN))
            .ok_or(SizeError::Overflow)?;
        Ok(ArchiveSummary {
            total_len,
            entry_count: self.entry_count,
            central_directory_offset: self.stream_len,
            central_directory_len: self.central_len,
        })
    }
}

pub fn archive_size(
    shapes: impl IntoIterator<Item = EntryShape>,
) -> Result<ArchiveSummary, SizeError> {
    let mut sizer = ArchiveSizer::new();
    for shape in shapes {
        sizer.push(shape)?;
    }
    sizer.finish()
}

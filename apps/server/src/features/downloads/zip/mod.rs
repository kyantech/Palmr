mod format;
mod names;
mod size;

#[cfg(test)]
mod tests;

pub use format::{
    central_record_len, data_descriptor_len, local_header_len, write_central_header,
    write_data_descriptor, write_end_records, write_local_header, ArchiveSummary, Crc32,
    FormatError, TrailerProgress, ZipEncoder, ZipEntry, END_RECORDS_LEN,
};
pub use names::{
    resolve_names, sanitize_segment, validate_entry_path, ArchiveNamer, NameError, NodeId,
    PathViolation, ResolvedName,
};
pub use size::{archive_size, ArchiveSizer, EntryShape, SizeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    File,
    Directory,
}

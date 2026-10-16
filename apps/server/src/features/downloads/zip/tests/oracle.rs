pub fn crc32_reference(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

pub fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

pub fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

pub fn expect_u16(value: u64) -> u16 {
    if value >= 0xFFFF {
        0xFFFF
    } else {
        value as u16
    }
}

pub fn expect_u32(value: u64) -> u32 {
    if value >= 0xFFFF_FFFF {
        0xFFFF_FFFF
    } else {
        value as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralEntry {
    pub name: String,
    pub raw_name: Vec<u8>,
    pub flags: u16,
    pub crc32: u32,
    pub compressed: u64,
    pub uncompressed: u64,
    pub local_offset: u64,
    pub external_attributes: u32,
    pub extra_len: usize,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTail {
    pub entries: Vec<CentralEntry>,
    pub central_directory_offset: u64,
    pub central_directory_len: u64,
    pub zip64_end_offset: u64,
    pub total_len: u64,
}

pub fn parse_tail(tail: &[u8], tail_start: u64) -> Result<ParsedTail, String> {
    let total_len = tail_start + tail.len() as u64;
    if tail.len() < 98 {
        return Err("tail shorter than the three end records".into());
    }
    let end = tail.len() - 22;
    if le32(tail, end) != 0x0605_4b50 {
        return Err("missing end of central directory signature".into());
    }
    if le16(tail, end + 4) != 0 || le16(tail, end + 6) != 0 || le16(tail, end + 20) != 0 {
        return Err("end record disk numbers or comment length not zero".into());
    }
    let locator = end - 20;
    if le32(tail, locator) != 0x0706_4b50 {
        return Err("missing zip64 locator".into());
    }
    if le32(tail, locator + 4) != 0 || le32(tail, locator + 16) != 1 {
        return Err("locator disk fields wrong".into());
    }
    let zip64_end_offset = le64(tail, locator + 8);
    let zip64 = locator - 56;
    if tail_start + zip64 as u64 != zip64_end_offset {
        return Err(format!(
            "locator points at {zip64_end_offset} but the record sits at {}",
            tail_start + zip64 as u64
        ));
    }
    if le32(tail, zip64) != 0x0606_4b50 || le64(tail, zip64 + 4) != 44 {
        return Err("zip64 end record header wrong".into());
    }
    if le16(tail, zip64 + 12) != 0x032D || le16(tail, zip64 + 14) != 45 {
        return Err("zip64 end record versions wrong".into());
    }
    if le32(tail, zip64 + 16) != 0 || le32(tail, zip64 + 20) != 0 {
        return Err("zip64 end record disk numbers not zero".into());
    }
    let count_on_disk = le64(tail, zip64 + 24);
    let count = le64(tail, zip64 + 32);
    let cd_len = le64(tail, zip64 + 40);
    let cd_offset = le64(tail, zip64 + 48);
    if count_on_disk != count {
        return Err("zip64 entry counts differ".into());
    }
    if cd_offset + cd_len != zip64_end_offset {
        return Err("central directory does not end where the zip64 record starts".into());
    }
    if le16(tail, end + 8) != expect_u16(count)
        || le16(tail, end + 10) != expect_u16(count)
        || le32(tail, end + 12) != expect_u32(cd_len)
        || le32(tail, end + 16) != expect_u32(cd_offset)
    {
        return Err("conventional end record compatibility values wrong".into());
    }
    if cd_offset < tail_start {
        return Err("tail does not cover the central directory".into());
    }
    let mut at = (cd_offset - tail_start) as usize;
    let cd_end = at + cd_len as usize;
    let mut entries = Vec::new();
    while at < cd_end {
        let (entry, next) = parse_central(tail, at)?;
        entries.push(entry);
        at = next;
    }
    if at != cd_end {
        return Err("central directory records overran their declared length".into());
    }
    if entries.len() as u64 != count {
        return Err(format!(
            "end records declare {count} entries, central directory holds {}",
            entries.len()
        ));
    }
    Ok(ParsedTail {
        entries,
        central_directory_offset: cd_offset,
        central_directory_len: cd_len,
        zip64_end_offset,
        total_len,
    })
}

fn parse_central(bytes: &[u8], at: usize) -> Result<(CentralEntry, usize), String> {
    if le32(bytes, at) != 0x0201_4b50 {
        return Err(format!("bad central signature at {at}"));
    }
    if le16(bytes, at + 4) != 0x032D || le16(bytes, at + 6) != 45 {
        return Err("central versions wrong".into());
    }
    let flags = le16(bytes, at + 8);
    if le16(bytes, at + 10) != 0 {
        return Err("method is not STORE".into());
    }
    if le16(bytes, at + 12) != 0 || le16(bytes, at + 14) != 0x0021 {
        return Err("dos time/date not the fixed epoch".into());
    }
    let crc32 = le32(bytes, at + 16);
    let comp32 = le32(bytes, at + 20);
    let uncomp32 = le32(bytes, at + 24);
    let name_len = le16(bytes, at + 28) as usize;
    let extra_len = le16(bytes, at + 30) as usize;
    if le16(bytes, at + 32) != 0 || le16(bytes, at + 34) != 0 || le16(bytes, at + 36) != 0 {
        return Err("comment, disk start or internal attributes not zero".into());
    }
    let external_attributes = le32(bytes, at + 38);
    let offset32 = le32(bytes, at + 42);
    let name_at = at + 46;
    let raw_name = bytes[name_at..name_at + name_len].to_vec();
    let extra_at = name_at + name_len;
    let extra = &bytes[extra_at..extra_at + extra_len];
    if comp32 != 0xFFFF_FFFF || uncomp32 != 0xFFFF_FFFF {
        return Err("sizes are not routed through zip64".into());
    }
    if le16(extra, 0) != 1 {
        return Err("central extra is not the zip64 field".into());
    }
    let data_len = le16(extra, 2) as usize;
    if extra.len() != 4 + data_len {
        return Err("central extra length mismatch".into());
    }
    let uncompressed = le64(extra, 4);
    let compressed = le64(extra, 12);
    let local_offset = if offset32 == 0xFFFF_FFFF {
        if data_len != 24 {
            return Err("saturated offset without an offset field".into());
        }
        let offset = le64(extra, 20);
        if offset < 0xFFFF_FFFF {
            return Err("offset field present for an offset that fits".into());
        }
        offset
    } else {
        if data_len != 16 {
            return Err("unneeded offset field".into());
        }
        u64::from(offset32)
    };
    let name = String::from_utf8(raw_name.clone()).map_err(|e| e.to_string())?;
    let is_directory = name.ends_with('/');
    let expected_flags = if is_directory { 0x0800 } else { 0x0808 };
    if flags != expected_flags {
        return Err(format!("flags {flags:#06x} != {expected_flags:#06x}"));
    }
    let expected_attributes: u32 = if is_directory {
        (0o040_755 << 16) | 0x10
    } else {
        0o100_644 << 16
    };
    if external_attributes != expected_attributes {
        return Err("external attributes wrong".into());
    }
    Ok((
        CentralEntry {
            name,
            raw_name,
            flags,
            crc32,
            compressed,
            uncompressed,
            local_offset,
            external_attributes,
            extra_len,
            is_directory,
        },
        extra_at + extra_len,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArchive {
    pub tail: ParsedTail,
    pub payloads: Vec<Vec<u8>>,
}

pub fn parse_archive(bytes: &[u8]) -> Result<ParsedArchive, String> {
    let tail = parse_tail(bytes, 0)?;
    let mut cursor = 0u64;
    let mut payloads = Vec::new();
    for entry in &tail.entries {
        if entry.local_offset != cursor {
            return Err(format!(
                "{}: local header at {} but the previous record ended at {cursor}",
                entry.name, entry.local_offset
            ));
        }
        let at = cursor as usize;
        if le32(bytes, at) != 0x0403_4b50 {
            return Err(format!("{}: bad local signature", entry.name));
        }
        if le16(bytes, at + 4) != 45 {
            return Err("local version wrong".into());
        }
        if le16(bytes, at + 6) != entry.flags {
            return Err("local flags differ from central".into());
        }
        if le16(bytes, at + 8) != 0 || le16(bytes, at + 10) != 0 || le16(bytes, at + 12) != 0x0021 {
            return Err("local method/time/date wrong".into());
        }
        if le32(bytes, at + 14) != 0 {
            return Err("local crc must be zero while a descriptor follows".into());
        }
        if le32(bytes, at + 18) != 0xFFFF_FFFF || le32(bytes, at + 22) != 0xFFFF_FFFF {
            return Err("local sizes are not routed through zip64".into());
        }
        let name_len = le16(bytes, at + 26) as usize;
        let extra_len = le16(bytes, at + 28) as usize;
        let name_at = at + 30;
        if bytes[name_at..name_at + name_len] != entry.raw_name[..] {
            return Err(format!("{}: local name differs from central", entry.name));
        }
        let extra = &bytes[name_at + name_len..name_at + name_len + extra_len];
        if extra_len != 20 || le16(extra, 0) != 1 || le16(extra, 2) != 16 {
            return Err("local zip64 extra malformed".into());
        }
        if le64(extra, 4) != entry.uncompressed || le64(extra, 12) != entry.compressed {
            return Err("local zip64 sizes differ from central".into());
        }
        if entry.compressed != entry.uncompressed {
            return Err("STORE entry with differing sizes".into());
        }
        let data_at = name_at + name_len + extra_len;
        let data_end = data_at + entry.uncompressed as usize;
        let payload = bytes[data_at..data_end].to_vec();
        if crc32_reference(&payload) != entry.crc32 {
            return Err(format!(
                "{}: central crc does not match payload",
                entry.name
            ));
        }
        cursor = data_end as u64;
        if entry.is_directory {
            if entry.uncompressed != 0 || entry.crc32 != 0 {
                return Err("directory carries data".into());
            }
        } else {
            let d = data_end;
            if le32(bytes, d) != 0x0807_4b50
                || le32(bytes, d + 4) != entry.crc32
                || le64(bytes, d + 8) != entry.compressed
                || le64(bytes, d + 16) != entry.uncompressed
            {
                return Err(format!("{}: data descriptor wrong", entry.name));
            }
            cursor += 24;
        }
        payloads.push(payload);
    }
    if cursor != tail.central_directory_offset {
        return Err(format!(
            "entries end at {cursor} but the central directory starts at {}",
            tail.central_directory_offset
        ));
    }
    Ok(ParsedArchive { tail, payloads })
}

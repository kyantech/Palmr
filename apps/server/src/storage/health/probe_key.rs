use std::fmt;

use uuid::{Uuid, Variant, Version};

pub(in crate::storage) const PROBE_PREFIX: &str = "_palmr/probe/";
pub(in crate::storage) const PROBE_DIRS: [&str; 2] = ["_palmr", "probe"];

const OID_LEN: usize = 32;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::storage) struct ProbeKey {
    text: String,
}

impl ProbeKey {
    pub(in crate::storage) fn generate() -> Self {
        Self::from_oid(&Uuid::now_v7().simple().to_string())
    }

    pub(in crate::storage) fn from_listed(text: &str) -> Option<Self> {
        let oid = text.strip_prefix(PROBE_PREFIX)?;
        generated_oid(oid).then(|| Self::from_oid(oid))
    }

    pub(in crate::storage) fn from_listed_name(name: &str) -> Option<Self> {
        generated_oid(name).then(|| Self::from_oid(name))
    }

    pub(in crate::storage) fn as_str(&self) -> &str {
        &self.text
    }

    pub(in crate::storage) fn oid(&self) -> &str {
        &self.text[PROBE_PREFIX.len()..]
    }

    pub(in crate::storage) fn seed(&self) -> u64 {
        u64::from_str_radix(&self.oid()[OID_LEN - 16..], 16).unwrap_or_default()
    }

    fn from_oid(oid: &str) -> Self {
        Self {
            text: format!("{PROBE_PREFIX}{oid}"),
        }
    }
}

impl fmt::Debug for ProbeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProbeKey(<probe>)")
    }
}

fn generated_oid(text: &str) -> bool {
    let lowercase_hex = text.len() == OID_LEN
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    lowercase_hex
        && Uuid::try_parse(text).is_ok_and(|uuid| {
            uuid.get_version() == Some(Version::SortRand) && uuid.get_variant() == Variant::RFC4122
        })
}

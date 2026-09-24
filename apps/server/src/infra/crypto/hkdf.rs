use std::fmt;

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

use super::instance_key::{InstanceKey, INSTANCE_KEY_LEN};
use crate::domain::secret::REDACTED;

pub const SUBKEY_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SealPurpose {
    Totp,
    Smtp,
    Idp,
    Oidc,
    OutboxToken,
    InviteToken,
    IdempotencyReplay,
}

impl SealPurpose {
    pub const ALL: [Self; 7] = [
        Self::Totp,
        Self::Smtp,
        Self::Idp,
        Self::Oidc,
        Self::OutboxToken,
        Self::InviteToken,
        Self::IdempotencyReplay,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Totp => "palmr:v1:totp",
            Self::Smtp => "palmr:v1:smtp",
            Self::Idp => "palmr:v1:idp",
            Self::Oidc => "palmr:v1:oidc",
            Self::OutboxToken => "palmr:v1:outbox-token",
            Self::InviteToken => "palmr:v1:invite-token",
            Self::IdempotencyReplay => "palmr:v1:idempotency-replay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacPurpose {
    IdempotencyRequest,
    Cursor,
    ArchiveTicket,
}

impl MacPurpose {
    pub const ALL: [Self; 3] = [Self::IdempotencyRequest, Self::Cursor, Self::ArchiveTicket];

    pub const fn label(self) -> &'static str {
        match self {
            Self::IdempotencyRequest => "palmr:v1:idempotency-request",
            Self::Cursor => "palmr:v1:cursor",
            Self::ArchiveTicket => "palmr:v1:archive-ticket",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyPurpose {
    Seal(SealPurpose),
    Mac(MacPurpose),
}

impl KeyPurpose {
    pub const ALL: [Self; 10] = [
        Self::Seal(SealPurpose::Totp),
        Self::Seal(SealPurpose::Smtp),
        Self::Seal(SealPurpose::Idp),
        Self::Seal(SealPurpose::Oidc),
        Self::Seal(SealPurpose::OutboxToken),
        Self::Seal(SealPurpose::InviteToken),
        Self::Seal(SealPurpose::IdempotencyReplay),
        Self::Mac(MacPurpose::IdempotencyRequest),
        Self::Mac(MacPurpose::Cursor),
        Self::Mac(MacPurpose::ArchiveTicket),
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Seal(purpose) => purpose.label(),
            Self::Mac(purpose) => purpose.label(),
        }
    }
}

pub(super) struct SubKey([u8; SUBKEY_LEN]);

impl SubKey {
    pub(super) const fn expose_secret(&self) -> &[u8; SUBKEY_LEN] {
        &self.0
    }
}

impl Drop for SubKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SubKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SubKey({REDACTED})")
    }
}

pub struct KeyRing {
    seal: [SubKey; SealPurpose::ALL.len()],
    mac: [SubKey; MacPurpose::ALL.len()],
}

impl KeyRing {
    pub fn new(instance_key: &InstanceKey) -> Self {
        Self::from_root(instance_key.expose_secret())
    }

    pub(super) fn from_root(root: &[u8; INSTANCE_KEY_LEN]) -> Self {
        let hkdf = Hkdf::<Sha256>::new(None, root);
        Self {
            seal: SealPurpose::ALL.map(|purpose| derive(&hkdf, purpose.label())),
            mac: MacPurpose::ALL.map(|purpose| derive(&hkdf, purpose.label())),
        }
    }

    pub(super) fn seal_key(&self, purpose: SealPurpose) -> &SubKey {
        &self.seal[purpose as usize]
    }

    pub(super) fn mac_key(&self, purpose: MacPurpose) -> &SubKey {
        &self.mac[purpose as usize]
    }
}

impl fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyRing({REDACTED})")
    }
}

fn derive(hkdf: &Hkdf<Sha256>, label: &str) -> SubKey {
    let mut okm = [0_u8; SUBKEY_LEN];
    if hkdf.expand(label.as_bytes(), &mut okm).is_err() {
        unreachable!("a {SUBKEY_LEN}-byte HKDF-SHA256 output is always within the expand limit");
    }
    SubKey(okm)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tempfile::TempDir;

    use super::{KeyPurpose, KeyRing, MacPurpose, SealPurpose, SubKey, SUBKEY_LEN};
    use crate::infra::crypto::instance_key::{InstanceKey, INSTANCE_KEY_LEN};

    const ROOT: [u8; INSTANCE_KEY_LEN] = *b"hkdf-root-sentinel-key-bytes-042";

    fn subkey(ring: &KeyRing, purpose: KeyPurpose) -> &SubKey {
        match purpose {
            KeyPurpose::Seal(purpose) => ring.seal_key(purpose),
            KeyPurpose::Mac(purpose) => ring.mac_key(purpose),
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn unit_hkdf_label_registry_is_closed() {
        let labels: Vec<&str> = KeyPurpose::ALL.iter().map(|p| p.label()).collect();

        assert_eq!(
            labels,
            [
                "palmr:v1:totp",
                "palmr:v1:smtp",
                "palmr:v1:idp",
                "palmr:v1:oidc",
                "palmr:v1:outbox-token",
                "palmr:v1:invite-token",
                "palmr:v1:idempotency-replay",
                "palmr:v1:idempotency-request",
                "palmr:v1:cursor",
                "palmr:v1:archive-ticket",
            ]
        );
        assert_eq!(
            SealPurpose::ALL.len() + MacPurpose::ALL.len(),
            KeyPurpose::ALL.len()
        );
        for (index, purpose) in SealPurpose::ALL.into_iter().enumerate() {
            assert_eq!(purpose as usize, index);
        }
        for (index, purpose) in MacPurpose::ALL.into_iter().enumerate() {
            assert_eq!(purpose as usize, index);
        }
    }

    #[test]
    fn unit_hkdf_every_purpose_derives_deterministically() {
        let first = KeyRing::from_root(&ROOT);
        let second = KeyRing::from_root(&ROOT);

        for purpose in KeyPurpose::ALL {
            let key = subkey(&first, purpose).expose_secret();
            assert_eq!(key.len(), SUBKEY_LEN);
            assert_eq!(key, subkey(&second, purpose).expose_secret(), "{purpose:?}");
            assert_ne!(key, &ROOT, "{purpose:?}");
        }
    }

    #[test]
    fn unit_hkdf_purposes_derive_distinct_keys() {
        let ring = KeyRing::from_root(&ROOT);
        let other_root = KeyRing::from_root(&[0x5a; INSTANCE_KEY_LEN]);

        let keys: HashSet<[u8; SUBKEY_LEN]> = [&ring, &other_root]
            .into_iter()
            .flat_map(|source| {
                KeyPurpose::ALL.map(|purpose| *subkey(source, purpose).expose_secret())
            })
            .collect();

        assert_eq!(keys.len(), 2 * KeyPurpose::ALL.len());
    }

    #[test]
    fn unit_hkdf_derivation_matches_known_answer() {
        let root: [u8; INSTANCE_KEY_LEN] = std::array::from_fn(|index| index as u8);
        let ring = KeyRing::from_root(&root);

        assert_eq!(
            hex(ring.seal_key(SealPurpose::Totp).expose_secret()),
            "0360472b18bf25d19c3151027c966e5a8439c947ed39f30dd0703196dca4ac1c"
        );
        assert_eq!(
            hex(ring.mac_key(MacPurpose::ArchiveTicket).expose_secret()),
            "8c0576c3b68f77258da3bf9fa8a9ede817846cdd112f6c3ce9f9bd939f67031e"
        );
    }

    #[test]
    fn unit_hkdf_instance_key_is_the_root() {
        let dir = TempDir::new().unwrap();
        let (instance_key, _) = InstanceKey::load_or_create(dir.path()).unwrap();

        let ring = KeyRing::new(&instance_key);
        let expected = KeyRing::from_root(instance_key.expose_secret());

        for purpose in KeyPurpose::ALL {
            assert_eq!(
                subkey(&ring, purpose).expose_secret(),
                subkey(&expected, purpose).expose_secret()
            );
        }
    }

    #[test]
    fn unit_hkdf_key_material_never_formatted() {
        let ring = KeyRing::from_root(&ROOT);
        let root_text = String::from_utf8(ROOT.to_vec()).unwrap();

        for text in [
            format!("{ring:?}"),
            format!("{ring:#?}"),
            format!("{:?}", ring.seal_key(SealPurpose::Smtp)),
        ] {
            assert!(text.contains("<redacted>"), "{text}");
            assert!(!text.contains(&root_text));
            for purpose in KeyPurpose::ALL {
                assert!(!text.contains(&hex(subkey(&ring, purpose).expose_secret())));
            }
        }
    }
}

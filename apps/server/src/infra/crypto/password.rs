use std::fmt;
use std::hint::black_box;

use argon2::password_hash::{
    self, PasswordHash, PasswordHasher, PasswordVerifier, Salt, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};

use super::{fill_random, CryptoError};
use crate::domain::secret::{Secret, REDACTED};

pub const ARGON2_ALGORITHM: Algorithm = Algorithm::Argon2id;
pub const ARGON2_VERSION: Version = Version::V0x13;
pub const ARGON2_M_COST_KIB: u32 = 19_456;
pub const ARGON2_T_COST: u32 = 2;
pub const ARGON2_P_COST: u32 = 1;
pub const ARGON2_SALT_LEN: usize = 16;
pub const ARGON2_OUTPUT_LEN: usize = 32;

const PARAMS: Params = match Params::new(
    ARGON2_M_COST_KIB,
    ARGON2_T_COST,
    ARGON2_P_COST,
    Some(ARGON2_OUTPUT_LEN),
) {
    Ok(params) => params,
    Err(_) => panic!("the Argon2id policy constants are valid parameters"),
};

const DUMMY_PROBE: &[u8] = b"\0palmr-timing-equalization-probe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordVerification {
    Mismatch,
    Verified { needs_rehash: bool },
}

pub fn hash_password(password: &[u8]) -> Result<Secret<String>, CryptoError> {
    let mut salt = [0_u8; ARGON2_SALT_LEN];
    fill_random(&mut salt)?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| CryptoError::PasswordHashing)?;
    hasher()
        .hash_password(password, &salt)
        .map(|hash| Secret::new(hash.to_string()))
        .map_err(|_| CryptoError::PasswordHashing)
}

pub fn verify_password(password: &[u8], stored: &str) -> Result<PasswordVerification, CryptoError> {
    let parsed = PasswordHash::new(stored).map_err(|_| CryptoError::MalformedPasswordHash)?;
    if parsed.salt.is_none() || parsed.hash.is_none() {
        return Err(CryptoError::MalformedPasswordHash);
    }
    match hasher().verify_password(password, &parsed) {
        Ok(()) => Ok(PasswordVerification::Verified {
            needs_rehash: needs_rehash(&parsed),
        }),
        Err(password_hash::Error::Password) => Ok(PasswordVerification::Mismatch),
        Err(_) => Err(CryptoError::MalformedPasswordHash),
    }
}

pub struct DummyPasswordHash(Secret<String>);

impl DummyPasswordHash {
    pub fn new() -> Result<Self, CryptoError> {
        hash_password(DUMMY_PROBE).map(Self)
    }

    pub fn verify(&self, password: &[u8]) -> PasswordVerification {
        let _ = black_box(verify_password(password, self.0.expose_secret()));
        PasswordVerification::Mismatch
    }
}

impl fmt::Debug for DummyPasswordHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DummyPasswordHash({REDACTED})")
    }
}

fn hasher() -> Argon2<'static> {
    Argon2::new(ARGON2_ALGORITHM, ARGON2_VERSION, PARAMS)
}

fn needs_rehash(hash: &PasswordHash<'_>) -> bool {
    let current_algorithm = hash.algorithm == ARGON2_ALGORITHM.ident();
    let current_version = hash.version == Some(u32::from(ARGON2_VERSION));
    let current_params = Params::try_from(hash).is_ok_and(|params| {
        params.m_cost() == ARGON2_M_COST_KIB
            && params.t_cost() == ARGON2_T_COST
            && params.p_cost() == ARGON2_P_COST
            && params.keyid().is_empty()
            && params.data().is_empty()
    });
    let current_salt = hash.salt.is_some_and(|salt| {
        let mut buffer = [0_u8; Salt::MAX_LENGTH];
        salt.decode_b64(&mut buffer)
            .is_ok_and(|decoded| decoded.len() == ARGON2_SALT_LEN)
    });
    let current_output = hash
        .hash
        .is_some_and(|output| output.len() == ARGON2_OUTPUT_LEN);

    !(current_algorithm && current_version && current_params && current_salt && current_output)
}

#[cfg(test)]
mod tests {
    use argon2::password_hash::{PasswordHash, PasswordHasher, SaltString};
    use argon2::{Algorithm, Argon2, Params, Version};
    use rstest::rstest;

    use super::{
        hash_password, verify_password, DummyPasswordHash, PasswordVerification, ARGON2_ALGORITHM,
        ARGON2_M_COST_KIB, ARGON2_OUTPUT_LEN, ARGON2_P_COST, ARGON2_SALT_LEN, ARGON2_T_COST,
        ARGON2_VERSION, DUMMY_PROBE,
    };
    use crate::infra::crypto::CryptoError;

    const PASSWORD: &[u8] = b"correct horse battery staple";
    const CURRENT: PasswordVerification = PasswordVerification::Verified {
        needs_rehash: false,
    };
    const STALE: PasswordVerification = PasswordVerification::Verified { needs_rehash: true };

    struct Profile {
        algorithm: Algorithm,
        version: Version,
        m_cost: u32,
        t_cost: u32,
        p_cost: u32,
        salt_len: usize,
        output_len: usize,
    }

    const fn current_profile() -> Profile {
        Profile {
            algorithm: Algorithm::Argon2id,
            version: Version::V0x13,
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 1,
            salt_len: 16,
            output_len: 32,
        }
    }

    fn hash_with(profile: &Profile) -> String {
        let params = Params::new(
            profile.m_cost,
            profile.t_cost,
            profile.p_cost,
            Some(profile.output_len),
        )
        .unwrap();
        let salt_bytes: Vec<u8> = (0..profile.salt_len)
            .map(|index| index as u8 ^ 0xa5)
            .collect();
        let salt = SaltString::encode_b64(&salt_bytes).unwrap();
        Argon2::new(profile.algorithm, profile.version, params)
            .hash_password(PASSWORD, &salt)
            .unwrap()
            .to_string()
    }

    #[test]
    fn unit_argon2id_params_fixed() {
        assert_eq!(ARGON2_ALGORITHM, Algorithm::Argon2id);
        assert_eq!(ARGON2_VERSION, Version::V0x13);
        assert_eq!(u32::from(ARGON2_VERSION), 19);
        assert_eq!(ARGON2_M_COST_KIB, 19_456);
        assert_eq!(ARGON2_T_COST, 2);
        assert_eq!(ARGON2_P_COST, 1);
        assert_eq!(ARGON2_SALT_LEN, 16);
        assert_eq!(ARGON2_OUTPUT_LEN, 32);

        let first = hash_password(PASSWORD).unwrap();
        let second = hash_password(PASSWORD).unwrap();
        let phc = first.expose_secret();

        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        let parsed = PasswordHash::new(phc).unwrap();
        let mut salt = [0_u8; 64];
        assert_eq!(
            parsed.salt.unwrap().decode_b64(&mut salt).unwrap().len(),
            16
        );
        assert_eq!(parsed.hash.unwrap().len(), 32);
        assert_ne!(first.expose_secret(), second.expose_secret());
        assert_eq!(verify_password(PASSWORD, phc).unwrap(), CURRENT);
        assert_eq!(
            verify_password(PASSWORD, &hash_with(&current_profile())).unwrap(),
            CURRENT
        );
    }

    #[test]
    fn unit_argon2id_verifies_correct_and_rejects_wrong_password() {
        let stored = hash_password(PASSWORD).unwrap();
        let stored = stored.expose_secret();

        assert_eq!(verify_password(PASSWORD, stored).unwrap(), CURRENT);
        for wrong in [
            b"correct horse battery stapl".as_slice(),
            b"correct horse battery staple ",
            b"Correct horse battery staple",
            b"",
        ] {
            assert_eq!(
                verify_password(wrong, stored).unwrap(),
                PasswordVerification::Mismatch
            );
        }
    }

    #[rstest]
    #[case::empty("")]
    #[case::not_phc("correct horse battery staple")]
    #[case::bcrypt("$2b$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy")]
    #[case::scrypt("$scrypt$ln=15,r=8,p=1$c2FsdHNhbHRzYWx0$c2FsdHNhbHRzYWx0c2FsdHNhbHRzYWx0c2FsdA")]
    #[case::missing_salt_and_hash("$argon2id$v=19$m=19456,t=2,p=1")]
    #[case::missing_hash("$argon2id$v=19$m=19456,t=2,p=1$pYSnpqGgoaKjrK2up6ipqg")]
    #[case::invalid_cost(
        "$argon2id$v=19$m=1,t=2,p=1$pYSnpqGgoaKjrK2up6ipqg$c2FsdHNhbHRzYWx0c2FsdHNhbHRzYWx0c2FsdA"
    )]
    #[case::unknown_param("$argon2id$v=19$m=19456,t=2,p=1,x=9$pYSnpqGgoaKjrK2up6ipqg$c2FsdHNhbHRzYWx0c2FsdHNhbHRzYWx0c2FsdA")]
    #[case::unknown_version("$argon2id$v=18$m=19456,t=2,p=1$pYSnpqGgoaKjrK2up6ipqg$c2FsdHNhbHRzYWx0c2FsdHNhbHRzYWx0c2FsdA")]
    #[case::invalid_base64("$argon2id$v=19$m=19456,t=2,p=1$pYSnpqGgoaKjrK2up6ipqg$!!!!")]
    fn unit_argon2id_malformed_phc_rejected(#[case] stored: &str) {
        assert_eq!(
            verify_password(PASSWORD, stored).unwrap_err(),
            CryptoError::MalformedPasswordHash
        );
    }

    #[rstest]
    #[case::argon2i(Profile { algorithm: Algorithm::Argon2i, ..current_profile() })]
    #[case::argon2d(Profile { algorithm: Algorithm::Argon2d, ..current_profile() })]
    #[case::version_16(Profile { version: Version::V0x10, ..current_profile() })]
    #[case::lower_memory(Profile { m_cost: 12_288, ..current_profile() })]
    #[case::higher_memory(Profile { m_cost: 19_457, ..current_profile() })]
    #[case::fewer_passes(Profile { t_cost: 1, ..current_profile() })]
    #[case::more_passes(Profile { t_cost: 3, ..current_profile() })]
    #[case::more_lanes(Profile { p_cost: 2, ..current_profile() })]
    #[case::short_salt(Profile { salt_len: 8, ..current_profile() })]
    #[case::long_salt(Profile { salt_len: 32, ..current_profile() })]
    #[case::short_output(Profile { output_len: 16, ..current_profile() })]
    #[case::long_output(Profile { output_len: 64, ..current_profile() })]
    fn unit_argon2id_rehash_on_param_drift(#[case] profile: Profile) {
        let stale = hash_with(&profile);

        assert_eq!(verify_password(PASSWORD, &stale).unwrap(), STALE, "{stale}");
        assert_eq!(
            verify_password(b"wrong password", &stale).unwrap(),
            PasswordVerification::Mismatch
        );
    }

    #[test]
    fn unit_argon2id_dummy_verification_uses_real_verifier() {
        let dummy = DummyPasswordHash::new().unwrap();
        let phc = dummy.0.expose_secret();

        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        assert_eq!(verify_password(DUMMY_PROBE, phc).unwrap(), CURRENT);
        assert_eq!(
            verify_password(PASSWORD, phc).unwrap(),
            PasswordVerification::Mismatch
        );
        for password in [PASSWORD, DUMMY_PROBE, b""] {
            assert_eq!(dummy.verify(password), PasswordVerification::Mismatch);
        }
        assert_eq!(dummy.0.expose_secret(), phc);
    }

    #[test]
    fn unit_argon2id_hashes_never_formatted() {
        let stored = hash_password(PASSWORD).unwrap();
        let dummy = DummyPasswordHash::new().unwrap();
        let password = String::from_utf8(PASSWORD.to_vec()).unwrap();

        for text in [
            format!("{stored:?}"),
            format!("{stored}"),
            format!("{dummy:?}"),
            format!("{:?}", verify_password(PASSWORD, stored.expose_secret())),
            CryptoError::MalformedPasswordHash.to_string(),
        ] {
            assert!(!text.contains("$argon2"), "{text}");
            assert!(!text.contains(&password), "{text}");
            assert!(!text.contains(dummy.0.expose_secret().as_str()));
        }
    }
}

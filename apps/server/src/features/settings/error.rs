use std::fmt;

use crate::domain::time::InvalidTimestamp;
use crate::infra::crypto::CryptoError;
use crate::infra::db::DbError;

pub const STARTUP_SETTINGS_DECRYPT_FAILED: &str = "STARTUP_SETTINGS_DECRYPT_FAILED";
pub const STARTUP_SETTINGS_INVALID: &str = "STARTUP_SETTINGS_INVALID";

#[derive(Debug)]
pub enum SettingsError {
    Db(DbError),
    UnknownKey {
        key: String,
    },
    GroupMismatch {
        key: String,
        stored: String,
        expected: &'static str,
    },
    ValueTypeMismatch {
        key: String,
        stored: String,
        expected: &'static str,
    },
    MalformedValue {
        key: String,
    },
    Decrypt {
        key: String,
    },
    Crypto(CryptoError),
    Time(InvalidTimestamp),
}

impl SettingsError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Decrypt { .. } => STARTUP_SETTINGS_DECRYPT_FAILED,
            _ => STARTUP_SETTINGS_INVALID,
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Db(error) => error.kind().as_str(),
            Self::UnknownKey { .. } => "settings_unknown_key",
            Self::GroupMismatch { .. } => "settings_group_mismatch",
            Self::ValueTypeMismatch { .. } => "settings_value_type_mismatch",
            Self::MalformedValue { .. } => "settings_malformed_value",
            Self::Decrypt { .. } => "settings_decrypt_failed",
            Self::Crypto(_) => "settings_crypto_failed",
            Self::Time(_) => "settings_time_out_of_range",
        }
    }
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(error) => write!(f, "the settings store could not be read: {error}"),
            Self::UnknownKey { key } => {
                write!(f, "the settings store holds unrecognized key {key:?}")
            }
            Self::GroupMismatch {
                key,
                stored,
                expected,
            } => write!(
                f,
                "the settings key {key:?} is stored in group {stored:?} but belongs to {expected:?}"
            ),
            Self::ValueTypeMismatch {
                key,
                stored,
                expected,
            } => write!(
                f,
                "the settings key {key:?} is stored with value type {stored:?} but must be {expected:?}"
            ),
            Self::MalformedValue { key } => {
                write!(f, "the settings key {key:?} holds a value that cannot be decoded")
            }
            Self::Decrypt { key } => write!(
                f,
                "{STARTUP_SETTINGS_DECRYPT_FAILED}: the encrypted settings value {key:?} could not be decrypted. The most likely cause is that /data/instance.key does not match this palmr.db, for example a database restored without its key. Restore the original instance.key from the same backup as the database; if it is lost, the encrypted value must be re-entered"
            ),
            Self::Crypto(error) => write!(f, "a settings secret could not be sealed: {error}"),
            Self::Time(error) => write!(f, "the server clock is outside the supported range: {error}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            Self::Crypto(error) => Some(error),
            Self::Time(error) => Some(error),
            Self::UnknownKey { .. }
            | Self::GroupMismatch { .. }
            | Self::ValueTypeMismatch { .. }
            | Self::MalformedValue { .. }
            | Self::Decrypt { .. } => None,
        }
    }
}

impl From<DbError> for SettingsError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for SettingsError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::from(error))
    }
}

impl From<CryptoError> for SettingsError {
    fn from(error: CryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<InvalidTimestamp> for SettingsError {
    fn from(error: InvalidTimestamp) -> Self {
        Self::Time(error)
    }
}

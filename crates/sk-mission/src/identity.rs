use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// Length of a canonical ULID mission identity.
pub const MISSION_ID_LENGTH: usize = 26;

/// Length of the canonical short mission disambiguator (`mission_id[..8]`).
pub const MID8_LENGTH: usize = 8;

/// Crockford base32 alphabet, as required by the ULID specification.
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Why a string is not a canonical ULID mission identity.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum MissionIdError {
    /// The value is not exactly 26 ASCII characters.
    #[error("expected exactly {MISSION_ID_LENGTH} characters, found {len}")]
    InvalidLength { len: usize },

    /// The value contains a character outside Crockford base32 or is lowercase.
    #[error("character {character:?} at index {index} is not uppercase Crockford base32")]
    InvalidCharacter { index: usize, character: char },

    /// The first character would overflow ULID's 128-bit timestamp space.
    #[error("leading character {character:?} is out of range for a ULID")]
    OutOfRange { character: char },
}

/// A canonical mission identity: an uppercase 26-character ULID.
///
/// The directory name is a *slug*; it is display/navigation metadata and may
/// drift from the stored slug (real fixtures include legacy slugs without the
/// numeric prefix). Only this value is identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct MissionId(String);

impl MissionId {
    /// Return the canonical 26-character identity string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the canonical 8-character disambiguator (`mission_id[..8]`).
    ///
    /// `MID8` is derived here exactly once per value, never recomputed by
    /// callers from an arbitrary string prefix.
    pub fn mid8(&self) -> &str {
        &self.0[..MID8_LENGTH]
    }

    /// Validate `value` without constructing a [`MissionId`].
    pub fn validate(value: &str) -> Result<(), MissionIdError> {
        if value.len() != MISSION_ID_LENGTH || !value.is_ascii() {
            return Err(MissionIdError::InvalidLength { len: value.len() });
        }
        for (index, byte) in value.bytes().enumerate() {
            if !CROCKFORD_ALPHABET.contains(&byte) {
                return Err(MissionIdError::InvalidCharacter {
                    index,
                    character: byte as char,
                });
            }
            if index == 0 && byte > b'7' {
                return Err(MissionIdError::OutOfRange {
                    character: byte as char,
                });
            }
        }
        Ok(())
    }
}

impl fmt::Display for MissionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for MissionId {
    type Err = MissionIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::validate(value)?;
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for MissionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

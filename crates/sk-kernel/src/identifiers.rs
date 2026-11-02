use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::KernelError;

/// A validated, single-segment mission slug.
///
/// The grammar is the upstream kernel safe-segment grammar:
/// `^[A-Za-z0-9][A-Za-z0-9._-]*$`, with `..` rejected anywhere and no leading
/// dot. That admits real mission directory handles (numeric prefixes,
/// `<slug>-<mid8>`, bare ULIDs) while making the slug safe to join as exactly
/// one path component.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MissionSlug(String);

impl MissionSlug {
    /// Return the slug as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for MissionSlug {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for MissionSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for MissionSlug {
    type Err = KernelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if is_safe_path_segment(value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(KernelError::InvalidMissionSlug {
                value: value.to_owned(),
            })
        }
    }
}

impl Serialize for MissionSlug {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for MissionSlug {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// The canonical upstream safe-segment grammar, implemented without a regex
/// dependency.
fn is_safe_path_segment(value: &str) -> bool {
    if value.is_empty() || !value.is_ascii() || value.contains("..") {
        return false;
    }

    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }

    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// A validated work-package identifier.
///
/// Parsing follows the canonical status authority's
/// `normalize_wp_id` (`specify_cli/status/models.py`): ASCII whitespace is
/// trimmed, ASCII letters are upper-cased, and the result must be `WP`
/// followed by one or more ASCII digits. The stored form is always canonical,
/// so `Display`, ordering, and serialization are stable.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WpId(String);

impl WpId {
    /// Return the canonical id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for WpId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for WpId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WpId {
    type Err = KernelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim_matches(|character: char| character.is_ascii_whitespace());
        if trimmed.is_ascii() {
            let canonical = trimmed.to_ascii_uppercase();
            if let Some(digits) = canonical.strip_prefix("WP") {
                if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Ok(Self(canonical));
                }
            }
        }

        Err(KernelError::InvalidWpId {
            value: value.to_owned(),
        })
    }
}

impl Serialize for WpId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WpId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

//! Canonical Charter Pack tier vocabulary and precedence.
//!
//! Mirror of `src/kernel/pack_tiers.py`. `built-in` is the hyphenated,
//! on-disk spelling; precedence is built-in < org < project.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CharterError;

/// A Charter Pack tier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackTier {
    /// The shipped `packs/built-in/` tier.
    BuiltIn,
    /// An organisation pack configured in `.kittify/config.yaml`.
    Org,
    /// The project overlay pack at `.kittify/charter-packs/`.
    Project,
}

impl PackTier {
    /// Every tier, lowest precedence first.
    pub const ORDER: [Self; 3] = [Self::BuiltIn, Self::Org, Self::Project];

    /// The canonical tier token (also the on-disk directory name).
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::Org => "org",
            Self::Project => "project",
        }
    }

    /// The precedence rank; a higher rank overrides a lower one.
    pub const fn precedence_rank(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::Org => 1,
            Self::Project => 2,
        }
    }

    /// Parse a canonical tier token.
    pub fn from_token(token: &str) -> Result<Self, CharterError> {
        match token.trim().to_ascii_lowercase().as_str() {
            "built-in" => Ok(Self::BuiltIn),
            "org" => Ok(Self::Org),
            "project" => Ok(Self::Project),
            other => Err(CharterError::UnknownPackTier {
                token: other.to_owned(),
            }),
        }
    }
}

impl fmt::Display for PackTier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_token())
    }
}

//! Canonical glossary scopes and their resolution precedence.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::GlossaryError;

/// Glossary scope levels, declared from highest to lowest precedence.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GlossaryScope {
    /// The current mission's local glossary seed.
    MissionLocal,
    /// Team-owned domain vocabulary.
    TeamDomain,
    /// Audience-facing domain vocabulary.
    AudienceDomain,
    /// The product's canonical core vocabulary.
    SpecKittyCore,
}

/// Resolution order, highest precedence first.
pub const SCOPE_RESOLUTION_ORDER: [GlossaryScope; 4] = [
    GlossaryScope::MissionLocal,
    GlossaryScope::TeamDomain,
    GlossaryScope::AudienceDomain,
    GlossaryScope::SpecKittyCore,
];

impl GlossaryScope {
    /// All scopes in resolution precedence order.
    pub const ALL: [Self; 4] = SCOPE_RESOLUTION_ORDER;

    /// The upstream snake_case scope value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissionLocal => "mission_local",
            Self::TeamDomain => "team_domain",
            Self::AudienceDomain => "audience_domain",
            Self::SpecKittyCore => "spec_kitty_core",
        }
    }

    /// The canonical seed filename for this scope.
    pub const fn filename(self) -> &'static str {
        match self {
            Self::MissionLocal => "mission_local.yaml",
            Self::TeamDomain => "team_domain.yaml",
            Self::AudienceDomain => "audience_domain.yaml",
            Self::SpecKittyCore => "spec_kitty_core.yaml",
        }
    }

    /// Map an exact seed filename to its scope. `.yml` is intentionally not accepted.
    pub fn from_filename(filename: &str) -> Option<Self> {
        match filename {
            "mission_local.yaml" => Some(Self::MissionLocal),
            "team_domain.yaml" => Some(Self::TeamDomain),
            "audience_domain.yaml" => Some(Self::AudienceDomain),
            "spec_kitty_core.yaml" => Some(Self::SpecKittyCore),
            _ => None,
        }
    }

    /// Map a path's final component to its scope.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(Self::from_filename)
    }
}

impl fmt::Display for GlossaryScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for GlossaryScope {
    type Err = GlossaryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mission_local" => Ok(Self::MissionLocal),
            "team_domain" => Ok(Self::TeamDomain),
            "audience_domain" => Ok(Self::AudienceDomain),
            "spec_kitty_core" => Ok(Self::SpecKittyCore),
            _ => Err(GlossaryError::InvalidScope {
                value: value.to_owned(),
            }),
        }
    }
}

impl TryFrom<&str> for GlossaryScope {
    type Error = GlossaryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

impl Serialize for GlossaryScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for GlossaryScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

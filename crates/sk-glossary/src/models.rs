//! Validated glossary domain models.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{GlossaryError, GlossaryScope};

/// A validated, normalized term surface.
///
/// The upstream implementation requires surfaces to be lowercase and trimmed.
/// [`TermSurface::parse`] enforces that exact form; [`TermSurface::normalized`]
/// is the convenient CLI-facing constructor that trims and lowercases first.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TermSurface(String);

impl TermSurface {
    /// Parse a surface that must already be lowercase and trimmed.
    pub fn parse(value: &str) -> Result<Self, GlossaryError> {
        if value.is_empty() {
            return Err(GlossaryError::EmptySurface);
        }
        let expected = value.trim().to_lowercase();
        if value != expected {
            return Err(GlossaryError::NonNormalizedSurface {
                value: value.to_owned(),
                expected,
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Trim and lowercase a user-provided surface, then validate it.
    pub fn normalized(value: &str) -> Result<Self, GlossaryError> {
        let expected = value.trim().to_lowercase();
        if expected.is_empty() {
            return Err(GlossaryError::EmptySurface);
        }
        Ok(Self(expected))
    }

    /// Return the normalized surface text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for TermSurface {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for TermSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TermSurface {
    type Err = GlossaryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for TermSurface {
    type Error = GlossaryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl Serialize for TermSurface {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TermSurface {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Lifecycle status of a term sense.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TermStatus {
    /// Auto-extracted or otherwise unconfirmed sense.
    #[default]
    Draft,
    /// Promoted or authoritative sense.
    Active,
    /// Historical sense that remains visible but is not active.
    Deprecated,
}

impl TermStatus {
    /// The upstream snake_case status value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Deprecated => "deprecated",
        }
    }

    /// Whether this sense participates in active ambiguity detection.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl fmt::Display for TermStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TermStatus {
    type Err = GlossaryError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "draft" => Ok(Self::Draft),
            "active" => Ok(Self::Active),
            "deprecated" => Ok(Self::Deprecated),
            _ => Err(GlossaryError::InvalidStatus {
                value: value.to_owned(),
            }),
        }
    }
}

impl TryFrom<&str> for TermStatus {
    type Error = GlossaryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_str(value)
    }
}

/// Provenance metadata recorded for a term sense.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Provenance {
    actor_id: String,
    timestamp: DateTime<Utc>,
    source: String,
}

impl Provenance {
    /// Build a provenance record.
    pub fn new(
        actor_id: impl Into<String>,
        timestamp: DateTime<Utc>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            actor_id: actor_id.into(),
            timestamp,
            source: source.into(),
        }
    }

    /// Provenance used by the read-only seed loader.
    pub fn seed_file(timestamp: DateTime<Utc>) -> Self {
        Self::new("system:seed_file", timestamp, "seed_file")
    }

    /// Actor identity such as `user:alice` or `system:seed_file`.
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    /// When the sense was recorded.
    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    /// Source such as `seed_file` or `user_clarification`.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Optional annotation metadata accepted by the upstream seed schema.
///
/// These fields are preserved for callers; this crate never writes a seed
/// file back to disk, so upstream comment and formatting preservation is out
/// of scope.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct TermMetadata {
    /// Cross-references such as `{fr: FR-008, description: ...}`.
    pub see_also: Vec<BTreeMap<String, String>>,
    /// Terms that should not be used for this sense.
    pub synonyms_to_avoid: Vec<String>,
    /// Mission that introduced the term, when annotated.
    pub introduced_in_mission: Option<String>,
}

/// A meaning of a term surface inside a specific glossary scope.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TermSense {
    surface: TermSurface,
    scope: GlossaryScope,
    definition: String,
    confidence: f64,
    status: TermStatus,
    metadata: TermMetadata,
    provenance: Provenance,
}

impl TermSense {
    /// Validate and build a term sense with seed-file provenance.
    pub fn new(
        surface: TermSurface,
        scope: GlossaryScope,
        definition: impl Into<String>,
        confidence: f64,
        status: TermStatus,
    ) -> Result<Self, GlossaryError> {
        let definition = definition.into();
        if definition.trim().is_empty() {
            return Err(GlossaryError::EmptyDefinition);
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(GlossaryError::InvalidConfidence { value: confidence });
        }
        Ok(Self {
            surface,
            scope,
            definition,
            confidence,
            status,
            metadata: TermMetadata::default(),
            provenance: Provenance::seed_file(Utc::now()),
        })
    }

    /// Attach parsed seed metadata.
    pub fn with_metadata(mut self, metadata: TermMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Attach explicit provenance.
    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = provenance;
        self
    }

    /// The normalized term surface.
    pub fn surface(&self) -> &TermSurface {
        &self.surface
    }

    /// The scope that owns this sense.
    pub fn scope(&self) -> GlossaryScope {
        self.scope
    }

    /// The sense definition.
    pub fn definition(&self) -> &str {
        &self.definition
    }

    /// Confidence in the range `0.0..=1.0`.
    pub fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Lifecycle status.
    pub fn status(&self) -> TermStatus {
        self.status
    }

    /// All optional seed metadata.
    pub fn metadata(&self) -> &TermMetadata {
        &self.metadata
    }

    /// Cross-reference metadata.
    pub fn see_also(&self) -> &[BTreeMap<String, String>] {
        &self.metadata.see_also
    }

    /// Terms that should not be used for this sense.
    pub fn synonyms_to_avoid(&self) -> &[String] {
        &self.metadata.synonyms_to_avoid
    }

    /// The mission that introduced this term, when annotated.
    pub fn introduced_in_mission(&self) -> Option<&str> {
        self.metadata.introduced_in_mission.as_deref()
    }

    /// Provenance recorded for the sense.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

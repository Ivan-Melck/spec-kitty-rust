//! The canonical artifact-kind taxonomy.
//!
//! Mirror of `src/charter/offering/artifact_kinds.py`. One enumeration owns the
//! canonical YAML value, the hyphenated operator token, the plural directory,
//! the discovery glob, the project-overlay directory, and the two boolean facts
//! (built-in content directory, charter-activatability). `mission-type` is a
//! charter kind but deliberately not an [`ArtifactKind`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::CharterError;

/// The operator token for the mission-tier kind, which is not an artifact kind.
pub const MISSION_TYPE_TOKEN: &str = "mission-type";

/// The hand-authorable operator token universe plus `mission-type`.
///
/// `template`, `asset` and `anti-pattern` are artifact kinds but not
/// hand-authored as standalone operator-facing artifacts.
pub const CHARTER_KIND_TOKENS: [&str; 11] = [
    "directive",
    "tactic",
    "styleguide",
    "toolguide",
    "paradigm",
    "procedure",
    "agent-profile",
    "mission-step-contract",
    "glossary-pack",
    "skill",
    "mission-type",
];

/// Every doctrine artifact kind.
///
/// Serialized as its canonical snake_case YAML value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A governance directive.
    Directive,
    /// A reusable tactic.
    Tactic,
    /// A styleguide.
    Styleguide,
    /// A toolguide.
    Toolguide,
    /// A paradigm.
    Paradigm,
    /// A procedure.
    Procedure,
    /// An agent profile (`*.agent.yaml`).
    AgentProfile,
    /// A mission step contract.
    MissionStepContract,
    /// A mission template (mission-tier, empty glob).
    Template,
    /// An asset sidecar manifest.
    Asset,
    /// A glossary pack.
    GlossaryPack,
    /// An anti-pattern node (never a standalone artifact file).
    AntiPattern,
    /// A pack skill.
    Skill,
}

impl ArtifactKind {
    /// Every member in declaration order.
    pub const ALL: [Self; 13] = [
        Self::Directive,
        Self::Tactic,
        Self::Styleguide,
        Self::Toolguide,
        Self::Paradigm,
        Self::Procedure,
        Self::AgentProfile,
        Self::MissionStepContract,
        Self::Template,
        Self::Asset,
        Self::GlossaryPack,
        Self::AntiPattern,
        Self::Skill,
    ];

    /// The canonical singular value stored in YAML `type` fields.
    pub const fn canonical(self) -> &'static str {
        match self {
            Self::Directive => "directive",
            Self::Tactic => "tactic",
            Self::Styleguide => "styleguide",
            Self::Toolguide => "toolguide",
            Self::Paradigm => "paradigm",
            Self::Procedure => "procedure",
            Self::AgentProfile => "agent_profile",
            Self::MissionStepContract => "mission_step_contract",
            Self::Template => "template",
            Self::Asset => "asset",
            Self::GlossaryPack => "glossary_pack",
            Self::AntiPattern => "anti_pattern",
            Self::Skill => "skill",
        }
    }

    /// The pinned Source `ArtifactKind` enum member identity used by Python repr.
    /// This is type metadata, distinct from YAML values and operator tokens.
    pub const fn python_member_name(self) -> &'static str {
        match self {
            Self::Directive => "DIRECTIVE",
            Self::Tactic => "TACTIC",
            Self::Styleguide => "STYLEGUIDE",
            Self::Toolguide => "TOOLGUIDE",
            Self::Paradigm => "PARADIGM",
            Self::Procedure => "PROCEDURE",
            Self::AgentProfile => "AGENT_PROFILE",
            Self::MissionStepContract => "MISSION_STEP_CONTRACT",
            Self::Template => "TEMPLATE",
            Self::Asset => "ASSET",
            Self::GlossaryPack => "GLOSSARY_PACK",
            Self::AntiPattern => "ANTI_PATTERN",
            Self::Skill => "SKILL",
        }
    }

    /// The hyphenated operator token used by CLI surfaces.
    pub const fn operator_token(self) -> &'static str {
        match self {
            Self::Directive => "directive",
            Self::Tactic => "tactic",
            Self::Styleguide => "styleguide",
            Self::Toolguide => "toolguide",
            Self::Paradigm => "paradigm",
            Self::Procedure => "procedure",
            Self::AgentProfile => "agent-profile",
            Self::MissionStepContract => "mission-step-contract",
            Self::Template => "template",
            Self::Asset => "asset",
            Self::GlossaryPack => "glossary-pack",
            Self::AntiPattern => "anti-pattern",
            Self::Skill => "skill",
        }
    }

    /// The plural directory name.
    pub const fn plural(self) -> &'static str {
        match self {
            Self::Directive => "directives",
            Self::Tactic => "tactics",
            Self::Styleguide => "styleguides",
            Self::Toolguide => "toolguides",
            Self::Paradigm => "paradigms",
            Self::Procedure => "procedures",
            Self::AgentProfile => "agent_profiles",
            Self::MissionStepContract => "mission_step_contracts",
            Self::Template => "templates",
            Self::Asset => "assets",
            Self::GlossaryPack => "glossary_packs",
            Self::AntiPattern => "anti_patterns",
            Self::Skill => "skills",
        }
    }

    /// The file discovery glob; empty for [`Self::Template`].
    pub const fn glob_pattern(self) -> &'static str {
        match self {
            Self::Directive => "*.directive.yaml",
            Self::Tactic => "*.tactic.yaml",
            Self::Styleguide => "*.styleguide.yaml",
            Self::Toolguide => "*.toolguide.yaml",
            Self::Paradigm => "*.paradigm.yaml",
            Self::Procedure => "*.procedure.yaml",
            Self::AgentProfile => "*.agent.yaml",
            Self::MissionStepContract => "*.step-contract.yaml",
            Self::Template => "",
            Self::Asset => "*.asset.yaml",
            Self::GlossaryPack => "*.glossary-pack.yaml",
            Self::AntiPattern => "*.anti_pattern.yaml",
            Self::Skill => "*.skill.yaml",
        }
    }

    /// The project-overlay directory under `.kittify/charter-packs/`.
    ///
    /// The runtime-managed four (`directive`, `tactic`, `styleguide`,
    /// `procedure`) use the singular form; every other kind uses its plural.
    pub const fn project_dir(self) -> &'static str {
        match self {
            Self::Directive => "directive",
            Self::Tactic => "tactic",
            Self::Styleguide => "styleguide",
            Self::Procedure => "procedure",
            other => other.plural(),
        }
    }

    /// Whether this kind ships a `packs/built-in/<plural>/` content directory.
    ///
    /// `false` for the three package-resource/graph-only kinds:
    /// `mission_step_contract`, `template`, `anti_pattern`.
    pub const fn has_built_in_content_dir(self) -> bool {
        !matches!(
            self,
            Self::MissionStepContract | Self::Template | Self::AntiPattern
        )
    }

    /// Whether this kind can be activated as a live governance rule.
    ///
    /// Every kind except `template` and `asset`; `anti_pattern` **is**
    /// activatable.
    pub const fn activatable(self) -> bool {
        !matches!(self, Self::Template | Self::Asset)
    }

    /// Parse a documented operator token.
    ///
    /// Normalizes case, surrounding whitespace and hyphens to the canonical
    /// underscore form. `mission-type` raises the distinct
    /// [`CharterError::MissionTypeNotAnArtifactKind`]; anything else unknown
    /// raises [`CharterError::UnknownArtifactKind`] listing the valid tokens.
    pub fn from_operator_token(token: &str) -> Result<Self, CharterError> {
        let normalized = token.trim().to_ascii_lowercase().replace('-', "_");
        if normalized == "mission_type" {
            return Err(CharterError::MissionTypeNotAnArtifactKind);
        }
        if let Some(kind) = Self::ALL.iter().find(|kind| kind.canonical() == normalized) {
            return Ok(*kind);
        }
        Err(CharterError::UnknownArtifactKind {
            token: token.to_owned(),
            valid: Self::ALL
                .iter()
                .map(|kind| kind.operator_token())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }

    /// Parse a canonical snake_case value.
    ///
    /// `mission-type` is reported as the distinct mission-tier error rather than
    /// as an unknown token, on either spelling.
    pub fn from_canonical(token: &str) -> Result<Self, CharterError> {
        let normalized = token.trim().to_ascii_lowercase();
        if normalized.replace('-', "_") == "mission_type" {
            return Err(CharterError::MissionTypeNotAnArtifactKind);
        }
        Self::ALL
            .iter()
            .find(|kind| kind.canonical() == normalized)
            .copied()
            .ok_or_else(|| CharterError::UnknownArtifactKind {
                token: token.to_owned(),
                valid: Self::ALL
                    .iter()
                    .map(|kind| kind.canonical())
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }

    /// Parse a plural directory name.
    pub fn from_plural(plural: &str) -> Result<Self, CharterError> {
        Self::ALL
            .iter()
            .find(|kind| kind.plural() == plural)
            .copied()
            .ok_or_else(|| CharterError::UnknownPlural {
                plural: plural.to_owned(),
            })
    }

    /// Return the kind whose discovery glob is exactly `pattern`.
    pub fn from_glob_pattern(pattern: &str) -> Option<Self> {
        if pattern.is_empty() {
            return None;
        }
        Self::ALL
            .iter()
            .find(|kind| kind.glob_pattern() == pattern)
            .copied()
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.canonical())
    }
}

impl FromStr for ArtifactKind {
    type Err = CharterError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_operator_token(value)
    }
}

/// A kind in the charter kind universe: an artifact kind or `mission-type`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CharterKind {
    /// A concrete artifact kind.
    Artifact(ArtifactKind),
    /// The mission-tier token, routed separately from artifact kinds.
    MissionType,
}

impl CharterKind {
    /// Parse an operator token, routing `mission-type` explicitly.
    pub fn from_token(token: &str) -> Result<Self, CharterError> {
        if token.trim().to_ascii_lowercase().replace('-', "_") == "mission_type" {
            return Ok(Self::MissionType);
        }
        ArtifactKind::from_operator_token(token).map(Self::Artifact)
    }

    /// The canonical operator token.
    pub const fn operator_token(self) -> &'static str {
        match self {
            Self::Artifact(kind) => kind.operator_token(),
            Self::MissionType => MISSION_TYPE_TOKEN,
        }
    }
}

impl fmt::Display for CharterKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.operator_token())
    }
}

//! Significance scoring, routing bands and hard-trigger policy.
//!
//! This is the pure policy kernel of the upstream `significance.py` module:
//! six fixed dimensions scored 0-3, a composite of 0-18, three contiguous
//! routing bands, and five fixed hard-trigger classes that override the
//! numeric band to `high`.
//!
//! Everything here is deterministic and side-effect free. The caller supplies
//! dimension scores, hard-trigger ids and (optionally) custom band cutoffs; the
//! result is a wire-compatible [`SignificanceScore`] that a run store can
//! persist under `decisions["significance:<decision_id>"]`.
//!
//! # Fail-closed rules
//!
//! * Dimension scores must be exactly the six V1 dimensions, each 0-3.
//! * Hard-trigger ids must be one of the five fixed classes.
//! * Custom cutoffs must be exactly `low`/`medium`/`high`, contiguous, cover
//!   0-18, and must not overlap.
//! * An unparseable `significance_band_cutoffs` policy extra is a typed
//!   [`RuntimeError::InvalidPolicy`], never a silent fall back to defaults.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;
use crate::snapshot::MissionPolicySnapshot;
use crate::template::SIGNIFICANCE_DIMENSIONS;

/// Highest score a single significance dimension can take.
pub const MAX_DIMENSION_SCORE: u8 = 3;

/// Highest possible composite score (six dimensions at three).
pub const MAX_COMPOSITE_SCORE: u8 = 18;

/// The closed set of significance dimensions (V1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionName {
    UserCustomerImpact,
    ArchitecturalSystemImpact,
    DataSecurityComplianceImpact,
    OperationalReliabilityImpact,
    FinancialCommercialImpact,
    CrossTeamBlastRadius,
}

impl DimensionName {
    /// The upstream wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserCustomerImpact => "user_customer_impact",
            Self::ArchitecturalSystemImpact => "architectural_system_impact",
            Self::DataSecurityComplianceImpact => "data_security_compliance_impact",
            Self::OperationalReliabilityImpact => "operational_reliability_impact",
            Self::FinancialCommercialImpact => "financial_commercial_impact",
            Self::CrossTeamBlastRadius => "cross_team_blast_radius",
        }
    }

    /// Resolve a wire name, case-sensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|dimension| dimension.as_str() == name)
    }

    /// Every dimension, in the upstream declaration order.
    pub const ALL: [Self; 6] = [
        Self::UserCustomerImpact,
        Self::ArchitecturalSystemImpact,
        Self::DataSecurityComplianceImpact,
        Self::OperationalReliabilityImpact,
        Self::FinancialCommercialImpact,
        Self::CrossTeamBlastRadius,
    ];
}

/// The closed set of fixed hard-trigger classes (V1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardTriggerId {
    ProductionDataDestructive,
    SecurityPrivacyAccessControl,
    LegalComplianceRegulatory,
    BillingFinancialCommitment,
    ArchitectureFoundation,
}

impl HardTriggerId {
    /// The upstream wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProductionDataDestructive => "production_data_destructive",
            Self::SecurityPrivacyAccessControl => "security_privacy_access_control",
            Self::LegalComplianceRegulatory => "legal_compliance_regulatory",
            Self::BillingFinancialCommitment => "billing_financial_commitment",
            Self::ArchitectureFoundation => "architecture_foundation",
        }
    }

    /// Resolve a wire name, case-sensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|trigger| trigger.as_str() == name)
    }

    /// The fixed upstream description for this trigger class.
    pub fn description(self) -> &'static str {
        match self {
            Self::ProductionDataDestructive => {
                "Production data-destructive or schema-impacting changes"
            }
            Self::SecurityPrivacyAccessControl => "Security/privacy/access-control changes",
            Self::LegalComplianceRegulatory => "Legal/compliance/regulatory impact",
            Self::BillingFinancialCommitment => "Billing/financial commitment changes",
            Self::ArchitectureFoundation => {
                "Architecture-foundation changes (language, framework, runtime, datastore, infrastructure)"
            }
        }
    }

    /// Every hard-trigger class, in the upstream declaration order.
    pub const ALL: [Self; 5] = [
        Self::ProductionDataDestructive,
        Self::SecurityPrivacyAccessControl,
        Self::LegalComplianceRegulatory,
        Self::BillingFinancialCommitment,
        Self::ArchitectureFoundation,
    ];
}

/// The closed set of routing bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingBandName {
    Low,
    Medium,
    High,
}

impl RoutingBandName {
    /// The upstream wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Resolve a wire name, case-sensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// One scored significance dimension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignificanceDimension {
    pub name: DimensionName,
    pub score: u8,
    #[serde(default)]
    pub description: String,
}

/// A routing band with an inclusive score range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingBand {
    pub name: RoutingBandName,
    pub min_score: u8,
    pub max_score: u8,
}

/// One resolved hard-trigger class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardTriggerClass {
    pub class_id: HardTriggerId,
    pub description: String,
}

/// The upstream default bands: low 0-6, medium 7-11, high 12-18.
pub fn default_bands() -> [RoutingBand; 3] {
    [
        RoutingBand {
            name: RoutingBandName::Low,
            min_score: 0,
            max_score: 6,
        },
        RoutingBand {
            name: RoutingBandName::Medium,
            min_score: 7,
            max_score: 11,
        },
        RoutingBand {
            name: RoutingBandName::High,
            min_score: 12,
            max_score: 18,
        },
    ]
}

/// Validated, contiguous band cutoffs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandCutoffs {
    bands: BTreeMap<RoutingBandName, (u8, u8)>,
}

impl BandCutoffs {
    /// Validate and build cutoffs from wire-shaped `{band: [min, max]}` data.
    pub fn new(cutoffs: &BTreeMap<String, [i64; 2]>) -> Result<Self, RuntimeError> {
        validate_band_cutoffs(cutoffs)?;
        let mut bands = BTreeMap::new();
        for (name, pair) in cutoffs {
            let band = RoutingBandName::from_name(name).expect("validated band name");
            let (min_score, max_score) = (pair[0] as u8, pair[1] as u8);
            bands.insert(band, (min_score, max_score));
        }
        Ok(Self { bands })
    }

    /// The three bands sorted by minimum score.
    pub fn bands(&self) -> Vec<RoutingBand> {
        let mut bands: Vec<RoutingBand> = self
            .bands
            .iter()
            .map(|(name, (min_score, max_score))| RoutingBand {
                name: *name,
                min_score: *min_score,
                max_score: *max_score,
            })
            .collect();
        bands.sort_by_key(|band| band.min_score);
        bands
    }
}

/// The composite significance assessment of one audit gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignificanceScore {
    pub dimensions: Vec<SignificanceDimension>,
    pub composite: u8,
    pub band: RoutingBand,
    #[serde(default)]
    pub hard_trigger_classes: Vec<HardTriggerClass>,
    pub effective_band: RoutingBand,
}

impl SignificanceScore {
    /// Validate a recorded score using the pinned Pydantic model's invariants.
    ///
    /// Like the oracle, this checks the set of dimension names (not vector
    /// length) and does not recompute the declared band's name from cutoffs.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        let invalid = |message: &str| RuntimeError::InvalidSignificanceInput {
            message: message.to_owned(),
        };
        let names: std::collections::BTreeSet<_> = self
            .dimensions
            .iter()
            .map(|dimension| dimension.name)
            .collect();
        if names != DimensionName::ALL.into_iter().collect() {
            return Err(invalid(
                "dimensions must contain all six fixed dimension names",
            ));
        }
        if self
            .dimensions
            .iter()
            .any(|dimension| dimension.score > MAX_DIMENSION_SCORE)
        {
            return Err(invalid("dimension scores must be within 0-3"));
        }
        let expected: usize = self
            .dimensions
            .iter()
            .map(|dimension| usize::from(dimension.score))
            .sum();
        if self.composite > MAX_COMPOSITE_SCORE || usize::from(self.composite) != expected {
            return Err(invalid(
                "composite must be within 0-18 and equal the sum of dimension scores",
            ));
        }
        for band in [self.band, self.effective_band] {
            if band.min_score > band.max_score || band.max_score > MAX_COMPOSITE_SCORE {
                return Err(invalid("band bounds must be ordered and within 0-18"));
            }
        }
        if self.hard_trigger_classes.is_empty() {
            if self.effective_band != self.band {
                return Err(invalid(
                    "effective_band must equal band when no hard triggers",
                ));
            }
        } else if self.effective_band.name != RoutingBandName::High {
            return Err(invalid(
                "effective_band must be high when hard triggers are present",
            ));
        }
        Ok(())
    }

    /// The effective band name: the numeric band, unless a hard trigger forced
    /// `high`.
    pub fn effective_band_name(&self) -> RoutingBandName {
        self.effective_band.name
    }
}

/// Validate that a dimension-score map is exactly the six V1 dimensions, each
/// within 0-3.
pub fn validate_dimension_scores(scores: &BTreeMap<String, i32>) -> Result<(), RuntimeError> {
    let provided: Vec<&str> = scores.keys().map(String::as_str).collect();
    let mut missing: Vec<&str> = Vec::new();
    for name in SIGNIFICANCE_DIMENSIONS {
        if !scores.contains_key(name) {
            missing.push(name);
        }
    }
    let mut unexpected: Vec<&str> = provided
        .iter()
        .copied()
        .filter(|name| DimensionName::from_name(name).is_none())
        .collect();
    unexpected.sort_unstable();
    if !missing.is_empty() || !unexpected.is_empty() {
        let mut parts = Vec::new();
        if !missing.is_empty() {
            parts.push(format!("missing: {missing:?}"));
        }
        if !unexpected.is_empty() {
            parts.push(format!("unexpected: {unexpected:?}"));
        }
        return Err(RuntimeError::InvalidSignificanceInput {
            message: format!(
                "Dimension scores must contain exactly {} dimensions. {}",
                SIGNIFICANCE_DIMENSIONS.len(),
                parts.join(", ")
            ),
        });
    }
    for (name, score) in scores {
        if !(0..=i32::from(MAX_DIMENSION_SCORE)).contains(score) {
            return Err(RuntimeError::InvalidSignificanceInput {
                message: format!("Dimension '{name}' score must be 0-3, got {score}"),
            });
        }
    }
    Ok(())
}

/// Validate custom band cutoffs: exactly three bands, contiguous, covering
/// 0-18 with no gaps or overlaps.
pub fn validate_band_cutoffs(cutoffs: &BTreeMap<String, [i64; 2]>) -> Result<(), RuntimeError> {
    let expected = ["low", "medium", "high"];
    if cutoffs.len() != expected.len() || expected.iter().any(|name| !cutoffs.contains_key(*name)) {
        let provided: Vec<&str> = cutoffs.keys().map(String::as_str).collect();
        return Err(RuntimeError::InvalidPolicy {
            message: format!("Expected exactly 3 bands (low, medium, high), got: {provided:?}"),
        });
    }

    for (band_name, pair) in cutoffs {
        if pair[0] > pair[1] {
            return Err(RuntimeError::InvalidPolicy {
                message: format!(
                    "Band '{band_name}': min_score ({}) > max_score ({})",
                    pair[0], pair[1]
                ),
            });
        }
        for bound in pair {
            if !(0..=i64::from(MAX_COMPOSITE_SCORE)).contains(bound) {
                return Err(RuntimeError::InvalidPolicy {
                    message: format!(
                        "Band '{band_name}' cutoff {bound} is outside 0..={MAX_COMPOSITE_SCORE}"
                    ),
                });
            }
        }
    }

    let mut sorted: Vec<(&str, [i64; 2])> = cutoffs
        .iter()
        .map(|(name, pair)| (name.as_str(), *pair))
        .collect();
    sorted.sort_by_key(|(_, pair)| pair[0]);

    let (first_name, first) = sorted[0];
    if first[0] != 0 {
        return Err(RuntimeError::InvalidPolicy {
            message: format!(
                "Band '{first_name}' must start at 0, starts at {}",
                first[0]
            ),
        });
    }
    let (last_name, last) = sorted[sorted.len() - 1];
    if last[1] != i64::from(MAX_COMPOSITE_SCORE) {
        return Err(RuntimeError::InvalidPolicy {
            message: format!("Band '{last_name}' must end at 18, ends at {}", last[1]),
        });
    }
    for window in sorted.windows(2) {
        let (prev_name, prev) = window[0];
        let (next_name, next) = window[1];
        if next[0] > prev[1] + 1 {
            return Err(RuntimeError::InvalidPolicy {
                message: format!(
                    "Gap between band '{prev_name}' (max={}) and '{next_name}' (min={})",
                    prev[1], next[0]
                ),
            });
        }
        if next[0] <= prev[1] {
            return Err(RuntimeError::InvalidPolicy {
                message: format!(
                    "Overlap between band '{prev_name}' (max={}) and '{next_name}' (min={})",
                    prev[1], next[0]
                ),
            });
        }
    }
    Ok(())
}

/// Extract validated band cutoffs from a policy snapshot's extras.
///
/// `None` means the policy does not configure cutoffs, so the default bands
/// apply. Any other present value must be a valid `{band: [min, max]}` mapping;
/// malformed policy is a typed error rather than a silent default.
pub fn parse_band_cutoffs_from_policy(
    policy: &MissionPolicySnapshot,
) -> Result<Option<BandCutoffs>, RuntimeError> {
    let Some(value) = policy.extras.get("significance_band_cutoffs") else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(RuntimeError::InvalidPolicy {
            message: format!(
                "significance_band_cutoffs must be a dict, got {}",
                json_type_name(value)
            ),
        });
    };
    let mut cutoffs: BTreeMap<String, [i64; 2]> = BTreeMap::new();
    for (band_name, bounds) in object {
        let Some(items) = bounds.as_array() else {
            return Err(RuntimeError::InvalidPolicy {
                message: format!("Band '{band_name}' cutoff must be [min, max], got {bounds}"),
            });
        };
        if items.len() != 2 {
            return Err(RuntimeError::InvalidPolicy {
                message: format!("Band '{band_name}' cutoff must be [min, max], got {bounds}"),
            });
        }
        let mut pair = [0_i64; 2];
        for (index, item) in items.iter().enumerate() {
            let Some(number) = item.as_i64() else {
                return Err(RuntimeError::InvalidPolicy {
                    message: format!("Band '{band_name}' cutoff values must be integers"),
                });
            };
            pair[index] = number;
        }
        cutoffs.insert(band_name.clone(), pair);
    }
    Ok(Some(BandCutoffs::new(&cutoffs)?))
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "NoneType",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "int",
        serde_json::Value::String(_) => "str",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "dict",
    }
}

/// Evaluate significance from dimension scores and hard-trigger ids.
///
/// Pure: identical inputs always produce identical output. Dimensions are
/// emitted sorted by name, matching upstream.
pub fn evaluate_significance(
    dimension_scores: &BTreeMap<String, i32>,
    hard_trigger_classes: &[String],
    band_cutoffs: Option<&BandCutoffs>,
) -> Result<SignificanceScore, RuntimeError> {
    validate_dimension_scores(dimension_scores)?;

    let mut dimensions: Vec<SignificanceDimension> = dimension_scores
        .iter()
        .map(|(name, score)| SignificanceDimension {
            name: DimensionName::from_name(name).expect("validated dimension name"),
            score: *score as u8,
            description: String::new(),
        })
        .collect();
    dimensions.sort_by_key(|dimension| dimension.name.as_str());

    let composite: u8 = dimension_scores
        .values()
        .map(|score| *score as u8)
        .sum::<u8>();

    let bands = match band_cutoffs {
        Some(cutoffs) => cutoffs.bands(),
        None => default_bands().to_vec(),
    };
    let band = bands
        .iter()
        .find(|band| band.min_score <= composite && composite <= band.max_score)
        .copied()
        .ok_or_else(|| RuntimeError::InvalidSignificanceInput {
            message: format!("composite score {composite} does not fall within any band"),
        })?;

    let mut triggers = Vec::with_capacity(hard_trigger_classes.len());
    for class_id in hard_trigger_classes {
        let Some(id) = HardTriggerId::from_name(class_id) else {
            return Err(RuntimeError::InvalidSignificanceInput {
                message: format!("Unknown hard-trigger class: {class_id:?}"),
            });
        };
        triggers.push(HardTriggerClass {
            class_id: id,
            description: id.description().to_string(),
        });
    }

    let effective_band = if triggers.is_empty() {
        band
    } else {
        *bands
            .iter()
            .find(|band| band.name == RoutingBandName::High)
            .ok_or_else(|| RuntimeError::InvalidSignificanceInput {
                message: "routing bands do not define a high band".to_string(),
            })?
    };

    Ok(SignificanceScore {
        dimensions,
        composite,
        band,
        hard_trigger_classes: triggers,
        effective_band,
    })
}

/// The `SignificanceEvaluated` record an advance commits for an audit gate.
///
/// Wire-compatible with the upstream event payload so the run-store owner can
/// append it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignificanceEvaluated {
    pub run_id: String,
    pub decision_id: String,
    pub step_id: String,
    pub significance_score: SignificanceScore,
    pub hard_trigger_classes: Vec<HardTriggerId>,
    pub effective_band: RoutingBandName,
    pub actor: crate::raci::RaciRoleBinding,
}

impl SignificanceEvaluated {
    /// Build the payload for an evaluated audit gate. The actor is the runtime
    /// service actor (`runtime`), matching upstream.
    pub fn for_gate(
        run_id: &str,
        decision_id: &str,
        step_id: &str,
        score: &SignificanceScore,
    ) -> Self {
        Self {
            run_id: run_id.to_string(),
            decision_id: decision_id.to_string(),
            step_id: step_id.to_string(),
            significance_score: score.clone(),
            hard_trigger_classes: score
                .hard_trigger_classes
                .iter()
                .map(|trigger| trigger.class_id)
                .collect(),
            effective_band: score.effective_band.name,
            actor: crate::raci::RaciRoleBinding {
                actor_type: crate::snapshot::ActorType::Service,
                actor_id: "runtime".to_string(),
            },
        }
    }
}

/// The responsible human's recorded action on a medium-band gate.
///
/// Wire-compatible with the upstream `SoftGateDecision` model persisted under
/// `decisions["soft_gate:<decision_id>"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftGateDecision {
    pub decision_id: String,
    pub action: SoftGateAction,
    pub actor: crate::raci::RaciRoleBinding,
    pub timestamp: String,
    pub significance_score: SignificanceScore,
    #[serde(default)]
    pub participants: Vec<crate::raci::RaciRoleBinding>,
    pub outcome: Option<SoftGateAction>,
    pub rationale: Option<String>,
}

/// The closed set of medium-band soft-gate actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoftGateAction {
    DecideSolo,
    OpenStandUp,
    Defer,
}

impl SoftGateAction {
    /// The upstream wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DecideSolo => "decide_solo",
            Self::OpenStandUp => "open_stand_up",
            Self::Defer => "defer",
        }
    }

    /// Resolve a wire value, case-sensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "decide_solo" => Some(Self::DecideSolo),
            "open_stand_up" => Some(Self::OpenStandUp),
            "defer" => Some(Self::Defer),
            _ => None,
        }
    }
}

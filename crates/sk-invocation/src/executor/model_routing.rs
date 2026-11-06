//! Advisory model-task routing: the catalog schema, the maintained verb map, and
//! the deterministic objective-function scorer.
//!
//! Ports the observable behaviour of upstream
//! `charter/offering/model_task_routing/{models,evaluator}.py` plus
//! `specify_cli/invocation/task_class_map.py`. The catalog document itself is
//! parsed by the composition root (the CLI owns YAML reading) and handed here as
//! a typed [`ModelRoutingCatalog`]; nothing in this module performs I/O.
//!
//! Advisory only: [`InvocationPayload::recommendation`](super::InvocationPayload)
//! carries the result, and no dispatch decision depends on it. `None` is a real
//! outcome (unknown verb, no matching candidate); every failure of the loader is
//! the caller's non-fatal envelope, never a dispatch failure.

use serde::{Deserialize, Serialize};

/// The maintained verb → catalog `task_type` bridge.
///
/// A flat map, exactly like the reference: the dispatch action model carries the
/// resolved verb only, not a `(role, verb)` pair.
pub const VERB_TO_TASK_TYPE: [(&str, &str); 21] = [
    // implementer
    ("generate", "code-generation"),
    ("refine", "code-refinement"),
    ("implement", "code-implementation"),
    // reviewer
    ("audit", "quality-audit"),
    ("assess", "quality-assessment"),
    ("review", "code-review"),
    // architect
    ("synthesize", "synthesis"),
    // designer
    ("draft", "drafting"),
    ("design", "design"),
    // planner
    ("plan", "planning"),
    ("decompose", "task-decomposition"),
    ("prioritize", "prioritization"),
    // researcher
    ("analyze", "analysis"),
    ("investigate", "investigation"),
    ("summarize", "summarization"),
    // curator
    ("classify", "classification"),
    ("curate", "curation"),
    ("validate", "validation"),
    // manager
    ("coordinate", "coordination"),
    ("delegate", "delegation"),
    ("monitor", "monitoring"),
];

/// Map a canonical action verb to a catalog `task_type` id, when maintained.
pub fn task_type_for_verb(verb: &str) -> Option<&'static str> {
    VERB_TO_TASK_TYPE
        .iter()
        .find(|(candidate, _)| *candidate == verb)
        .map(|(_, task_type)| *task_type)
}

/// A cost tier (`CostTier`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CostTier {
    /// Cheapest.
    Low,
    /// Mid.
    Medium,
    /// Expensive.
    High,
    /// Most expensive.
    Premium,
}

impl CostTier {
    /// The tier's desirability weight (`_COST_DESIRABILITY`).
    pub fn desirability(self) -> f64 {
        match self {
            Self::Low => 1.0,
            Self::Medium => 0.66,
            Self::High => 0.33,
            Self::Premium => 0.0,
        }
    }

    /// The tier's ordinal rank (`_COST_TIER_ORDER`).
    pub fn order(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Premium => 3,
        }
    }
}

/// A confidence level (`Confidence`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Low confidence.
    Low,
    /// Medium confidence.
    Medium,
    /// High confidence.
    High,
}

impl Confidence {
    /// The confidence's desirability weight (`_RISK_DESIRABILITY`).
    pub fn desirability(self) -> f64 {
        match self {
            Self::High => 1.0,
            Self::Medium => 0.66,
            Self::Low => 0.33,
        }
    }
}

/// A latency tier (`LatencyTier`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyTier {
    /// Fast.
    Low,
    /// Mid.
    Medium,
    /// Slow.
    High,
}

impl LatencyTier {
    /// The tier's desirability weight (`_LATENCY_DESIRABILITY`).
    pub fn desirability(self) -> f64 {
        match self {
            Self::Low => 1.0,
            Self::Medium => 0.66,
            Self::High => 0.33,
        }
    }
}

/// The routing objective (`RoutingObjective`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingObjective {
    /// The capability lever: strongest `task_fit` wins outright.
    QualityFirst,
    /// The plain weighted sum.
    Balanced,
    /// The plain weighted sum (cost-heavy weights).
    CostFirst,
}

/// The routing weights (`RoutingWeights`).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoutingWeights {
    /// Quality weight.
    pub quality: f64,
    /// Cost weight.
    pub cost: f64,
    /// Risk weight.
    pub risk: f64,
    /// Latency weight.
    pub latency: f64,
}

/// One per-task fit entry (`TaskFit`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskFit {
    /// The catalog `task_type` this entry scores.
    pub task_type: String,
    /// The 0..=1 fit score.
    pub score: f64,
    /// Optional confidence.
    #[serde(default)]
    pub confidence: Option<Confidence>,
    /// Optional rationale.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// A model's cost block.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelCost {
    /// The cost tier.
    pub tier: CostTier,
}

/// One catalog model entry (`ModelEntry`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelEntry {
    /// The model id.
    pub id: String,
    /// The cost block.
    pub cost: ModelCost,
    /// Optional latency tier.
    #[serde(default)]
    pub latency_tier: Option<LatencyTier>,
    /// The per-task fits.
    #[serde(default)]
    pub task_fit: Vec<TaskFit>,
}

/// One per-task cost-tier cap (`TierConstraint`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TierConstraint {
    /// The `task_type` this cap applies to.
    pub task_type: String,
    /// The highest allowed cost tier.
    pub max_tier: CostTier,
}

/// The override policy block (`OverridePolicy`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OverridePolicy {
    /// The declared mode (`advisory` is the only exercised one).
    pub mode: String,
}

/// The routing policy block (`RoutingPolicy`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoutingPolicy {
    /// The objective.
    pub objective: RoutingObjective,
    /// The weights.
    pub weights: RoutingWeights,
    /// The per-task caps.
    #[serde(default)]
    pub tier_constraints: Vec<TierConstraint>,
    /// The override policy.
    pub override_policy: OverridePolicy,
}

/// The loaded catalog (`ModelToTaskType`).
///
/// Only the fields the evaluator reads are typed; the catalog's documentation
/// blocks (`task_types` titles/descriptions, `source_snapshot`,
/// `generated_at`, freshness policy) are not needed to score a dispatch and are
/// deliberately not modelled here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelRoutingCatalog {
    /// The catalog schema version.
    pub schema_version: String,
    /// The routing policy.
    pub routing_policy: RoutingPolicy,
    /// The model entries.
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

/// One provenance-tagged routing candidate (`RoutingCandidate`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoutingCandidate {
    /// The recommended model id.
    pub model_id: String,
    /// `catalog` or `profile`.
    pub source: String,
    /// The catalog-computed weighted score (absent on profile candidates).
    pub score: Option<f64>,
    /// The fit rationale.
    pub rationale: Option<String>,
    /// The profile-declared effort.
    pub effort: Option<String>,
}

/// The resolved step-offer seam value (`OfferResolution`).
///
/// `charter.offering.missions.step_offer_seam.resolve_model_tier_offer`:
/// an operator/runtime **override** always wins over a doctrine-authored step
/// offer, the raw step offer stays observable, and neither value present means
/// there is no preference at all (`None`, not an empty resolution).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelTierOffer {
    /// The value callers should act on.
    pub effective: Option<String>,
    /// `override` when a charter/runtime override decided the value, `offer`
    /// when the step's own advisory offer was used unmodified.
    pub source: String,
    /// The raw step-authored offer, kept even when it was overridden.
    pub offer: Option<String>,
}

/// Resolve a step's `recommended_model_tier` offer against an override.
pub fn resolve_model_tier_offer(
    step_offer: Option<&str>,
    override_value: Option<&str>,
) -> ModelTierOffer {
    if let Some(override_value) = override_value {
        return ModelTierOffer {
            effective: Some(override_value.to_owned()),
            source: "override".to_owned(),
            offer: step_offer.map(str::to_owned),
        };
    }
    ModelTierOffer {
        effective: step_offer.map(str::to_owned),
        source: "offer".to_owned(),
        offer: step_offer.map(str::to_owned),
    }
}

/// The evaluator's output (`RoutingRecommendation`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoutingRecommendation {
    /// The catalog `task_type` that was scored.
    pub task_type: String,
    /// The objective that produced the ranking.
    pub objective: String,
    /// The declared override mode.
    pub override_mode: String,
    /// The candidates, catalog pick first.
    pub candidates: Vec<RoutingCandidate>,
    /// The resolved step model-tier offer. `None` when the caller supplied
    /// neither a step offer nor an override, matching the reference default.
    pub model_tier: Option<ModelTierOffer>,
}

impl RoutingRecommendation {
    /// The catalog-computed pick, when there is one.
    pub fn catalog_candidate(&self) -> Option<&RoutingCandidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.source == "catalog")
    }
}

/// The profile projection the evaluator reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileRoutingView {
    /// The profile's declared preferred model (`preferred_model` / `model`).
    pub preferred_model: Option<String>,
    /// The profile's declared effort.
    pub effort: Option<String>,
}

/// The objective's string spelling (`RoutingObjective.value`).
pub fn objective_str(objective: RoutingObjective) -> &'static str {
    match objective {
        RoutingObjective::QualityFirst => "quality_first",
        RoutingObjective::Balanced => "balanced",
        RoutingObjective::CostFirst => "cost_first",
    }
}

/// Score the catalog for `(task_type, profile)`.
///
/// Deterministic and total: an unmatched `task_type` or a catalog with no
/// matching model yields a recommendation with no catalog candidate rather than
/// an error.
pub fn evaluate(
    catalog: &ModelRoutingCatalog,
    task_type: &str,
    profile: &ProfileRoutingView,
) -> RoutingRecommendation {
    evaluate_with_offers(catalog, task_type, profile, None, None)
}

/// [`evaluate`] with the reference's optional advisory step offer and override.
///
/// `charter.offering.model_task_routing.evaluator.evaluate`'s
/// `recommended_model_tier` / `model_tier_override` keyword arguments: either
/// supplied value produces a resolved `model_tier`; neither leaves it `None`.
/// The offer never changes the catalog/profile candidates — it is advisory and
/// override-wins, exactly like the reference seam.
pub fn evaluate_with_offers(
    catalog: &ModelRoutingCatalog,
    task_type: &str,
    profile: &ProfileRoutingView,
    recommended_model_tier: Option<&str>,
    model_tier_override: Option<&str>,
) -> RoutingRecommendation {
    let catalog_pick = best_catalog_candidate(catalog, task_type);
    let profile_pick = profile_candidate(profile);
    let candidates: Vec<RoutingCandidate> = catalog_pick.into_iter().chain(profile_pick).collect();
    let model_tier = if recommended_model_tier.is_none() && model_tier_override.is_none() {
        None
    } else {
        Some(resolve_model_tier_offer(
            recommended_model_tier,
            model_tier_override,
        ))
    };
    RoutingRecommendation {
        task_type: task_type.to_string(),
        objective: objective_str(catalog.routing_policy.objective).to_string(),
        override_mode: catalog.routing_policy.override_policy.mode.clone(),
        candidates,
        model_tier,
    }
}

fn tier_allowed(model: &ModelEntry, task_type: &str, constraints: &[TierConstraint]) -> bool {
    match constraints
        .iter()
        .find(|constraint| constraint.task_type == task_type)
    {
        None => true,
        Some(constraint) => model.cost.tier.order() <= constraint.max_tier.order(),
    }
}

fn weighted_score(model: &ModelEntry, fit: &TaskFit, weights: &RoutingWeights) -> f64 {
    let risk = fit.confidence.map(Confidence::desirability).unwrap_or(0.5);
    let latency = model
        .latency_tier
        .map(LatencyTier::desirability)
        .unwrap_or(0.5);
    weights.quality * fit.score
        + weights.cost * model.cost.tier.desirability()
        + weights.risk * risk
        + weights.latency * latency
}

fn best_catalog_candidate(
    catalog: &ModelRoutingCatalog,
    task_type: &str,
) -> Option<RoutingCandidate> {
    let policy = &catalog.routing_policy;
    let mut best: Option<(&ModelEntry, &TaskFit, f64)> = None;
    for model in &catalog.models {
        if !tier_allowed(model, task_type, &policy.tier_constraints) {
            continue;
        }
        let Some(fit) = model.task_fit.iter().find(|fit| fit.task_type == task_type) else {
            continue;
        };
        let weighted = weighted_score(model, fit, &policy.weights);
        // `max()` keeps the first of equal keys; the reference's `max` also
        // keeps the first, so iteration order is the tie-break in both.
        let replace = match best {
            None => true,
            Some((_, best_fit, best_weighted)) => {
                if policy.objective == RoutingObjective::QualityFirst {
                    (fit.score, weighted) > (best_fit.score, best_weighted)
                } else {
                    weighted > best_weighted
                }
            }
        };
        if replace {
            best = Some((model, fit, weighted));
        }
    }
    best.map(|(model, fit, weighted)| RoutingCandidate {
        model_id: model.id.clone(),
        source: "catalog".to_string(),
        score: Some(weighted),
        rationale: fit.rationale.clone(),
        effort: None,
    })
}

fn profile_candidate(profile: &ProfileRoutingView) -> Option<RoutingCandidate> {
    let model_id = profile.preferred_model.clone()?;
    Some(RoutingCandidate {
        model_id,
        source: "profile".to_string(),
        score: None,
        rationale: None,
        effort: profile.effort.clone(),
    })
}

//! `ProfileInvocationExecutor`: the single execution primitive for
//! profile-governed invocations.
//!
//! Ports `src/specify_cli/invocation/executor.py` over the crate's existing
//! durable authorities: [`InvocationWriter`] (records, index, closure spine),
//! the [`OpCommitService`] admission/commit pair, the [`ActionRouter`], the
//! kernel clock/clock-derived digest, and the canonical [`ModeOfWork`].
//!
//! **First-load safety.** The governance context is built by the caller's
//! [`GovernancePort`] with `mark_loaded=False` semantics: this executor never
//! claims a first-load token and never writes `context-state.json`. The port is
//! documented as a read-only seam for exactly that reason.
//!
//! **What this type does not do.** It spawns no LLM call and generates no text.
//! [`ProfileInvocationExecutor::invoke`] returns synchronously with invocation
//! guidance; the closing half is
//! [`ProfileInvocationExecutor::complete_invocation`], which records the real
//! outcome and performs the same best-effort Op-record commit as the reference.
//!
//! **Composition, not duplication.** [`super::step_contract::StepContractExecutor`]
//! composes contracts out of these invocations and closes each one exactly once;
//! the governed `next` dispatch calls *that* executor and never this one
//! directly (the reference's C-001).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};
use sk_doctrine::profile_id_label;
use sk_kernel::clock::now_utc_iso;
use sk_kernel::content_digest::sha256_hex;
use sk_kernel::python_repr_codepoints;

use crate::error::InvocationError;
use crate::legacy_json::PythonString;
use crate::modes::{ModeOfWork, RecordedMode};
use crate::op_commit::{OpClosedBy, OpCommitOutcome, OpCommitService};
use crate::op_custody::{OpWriteAction, OpWriteRefusal, OpWriteRefused};
pub use crate::python_json::PythonJsonValue;
use crate::python_json::python_float_repr;
use crate::record::{
    ClosedBy, InvocationId, OpCompletedEvent, OpCompletedFields, OpOutcome, OpStartedEvent,
    OpStartedFields,
};
use crate::router::{ActionRouter, RoutableProfile, RouterDecision};
use crate::writer::{CorrelationLink, InvocationWriter};

pub mod model_routing;

pub use model_routing::{
    CostTier, LatencyTier, ModelCost, ModelEntry, ModelRoutingCatalog, ModelTierOffer,
    OverridePolicy, ProfileRoutingView, RoutingCandidate, RoutingObjective, RoutingPolicy,
    RoutingRecommendation, RoutingWeights, TaskFit, evaluate, evaluate_with_offers,
    resolve_model_tier_offer, task_type_for_verb,
};

/// The reference's `DEFAULT_ROLE_CAPABILITIES` default action, with the
/// reference's `"review"` fallback for a custom role.
pub fn derive_action_from_role(role: &str) -> &'static str {
    sk_doctrine::role_default_verb(role).unwrap_or("review")
}

/// [`derive_action_from_role`] over the exact role.
///
/// The reference looks the `Role` (a `str` subclass) up in a string-keyed
/// table; a non-scalar role is not in that table, so the reference falls back to
/// `"review"` rather than scalar-projecting the role into a match.
pub fn derive_action_from_role_exact(role: &PythonString) -> &'static str {
    match role.to_scalar_string() {
        Ok(scalar) => derive_action_from_role(&scalar),
        Err(_) => "review",
    }
}

/// Whether a recorded mode may carry a Tier 2 evidence artifact.
pub fn mode_permits_evidence(mode: &RecordedMode) -> bool {
    mode.permits_evidence()
}

/// A read-only view of one profile in the invocation catalogue.
///
/// Every authored string is carried as the kernel's exact [`PythonString`], and
/// the role/keyword/verb lists are the routing projection's own values (already
/// normalized by the one Source role authority) — this view never re-cases,
/// scalar-projects or re-parses authored data. It is a projection of the
/// canonical catalogue, not a second profile model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogProfile {
    /// The exact authored profile id.
    pub profile_id: PythonString,
    /// The display name, when declared.
    pub name: Option<PythonString>,
    /// The primary role (`roles[0]`), already normalized like `Role`.
    pub role: PythonString,
    /// The `routing-priority` (`0..=100`).
    pub routing_priority: i32,
    /// The declared preferred model, when one is.
    pub preferred_model: Option<PythonString>,
    /// The declared effort, when one is.
    pub effort: Option<PythonString>,
    /// The authored domain keywords.
    pub domain_keywords: Vec<PythonString>,
    /// The authored canonical verbs.
    pub canonical_verbs: Vec<PythonString>,
}

impl CatalogProfile {
    /// The scalar projection of the id, when it has one.
    ///
    /// A real capability boundary: a lone surrogate cannot be named by a Rust
    /// `String` (the Op record and the runtime payloads are `String`-keyed).
    /// Callers must surface that boundary as a typed refusal, never drop the
    /// profile silently.
    pub fn scalar_id(&self) -> Option<String> {
        self.profile_id.to_scalar_string().ok()
    }

    /// The display name's scalar projection, falling back to the id text.
    pub fn scalar_name(&self) -> Option<String> {
        self.name
            .as_ref()
            .and_then(|name| name.to_scalar_string().ok())
    }

    /// The router's exact caller-supplied projection of this profile.
    pub fn routable(&self) -> RoutableProfile {
        RoutableProfile::new(
            self.profile_id.clone(),
            self.role.clone(),
            self.routing_priority,
            self.domain_keywords.clone(),
            self.canonical_verbs.clone(),
        )
    }

    /// The advisory model-routing projection (scalar hints only; an advisory
    /// recommendation is never a dispatch decision).
    pub fn routing_view(&self) -> ProfileRoutingView {
        ProfileRoutingView {
            preferred_model: self
                .preferred_model
                .as_ref()
                .and_then(|model| model.to_scalar_string().ok()),
            effort: self
                .effort
                .as_ref()
                .and_then(|effort| effort.to_scalar_string().ok()),
        }
    }
}

/// The invocation catalogue the executor resolves profile hints against.
///
/// The real implementation is [`RegistryCatalog`] over the canonical
/// `crate::catalogue::ProfileRegistry` (activation gate, legacy overlay,
/// language scope, exact strings). The trait exists so the executor's routing
/// stays a pure function of a caller-supplied catalogue value, exactly like the
/// router's own snapshot contract.
pub trait InvocationCatalog {
    /// The exact router snapshot, in the catalogue's own order.
    fn routing_profiles(&self) -> Vec<RoutableProfile>;
    /// One profile's exact facts, when it is resolvable.
    fn profile(&self, profile_id: &PythonString) -> Option<CatalogProfile>;
    /// One scalar profile id's exact facts, when it is resolvable.
    fn profile_str(&self, profile_id: &str) -> Option<CatalogProfile> {
        self.profile(&PythonString::from_scalars(profile_id))
    }
}

/// The real catalogue: the canonical `ProfileRegistry`, projected.
///
/// No gate, overlay, language filter or normalization is re-applied here — the
/// registry already resolved all of them (Track A's authority).
#[derive(Clone, Debug)]
pub struct RegistryCatalog<'a> {
    registry: &'a crate::catalogue::ProfileRegistry,
}

impl<'a> RegistryCatalog<'a> {
    /// Adapt one loaded registry.
    pub fn new(registry: &'a crate::catalogue::ProfileRegistry) -> Self {
        Self { registry }
    }

    /// The underlying registry.
    pub fn registry(&self) -> &'a crate::catalogue::ProfileRegistry {
        self.registry
    }
}

impl InvocationCatalog for RegistryCatalog<'_> {
    fn routing_profiles(&self) -> Vec<RoutableProfile> {
        self.registry.routing_profiles()
    }

    fn profile(&self, profile_id: &PythonString) -> Option<CatalogProfile> {
        self.registry.get(profile_id).map(project_profile)
    }

    fn profile_str(&self, profile_id: &str) -> Option<CatalogProfile> {
        self.registry.get_str(profile_id).map(project_profile)
    }
}

/// Project one canonical `AgentProfile` onto the executor's exact view.
///
/// Public so the composition root's catalogue adapter (and the `profiles` view)
/// project a profile exactly the way this executor does, with no second mapping.
pub fn project_profile(profile: &sk_doctrine::AgentProfile) -> CatalogProfile {
    CatalogProfile {
        profile_id: profile.id.clone(),
        name: profile.name(),
        role: profile.role().unwrap_or_default(),
        routing_priority: profile.routing_priority() as i32,
        preferred_model: profile.preferred_model(),
        effort: profile.effort(),
        domain_keywords: profile.domain_keywords(),
        canonical_verbs: profile.canonical_verbs(),
    }
}

/// A catalogue over a fixed list (the composition root's tests and fakes).
#[derive(Clone, Debug, Default)]
pub struct StaticCatalog {
    profiles: Vec<CatalogProfile>,
}

impl StaticCatalog {
    /// Build a catalogue from already-resolved profiles.
    pub fn new(profiles: Vec<CatalogProfile>) -> Self {
        Self { profiles }
    }
}

impl InvocationCatalog for StaticCatalog {
    fn routing_profiles(&self) -> Vec<RoutableProfile> {
        self.profiles.iter().map(CatalogProfile::routable).collect()
    }

    fn profile(&self, profile_id: &PythonString) -> Option<CatalogProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.profile_id == *profile_id)
            .cloned()
    }
}

/// The assembled governance context (`build_charter_context(..., mark_loaded=False)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceContext {
    /// The context text, when a context was assembled.
    pub text: Option<String>,
    /// The reference mode (`missing`, `bootstrap`, `compact`, …).
    pub mode: String,
}

impl GovernanceContext {
    /// A governance context that could not be assembled.
    pub fn missing() -> Self {
        Self {
            text: None,
            mode: "missing".to_string(),
        }
    }

    /// Whether the context was available (`mode != "missing"`).
    pub fn available(&self) -> bool {
        self.mode != "missing"
    }
}

/// The governance-context port (`build_charter_context`, `mark_loaded=False`).
pub trait GovernancePort {
    /// Assemble the read-only governance context for `(profile, action)`.
    ///
    /// Implementations MUST pass `mark_loaded=False` semantics: no
    /// `context-state.json` write, no first-load claim.
    fn build_context(&self, profile_id: &PythonString, action: &str) -> GovernanceContext;
}

/// The empty-charter generic fallback decision (`resolve_generic_fallback`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyCharterFallback {
    /// The pinned profile id (`generic-agent`).
    pub profile_id: String,
    /// The token-derived canonical action.
    pub action: String,
    /// The recorded match reason.
    pub match_reason: String,
}

/// The empty-charter seam (`resolve_generic_fallback`).
pub trait EmptyCharterPort {
    /// Return the generic fallback when the compiled charter is wholly absent
    /// and nothing else makes the project router-routable.
    fn fallback(&self, request_text: &str) -> Option<EmptyCharterFallback>;
}

/// One glossary conflict body (`semantic_conflict_to_dict`), in insertion order.
pub type GlossaryConflictJson = PythonJsonValue;

/// One candidate sense of a serialised conflict.
#[derive(Clone, Debug, PartialEq)]
pub struct ConflictCandidateView {
    /// The sense's canonical surface text.
    pub surface: String,
    /// The sense's glossary scope.
    pub scope: String,
    /// The sense's definition.
    pub definition: String,
    /// The sense's authored confidence.
    pub confidence: f64,
}

/// The reference `semantic_conflict_to_dict` body, ready for exact rendering.
///
/// The reference builds a plain Python dict in source order
/// (`term`, `conflict_type`, `severity`, `confidence`, `candidate_senses`,
/// `context`); a `serde_json::Map` would sort those keys and change the wire
/// bytes for a non-empty conflict.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticConflictView {
    /// The normalised token that matched.
    pub surface_text: String,
    /// The conflict classification's `value`.
    pub conflict_type: String,
    /// The scored severity's `value`.
    pub severity: String,
    /// Confidence in conflict detection.
    pub confidence: f64,
    /// The matching senses, in glossary order.
    pub candidate_senses: Vec<ConflictCandidateView>,
    /// The usage context.
    pub context: String,
}

impl SemanticConflictView {
    /// `semantic_conflict_to_dict` as an exact `json.dumps` value.
    pub fn to_python_json(&self) -> PythonJsonValue {
        PythonJsonValue::Object(vec![
            (
                "term".to_owned(),
                PythonJsonValue::Object(vec![(
                    "surface_text".to_owned(),
                    PythonJsonValue::scalar(&self.surface_text),
                )]),
            ),
            (
                "conflict_type".to_owned(),
                PythonJsonValue::scalar(&self.conflict_type),
            ),
            (
                "severity".to_owned(),
                PythonJsonValue::scalar(&self.severity),
            ),
            (
                "confidence".to_owned(),
                PythonJsonValue::Number(python_float_repr(self.confidence)),
            ),
            (
                "candidate_senses".to_owned(),
                PythonJsonValue::Array(
                    self.candidate_senses
                        .iter()
                        .map(|candidate| {
                            PythonJsonValue::Object(vec![
                                (
                                    "surface".to_owned(),
                                    PythonJsonValue::scalar(&candidate.surface),
                                ),
                                (
                                    "scope".to_owned(),
                                    PythonJsonValue::scalar(&candidate.scope),
                                ),
                                (
                                    "definition".to_owned(),
                                    PythonJsonValue::scalar(&candidate.definition),
                                ),
                                (
                                    "confidence".to_owned(),
                                    PythonJsonValue::Number(python_float_repr(
                                        candidate.confidence,
                                    )),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("context".to_owned(), PythonJsonValue::scalar(&self.context)),
        ])
    }
}

/// The glossary chokepoint observation bundle (`GlossaryObservationBundle`).
#[derive(Clone, Debug, PartialEq)]
pub struct GlossaryObservation {
    /// `glossary:<id>` URNs for every token found.
    pub matched_urns: Vec<String>,
    /// Conflicts scored `high`.
    pub high_severity: Vec<GlossaryConflictJson>,
    /// Every conflict detected.
    pub all_conflicts: Vec<GlossaryConflictJson>,
    /// Distinct normalised tokens examined.
    pub tokens_checked: u64,
    /// Scan duration in milliseconds.
    pub duration_ms: f64,
    /// Set only for the error envelope.
    pub error_msg: Option<String>,
}

impl GlossaryObservation {
    /// The empty bundle (a scan that matched nothing).
    pub fn empty() -> Self {
        Self {
            matched_urns: Vec::new(),
            high_severity: Vec::new(),
            all_conflicts: Vec::new(),
            tokens_checked: 0,
            duration_ms: 0.0,
            error_msg: None,
        }
    }
}

/// The glossary-scan port (`GlossaryChokepoint.run`). Never fails.
pub trait GlossaryScanPort {
    /// Scan `request_text`; an unmatched token is not an error.
    fn scan(&self, request_text: &str, invocation_id: &str, actor_id: &str) -> GlossaryObservation;
}

/// One invocation request.
#[derive(Clone, Debug)]
pub struct InvocationRequest<'a> {
    /// The caller's request text.
    pub request_text: &'a str,
    /// An explicit profile hint (`--profile`), exact.
    pub profile_hint: Option<&'a PythonString>,
    /// The acting agent identity.
    pub actor: &'a str,
    /// The Op's mode of work.
    pub mode_of_work: Option<ModeOfWork>,
    /// A caller-supplied action that replaces the role-default derivation.
    pub action_hint: Option<&'a str>,
    /// The Mission id, when the Op belongs to one.
    pub mission_id: Option<&'a str>,
    /// The work-package id, when the Op belongs to one.
    pub wp_id: Option<&'a str>,
}

impl<'a> InvocationRequest<'a> {
    /// A request for `request_text` by `actor`.
    pub fn new(request_text: &'a str, actor: &'a str) -> Self {
        Self {
            request_text,
            profile_hint: None,
            actor,
            mode_of_work: None,
            action_hint: None,
            mission_id: None,
            wp_id: None,
        }
    }
}

/// The machine-readable close contract attached to every open Op payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CloseContract {
    /// The exact closing command.
    pub command: String,
    /// The accepted outcomes.
    pub outcomes: Vec<String>,
    /// The evidence flag, omitted for non-evidence-eligible modes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_flag: Option<String>,
    /// The artifact flag.
    pub artifact_flag: String,
    /// The commit flag.
    pub commit_flag: String,
}

/// Build the close contract for an open Op (`build_close_contract`).
pub fn build_close_contract(invocation_id: &str, mode_of_work: Option<&str>) -> CloseContract {
    let recorded = mode_of_work
        .map(|raw| crate::modes::classify_mode_of_work(Some(&Value::String(raw.to_string()))));
    let permits = recorded.as_ref().map(mode_permits_evidence).unwrap_or(true);
    CloseContract {
        command: format!(
            "spec-kitty profile-invocation complete --invocation-id {invocation_id} --outcome <done|failed|abandoned>"
        ),
        outcomes: ["done", "failed", "abandoned"]
            .iter()
            .map(|outcome| (*outcome).to_string())
            .collect(),
        evidence_flag: permits.then(|| "--evidence".to_string()),
        artifact_flag: "--artifact".to_string(),
        commit_flag: "--commit".to_string(),
    }
}

/// The ephemeral response returned to CLI callers (`InvocationPayload`).
#[derive(Clone, Debug, PartialEq)]
pub struct InvocationPayload {
    /// The minted Op id (`""` on the dry-run path).
    pub invocation_id: String,
    /// The resolved profile id, exact (a dry run accepts a non-scalar id).
    pub profile_id: PythonString,
    /// The profile's display name, exact.
    pub profile_friendly_name: PythonString,
    /// The resolved canonical action.
    pub action: String,
    /// The assembled governance text.
    pub governance_context_text: Option<String>,
    /// The first 16 hex characters of the SHA-256 of the text.
    pub governance_context_hash: Option<String>,
    /// Whether governance context was available.
    pub governance_context_available: bool,
    /// The router confidence (`exact`, `canonical_verb`, …).
    pub router_confidence: Option<String>,
    /// The glossary observation bundle.
    pub glossary_observations: Option<GlossaryObservation>,
    /// How the Op was opened (not part of the serialised payload).
    pub mode_of_work: Option<String>,
    /// The advisory model-routing recommendation.
    pub recommendation: Option<RoutingRecommendation>,
    /// Whether the generic empty-charter fallback produced this dispatch.
    pub empty_charter_fallback: bool,
    /// Every non-winning router candidate.
    pub alternatives: Vec<RoutingAlternativeView>,
}

/// One serialised router alternative (`alternatives[]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingAlternativeView {
    /// The candidate profile id, exact.
    pub profile_id: PythonString,
    /// The action the candidate would run.
    pub action: String,
    /// The tier the candidate matched in.
    pub confidence: String,
    /// The reference match-reason text, exact.
    pub match_reason: PythonString,
}

impl InvocationPayload {
    /// Serialise `to_dict()`: every payload field, plus `status` and
    /// `close_contract`.
    ///
    /// `None` when a field holds a lone surrogate: `serde_json` cannot represent
    /// one. The exact wire is [`InvocationPayload::to_python_json`], which is the
    /// canonical `json.dumps` rendering the CLI writes.
    pub fn to_dict(&self) -> Option<Value> {
        self.envelope(true).to_serde()
    }

    /// Serialise `to_dry_run_dict()`: no `invocation_id`, no `mode_of_work`, no
    /// close contract, terminal `status = dry_run`.
    pub fn to_dry_run_dict(&self) -> Option<Value> {
        self.envelope(false).to_serde()
    }

    /// The exact `json.dumps` envelope.
    ///
    /// `include_invocation_id` selects the real (`status = "open"`, close
    /// contract) shape or the dry-run (`status = "dry_run"`) shape; both follow
    /// the reference's `__slots__` order and omit `mode_of_work`.
    pub fn to_python_json(&self, include_invocation_id: bool) -> PythonJsonValue {
        self.envelope(include_invocation_id)
    }

    fn envelope(&self, include_invocation_id: bool) -> PythonJsonValue {
        let mut entries: Vec<(String, PythonJsonValue)> = Vec::new();
        if include_invocation_id {
            entries.push((
                "invocation_id".to_owned(),
                PythonJsonValue::scalar(&self.invocation_id),
            ));
        }
        entries.push((
            "profile_id".to_owned(),
            PythonJsonValue::string(self.profile_id.clone()),
        ));
        entries.push((
            "profile_friendly_name".to_owned(),
            PythonJsonValue::string(self.profile_friendly_name.clone()),
        ));
        entries.push(("action".to_owned(), PythonJsonValue::scalar(&self.action)));
        entries.push((
            "governance_context_text".to_owned(),
            PythonJsonValue::optional_scalar(self.governance_context_text.as_deref()),
        ));
        entries.push((
            "governance_context_hash".to_owned(),
            PythonJsonValue::optional_scalar(self.governance_context_hash.as_deref()),
        ));
        entries.push((
            "governance_context_available".to_owned(),
            PythonJsonValue::Bool(self.governance_context_available),
        ));
        entries.push((
            "router_confidence".to_owned(),
            PythonJsonValue::optional_scalar(self.router_confidence.as_deref()),
        ));
        entries.push((
            "glossary_observations".to_owned(),
            self.glossary_observations
                .as_ref()
                .map(glossary_json)
                .unwrap_or(PythonJsonValue::Null),
        ));
        entries.push((
            "recommendation".to_owned(),
            self.recommendation
                .as_ref()
                .map(recommendation_json)
                .unwrap_or(PythonJsonValue::Null),
        ));
        entries.push((
            "empty_charter_fallback".to_owned(),
            PythonJsonValue::Bool(self.empty_charter_fallback),
        ));
        entries.push((
            "alternatives".to_owned(),
            PythonJsonValue::Array(
                self.alternatives
                    .iter()
                    .map(|alternative| {
                        PythonJsonValue::Object(vec![
                            (
                                "profile_id".to_owned(),
                                PythonJsonValue::string(alternative.profile_id.clone()),
                            ),
                            (
                                "action".to_owned(),
                                PythonJsonValue::scalar(&alternative.action),
                            ),
                            (
                                "confidence".to_owned(),
                                PythonJsonValue::scalar(&alternative.confidence),
                            ),
                            (
                                "match_reason".to_owned(),
                                PythonJsonValue::string(alternative.match_reason.clone()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ));
        if include_invocation_id {
            entries.push(("status".to_owned(), PythonJsonValue::scalar("open")));
            entries.push((
                "close_contract".to_owned(),
                close_contract_json(&build_close_contract(
                    &self.invocation_id,
                    self.mode_of_work.as_deref(),
                )),
            ));
        } else {
            entries.push(("status".to_owned(), PythonJsonValue::scalar("dry_run")));
        }
        PythonJsonValue::Object(entries)
    }
}

/// The reference `GlossaryObservationBundle.to_dict()` shape, in field order.
///
/// Public so the standalone advisory surface can be rendered (and verified)
/// without an executor instance; the dispatch path uses this same function.
pub fn glossary_json(observation: &GlossaryObservation) -> PythonJsonValue {
    PythonJsonValue::Object(vec![
        (
            "matched_urns".to_owned(),
            PythonJsonValue::Array(
                observation
                    .matched_urns
                    .iter()
                    .map(|urn| PythonJsonValue::scalar(urn))
                    .collect(),
            ),
        ),
        (
            "high_severity".to_owned(),
            PythonJsonValue::Array(observation.high_severity.clone()),
        ),
        (
            "all_conflicts".to_owned(),
            PythonJsonValue::Array(observation.all_conflicts.clone()),
        ),
        (
            "tokens_checked".to_owned(),
            PythonJsonValue::Number(observation.tokens_checked.to_string()),
        ),
        (
            "duration_ms".to_owned(),
            PythonJsonValue::Number(python_float_repr(observation.duration_ms)),
        ),
        (
            "error_msg".to_owned(),
            PythonJsonValue::optional_scalar(observation.error_msg.as_deref()),
        ),
    ])
}

/// The reference `RoutingRecommendation` dataclass, in field order.
///
/// Public for the standalone advisory API and its conformance test; the Op
/// payload uses this same function, so there is one wire shape.
pub fn recommendation_json(recommendation: &RoutingRecommendation) -> PythonJsonValue {
    PythonJsonValue::Object(vec![
        (
            "task_type".to_owned(),
            PythonJsonValue::scalar(&recommendation.task_type),
        ),
        (
            "objective".to_owned(),
            PythonJsonValue::scalar(&recommendation.objective),
        ),
        (
            "override_mode".to_owned(),
            PythonJsonValue::scalar(&recommendation.override_mode),
        ),
        (
            "candidates".to_owned(),
            PythonJsonValue::Array(
                recommendation
                    .candidates
                    .iter()
                    .map(|candidate| {
                        PythonJsonValue::Object(vec![
                            (
                                "model_id".to_owned(),
                                PythonJsonValue::scalar(&candidate.model_id),
                            ),
                            (
                                "source".to_owned(),
                                PythonJsonValue::scalar(&candidate.source),
                            ),
                            (
                                "score".to_owned(),
                                candidate
                                    .score
                                    .map(|score| PythonJsonValue::Number(python_float_repr(score)))
                                    .unwrap_or(PythonJsonValue::Null),
                            ),
                            (
                                "rationale".to_owned(),
                                PythonJsonValue::optional_scalar(candidate.rationale.as_deref()),
                            ),
                            (
                                "effort".to_owned(),
                                PythonJsonValue::optional_scalar(candidate.effort.as_deref()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "model_tier".to_owned(),
            recommendation
                .model_tier
                .as_ref()
                .map(|offer| {
                    // `OfferResolution` field order: effective, source, offer.
                    PythonJsonValue::Object(vec![
                        (
                            "effective".to_owned(),
                            PythonJsonValue::optional_scalar(offer.effective.as_deref()),
                        ),
                        ("source".to_owned(), PythonJsonValue::scalar(&offer.source)),
                        (
                            "offer".to_owned(),
                            PythonJsonValue::optional_scalar(offer.offer.as_deref()),
                        ),
                    ])
                })
                .unwrap_or(PythonJsonValue::Null),
        ),
    ])
}

/// The `close_contract` dict, in the reference's insertion order.
fn close_contract_json(contract: &CloseContract) -> PythonJsonValue {
    let mut entries = vec![
        (
            "command".to_owned(),
            PythonJsonValue::scalar(&contract.command),
        ),
        (
            "outcomes".to_owned(),
            PythonJsonValue::Array(
                contract
                    .outcomes
                    .iter()
                    .map(|outcome| PythonJsonValue::scalar(outcome))
                    .collect(),
            ),
        ),
    ];
    if let Some(flag) = &contract.evidence_flag {
        entries.push(("evidence_flag".to_owned(), PythonJsonValue::scalar(flag)));
    }
    entries.push((
        "artifact_flag".to_owned(),
        PythonJsonValue::scalar(&contract.artifact_flag),
    ));
    entries.push((
        "commit_flag".to_owned(),
        PythonJsonValue::scalar(&contract.commit_flag),
    ));
    PythonJsonValue::Object(entries)
}

/// The executor's operation surface.
///
/// `derive(Debug, thiserror::Error)` carries the reference's message text; the
/// variants mirror the reference's exception classes one-for-one (plus the
/// `OpStartedEvent` string-validation failure, which the reference raises as a
/// raw Pydantic `ValidationError`).
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// The catalogue resolved no profile for the hint.
    #[error("Profile '{0}' not found.")]
    ProfileNotFound(String),
    /// No hint and no catalogue/router was configured.
    #[error(
        "No profile_hint and no router configured. Use 'spec-kitty dispatch \"<request>\" --profile <profile>' or supply a router."
    )]
    NoRouter,
    /// The router refused the request (`ROUTER_AMBIGUOUS` / `ROUTER_NO_MATCH`).
    #[error("{message}")]
    Router {
        /// The router error code.
        code: String,
        /// The router's message.
        message: String,
        /// The considered candidates.
        candidates: Vec<RoutingAlternativeView>,
        /// The router's suggestion text.
        suggestion: String,
    },
    /// The Op may not be written on this branch.
    #[error(transparent)]
    OpWriteRefused(#[from] OpWriteRefused),
    /// The effect-free admission probe could not answer.
    #[error("Op write admission probe failed: {0}")]
    Admission(String),
    /// The durable record surface refused.
    #[error(transparent)]
    Invocation(#[from] InvocationError),
    /// The Op is already closed.
    #[error("Invocation {0} is already closed.")]
    AlreadyClosed(String),
    /// `--evidence` on an advisory/query Op.
    #[error(
        "Invalid invocation mode for evidence: invocation {invocation_id} has mode {mode:?}, which does not permit a Tier 2 evidence artifact."
    )]
    InvalidModeForEvidence {
        /// The invocation id.
        invocation_id: String,
        /// The recorded mode's raw spelling.
        mode: String,
    },
    /// `--evidence` on an Op whose recorded mode cannot be read at all.
    #[error(
        "Undetermined invocation mode for evidence: invocation {invocation_id} records mode_of_work={mode}, which is not a mode this reader understands."
    )]
    UndeterminedModeForEvidence {
        /// The invocation id.
        invocation_id: String,
        /// The unreadable recorded value.
        mode: String,
    },
    /// The evidence content is not UTF-8 text.
    #[error("Unsupported evidence for invocation {invocation_id}: {reason}")]
    UnsupportedEvidence {
        /// The invocation id.
        invocation_id: String,
        /// Why the evidence was refused.
        reason: String,
    },
    /// An evidence/artifact write failed.
    #[error("Could not write evidence artifact: {0}")]
    Evidence(String),
    /// `OpStartedEvent` validation refused a field, exactly like the reference's
    /// constrained Pydantic `str` (`min_length=1`, UTF-8 encodable).
    #[error("1 validation error for OpStartedEvent\n{field}\n  {message}")]
    RecordValidation {
        /// The refused Pydantic field name.
        field: String,
        /// The reference's error text.
        message: String,
    },
}

/// The single execution primitive for profile-governed invocations.
pub struct ProfileInvocationExecutor<'a> {
    repo_root: PathBuf,
    record_root: PathBuf,
    writer: InvocationWriter,
    commits: OpCommitService,
    catalog: Option<&'a dyn InvocationCatalog>,
    governance: Option<&'a dyn GovernancePort>,
    glossary: Option<&'a dyn GlossaryScanPort>,
    empty_charter: Option<&'a dyn EmptyCharterPort>,
    model_catalog: Option<&'a ModelRoutingCatalog>,
    last_op_commit: Option<OpCommitOutcome>,
}

/// One close request (`complete_invocation`).
///
/// `closed_by` is explicit and default-free, mirroring the reference's
/// keyword-only argument: a default would let the doctor sweep misattribute a
/// close.
pub struct CompletionRequest<'a> {
    /// The Op id to close.
    pub invocation_id: &'a str,
    /// The required terminal outcome.
    pub outcome: OpOutcome,
    /// A Tier 2 evidence reference (file path or inline text).
    pub evidence_ref: Option<&'a str>,
    /// Artifact correlation links to append.
    pub artifact_refs: &'a [String],
    /// A commit correlation link to append.
    pub commit_sha: Option<&'a str>,
    /// Who closed the Op.
    pub closed_by: ClosedBy,
}

impl<'a> CompletionRequest<'a> {
    /// An agent close with no evidence or links.
    pub fn agent(invocation_id: &'a str, outcome: OpOutcome) -> Self {
        Self {
            invocation_id,
            outcome,
            evidence_ref: None,
            artifact_refs: &[],
            commit_sha: None,
            closed_by: ClosedBy::Agent,
        }
    }
}

/// The result of a close: the appended event, the commit outcome and any
/// promoted evidence directory.
#[derive(Clone, Debug)]
pub struct CompletionOutcome {
    /// The appended `completed` event.
    pub completed: OpCompletedEvent,
    /// The best-effort Op-record commit outcome.
    pub op_commit: Option<OpCommitOutcome>,
    /// The promoted evidence directory, when one was written.
    pub evidence_dir: Option<PathBuf>,
}

impl<'a> ProfileInvocationExecutor<'a> {
    /// Build an executor writing Op records under `record_root`.
    pub fn new(record_root: impl Into<PathBuf>, repo_root: impl Into<PathBuf>) -> Self {
        let record_root = record_root.into();
        let repo_root = repo_root.into();
        let writer = InvocationWriter::new(record_root.clone());
        let commits = OpCommitService::new(repo_root.clone(), writer.clone());
        Self {
            repo_root,
            record_root,
            writer,
            commits,
            catalog: None,
            governance: None,
            glossary: None,
            empty_charter: None,
            model_catalog: None,
            last_op_commit: None,
        }
    }

    /// Attach the canonical profile catalogue.
    pub fn with_catalog(mut self, catalog: &'a dyn InvocationCatalog) -> Self {
        self.catalog = Some(catalog);
        self
    }

    /// Attach the governance-context port (`mark_loaded=False`).
    pub fn with_governance(mut self, governance: &'a dyn GovernancePort) -> Self {
        self.governance = Some(governance);
        self
    }

    /// Attach the glossary chokepoint.
    pub fn with_glossary(mut self, glossary: &'a dyn GlossaryScanPort) -> Self {
        self.glossary = Some(glossary);
        self
    }

    /// Attach the empty-charter generic-fallback seam.
    pub fn with_empty_charter(mut self, empty_charter: &'a dyn EmptyCharterPort) -> Self {
        self.empty_charter = Some(empty_charter);
        self
    }

    /// Attach the advisory model-routing catalog.
    pub fn with_model_catalog(mut self, catalog: &'a ModelRoutingCatalog) -> Self {
        self.model_catalog = Some(catalog);
        self
    }

    /// The project root governance is read from.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// The checkout that owns the Op records this executor writes.
    pub fn record_root(&self) -> &Path {
        &self.record_root
    }

    /// The writer this executor records through.
    pub fn writer(&self) -> &InvocationWriter {
        &self.writer
    }

    /// The outcome of the most recent Op-record auto-commit.
    pub fn last_op_commit(&self) -> Option<&OpCommitOutcome> {
        self.last_op_commit.as_ref()
    }

    /// The invocation catalogue (`invoke` resolves `profile_hint` against it).
    pub fn list_available_profiles(&self) -> Vec<CatalogProfile> {
        match self.catalog {
            Some(catalog) => catalog
                .routing_profiles()
                .iter()
                .filter_map(|profile| catalog.profile(&profile.profile_id))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Say why no Op record may be written here, before anything is.
    pub fn op_write_refusal(&mut self) -> Result<Option<OpWriteRefusal>, ExecutorError> {
        self.commits
            .op_write_refusal()
            .map_err(|error| ExecutorError::Admission(error.to_string()))
    }

    /// Refuse the write when admission does not admit it (`require_op_write_admitted`).
    pub fn require_op_write_admitted(
        &mut self,
        action: OpWriteAction,
    ) -> Result<(), ExecutorError> {
        if let Some(refusal) = self.op_write_refusal()? {
            return Err(ExecutorError::OpWriteRefused(OpWriteRefused::new(
                refusal,
                &self.record_root,
                action,
            )));
        }
        Ok(())
    }

    /// Route the request, load governance context, write the `started` record
    /// and return the payload. Spawns no LLM call; leaves the Op open.
    pub fn invoke(
        &mut self,
        request: &InvocationRequest<'_>,
    ) -> Result<InvocationPayload, ExecutorError> {
        // #6564: admission precedes every write of the invoke path, the glossary
        // events of the routing scan included.
        self.require_op_write_admitted(OpWriteAction::Open)?;
        let invocation_id = InvocationId::mint();
        let resolution = self.resolve_routing_and_context(request, invocation_id.as_str(), None)?;
        // The reference builds `OpStartedEvent(profile_id=profile.profile_id)`
        // *after* routing/context/glossary; its constrained `str` rejects a lone
        // surrogate with a Pydantic `string_unicode` error and nothing is
        // written. Reproduce that failure at the same point and footprint.
        let profile_id = Self::record_string(&resolution.profile.profile_id, "profile_id")?;
        let started_at = now_utc_iso();
        let record = OpStartedEvent::new(OpStartedFields {
            invocation_id: invocation_id.clone(),
            profile_id: profile_id.clone(),
            action: resolution.action.clone(),
            request_text: request.request_text.to_string(),
            actor: request.actor.to_string(),
            mode_of_work: request.mode_of_work.unwrap_or(ModeOfWork::TaskExecution),
            governance_context_hash: resolution.context_hash.clone().unwrap_or_default(),
            governance_context_available: resolution.context_available,
            router_confidence: resolution.router_confidence.clone(),
            started_at,
            mission_id: request.mission_id.map(str::to_owned),
            wp_id: request.wp_id.map(str::to_owned),
            model_id: resolution.model_id.clone(),
        })
        .map_err(|error| InvocationError::Record(error.to_string()))?;
        // A write failure is the invocation's failure: no payload is returned and
        // no close is attempted for this id.
        self.writer.write_started(&record)?;
        Ok(InvocationPayload {
            invocation_id: invocation_id.as_str().to_string(),
            profile_id: resolution.profile.profile_id.clone(),
            profile_friendly_name: resolution
                .profile
                .name
                .clone()
                .unwrap_or_else(|| resolution.profile.profile_id.clone()),
            action: resolution.action,
            governance_context_text: resolution.context_text,
            governance_context_hash: resolution.context_hash,
            governance_context_available: resolution.context_available,
            router_confidence: resolution.router_confidence,
            glossary_observations: Some(resolution.bundle),
            mode_of_work: Some(record.mode_of_work().as_str().to_string()),
            recommendation: resolution.recommendation,
            empty_charter_fallback: resolution.empty_charter_fallback,
            alternatives: resolution.alternatives,
        })
    }

    /// Route the request and assemble the read-only halves; write nothing.
    pub fn dry_run(
        &mut self,
        request: &InvocationRequest<'_>,
    ) -> Result<InvocationPayload, ExecutorError> {
        // The empty invocation id suppresses the persisted candidate events in
        // the reference; the port receives `""` for that reason.
        let resolution = self.resolve_routing_and_context(request, "", Some("exact"))?;
        // A dry run writes no Op record, so the reference's `OpStartedEvent`
        // string constraint does not apply: the exact authored id is returned.
        Ok(InvocationPayload {
            invocation_id: String::new(),
            profile_id: resolution.profile.profile_id.clone(),
            profile_friendly_name: resolution
                .profile
                .name
                .clone()
                .unwrap_or_else(|| resolution.profile.profile_id.clone()),
            action: resolution.action,
            governance_context_text: resolution.context_text,
            governance_context_hash: resolution.context_hash,
            governance_context_available: resolution.context_available,
            router_confidence: resolution.router_confidence,
            glossary_observations: Some(resolution.bundle),
            mode_of_work: None,
            recommendation: resolution.recommendation,
            empty_charter_fallback: resolution.empty_charter_fallback,
            alternatives: resolution.alternatives,
        })
    }

    fn resolve_routing_and_context(
        &mut self,
        request: &InvocationRequest<'_>,
        invocation_id: &str,
        explicit_hint_confidence: Option<&str>,
    ) -> Result<Resolution, ExecutorError> {
        let (profile, action, router_confidence, empty_charter_fallback, alternatives) =
            match request.profile_hint {
                Some(hint) => {
                    // FR-009/EDGE-005: a truthy action_hint wins; the empty string does
                    // not, so the role-default derivation runs exactly as before.
                    let profile = self.resolve_profile(hint)?;
                    let action = match request.action_hint {
                        Some(action) if !action.is_empty() => action.to_string(),
                        _ => derive_action_from_role_exact(&profile.role).to_string(),
                    };
                    (
                        profile,
                        action,
                        explicit_hint_confidence.map(str::to_owned),
                        false,
                        Vec::new(),
                    )
                }
                None => {
                    let fallback = self
                        .empty_charter
                        .and_then(|port| port.fallback(request.request_text));
                    if let Some(fallback) = fallback {
                        let profile = self
                            .resolve_profile(&PythonString::from_scalars(&fallback.profile_id))?;
                        (
                            profile,
                            fallback.action,
                            Some("generic_fallback".to_string()),
                            true,
                            Vec::new(),
                        )
                    } else {
                        let decision = self.route(request.request_text)?;
                        let profile = self.resolve_profile(&decision.profile_id)?;
                        let alternatives = decision
                            .alternatives
                            .iter()
                            .map(|alternative| RoutingAlternativeView {
                                profile_id: alternative.profile_id.clone(),
                                action: alternative.action.to_string(),
                                confidence: alternative.confidence.as_str().to_string(),
                                match_reason: alternative.match_reason.clone(),
                            })
                            .collect();
                        (
                            profile,
                            decision.action.to_string(),
                            Some(decision.confidence.as_str().to_string()),
                            false,
                            alternatives,
                        )
                    }
                }
            };

        let recommendation = self.recommendation(&profile, &action);
        let model_id = recommendation
            .as_ref()
            .and_then(RoutingRecommendation::catalog_candidate)
            .map(|candidate| candidate.model_id.clone());

        // Assemble the governance context with mark_loaded=False semantics.
        let context = self
            .governance
            .map(|port| port.build_context(&profile.profile_id, &action))
            .unwrap_or_else(GovernanceContext::missing);
        let context_hash = context
            .text
            .as_ref()
            .map(|text| sha256_hex(text.as_bytes())[..16].to_string());
        let context_available = context.available();

        let bundle = self
            .glossary
            .map(|port| port.scan(request.request_text, invocation_id, request.actor))
            .unwrap_or_else(GlossaryObservation::empty);

        Ok(Resolution {
            profile,
            action,
            router_confidence,
            empty_charter_fallback,
            alternatives,
            recommendation,
            model_id,
            context_text: context.text,
            context_hash,
            context_available,
            bundle,
        })
    }

    fn resolve_profile(&self, profile_id: &PythonString) -> Result<CatalogProfile, ExecutorError> {
        self.catalog
            .and_then(|catalog| catalog.profile(profile_id))
            .ok_or_else(|| ExecutorError::ProfileNotFound(profile_id_label(profile_id)))
    }

    /// A constrained Pydantic `str` field of `OpStartedEvent`.
    ///
    /// `min_length=1` plus `string_unicode`: empty strings and any lone
    /// surrogate are refused with the reference's error text. The surrogate
    /// check is what makes a Source-valid profile id fail here (and only here).
    fn record_string(value: &PythonString, field: &str) -> Result<String, ExecutorError> {
        if value.is_empty() {
            return Err(ExecutorError::RecordValidation {
                field: field.to_owned(),
                message: "String should have at least 1 character".to_owned(),
            });
        }
        if value
            .codepoints()
            .iter()
            .any(|code| (0xd800..=0xdfff).contains(code))
        {
            return Err(ExecutorError::RecordValidation {
                field: field.to_owned(),
                message: format!(
                    "Input should be a valid string, unable to parse raw data as a unicode string \
                     [type=string_unicode, input_value={}, input_type=str]",
                    python_repr_codepoints(value.codepoints())
                ),
            });
        }
        Ok(value
            .codepoints()
            .iter()
            .map(|&code| char::from_u32(code).expect("surrogates were refused above"))
            .collect())
    }

    fn route(&self, request_text: &str) -> Result<RouterDecision, ExecutorError> {
        let Some(catalog) = self.catalog else {
            return Err(ExecutorError::NoRouter);
        };
        let profiles = catalog.routing_profiles();
        ActionRouter::route(&PythonString::from_scalars(request_text), None, &profiles).map_err(
            |error| ExecutorError::Router {
                code: error.code().as_str().to_string(),
                message: py_to_string(&error.message()),
                candidates: error
                    .candidates()
                    .iter()
                    .map(|candidate| RoutingAlternativeView {
                        profile_id: candidate.profile_id.clone(),
                        action: candidate.action.to_string(),
                        confidence: candidate.confidence.as_str().to_string(),
                        match_reason: candidate.match_reason.clone(),
                    })
                    .collect(),
                suggestion: py_to_string(error.suggestion()),
            },
        )
    }

    fn recommendation(
        &self,
        profile: &CatalogProfile,
        action: &str,
    ) -> Option<RoutingRecommendation> {
        let catalog = self.model_catalog?;
        let task_type = task_type_for_verb(action)?;
        let recommendation = evaluate(catalog, task_type, &profile.routing_view());
        (!recommendation.candidates.is_empty()).then_some(recommendation)
    }

    /// Close an open invocation record, promote evidence, append links, and run
    /// the best-effort Op-record commit — the reference's exact order.
    pub fn complete_invocation(
        &mut self,
        request: &CompletionRequest<'_>,
    ) -> Result<CompletionOutcome, ExecutorError> {
        let invocation_id =
            InvocationId::parse(request.invocation_id).map_err(InvocationError::from)?;
        let started_mode = self.writer.read_started_mode(&invocation_id)?;

        // An agent close commits the record, so it is admitted before any write;
        // a doctor-sweep close records on the spine and is not judged here.
        if request.closed_by == ClosedBy::Agent {
            self.require_op_write_admitted(OpWriteAction::Close)?;
        }

        // Evidence-mode gate, before any write.
        if request.evidence_ref.is_some() {
            match &started_mode {
                RecordedMode::Known(mode) if !mode.permits_evidence() => {
                    return Err(ExecutorError::InvalidModeForEvidence {
                        invocation_id: request.invocation_id.to_string(),
                        mode: mode.as_str().to_string(),
                    });
                }
                RecordedMode::Undetermined(raw) => {
                    return Err(ExecutorError::UndeterminedModeForEvidence {
                        invocation_id: request.invocation_id.to_string(),
                        mode: raw.to_string(),
                    });
                }
                _ => {}
            }
        }

        // The closure spine is the shared idempotency authority.
        let closed = self.writer.closed_ids()?;
        if closed.contains(&invocation_id) {
            return Err(ExecutorError::AlreadyClosed(
                request.invocation_id.to_string(),
            ));
        }

        // Resolve the evidence content BEFORE the terminal write, so a read or
        // encoding failure cannot strand a closed Op without its links.
        let evidence_content = match request.evidence_ref {
            Some(reference) => {
                Some(self.resolve_evidence_content(request.invocation_id, reference)?)
            }
            None => None,
        };

        let completed = OpCompletedEvent::new(OpCompletedFields {
            invocation_id: invocation_id.clone(),
            completed_at: now_utc_iso(),
            outcome: request.outcome,
            closed_by: request.closed_by,
            evidence_ref: request.evidence_ref.map(str::to_owned),
        })
        .map_err(|error| InvocationError::Record(error.to_string()))?;
        // Recording surface (#4397): an agent close appends to the per-record
        // trail it minted this session; a doctor-sweep close targets records
        // that are pre-existing archive history — byte-frozen — so its closure
        // goes to the append-only spine and the per-record file is not touched.
        match request.closed_by {
            ClosedBy::Agent => {
                self.writer.write_completed(&completed)?;
            }
            ClosedBy::DoctorSweep => {
                self.writer.write_spine_closure(&completed)?;
            }
        }

        let evidence_dir = match evidence_content {
            Some(content) => Some(promote_to_evidence(
                &self.record_root,
                &completed,
                &content,
            )?),
            None => None,
        };

        for reference in request.artifact_refs {
            let normalised = crate::writer::normalise_ref(reference, &self.record_root);
            self.writer
                .append_correlation_link(&invocation_id, CorrelationLink::artifact(normalised))?;
        }
        if let Some(sha) = request.commit_sha {
            self.writer
                .append_correlation_link(&invocation_id, CorrelationLink::commit(sha))?;
        }

        let closed_by = match request.closed_by {
            ClosedBy::Agent => OpClosedBy::Agent,
            ClosedBy::DoctorSweep => OpClosedBy::DoctorSweep,
        };
        let op_commit = self.commits.commit_op_record(&invocation_id, closed_by);
        self.last_op_commit = Some(op_commit.clone());
        Ok(CompletionOutcome {
            completed,
            op_commit: Some(op_commit),
            evidence_dir,
        })
    }

    fn resolve_evidence_content(
        &self,
        invocation_id: &str,
        evidence_ref: &str,
    ) -> Result<String, ExecutorError> {
        let candidate = Path::new(evidence_ref);
        let path = if candidate.is_absolute() {
            Some(candidate.to_path_buf())
        } else {
            let root = self
                .record_root
                .canonicalize()
                .unwrap_or_else(|_| self.record_root.clone());
            let resolved = root.join(candidate);
            resolved
                .canonicalize()
                .ok()
                .filter(|resolved| resolved.starts_with(&root))
        };
        if let Some(path) = path
            && let Ok(bytes) = std::fs::read(&path)
        {
            return String::from_utf8(bytes).map_err(|_| ExecutorError::UnsupportedEvidence {
                invocation_id: invocation_id.to_string(),
                reason: "the file is not UTF-8 text".to_string(),
            });
        }
        // Inline evidence text must itself be UTF-8 encodable (Rust strings always
        // are; the check exists for symmetry with the reference's refusal).
        Ok(evidence_ref.to_string())
    }
}

struct Resolution {
    profile: CatalogProfile,
    action: String,
    router_confidence: Option<String>,
    empty_charter_fallback: bool,
    alternatives: Vec<RoutingAlternativeView>,
    recommendation: Option<RoutingRecommendation>,
    model_id: Option<String>,
    context_text: Option<String>,
    context_hash: Option<String>,
    context_available: bool,
    bundle: GlossaryObservation,
}

/// Promote a completed Op to a Tier 2 evidence artifact
/// (`record.promote_to_evidence`): `<root>/.kittify/evidence/<id>/evidence.md`
/// plus `record.json`.
pub fn promote_to_evidence(
    record_root: &Path,
    completed: &OpCompletedEvent,
    content: &str,
) -> Result<PathBuf, ExecutorError> {
    let directory = record_root
        .join(".kittify")
        .join("evidence")
        .join(completed.invocation_id().as_str());
    std::fs::create_dir_all(&directory)
        .map_err(|error| ExecutorError::Evidence(error.to_string()))?;
    std::fs::write(directory.join("evidence.md"), content)
        .map_err(|error| ExecutorError::Evidence(error.to_string()))?;
    let record = json!({
        "event": "completed",
        "invocation_id": completed.invocation_id().as_str(),
        "completed_at": completed.completed_at(),
        "outcome": completed.outcome().as_str(),
        "closed_by": completed.closed_by().as_str(),
        "evidence_ref": completed.evidence_ref(),
    });
    let pretty = serde_json::to_string_pretty(&record)
        .map_err(|error| ExecutorError::Evidence(error.to_string()))?;
    std::fs::write(directory.join("record.json"), pretty)
        .map_err(|error| ExecutorError::Evidence(error.to_string()))?;
    Ok(directory)
}

/// Convert a router-produced Python string to owned text.
pub fn py_to_string(value: &PythonString) -> String {
    value.to_scalar_string().unwrap_or_else(|_| value.repr())
}

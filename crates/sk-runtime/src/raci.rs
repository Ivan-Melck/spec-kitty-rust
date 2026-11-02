//! Deterministic RACI inference and resolution.
//!
//! The upstream `raci.py` module is local-only, offline and deterministic. This
//! module owns the same policy:
//!
//! * every step type has a default assignment
//!   (`prompt_default`, `audit_advisory`, `audit_blocking`),
//! * accountable is always human (P0), and a blocking audit also makes the
//!   responsible role human,
//! * an explicit template RACI block wins over the inferred default and must
//!   carry a non-empty override reason (validated when the template loads),
//! * a missing actor id resolves from the runtime inputs
//!   (`mission_owner_id` / `agent_id` / `service_id`),
//! * unresolvable responsible/accountable roles fail closed with
//!   [`RuntimeError::RaciEscalation`]; unresolvable consulted/informed roles
//!   take the per-type sentinel (`human-in-charge` or `UNKNOWN`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RuntimeError;
use crate::snapshot::{ActorType, MissionPolicySnapshot};
use crate::template::{AuditEnforcement, PromptStep, RaciAssignment, RaciRoleSpec};

/// Sentinel actor id recorded for an unresolved human role.
pub const HUMAN_IN_CHARGE_ACTOR_ID: &str = "human-in-charge";

/// Sentinel actor id recorded for an unresolved llm/service role.
pub const UNKNOWN_ACTOR_ID: &str = "UNKNOWN";

/// A resolved actor-role binding: like the template spec, but the id is always
/// present (possibly a sentinel).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaciRoleBinding {
    pub actor_type: ActorType,
    pub actor_id: String,
}

/// Which rule produced the assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaciRule {
    PromptDefault,
    AuditAdvisory,
    AuditBlocking,
}

impl RaciRule {
    /// The upstream wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PromptDefault => "prompt_default",
            Self::AuditAdvisory => "audit_advisory",
            Self::AuditBlocking => "audit_blocking",
        }
    }
}

/// Whether the assignment was authored explicitly or inferred by policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaciSource {
    Explicit,
    Inferred,
}

impl RaciSource {
    /// The upstream wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Inferred => "inferred",
        }
    }
}

/// A fully resolved RACI snapshot, wire-compatible with the upstream
/// `ResolvedRaciSnapshot` record persisted under `decisions["raci:<step>"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRaciSnapshot {
    pub step_id: String,
    pub responsible: RaciRoleBinding,
    pub accountable: RaciRoleBinding,
    #[serde(default)]
    pub consulted: Vec<RaciRoleBinding>,
    #[serde(default)]
    pub informed: Vec<RaciRoleBinding>,
    pub source: RaciSource,
    pub inferred_rule: Option<RaciRule>,
    pub override_reason: Option<String>,
}

/// A borrowed view over either step flavour, so RACI policy has one signature.
#[derive(Debug, Clone, Copy)]
pub enum StepRef<'a> {
    Prompt(&'a PromptStep),
    Audit(&'a crate::template::AuditStep),
}

impl StepRef<'_> {
    /// The step id.
    pub fn id(&self) -> &str {
        match self {
            Self::Prompt(step) => &step.id,
            Self::Audit(step) => &step.id,
        }
    }

    /// The explicit template RACI block, when authored.
    pub fn raci(&self) -> Option<&RaciAssignment> {
        match self {
            Self::Prompt(step) => step.raci.as_ref(),
            Self::Audit(step) => step.raci.as_ref(),
        }
    }

    /// The explicit override reason, when authored.
    pub fn raci_override_reason(&self) -> Option<&str> {
        match self {
            Self::Prompt(step) => step.raci_override_reason.as_deref(),
            Self::Audit(step) => step.raci_override_reason.as_deref(),
        }
    }

    /// Whether this is a blocking audit step.
    pub fn is_blocking_audit(&self) -> bool {
        matches!(
            self,
            Self::Audit(step) if step.audit.enforcement == AuditEnforcement::Blocking
        )
    }
}

impl<'a> From<&'a PromptStep> for StepRef<'a> {
    fn from(step: &'a PromptStep) -> Self {
        Self::Prompt(step)
    }
}

impl<'a> From<&'a crate::template::AuditStep> for StepRef<'a> {
    fn from(step: &'a crate::template::AuditStep) -> Self {
        Self::Audit(step)
    }
}

/// Build a resolved binding, filling a missing or blank id with the sentinel
/// for the actor type.
pub fn bind_actor(actor_type: ActorType, actor_id: Option<&str>) -> RaciRoleBinding {
    // A present, non-blank id is kept verbatim (upstream does not trim it);
    // only a missing/blank id falls back to the sentinel.
    match actor_id.filter(|id| !id.trim().is_empty()) {
        Some(id) => RaciRoleBinding {
            actor_type,
            actor_id: id.to_string(),
        },
        None => RaciRoleBinding {
            actor_type,
            actor_id: match actor_type {
                ActorType::Human => HUMAN_IN_CHARGE_ACTOR_ID.to_string(),
                ActorType::Llm | ActorType::Service => UNKNOWN_ACTOR_ID.to_string(),
            },
        },
    }
}

fn inferred_assignment(step: StepRef<'_>) -> (RaciAssignment, RaciRule) {
    let (responsible, rule) = match step {
        StepRef::Audit(audit) if audit.audit.enforcement == AuditEnforcement::Blocking => {
            (ActorType::Human, RaciRule::AuditBlocking)
        }
        StepRef::Audit(_) => (ActorType::Llm, RaciRule::AuditAdvisory),
        StepRef::Prompt(_) => (ActorType::Llm, RaciRule::PromptDefault),
    };
    (
        RaciAssignment {
            responsible: RaciRoleSpec {
                actor_type: responsible,
                actor_id: None,
            },
            accountable: RaciRoleSpec {
                actor_type: ActorType::Human,
                actor_id: None,
            },
            consulted: Vec::new(),
            informed: Vec::new(),
        },
        rule,
    )
}

/// Infer default RACI bindings from step type, resolving each role best-effort.
///
/// This never fails closed: every role is resolved from the inputs when it can
/// be, and otherwise takes the sentinel for its actor type. The upstream engine
/// uses it when [`resolve_raci`] escalates.
pub fn infer_raci(
    step: StepRef<'_>,
    _policy: &MissionPolicySnapshot,
    inputs: Option<&BTreeMap<String, Value>>,
) -> ResolvedRaciSnapshot {
    let (assignment, rule) = inferred_assignment(step);
    let empty = BTreeMap::new();
    let given = inputs.unwrap_or(&empty);
    ResolvedRaciSnapshot {
        step_id: step.id().to_string(),
        responsible: resolve_optional(&assignment.responsible, given),
        accountable: resolve_optional(&assignment.accountable, given),
        consulted: assignment
            .consulted
            .iter()
            .map(|role| resolve_optional(role, given))
            .collect(),
        informed: assignment
            .informed
            .iter()
            .map(|role| resolve_optional(role, given))
            .collect(),
        source: RaciSource::Inferred,
        inferred_rule: Some(rule),
        override_reason: None,
    }
}

/// Validate an explicit assignment against the P0 invariants.
///
/// Returns the human-readable violations (empty means valid). Callers that need
/// a typed failure use [`resolve_raci`], which escalates on a violation.
pub fn validate_raci_assignment(assignment: &RaciAssignment, step: StepRef<'_>) -> Vec<String> {
    let mut errors = Vec::new();
    if assignment.accountable.actor_type != ActorType::Human {
        errors.push(format!(
            "P0 invariant violation: accountable must be human, got '{}'",
            assignment.accountable.actor_type
        ));
    }
    if step.is_blocking_audit() && assignment.responsible.actor_type != ActorType::Human {
        errors.push(format!(
            "Blocking audit step '{}': responsible must be human, got '{}'",
            step.id(),
            assignment.responsible.actor_type
        ));
    }
    errors
}

/// Resolve RACI to concrete actors using runtime inputs.
///
/// An explicit template block wins (`source = explicit`) and is validated; the
/// inferred default applies otherwise. Responsible and accountable roles fail
/// closed when neither the template nor the inputs name an actor. Consulted and
/// informed roles take their sentinel instead.
pub fn resolve_raci(
    step: StepRef<'_>,
    inputs: &BTreeMap<String, Value>,
    _policy: &MissionPolicySnapshot,
) -> Result<ResolvedRaciSnapshot, RuntimeError> {
    let (assignment, inferred_rule, override_reason) = match step.raci() {
        Some(assignment) => {
            let errors = validate_raci_assignment(assignment, step);
            if !errors.is_empty() {
                return Err(RuntimeError::InvalidExplicitRaci {
                    step_id: step.id().to_string(),
                    message: errors.join("; "),
                });
            }
            (
                assignment.clone(),
                None,
                step.raci_override_reason().map(str::to_string),
            )
        }
        None => {
            let (assignment, rule) = inferred_assignment(step);
            (assignment, Some(rule), None)
        }
    };

    Ok(ResolvedRaciSnapshot {
        step_id: step.id().to_string(),
        responsible: resolve_required(&assignment.responsible, "responsible", step.id(), inputs)?,
        accountable: resolve_required(&assignment.accountable, "accountable", step.id(), inputs)?,
        consulted: assignment
            .consulted
            .iter()
            .map(|role| resolve_optional(role, inputs))
            .collect(),
        informed: assignment
            .informed
            .iter()
            .map(|role| resolve_optional(role, inputs))
            .collect(),
        source: if step.raci().is_some() {
            RaciSource::Explicit
        } else {
            RaciSource::Inferred
        },
        inferred_rule,
        override_reason,
    })
}

fn resolve_required(
    binding: &RaciRoleSpec,
    role_name: &str,
    step_id: &str,
    inputs: &BTreeMap<String, Value>,
) -> Result<RaciRoleBinding, RuntimeError> {
    let Some(actor_id) = given_or_input_id(binding, inputs) else {
        let input_key = actor_type_to_input_key(binding.actor_type);
        return Err(RuntimeError::RaciEscalation {
            step_id: step_id.to_string(),
            role: role_name.to_string(),
            input_key: input_key.to_string(),
        });
    };
    Ok(RaciRoleBinding {
        actor_type: binding.actor_type,
        actor_id,
    })
}

fn resolve_optional(binding: &RaciRoleSpec, inputs: &BTreeMap<String, Value>) -> RaciRoleBinding {
    bind_actor(
        binding.actor_type,
        given_or_input_id(binding, inputs).as_deref(),
    )
}

fn given_or_input_id(binding: &RaciRoleSpec, inputs: &BTreeMap<String, Value>) -> Option<String> {
    if let Some(actor_id) = binding
        .actor_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        return Some(actor_id.to_string());
    }
    lookup_actor_id(binding.actor_type, inputs)
}

fn lookup_actor_id(actor_type: ActorType, inputs: &BTreeMap<String, Value>) -> Option<String> {
    let value = inputs.get(actor_type_to_input_key(actor_type))?;
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn actor_type_to_input_key(actor_type: ActorType) -> &'static str {
    match actor_type {
        ActorType::Human => "mission_owner_id",
        ActorType::Llm => "agent_id",
        ActorType::Service => "service_id",
    }
}

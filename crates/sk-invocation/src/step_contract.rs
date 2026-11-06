//! `StepContractExecutor`: compose a Mission step contract out of profile
//! invocations, exactly once per step.
//!
//! Ports `src/specify_cli/mission_step_contracts/executor.py` (the contract
//! model, the shipped default-profile table and its availability fallback, the
//! merged-DRG delegation resolution, the request-text builder, and the
//! invoke-then-close-once loop) plus
//! `src/charter/offering/missions/step_contracts.py`'s on-disk schema.
//!
//! The **only** execution call this module makes is
//! [`ProfileInvocationExecutor::invoke`]; the governed `next` dispatch reaches
//! the reference's C-001 invariant by calling this executor and never the
//! profile executor directly.
//!
//! YAML loading is deliberately *not* here: the composition root (the CLI,
//! which already owns the workspace's canonical YAML reader) parses
//! `*.step-contract.yaml` documents into these typed records. This keeps the
//! crate free of a second YAML authority and of filesystem policy.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::executor::{
    CatalogProfile, ExecutorError, InvocationPayload, InvocationRequest, ProfileInvocationExecutor,
};
use crate::modes::ModeOfWork;
use crate::record::OpOutcome;

/// One declared runtime input (`MissionStepInput`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StepInput {
    /// The CLI flag (`--mission`).
    pub flag: String,
    /// The value's source placeholder (`mission_slug`).
    pub source: String,
    /// Whether the input may be omitted.
    #[serde(default)]
    pub optional: bool,
}

impl StepInput {
    /// Render one input as `flag {source}` (bracketed when optional).
    pub fn render(&self) -> String {
        let rendered = format!("{} {{{}}}", self.flag, self.source);
        if self.optional {
            format!("[{rendered}]")
        } else {
            rendered
        }
    }
}

/// A delegation link from a contract step to doctrine artifacts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegatesTo {
    /// The artifact kind (`paradigm`, `tactic`, `directive`, …).
    pub kind: String,
    /// The candidate ids, in authored order.
    pub candidates: Vec<String>,
}

/// One step inside a Mission step contract (`MissionStepContractStep`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContractStep {
    /// The step id.
    pub id: String,
    /// The step description.
    pub description: String,
    /// A declared command; guidance for the host, never executed here.
    #[serde(default)]
    pub command: Option<String>,
    /// The declared inputs.
    #[serde(default)]
    pub inputs: Vec<StepInput>,
    /// The delegation declaration.
    #[serde(default)]
    pub delegates_to: Option<DelegatesTo>,
    /// Free-form step guidance.
    #[serde(default)]
    pub guidance: Option<String>,
}

/// A transition-gate binding. Parsed and carried verbatim; gate execution is a
/// separate authority and is not performed by this executor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GateBinding {
    /// The bound lane transition (`<from>-><to>`).
    pub on_transition: String,
    /// The registry handler name.
    pub handler: String,
    /// The handler kind.
    #[serde(default = "default_handler_kind")]
    pub handler_kind: String,
    /// The binding schema version.
    pub schema_version: String,
    /// Inert half-A fail-open flag, carried byte-stable.
    #[serde(default = "default_true")]
    pub fail_open: bool,
    /// Optional provenance note.
    #[serde(default)]
    pub provenance: Option<String>,
}

fn default_handler_kind() -> String {
    "mission_step_contract".to_string()
}

fn default_true() -> bool {
    true
}

/// One on-disk `*.step-contract.yaml` document (`MissionStepContract`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MissionStepContract {
    /// The contract schema version.
    pub schema_version: String,
    /// The contract id (which may differ from `<mission>/<action>`).
    pub id: String,
    /// The composed action.
    pub action: String,
    /// The mission type.
    pub mission: String,
    /// The contract's steps, in order.
    pub steps: Vec<ContractStep>,
    /// Contract/action-level gate bindings.
    #[serde(default)]
    pub gates: Vec<GateBinding>,
}

impl MissionStepContract {
    /// Validate the contract's structural invariants (non-empty steps, unique
    /// step ids) — the reference's Pydantic `min_length=1` plus
    /// `validate_unique_step_ids`.
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("steps: List should have at least 1 item after validation".to_string());
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut duplicates: BTreeSet<&str> = BTreeSet::new();
        for step in &self.steps {
            if !seen.insert(step.id.as_str()) {
                duplicates.insert(step.id.as_str());
            }
        }
        if !duplicates.is_empty() {
            let mut listed: Vec<&str> = duplicates.into_iter().collect();
            listed.sort_unstable();
            return Err(format!("duplicate step IDs: {listed:?}"));
        }
        Ok(())
    }

    /// The two spellings a deny/allow list may name (`id`, `<mission>/<action>`).
    pub fn spellings(&self) -> [String; 2] {
        [self.id.clone(), format!("{}/{}", self.mission, self.action)]
    }
}

/// The loaded contract set, with the reference's `get_by_action` lookup.
#[derive(Clone, Debug, Default)]
pub struct StepContractCatalog {
    contracts: Vec<MissionStepContract>,
}

impl StepContractCatalog {
    /// An empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a catalog from already-loaded contracts.
    pub fn from_contracts(contracts: Vec<MissionStepContract>) -> Self {
        Self { contracts }
    }

    /// Replace the whole set.
    pub fn set_contracts(&mut self, contracts: Vec<MissionStepContract>) {
        self.contracts = contracts;
    }

    /// Every loaded contract.
    pub fn contracts(&self) -> &[MissionStepContract] {
        &self.contracts
    }

    /// Look a contract up by mission and action (first match in load order).
    pub fn get_by_action(&self, mission: &str, action: &str) -> Option<&MissionStepContract> {
        self.contracts
            .iter()
            .find(|contract| contract.mission == mission && contract.action == action)
    }
}

/// Whether a contract is in the effective mission-step-contract set (D-21).
///
/// The real implementation resolves the activation ledger; the trait keeps this
/// executor free of an activation-store read of its own.
pub trait ContractAdmission {
    /// `true` when the contract may run.
    fn admits(&self, contract: &MissionStepContract) -> bool;
}

/// The execution context (`StepContractExecutionContext`).
#[derive(Clone, Debug)]
pub struct StepContractExecutionContext<'a> {
    /// The project root governance is read from.
    pub repo_root: &'a Path,
    /// The mission type.
    pub mission: &'a str,
    /// The composed action.
    pub action: &'a str,
    /// The acting agent.
    pub actor: &'a str,
    /// An explicit profile hint (custom missions' frozen-template binding).
    pub profile_hint: Option<&'a str>,
    /// The caller's request text.
    pub request_text: Option<&'a str>,
    /// The Op mode of work.
    pub mode_of_work: Option<ModeOfWork>,
    /// The DRG resolution depth (the reference's default is 2).
    pub resolution_depth: usize,
}

impl<'a> StepContractExecutionContext<'a> {
    /// A context for `(mission, action)` by `actor`.
    pub fn new(repo_root: &'a Path, mission: &'a str, action: &'a str, actor: &'a str) -> Self {
        Self {
            repo_root,
            mission,
            action,
            actor,
            profile_hint: None,
            request_text: None,
            mode_of_work: None,
            resolution_depth: 2,
        }
    }
}

/// One resolved delegation (`ResolvedStepDelegation`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStepDelegation {
    /// The declared artifact kind.
    pub kind: String,
    /// The candidate token as authored.
    pub candidate: String,
    /// The resolved DRG URN.
    pub urn: String,
    /// The node's label, when one is declared.
    pub label: Option<String>,
}

/// One composed step's result (`StepContractStepResult`).
#[derive(Clone, Debug)]
pub struct StepContractStepResult {
    /// The step id.
    pub step_id: String,
    /// The 1-based sequence.
    pub sequence: usize,
    /// The description.
    pub description: String,
    /// The declared command, verbatim.
    pub command: Option<String>,
    /// Whether a command was declared.
    pub command_declared: bool,
    /// The step guidance.
    pub guidance: Option<String>,
    /// The declared inputs.
    pub inputs: Vec<StepInput>,
    /// The delegations that resolved against the action context.
    pub resolved_delegations: Vec<ResolvedStepDelegation>,
    /// The candidates that did not resolve (a WARNING upstream, never fatal).
    pub unresolved_candidates: Vec<String>,
    /// The invocation payload for this step.
    pub invocation_payload: InvocationPayload,
}

impl StepContractStepResult {
    /// The underlying invocation id.
    pub fn invocation_id(&self) -> &str {
        &self.invocation_payload.invocation_id
    }
}

/// A complete contract run (`StepContractExecutionResult`).
#[derive(Clone, Debug)]
pub struct StepContractExecutionResult {
    /// The contract id.
    pub contract_id: String,
    /// The mission type.
    pub mission: String,
    /// The composed action.
    pub action: String,
    /// The resolved profile hint.
    pub profile_hint: String,
    /// How the profile hint resolved (`merged_drg`).
    pub resolution_source: String,
    /// One result per executed step, in order.
    pub steps: Vec<StepContractStepResult>,
}

impl StepContractExecutionResult {
    /// Every invocation id emitted by the run, in step order.
    pub fn invocation_ids(&self) -> Vec<&str> {
        self.steps
            .iter()
            .map(StepContractStepResult::invocation_id)
            .collect()
    }
}

/// Composition failures (`StepContractExecutionError`).
#[derive(Debug, thiserror::Error)]
pub enum StepContractExecutionError {
    /// No contract was found for the requested action.
    #[error("No step contract found for mission/action {mission}/{action}")]
    NotFound {
        /// The mission type.
        mission: String,
        /// The action.
        action: String,
    },
    /// The contract is not in the effective activation set.
    #[error(
        "Step contract {contract_id:?} ({mission}/{action}) is not runnable: it is not in the effective mission-step-contract set. Run: spec-kitty charter activate mission-step-contract {contract_id}"
    )]
    NotAdmitted {
        /// The contract id.
        contract_id: String,
        /// The mission type.
        mission: String,
        /// The action.
        action: String,
    },
    /// No built-in default and no explicit hint.
    #[error("profile_hint is required when no action default exists for {mission}/{action}")]
    ProfileHintRequired {
        /// The mission type.
        mission: String,
        /// The action.
        action: String,
    },
    /// The default profile is deactivated and no same-role profile is available.
    #[error(
        "No available profile for {mission}/{action}: the built-in default profile '{default}'{role_note} is deactivated and no available profile carries that role. Activate an agent profile with that role, or supply an explicit profile for the step."
    )]
    NoAvailableProfile {
        /// The mission type.
        mission: String,
        /// The action.
        action: String,
        /// The deactivated default.
        default: String,
        /// The rendered role note (` (role 'implementer')`).
        role_note: String,
    },
    /// An invocation (or its close) failed.
    #[error(transparent)]
    Executor(#[from] ExecutorError),
}

/// A resolved action-context selection: the delivered artifact URNs plus the
/// graph lookup needed to canonicalise a candidate token.
#[derive(Clone, Debug, Default)]
pub struct ActionDoctrine {
    /// The artifact URNs resolved for the action.
    pub delivered: BTreeSet<String>,
    /// Every graph node, keyed by URN: `(kind prefix, label)`.
    pub nodes: BTreeMap<String, (String, Option<String>)>,
}

impl ActionDoctrine {
    /// Build the selection from an (already activation-filtered) merged graph.
    pub fn from_graph(graph: &sk_doctrine::DrgGraph, action_urn: &str, depth: usize) -> Self {
        let resolved = sk_doctrine::resolve_context(graph, action_urn, depth);
        let nodes = graph
            .nodes()
            .iter()
            .map(|node| {
                (
                    node.urn().to_string(),
                    (
                        node.kind().as_str().to_string(),
                        node.label().map(str::to_owned),
                    ),
                )
            })
            .collect();
        Self {
            delivered: resolved.artifact_urns,
            nodes,
        }
    }
}

/// The shipped built-in default profile per `(mission, action)` (#4115).
pub const ACTION_PROFILE_DEFAULTS: [(&str, &str, &str); 21] = [
    ("software-dev", "specify", "researcher-robbie"),
    ("software-dev", "plan", "architect-alphonso"),
    ("software-dev", "tasks", "architect-alphonso"),
    ("software-dev", "implement", "implementer-ivan"),
    ("software-dev", "review", "reviewer-renata"),
    ("plan", "specify", "researcher-robbie"),
    ("plan", "research", "researcher-robbie"),
    ("plan", "plan", "architect-alphonso"),
    ("plan", "review", "reviewer-renata"),
    ("research", "scoping", "researcher-robbie"),
    ("research", "methodology", "researcher-robbie"),
    ("research", "gathering", "researcher-robbie"),
    ("research", "synthesis", "researcher-robbie"),
    ("research", "output", "reviewer-renata"),
    ("documentation", "discover", "researcher-robbie"),
    ("documentation", "audit", "researcher-robbie"),
    ("documentation", "design", "architect-alphonso"),
    ("documentation", "generate", "implementer-ivan"),
    ("documentation", "validate", "reviewer-renata"),
    ("documentation", "publish", "reviewer-renata"),
    ("documentation", "accept", "reviewer-renata"),
];

/// Each shipped default's primary role (`_DEFAULT_PROFILE_ROLES`).
pub const DEFAULT_PROFILE_ROLES: [(&str, &str); 4] = [
    ("researcher-robbie", "researcher"),
    ("architect-alphonso", "architect"),
    ("implementer-ivan", "implementer"),
    ("reviewer-renata", "reviewer"),
];

/// The shipped default profile for a built-in `(mission, action)`.
pub fn action_profile_default(mission: &str, action: &str) -> Option<&'static str> {
    ACTION_PROFILE_DEFAULTS
        .iter()
        .find(|(candidate_mission, candidate_action, _)| {
            *candidate_mission == mission && *candidate_action == action
        })
        .map(|(_, _, profile)| *profile)
}

/// The pinned primary role of a shipped default profile.
pub fn default_profile_role(profile_id: &str) -> Option<&'static str> {
    DEFAULT_PROFILE_ROLES
        .iter()
        .find(|(candidate, _)| *candidate == profile_id)
        .map(|(_, role)| *role)
}

/// The composed contract executor.
pub struct StepContractExecutor<'a, 'b> {
    repo_root: &'a Path,
    contracts: &'a StepContractCatalog,
    invocation: &'a mut ProfileInvocationExecutor<'b>,
    graph: Option<&'a sk_doctrine::DrgGraph>,
    admission: Option<&'a dyn ContractAdmission>,
}

impl<'a, 'b> StepContractExecutor<'a, 'b> {
    /// Build an executor over `contracts`, recording through `invocation`.
    pub fn new(
        repo_root: &'a Path,
        contracts: &'a StepContractCatalog,
        invocation: &'a mut ProfileInvocationExecutor<'b>,
    ) -> Self {
        Self {
            repo_root,
            contracts,
            invocation,
            graph: None,
            admission: None,
        }
    }

    /// Attach the merged (activation-filtered) DRG used for delegation
    /// resolution.
    pub fn with_graph(mut self, graph: &'a sk_doctrine::DrgGraph) -> Self {
        self.graph = Some(graph);
        self
    }

    /// Attach the activation admission filter (D-21).
    pub fn with_admission(mut self, admission: &'a dyn ContractAdmission) -> Self {
        self.admission = Some(admission);
        self
    }

    /// The project root governance is read from.
    pub fn repo_root(&self) -> &Path {
        self.repo_root
    }

    /// Execute the contract's steps in order, invoking and closing each once.
    pub fn execute(
        &mut self,
        context: &StepContractExecutionContext<'_>,
        contract: Option<&MissionStepContract>,
    ) -> Result<StepContractExecutionResult, StepContractExecutionError> {
        let selected = match contract {
            Some(contract) => contract.clone(),
            None => self
                .contracts
                .get_by_action(context.mission, context.action)
                .cloned()
                .ok_or_else(|| StepContractExecutionError::NotFound {
                    mission: context.mission.to_string(),
                    action: context.action.to_string(),
                })?,
        };
        if let Some(admission) = self.admission
            && !admission.admits(&selected)
        {
            return Err(StepContractExecutionError::NotAdmitted {
                contract_id: selected.id.clone(),
                mission: selected.mission.clone(),
                action: selected.action.clone(),
            });
        }

        let profile_hint = self.resolved_profile_hint(context, &selected)?;
        let profile_hint_exact = sk_kernel::PythonString::from_scalars(&profile_hint);
        let doctrine = self.action_doctrine(&selected, context.resolution_depth);
        let mut steps: Vec<StepContractStepResult> = Vec::new();
        for (index, step) in selected.steps.iter().enumerate() {
            let (resolved, unresolved) = resolve_step_delegations(&doctrine, step);
            let request_text = build_request_text(&selected, context, step, &resolved, &unresolved);
            let request = InvocationRequest {
                request_text: &request_text,
                profile_hint: Some(&profile_hint_exact),
                actor: context.actor,
                mode_of_work: context.mode_of_work,
                action_hint: Some(&selected.action),
                mission_id: None,
                wp_id: None,
            };
            let payload = self.invocation.invoke(&request)?;
            // Result assembly can only fail on an unusable Op id; that is the
            // reference's `except` arm: close the Op failed, never report done.
            if let Err(error) = crate::record::InvocationId::parse(&payload.invocation_id) {
                let invocation_id = payload.invocation_id.clone();
                if let Err(close_error) = self.close_failed(&invocation_id) {
                    return Err(StepContractExecutionError::Executor(close_error));
                }
                return Err(StepContractExecutionError::Executor(
                    ExecutorError::Invocation(error.into()),
                ));
            }
            let result = StepContractStepResult {
                step_id: step.id.clone(),
                sequence: index + 1,
                description: step.description.clone(),
                command: step.command.clone(),
                command_declared: step.command.is_some(),
                guidance: step.guidance.clone(),
                inputs: step.inputs.clone(),
                resolved_delegations: resolved,
                unresolved_candidates: unresolved,
                invocation_payload: payload,
            };
            // outcome describes the composition-step trail only; not host-LLM status.
            self.invocation
                .complete_invocation(&crate::executor::CompletionRequest::agent(
                    &result.invocation_payload.invocation_id,
                    OpOutcome::Done,
                ))?;
            steps.push(result);
        }
        Ok(StepContractExecutionResult {
            contract_id: selected.id,
            mission: selected.mission,
            action: selected.action,
            profile_hint,
            resolution_source: "merged_drg".to_string(),
            steps,
        })
    }

    fn close_failed(&mut self, invocation_id: &str) -> Result<(), ExecutorError> {
        self.invocation
            .complete_invocation(&crate::executor::CompletionRequest::agent(
                invocation_id,
                OpOutcome::Failed,
            ))
            .map(|_| ())
    }

    fn action_doctrine(&self, contract: &MissionStepContract, depth: usize) -> ActionDoctrine {
        match self.graph {
            Some(graph) => ActionDoctrine::from_graph(
                graph,
                &format!("action:{}/{}", contract.mission, contract.action),
                depth,
            ),
            None => ActionDoctrine::default(),
        }
    }

    /// Resolve the profile hint, surfacing the no-available-profile refusal.
    ///
    /// Order: an explicit context hint (custom missions' frozen-template
    /// binding) wins; a built-in action uses the shipped default table, which is
    /// availability-adjusted (a deactivated default falls back to the highest
    /// `routing-priority` same-role profile, then profile id ascending).
    pub fn resolved_profile_hint(
        &self,
        context: &StepContractExecutionContext<'_>,
        contract: &MissionStepContract,
    ) -> Result<String, StepContractExecutionError> {
        if let Some(hint) = context.profile_hint.filter(|hint| !hint.is_empty()) {
            return Ok(hint.to_string());
        }
        let Some(default) = action_profile_default(&contract.mission, &contract.action) else {
            return Err(StepContractExecutionError::ProfileHintRequired {
                mission: contract.mission.clone(),
                action: contract.action.clone(),
            });
        };
        let catalog = self.invocation.list_available_profiles();
        if catalog.is_empty()
            || catalog
                .iter()
                .any(|profile| profile.scalar_id().as_deref() == Some(default))
        {
            return Ok(default.to_string());
        }
        let role = default_profile_role(default);
        if let Some(role) = role
            && let Some(chosen) = select_role_fallback(&catalog, role)
            && let Some(chosen_id) = chosen.scalar_id()
        {
            return Ok(chosen_id);
        }
        let role_note = role
            .map(|role| format!(" (role '{role}')"))
            .unwrap_or_default();
        Err(StepContractExecutionError::NoAvailableProfile {
            mission: contract.mission.clone(),
            action: contract.action.clone(),
            default: default.to_string(),
            role_note,
        })
    }
}

/// The deterministic same-role fallback over `catalog` (#4115).
pub fn select_role_fallback<'c>(
    catalog: &'c [CatalogProfile],
    role: &str,
) -> Option<&'c CatalogProfile> {
    catalog
        .iter()
        .filter(|profile| {
            profile
                .role
                .to_scalar_string()
                .map(|candidate| candidate == role)
                .unwrap_or(false)
        })
        .min_by(|left, right| {
            right
                .routing_priority
                .cmp(&left.routing_priority)
                .then_with(|| {
                    left.scalar_id()
                        .unwrap_or_default()
                        .cmp(&right.scalar_id().unwrap_or_default())
                })
        })
}

/// Resolve a step's declared delegations against the action context.
pub fn resolve_step_delegations(
    doctrine: &ActionDoctrine,
    step: &ContractStep,
) -> (Vec<ResolvedStepDelegation>, Vec<String>) {
    let Some(delegates_to) = &step.delegates_to else {
        return (Vec::new(), Vec::new());
    };
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for candidate in &delegates_to.candidates {
        let Some(urn) = candidate_urn(doctrine, &delegates_to.kind, candidate) else {
            unresolved.push(candidate.clone());
            continue;
        };
        if !doctrine.delivered.contains(&urn) {
            unresolved.push(candidate.clone());
            continue;
        }
        let label = doctrine
            .nodes
            .get(&urn)
            .and_then(|(_, label)| label.clone());
        resolved.push(ResolvedStepDelegation {
            kind: delegates_to.kind.clone(),
            candidate: candidate.clone(),
            urn,
            label,
        });
    }
    (resolved, unresolved)
}

fn candidate_urn(doctrine: &ActionDoctrine, kind: &str, candidate: &str) -> Option<String> {
    // `anti_pattern` and `skill` carry no step-contract delegation node kind.
    if kind == "anti_pattern" || kind == "skill" {
        return None;
    }
    let direct = format!("{kind}:{candidate}");
    if doctrine
        .nodes
        .get(&direct)
        .is_some_and(|(node_kind, _)| node_kind == kind)
    {
        return Some(direct);
    }
    if kind == "directive"
        && let Some(urn) = directive_candidate_urn(candidate)
        && doctrine
            .nodes
            .get(&urn)
            .is_some_and(|(node_kind, _)| node_kind == kind)
    {
        return Some(urn);
    }
    let matches: Vec<&String> = doctrine
        .nodes
        .iter()
        .filter(|(urn, (node_kind, _))| {
            node_kind == kind
                && urn
                    .split_once(':')
                    .map(|(_, id)| id == candidate)
                    .unwrap_or(false)
        })
        .map(|(urn, _)| urn)
        .collect();
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    None
}

fn directive_candidate_urn(candidate: &str) -> Option<String> {
    let numeric: String = candidate.chars().take_while(char::is_ascii_digit).collect();
    if numeric.is_empty() {
        return None;
    }
    let value = numeric.parse::<u32>().ok()?;
    Some(format!("directive:DIRECTIVE_{value:03}"))
}

/// Build the composed request text (`_build_request_text`), line for line.
pub fn build_request_text(
    contract: &MissionStepContract,
    context: &StepContractExecutionContext<'_>,
    step: &ContractStep,
    resolved: &[ResolvedStepDelegation],
    unresolved: &[String],
) -> String {
    let mut lines = vec![
        format!(
            "Execute mission step contract {} ({}/{}).",
            contract.id, contract.mission, contract.action
        ),
        format!("Step {}: {}", step.id, step.description),
    ];
    if let Some(request_text) = context.request_text.filter(|text| !text.is_empty()) {
        lines.push(format!("Run request: {request_text}"));
    }
    match &step.command {
        Some(command) => {
            lines.push(format!(
                "Declared command: {}",
                render_declared_command(command, step)
            ));
            lines.push(
                "Command status: declared only; the host/operator owns execution.".to_string(),
            );
        }
        None if !step.inputs.is_empty() => {
            let joined = step
                .inputs
                .iter()
                .map(StepInput::render)
                .collect::<Vec<String>>()
                .join(" ");
            lines.push(format!("Declared step inputs: {joined}"));
        }
        None => {}
    }
    if !resolved.is_empty() {
        let joined = resolved
            .iter()
            .map(|delegation| delegation.urn.clone())
            .collect::<Vec<String>>()
            .join(", ");
        lines.push(format!("Resolved delegations: {joined}"));
    }
    if !unresolved.is_empty() {
        lines.push(format!(
            "Unresolved delegation candidates: {}",
            unresolved.join(", ")
        ));
    }
    if let Some(guidance) = &step.guidance {
        lines.push(format!("Step guidance: {guidance}"));
    }
    lines.join("\n")
}

fn render_declared_command(command: &str, step: &ContractStep) -> String {
    if step.inputs.is_empty() {
        return command.to_string();
    }
    let joined = step
        .inputs
        .iter()
        .map(StepInput::render)
        .collect::<Vec<String>>()
        .join(" ");
    format!("{command} {joined}")
}

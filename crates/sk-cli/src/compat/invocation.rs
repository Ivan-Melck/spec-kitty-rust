//! Invocation/dispatch compatibility adapters.
//!
//! The composition root for the governed invocation vertical: it loads the real
//! authored data (pack mission-step contracts, mission-type action sequences,
//! the profile registry, the model-routing catalog), wires the canonical
//! doctrine/glossary/kernel authorities into the
//! [`ProfileInvocationExecutor`]
//! and [`StepContractExecutor`],
//! and exposes the `dispatch`, `profile-invocation` and dry-run command
//! interfaces. `compat::runtime::next` calls [`compose_action`] for a
//! composition-eligible step and never touches the profile executor directly.
//!
//! Everything here is a *projection over existing authorities*: no YAML
//! re-serialisation, no second router/catalogue/activation store, and no
//! generated text. Declared step commands are carried as guidance and are never
//! executed.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::json;

use sk_invocation::executor::{
    CatalogProfile, CloseContract, ConflictCandidateView, EmptyCharterFallback, EmptyCharterPort,
    ExecutorError, GlossaryObservation, GlossaryScanPort, GovernanceContext, GovernancePort,
    InvocationCatalog, InvocationRequest, ProfileInvocationExecutor, PythonJsonValue,
    SemanticConflictView, build_close_contract,
};
use sk_invocation::step_contract::{
    ContractAdmission, MissionStepContract, StepContractCatalog, StepContractExecutionContext,
    StepContractExecutionError, StepContractExecutor,
};
use sk_invocation::{ClosedBy, ModeOfWork, OpOutcome, RecordedMode};

use crate::compat::support::{self, CompatError, emit_json_pretty};

// The Rich table layout lives beside this adapter (its own file, declared here
// so the shared `compat` module needs no edit).
#[path = "rich_table.rs"]
mod rich_table;

use rich_table::{RichTable, TableColumn};

/// `spec-kitty dispatch` options (Source `dispatch.py`).
#[derive(Debug, Args)]
pub struct DispatchArgs {
    /// Natural language request. The router picks the best profile.
    #[arg(value_name = "REQUEST")]
    pub request: String,
    /// Optional profile ID; bypasses the router when the verb is ambiguous.
    #[arg(long)]
    pub profile: Option<String>,
    /// Emit the JSON payload.
    #[arg(long)]
    pub json: bool,
    /// Emit routing/orientation metadata only: replace the embedded governance
    /// text with its digest and the command that fetches it.
    #[arg(long)]
    pub brief: bool,
    /// Route the request and return the signal without opening an Op.
    #[arg(long)]
    pub dry_run: bool,
    /// Owned checkout that keeps the Op record (a worktree of this project).
    #[arg(long)]
    pub owned_checkout: Option<PathBuf>,
}

/// `spec-kitty profile-invocation complete` options (Source `profile_invocation.py`).
#[derive(Debug, Args)]
pub struct CompleteArgs {
    /// Invocation ULID to close.
    #[arg(long, short = 'i')]
    pub invocation_id: String,
    /// done | failed | abandoned (required; never silently coerced).
    #[arg(long)]
    pub outcome: String,
    /// Path to a UTF-8 text evidence file (Tier 2 promotion).
    #[arg(long)]
    pub evidence: Option<String>,
    /// Path or URL of an artifact produced by this invocation. Repeatable.
    #[arg(long = "artifact")]
    pub artifact: Vec<String>,
    /// Git commit SHA most directly produced by this invocation.
    #[arg(long)]
    pub commit: Option<String>,
    /// Emit the JSON payload.
    #[arg(long)]
    pub json: bool,
    /// Owned checkout that keeps the Op record (a worktree of this project).
    #[arg(long)]
    pub owned_checkout: Option<PathBuf>,
}

/// `spec-kitty profiles list` options (Source `profiles_cmd.py`).
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Output a JSON array.
    #[arg(long)]
    pub json: bool,
    /// Show every profile across all source layers (implies --show-available).
    #[arg(long)]
    pub all: bool,
    /// Also show available-but-not-activated profiles (annotated by state).
    #[arg(long)]
    pub show_available: bool,
}

// ---------------------------------------------------------------------------
// Authored-data loading
// ---------------------------------------------------------------------------

/// The docs tree's built-in pack root for this checkout.
///
/// `support::pack_root()` already answers "the built-in tree" (a project
/// `packs/built-in`, or `SPEC_KITTY_PACKS_ROOT`, or this checkout's tree), so it
/// is used as-is; the project-relative probe covers a project that vendors its
/// own pack tree.
pub(crate) fn built_in_pack_root(repo_root: &Path) -> Option<PathBuf> {
    let candidate = repo_root.join("packs").join("built-in");
    if candidate.join("missions").is_dir() {
        return Some(candidate);
    }
    support::pack_root().filter(|root| root.join("missions").is_dir())
}

fn mission_steps_root(repo_root: &Path, mission: &str) -> Option<PathBuf> {
    let root = built_in_pack_root(repo_root)?
        .join("missions")
        .join("mission-steps")
        .join(mission);
    root.is_dir().then_some(root)
}

/// Normalise the legacy `tasks` sub-step ids onto the single public action.
pub fn normalize_action_for_composition(step_id: &str) -> &str {
    match step_id {
        "tasks_outline" | "tasks_packages" | "tasks_finalize" => "tasks",
        other => other,
    }
}

/// The authored `action_sequence` for `mission`: every `step.yaml` with
/// `in_action_sequence: true`, ordered by `sequence_index` then `id` — the
/// reference's `project_action_sequence` over the same authored data.
pub fn action_sequence(repo_root: &Path, mission: &str) -> Vec<String> {
    let Some(root) = mission_steps_root(repo_root, mission) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut members: Vec<(i64, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("step.yaml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
            continue;
        };
        let in_sequence = value
            .get("in_action_sequence")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if !in_sequence {
            continue;
        }
        let Some(id) = value.get("id").and_then(serde_yaml_ng::Value::as_str) else {
            continue;
        };
        let index = value
            .get("sequence_index")
            .and_then(|value| value.as_i64())
            .unwrap_or(i64::MAX);
        members.push((index, id.to_string()));
    }
    members.sort();
    members.into_iter().map(|(_, id)| id).collect()
}

/// The frozen template's `(agent_profile, contract_ref)` binding for `step`.
pub fn frozen_step_binding(run_dir: &Path, step_id: &str) -> (Option<String>, Option<String>) {
    let path = run_dir.join(sk_run_store::FROZEN_TEMPLATE_FILENAME);
    let Ok(template) = sk_runtime::load_template(&path) else {
        return (None, None);
    };
    let normalized = normalize_action_for_composition(step_id);
    for step in &template.steps {
        if step.id == step_id || step.id == normalized {
            let profile = step
                .agent_profile
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let contract = step
                .contract_ref
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            return (profile, contract);
        }
    }
    (None, None)
}

/// Whether `(mission, step)` routes through composition.
///
/// The reference's `_should_dispatch_via_composition` order: the mission type's
/// authored `action_sequence` first (any mission type, C-008), then the frozen
/// template's explicit custom binding (`agent_profile` or `contract_ref`).
/// An unresolvable type falls through to the binding check, never to a refusal.
pub fn should_dispatch_via_composition(
    repo_root: &Path,
    mission: &str,
    step_id: &str,
    run_dir: Option<&Path>,
) -> bool {
    let sequence = action_sequence(repo_root, mission);
    if sequence
        .iter()
        .any(|action| action == normalize_action_for_composition(step_id))
    {
        return true;
    }
    let Some(run_dir) = run_dir else {
        return false;
    };
    let (profile, contract) = frozen_step_binding(run_dir, step_id);
    profile.is_some() || contract.is_some()
}

// ---------------------------------------------------------------------------
// Real ports
// ---------------------------------------------------------------------------

/// The canonical invocation catalogue: Track A's `ProfileRegistry`, projected.
///
/// The registry is the one loader (activation gate, legacy `.kittify/profiles`
/// overlay, language scope, exact strings) and it also produces the exact router
/// snapshot. This adapter adds no gate, overlay or normalization of its own.
#[derive(Clone, Debug)]
pub struct RegistryCatalog {
    registry: sk_invocation::catalogue::ProfileRegistry,
}

impl RegistryCatalog {
    /// Load every profile `repo_root` resolves.
    pub fn load(repo_root: &Path) -> Result<Self, CompatError> {
        let registry = sk_invocation::catalogue::ProfileRegistry::load(repo_root)
            .map_err(|error| CompatError::coded("PROFILE_CATALOGUE_FAILED", error.to_string()))?;
        Ok(Self { registry })
    }

    /// The underlying registry (provenance, skips, exact ids).
    pub fn registry(&self) -> &sk_invocation::catalogue::ProfileRegistry {
        &self.registry
    }
}

impl InvocationCatalog for RegistryCatalog {
    fn routing_profiles(&self) -> Vec<sk_invocation::RoutableProfile> {
        self.registry.routing_profiles()
    }

    fn profile(&self, profile_id: &sk_invocation::PythonString) -> Option<CatalogProfile> {
        self.registry
            .get(profile_id)
            .map(sk_invocation::executor::project_profile)
    }
}

/// The governance-context port over the canonical action-context builder.
///
/// Read-only: `build_action_context` assembles and returns a value; nothing here
/// writes `context-state.json`, so a first-load token can never be claimed.
pub struct GovernanceAdapter {
    repo_root: PathBuf,
    mission_type: String,
}

impl GovernanceAdapter {
    /// Build the adapter for one Mission type.
    pub fn new(repo_root: &Path, mission_type: &str) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            mission_type: mission_type.to_string(),
        }
    }
}

impl GovernancePort for GovernanceAdapter {
    fn build_context(
        &self,
        profile_id: &sk_kernel::PythonString,
        action: &str,
    ) -> GovernanceContext {
        // The reference passes the exact id; a non-scalar id is simply not found
        // by the profile-cited section resolver, so build the context without a
        // profile rather than substituting a different id.
        let scalar = profile_id.to_scalar_string().ok();
        match crate::compat::runtime::action_context(
            &self.repo_root,
            &self.mission_type,
            action,
            scalar.as_deref(),
        ) {
            Ok(facts) => GovernanceContext {
                text: Some(crate::compat::runtime::render_governance(&facts)),
                mode: context_mode(facts.context.mode),
            },
            Err(_) => GovernanceContext::missing(),
        }
    }
}

fn context_mode(mode: sk_doctrine::ContextMode) -> String {
    match mode {
        sk_doctrine::ContextMode::Bootstrap => "bootstrap".to_string(),
        sk_doctrine::ContextMode::Compact => "compact".to_string(),
        // Unreachable on this path: the invocation build is a single combined
        // context build, which never takes the text-only `missing` rail. The
        // arm exists because the mode enum is closed.
        sk_doctrine::ContextMode::Missing => "missing".to_string(),
    }
}

/// The glossary chokepoint port.
pub struct GlossaryAdapter {
    chokepoint: RefCell<sk_glossary::GlossaryChokepoint>,
}

impl GlossaryAdapter {
    /// Build the adapter over `repo_root`'s glossary seeds.
    pub fn new(repo_root: &Path) -> Self {
        Self {
            chokepoint: RefCell::new(sk_glossary::GlossaryChokepoint::new(repo_root)),
        }
    }
}

/// `semantic_conflict_to_dict`'s scalar fields, with the reference's enum
/// `value` spellings, so the executor can keep the dict's insertion order.
fn conflict_view(conflict: &sk_glossary::GlossaryConflict) -> SemanticConflictView {
    SemanticConflictView {
        surface_text: conflict.term.surface_text.clone(),
        conflict_type: match conflict.conflict_type {
            sk_glossary::ConflictType::Unknown => "unknown".to_owned(),
            sk_glossary::ConflictType::Ambiguous => "ambiguous".to_owned(),
            sk_glossary::ConflictType::UnresolvedCritical => "unresolved_critical".to_owned(),
        },
        severity: match conflict.severity {
            sk_glossary::Severity::Low => "low".to_owned(),
            sk_glossary::Severity::Medium => "medium".to_owned(),
            sk_glossary::Severity::High => "high".to_owned(),
        },
        confidence: conflict.confidence,
        candidate_senses: conflict
            .candidate_senses
            .iter()
            .map(|candidate| ConflictCandidateView {
                surface: candidate.surface.clone(),
                scope: candidate.scope.as_str().to_owned(),
                definition: candidate.definition.clone(),
                confidence: candidate.confidence,
            })
            .collect(),
        context: conflict.context.clone(),
    }
}

impl GlossaryScanPort for GlossaryAdapter {
    fn scan(&self, request_text: &str, invocation_id: &str, actor_id: &str) -> GlossaryObservation {
        let bundle = self
            .chokepoint
            .borrow_mut()
            .run(request_text, invocation_id, actor_id);
        GlossaryObservation {
            matched_urns: bundle.matched_urns.clone(),
            high_severity: bundle
                .high_severity
                .iter()
                .map(conflict_view)
                .map(|view| view.to_python_json())
                .collect(),
            all_conflicts: bundle
                .all_conflicts
                .iter()
                .map(conflict_view)
                .map(|view| view.to_python_json())
                .collect(),
            tokens_checked: bundle.tokens_checked as u64,
            duration_ms: bundle.duration_ms,
            error_msg: bundle.error_msg.clone(),
        }
    }
}

/// The empty-charter generic fallback, read from real project configuration.
pub struct EmptyCharterAdapter {
    repo_root: PathBuf,
}

impl EmptyCharterAdapter {
    /// Build the adapter over one project root.
    pub fn new(repo_root: &Path) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
        }
    }
}

impl EmptyCharterPort for EmptyCharterAdapter {
    fn fallback(&self, request_text: &str) -> Option<EmptyCharterFallback> {
        if self
            .repo_root
            .join(".kittify/charter/charter.yaml")
            .exists()
        {
            return None;
        }
        let (_, org_packs, _) =
            crate::compat::runtime::doctrine_roots(&self.repo_root, None).ok()?;
        if !org_packs.is_empty() {
            return None;
        }
        let ledger = sk_doctrine::load_activation_ledger(&self.repo_root).ok()?;
        let kind = sk_charter::ArtifactKind::AgentProfile;
        if ledger.allow_for(kind).is_some() || !ledger.deny_for(kind).is_empty() {
            return None;
        }
        let tokens = sk_invocation::normalize_tokens(&sk_invocation::PythonString::from_scalars(
            request_text,
        ));
        let action = tokens
            .iter()
            .find_map(sk_invocation::canonical_verb)
            .map(|matched| matched.action)
            .unwrap_or("review");
        Some(EmptyCharterFallback {
            profile_id: GENERIC_AGENT_ID.to_string(),
            action: action.to_string(),
            match_reason: EMPTY_CHARTER_REASON.to_string(),
        })
    }
}

/// The profile pinned when the charter is wholly empty.
pub const GENERIC_AGENT_ID: &str = "generic-agent";

/// The reference's recorded match reason for the generic fallback.
pub const EMPTY_CHARTER_REASON: &str = "empty charter: no compiled charter bundle (.kittify/charter/charter.yaml), no org/project pack, and no explicit agent-profile activation found";

/// Read the advisory model-routing catalog from the checkout's authored data.
///
/// Missing or malformed data is the reference's non-fatal envelope: advisory
/// routing simply produces no recommendation.
pub fn load_model_catalog(
    repo_root: &Path,
) -> Option<sk_invocation::executor::ModelRoutingCatalog> {
    let path =
        repo_root.join("src/charter/offering/model_task_routing/catalog/model-to-task_type.yaml");
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml_ng::from_str(&text).ok()
}

/// Load every mission-step contract visible to `repo_root`.
///
/// Layer order is built-in, then each configured org pack, then the project
/// pack; a later layer replaces an earlier contract with the same id in place.
pub fn load_step_contracts(repo_root: &Path) -> Result<StepContractCatalog, CompatError> {
    let mut contracts: Vec<MissionStepContract> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Some(built_in) = built_in_pack_root(repo_root) {
        directories.push(built_in.join("missions").join("built_in_step_contracts"));
    }
    if let Ok((_, org_packs, project)) = crate::compat::runtime::doctrine_roots(repo_root, None) {
        for pack in &org_packs {
            directories.push(pack.root.join("mission_step_contracts"));
        }
        if let Some(project_root) = project {
            directories.push(project_root.join("mission_step_contracts"));
        }
    }
    directories.push(repo_root.join(".kittify/charter-packs/mission_step_contracts"));
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".step-contract.yaml"))
            })
            .collect();
        files.sort();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(contract) = serde_yaml_ng::from_str::<MissionStepContract>(&text) else {
                continue;
            };
            if contract.validate().is_err() {
                continue;
            }
            match index.get(&contract.id) {
                Some(position) => contracts[*position] = contract,
                None => {
                    index.insert(contract.id.clone(), contracts.len());
                    contracts.push(contract);
                }
            }
        }
    }
    Ok(StepContractCatalog::from_contracts(contracts))
}

/// Whether the ledger's allow/deny state admits one contract (`_contract_admitted`).
///
/// A contract has two spellings (`id`, `<mission>/<action>`); a deny of either
/// excludes it, and an allow list admits it when it names either one.
pub fn contract_admitted(
    ledger: &sk_doctrine::ActivationLedger,
    contract: &MissionStepContract,
) -> bool {
    use sk_charter::ArtifactKind;
    let kind = ArtifactKind::MissionStepContract;
    let spellings = contract.spellings();
    let deny = ledger.deny_for(kind);
    if spellings.iter().any(|spelling| {
        deny.iter().any(|token| {
            token == spelling || token.trim_start_matches("mission_step_contract:") == spelling
        })
    }) {
        return false;
    }
    match ledger.allow_for(kind) {
        None => true,
        Some(allowed) => spellings.iter().any(|spelling| {
            allowed.iter().any(|token| {
                token == spelling || token.trim_start_matches("mission_step_contract:") == spelling
            })
        }),
    }
}

// ---------------------------------------------------------------------------
// Executor assembly
// ---------------------------------------------------------------------------

/// The real, fully wired invocation services for one project root + Mission type.
pub struct InvocationServices {
    /// The canonical profile catalogue.
    pub catalog: RegistryCatalog,
    /// The governance-context port.
    pub governance: GovernanceAdapter,
    /// The glossary chokepoint port.
    pub glossary: GlossaryAdapter,
    /// The empty-charter generic fallback.
    pub empty_charter: EmptyCharterAdapter,
    /// The loaded mission-step contracts.
    pub contracts: StepContractCatalog,
    /// The activation-filtered merged DRG, when one loads.
    pub graph: Option<sk_doctrine::DrgGraph>,
    /// The advisory model-routing catalog, when present and valid.
    pub model_catalog: Option<sk_invocation::executor::ModelRoutingCatalog>,
    /// The activation ledger, when it reads.
    pub ledger: Option<sk_doctrine::ActivationLedger>,
    /// The project root.
    pub repo_root: PathBuf,
    /// The Mission type.
    pub mission_type: String,
}

impl InvocationServices {
    /// Load every authority this vertical needs for `(repo_root, mission_type)`.
    pub fn load(repo_root: &Path, mission_type: &str) -> Result<Self, CompatError> {
        let catalog = RegistryCatalog::load(repo_root)?;
        let contracts = load_step_contracts(repo_root)?;
        let graph = crate::compat::runtime::filtered_graph(repo_root).ok();
        let model_catalog = load_model_catalog(repo_root);
        let ledger = sk_doctrine::load_activation_ledger(repo_root).ok();
        Ok(Self {
            catalog,
            governance: GovernanceAdapter::new(repo_root, mission_type),
            glossary: GlossaryAdapter::new(repo_root),
            empty_charter: EmptyCharterAdapter::new(repo_root),
            contracts,
            graph,
            model_catalog,
            ledger,
            repo_root: repo_root.to_path_buf(),
            mission_type: mission_type.to_string(),
        })
    }
}

/// Build the wired profile executor for one checkout.
pub fn executor<'a>(
    services: &'a InvocationServices,
    record_root: &Path,
) -> ProfileInvocationExecutor<'a> {
    let mut executor = ProfileInvocationExecutor::new(record_root, services.repo_root.clone())
        .with_catalog(&services.catalog)
        .with_glossary(&services.glossary)
        .with_governance(&services.governance)
        .with_empty_charter(&services.empty_charter);
    if let Some(catalog) = &services.model_catalog {
        executor = executor.with_model_catalog(catalog);
    }
    executor
}

/// Compose one action through the real contract executor.
///
/// Returns the invocation ids and contract identity on success, or the
/// structured failure text the `next` blocked decision renders. The post-action
/// guard is the caller's (`compat::runtime`), matching the reference's ordering:
/// execute, then guard, then advance.
pub fn compose_action(
    services: &InvocationServices,
    action: &str,
    actor: &str,
    record_root: &Path,
    profile_hint: Option<&str>,
    request_text: Option<&str>,
) -> Result<CompositionOutcome, String> {
    let mut invocation = executor(services, record_root);
    let admission = services
        .ledger
        .as_ref()
        .map(|ledger| LedgerAdmission { ledger });
    let mut step_executor =
        StepContractExecutor::new(&services.repo_root, &services.contracts, &mut invocation);
    if let Some(graph) = &services.graph {
        step_executor = step_executor.with_graph(graph);
    }
    if let Some(admission) = &admission {
        step_executor = step_executor.with_admission(admission);
    }
    let context = StepContractExecutionContext {
        repo_root: &services.repo_root,
        mission: &services.mission_type,
        action,
        actor,
        profile_hint,
        request_text,
        mode_of_work: Some(ModeOfWork::MissionStep),
        resolution_depth: 2,
    };
    match step_executor.execute(&context, None) {
        Ok(result) => {
            let invocation_ids = result
                .invocation_ids()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let unresolved_candidates = result
                .steps
                .iter()
                .flat_map(|step| step.unresolved_candidates.clone())
                .collect();
            Ok(CompositionOutcome {
                contract_id: result.contract_id,
                resolved_profile: result.profile_hint,
                invocation_ids,
                unresolved_candidates,
            })
        }
        Err(error) => Err(describe_composition_failure(
            &services.mission_type,
            action,
            &error,
        )),
    }
}

/// The composed-action outcome the `next` dispatch threads onward.
#[derive(Clone, Debug)]
pub struct CompositionOutcome {
    /// The executed contract id.
    pub contract_id: String,
    /// The resolved profile hint.
    pub resolved_profile: String,
    /// Every Op id the run emitted, in step order.
    pub invocation_ids: Vec<String>,
    /// Candidates that did not resolve (a warning upstream, never fatal).
    pub unresolved_candidates: Vec<String>,
}

fn describe_composition_failure(
    mission: &str,
    action: &str,
    error: &StepContractExecutionError,
) -> String {
    match error {
        StepContractExecutionError::Executor(ExecutorError::OpWriteRefused(refusal)) => {
            format!("composition crashed for {mission}/{action}: OpWriteRefused: {refusal}")
        }
        StepContractExecutionError::Executor(other) => {
            format!("composition crashed for {mission}/{action}: {other}")
        }
        other => format!("composition failed for {mission}/{action}: {other}"),
    }
}

struct LedgerAdmission<'a> {
    ledger: &'a sk_doctrine::ActivationLedger,
}

impl ContractAdmission for LedgerAdmission<'_> {
    fn admits(&self, contract: &MissionStepContract) -> bool {
        contract_admitted(self.ledger, contract)
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn parse_outcome(raw: &str) -> Result<OpOutcome, CompatError> {
    OpOutcome::parse(raw).ok_or_else(|| {
        CompatError::usage(format!(
            "invalid outcome {raw:?}: must be one of done, failed, abandoned"
        ))
    })
}

/// Source `_detect_actor`: the caller identity from the harness environment.
pub fn detect_actor() -> &'static str {
    if std::env::var_os("CLAUDE_CODE_ENTRYPOINT").is_some() {
        return "claude";
    }
    if std::env::var_os("CODEX_CLI").is_some() {
        return "codex";
    }
    "operator"
}

/// Source `resolve_owned_op_record_root`: the invoking checkout's Op root, or
/// the explicit owned checkout when one is supplied (and is a worktree of this
/// project).
fn op_record_root(
    repo_root: &Path,
    owned_checkout: Option<&Path>,
    json_output: bool,
) -> Result<PathBuf, CompatError> {
    if let Some(owned) = owned_checkout {
        if !owned.is_dir() {
            let message = format!("--owned-checkout {} is not a directory", owned.display());
            return Err(CompatError::coded("OWNED_CHECKOUT_INVALID", message));
        }
        if !sk_invocation::is_worktree_of(repo_root, owned) {
            let message = format!(
                "--owned-checkout {} is not a worktree of this repository",
                owned.display()
            );
            return Err(CompatError::coded("OWNED_CHECKOUT_INVALID", message));
        }
        let _ = json_output;
        return Ok(owned.to_path_buf());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| repo_root.to_path_buf());
    Ok(sk_invocation::resolve_op_record_root(repo_root, &cwd))
}

/// The Mission type used for a bare `dispatch` governance read.
///
/// Source's dispatch resolves governance by action, not by Mission; the native
/// action-context builder also wants a Mission type for the action URN. A
/// project that declares `mission_type` (or `default_mission_type`) in
/// `.kittify/config.yaml` supplies it; otherwise the software-dev default the
/// charter ships applies. No Mission metadata is fabricated.
fn mission_type_for_root(repo_root: &Path) -> String {
    let path = repo_root.join(".kittify/config.yaml");
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        && let Some(mission_type) = value
            .get("mission_type")
            .or_else(|| value.get("default_mission_type"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
    {
        return mission_type.to_string();
    }
    "software-dev".to_string()
}

/// Source `_governance_reference` (#6990): the digest/size/fetch reference that
/// replaces the re-embedded governance text in `--brief` mode.
///
/// Returns `None` — "leave the payload alone" — when there is no text, the
/// context was unavailable, or the empty-charter fallback produced it (its
/// suppressed-project-resolver render cannot be reproduced by `charter context`).
pub fn governance_reference(
    payload: &sk_invocation::executor::InvocationPayload,
) -> Option<PythonJsonValue> {
    let text = payload.governance_context_text.as_deref().unwrap_or("");
    if !payload.governance_context_available || text.trim().is_empty() {
        return None;
    }
    if payload.empty_charter_fallback {
        return None;
    }
    let mut command = sk_kernel::PythonString::from_scalars(&format!(
        "spec-kitty charter context --action {} --profile ",
        payload.action
    ));
    command.append(&payload.profile_id);
    command.push_scalars(" --json --no-mark-loaded");
    Some(PythonJsonValue::Object(vec![
        (
            "digest".to_owned(),
            PythonJsonValue::optional_scalar(payload.governance_context_hash.as_deref()),
        ),
        (
            "bytes".to_owned(),
            PythonJsonValue::Number(text.len().to_string()),
        ),
        (
            "lines".to_owned(),
            PythonJsonValue::Number(text.lines().count().to_string()),
        ),
        ("fetch_command".to_owned(), PythonJsonValue::String(command)),
    ]))
}

/// Apply `--brief` to an already-built envelope (`_apply_governance_brief`).
pub fn apply_governance_brief(
    envelope: &mut PythonJsonValue,
    payload: &sk_invocation::executor::InvocationPayload,
) {
    let Some(reference) = governance_reference(payload) else {
        return;
    };
    envelope.set("governance_context_text", PythonJsonValue::Null);
    envelope.set("governance_context_reference", reference);
}

/// Write an exact `json.dumps` value plus the trailing newline `typer.echo` adds.
fn emit_python_json(value: &PythonJsonValue, indent: Option<usize>) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(value.render(indent).as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .and_then(|_| stdout.flush())
    {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// `spec-kitty dispatch`: route, assemble governance/glossary, open the Op.
pub fn run_dispatch(args: &DispatchArgs) -> Result<i32, CompatError> {
    if let Some(code) = fail_on_invalid_hash_seed() {
        return Ok(code);
    }
    let repo_root = support::project_root()?;
    let mission_type = mission_type_for_root(&repo_root);
    let services = InvocationServices::load(&repo_root, &mission_type)?;
    let record_root = op_record_root(&repo_root, args.owned_checkout.as_deref(), args.json)?;
    let mut invocation = executor(&services, &record_root);
    let profile_hint = args
        .profile
        .as_deref()
        .map(sk_kernel::PythonString::from_scalars);
    let request = InvocationRequest {
        request_text: &args.request,
        profile_hint: profile_hint.as_ref(),
        actor: detect_actor(),
        // `derive_mode("dispatch")` is the standalone task-execution Op.
        mode_of_work: Some(ModeOfWork::TaskExecution),
        action_hint: None,
        mission_id: None,
        wp_id: None,
    };
    let payload = if args.dry_run {
        invocation
            .dry_run(&request)
            .map_err(|error| executor_error(&error, args.json))?
    } else {
        invocation
            .invoke(&request)
            .map_err(|error| executor_error(&error, args.json))?
    };
    let mut envelope = payload.to_python_json(!args.dry_run);
    if args.brief {
        apply_governance_brief(&mut envelope, &payload);
    }
    if args.json {
        // The real (Op-opening) envelope is pretty-printed; the dry run uses
        // `json.dumps` defaults, exactly like the reference's two call sites.
        let indent = if args.dry_run { None } else { Some(2) };
        emit_python_json(&envelope, indent)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if args.dry_run {
        println!(
            "dry run: {} → {} ({})",
            payload.profile_id,
            payload.action,
            payload.router_confidence.as_deref().unwrap_or("explicit")
        );
    } else {
        println!(
            "Op {} open: {} → {}",
            payload.invocation_id, payload.profile_id, payload.action
        );
        println!(
            "Close: {}",
            close_contract(&payload.invocation_id, payload.mode_of_work.as_deref()).command
        );
    }
    Ok(0)
}

/// `spec-kitty profile-invocation complete`: close the Op and commit its record.
pub fn run_complete(args: &CompleteArgs) -> Result<i32, CompatError> {
    if let Some(code) = fail_on_invalid_hash_seed() {
        return Ok(code);
    }
    let outcome = parse_outcome(&args.outcome)?;
    let repo_root = support::project_root()?;
    let mission_type = mission_type_for_root(&repo_root);
    let services = InvocationServices::load(&repo_root, &mission_type)?;
    let invoking_root = op_record_root(&repo_root, args.owned_checkout.as_deref(), args.json)?;
    // Source closes the Op in the checkout that holds its record, judging
    // admission there; a record no checkout holds keeps the not-found error.
    let record_root =
        sk_invocation::locate_op_record_root(&repo_root, &args.invocation_id, Some(&invoking_root))
            .unwrap_or(invoking_root);
    let mut invocation = executor(&services, &record_root);
    let request = sk_invocation::executor::CompletionRequest {
        invocation_id: &args.invocation_id,
        outcome,
        evidence_ref: args.evidence.as_deref(),
        artifact_refs: &args.artifact,
        commit_sha: args.commit.as_deref(),
        closed_by: ClosedBy::Agent,
    };
    let completed = match invocation.complete_invocation(&request) {
        Ok(completed) => completed,
        Err(error) => {
            // Already-closed is the reference's dedicated exit-1 envelope.
            if let sk_invocation::executor::ExecutorError::AlreadyClosed(_) = error {
                if args.json {
                    let _ = emit_json_pretty(&json!({
                        "error": "already_closed",
                        "invocation_id": args.invocation_id,
                    }));
                }
                eprintln!(
                    "Error: Invocation {} is already closed.",
                    args.invocation_id
                );
                return Ok(1);
            }
            return Err(executor_error(&error, args.json));
        }
    };
    let commit_outcome = invocation.last_op_commit().cloned();
    if !args.json
        && let Some(commit_outcome) = &commit_outcome
        && !commit_outcome.committed()
    {
        print_uncommitted_note(commit_outcome, &args.invocation_id);
    }
    let response = json!({
        "result": "success",
        "invocation_id": args.invocation_id,
        "outcome": args.outcome,
        "evidence_ref": completed.completed.evidence_ref(),
        "artifact_links": args
            .artifact
            .iter()
            .map(|artifact| sk_invocation::normalise_ref(artifact, &record_root))
            .collect::<Vec<String>>(),
        "commit_link": args.commit,
    });
    if args.json {
        emit_json_pretty(&response)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("✓ Invocation {} closed.", args.invocation_id);
        println!("  Outcome: {}", args.outcome);
        for artifact in &args.artifact {
            println!(
                "  Artifact: {}",
                sk_invocation::normalise_ref(artifact, &record_root)
            );
        }
        if let Some(commit) = &args.commit {
            println!("  Commit: {commit}");
        }
    }
    Ok(0)
}

/// Source `_print_uncommitted_note`: say where the record went, never advise
/// overriding the protected-branch guard.
fn print_uncommitted_note(outcome: &sk_invocation::OpCommitOutcome, invocation_id: &str) {
    let record = format!("kitty-ops/{invocation_id}.jsonl");
    if let Some(skipped) = outcome.skipped() {
        println!("Op record kept on disk at {record}, not committed: {skipped}.");
        return;
    }
    println!(
        "Warning: Op record auto-commit was refused ({}). The completed event is on disk at {record} but uncommitted. Commit the Op record yourself on the branch where it belongs.",
        outcome.refusal().unwrap_or("unknown refusal")
    );
}

/// `spec-kitty profiles list`: the Source descriptor array.
pub fn run_list(args: &ListArgs) -> Result<i32, CompatError> {
    // The reference's interpreter refuses to start on an unusable
    // `PYTHONHASHSEED`, so no command body runs at all (exit 1, empty stdout).
    if let Some(code) = fail_on_invalid_hash_seed() {
        return Ok(code);
    }
    let repo_root = support::project_root()?;
    let registry = RegistryCatalog::load(&repo_root)?;
    let registry = registry.registry();
    let ledger = sk_doctrine::load_activation_ledger(&repo_root).ok();
    let kind = sk_charter::ArtifactKind::AgentProfile;
    let allowed = ledger.as_ref().and_then(|ledger| ledger.allow_for(kind));
    let denied: &[String] = ledger
        .as_ref()
        .map(|ledger| ledger.deny_for(kind))
        .unwrap_or(&[]);
    let show_available = args.all || args.show_available;
    // `list_all()` sorts by the Python id; `PythonString` ordering is the same
    // code-point order, including non-scalar ids.
    let mut ids = registry.ids();
    ids.sort();
    let mut descriptors: Vec<PythonJsonValue> = Vec::new();
    for id in ids {
        let Some(profile) = registry.get(&id) else {
            continue;
        };
        let mut descriptor = profile_descriptor(profile, registry.provenance(&id));
        let is_activated = profile_activated(&id, allowed, denied);
        if !show_available && !is_activated {
            continue;
        }
        if show_available {
            descriptor.set(
                "state",
                PythonJsonValue::scalar(if is_activated {
                    "activated"
                } else {
                    "available"
                }),
            );
        }
        descriptors.push(descriptor);
    }
    if args.json {
        emit_python_json(&PythonJsonValue::Array(descriptors), Some(2))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if descriptors.is_empty() {
        println!(
            "No profiles found. Run 'spec-kitty charter synthesize' to create project-local profiles."
        );
    } else {
        // The reference renders the whole Rich table in one write. A lone
        // surrogate in a row identity makes that write raise `UnicodeEncodeError`,
        // so no byte reaches stdout and the command exits nonzero. Match that
        // observable contract — never substitute a `repr` or a replacement
        // character, and never write a partial table.
        if let Some((key, exact)) = descriptors.iter().find_map(|descriptor| {
            ["profile_id", "name", "role"]
                .into_iter()
                .find_map(|key| match descriptor.field(key) {
                    Some(PythonJsonValue::String(exact)) if exact.to_scalar_string().is_err() => {
                        Some((key, exact.clone()))
                    }
                    _ => None,
                })
        }) {
            return Err(CompatError::coded(
                "PROFILE_CONSOLE_ENCODING",
                format!(
                    "the human console view cannot encode the {key} {} as UTF-8; \
                     the reference raises UnicodeEncodeError and writes no table \
                     (use --json for the exact escaped wire)",
                    exact.repr()
                ),
            ));
        }
        // The reference's `_render_list` / `_render_list_annotated` tables.
        let mut columns = vec![
            TableColumn::unwrapped("Profile ID"),
            TableColumn::wrapped("Friendly Name"),
            TableColumn::wrapped("Role"),
            TableColumn::wrapped("Source"),
        ];
        if show_available {
            columns.push(TableColumn::wrapped("State"));
        }
        let mut table = RichTable::new("Agent Profiles", columns);
        for descriptor in &descriptors {
            let mut cells = vec![
                descriptor_field(descriptor, "profile_id"),
                descriptor_field(descriptor, "name"),
                descriptor_field(descriptor, "role"),
                descriptor_field(descriptor, "source"),
            ];
            if show_available {
                cells.push(descriptor_field(descriptor, "state"));
            }
            table.push_row(cells);
        }
        let width = super::console_text::width_from_environment().map_err(CompatError::domain)?;
        if width == 0 {
            return Ok(0);
        }
        for line in table.render(width) {
            println!("{line}");
        }
    }
    Ok(0)
}

/// `PYTHONHASHSEED` start-up validation, byte-identical to the reference.
///
/// CPython resolves the hash secret before `main` runs, so an invalid value is a
/// *fatal start-up* error: exit 1, no stdout, and the two-line fatal message on
/// stderr. A command that can construct a `!!set` calls this first, so the seed
/// can never silently become the all-zero secret.
fn fail_on_invalid_hash_seed() -> Option<i32> {
    let error = sk_doctrine::validate_hash_seed_from_env().err()?;
    // The reference's own fatal line, then exit 1 with no other output.
    eprint!("{error}");
    Some(1)
}

/// The scalar text of a descriptor field for the human console view.
///
/// The caller has already refused a row whose identity holds a lone surrogate
/// (the reference's Rich table write raises `UnicodeEncodeError` and writes
/// nothing), so this only ever renders encodable text. The JSON view above is
/// the exact escaped wire and never projects.
fn descriptor_field(descriptor: &PythonJsonValue, key: &str) -> String {
    descriptor
        .field(key)
        .and_then(exact_text)
        .unwrap_or_default()
}

fn exact_text(value: &PythonJsonValue) -> Option<String> {
    match value {
        PythonJsonValue::String(exact) => exact.to_scalar_string().ok(),
        _ => None,
    }
}

/// Source `_build_descriptor`: the legacy descriptor row, schema preserved.
///
/// Every authored string is exact. `action_domains` is the sorted set of the
/// role-capability verbs plus the profile's collaboration verbs and
/// specialization keywords, matching `_build_descriptor`'s set arithmetic.
fn profile_descriptor(
    profile: &sk_doctrine::AgentProfile,
    source_layer: Option<&'static str>,
) -> PythonJsonValue {
    let profile_id = profile.id.clone();
    // `_build_descriptor` emits `p.name` verbatim; the model requires the field,
    // so an authored empty name is the empty string, never the profile id.
    let name = profile.name().unwrap_or_default();
    let role = profile.role().unwrap_or_default();
    let mut domains: Vec<sk_kernel::PythonString> = Vec::new();
    if let Ok(scalar) = role.to_scalar_string()
        && let Some(row) = sk_doctrine::role_capabilities(&scalar)
    {
        domains.extend(
            row.canonical_verbs
                .iter()
                .map(|verb| sk_kernel::PythonString::from_scalars(verb)),
        );
    }
    domains.extend(profile.canonical_verbs());
    domains.extend(profile.domain_keywords());
    domains.sort();
    domains.dedup();
    let source = match source_layer {
        Some("project") => "project_local",
        Some("org") => "org",
        _ => "built-in",
    };
    PythonJsonValue::Object(vec![
        (
            "profile_id".to_owned(),
            PythonJsonValue::string(profile_id.clone()),
        ),
        ("identifier".to_owned(), PythonJsonValue::string(profile_id)),
        ("name".to_owned(), PythonJsonValue::string(name)),
        ("role".to_owned(), PythonJsonValue::string(role)),
        (
            "action_domains".to_owned(),
            PythonJsonValue::Array(domains.into_iter().map(PythonJsonValue::string).collect()),
        ),
        ("source".to_owned(), PythonJsonValue::scalar(source)),
    ])
}

fn profile_activated(
    profile_id: &sk_kernel::PythonString,
    allowed: Option<&[String]>,
    denied: &[String],
) -> bool {
    // Source compares the exact Python id against the ledger's exact tokens.
    // A non-scalar id has no `String` token, so it can neither match nor be
    // denied by a scalar ledger entry; an absent allow set still admits it.
    let Ok(profile_id) = profile_id.to_scalar_string() else {
        return allowed.is_none();
    };
    let spellings = [profile_id.clone(), format!("agent_profile:{profile_id}")];
    if spellings.iter().any(|spelling| {
        denied.iter().any(|token| {
            token == spelling || token.trim_start_matches("agent_profile:") == profile_id
        })
    }) {
        return false;
    }
    match allowed {
        None => true,
        Some(allowed) => spellings.iter().any(|spelling| {
            allowed.iter().any(|token| {
                token == spelling || token.trim_start_matches("agent_profile:") == profile_id
            })
        }),
    }
}

fn executor_error(error: &ExecutorError, json_output: bool) -> CompatError {
    // A protected-branch admission refusal is the reference's dedicated
    // error envelope (code + record root), not a generic invocation failure.
    if let ExecutorError::OpWriteRefused(refusal) = error {
        let _ = json_output;
        return CompatError::coded("OP_WRITE_PROTECTED_BRANCH", refusal.to_string());
    }
    CompatError::coded("PROFILE_INVOCATION_FAILED", error.to_string())
}

fn close_contract(invocation_id: &str, mode: Option<&str>) -> CloseContract {
    build_close_contract(invocation_id, mode)
}

/// Whether a recorded mode permits a Tier 2 evidence artifact.
pub fn recorded_mode_permits_evidence(mode: &RecordedMode) -> bool {
    mode.permits_evidence()
}

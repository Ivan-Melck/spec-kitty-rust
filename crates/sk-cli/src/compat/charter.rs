//! `spec-kitty charter` adapters: context, activation reads and safe
//! activation/deactivation mutations on the canonical Doctrine authorities.
//!
//! Mirrors the reference CLI package `src/specify_cli/cli/commands/charter/`
//! (`context.py`, `list_cmd.py`, `activate.py`, `deactivate.py`) on top of the
//! delivered Native primitives:
//!
//! * context resolution and its versioned JSON contract:
//!   [`sk_doctrine::build_action_context`];
//! * text rendering: [`sk_doctrine::context_render`];
//! * activation planning + the one locked lossless write:
//!   [`sk_doctrine::ActivationTransaction`], [`sk_doctrine::plan_activation`],
//!   [`sk_doctrine::plan_deactivation`];
//! * cascade traversal: [`sk_doctrine::cascade_activation_targets`],
//!   [`sk_doctrine::deactivation_plan`];
//! * the activation ledger and org required floor.
//!
//! This module owns no second profile/YAML/activation model. Every mutation is
//! planned purely, committed through the canonical observation-checked
//! transaction (which takes the cross-language charter synthesis lock), and
//! preserves the rest of the authored document byte-for-byte through the
//! lossless YAML writer.
//!
//! Registration in the root CLI enum is parent-owned; the typed command tree
//! and [`run`] are public so a parent only adds `Charter { command }` and one
//! dispatch arm.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};
use sk_kernel::python_json::{PythonJsonRenderOptions, PythonJsonValue};

use sk_charter::{ArtifactKind, CHARTER_YAML};
use sk_doctrine::{
    ActivationInputs, ActivationState, ActivationTransaction, CascadeActivationResult,
    CascadeScope, DeactivationInputs, DenyListChoice, DoctrineError, EffectiveSet,
    EffectiveWhenAbsent, IdentityIndex, NativeOfferingService, OfferingRoots, TextRail,
    YamlDocument, activation_write_target, build_text_stage_with_presence,
    cascade_activation_targets, charter_present, check_deactivation_permitted, deactivation_plan,
    effective_deny, load_activation_ledger, load_reference_catalog, mission_type_fallback,
    plan_activation, plan_deactivation, read_org_required_provenance, referenced_but_not_cascaded,
    resolve_activation_source, resolve_effective_sets,
};

use crate::compat::runtime::{
    DoctrineBundle, active_project_languages, doctrine_bundle, doctrine_roots, packs_root,
};
use crate::compat::support::{CompatError, emit_json_pretty, project_root};

mod context_include;
mod list_availability;
mod list_templates;
mod preset;
mod status;

/// The composed `charter list` view (payload + its text cells), public so the
/// compatibility tests exercise the exact rendering seam the command uses.
pub use list_availability::Listing;

/// Local success-payload schema version of the change report.
const SCHEMA_VERSION: &str = "1.0.0";
/// The mission-type activation token (part of the operator vocabulary).
const MISSION_TYPE_TOKEN: &str = "mission-type";
/// The mission-type activation ledger key.
const MISSION_TYPE_KEY: &str = "mission_type_activations";

// Local refusal codes (`contracts/cli-json.md`), UPPER_SNAKE and core-local.
const ACTIVATION_KIND_UNKNOWN: &str = "ACTIVATION_KIND_UNKNOWN";
const CASCADE_SCOPE_INVALID: &str = "CASCADE_SCOPE_INVALID";
const CHARTER_WRITE_ROOT_INVALID: &str = "CHARTER_WRITE_ROOT_INVALID";
const DEACTIVATION_REFUSED_REQUIRED: &str = "DEACTIVATION_REFUSED_REQUIRED";
const ACTIVATION_FAILED: &str = "ACTIVATION_FAILED";
const ACTIVE_CHARTER_CONFIG_INVALID: &str = "ACTIVE_CHARTER_CONFIG_INVALID";

/// The `charter` command group.
#[derive(Debug, Subcommand)]
pub enum CharterCommand {
    /// Render charter context for a workflow action.
    Context(ContextArgs),
    /// List activated charter artifacts by kind.
    List(ListArgs),
    /// Activate a doctrine artifact by kind and ID.
    Activate(ActivateArgs),
    /// Deactivate a doctrine artifact by kind and ID.
    Deactivate(DeactivateArgs),
    /// Read-only charter sync/synthesis/freshness/activation status.
    Status(StatusArgs),
}

/// Dispatch one parsed `charter` subcommand.
///
/// Returns the process exit code; a refusal that prints its own envelope (the
/// reference's coded errors) returns `Ok(1)` after rendering.
pub fn run(command: CharterCommand) -> Result<u8, CompatError> {
    match command {
        CharterCommand::Context(args) => context_command(&args),
        CharterCommand::List(args) => list_command(&args),
        CharterCommand::Activate(args) => activate_command(&args),
        CharterCommand::Deactivate(args) => deactivate_command(&args),
        CharterCommand::Status(args) => status_command(&args),
    }
}

/// `charter context` options.
#[derive(Debug, Args)]
pub struct ContextArgs {
    /// Workflow action (specify|plan|implement|review).
    #[arg(long)]
    pub action: Option<String>,
    /// Fetch selector: `agent-profile:<id>`, `template:<mission>/<name>`,
    /// `directive:<id>`, `tactic:<id>`, `artifact:<id>` or `section:<slug>`.
    #[arg(long)]
    pub include: Option<String>,
    /// Canonical mission type for the action doctrine grain.
    #[arg(long = "mission-type")]
    pub mission_type: Option<String>,
    /// Agent profile whose cited doctrine is added.
    #[arg(long)]
    pub profile: Option<String>,
    /// Fail-open here: accepted, but never persisted by this native surface.
    #[arg(long = "mark-loaded")]
    pub mark_loaded: bool,
    /// Do not persist first-load state.
    #[arg(long = "no-mark-loaded")]
    pub no_mark_loaded: bool,
    /// Materialise the whole reachable closure in the structured payload.
    #[arg(long = "include-all")]
    pub include_all: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `charter list` options.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Also show available-but-not-activated artifacts.
    #[arg(long = "show-available")]
    pub show_available: bool,
    /// Show every available artifact across the built-in, org and project layers.
    #[arg(long)]
    pub all: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `charter activate` options.
#[derive(Debug, Args)]
pub struct ActivateArgs {
    /// Activation kind (e.g. directive, agent-profile).
    pub kind: Option<String>,
    /// Artifact ID to activate.
    pub artifact_id: Option<String>,
    /// Cascade activation scope: 'all' or a comma-separated kind list.
    #[arg(long)]
    pub cascade: Option<String>,
    /// Walk through deactivated artifacts and clear their exclusions.
    #[arg(long = "include-deactivated")]
    pub include_deactivated: bool,
    /// Replace this kind's activation list with this id plus org-required ids.
    #[arg(long)]
    pub only: bool,
    /// Apply a pack preset with replace semantics (--pack/--force narrow it).
    #[arg(long)]
    pub preset: Option<String>,
    /// Pack whose preset --preset applies (only with --preset).
    #[arg(long, default_value = "built-in")]
    pub pack: String,
    /// Apply the preset over a customised key and clear the exclusion of every
    /// deactivated id the preset lists (only with --preset).
    #[arg(long)]
    pub force: bool,
    /// Skip the post-change catalog recompile notice.
    #[arg(long = "no-compile")]
    pub no_compile: bool,
    /// Explicit --compile (the default).
    #[arg(long = "compile")]
    pub compile: bool,
    /// Eagerly refresh the derived bundle/DRG (pending; refused before writes).
    #[arg(long)]
    pub resynthesize: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `charter deactivate` options.
#[derive(Debug, Args)]
pub struct DeactivateArgs {
    /// Activation kind (e.g. directive, agent-profile).
    pub kind: Option<String>,
    /// Artifact ID to deactivate.
    pub artifact_id: Option<String>,
    /// Cascade deactivation scope: 'all' or a comma-separated kind list.
    #[arg(long)]
    pub cascade: Option<String>,
    /// Skip the post-change catalog recompile notice.
    #[arg(long = "no-compile")]
    pub no_compile: bool,
    /// Explicit --compile (the default).
    #[arg(long = "compile")]
    pub compile: bool,
    /// Eagerly refresh the derived bundle/DRG (pending; refused before writes).
    #[arg(long)]
    pub resynthesize: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// `charter status` options.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Include per-artifact provenance details.
    #[arg(long)]
    pub provenance: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// Coded refusals
// ---------------------------------------------------------------------------

/// One reference-shaped coded refusal.
struct Refusal {
    code: &'static str,
    message: String,
    payload: Map<String, Value>,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            payload: Map::new(),
        }
    }
}

/// Render a coded refusal exactly once (text line or the canonical `--json`
/// envelope) and exit 1.
fn render_refusal(refusal: &Refusal, json_output: bool) -> u8 {
    if json_output {
        let mut body = refusal.payload.clone();
        body.insert("result".to_owned(), json!("error"));
        body.insert("success".to_owned(), json!(false));
        body.insert("code".to_owned(), json!(refusal.code));
        body.insert("error".to_owned(), json!(refusal.message));
        let _ = emit_json_pretty(&Value::Object(body));
    } else {
        eprintln!("Error ({}): {}", refusal.code, refusal.message);
    }
    1
}

/// A CLI-layer failure from a domain error, with the reference's machine code
/// where the reference defines one.
fn domain_refusal(error: DoctrineError) -> Refusal {
    let code = error.reference_code().unwrap_or(ACTIVATION_FAILED);
    Refusal::new(code, error.to_string())
}

fn compat_domain(error: &CompatError) -> Refusal {
    let code = error
        .to_json()
        .get("error_code")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Refusal {
        code: match code.as_deref() {
            Some("PROJECT_ROOT_NOT_FOUND") => ACTIVATION_FAILED,
            Some(other) if other.starts_with("ACTIVATION") => ACTIVATION_FAILED,
            _ => ACTIVE_CHARTER_CONFIG_INVALID,
        },
        message: error.message().to_owned(),
        payload: Map::new(),
    }
}

// ---------------------------------------------------------------------------
// Shared resolution
// ---------------------------------------------------------------------------

/// The write root for a charter mutation.
///
/// A linked git worktree fails closed exactly like the reference
/// (`_charter_write_root.py`): the write must never land in the primary
/// checkout while the operator stands in a worktree. A non-git project keeps
/// the resolved root (the reference's own safe degrade).
fn resolve_write_root(start: &Path) -> Result<PathBuf, Refusal> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let Ok(toplevel) = sk_kernel::git_topology::git_toplevel(&start) else {
        return Ok(start);
    };
    let Ok(common_dir) = sk_kernel::git_topology::git_common_dir(&start) else {
        return Ok(start);
    };
    if common_dir.parent() != Some(toplevel.as_path()) {
        return Err(Refusal::new(
            CHARTER_WRITE_ROOT_INVALID,
            format!(
                "Refusing charter write from linked git worktree {}: use a repository-root checkout or dedicated clone for charter authoring.",
                start.display()
            ),
        ));
    }
    Ok(toplevel)
}

fn kind_token_known(token: &str) -> bool {
    token == MISSION_TYPE_TOKEN
        || ArtifactKind::from_operator_token(token)
            .map(|kind| kind.activatable())
            .unwrap_or(false)
}

fn kind_tokens() -> Vec<String> {
    sk_charter::CHARTER_KIND_TOKENS
        .iter()
        .map(|token| (*token).to_owned())
        .collect()
}

fn artifact_kind(token: &str) -> Option<ArtifactKind> {
    ArtifactKind::from_operator_token(token).ok()
}

fn yaml_key_for(token: &str) -> Result<String, Refusal> {
    if token == MISSION_TYPE_TOKEN {
        return Ok(MISSION_TYPE_KEY.to_owned());
    }
    let Some(kind) = artifact_kind(token) else {
        return Err(Refusal::new(
            ACTIVATION_KIND_UNKNOWN,
            format!(
                "Unknown kind '{token}'. Valid kinds: {}.",
                kind_tokens().join(", ")
            ),
        ));
    };
    sk_doctrine::activation_key(kind).map_err(domain_refusal)
}

fn parse_scope(cascade: Option<&str>) -> Result<Option<CascadeScope>, Refusal> {
    CascadeScope::parse(cascade)
        .map_err(|error| Refusal::new(CASCADE_SCOPE_INVALID, error.to_string()))
}

/// The pack roots and identity a cascade/availability read needs.
struct Roots {
    offering: OfferingRoots,
    identity: IdentityIndex,
    floor: BTreeMap<ArtifactKind, Vec<String>>,
}

impl Roots {
    fn load(root: &Path) -> Result<Self, CompatError> {
        let bundle = doctrine_bundle(root)?;
        let (built_in, org_roots, project) = sk_doctrine::context_roots(root)
            .map_err(|error| CompatError::domain(error.to_string()))?;
        Ok(Self {
            offering: OfferingRoots {
                built_in: Some(built_in),
                org_roots,
                project,
            },
            identity: bundle.identity,
            floor: bundle.floor,
        })
    }

    /// The available ids of `kind` in stem spelling plus the declared ids.
    fn available_ids(&self, kind: ArtifactKind) -> Vec<String> {
        let mut ids: Vec<String> = self.identity.stems(kind);
        let mut declared: Vec<String> = ids
            .iter()
            .filter_map(|stem| self.identity.declared_id(kind, stem))
            .collect();
        ids.append(&mut declared);
        ids.sort();
        ids.dedup();
        ids
    }

    fn available_mission_types(&self) -> Vec<String> {
        mission_type_fallback(&self.offering)
            .into_iter()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect()
    }
}

fn scalar_ids(set: &EffectiveSet) -> Result<Vec<String>, Refusal> {
    let ids = if set.resolved {
        set.scalar_ids()
    } else {
        set.scalar_fallback_ids()
    };
    ids.ok_or_else(|| {
        Refusal::new(
            ACTIVATION_FAILED,
            format!(
                "the effective set for {} holds ids outside Rust scalar text; exact-id activation is pending.",
                set.yaml_key
            ),
        )
    })
}

/// Resolve the effective set of `yaml_key`, or an honest refusal.
fn effective_set(root: &Path, yaml_key: &str) -> Result<EffectiveSet, Refusal> {
    let mut sets = resolve_effective_sets(root, &[yaml_key], &NativeOfferingService)
        .map_err(|error| Refusal::new(ACTIVATION_FAILED, error.to_string()))?;
    sets.pop().ok_or_else(|| {
        Refusal::new(
            ACTIVATION_FAILED,
            format!("no effective set was resolved for {yaml_key}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

fn context_command(args: &ContextArgs) -> Result<u8, CompatError> {
    let root = project_root()?;
    if let Some(include) = args.include.as_deref() {
        return context_include::render(&root, include, args.json);
    }
    let Some(action) = args.action.as_deref() else {
        return Err(CompatError::usage(
            "--action is required unless --include is provided.",
        ));
    };
    // The reference CLI executes its stages in order: the profile probe and its
    // warning, then the text build (freshness gate, state read, render and the
    // first-load mark), and only for `--json` the structured build. The facade
    // owns that order; this handler only prints the finished result.
    let execution = execute_context(
        &root,
        action,
        args.mission_type.as_deref(),
        args.profile.as_deref(),
        args.include_all,
        args.json,
        !args.no_mark_loaded,
    )?;
    if let Some(payload_exact) = execution.payload_exact.as_ref() {
        emit_json_exact(payload_exact).map_err(context_stdout_error)?;
    } else {
        if execution.mode == sk_doctrine::ContextMode::Bootstrap {
            super::python_output::write_source_text(&sk_kernel::PythonString::from_scalars(
                &format!("Action: {} (bootstrap)", execution.action),
            ))
            .map_err(context_stdout_error)?;
        }
        super::python_output::write_source_text(&execution.text).map_err(context_stdout_error)?;
    }
    Ok(0)
}

/// One ordered reference `charter context` execution (the real CLI consumer).
///
/// The first stage is the reference's `build_charter_context`: the profile
/// channel applies, `--include-all` is not an input, no record body is dumped,
/// and the first-load state write happens at that stage's own time. The second
/// stage is the reference's `build_charter_context_json`, executed only for
/// `--json`: no profile channel, the first stage's depth, `--include-all`
/// applied and inline bodies serialized. The envelope takes `context`/`text`,
/// `action`, `mode`, `first_load` and `references_count` from the first stage and
/// every structured block from the second.
pub struct ContextExecution {
    /// The exact first-stage text, before the physical stdout policy.
    pub text: sk_kernel::PythonString,
    /// The first-stage delivery mode.
    pub mode: sk_doctrine::ContextMode,
    /// The first-stage normalized action name.
    pub action: String,
    /// Whether this action had never been loaded before the first stage.
    pub first_load: bool,
    /// The first-stage effective depth (what the second stage receives).
    pub depth: usize,
    /// The first-stage reference count.
    pub references_count: usize,
    /// The second-stage payload (JSON only), as the canonical ordered exact value.
    pub payload_exact: Option<PythonJsonValue>,
    /// JSON-only checked FINITE adapter. Projection failure is an explicit
    /// Native capability error here, never a successful null business payload.
    /// The CLI always emits `payload_exact` instead.
    pub payload: Option<Result<Value, CompatError>>,
}

/// Execute the reference's ordered `charter context` stages for `root`.
///
/// Phase order matches the reference CLI exactly:
///
/// 1. the profile probe/warning (before the text builder and its freshness gate);
/// 2. the real text build, including the bundle-freshness gate, the first-load
///    state read, the render and the first-load state write at that stage's
///    Source time; a failure here prevents the structured stage;
/// 3. for `--json` only, the real structured build, which runs its own
///    freshness gate (a second real ingestion when the bundle is stale), takes
///    the first stage's depth, carries `include_all` and has no profile channel.
///
/// A completed first-stage state write is **not** rolled back when the later
/// structured stage or the output encoding fails.
pub fn execute_context(
    root: &Path,
    action: &str,
    mission_type: Option<&str>,
    profile: Option<&str>,
    include_all: bool,
    json_output: bool,
    mark_loaded: bool,
) -> Result<ContextExecution, CompatError> {
    // Source phase 0: the CLI resolves the profile before it calls the text
    // builder. A probe failure is the reference's repository load failure and
    // escapes before the freshness gate, not a healthy absence.
    let profile = match profile {
        Some(id) => {
            if profile_is_loadable(root, id)? {
                Some(id)
            } else {
                eprintln!("Warning: profile '{id}' not found; profile-cited sections omitted.");
                None
            }
        }
        None => None,
    };

    // Source phase 1: the text build.
    let stage = execute_text_stage(root, action, mission_type, profile, mark_loaded)?;

    // Source phase 2 (JSON only): the structured build, from the first stage's
    // depth. It is a second real build with its own freshness gate.
    let (payload_exact, payload) = if json_output {
        let exact = execute_structured_stage(root, action, mission_type, stage.depth, include_all)?;
        let envelope = context_envelope(&stage, &exact);
        let finite = envelope.to_serde().ok_or_else(|| {
            CompatError::domain(
                "Native finite JSON adapter cannot represent the exact context payload".to_owned(),
            )
        });
        (Some(envelope), Some(finite))
    } else {
        (None, None)
    };

    Ok(ContextExecution {
        text: stage.text,
        mode: stage.mode,
        action: stage.action,
        first_load: stage.first_load,
        depth: stage.depth,
        references_count: stage.references_count,
        payload_exact,
        payload,
    })
}

/// The first-stage result carried into the envelope and the second stage.
struct TextStage {
    text: sk_kernel::PythonString,
    mode: sk_doctrine::ContextMode,
    action: String,
    first_load: bool,
    depth: usize,
    references_count: usize,
}

/// The catalogue-miss warning channel used by the CLI's profile block.
///
/// Source raises a Python `UserWarning` on stderr; Native has no warnings module,
/// so the canonical diagnosis message is printed here as its own line (the
/// physical `warnings.warn` prefix/filename formatting is a disclosed boundary).
fn profile_warning_sink(message: &str) {
    eprintln!("{message}");
}

/// The already-resolved profile and canonical repositories the render needs.
///
/// Resolution is a Source-phase input (the profile is resolved before the text
/// builder); the block itself is rendered by the render owner at its own stage,
/// so compact's mark-before-render ordering is preserved.
struct PreparedProfileContext {
    profile: sk_doctrine::AgentProfile,
    catalog: sk_doctrine::CanonicalOfferingService,
}

/// Resolve one already-requested profile for profile-cited rendering.
///
/// Source `_load_agent_profile` resolves through the activation-aware profile
/// map and swallows every lookup failure (a missing/deactivated profile renders
/// no sections). `swallow_errors=false` keeps this helper's other callers' older
/// strictness for non-absence resolution failures.
fn prepare_profile_context(
    root: &Path,
    bundle: &DoctrineBundle,
    id: &str,
    swallow_errors: bool,
) -> Result<Option<PreparedProfileContext>, CompatError> {
    let profile = match bundle.profiles.resolve_str(id, &bundle.graph) {
        Ok(profile) => profile,
        Err(DoctrineError::ProfileNotFound { .. }) => return Ok(None),
        Err(_) if swallow_errors => return Ok(None),
        Err(error) => return Err(CompatError::domain(error.to_string())),
    };
    // The reference's text stage reads the activation-gated profile map, so a
    // deactivated profile resolves to None (no sections) even though the CLI's
    // own pre-probe only checks loadability.
    let state = ActivationState::from_ledger(&bundle.ledger, &bundle.identity, &bundle.floor)
        .map_err(|error| CompatError::domain(error.to_string()))?;
    if !state.admits_profile_exact(&profile.id) {
        return Ok(None);
    }
    let packs_override = packs_root(root);
    let (built_in, org_roots, project) =
        sk_doctrine::context_roots_with_packs_root(root, packs_override.as_deref())
            .map_err(|error| CompatError::domain(error.to_string()))?;
    let mut language_service = sk_doctrine::LanguageScopeService::new();
    let languages =
        active_project_languages(&mut language_service, root, packs_override.as_deref())
            .map_err(CompatError::domain)?;
    let context = sk_doctrine::OfferingContext::new(
        root.to_path_buf(),
        OfferingRoots {
            built_in: Some(built_in),
            org_roots,
            project,
        },
        languages,
    );
    Ok(Some(PreparedProfileContext {
        profile,
        catalog: sk_doctrine::CanonicalOfferingService::new(context),
    }))
}

/// The canonical layered offering service for one repository root.
///
/// The same roots, project layer and inferred active languages
/// [`prepare_profile_context`] uses, so the selection block and the profile block
/// read one repository topology. The reference always holds a bundle service; this
/// stage builds the same service on demand for the prepared selection block.
fn offering_catalog(root: &Path) -> Result<sk_doctrine::CanonicalOfferingService, CompatError> {
    let packs_override = packs_root(root);
    let (built_in, org_roots, project) =
        sk_doctrine::context_roots_with_packs_root(root, packs_override.as_deref())
            .map_err(|error| CompatError::domain(error.to_string()))?;
    let mut language_service = sk_doctrine::LanguageScopeService::new();
    let languages =
        active_project_languages(&mut language_service, root, packs_override.as_deref())
            .map_err(CompatError::domain)?;
    let context = sk_doctrine::OfferingContext::new(
        root.to_path_buf(),
        OfferingRoots {
            built_in: Some(built_in),
            org_roots,
            project,
        },
        languages,
    );
    Ok(sk_doctrine::CanonicalOfferingService::new(context))
}

/// Whether the effective selection names at least one of the eight kinds.
fn has_selected_artifacts(selection: &sk_doctrine::GovernanceCharterSelection) -> bool {
    [
        sk_charter::ArtifactKind::Paradigm,
        sk_charter::ArtifactKind::Directive,
        sk_charter::ArtifactKind::Tactic,
        sk_charter::ArtifactKind::Styleguide,
        sk_charter::ArtifactKind::Toolguide,
        sk_charter::ArtifactKind::Procedure,
        sk_charter::ArtifactKind::AgentProfile,
        sk_charter::ArtifactKind::MissionStepContract,
    ]
    .iter()
    .any(|kind| !selection.selected(*kind).is_empty())
}

/// The already-resolved effective selection and the repositories its block reads.
struct PreparedSelection {
    selection: sk_doctrine::GovernanceCharterSelection,
    catalog: sk_doctrine::CanonicalOfferingService,
}

/// Resolve the effective charter selection for the bootstrap text rail.
///
/// Source reads `_load_governance_charter_config` after the charter prose and the
/// reference catalogue are read and before the text render; the effective
/// selection (org required union, effective deny) is the one the renderer
/// receives. The catalogue service is built only when a selection exists, since
/// this stage has no always-present bundle service.
fn prepare_selection(
    root: &Path,
    identity: &sk_doctrine::IdentityIndex,
) -> Result<Option<PreparedSelection>, CompatError> {
    let selection = sk_doctrine::governance_charter_selection(root, Some(identity))
        .map_err(|error| CompatError::domain(error.to_string()))?;
    if !has_selected_artifacts(&selection) {
        return Ok(None);
    }
    Ok(Some(PreparedSelection {
        selection,
        catalog: offering_catalog(root)?,
    }))
}

/// The render inputs' selection-block carrier.
fn selection_block_inputs<'a>(
    prepared: Option<&'a PreparedSelection>,
    identity: &'a sk_doctrine::IdentityIndex,
    root: &'a Path,
) -> Option<sk_doctrine::SelectionBlockInputs<'a>> {
    prepared.map(|prepared| sk_doctrine::SelectionBlockInputs {
        charter: Some(&prepared.selection),
        catalog: &prepared.catalog,
        identity: Some(identity),
        repo_root: Some(root),
        scope: None,
        warning_sink: Some(&profile_warning_sink),
    })
}

/// The render inputs' prepared profile carrier for one resolved profile.
fn profile_sections_inputs<'a>(
    prepared: Option<&'a PreparedProfileContext>,
    graph: &'a sk_doctrine::DrgGraph,
) -> Option<sk_doctrine::ProfileSectionsInputs<'a>> {
    prepared.map(|prepared| sk_doctrine::ProfileSectionsInputs {
        profile: &prepared.profile,
        graph,
        catalog: &prepared.catalog,
        scope: None,
        warning_sink: Some(&profile_warning_sink),
    })
}

/// Probe one profile through the canonical profile repository, without the DRG
/// bundle, freshness gate or any context-state read.
///
/// The reference CLI's probe is `load_agent_profile(profile, repo_root) is None`,
/// which builds only the layered profile repository. This adapter reuses the
/// same canonical repository loader and root resolution `doctrine_bundle` uses,
/// so a typo is reported before the text builder runs instead of loading the
/// whole graph first.
fn profile_is_loadable(root: &Path, id: &str) -> Result<bool, CompatError> {
    let packs_override = packs_root(root);
    let (built_in, org_packs, project) =
        doctrine_roots(root, packs_override.as_deref()).map_err(CompatError::domain)?;
    let org_roots: Vec<PathBuf> = org_packs.iter().map(|pack| pack.root.clone()).collect();
    let mut language_service = sk_doctrine::LanguageScopeService::new();
    let languages =
        active_project_languages(&mut language_service, root, packs_override.as_deref())
            .map_err(CompatError::domain)?;
    let profiles = sk_doctrine::ProfileRepository::load_with_languages(
        &built_in,
        &org_roots,
        project.as_deref(),
        languages.as_deref(),
    )
    .map_err(|error| CompatError::domain(error.to_string()))?;
    Ok(profiles.get_str(id).is_some())
}

/// Execute the reference's text stage: freshness, state, rail, read, render, mark.
///
/// The reference `build_charter_context` selects one of four rails in one order
/// (undeclared non-bootstrap → charter presence → depth tier → bootstrap), and
/// each rail has its own prose-read, mark-timing and reference-count policy.
/// This function is that policy for the live CLI:
///
/// * **bootstrap** — reads `<canonical_root>/charter.md` (errors escape) and the
///   `catalog.references` catalogue at the same root, renders, *then* marks; its
///   `references_count` is the catalogue length (never the delivered link set
///   and never the resolvable-pointer count);
/// * **compact bundle** (below the minimum depth) — reads `<repo_root>/charter.md`
///   with the reference's compact catch (`OSError` degrades to no prose, a
///   decode failure escapes), marks *then* renders, count `0`;
/// * **undeclared non-bootstrap** — no charter-presence gate at all, no state
///   mark, count `0`; the reported depth is the caller's own optional request
///   (`None` ⇒ `1`), which is what the reference's builder receives;
/// * **missing** — the fixed missing-charter text, the state's own
///   first-load/depth, no prose read, no catalogue read, no mark, count `0`.
fn execute_text_stage(
    root: &Path,
    action: &str,
    mission_type: Option<&str>,
    profile: Option<&str>,
    mark_loaded: bool,
) -> Result<TextStage, CompatError> {
    // Source phase: `build_charter_context` runs the bundle-freshness gate
    // before its charter-presence gate, before context-state preparation and
    // before any charter prose read.
    let freshness =
        sk_doctrine::ensure_charter_bundle_fresh(root, None, &sk_doctrine::source_cwd_journal())
            .map_err(|error| CompatError::domain(error.to_string()))?;
    let canonical_root = freshness
        .as_ref()
        .and_then(|result| result.canonical_root.as_deref())
        .unwrap_or(root);
    let mut state = context_state(root, action)?;
    let (first_load, depth) = (state.first_load(), state.depth());
    let bundle = doctrine_bundle(root)?;
    let inputs = sk_doctrine::ContextInputs {
        graph: &bundle.graph,
        ledger: &bundle.ledger,
        identity: &bundle.identity,
        profiles: &bundle.profiles,
        project_root: root,
        required_floor: &bundle.floor,
        project_warnings: &[],
    };
    let mut request = sk_doctrine::ContextRequest::new(action);
    request.mission_type = mission_type;
    // The bundle resolves against the *effective* depth (what the state
    // prepared), exactly like `_resolve_action_bundle`.
    request.depth = depth;
    request.profile = profile;
    // `--include-all` is deliberately not assigned: the reference text builder
    // has no such parameter. `build_text_stage` also drops it, so a caller
    // cannot accidentally make the text stage inline a body.
    let staged =
        build_text_stage_with_presence(&inputs, &request, || charter_present(canonical_root))
            .map_err(|error| CompatError::coded(ACTIVATION_FAILED, error.to_string()))?;
    let context = staged.context;
    let rail = staged.rail;

    // Per-rail reads. Only the bootstrap rail reads the catalogue; only the two
    // compact rails read the repository-root prose through the compact catch;
    // the missing rail reads neither.
    let (markdown, reference_docs, references_count) = match rail {
        TextRail::Bootstrap => {
            let markdown = read_charter_markdown(canonical_root)
                .map_err(|error| CompatError::domain(error.to_string()))?;
            let catalog = load_reference_catalog(canonical_root, &bundle.identity)
                .map_err(|error| CompatError::coded(ACTIVATION_FAILED, error.to_string()))?;
            // Source captures the filesystem codec at interpreter startup; the
            // safety here is bound per stage call, and the SAME captured context
            // decodes the pointer paths.
            let os_encoding = sk_kernel::python_os_encoding::OsEncodingContext::from_environment();
            let docs = reference_docs(&catalog, &context.action, &os_encoding)?;
            (markdown, docs, catalog.len())
        }
        TextRail::CompactBundle | TextRail::NonBootstrap => {
            let markdown = read_compact_markdown(root).map_err(|error| {
                CompatError::domain(format!(
                    "could not read the charter markdown for the compact rail: {error}"
                ))
            })?;
            (markdown, Vec::new(), 0)
        }
        TextRail::Missing => (None, Vec::new(), 0),
    };

    let declared_authority_paths = declared_authority_paths(root);
    let languages = project_languages(root);
    let lacks_specialist_guidance =
        sk_doctrine::lacks_specialist_guidance(languages.as_deref(), Some(root), None)
            .unwrap_or(false);
    // Source phase 0 already resolved the profile; a missing/deactivated profile
    // simply renders no profile block. The block itself is prepared by the
    // render owner at each rail's own mark/render position.
    let prepared_profile = match profile {
        Some(id) if rail != TextRail::Missing => prepare_profile_context(root, &bundle, id, true)?,
        _ => None,
    };
    // Source reads the effective charter selection on the bootstrap text rail
    // only (the compact rails render no global-selection block).
    let prepared_selection = match rail {
        TextRail::Bootstrap => prepare_selection(root, &bundle.identity)?,
        _ => None,
    };
    let normalized_action = context.action.clone();
    let mode = context.mode;
    let render = |compact_governance: Option<&sk_doctrine::context_render::CompactGovernance>| {
        let render_inputs = sk_doctrine::context_render::RenderInputs {
            charter_markdown: markdown.as_deref(),
            project_root: Some(root),
            declared_authority_paths: &declared_authority_paths,
            reference_docs: &reference_docs,
            compact_governance,
            languages: languages.as_deref(),
            lacks_specialist_guidance,
            profile_sections: profile_sections_inputs(prepared_profile.as_ref(), &bundle.graph),
            selection_block: selection_block_inputs(
                prepared_selection.as_ref(),
                &bundle.identity,
                root,
            ),
        };
        sk_doctrine::context_render::render_with_exact(&context, &render_inputs)
            .map_err(|error| CompatError::domain(error.to_string()))
    };
    // The compact rails resolve the reference's `_resolve_governance_summary`
    // at their own render position; the direct resolver's errors are projected
    // by that caller-specific catch (a retired pack field escapes).
    let compact_summary =
        || -> Result<sk_doctrine::context_render::CompactGovernance, CompatError> {
            sk_doctrine::compact_governance_summary(root)
                .map_err(|error| CompatError::domain(error.to_string()))
        };
    // Source mark timing per rail: the bootstrap builder renders first and marks
    // after; the compact-bundle builder marks first and renders after; the
    // undeclared and missing rails never mark. A later structured-stage failure
    // does not roll a completed write back.
    let mark_first = mark_loaded && first_load && rail.marks_first_load();
    let text = match rail {
        TextRail::Bootstrap => {
            let text = render(None)?;
            if mark_first {
                persist_first_load(&mut state, &normalized_action)?;
            }
            text
        }
        TextRail::CompactBundle => {
            if mark_first {
                persist_first_load(&mut state, &normalized_action)?;
            }
            let summary = compact_summary()?;
            render(Some(&summary))?
        }
        TextRail::NonBootstrap => {
            let summary = compact_summary()?;
            render(Some(&summary))?
        }
        TextRail::Missing => render(None)?,
    };
    // The undeclared rail reports its own original optional request (`None` ⇒ 1),
    // never the assumed first-load depth, and it always reports
    // `first_load=false` (the reference's `_non_bootstrap_context_result` hard-codes
    // it, since that rail neither reads nor writes first-load state). Every other
    // rail reports the state's facts.
    let stage_depth = match rail {
        TextRail::NonBootstrap => 1,
        _ => depth,
    };
    let stage_first_load = match rail {
        TextRail::NonBootstrap => false,
        _ => first_load,
    };
    Ok(TextStage {
        text,
        mode,
        action: normalized_action,
        first_load: stage_first_load,
        depth: stage_depth,
        references_count,
    })
}

/// Execute the reference's structured stage: its own freshness gate and build.
fn execute_structured_stage(
    root: &Path,
    action: &str,
    mission_type: Option<&str>,
    depth: usize,
    include_all: bool,
) -> Result<PythonJsonValue, CompatError> {
    // The reference JSON builder runs its own bundle reads, so a stale bundle is
    // ingested a second time here. This is a real second build, not a replayed
    // or cloned DTO.
    sk_doctrine::ensure_charter_bundle_fresh(root, None, &sk_doctrine::source_cwd_journal())
        .map_err(|error| CompatError::domain(error.to_string()))?;
    let bundle = doctrine_bundle(root)?;
    let inputs = sk_doctrine::ContextInputs {
        graph: &bundle.graph,
        ledger: &bundle.ledger,
        identity: &bundle.identity,
        profiles: &bundle.profiles,
        project_root: root,
        required_floor: &bundle.floor,
        project_warnings: &[],
    };
    let mut request = sk_doctrine::ContextRequest::new(action);
    request.mission_type = mission_type;
    // Source prepares state again even with an explicit depth. This read can
    // escape after a completed text-stage mark/observer; carrying depth alone
    // must not skip that execution phase. The second snapshot is never marked.
    let structured_state = sk_doctrine::prepare_context_state(
        root,
        &sk_doctrine::normalize_context_action(action),
        Some(depth),
    )
    .map_err(|error| CompatError::domain(error.to_string()))?;
    request.depth = structured_state.effective_depth();
    // The reference JSON builder takes no profile argument: the text stage's
    // profile channel must not reach the structured arrays.
    request.profile = None;
    request.include_all = include_all;
    let structured = sk_doctrine::build_action_context_structured(&inputs, &request)
        .map_err(|error| CompatError::coded(ACTIVATION_FAILED, error.to_string()))?;
    Ok(structured.to_json_exact())
}

/// Compose the reference CLI's `--json` envelope from the two stages.
fn context_envelope(stage: &TextStage, structured: &PythonJsonValue) -> PythonJsonValue {
    let mut payload: Vec<(&str, PythonJsonValue)> = vec![
        ("result", PythonJsonValue::scalar("success")),
        ("success", PythonJsonValue::Bool(true)),
        (
            "context_schema_version",
            structured
                .field("context_schema_version")
                .cloned()
                .unwrap_or_else(|| PythonJsonValue::scalar(sk_doctrine::CONTEXT_SCHEMA_VERSION)),
        ),
        ("action", PythonJsonValue::scalar(&stage.action)),
        ("mode", exact_mode(stage.mode)),
        ("first_load", PythonJsonValue::Bool(stage.first_load)),
        (
            "references_count",
            PythonJsonValue::Integer(stage.references_count.to_string()),
        ),
        ("context", PythonJsonValue::String(stage.text.clone())),
        ("text", PythonJsonValue::String(stage.text.clone())),
    ];
    for key in [
        "directives",
        "all_directives",
        "diagnostics",
        "directives_source",
        "tactics",
        "styleguides",
        "toolguides",
        "references",
        "governance_references",
        "project_charter",
        "org_charter",
    ] {
        let value = structured
            .field(key)
            .cloned()
            .unwrap_or(PythonJsonValue::Null);
        payload.push((key, value));
    }
    PythonJsonValue::Object(
        payload
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// Persist the first-load stamp for `action` from the PREPARED snapshot.
///
/// Source `_mark_action_loaded` mutates the dict `_prepare_context_state`
/// decoded and writes that dict: there is no second disk read, no
/// compare-and-swap and no foreign merge at mark time. The ticket carries that
/// snapshot here, so a change made to the file between prepare and mark is
/// overwritten exactly like the reference.
fn persist_first_load(ticket: &mut ContextStateTicket, action: &str) -> Result<(), CompatError> {
    ticket.mark_first_load(action).map_err(|error| {
        CompatError::domain(format!("could not persist first-load state: {error}"))
    })
}

/// The rendered context plus its versioned JSON payload (no printing, no state).
pub struct ContextOutcome {
    /// The `charter context --json` payload, as the canonical ordered exact value.
    ///
    /// This is what `--json` writes (through the one Kernel encoder): every
    /// delivered `body` is the merged record's exact model dump, so a lone
    /// surrogate, an arbitrary-width integer or a non-finite float reaches the
    /// operator instead of being reduced to a scalar projection.
    pub payload_exact: PythonJsonValue,
    /// The same payload through the FINITE `serde_json` adapter.
    ///
    /// In-process callers that can only hold scalars (and the compatibility
    /// tests) read this; it is `null` when the exact payload has no scalar form.
    /// The command never prints it.
    pub payload: Value,
    /// Exact rendered text, before the physical stdout encoding policy.
    pub text: sk_kernel::PythonString,
    /// The delivery mode.
    pub mode: sk_doctrine::ContextMode,
    /// The normalized action name.
    pub action: String,
    /// Whether this action had never been loaded in this project.
    pub first_load: bool,
}

/// Build the action-scoped context for `root` in ONE combined build.
///
/// This is the explicit **single-build helper**: it applies `profile` and
/// `include_all` to the same build and serializes inline bodies. It is not the
/// reference CLI's two-stage contract; [`execute_context`] is the real consumer
/// of the ordered text-first/structured-second execution.
///
/// The reference's per-project first-load depth applies (2 for a first load,
/// 1 afterwards), and an unresolvable profile is omitted with a warning
/// instead of failing the render.
pub fn context_payload(
    root: &Path,
    action: &str,
    mission_type: Option<&str>,
    profile: Option<&str>,
    include_all: bool,
) -> Result<ContextOutcome, CompatError> {
    // Source phase: `charter.activation.context.build_context` runs the bundle
    // freshness gate before its charter-presence gate, before context-state
    // preparation and before any charter prose read. The gate resolves the
    // canonical (main-checkout) read root, ingests encoding provenance only when
    // the bundle is incomplete or stale, and retires the derived-file write. The
    // journal is anchored on the process CWD because the reference's global
    // provenance path is CWD-relative, not read-root-relative.
    let freshness =
        sk_doctrine::ensure_charter_bundle_fresh(root, None, &sk_doctrine::source_cwd_journal())
            .map_err(|error| CompatError::domain(error.to_string()))?;
    let canonical_root = freshness
        .as_ref()
        .and_then(|result| result.canonical_root.as_deref())
        .unwrap_or(root);
    let state = context_state(root, action)?;
    let (first_load, depth) = (state.first_load(), state.depth());
    let bundle = doctrine_bundle(root)?;
    let mut profile = profile;
    let mut prepared_profile = None;
    if let Some(id) = profile {
        match prepare_profile_context(root, &bundle, id, false)? {
            Some(prepared) => prepared_profile = Some(prepared),
            None => {
                eprintln!("Warning: profile '{id}' not found; profile-cited sections omitted.");
                profile = None;
            }
        }
    }
    let inputs = sk_doctrine::ContextInputs {
        graph: &bundle.graph,
        ledger: &bundle.ledger,
        identity: &bundle.identity,
        profiles: &bundle.profiles,
        project_root: root,
        required_floor: &bundle.floor,
        project_warnings: &[],
    };
    let mut request = sk_doctrine::ContextRequest::new(action);
    request.mission_type = mission_type;
    request.depth = depth;
    request.profile = profile;
    request.include_all = include_all;
    let context = sk_doctrine::build_action_context(&inputs, &request)
        .map_err(|error| CompatError::coded(ACTIVATION_FAILED, error.to_string()))?;

    let markdown = read_charter_markdown(canonical_root)
        .map_err(|error| CompatError::domain(error.to_string()))?;
    // ONE count authority: `references_count` is the `catalog.references`
    // catalogue length on the bootstrap rail and zero on every other rail —
    // never the delivered link set.
    let catalog = load_reference_catalog(canonical_root, &bundle.identity)
        .map_err(|error| CompatError::coded(ACTIVATION_FAILED, error.to_string()))?;
    let references_count = if context.mode == sk_doctrine::ContextMode::Bootstrap {
        catalog.len()
    } else {
        0
    };
    let reference_docs = reference_docs(
        &catalog,
        &context.action,
        &sk_kernel::python_os_encoding::OsEncodingContext::from_environment(),
    )?;
    let declared_authority_paths = declared_authority_paths(root);
    let languages = project_languages(root);
    // The structured path renders the bootstrap text too, so it prepares the same
    // effective selection there (and none on the compact rails).
    let prepared_selection = if context.mode == sk_doctrine::ContextMode::Bootstrap {
        prepare_selection(root, &bundle.identity)?
    } else {
        None
    };
    let render_inputs = sk_doctrine::context_render::RenderInputs {
        charter_markdown: markdown.as_deref(),
        project_root: Some(root),
        declared_authority_paths: &declared_authority_paths,
        reference_docs: &reference_docs,
        compact_governance: None,
        languages: languages.as_deref(),
        lacks_specialist_guidance: sk_doctrine::lacks_specialist_guidance(
            languages.as_deref(),
            Some(root),
            None,
        )
        .unwrap_or(false),
        profile_sections: profile_sections_inputs(prepared_profile.as_ref(), &bundle.graph),
        selection_block: selection_block_inputs(
            prepared_selection.as_ref(),
            &bundle.identity,
            root,
        ),
    };
    let text = sk_doctrine::context_render::render_with_exact(&context, &render_inputs)
        .map_err(|error| CompatError::domain(error.to_string()))?;
    // ONE payload: the doctrine layer's exact ordered value graph. The envelope is
    // the reference CLI's key order (`context.py` builds it in this sequence), and
    // every block is taken from that one value — never re-serialized through
    // `serde_json`, which would sort every mapping and drop exact data.
    let structured = context.to_json_exact();
    let mut payload: Vec<(&str, PythonJsonValue)> = vec![
        ("result", PythonJsonValue::scalar("success")),
        ("success", PythonJsonValue::Bool(true)),
        (
            "context_schema_version",
            PythonJsonValue::scalar(&context.context_schema_version),
        ),
        ("action", PythonJsonValue::scalar(&context.action)),
        ("mode", exact_mode(context.mode)),
        ("first_load", PythonJsonValue::Bool(first_load)),
        (
            "references_count",
            PythonJsonValue::Integer(references_count.to_string()),
        ),
        ("context", PythonJsonValue::String(text.clone())),
        ("text", PythonJsonValue::String(text.clone())),
    ];
    for key in [
        "directives",
        "all_directives",
        "diagnostics",
        "directives_source",
        "tactics",
        "styleguides",
        "toolguides",
        "references",
        "governance_references",
        "project_charter",
        "org_charter",
    ] {
        let value = structured
            .field(key)
            .cloned()
            .unwrap_or(PythonJsonValue::Null);
        payload.push((key, value));
    }
    let payload_exact = PythonJsonValue::Object(
        payload
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    );
    let payload = payload_exact.to_serde().unwrap_or(Value::Null);
    Ok(ContextOutcome {
        payload_exact,
        payload,
        text,
        mode: context.mode,
        action: context.action,
        first_load,
    })
}

/// The `mode` contract spelling, from the doctrine type's own `serde` authority.
fn exact_mode(mode: sk_doctrine::ContextMode) -> PythonJsonValue {
    match mode {
        sk_doctrine::ContextMode::Bootstrap => PythonJsonValue::scalar("bootstrap"),
        sk_doctrine::ContextMode::Compact => PythonJsonValue::scalar("compact"),
        sk_doctrine::ContextMode::Missing => PythonJsonValue::scalar("missing"),
    }
}

/// One ordered Kernel JSON rendering, then the canonical physical stdout policy.
fn emit_json_exact(value: &PythonJsonValue) -> Result<(), super::python_output::StdoutError> {
    let text = value.render_python_with(PythonJsonRenderOptions::new(Some(2)));
    super::python_output::write_source_json(&text)
}

fn context_stdout_error(error: super::python_output::StdoutError) -> CompatError {
    let mut message = error.to_string();
    if error.is_encode_failure() {
        message = format!(
            "Unexpected error: {message}\n{}",
            super::python_output::PYTHONIOENCODING_HINT
        );
    }
    let exit = error.exit_code();
    let envelope = json!({"result": "error", "success": false, "error": message});
    CompatError::envelope_with_exit(exit, envelope, message)
}

/// The first stage's context-state ticket.
///
/// At Source, `_prepare_context_state` decodes the bookkeeping document ONCE and
/// returns it alongside the resolved first-load/depth facts; the first stage then
/// marks and writes *that* snapshot. This ticket is the same thing at the CLI
/// seam: the canonical [`sk_doctrine::ContextStateBundle`], carried from the
/// prepare site to the mark site so the write persists the prepared snapshot.
struct ContextStateTicket {
    bundle: sk_doctrine::ContextStateBundle,
}

impl ContextStateTicket {
    /// Whether this action had never been loaded when the snapshot was read.
    fn first_load(&self) -> bool {
        self.bundle.first_load()
    }

    /// The effective depth the reference resolved (the second stage's input).
    fn depth(&self) -> usize {
        self.bundle.effective_depth()
    }

    /// Source `_mark_action_loaded`: stamp the prepared snapshot and write it.
    fn mark_first_load(&mut self, action: &str) -> Result<(), String> {
        self.bundle
            .mark_action_loaded(action)
            .map_err(|error| error.to_string())
    }
}

/// The first-load state and effective depth for `action`, prepared ONCE.
///
/// The action key is the Source-normalized spelling
/// ([`sk_doctrine::normalize_context_action`]), used for both the membership
/// test and the later mark — the reference reads and writes the same key.
fn context_state(root: &Path, action: &str) -> Result<ContextStateTicket, CompatError> {
    let action = sk_doctrine::normalize_context_action(action);
    sk_doctrine::prepare_context_state(root, &action, None)
        .map(|bundle| ContextStateTicket { bundle })
        .map_err(|error| CompatError::domain(error.to_string()))
}

/// Read the project's authored `charter.md` for the ordinary (non-provenance)
/// render.
///
/// The reference's bootstrap render is `charter_path.read_text(encoding="utf-8")`
/// guarded by `charter_path.exists()`: the read is plain strict UTF-8, so an
/// authored BOM stays authored text (it is not stripped for display and never
/// routed through the encoding-provenance chokepoint), while an invalid-UTF-8
/// body or a directory at that path raises instead of silently rendering without
/// prose. `Ok(None)` is only the healthy absence (`exists()` was false); every
/// other failure is preserved for the caller.
fn read_charter_markdown(root: &Path) -> std::io::Result<Option<String>> {
    let path = root.join(sk_charter::CHARTER_MD);
    if !sk_kernel::paths::source_path_exists(&path)? {
        return Ok(None);
    }
    fs::read_to_string(path).map(Some)
}

/// Read the repository-root `charter.md` prose for a **compact** rail.
///
/// The reference's compact section block is display-only, so an absent file and
/// every `OSError` degrade to "no prose" (`except OSError: return ""`); a
/// decode failure is a `UnicodeDecodeError` (a `ValueError`) and therefore
/// **escapes**, exactly as `read_text(encoding="utf-8")` does. Splitting the
/// byte read from the decode is what keeps those two phases distinct.
fn read_compact_markdown(root: &Path) -> Result<Option<String>, String> {
    let path = root.join(sk_charter::CHARTER_MD);
    // `Path.exists()` absorbs every stat error as "absent".
    if !sk_kernel::paths::source_path_exists(&path).unwrap_or(false) {
        return Ok(None);
    }
    let Ok(bytes) = fs::read(&path) else {
        return Ok(None);
    };
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(error) => Err(format!("charter markdown is not valid UTF-8: {error}")),
    }
}

fn project_languages(root: &Path) -> Option<Vec<String>> {
    let mut service = sk_doctrine::LanguageScopeService::new();
    let request = sk_doctrine::InferRequest::new(Some(root));
    service
        .infer(&request)
        .ok()
        .and_then(sk_doctrine::InferredLanguages::into_option)
}

/// The charter-declared authority paths (read-only, never rewritten).
fn declared_authority_paths(root: &Path) -> Vec<String> {
    let document = match YamlDocument::open(&root.join(CHARTER_YAML)) {
        Ok(document) => document,
        Err(_) => return Vec::new(),
    };
    let Some(governance) = document
        .mapping()
        .and_then(|mapping| mapping.get("governance"))
    else {
        return Vec::new();
    };
    let Some(paths) = governance
        .as_mapping()
        .and_then(|mapping| mapping.get("authority_paths"))
    else {
        return Vec::new();
    };
    paths
        .as_sequence()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The resolved `catalog.references` pointers that open inside the built-in pack.
///
/// The catalogue itself is read by the ONE canonical reader
/// ([`sk_doctrine::load_reference_catalog`]); this function only routes the
/// selector with the **normalized** action, the canonical built-in offering
/// root and the captured filesystem codec. The Source pipeline
/// (action filter → first-seen-kind rotation/round-robin → offering index/cache/
/// lookup → cap) lives in [`sk_doctrine::select_reference_pointers`], and the
/// returned model carries prepared exact `str(Path)` text for the ONE renderer.
/// Nothing here re-counts: `references_count` stays the catalogue length.
fn reference_docs(
    catalog: &[sk_doctrine::ReferenceEntry],
    action: &str,
    codec: &sk_kernel::python_os_encoding::OsEncodingContext,
) -> Result<Vec<sk_doctrine::context_render::ReferenceDoc>, CompatError> {
    let offering_root = sk_kernel::get_built_in_pack_root()
        .map_err(|error| CompatError::domain(error.to_string()))?;
    let action = sk_kernel::PythonString::from_scalars(action);
    sk_doctrine::select_reference_pointers(catalog, &action, &offering_root, codec)
        .map_err(|error| CompatError::coded(ACTIVATION_FAILED, error.to_string()))
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

fn list_command(args: &ListArgs) -> Result<u8, CompatError> {
    let root = project_root()?;
    let listing = list_availability::Listing::load(&root, args.show_available, args.all)?;
    if args.json {
        let _ = emit_json_pretty(&listing.payload);
        return Ok(0);
    }
    println!("Charter Activation State");
    for row in &listing.rows {
        // The Activated cell may carry the reference's required-floor markers on
        // their own lines; the plain renderer keeps them, indented under the row.
        println!("{}: {}", row.kind, row.activated_text.replace('\n', "\n  "));
        if row.available.is_some() {
            println!(
                "  available: {}",
                if row.available_text.is_empty() {
                    "(none)".to_owned()
                } else {
                    row.available_text.clone()
                }
            );
        }
        if let Some(ids) = &row.deactivated
            && !ids.is_empty()
        {
            println!("  deactivated: {}", ids.join(", "));
        }
    }
    if args.all {
        // FR-025: the template kind is mission-scoped and has no activation list, so it
        // only appears in the layer-aware (`--all`) availability view. The row is
        // composed from the same discovered rows the JSON payload publishes, so the
        // text view and `--json` cannot drift or re-scan the tiers.
        println!(
            "{}: {}",
            list_templates::TEMPLATE_KIND,
            list_templates::TEMPLATE_ACTIVATED_CELL
        );
        println!("  available: {}", listing.template_text());
    }
    Ok(0)
}

/// The `charter list --json` payload (the same rows the text view renders).
pub fn list_payload(
    root: &Path,
    show_available: bool,
    all_layers: bool,
) -> Result<Value, CompatError> {
    Ok(list_availability::Listing::load(root, show_available, all_layers)?.payload)
}

// ---------------------------------------------------------------------------
// Activation / deactivation
// ---------------------------------------------------------------------------

/// One direct write's outcome.
struct DirectOutcome {
    activated: Vec<String>,
    deactivated: Vec<String>,
    listed: Option<Vec<String>>,
    warnings: Vec<String>,
}

fn activate_command(args: &ActivateArgs) -> Result<u8, CompatError> {
    let positional = args.kind.is_some() || args.artifact_id.is_some();
    if args.preset.is_some() {
        if positional {
            return Err(CompatError::usage(
                "--preset cannot be combined with a positional KIND ARTIFACT_ID.",
            ));
        }
        if args.cascade.is_some() {
            return Err(CompatError::usage(
                "--cascade cannot be combined with --preset (presets do not cascade).",
            ));
        }
        if args.only {
            return Err(CompatError::usage(
                "--only cannot be combined with --preset (a preset already replaces every key it governs).",
            ));
        }
        if args.resynthesize {
            return Err(CompatError::unsupported(
                ACTIVATION_FAILED,
                "native `--resynthesize` is pending: the canonical charter synthesizer is not ported yet. Nothing was written.",
            ));
        }
        let root = project_root()?;
        let root = match resolve_write_root(&root) {
            Ok(root) => root,
            Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
        };
        let preset = args.preset.clone().unwrap_or_default();
        return preset::activate_preset_command(
            &root,
            &args.pack,
            &preset,
            args.force,
            args.json,
            args.no_compile,
        );
    }
    if args.force {
        return Err(CompatError::usage("--force only applies with --preset."));
    }
    if args.pack != "built-in" {
        return Err(CompatError::usage("--pack only applies with --preset."));
    }
    if args.include_deactivated && args.cascade.is_none() {
        return Err(CompatError::usage(
            "--include-deactivated only applies with --cascade.",
        ));
    }
    if args.resynthesize {
        return Err(CompatError::unsupported(
            ACTIVATION_FAILED,
            "native `--resynthesize` is pending: the canonical charter synthesizer is not ported yet. Nothing was written.",
        ));
    }
    let (Some(kind), Some(artifact_id)) = (args.kind.as_deref(), args.artifact_id.as_deref())
    else {
        println!(
            "Usage: spec-kitty charter activate KIND ARTIFACT_ID [--cascade SCOPE] [--only] [--json]"
        );
        return Ok(0);
    };
    if !kind_token_known(kind) {
        return Ok(render_refusal(
            &Refusal::new(
                ACTIVATION_KIND_UNKNOWN,
                format!(
                    "Unknown kind '{kind}'. Valid kinds: {}.",
                    kind_tokens().join(", ")
                ),
            ),
            args.json,
        ));
    }
    let scope = match parse_scope(args.cascade.as_deref()) {
        Ok(scope) => scope,
        Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
    };
    let root = project_root()?;
    let root = match resolve_write_root(&root) {
        Ok(root) => root,
        Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
    };

    let outcome = match activate_direct(
        &root,
        kind,
        artifact_id,
        args.only,
        DenyListChoice::Activate,
    ) {
        Ok(outcome) => outcome,
        Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
    };

    let mut report = ChangeReport::new("activate", kind, artifact_id, &outcome, args.only);
    if args.only {
        report.now_listed = Some(outcome.listed.clone().unwrap_or_default());
    }
    if scope.is_none() {
        report.referenced = no_cascade_report(&root, kind, artifact_id);
    } else if let Some(scope) = &scope {
        report.cascade =
            cascade_activation_report(&root, kind, artifact_id, scope, args.include_deactivated);
    }

    let notices = post_change_notices(&root, kind, args.no_compile);
    report.notices = notices.clone();
    if args.json {
        let _ = emit_json_pretty(&report.to_json());
    } else {
        print_change_report(&report);
        for notice in &notices {
            println!("{notice}");
        }
    }
    Ok(0)
}

fn deactivate_command(args: &DeactivateArgs) -> Result<u8, CompatError> {
    if args.resynthesize {
        return Err(CompatError::unsupported(
            ACTIVATION_FAILED,
            "native `--resynthesize` is pending: the canonical charter synthesizer is not ported yet. Nothing was written.",
        ));
    }
    let (Some(kind), Some(artifact_id)) = (args.kind.as_deref(), args.artifact_id.as_deref())
    else {
        println!(
            "Usage: spec-kitty charter deactivate KIND ARTIFACT_ID [--cascade SCOPE] [--json]"
        );
        return Ok(0);
    };
    if !kind_token_known(kind) {
        return Ok(render_refusal(
            &Refusal::new(
                ACTIVATION_KIND_UNKNOWN,
                format!(
                    "Unknown kind '{kind}'. Valid kinds: {}.",
                    kind_tokens().join(", ")
                ),
            ),
            args.json,
        ));
    }
    let scope = match parse_scope(args.cascade.as_deref()) {
        Ok(scope) => scope,
        Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
    };
    let root = project_root()?;
    let root = match resolve_write_root(&root) {
        Ok(root) => root,
        Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
    };
    let outcome = match deactivate_direct(&root, kind, artifact_id) {
        Ok(outcome) => outcome,
        Err(refusal) => return Ok(render_refusal(&refusal, args.json)),
    };
    let mut report = ChangeReport::new("deactivate", kind, artifact_id, &outcome, false);
    if let Some(scope) = &scope {
        report.cascade = cascade_deactivation_report(&root, kind, artifact_id, scope);
    }
    let notices = post_change_notices(&root, kind, args.no_compile);
    report.notices = notices.clone();
    if args.json {
        let _ = emit_json_pretty(&report.to_json());
    } else {
        print_change_report(&report);
        for notice in &notices {
            println!("{notice}");
        }
    }
    Ok(0)
}

/// The canonical stored spelling of `artifact_id` for `kind`.
fn stored_id(kind: &str, artifact_id: &str, roots: &Roots) -> Result<String, Refusal> {
    match artifact_kind(kind) {
        Some(kind) => roots
            .identity
            .stored_spelling(kind, artifact_id)
            .map_err(domain_refusal),
        None => Ok(artifact_id.to_owned()),
    }
}

fn activate_direct(
    root: &Path,
    kind: &str,
    artifact_id: &str,
    only: bool,
    deny_change: DenyListChoice,
) -> Result<DirectOutcome, Refusal> {
    let roots = Roots::load(root).map_err(|error| compat_domain(&error))?;
    let yaml_key = yaml_key_for(kind)?;
    let stored = stored_id(kind, artifact_id, &roots)?;
    let artifact_kind = artifact_kind(kind);
    let (available_ids, deny_key, current_deny, effective_deny_set) = match artifact_kind {
        Some(kind) => {
            let mut available = roots.available_ids(kind);
            if !available.contains(&stored) {
                available.push(stored.clone());
            }
            let current_deny = ledger_deny(root, kind)?;
            let deny_set = effective_deny(
                kind,
                current_deny.iter().cloned(),
                roots.floor.get(&kind).cloned().unwrap_or_default(),
                &roots.identity,
            );
            (
                available,
                sk_doctrine::kind_deny_key(kind),
                current_deny,
                deny_set,
            )
        }
        None => (
            roots.available_mission_types(),
            None,
            Vec::new(),
            sk_doctrine::DenySet::empty(),
        ),
    };
    let document = open_write_target(root)?;
    let present = document.get_non_null(&yaml_key).is_some();
    // Source `_preservation_ids`: a present list is appended to and needs no
    // absent-key offering resolution (or its logger/constructor side effects).
    let (effective_ids, in_force) = if present {
        (Vec::new(), true)
    } else {
        let set = effective_set(root, &yaml_key)?;
        (scalar_ids(&set)?, set.resolved)
    };
    let narrow_base: Option<Vec<String>> = if only {
        if let Some(kind) = artifact_kind {
            let mut warnings = Vec::new();
            let stems = sk_doctrine::org_required_stems(root, kind, &roots.identity, &mut warnings)
                .map_err(domain_refusal)?
                .into_iter()
                .filter_map(|id| id.to_scalar_string().ok())
                .collect();
            Some(stems)
        } else {
            Some(Vec::new())
        }
    } else {
        None
    };
    let when_absent = match artifact_kind {
        Some(kind) => EffectiveWhenAbsent::for_kind(kind),
        None => EffectiveWhenAbsent::All,
    };
    let previous_ids = if only && present {
        let current = document
            .get_string_list(&yaml_key)
            .map_err(domain_refusal)?
            .unwrap_or_default();
        let identity = sk_doctrine::IdentityIndex::from_roots(
            &roots.identity.built_in_root,
            &roots.identity.org_roots,
            roots.identity.project_root.as_deref(),
        );
        let mut seen = BTreeSet::new();
        current
            .into_iter()
            .map(|id| {
                artifact_kind
                    .and_then(|kind| identity.stem(kind, &id))
                    .unwrap_or(id)
            })
            .filter(|id| seen.insert(id.clone()))
            .collect()
    } else {
        effective_ids.clone()
    };
    let inputs = ActivationInputs {
        yaml_key: &yaml_key,
        available_ids: &available_ids,
        config_data: &document,
        effective_ids: &effective_ids,
        effective_when_absent: when_absent,
        effective_ids_in_force: in_force,
        narrow_base: narrow_base.as_deref(),
        previous_ids: &previous_ids,
        deny_key: deny_key.as_deref(),
        current_deny: &current_deny,
        deny_change,
        effective_deny: &effective_deny_set,
    };
    let plan = plan_activation(kind, &stored, &inputs).map_err(domain_refusal)?;
    let transaction = ActivationTransaction::begin(root).map_err(domain_refusal)?;
    transaction.commit(&plan).map_err(domain_refusal)?;
    Ok(DirectOutcome {
        activated: plan.activated,
        deactivated: plan.deactivated,
        listed: plan.new_list,
        warnings: plan.warnings,
    })
}

fn deactivate_direct(root: &Path, kind: &str, artifact_id: &str) -> Result<DirectOutcome, Refusal> {
    let roots = Roots::load(root).map_err(|error| compat_domain(&error))?;
    let yaml_key = yaml_key_for(kind)?;
    let stored = stored_id(kind, artifact_id, &roots)?;
    let artifact_kind = artifact_kind(kind);
    let (available_ids, deny_key, current_deny, required_by, also_matches) = match artifact_kind {
        Some(kind) => {
            let available = roots.available_ids(kind);
            let current_deny = ledger_deny(root, kind)?;
            let required_by = read_org_required_provenance(root, kind, &roots.identity, true)
                .map_err(|error| domain_refusal(error).with_code(ACTIVE_CHARTER_CONFIG_INVALID))?;
            let mut also: Vec<String> = Vec::new();
            if let Some(declared) = roots.identity.declared_id(kind, &stored)
                && declared != stored
            {
                also.push(declared);
            }
            if let Some(stem) = roots.identity.stem(kind, &stored)
                && stem != stored
            {
                also.push(stem);
            }
            (
                available,
                sk_doctrine::kind_deny_key(kind),
                current_deny,
                required_by,
                also,
            )
        }
        None => (
            roots.available_mission_types(),
            None,
            Vec::new(),
            BTreeMap::new(),
            Vec::new(),
        ),
    };
    let document = open_write_target(root)?;
    let inputs = DeactivationInputs {
        yaml_key: &yaml_key,
        config_data: &document,
        available_ids: &available_ids,
        required_by: &required_by,
        deny_key: deny_key.as_deref(),
        current_deny: &current_deny,
        record_exclusion: artifact_kind.is_some(),
        also_matches: &also_matches,
    };
    let plan = plan_deactivation(kind, &stored, &inputs).map_err(|error| {
        if let DoctrineError::RequiredArtifact { .. } = &error {
            let packs: Vec<String> = required_by
                .get(&stored)
                .map(|entries| entries.iter().map(|entry| entry.pack.clone()).collect())
                .unwrap_or_default();
            let mut refusal = Refusal::new(DEACTIVATION_REFUSED_REQUIRED, error.to_string());
            refusal
                .payload
                .insert("requiring_packs".to_owned(), json!(packs));
            refusal
        } else {
            domain_refusal(error)
        }
    })?;
    let transaction = ActivationTransaction::begin(root).map_err(domain_refusal)?;
    transaction.commit(&plan).map_err(domain_refusal)?;
    Ok(DirectOutcome {
        activated: plan.activated,
        deactivated: plan.deactivated,
        listed: plan.new_list,
        warnings: plan.warnings,
    })
}

/// One direct activation/deactivation's observable result (no printing).
#[derive(Debug)]
pub struct ActivationSummary {
    /// The ids the direct write activated.
    pub changed: Vec<String>,
    /// What a narrowing (`--only`) stopped putting in force.
    pub dropped: Vec<String>,
    /// The written list of a narrowing (`--only`), else `None`.
    pub listed: Option<Vec<String>>,
    /// Warnings the plan produced.
    pub warnings: Vec<String>,
}

fn summary(outcome: DirectOutcome) -> ActivationSummary {
    ActivationSummary {
        changed: outcome.activated,
        dropped: outcome.deactivated,
        listed: outcome.listed,
        warnings: outcome.warnings,
    }
}

fn activation_error(refusal: Refusal) -> CompatError {
    CompatError::coded(refusal.code, refusal.message)
}

/// Activate one artifact through the canonical plan/commit transaction.
pub fn activate(
    root: &Path,
    kind: &str,
    artifact_id: &str,
    only: bool,
) -> Result<ActivationSummary, CompatError> {
    activate_direct(root, kind, artifact_id, only, DenyListChoice::Activate)
        .map(summary)
        .map_err(activation_error)
}

/// Deactivate one artifact through the canonical plan/commit transaction.
pub fn deactivate(
    root: &Path,
    kind: &str,
    artifact_id: &str,
) -> Result<ActivationSummary, CompatError> {
    deactivate_direct(root, kind, artifact_id)
        .map(|outcome| ActivationSummary {
            changed: outcome.deactivated,
            dropped: Vec::new(),
            listed: outcome.listed,
            warnings: outcome.warnings,
        })
        .map_err(activation_error)
}

/// The cascade-activation report after a direct activation, as JSON.
pub fn cascade_activation_after(
    root: &Path,
    kind: &str,
    artifact_id: &str,
    cascade: Option<&str>,
    include_deactivated: bool,
) -> Result<Option<Value>, CompatError> {
    let Some(scope) = parse_scope(cascade).map_err(activation_error)? else {
        return Ok(None);
    };
    cascade_activation_report(root, kind, artifact_id, &scope, include_deactivated)
        .map(|report| Some(report.to_json()))
        .ok_or_else(|| {
            CompatError::domain("the cascade source has no resolvable DRG node".to_owned())
        })
}

/// The cascade-deactivation report after a direct deactivation, as JSON.
pub fn cascade_deactivation_after(
    root: &Path,
    kind: &str,
    artifact_id: &str,
    cascade: Option<&str>,
) -> Result<Option<Value>, CompatError> {
    let Some(scope) = parse_scope(cascade).map_err(activation_error)? else {
        return Ok(None);
    };
    cascade_deactivation_report(root, kind, artifact_id, &scope)
        .map(|report| Some(report.to_json()))
        .ok_or_else(|| {
            CompatError::domain("the cascade source has no resolvable DRG node".to_owned())
        })
}

impl Refusal {
    fn with_code(mut self, code: &'static str) -> Self {
        self.code = code;
        self
    }
}

fn ledger_deny(root: &Path, kind: ArtifactKind) -> Result<Vec<String>, Refusal> {
    let ledger = load_activation_ledger(root).map_err(domain_refusal)?;
    Ok(ledger.deny_for(kind).to_vec())
}

fn open_write_target(root: &Path) -> Result<YamlDocument, Refusal> {
    let resolved = resolve_activation_source(root).map_err(domain_refusal)?;
    let target = activation_write_target(root, &resolved).map_err(domain_refusal)?;
    YamlDocument::open(&target).map_err(domain_refusal)
}

// ---------------------------------------------------------------------------
// Cascade reports
// ---------------------------------------------------------------------------

/// One cascade entry, in render order.
#[derive(Clone, Debug)]
struct CascadeEntry {
    outcome: &'static str,
    kind: String,
    id: String,
    urn: Option<String>,
    message: Option<String>,
    required_by: Vec<String>,
    referenced_by: Option<String>,
    packs: Vec<String>,
    deactivated: bool,
}

impl CascadeEntry {
    fn new(outcome: &'static str, kind: &str, id: &str, urn: Option<&str>) -> Self {
        Self {
            outcome,
            kind: kind.to_owned(),
            id: id.to_owned(),
            urn: urn.map(str::to_owned),
            message: None,
            required_by: Vec::new(),
            referenced_by: None,
            packs: Vec::new(),
            deactivated: false,
        }
    }

    fn to_json(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("kind".to_owned(), json!(self.kind));
        payload.insert("id".to_owned(), json!(self.id));
        payload.insert("urn".to_owned(), json!(self.urn));
        match self.outcome {
            "failed" => {
                payload.insert("message".to_owned(), json!(self.message));
            }
            "skipped_deactivated" => {
                payload.insert("required_by".to_owned(), json!(self.required_by));
            }
            "skipped_out_of_scope" => {
                payload.insert("deactivated".to_owned(), json!(self.deactivated));
            }
            "skipped_shared" => {
                payload.insert("referenced_by".to_owned(), json!(self.referenced_by));
            }
            "skipped_required" => {
                payload.insert("packs".to_owned(), json!(self.packs));
            }
            _ => {}
        }
        Value::Object(payload)
    }

    fn label(&self) -> String {
        format!("{}/{}", self.kind, self.id)
    }
}

#[derive(Clone, Debug)]
struct CascadeReport {
    source_urn: String,
    scope: Value,
    entries: Vec<CascadeEntry>,
    zero_activatable: bool,
}

impl CascadeReport {
    fn of(&self, outcome: &str) -> Vec<&CascadeEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.outcome == outcome)
            .collect()
    }

    fn to_json(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("source_urn".to_owned(), json!(self.source_urn));
        payload.insert("scope".to_owned(), self.scope.clone());
        for outcome in [
            "cascaded",
            "reinstated",
            "failed",
            "skipped_out_of_scope",
            "skipped_deactivated",
            "unreached_behind_deactivated",
            "skipped_kind_filtered",
            "skipped_shared",
            "skipped_required",
            "still_active_unrestricted",
        ] {
            let entries: Vec<Value> = self
                .of(outcome)
                .iter()
                .map(|entry| entry.to_json())
                .collect();
            payload.insert(outcome.to_owned(), Value::Array(entries));
        }
        Value::Object(payload)
    }

    fn warning_lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.outcome == "cascaded")
            .filter_map(|entry| entry.message.clone())
            .collect()
    }
}

fn scope_json(scope: &CascadeScope) -> Value {
    if scope.is_all() {
        json!("all")
    } else {
        let mut tokens: Vec<String> = scope
            .explicit_kinds()
            .iter()
            .map(|kind| kind.operator_token().to_owned())
            .collect();
        tokens.sort();
        json!(tokens)
    }
}

/// Resolve an artifact's DRG URN from its kind token and stored id.
fn artifact_urn(
    roots: &Roots,
    graph: &sk_doctrine::DrgGraph,
    kind_token: &str,
    stored: &str,
) -> Option<String> {
    if kind_token == MISSION_TYPE_TOKEN {
        return Some(format!("mission_type:{stored}"));
    }
    let kind = artifact_kind(kind_token)?;
    let declared = roots
        .identity
        .declared_id(kind, stored)
        .unwrap_or_else(|| stored.to_owned());
    let candidates = [
        format!("{}:{declared}", kind.canonical()),
        format!("{}:{stored}", kind.canonical()),
    ];
    candidates.into_iter().find(|urn| graph.node(urn).is_some())
}

/// The kind token for a DRG URN prefix.
fn token_for_prefix(prefix: &str) -> String {
    match prefix {
        "agent_profile" => "agent-profile".to_owned(),
        "mission_step_contract" => "mission-step-contract".to_owned(),
        "glossary_pack" => "glossary-pack".to_owned(),
        other => other.to_owned(),
    }
}

fn kind_from_prefix(prefix: &str) -> Option<ArtifactKind> {
    ArtifactKind::from_operator_token(prefix).ok()
}

fn config_id(roots: &Roots, prefix: &str, bare: &str) -> String {
    match kind_from_prefix(prefix) {
        Some(kind) => roots
            .identity
            .stem(kind, bare)
            .or_else(|| roots.identity.declared_id(kind, bare))
            .unwrap_or_else(|| bare.to_owned()),
        None => bare.to_owned(),
    }
}

fn deactivated_urns(root: &Path, roots: &Roots) -> BTreeSet<String> {
    let ledger = match load_activation_ledger(root) {
        Ok(ledger) => ledger,
        Err(_) => return BTreeSet::new(),
    };
    let state = match ActivationState::from_ledger(&ledger, &roots.identity, &roots.floor) {
        Ok(state) => state,
        Err(_) => return BTreeSet::new(),
    };
    state.deactivated_urns()
}

fn no_cascade_report(root: &Path, kind: &str, artifact_id: &str) -> Option<NoCascade> {
    let roots = Roots::load(root).ok()?;
    let bundle = doctrine_bundle(root).ok()?;
    let source_urn = artifact_urn(&roots, &bundle.graph, kind, artifact_id)?;
    let denied = deactivated_urns(root, &roots);
    let report = referenced_but_not_cascaded(&bundle.graph, &source_urn, &denied);
    let mut out = NoCascade::default();
    let ledger = load_activation_ledger(root).ok()?;
    let state = ActivationState::from_ledger(&ledger, &roots.identity, &roots.floor).ok()?;
    let in_force = |prefix: &str, bare: &str| state.admits(prefix, bare);
    for (prefix, ids) in &report.skipped {
        for bare in ids {
            let entry = CascadeEntry::new(
                "skipped",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                None,
            );
            if in_force(prefix, bare) {
                out.already_in_force.push(entry);
            } else {
                out.skipped.push(entry);
            }
        }
    }
    for (prefix, ids) in &report.deactivated {
        for bare in ids {
            out.deactivated.push(CascadeEntry::new(
                "deactivated",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                None,
            ));
        }
    }
    for (prefix, ids) in &report.not_cascaded_kind_filtered {
        for bare in ids {
            out.kind_filtered.push(CascadeEntry::new(
                "kind_filtered",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                None,
            ));
        }
    }
    out.hint = if out.skipped.is_empty() {
        None
    } else {
        Some(report.recovery_hint.clone())
    };
    Some(out)
}

#[derive(Default)]
struct NoCascade {
    skipped: Vec<CascadeEntry>,
    deactivated: Vec<CascadeEntry>,
    kind_filtered: Vec<CascadeEntry>,
    already_in_force: Vec<CascadeEntry>,
    hint: Option<String>,
}

fn cascade_activation_report(
    root: &Path,
    kind: &str,
    artifact_id: &str,
    scope: &CascadeScope,
    include_deactivated: bool,
) -> Option<CascadeReport> {
    let roots = Roots::load(root).ok()?;
    let bundle = doctrine_bundle(root).ok()?;
    let source_urn = artifact_urn(&roots, &bundle.graph, kind, artifact_id)?;
    let denied = deactivated_urns(root, &roots);
    let result: CascadeActivationResult = cascade_activation_targets(
        &bundle.graph,
        &source_urn,
        scope,
        &denied,
        include_deactivated,
    );
    let mut entries: Vec<CascadeEntry> = Vec::new();
    for (prefix, ids) in &result.activated {
        let token = token_for_prefix(prefix);
        for bare in ids {
            let stored = config_id(&roots, prefix, bare);
            let mut entry = CascadeEntry::new(
                "cascaded",
                &token,
                &stored,
                Some(&format!("{prefix}:{bare}")),
            );
            match activate_direct(
                root,
                &token,
                &stored,
                false,
                if include_deactivated {
                    DenyListChoice::CascadeIncludeDeactivated
                } else {
                    DenyListChoice::Activate
                },
            ) {
                Ok(_) => {}
                Err(refusal) => {
                    entry.outcome = "failed";
                    entry.message = Some(refusal.message);
                }
            }
            entries.push(entry);
        }
    }
    for (prefix, ids) in &result.skipped_by_scope {
        for bare in ids {
            let urn = format!("{prefix}:{bare}");
            let mut entry = CascadeEntry::new(
                "skipped_out_of_scope",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                Some(&urn),
            );
            entry.deactivated = denied.contains(&urn) && !include_deactivated;
            entries.push(entry);
        }
    }
    for (prefix, refs) in &result.skipped_deactivated {
        for reference in refs {
            let mut entry = CascadeEntry::new(
                "skipped_deactivated",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, &reference.artifact_id),
                Some(&reference.urn),
            );
            if reference.via_requires {
                entry.required_by = reference.required_by.clone();
            }
            entries.push(entry);
        }
    }
    for (prefix, ids) in &result.unreached_behind_deactivated {
        for bare in ids {
            entries.push(CascadeEntry::new(
                "unreached_behind_deactivated",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                None,
            ));
        }
    }
    for (prefix, ids) in &result.not_cascaded_kind_filtered {
        for bare in ids {
            entries.push(CascadeEntry::new(
                "skipped_kind_filtered",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                None,
            ));
        }
    }
    let zero_activatable = result.activated.is_empty()
        && result.skipped_by_scope.is_empty()
        && result.skipped_deactivated.is_empty()
        && result.unreached_behind_deactivated.is_empty()
        && !result.not_cascaded_kind_filtered.is_empty();
    Some(CascadeReport {
        source_urn,
        scope: scope_json(scope),
        entries,
        zero_activatable,
    })
}

fn cascade_deactivation_report(
    root: &Path,
    kind: &str,
    artifact_id: &str,
    scope: &CascadeScope,
) -> Option<CascadeReport> {
    let roots = Roots::load(root).ok()?;
    let bundle = doctrine_bundle(root).ok()?;
    let stored = stored_id(kind, artifact_id, &roots).ok()?;
    let source_urn = artifact_urn(&roots, &bundle.graph, kind, &stored)?;
    let ledger = load_activation_ledger(root).ok()?;
    let state = ActivationState::from_ledger(&ledger, &roots.identity, &roots.floor).ok()?;
    let active = active_urns(&bundle.graph, &state);
    let plan = deactivation_plan(&bundle.graph, &source_urn, scope, &active);
    let mut entries: Vec<CascadeEntry> = Vec::new();
    for urn in &plan.deactivate {
        let (prefix, bare) = urn.split_once(':').unwrap_or((urn.as_str(), ""));
        let token = token_for_prefix(prefix);
        let stored_child = config_id(&roots, prefix, bare);
        let allow_absent = match kind_from_prefix(prefix) {
            Some(charter_kind) => ledger.allow_for(charter_kind).is_none(),
            None => false,
        };
        if allow_absent {
            entries.push(CascadeEntry::new(
                "still_active_unrestricted",
                &token,
                &stored_child,
                Some(urn),
            ));
            continue;
        }
        match deactivate_child(root, &token, &stored_child) {
            Ok(()) => entries.push(CascadeEntry::new(
                "cascaded",
                &token,
                &stored_child,
                Some(urn),
            )),
            Err(ChildRefusal::Required(packs)) => {
                let mut entry =
                    CascadeEntry::new("skipped_required", &token, &stored_child, Some(urn));
                entry.packs = packs;
                entries.push(entry);
            }
            Err(ChildRefusal::Other(message)) => {
                let mut entry = CascadeEntry::new("failed", &token, &stored_child, Some(urn));
                entry.message = Some(message);
                entries.push(entry);
            }
            Err(ChildRefusal::StillActive) => entries.push(CascadeEntry::new(
                "still_active_unrestricted",
                &token,
                &stored_child,
                Some(urn),
            )),
        }
    }
    for skip in &plan.skipped_shared {
        let (prefix, bare) = skip.urn.split_once(':').unwrap_or((skip.urn.as_str(), ""));
        let token = token_for_prefix(prefix);
        let stored_child = config_id(&roots, prefix, bare);
        let allow_absent = match kind_from_prefix(prefix) {
            Some(charter_kind) => ledger.allow_for(charter_kind).is_none(),
            None => false,
        };
        if allow_absent {
            entries.push(CascadeEntry::new(
                "still_active_unrestricted",
                &token,
                &stored_child,
                Some(&skip.urn),
            ));
        } else {
            let mut entry =
                CascadeEntry::new("skipped_shared", &token, &stored_child, Some(&skip.urn));
            entry.referenced_by = Some(skip.referencing_active_urn.clone());
            entries.push(entry);
        }
    }
    for (prefix, ids) in &plan.not_cascaded_kind_filtered {
        for bare in ids {
            entries.push(CascadeEntry::new(
                "skipped_kind_filtered",
                &token_for_prefix(prefix),
                &config_id(&roots, prefix, bare),
                None,
            ));
        }
    }
    Some(CascadeReport {
        source_urn,
        scope: scope_json(scope),
        entries,
        zero_activatable: false,
    })
}

enum ChildRefusal {
    Required(Vec<String>),
    StillActive,
    Other(String),
}

/// Remove one cascade child's allow entry; never records an exclusion.
fn deactivate_child(root: &Path, kind: &str, artifact_id: &str) -> Result<(), ChildRefusal> {
    let roots = match Roots::load(root) {
        Ok(roots) => roots,
        Err(error) => return Err(ChildRefusal::Other(error.message().to_owned())),
    };
    let yaml_key = match yaml_key_for(kind) {
        Ok(key) => key,
        Err(refusal) => return Err(ChildRefusal::Other(refusal.message)),
    };
    let Some(charter_kind) = artifact_kind(kind) else {
        return Err(ChildRefusal::StillActive);
    };
    let current_deny = match ledger_deny(root, charter_kind) {
        Ok(deny) => deny,
        Err(refusal) => return Err(ChildRefusal::Other(refusal.message)),
    };
    let required_by = match read_org_required_provenance(root, charter_kind, &roots.identity, true)
    {
        Ok(map) => map,
        Err(error) => return Err(ChildRefusal::Other(error.to_string())),
    };
    match check_deactivation_permitted(kind, artifact_id, &required_by) {
        Ok(()) => {}
        Err(DoctrineError::RequiredArtifact { .. }) => {
            let packs = required_by
                .get(artifact_id)
                .map(|entries| entries.iter().map(|pack| pack.pack.clone()).collect())
                .unwrap_or_default();
            return Err(ChildRefusal::Required(packs));
        }
        Err(error) => return Err(ChildRefusal::Other(error.to_string())),
    }
    let document = match open_write_target(root) {
        Ok(document) => document,
        Err(refusal) => return Err(ChildRefusal::Other(refusal.message)),
    };
    let available = roots.available_ids(charter_kind);
    let deny_key = sk_doctrine::kind_deny_key(charter_kind);
    let inputs = DeactivationInputs {
        yaml_key: &yaml_key,
        config_data: &document,
        available_ids: &available,
        required_by: &required_by,
        deny_key: deny_key.as_deref(),
        current_deny: &current_deny,
        record_exclusion: false,
        also_matches: &[],
    };
    let plan = match plan_deactivation(kind, artifact_id, &inputs) {
        Ok(plan) => plan,
        Err(error) => return Err(ChildRefusal::Other(error.to_string())),
    };
    let transaction = match ActivationTransaction::begin(root) {
        Ok(transaction) => transaction,
        Err(error) => return Err(ChildRefusal::Other(error.to_string())),
    };
    transaction
        .commit(&plan)
        .map(|_| ())
        .map_err(|error| ChildRefusal::Other(error.to_string()))
}

/// The active URNs of the post-direct-write state (Contract C3.4 safety set).
fn active_urns(graph: &sk_doctrine::DrgGraph, state: &ActivationState) -> BTreeSet<String> {
    let mut urns: BTreeSet<String> = BTreeSet::new();
    for (kind, allow) in &state.allow {
        let prefix = kind.canonical();
        match allow {
            Some(ids) => urns.extend(ids.iter().cloned()),
            None => urns.extend(
                graph
                    .nodes()
                    .iter()
                    .filter(|node| node.kind().as_str() == prefix)
                    .map(|node| node.urn().to_owned()),
            ),
        }
    }
    for deny in state.deny.values() {
        for urn in &deny.urns {
            urns.remove(urn);
        }
    }
    urns.extend(
        state
            .mission_types
            .iter()
            .map(|mission_type| format!("mission_type:{mission_type}")),
    );
    urns
}

// ---------------------------------------------------------------------------
// Change report
// ---------------------------------------------------------------------------

/// The `charter activate`/`deactivate` change report (reference shape).
struct ChangeReport {
    command: &'static str,
    kind: String,
    id: String,
    changed: Vec<String>,
    no_longer_in_force: Option<Vec<String>>,
    now_listed: Option<Vec<String>>,
    warnings: Vec<String>,
    referenced: Option<NoCascade>,
    cascade: Option<CascadeReport>,
    notices: Vec<String>,
}

impl ChangeReport {
    fn new(
        command: &'static str,
        kind: &str,
        id: &str,
        outcome: &DirectOutcome,
        only: bool,
    ) -> Self {
        Self {
            command,
            kind: kind.to_owned(),
            id: id.to_owned(),
            changed: if command == "activate" {
                outcome.activated.clone()
            } else {
                outcome.deactivated.clone()
            },
            no_longer_in_force: only.then(|| outcome.deactivated.clone()),
            now_listed: None,
            warnings: outcome.warnings.clone(),
            referenced: None,
            cascade: None,
            notices: Vec::new(),
        }
    }

    fn all_warnings(&self) -> Vec<String> {
        let mut warnings = self.warnings.clone();
        if let Some(cascade) = &self.cascade {
            warnings.extend(cascade.warning_lines());
        }
        warnings
    }

    fn to_json(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("schema_version".to_owned(), json!(SCHEMA_VERSION));
        payload.insert("command".to_owned(), json!(self.command));
        payload.insert("kind".to_owned(), json!(self.kind));
        payload.insert("id".to_owned(), json!(self.id));
        payload.insert("changed".to_owned(), json!(self.changed));
        payload.insert(
            "now_listed".to_owned(),
            json!(self.now_listed.clone().unwrap_or_default()),
        );
        payload.insert(
            "no_longer_in_force".to_owned(),
            match &self.no_longer_in_force {
                Some(ids) => json!(ids),
                None => Value::Null,
            },
        );
        payload.insert("warnings".to_owned(), json!(self.all_warnings()));
        payload.insert("notices".to_owned(), json!(self.notices));
        let (not_cascaded, already_in_force, kind_filtered) = match &self.referenced {
            Some(report) => (
                json!(
                    report
                        .skipped
                        .iter()
                        .chain(report.deactivated.iter())
                        .map(CascadeEntry::to_json)
                        .collect::<Vec<_>>()
                ),
                json!(
                    report
                        .already_in_force
                        .iter()
                        .map(CascadeEntry::to_json)
                        .collect::<Vec<_>>()
                ),
                json!(
                    report
                        .kind_filtered
                        .iter()
                        .map(CascadeEntry::to_json)
                        .collect::<Vec<_>>()
                ),
            ),
            None => (json!([]), json!([]), json!([])),
        };
        payload.insert("referenced_not_cascaded".to_owned(), not_cascaded);
        payload.insert("referenced_already_in_force".to_owned(), already_in_force);
        payload.insert("referenced_kind_filtered".to_owned(), kind_filtered);
        payload.insert(
            "cascade".to_owned(),
            self.cascade
                .as_ref()
                .map_or(Value::Null, CascadeReport::to_json),
        );
        Value::Object(payload)
    }
}

fn print_change_report(report: &ChangeReport) {
    for change in &report.changed {
        if report.command == "activate" {
            println!("Activated: {change}");
        } else {
            println!("Deactivated: {change}");
        }
    }
    for warning in &report.warnings {
        println!("Warning: {warning}");
    }
    if let Some(listed) = &report.now_listed {
        println!(
            "Now listed for {} ({}): {}",
            report.kind,
            listed.len(),
            if listed.is_empty() {
                "(none)".to_owned()
            } else {
                listed.join(", ")
            }
        );
        match &report.no_longer_in_force {
            Some(dropped) => println!(
                "No longer in force ({}): {}",
                dropped.len(),
                if dropped.is_empty() {
                    "(none)".to_owned()
                } else {
                    dropped.join(", ")
                }
            ),
            None => println!("No longer in force: unknown (see the warning above)"),
        }
    }
    if let Some(referenced) = &report.referenced {
        for entry in &referenced.already_in_force {
            println!(
                "Note: referenced {} is already in force (no explicit list); not added by this command.",
                entry.label()
            );
        }
        for entry in &referenced.skipped {
            println!(
                "Warning: referenced {} was not activated (no --cascade).",
                entry.label()
            );
        }
        for entry in &referenced.deactivated {
            println!(
                "Warning: referenced {} is deactivated and was not activated.",
                entry.label()
            );
        }
        if let Some(hint) = &referenced.hint {
            println!("Hint: {hint}");
        }
        for entry in &referenced.kind_filtered {
            println!(
                "Not cascaded: {} (kind not charter-activatable)",
                entry.label()
            );
        }
    }
    if let Some(cascade) = &report.cascade {
        print_cascade(report.command, cascade);
    }
}

fn print_cascade(command: &str, report: &CascadeReport) {
    for entry in &report.entries {
        let label = entry.label();
        match entry.outcome {
            "cascaded" => {
                let verb = if command == "deactivate" {
                    "Cascade-deactivated"
                } else {
                    "Cascade-activated"
                };
                println!("{verb}: {label}");
                if let Some(message) = &entry.message {
                    println!("Warning: {message}");
                }
            }
            "reinstated" => println!("Reinstated: {label} (exclusion cleared)"),
            "failed" => println!(
                "Warning: could not cascade-{} {label}: {}",
                if command == "deactivate" {
                    "deactivate"
                } else {
                    "activate"
                },
                entry.message.as_deref().unwrap_or("")
            ),
            "skipped_out_of_scope" => {
                let note = if entry.deactivated {
                    " (deactivated; nothing behind it was walked)"
                } else {
                    ""
                };
                println!("Skipped (out of scope): {label}{note}");
            }
            "skipped_deactivated" => {
                let note = if entry.required_by.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (required by {} — the parent no longer has this requirement)",
                        entry.required_by.join(", ")
                    )
                };
                println!("Skipped, deactivated: {label}{note}");
            }
            "unreached_behind_deactivated" => {
                println!("Unreached (behind a deactivated artifact): {label}");
            }
            "skipped_kind_filtered" => {
                println!("Not cascaded: {label} (kind not charter-activatable)");
            }
            "skipped_shared" => println!(
                "Skipped (shared artifact): {} (still referenced by {})",
                entry.urn.as_deref().unwrap_or(&label),
                entry.referenced_by.as_deref().unwrap_or("")
            ),
            "skipped_required" => println!(
                "Cascade: {label}: skipped (required by {})",
                entry.packs.join(", ")
            ),
            "still_active_unrestricted" => println!(
                "Cascade: {label}: still active (no allow-list; only the named artifact excluded). To turn it off: `spec-kitty charter deactivate {} {}`",
                entry.kind, entry.id
            ),
            _ => {}
        }
    }
    if report.zero_activatable {
        println!(
            "Cascade resolved zero activatable targets (every referenced node was kind-filtered; see the lines above)."
        );
    }
}

/// The post-change tail: the reference's catalog refresh / `--no-compile`
/// notice, degraded honestly where the native compiler is not ported.
fn post_change_notices(root: &Path, kind: &str, no_compile: bool) -> Vec<String> {
    let mut notices = Vec::new();
    if no_compile {
        notices.push(
            "Catalog not recompiled (--no-compile): catalog.references may go stale until the next `charter generate` or `charter activate --resynthesize`."
                .to_owned(),
        );
    } else if root.join(CHARTER_YAML).exists() {
        notices.push(
            "Catalog not recompiled (native compiler pending): the derived catalog is not regenerated by this native surface; catalog.references may go stale until the native charter compiler lands."
                .to_owned(),
        );
    }
    if kind == ArtifactKind::Skill.operator_token() {
        notices.push(
            "Pack skill projection is pending natively: the activation change is recorded, but no SKILL.md file was written."
                .to_owned(),
        );
    }
    notices
}

// ---------------------------------------------------------------------------
// Status (read-only report; see `charter/status.rs`)
// ---------------------------------------------------------------------------

/// Delegate to the read-only status operation.
///
/// The collectors live in [`status`] because they compose several owners
/// (charter sync hash, synthesis manifest, freshness, activation ledger) and
/// every one of them is a pure read: this surface never generates, synthesizes,
/// repairs or writes.
fn status_command(args: &StatusArgs) -> Result<u8, CompatError> {
    status::status_command(args)
}

/// The read-only `charter status` payload for `root`.
///
/// Public so tests (and a parent integration surface) can evaluate the report
/// without a process-wide working directory. A refusal carries the reference's
/// machine code (`ACTIVE_CHARTER_CONFIG_INVALID`, `CHARTER_CONFIG_SHAPE_INVALID`,
/// `INTERNAL_ERROR`).
pub fn status_payload(root: &Path, provenance: bool) -> Result<Value, CompatError> {
    status::payload(root, provenance)
}

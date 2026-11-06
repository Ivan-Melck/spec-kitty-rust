//! Mission authoring routes: `agent mission create`, `specify`, `plan`,
//! `agent mission setup-plan`, `tasks` and `agent mission finalize-tasks`.
//!
//! Creation and dependency finalization are executed by `sk-mission` plans;
//! this module resolves canonical templates, supplies the request, and renders
//! the compatibility payload.
//!
//! # `finalize-tasks` (CF15, write placement + ownership validation CF18)
//!
//! `finalize_tasks` is the real orchestration, not a payload-only plan. It
//! composes the delivered owners in the pinned reference's phase order
//! (`tasks_finalize._do_finalize_tasks`: resolve → validate → apply → output):
//!
//! 1. **resolve** — project root, Mission handle, `tasks.md` and `tasks/`
//!    existence, plus the pre-3.0 lane-directory guard;
//! 2. **validate** — the bulk-edit occurrence-map gate (first, fail-fast, like
//!    `tasks_finalize._ft_validate_occurrence_map_ready`) and then the
//!    dependency plan, whose coverage / disagree-loud conflict / self / unknown /
//!    cycle gates are all pre-write refusals computed by
//!    [`DependencyFinalizePlan::compute`];
//! 3. **apply** — frontmatter writes gated on `--validate-only`, then the
//!    coordination STATUS_STATE write location is established (materialize /
//!    seed / refuse, exactly `tasks_finalize._ft_emit_status_events`'s
//!    `placement_seam(...).write_dir(STATUS_STATE)`), then the `lanes.json`
//!    planning write (after Source's ownership glob re-validation) and then the
//!    canonical status bootstrap, all under the Source partial-write contract
//!    (completed writes remain on failure);
//! 4. **output** — the reference's machine payload.
//!
//! Every Git/Status/Lanes write goes through the canonical owner
//! (`sk-workspace` `LanesStore` and `establish_write_location`, `sk-status`
//! `emit_status_transition`, `sk-kernel` `atomic_write`). Nothing here invents a
//! commit, a planning sha, an event id, a timestamp or a readiness verdict, and
//! the route never commits.
//!
//! ## Deliberate differences from the reference (declared, not hidden)
//!
//! - `--validate-only` performs the reference's dry-run bootstrap *read* but the
//!   `would_modify` list carries the same `changes.dependencies` shape the live
//!   path would write (the reference emits the same map, it does not diff files).
//! - `finalize-tasks` has no implicit rollback in Source. Explicit Native
//!   owned-byte recovery remains available through the Workspace owner, but is
//!   not silently applied by this compatibility command.
//! - The status bootstrap writes into the location the canonical workspace owner
//!   establishes (the coordination Mission directory for `coord` /
//!   `lanes_with_coord`), while the WP scan, the dependency plan and the identity
//!   payload keep reading the PRIMARY directory. The reference's bootstrap reads
//!   that location through its transactional writer and the published Mission
//!   short-circuit; this route derives the published phase explicitly and reads
//!   the coordination directory directly once it exists.
//! - The ownership glob re-validation reproduces the reference's refusal
//!   decision and its `ownership_literal_path_errors` payload. The reference also
//!   appends a `difflib` "Did you mean '<path>'?" hint to that message; this route
//!   omits the hint rather than shipping an unverified approximation of
//!   `SequenceMatcher`, and the refusal itself is byte-identical.
//! - Source's transactional status writer commits one commit per seed on the
//!   status surface. This route writes the seeded bytes through the canonical
//!   status owner but does not commit them; the gap is declared, not simulated.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_mission::{
    CanonicalTemplate, DependencyFinalizePlan, Mission, MissionCreateOptions, MissionCreatePlan,
    MissionCreateRequest, MissionIdentity, MissionRetention, MissionTemplateKind, MissionTemplates,
    MissionTopology, WorkPackage, WpId,
};

use super::support::{
    CompatError, emit_json, humanize_slug, inject_branch_contract, path_string, resolve_mission,
    resolve_mission_template,
};

/// Canonical create-time shape requested by `--topology`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyChoice {
    SingleBranch,
    Lanes,
    Coord,
    LanesWithCoord,
}

impl TopologyChoice {
    /// Parse the upstream token.
    pub fn parse(value: &str) -> Result<Self, CompatError> {
        match value.trim() {
            "single_branch" => Ok(Self::SingleBranch),
            "lanes" => Ok(Self::Lanes),
            "coord" => Ok(Self::Coord),
            "lanes_with_coord" => Ok(Self::LanesWithCoord),
            other => Err(CompatError::usage(format!(
                "invalid --topology {other:?}: expected single_branch, lanes, coord, or lanes_with_coord"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::SingleBranch => "single_branch",
            Self::Lanes => "lanes",
            Self::Coord => "coord",
            Self::LanesWithCoord => "lanes_with_coord",
        }
    }

    fn mission_topology(self) -> MissionTopology {
        match self {
            Self::SingleBranch => MissionTopology::SingleBranch,
            Self::Lanes => MissionTopology::Lanes,
            Self::Coord => MissionTopology::Coord,
            Self::LanesWithCoord => MissionTopology::LanesWithCoord,
        }
    }

    fn routes_through_coordination(self) -> bool {
        matches!(self, Self::Coord | Self::LanesWithCoord)
    }
}

/// Options accepted by `agent mission create` that this slice honors.
#[derive(Clone)]
pub struct CreateOptions<'a> {
    pub slug: &'a str,
    pub mission_type: Option<&'a str>,
    pub target_branch: Option<&'a str>,
    pub friendly_name: Option<&'a str>,
    pub purpose_tldr: Option<&'a str>,
    pub purpose_context: Option<&'a str>,
    pub topology: Option<TopologyChoice>,
    pub pr_bound: bool,
    pub retain_branches: bool,
    pub retain_worktrees: bool,
    pub allow_duplicate: bool,
    /// Unsupported required options, refused before any mutation.
    pub unsupported: Vec<(&'static str, String)>,
}

impl<'a> CreateOptions<'a> {
    /// Build options with every optional value absent.
    pub fn new(slug: &'a str) -> Self {
        Self {
            slug,
            mission_type: None,
            target_branch: None,
            friendly_name: None,
            purpose_tldr: None,
            purpose_context: None,
            topology: None,
            pr_bound: false,
            retain_branches: false,
            retain_worktrees: false,
            allow_duplicate: false,
            unsupported: Vec::new(),
        }
    }
}

/// Slugify a `specify` name exactly as upstream does, refusing non-ASCII.
pub fn slugify(name: &str) -> Result<String, CompatError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CompatError::domain("no name given"));
    }
    if !trimmed.is_ascii() {
        return Err(CompatError::domain(format!(
            "name {trimmed:?} has no usable ASCII characters"
        )));
    }
    let slug: String = {
        let mut out = String::new();
        for character in trimmed.to_ascii_lowercase().chars() {
            if character.is_ascii_lowercase() || character.is_ascii_digit() {
                out.push(character);
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
        out.trim_matches('-').to_owned()
    };
    if slug.is_empty() {
        return Err(CompatError::domain(format!(
            "name {trimmed:?} has no usable ASCII characters"
        )));
    }
    Ok(slug)
}

fn mission_type_of(mission: &sk_mission::Mission) -> String {
    mission
        .mission_type()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("software-dev")
        .to_owned()
}

fn target_branch_of(mission: &sk_mission::Mission, fallback: &str) -> String {
    mission
        .meta_field("target_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.to_owned())
}

fn canonical_template(
    resolved: &super::support::ResolvedMissionTemplate,
    kind: MissionTemplateKind,
) -> Result<CanonicalTemplate, CompatError> {
    CanonicalTemplate::new(
        kind,
        resolved.id(),
        resolved.path.clone(),
        resolved.text.clone(),
    )
    .map_err(|error| CompatError::domain(format!("invalid canonical template: {error}")))
}

/// Refuse required options this slice cannot honor, before any mutation.
fn refuse_unsupported(options: &CreateOptions<'_>) -> Result<(), CompatError> {
    if let Some((option, detail)) = options.unsupported.first() {
        return Err(CompatError::unsupported(
            "UNSUPPORTED_REQUIRED_OPTION",
            format!("{option} is not supported by the native create slice: {detail}"),
        )
        .with("option", json!(option)));
    }
    let topology = options.topology.unwrap_or(TopologyChoice::Coord);
    if topology.routes_through_coordination() {
        return Err(CompatError::unsupported(
            "COORDINATION_TOPOLOGY_UNSUPPORTED",
            format!(
                "--topology {} requires minting and materializing a coordination branch; the workspace owner is not integrated yet, so this slice refuses before writing anything",
                topology.as_str()
            ),
        )
        .with("topology", json!(topology.as_str())));
    }
    Ok(())
}

/// Create a mission scaffold and return the upstream-shaped payload.
pub fn create(root: &Path, options: &CreateOptions<'_>) -> Result<Value, CompatError> {
    refuse_unsupported(options)?;
    if !root.join(".kittify").is_dir() {
        return Err(CompatError::coded(
            "SPEC_KITTY_REPO_NOT_INITIALIZED",
            format!(
                "{} is not an initialized Spec Kitty project (missing .kittify/); run `spec-kitty init` first",
                root.display()
            ),
        ));
    }

    let mission_type = options.mission_type.unwrap_or("software-dev").trim();
    if mission_type.is_empty() {
        return Err(CompatError::usage("--mission-type must not be empty"));
    }

    let current = super::support::current_branch(root).unwrap_or_default();
    let target = match options.target_branch {
        Some(branch) if !branch.trim().is_empty() => branch.trim().to_owned(),
        _ => {
            if current.is_empty() {
                return Err(CompatError::domain(
                    "could not determine the target branch: the project is not a Git work tree; pass --target-branch",
                ));
            }
            current.clone()
        }
    };

    let resolved = resolve_mission_template(root, mission_type, MissionTemplateKind::Spec)?;
    let templates =
        MissionTemplates::new(canonical_template(&resolved, MissionTemplateKind::Spec)?);

    let mut request = MissionCreateRequest::new(options.slug, target.clone())
        .map_err(|error| CompatError::domain(format!("invalid mission slug: {error}")))?
        .with_mission_type(mission_type)
        .with_topology(
            options
                .topology
                .unwrap_or(TopologyChoice::Coord)
                .mission_topology(),
        )
        .with_retention(MissionRetention {
            pr_bound: options.pr_bound,
            retain_branches: options.retain_branches,
            retain_worktrees: options.retain_worktrees,
            commit_to_target: false,
        });

    let friendly = options
        .friendly_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| humanize_slug(options.slug));
    request = request.with_friendly_name(friendly.clone());
    if options.purpose_tldr.is_some() || options.purpose_context.is_some() {
        request = request.with_purpose(
            options.purpose_tldr.unwrap_or("").trim().to_owned(),
            options.purpose_context.unwrap_or("").trim().to_owned(),
        );
    }

    let identity = MissionIdentity::mint()
        .map_err(|error| CompatError::domain(format!("could not mint a mission id: {error}")))?;
    let create_options = MissionCreateOptions::new().allow_duplicate(options.allow_duplicate);
    let plan = MissionCreatePlan::plan(root, request, identity, &templates, &create_options)
        .map_err(|error| CompatError::domain(format!("could not plan the mission: {error}")))?;
    let receipt = plan
        .commit(root)
        .map_err(|error| CompatError::domain(format!("could not create the mission: {error}")))?;

    let spec_dir = plan.spec_dir().to_path_buf();
    let spec_file = spec_dir.join("spec.md");
    let meta_file = spec_dir.join("meta.json");
    let created: Vec<Value> = receipt
        .created()
        .iter()
        .map(|path| json!(path_string(path)))
        .collect();
    let created_files: Vec<PathBuf> = receipt.created().to_vec();

    let meta = plan.meta();
    let string_field = |key: &str| {
        meta.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let mission_number = plan.mission_number().unwrap_or(0);

    let mut payload = Map::new();
    payload.insert("result".into(), json!("success"));
    payload.insert("mission_slug".into(), json!(plan.directory_name().as_str()));
    payload.insert("mission_number".into(), json!(mission_number));
    payload.insert("mission_id".into(), json!(string_field("mission_id")));
    payload.insert("mission_type".into(), json!(string_field("mission_type")));
    payload.insert("slug".into(), json!(string_field("slug")));
    payload.insert("friendly_name".into(), json!(string_field("friendly_name")));
    payload.insert("purpose_tldr".into(), json!(string_field("purpose_tldr")));
    payload.insert(
        "purpose_context".into(),
        json!(string_field("purpose_context")),
    );
    payload.insert("mission_dir".into(), json!(path_string(&spec_dir)));
    payload.insert("feature_dir".into(), json!(path_string(&spec_dir)));
    payload.insert("spec_file".into(), json!(path_string(&spec_file)));
    payload.insert("meta_file".into(), json!(path_string(&meta_file)));
    payload.insert("created_at".into(), json!(string_field("created_at")));
    payload.insert("created_files".into(), Value::Array(created));
    payload.insert("mission_branch".into(), Value::Null);
    payload.insert("commit_to_target".into(), Value::Null);
    payload.insert("allocation".into(), json!("planning_artifacts_only"));
    payload.insert(
        "uncommitted_artifacts".into(),
        json!([{
            "path": path_string(&spec_file),
            "reason": "Scaffolded empty at create time; populated and committed with substantive content later (#846).",
            "responsible_command": "/spec-kitty.specify",
        }]),
    );
    payload.insert("write_mode".into(), json!("update_existing_files"));
    payload.insert("scaffold_only".into(), json!(true));
    payload.insert("requires_agent_authoring".into(), json!(true));
    payload.insert(
        "plan_guard".into(),
        json!("SPEC_NOT_SUBSTANTIVE_OR_UNCOMMITTED"),
    );
    payload.insert(
        "next_step".into(),
        json!("Created scaffold only. Run `/spec-kitty.specify <intent>` in your agent or edit and commit spec_file before `spec-kitty plan`."),
    );
    payload.insert(
        "origin_binding".into(),
        json!({"attempted": false, "succeeded": false, "error": null}),
    );
    payload.insert("coordination_branch".into(), Value::Null);
    payload.insert("coordination_branch_created".into(), json!(false));
    payload.insert("topology".into(), json!(string_field("topology")));
    payload.insert(
        "spec_kitty_version".into(),
        json!(env!("CARGO_PKG_VERSION")),
    );
    payload.insert("committed".into(), json!(false));
    inject_branch_contract(&mut payload, &target, &current, &target);
    payload.insert("created_files_count".into(), json!(created_files.len()));
    Ok(Value::Object(payload))
}

/// The `specify` wrapper: create plus scaffold-only guidance.
pub fn specify(root: &Path, options: &CreateOptions<'_>) -> Result<Value, CompatError> {
    let slug = slugify(options.slug)?;
    let mut enriched = options.clone();
    enriched.slug = &slug;
    // `specify` takes a name, so the derived title must reflect it.
    let mut value = create(root, &enriched)?;
    let object = value.as_object_mut().expect("create returns a JSON object");
    object.insert("scaffold_only".into(), json!(true));
    object.insert("spec_state".into(), json!("scaffold_only"));
    let next_action = format!(
        "Open spec_file and replace the scaffold with a complete specification; then run `spec-kitty plan --mission {}`.",
        slug
    );
    object.insert("next_action".into(), json!(next_action));
    object.insert("next_step".into(), json!(next_action));
    Ok(value)
}

/// Scaffold `plan.md` from the canonical template.
pub fn plan(root: &Path, handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let handle = handle.map(str::trim).filter(|value| !value.is_empty());
    let Some(handle) = handle else {
        return Err(CompatError::domain("--mission <slug> is required"));
    };
    let mission = resolve_mission(root, Some(handle))?;
    let mission_type = mission_type_of(&mission);
    let target = target_branch_of(&mission, "");
    let current = super::support::current_branch(root).unwrap_or_else(|| target.clone());
    let spec_file = mission.spec_dir().join("spec.md");
    if !spec_file.is_file() {
        return Err(CompatError::coded(
            "SPEC_FILE_MISSING",
            format!("spec.md is missing at {}", spec_file.display()),
        ));
    }

    let resolved = resolve_mission_template(root, &mission_type, MissionTemplateKind::Plan)?;
    let plan_file = mission.spec_dir().join("plan.md");
    let existed = plan_file.exists();
    if !existed {
        sk_kernel::atomic_write(&plan_file, resolved.text.as_bytes()).map_err(|error| {
            CompatError::domain(format!("could not write {}: {error}", plan_file.display()))
        })?;
    }
    let current_bytes = std::fs::read(&plan_file).map_err(|error| {
        CompatError::domain(format!("could not read {}: {error}", plan_file.display()))
    })?;
    let scaffold_only = current_bytes == resolved.text.as_bytes();

    let mut payload = Map::new();
    payload.insert("result".into(), json!("success"));
    payload.insert("phase_complete".into(), json!(!scaffold_only));
    payload.insert("mission_slug".into(), json!(mission.slug().as_str()));
    payload.insert("plan_file".into(), json!(path_string(&plan_file)));
    payload.insert("mission_dir".into(), json!(path_string(mission.spec_dir())));
    payload.insert("feature_dir".into(), json!(path_string(mission.spec_dir())));
    payload.insert("spec_file".into(), json!(path_string(&spec_file)));
    payload.insert("plan_substantive".into(), json!(!scaffold_only));
    payload.insert("requirement_id_warnings".into(), json!([]));
    if scaffold_only {
        payload.insert("scaffold_only".into(), json!(true));
    }
    payload.insert("plan_template".into(), json!(path_string(&resolved.path)));
    payload.insert("template_tier".into(), json!(resolved.tier));
    payload.insert("scaffold_written".into(), json!(!existed));
    payload.insert("committed".into(), json!(false));
    payload.insert(
        "spec_kitty_version".into(),
        json!(env!("CARGO_PKG_VERSION")),
    );
    inject_branch_contract(&mut payload, &target, &current, &target);
    let value = Value::Object(payload);
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("✓ Plan scaffolded: {}", plan_file.display());
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// `finalize-tasks` orchestration (CF15)
// ---------------------------------------------------------------------------

/// The reference phase an orchestration refusal belongs to.
///
/// `tasks_finalize._do_finalize_tasks` runs `resolve → validate → apply →
/// output`. Reporting the phase lets a caller tell a refusal that wrote nothing
/// from one that had to undo its own bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalizePhase {
    /// Project/Mission/tasks-artifact resolution and the pre-3.0 guard.
    Resolve,
    /// Every pre-write gate (occurrence map, coverage, graph, conflict).
    Validate,
    /// The write leg: prompt frontmatter, `lanes.json`, status bootstrap.
    Apply,
    /// Payload construction and emission.
    Output,
}

impl FinalizePhase {
    /// The stable machine token used in the refusal payload.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Validate => "validate",
            Self::Apply => "apply",
            Self::Output => "output",
        }
    }
}

/// A typed `finalize-tasks` refusal carrying its reference phase.
#[derive(Debug)]
pub struct FinalizeFailure {
    phase: FinalizePhase,
    message: String,
    details: Map<String, Value>,
}

impl FinalizeFailure {
    fn new(phase: FinalizePhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
            details: Map::new(),
        }
    }

    fn resolve(message: impl Into<String>) -> Self {
        Self::new(FinalizePhase::Resolve, message)
    }

    fn validate(message: impl Into<String>) -> Self {
        Self::new(FinalizePhase::Validate, message)
    }

    fn apply(message: impl Into<String>) -> Self {
        Self::new(FinalizePhase::Apply, message)
    }

    fn with_detail(mut self, key: impl Into<String>, value: Value) -> Self {
        self.details.insert(key.into(), value);
        self
    }

    fn into_compat(self) -> CompatError {
        let phase = self.phase.as_str();
        let mut error = CompatError::domain(self.message).with("phase", json!(phase));
        for (key, value) in self.details {
            error = error.with(key, value);
        }
        error
    }
}

/// Source `status.bootstrap`'s `BootstrapResult`.
#[derive(Clone, Debug, Default)]
pub struct BootstrapResult {
    total_wps: usize,
    already_initialized: usize,
    newly_seeded: usize,
    skipped: usize,
    wp_details: BTreeMap<String, String>,
}

impl BootstrapResult {
    fn to_json(&self) -> Value {
        let mut details = Map::new();
        for (wp, detail) in &self.wp_details {
            details.insert(wp.clone(), json!(detail));
        }
        json!({
            "total_wps": self.total_wps,
            "already_initialized": self.already_initialized,
            "newly_seeded": self.newly_seeded,
            "skipped": self.skipped,
            "wp_details": Value::Object(details),
        })
    }
}

/// The `status.bootstrap` detail tokens, preserved verbatim.
const BOOTSTRAP_INITIALIZED: &str = "initialized";
const BOOTSTRAP_ALREADY_EXISTS: &str = "already_exists";
const BOOTSTRAP_WOULD_SEED: &str = "would_seed";
const BOOTSTRAP_SKIPPED_MALFORMED: &str = "skipped_malformed";

/// The reference's seed `reason` (`status.bootstrap.bootstrap_canonical_state`).
const BOOTSTRAP_REASON: &str = "canonical bootstrap";

/// The reference's bulk-edit gate message.
const OCCURRENCE_GATE_BLOCKED: &str = "Bulk edit occurrence-map gate blocked finalize-tasks.";

/// The reference's coverage-gate message.
const COVERAGE_GATE_MESSAGE: &str = "tasks.md work package coverage is incomplete. finalize-tasks could not match all WP files to parsed sections, so dependency lanes would be unreliable.";

/// Pre-3.0 lane directories (Source `upgrade.legacy_detector.LEGACY_LANE_DIRS`).
const LEGACY_LANE_DIRS: [&str; 4] = ["planned", "doing", "for_review", "done"];

/// Default budget for one `occurrence_map.yaml` read.
const OCCURRENCE_MAP_BYTE_LIMIT: u64 = 4 * 1024 * 1024;

/// The canonical `occurrence_map.yaml` file name.
const OCCURRENCE_MAP_FILE_NAME: &str = "occurrence_map.yaml";

/// Finalize `tasks.md` dependencies into WP frontmatter, then land the planning
/// lanes and the canonical status bootstrap.
///
/// This is the real orchestration over the delivered owners. See the module
/// documentation for the phase order and the declared differences from the
/// pinned reference.
pub fn finalize_tasks(
    root: &Path,
    handle: Option<&str>,
    validate_only: bool,
    json_output: bool,
) -> Result<Value, CompatError> {
    let payload =
        run_finalize_tasks(root, handle, validate_only).map_err(FinalizeFailure::into_compat)?;
    if json_output {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        let updated = payload
            .get("updated_wp_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let bootstrap = payload.get("bootstrap");
        let seeded = bootstrap
            .and_then(|value| value.get("newly_seeded"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let existing = bootstrap
            .and_then(|value| value.get("already_initialized"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if validate_only {
            println!(
                "tasks validation passed: {updated} WP file(s) would change (bootstrap: {seeded} would seed, {existing} existing)"
            );
        } else {
            println!(
                "✓ Updated {updated} WP files with dependencies (bootstrap: {seeded} seeded, {existing} existing)"
            );
        }
    }
    Ok(payload)
}

/// Run the pipeline and build the reference payload.
fn run_finalize_tasks(
    root: &Path,
    handle: Option<&str>,
    validate_only: bool,
) -> Result<Value, FinalizeFailure> {
    // --- Phase: resolve -----------------------------------------------------
    let mission = resolve_mission(root, handle)
        .map_err(|error| FinalizeFailure::resolve(error.message().to_owned()))?;
    let mission_dir = mission.spec_dir().to_path_buf();
    // Boundary guard: hard-reject a pre-3.0 layout before any WP mutation (#1057).
    check_pre30_layout(&mission_dir)?;
    let tasks_md = mission_dir.join(sk_mission::TASKS_FILE_NAME);
    let tasks_dir = mission_dir.join(sk_mission::TASKS_DIR_NAME);
    if !tasks_md.is_file() {
        return Err(FinalizeFailure::resolve(format!(
            "tasks.md not found: {}",
            tasks_md.display()
        )));
    }
    if !tasks_dir.is_dir() {
        return Err(FinalizeFailure::resolve(format!(
            "Tasks directory not found: {}",
            tasks_dir.display()
        )));
    }

    // --- Phase: validate ----------------------------------------------------
    validate_occurrence_map_gate(&mission)?;
    let plan = DependencyFinalizePlan::compute(&tasks_md, &tasks_dir).map_err(map_plan_error)?;

    let mut dependencies = Map::new();
    for (wp, deps) in plan.effective_dependencies() {
        dependencies.insert(
            wp.as_str().to_owned(),
            Value::Array(deps.iter().map(|dep| json!(dep.as_str())).collect()),
        );
    }

    if validate_only {
        let surface = status_read_surface(&mission)?;
        let bootstrap = preview_canonical_state(&mission, &surface)?;
        let would_modify: Vec<Value> = plan
            .updates()
            .iter()
            .map(|update| {
                json!({
                    "wp_id": update.wp_id().as_str(),
                    "changes": {
                        "dependencies": Value::Array(
                            update.dependencies().iter().map(|dep| json!(dep.as_str())).collect()
                        ),
                    },
                })
            })
            .collect();
        let mut payload = Map::new();
        payload.insert("result".into(), json!("validation_passed"));
        payload.insert("validate_only".into(), json!(true));
        payload.insert("would_modify".into(), Value::Array(would_modify));
        payload.insert("would_preserve".into(), json!(plan.preserved_wps()));
        payload.insert("unchanged".into(), json!(plan.unchanged_wps()));
        payload.insert("updated_wp_count".into(), json!(plan.updated_wps().len()));
        payload.insert("dependencies".into(), Value::Object(dependencies));
        merge_identity(&mut payload, &mission);
        payload.insert("bootstrap".into(), bootstrap.to_json());
        return Ok(Value::Object(payload));
    }

    // --- Phase: apply -------------------------------------------------------
    let bootstrap = apply_finalize_side_effects(root, &mission, &plan, &tasks_dir)?;

    // --- Phase: output ------------------------------------------------------
    let mut payload = Map::new();
    payload.insert("result".into(), json!("success"));
    payload.insert("updated_wp_count".into(), json!(plan.updated_wps().len()));
    payload.insert("modified_wps".into(), json!(plan.updated_wps()));
    payload.insert("unchanged_wps".into(), json!(plan.unchanged_wps()));
    payload.insert("preserved_wps".into(), json!(plan.preserved_wps()));
    payload.insert("dependencies".into(), Value::Object(dependencies));
    merge_identity(&mut payload, &mission);
    payload.insert("bootstrap".into(), bootstrap.to_json());
    Ok(Value::Object(payload))
}

/// The reference's `_mission_identity_payload` fields.
fn merge_identity(payload: &mut Map<String, Value>, mission: &Mission) {
    payload.insert("mission_slug".into(), json!(mission.slug().as_str()));
    match mission.mission_number() {
        Some(number) => {
            payload.insert("mission_number".into(), json!(number));
        }
        None => {
            payload.insert("mission_number".into(), Value::Null);
        }
    }
    payload.insert("mission_type".into(), json!(mission_type_of(mission)));
}

/// Map a plan failure onto the reference phase/message it corresponds to.
///
/// The plan owner refuses every gate before the first byte is written, so both
/// coverage and dependency-graph refusals belong to the validate phase; the
/// reference spells the coverage refusal with its own message, which is
/// preserved here rather than re-worded.
fn map_plan_error(error: sk_mission::TasksAuthoringError) -> FinalizeFailure {
    use sk_mission::TasksAuthoringError as E;
    match &error {
        E::Coverage { .. } => FinalizeFailure::validate(COVERAGE_GATE_MESSAGE),
        E::DependencyConflict {
            wp,
            existing,
            parsed,
        } => FinalizeFailure::validate(format!(
            "Dependency disagreement detected:\n{wp}: frontmatter has {existing:?}, tasks.md parsed {parsed:?}. Resolve the disagreement in tasks.md or WP frontmatter before finalizing."
        )),
        _ => FinalizeFailure::validate(format!("could not parse tasks.md: {error}")),
    }
}

/// Source `tasks_finalize._ft_validate_occurrence_map_ready`.
///
/// The gate self-conditions on the stored `change_mode`, so a non-bulk-edit
/// Mission is unaffected. It runs FIRST (fail-fast) before the more expensive
/// dependency-graph gates, mirroring the reference's placement.
fn validate_occurrence_map_gate(mission: &Mission) -> Result<(), FinalizeFailure> {
    let change_mode = mission.meta_field("change_mode").and_then(Value::as_str);
    let map_path = mission.spec_dir().join(OCCURRENCE_MAP_FILE_NAME);
    let map = read_occurrence_map(&map_path)?;
    let errors =
        sk_runtime::evaluate_occurrence_gate(change_mode, map.as_ref(), &path_string(&map_path));
    if errors.is_empty() {
        return Ok(());
    }
    Err(FinalizeFailure::validate(OCCURRENCE_GATE_BLOCKED)
        .with_detail("gate_errors", json!(errors)))
}

/// Read and parse `occurrence_map.yaml` (Source `bulk_edit.load_occurrence_map`).
fn read_occurrence_map(path: &Path) -> Result<Option<Value>, FinalizeFailure> {
    let bytes = match sk_kernel::read_bounded(path, OCCURRENCE_MAP_BYTE_LIMIT) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(FinalizeFailure::validate(format!(
                "could not read {}: {error}",
                path.display()
            )));
        }
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        FinalizeFailure::validate(format!(
            "occurrence map {} is not valid UTF-8: {error}",
            path.display()
        ))
    })?;
    match serde_yaml_ng::from_str::<Value>(text) {
        Ok(Value::Null) => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(error) => Err(FinalizeFailure::validate(format!(
            "could not parse {}: {error}",
            path.display()
        ))),
    }
}

/// Source `upgrade.pre30_guard.check_pre30_layout`.
fn check_pre30_layout(mission_dir: &Path) -> Result<(), FinalizeFailure> {
    let tasks_dir = mission_dir.join(sk_mission::TASKS_DIR_NAME);
    if !tasks_dir.is_dir() {
        return Ok(());
    }
    let mut detected: Vec<&str> = Vec::new();
    for lane in LEGACY_LANE_DIRS {
        let lane_dir = tasks_dir.join(lane);
        if !lane_dir.is_dir() {
            continue;
        }
        let has_markdown = std::fs::read_dir(&lane_dir)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
                })
            })
            .unwrap_or(false);
        if has_markdown {
            detected.push(lane);
        }
    }
    if detected.is_empty() {
        return Ok(());
    }
    let lane_hint = detected.first().copied().unwrap_or("tasks/{lane}/");
    Err(FinalizeFailure::resolve(format!(
        "Pre-3.0 layout detected (tasks/{lane_hint}/ directories or frontmatter lane state).\nRun `spec-kitty upgrade` to migrate before continuing."
    )))
}

/// The Source write leg: prompts, then the coordination STATUS_STATE write
/// location, then lanes, then the status bootstrap. Source's
/// `_do_finalize_tasks` reports an apply error without undoing completed writes;
/// `agent mission finalize`'s different recovery policy must not leak into here.
fn apply_finalize_side_effects(
    root: &Path,
    mission: &Mission,
    plan: &DependencyFinalizePlan,
    tasks_dir: &Path,
) -> Result<BootstrapResult, FinalizeFailure> {
    apply_finalize_writes(root, mission, plan, tasks_dir)
}

/// The observable writes of one finalize attempt.
fn apply_finalize_writes(
    root: &Path,
    mission: &Mission,
    plan: &DependencyFinalizePlan,
    tasks_dir: &Path,
) -> Result<BootstrapResult, FinalizeFailure> {
    let receipt = plan
        .apply(tasks_dir)
        .map_err(|error| FinalizeFailure::apply(format!("could not finalize tasks: {error}")))?;

    // `sk_kernel::atomic_write` is the durable prompt writer and does not notify
    // the ambient observer, so each rewritten prompt is published to the ledger
    // at the owner boundary with the EXACT bytes this run computed — the same
    // shape the metadata and lanes owners use. Nothing is read back to guess it.
    let written: BTreeSet<&Path> = receipt.updated().iter().map(PathBuf::as_path).collect();
    for update in plan.updates() {
        if !written.contains(update.path()) {
            continue;
        }
        sk_workspace::notify_published(update.path(), update.updated_bytes()).map_err(|error| {
            FinalizeFailure::apply(format!(
                "the write observer refused {} after it was published: {error}",
                update.path().display()
            ))
        })?;
    }

    // Source `_ft_emit_status_events` (coord-artifact-single-home WP15, B1/B2):
    // establishing the STATUS_STATE write location is a SIDE EFFECT of the
    // non-validate-only leg and runs before the lanes write and the bootstrap.
    // A named write-location refusal (a deleted or remote-only coordination
    // branch, a stray worktree, a seed fork, a held lock) propagates; this leg
    // has no best-effort swallow and never degrades to the root checkout.
    let status_location = status_write_location(root, mission)?;

    finalize_lanes(root, mission, plan.effective_dependencies())?;
    bootstrap_canonical_state(mission, &status_location)
}

/// The Mission directory this run's status bootstrap must write into.
///
/// This is `tasks_finalize._ft_emit_status_events`'s
/// `placement_seam(main_repo_root, slug).write_dir(MissionArtifactKind.STATUS_STATE)`:
/// for a coordination-routed topology it establishes (materializes / seeds /
/// restores) the coordination surface through the canonical workspace owner and
/// returns the coordination Mission directory; every other topology returns the
/// declared primary directory with no side effect at all.
///
/// The PRIMARY directory stays the scan and identity authority (Source B1): the
/// coordination Mission directory carries no `tasks/`, so handing it to the
/// bootstrap would report `total_wps = 0`.
fn status_write_location(
    root: &Path,
    mission: &Mission,
) -> Result<StatusWriteLocation, FinalizeFailure> {
    let Some(topology) = mission.topology() else {
        // Source's coord-less arm: a topology-less (legacy) meta declares PRIMARY.
        return Ok(StatusWriteLocation::primary(
            mission.spec_dir().to_path_buf(),
        ));
    };
    if !sk_mission::routes_through_coordination(topology) {
        return Ok(StatusWriteLocation::primary(
            mission.spec_dir().to_path_buf(),
        ));
    }
    let workspace = open_mission_workspace(root, mission)?;
    let phase = lifecycle_phase_for_finalize(&workspace, mission)?;
    let policy = sk_workspace::ProtectionPolicy::resolve_for_mission(root, mission.slug().as_str())
        .map_err(|error| {
            FinalizeFailure::apply(format!("could not resolve the protected branches: {error}"))
        })?;
    let location = sk_workspace::coord::establish_write_location(
        &workspace,
        sk_workspace::MissionArtifactKind::StatusState,
        phase,
        &policy,
    )
    .map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not establish the status write location for mission {}: {error}",
            mission.slug().as_str()
        ))
    })?;
    // Only a COORD-partition write is published. A PRIMARY establishment (a
    // coordination-less topology, or the published/E2 short-circuit) is the
    // reference's primary-uncommitted arm: the rows are written and nothing is
    // committed.
    if location.surface != sk_workspace::TopologySurface::Coord {
        return Ok(StatusWriteLocation::primary(location.path));
    }
    let repository = sk_git::GitRepository::open(&location.surface_root).map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not open the coordination checkout {} to publish the seeded status rows: {error}",
            location.surface_root.display()
        ))
    })?;
    // The destination is the branch the established checkout is really on — the
    // same fact `establish_write_location` just proved by refusing a worktree on
    // any other branch. `plan_safe_commit` re-checks `HEAD == destination`, so a
    // checkout that moved in between is refused rather than misrouted.
    let branch = repository.current_branch().map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not read the branch of the coordination checkout {}: {error}",
            location.surface_root.display()
        ))
    })?;
    let Some(branch) = branch else {
        return Err(FinalizeFailure::apply(format!(
            "the coordination checkout {} is on a detached HEAD; the seeded status rows cannot be published to a branch",
            location.surface_root.display()
        )));
    };
    let protected = policy.branch_set().map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not resolve the protected set for the status publication: {error}"
        ))
    })?;
    Ok(StatusWriteLocation {
        dir: location.path,
        publish: Some(StatusSeedTarget {
            worktree_root: location.surface_root,
            branch,
            protected,
        }),
    })
}

/// Where a finalize run writes its status seed rows, and where it publishes them.
struct StatusWriteLocation {
    /// The Mission directory the seed rows are read from and written into.
    dir: PathBuf,
    /// The coordination surface each seed is committed on, when this write is
    /// COORD-partition. `None` is the reference's primary-uncommitted arm.
    publish: Option<StatusSeedTarget>,
}

impl StatusWriteLocation {
    /// A PRIMARY-partition write: no commit, exactly like the reference's
    /// coord-less arm (`status_transition._emit_via_non_transactional_fallback`,
    /// row 8).
    fn primary(dir: PathBuf) -> Self {
        Self { dir, publish: None }
    }
}

/// The one publication target a coordination seed commit uses.
///
/// Built only from facts the establishment already proved: the checkout the
/// write landed in, the branch that checkout is on, and the caller-resolved
/// protected set.
struct StatusSeedTarget {
    /// The coordination worktree the commit runs from (Source `worktree_root`).
    worktree_root: PathBuf,
    /// The destination branch, as observed on that checkout.
    branch: String,
    /// The caller-resolved protected branches.
    protected: sk_git::ProtectedBranches,
}

/// Where a READ of the status surface resolves to, with no mutation (Source B2:
/// `--validate-only` must never materialize, seed, lock, or write).
///
/// This is the reference's event-log read contract
/// (`coordination.status_transition._read_contract_from_transaction_target`),
/// delegated to the coordination owner so the coordination worktree, the
/// branch-ref arm and the primary fallback are decided in exactly one place:
///
/// * an attached coordination worktree reads its Mission directory;
/// * no worktree but an existing local coordination branch reads the branch ref
///   (`git show <branch>:kitty-specs/<dir>/status.events.jsonl`), which is the
///   arm the dry run was missing before this task;
/// * a deleted/absent branch reads the declared primary checkout.
fn status_read_surface(
    mission: &Mission,
) -> Result<sk_workspace::coord::CoordStatusRead, FinalizeFailure> {
    let Some(topology) = mission.topology() else {
        return Ok(sk_workspace::coord::CoordStatusRead::Directory(
            mission.spec_dir().to_path_buf(),
        ));
    };
    if !sk_mission::routes_through_coordination(topology) {
        return Ok(sk_workspace::coord::CoordStatusRead::Directory(
            mission.spec_dir().to_path_buf(),
        ));
    }
    let workspace = open_mission_workspace(mission_root(mission), mission)?;
    sk_workspace::coord::read_status_surface(&workspace).map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not resolve the status read surface for mission {}: {error}",
            mission.slug().as_str()
        ))
    })
}

/// The repository root a loaded Mission lives in.
///
/// A Mission's `spec_dir()` is `<root>/kitty-specs/<slug>`, and the workspace
/// owner re-anchors any checkout to the canonical primary root itself, so passing
/// the Mission directory is the CWD-invariant answer.
fn mission_root(mission: &Mission) -> &Path {
    mission
        .spec_dir()
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| mission.spec_dir())
}

/// Load the mission through the canonical workspace owner.
fn open_mission_workspace(
    root: &Path,
    mission: &Mission,
) -> Result<sk_workspace::MissionWorkspace, FinalizeFailure> {
    sk_workspace::ProjectWorkspace::open(root)
        .and_then(|workspace| workspace.mission(mission.slug().as_str()))
        .map_err(|error| {
            FinalizeFailure::apply(format!(
                "could not load mission {} as a workspace: {error}",
                mission.slug().as_str()
            ))
        })
}

/// Source `mission_runtime.lifecycle_phase.resolve_lifecycle_phase`, composed
/// from the workspace owner's canonical evidence helpers.
fn lifecycle_phase_for_finalize(
    workspace: &sk_workspace::MissionWorkspace,
    mission: &Mission,
) -> Result<sk_workspace::LifecyclePhase, FinalizeFailure> {
    let target_ref = workspace
        .target_branch()
        .ok()
        .and_then(|branch| sk_git::BranchName::parse(&branch).ok())
        .map(|branch| sk_workspace::probe_target_ref(workspace.workspace().repository(), &branch))
        .unwrap_or(sk_workspace::Probe::Absent);
    let work_packages: Vec<(String, String)> = super::support::canonical_lanes(mission)
        .map_err(|error| {
            FinalizeFailure::apply(format!(
                "could not read the status surface: {}",
                error.message()
            ))
        })?
        .into_iter()
        .collect();
    let evidence = sk_workspace::LifecycleEvidence {
        baseline_merge_commit: sk_workspace::baseline_merge_commit_from_mission(mission),
        target_ref,
        terminal_completion: sk_workspace::terminal_completion(
            mission.mission_number(),
            &work_packages,
        ),
    };
    sk_workspace::derive_lifecycle_phase(&evidence).map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not derive the mission lifecycle phase: {error}"
        ))
    })
}

/// Source `lanes.compute_and_persist.compute_and_write_lanes`'s abort message.
const LANE_GLOB_ABORT: &str = "Lane computation aborted: literal-path owned_files entries match zero files. Fix the paths before lanes.json is written.";

/// Source `tasks_finalize._finalize_lanes` (#4758): co-locate the `lanes.json`
/// planning write with the event-log bootstrap.
fn finalize_lanes(
    root: &Path,
    mission: &Mission,
    dependencies: &BTreeMap<WpId, Vec<WpId>>,
) -> Result<(), FinalizeFailure> {
    let mission_dir = mission.spec_dir();
    let store = sk_workspace::LanesStore::new(mission_dir);
    let existing = store
        .read()
        .map_err(|error| FinalizeFailure::apply(format!("could not read lanes.json: {error}")))?;
    if existing.is_some() {
        // #3311 guard: never rewrite existing lanes.
        return Ok(());
    }

    let began = execution_has_begun(mission);
    if is_execution_wedged(began, false) {
        let slug = mission.slug().as_str();
        return Err(FinalizeFailure::apply(format!(
            "Cannot finalize mission {slug:?}: execution has begun (a WP is past 'planned') but no lanes.json exists on disk. Refusing to seed further canonical events without lanes.json. Run 'spec-kitty doctor mission-state --fix --mission {slug}' to rebuild lanes.json from the event log."
        )));
    }

    let ownership = gather_wp_ownership(mission)?;
    if ownership.plans.is_empty() {
        // No WP declares ownership — nothing meaningful to lane-compute.
        return Ok(());
    }

    // Source re-validates `owned_files` against the repository inside the same
    // core that writes `lanes.json`, BEFORE any lane is computed or published:
    // a literal path matching nothing is a refusal, and the refusal payload
    // carries the per-entry diagnostics. Running it here (after the
    // never-rewrite and wedge guards, before `compute_lanes`) keeps Source's
    // precedence exactly.
    let glob_result = sk_workspace::lanes::validate_glob_matches(
        &ownership.plans,
        root,
        &ownership.create_intent,
    );
    if !glob_result.passed() {
        return Err(FinalizeFailure::apply(LANE_GLOB_ABORT)
            .with_detail("ownership_literal_path_errors", json!(glob_result.errors)));
    }

    let target_branch = {
        let current = super::support::current_branch(root).unwrap_or_default();
        let resolved = target_branch_of(mission, &current);
        if resolved.is_empty() {
            current
        } else {
            resolved
        }
    };
    let topology = mission.topology().unwrap_or(MissionTopology::SingleBranch);
    let mut input = sk_workspace::LanePlannerInput::new(
        mission.slug().clone(),
        topology,
        target_branch.clone(),
    );
    input.mission_id = Some(mission.id().clone());
    match mission.meta_field("mission_branch").and_then(Value::as_str) {
        Some(branch) if !branch.trim().is_empty() => {
            input.mission_branch = Some(branch.trim().to_owned());
        }
        _ => {}
    }
    for (wp, plan) in ownership.plans {
        input = input.with_work_package(plan);
        let deps = dependencies.get(&wp).cloned().unwrap_or_default();
        input = input.with_dependencies(wp, deps);
    }
    input.bodies = ownership.bodies;

    let manifest = sk_workspace::compute_lanes(&input)
        .map_err(|error| FinalizeFailure::apply(format!("could not compute lanes: {error}")))?;
    // The planning commit is CAPTURED from the real branch tip, never predicted.
    let manifest = match capture_branch_tip(root, &target_branch) {
        Some(tip) => manifest.with_planning_commit(&tip),
        None => manifest,
    };
    store
        .write(&manifest)
        .map_err(|error| FinalizeFailure::apply(format!("could not write lanes.json: {error}")))?;
    Ok(())
}

/// The ownership plans, prompt bodies and declared `create_intent` entries one
/// lane computation needs.
struct WpOwnership {
    plans: BTreeMap<WpId, sk_workspace::WpPlan>,
    bodies: BTreeMap<WpId, String>,
    /// WP frontmatter `create_intent`: literal paths the work package intends to
    /// create, which suppress the zero-match refusal for them.
    create_intent: BTreeMap<WpId, Vec<String>>,
}

/// Source `_gather_wp_frontmatter_and_bodies` + `resolve_wp_manifests`.
///
/// A work package that declares no `execution_mode` or no `owned_files` is not
/// an ownership manifest (Source `build_wp_manifests`); an unreadable prompt is
/// skipped rather than blocking finalize, exactly as the reference degrades here.
fn gather_wp_ownership(mission: &Mission) -> Result<WpOwnership, FinalizeFailure> {
    let tasks_dir = mission.spec_dir().join(sk_mission::TASKS_DIR_NAME);
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&tasks_dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("WP") && name.ends_with(".md"))
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(FinalizeFailure::apply(format!(
                "could not read {}: {error}",
                tasks_dir.display()
            )));
        }
    };
    paths.sort();

    let mut plans = BTreeMap::new();
    let mut bodies = BTreeMap::new();
    let mut create_intent = BTreeMap::new();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(package) = WorkPackage::parse(&path, &text) else {
            continue;
        };
        let string_list = |key: &str| -> Vec<String> {
            package
                .frontmatter_field(key)
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let owned = string_list("owned_files");
        let execution_mode = package
            .frontmatter_field("execution_mode")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if execution_mode.is_empty() || owned.is_empty() {
            continue;
        }
        let wp_id = package.id().clone();
        let intended = string_list("create_intent");
        if !intended.is_empty() {
            create_intent.insert(wp_id.clone(), intended);
        }
        let plan = if execution_mode == "planning_artifact" {
            sk_workspace::WpPlan::planning(wp_id.clone(), owned)
        } else {
            sk_workspace::WpPlan::code(wp_id.clone(), owned)
        };
        plans.insert(wp_id.clone(), plan);
        if let Ok(frontmatter) = sk_mission::split_frontmatter(&text) {
            bodies.insert(wp_id, frontmatter.body.to_owned());
        }
    }
    Ok(WpOwnership {
        plans,
        bodies,
        create_intent,
    })
}

/// Source `mission_finalize_planning_pin.capture_branch_tip`: best-effort.
fn capture_branch_tip(root: &Path, branch: &str) -> Option<sk_git::Oid> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    let repository = sk_git::GitRepository::open(root).ok()?;
    let name = sk_git::BranchName::parse(branch).ok()?;
    repository.read_branch_tip(&name).ok().flatten()
}

/// Source `mission_finalize_planning_pin._execution_has_begun`: read-only, and a
/// signal helper never turns an unreadable surface into a hard failure.
fn execution_has_begun(mission: &Mission) -> bool {
    let lanes = super::support::canonical_lanes(mission).unwrap_or_default();
    lanes
        .values()
        .any(|lane| lane != "planned" && lane != "genesis")
}

/// Source `lanes.persistence.is_execution_wedged` (#4758).
fn is_execution_wedged(execution_has_begun: bool, lanes_present: bool) -> bool {
    execution_has_begun && !lanes_present
}

/// Source `status.bootstrap.bootstrap_canonical_state`.
///
/// Seeds a `genesis -> planned` event for every `tasks/WP*.md` that has no
/// canonical transition yet, through the canonical `sk-status` transition writer
/// (its own lock, validation, verified whole-file replacement and
/// materialization), then reports the reference's counts.
///
/// On a COORD-partition write each seed is **published**: one path-scoped commit
/// of that seed's status artifacts on the coordination branch, exactly like the
/// reference's transactional writer (`BookkeepingTransaction` on the transaction
/// path, `_commit_status_artifacts_to_coord` on the fallback path). The Mission
/// lock is held across emit → commit → rollback, as `coord_status_lock` does, so
/// this operation's own appended tail is the only thing it can ever cut. A
/// PRIMARY-partition write keeps the reference's uncommitted arm.
///
/// `mission` is the PRIMARY identity: the work-package scan always reads
/// `<spec_dir>/tasks/`, because a coordination Mission directory carries no
/// `tasks/` (Source cycle-2 B1). `location.dir` is the surface the event log and
/// snapshot are read from and written to — the directory this run established
/// (the coordination Mission directory for a coord topology, the primary
/// directory otherwise). Source's transactional writer resolves that split
/// internally; here it is explicit.
fn bootstrap_canonical_state(
    mission: &Mission,
    location: &StatusWriteLocation,
) -> Result<BootstrapResult, FinalizeFailure> {
    let mut result = BootstrapResult::default();
    let wp_ids = collect_bootstrap_wp_ids(mission, &mut result)?;
    if wp_ids.is_empty() {
        return Ok(result);
    }
    result.total_wps = wp_ids.len();

    // The write leg reads the same surface it writes: the directory this run
    // established. For a coordination Mission that is the coordination log, so
    // a re-run does not reseed rows that already landed there.
    let initialized = read_initialized_wp_ids(
        mission,
        &sk_workspace::coord::CoordStatusRead::Directory(location.dir.clone()),
    )?;
    for wp_id in classify_wp_ids(wp_ids, &initialized, &mut result) {
        match &location.publish {
            Some(target) => seed_and_publish(mission, &location.dir, target, &wp_id)?,
            None => emit_seed(mission, &location.dir, &wp_id)?,
        }
        result.newly_seeded += 1;
        result
            .wp_details
            .insert(wp_id, BOOTSTRAP_INITIALIZED.to_owned());
    }
    Ok(result)
}

/// Source `status.bootstrap.bootstrap_canonical_state(dry_run=True)`.
///
/// Reads whichever status surface the reference's read contract resolves — the
/// attached coordination worktree, the coordination branch ref when no worktree
/// is attached, or the primary checkout — and reports what a real run would
/// seed. It materializes nothing, takes no lock, writes nothing, and still scans
/// `tasks/` from PRIMARY (a coordination Mission directory carries no `tasks/`).
fn preview_canonical_state(
    mission: &Mission,
    surface: &sk_workspace::coord::CoordStatusRead,
) -> Result<BootstrapResult, FinalizeFailure> {
    let mut result = BootstrapResult::default();
    let wp_ids = collect_bootstrap_wp_ids(mission, &mut result)?;
    if wp_ids.is_empty() {
        return Ok(result);
    }
    result.total_wps = wp_ids.len();

    let initialized = read_initialized_wp_ids(mission, surface)?;
    for wp_id in classify_wp_ids(wp_ids, &initialized, &mut result) {
        result
            .wp_details
            .insert(wp_id, BOOTSTRAP_WOULD_SEED.to_owned());
    }
    result.newly_seeded = result
        .wp_details
        .values()
        .filter(|detail| detail.as_str() == BOOTSTRAP_WOULD_SEED)
        .count();
    Ok(result)
}

/// Source `status.bootstrap._collect_wp_ids`.
///
/// Every `tasks/WP*.md` in file order; a malformed prompt is skipped and
/// recorded, never a hard failure (the reference logs a warning and carries on),
/// and a duplicate id is kept in file order exactly like the reference.
fn collect_bootstrap_wp_ids(
    mission: &Mission,
    result: &mut BootstrapResult,
) -> Result<Vec<String>, FinalizeFailure> {
    let tasks_dir = mission.spec_dir().join(sk_mission::TASKS_DIR_NAME);
    if !tasks_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&tasks_dir) {
        Ok(entries) => entries.flatten().map(|entry| entry.path()).collect(),
        Err(error) => {
            return Err(FinalizeFailure::apply(format!(
                "could not read {}: {error}",
                tasks_dir.display()
            )));
        }
    };
    paths.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("WP") && name.ends_with(".md"))
    });
    paths.sort();
    let mut wp_ids: Vec<String> = Vec::new();
    for path in paths {
        match WorkPackage::from_file(&path) {
            Ok(package) => wp_ids.push(package.id().as_str().to_owned()),
            Err(_) => {
                result.skipped += 1;
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_owned();
                result
                    .wp_details
                    .insert(stem, BOOTSTRAP_SKIPPED_MALFORMED.to_owned());
            }
        }
    }
    Ok(wp_ids)
}

/// Source `status.bootstrap._classify_wp_ids`: split seeded from unseeded.
fn classify_wp_ids(
    wp_ids: Vec<String>,
    initialized: &BTreeSet<String>,
    result: &mut BootstrapResult,
) -> Vec<String> {
    let mut to_seed: Vec<String> = Vec::new();
    for wp_id in wp_ids {
        if initialized.contains(&wp_id) {
            result.already_initialized += 1;
            result
                .wp_details
                .insert(wp_id, BOOTSTRAP_ALREADY_EXISTS.to_owned());
        } else {
            to_seed.push(wp_id);
        }
    }
    to_seed
}

/// The canonical seed request for one work package.
fn seed_request(mission: &Mission, status_dir: &Path, wp_id: &str) -> sk_status::TransitionRequest {
    let mut request = sk_status::TransitionRequest::new(
        status_dir,
        mission.slug().as_str(),
        wp_id,
        "planned",
        sk_status::mutation_emit::ActorValue::Structured {
            role: Some("system".to_owned()),
            profile: None,
            tool: Some("finalize-tasks".to_owned()),
            model: None,
        },
    );
    request.mission_id = Some(mission.id().as_str().to_owned());
    request.reason = Some(BOOTSTRAP_REASON.to_owned());
    request.repo_root = Some(mission_root(mission).to_path_buf());
    request
}

/// The reference's primary-uncommitted arm: write the seed, commit nothing.
fn emit_seed(mission: &Mission, status_dir: &Path, wp_id: &str) -> Result<(), FinalizeFailure> {
    sk_status::emit_status_transition(&seed_request(mission, status_dir, wp_id))
        .map(|_| ())
        .map_err(|error| {
            FinalizeFailure::apply(format!(
                "could not seed canonical status for {wp_id}: {error}"
            ))
        })
}

/// The reference's COORD-partition arm: one seed, one commit.
///
/// Order is load-bearing and mirrors `_emit_on_coord_then_commit`: take the
/// Mission lock (L1) → capture the rollback point → emit → commit → release.
/// Only a provably unlanded commit lets this operation cut its own tail back;
/// the rollback authority re-checks the multiset, a torn tail and the ids
/// already committed at HEAD, and leaves every byte alone on any refusal.
fn seed_and_publish(
    mission: &Mission,
    status_dir: &Path,
    target: &StatusSeedTarget,
    wp_id: &str,
) -> Result<(), FinalizeFailure> {
    let lock = sk_status::mutation::acquire_mission_lock(
        status_dir,
        Some(mission_root(mission)),
        sk_kernel::LockWait::Bounded(sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT),
    )
    .map_err(|error| {
        FinalizeFailure::apply(format!(
            "could not take the Mission write lock for {wp_id}: {error}"
        ))
    })?;
    let point =
        sk_status::mutation::capture_rollback_point(&lock, status_dir).map_err(|error| {
            FinalizeFailure::apply(format!(
                "could not capture the rollback point for {wp_id}: {error}"
            ))
        })?;

    let outcome = sk_status::emit_status_transition(&seed_request(mission, status_dir, wp_id))
        .map_err(|error| {
            FinalizeFailure::apply(format!(
                "could not seed canonical status for {wp_id}: {error}"
            ))
        })?;

    if let Err(reason) = publish_status_seed(target, status_dir) {
        let expected = vec![outcome.event.event_id.clone()];
        let probe = super::support::StatusHeadCommittedProbe::open(&target.worktree_root).ok();
        let probe_ref: Option<&dyn sk_status::HeadCommittedProbe> = probe
            .as_ref()
            .map(|probe| probe as &dyn sk_status::HeadCommittedProbe);
        // The reference appends its rollback outcome to the propagating error
        // (`append_refusal_to_error`), so the note rides the message instead of
        // becoming an extra envelope key.
        let rollback_note = match sk_status::mutation::rollback_status_artifacts(
            &lock,
            &point,
            Some(&expected),
            probe_ref,
        ) {
            Ok(rollback) if rollback.rolled_back => " The seeded row was rolled back.".to_owned(),
            Ok(rollback) => format!(" {}", rollback.message()),
            Err(error) => format!(
                " STATUS_ROLLBACK_REFUSED: the rollback could not run ({error}); inspect the status files."
            ),
        };
        return Err(FinalizeFailure::apply(format!(
            "could not publish the seeded status row for {wp_id}: {reason}.{rollback_note}"
        )));
    }
    Ok(())
}

/// Publish one seed's status artifacts with the canonical Git commit authority.
///
/// Source `coordination.status_transition._commit_status_artifacts_to_coord`: the
/// existing `status.events.jsonl` and `status.json` in the coordination Mission
/// directory, one path-scoped commit on the coordination branch, message
/// `chore(spec-kitty): status transition <dir>`
/// (`sk_status::mutation_commit::status_commit_message`), under the
/// caller-resolved protected set and the `safe_commit` substrate. Nothing is
/// invented: the identity, author/committer times, parent and tree are Git's, and
/// a refusal is a refusal.
fn publish_status_seed(target: &StatusSeedTarget, status_dir: &Path) -> Result<bool, String> {
    let artifacts: Vec<PathBuf> = [sk_status::EVENTS_FILENAME, sk_status::SNAPSHOT_FILENAME]
        .into_iter()
        .map(|name| status_dir.join(name))
        .filter(|path| path.exists())
        .collect();
    if artifacts.is_empty() {
        // The reference returns without committing when no status artifact is on
        // disk; there is nothing to publish.
        return Ok(false);
    }
    let mut git_paths: Vec<sk_git::GitPath> = Vec::with_capacity(artifacts.len());
    for path in &artifacts {
        git_paths.push(
            sk_git::normalize_safe_commit_path(&target.worktree_root, path).map_err(|error| {
                format!(
                    "the status artifact {} could not be addressed for the commit ({error})",
                    path.display()
                )
            })?,
        );
    }
    let destination = sk_git::BranchName::parse(&target.branch).map_err(|error| {
        format!(
            "the coordination branch {:?} is not a valid destination ({error})",
            target.branch
        )
    })?;
    let dir_name = status_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "the coordination Mission directory {} is not valid UTF-8, so its commit message cannot be spelled exactly",
                status_dir.display()
            )
        })?;
    let message = sk_status::mutation_commit::status_commit_message(dir_name);
    let request = sk_git::SafeCommitRequest::new(destination, message, git_paths).with_protection(
        target.protected.clone(),
        sk_git::CommitGuardCapability::Standard,
    );
    let repository = sk_git::GitRepository::open(&target.worktree_root).map_err(|error| {
        format!(
            "the coordination checkout {} could not be opened ({error})",
            target.worktree_root.display()
        )
    })?;
    // Every failure below carries the truth the adapter gave: a refused plan
    // provably created nothing, and an apply failure may or may not have landed —
    // the rollback authority, never this function, decides what may be cut.
    let plan = repository
        .plan_safe_commit(&request)
        .map_err(|error| format!("its commit was refused before it could land ({error})"))?;
    repository
        .apply_safe_commit(&plan)
        .map_err(|error| format!("its commit did not land ({error})"))?;
    Ok(true)
}

/// The work packages the canonical event log already knows about.
///
/// Source reads the log through `read_events_transactional` →
/// `read_event_log` → `read_events_from_text`, whose transitions-only view is
/// what `initialized_wp_ids` is built from (off-axis annotation rows are
/// partitioned out). The directory arm is the canonical bounded reader; the
/// branch-ref arm reads the exact committed bytes at the coordination branch
/// with no worktree, lock or write.
fn read_initialized_wp_ids(
    mission: &Mission,
    surface: &sk_workspace::coord::CoordStatusRead,
) -> Result<BTreeSet<String>, FinalizeFailure> {
    let rows = match surface {
        sk_workspace::coord::CoordStatusRead::Directory(dir) => {
            let path = dir.join(sk_status::EVENTS_FILENAME);
            sk_status::read_raw_events(&path).map_err(|error| {
                FinalizeFailure::apply(format!("could not read {}: {error}", path.display()))
            })?
        }
        sk_workspace::coord::CoordStatusRead::BranchRef { branch, .. } => {
            let workspace = open_mission_workspace(mission_root(mission), mission)?;
            let bytes = sk_workspace::coord::read_status_events_at_branch_ref(&workspace)
                .map_err(|error| {
                    FinalizeFailure::apply(format!(
                        "could not read the status event log at the coordination branch {branch:?}: {error}"
                    ))
                })?;
            match bytes {
                None => Vec::new(),
                Some(bytes) => {
                    let text = std::str::from_utf8(&bytes).map_err(|error| {
                        FinalizeFailure::apply(format!(
                            "the status event log at the coordination branch {branch:?} is not valid UTF-8: {error}"
                        ))
                    })?;
                    // Source parses the branch-ref text with the feature dir as
                    // the diagnostic path (`parser_feature_dir`), so a malformed
                    // row is attributed to the Mission, not to a ref spelling.
                    let diagnostic = mission.spec_dir().join(sk_status::EVENTS_FILENAME);
                    sk_status::mutation_log::parse_log_rows(&diagnostic, text).map_err(|error| {
                        FinalizeFailure::apply(format!(
                            "could not parse the status event log at the coordination branch {branch:?}: {error}"
                        ))
                    })?
                }
            }
        }
    };
    initialized_ids_from_rows(&rows)
}

/// The initialized ids from already-parsed log rows.
fn initialized_ids_from_rows(rows: &[Value]) -> Result<BTreeSet<String>, FinalizeFailure> {
    if rows.is_empty() {
        return Ok(BTreeSet::new());
    }
    let stream = sk_status::replay::parse_diary(rows).map_err(|error| {
        FinalizeFailure::apply(format!("could not parse the status event log: {error}"))
    })?;
    Ok(stream
        .transitions
        .iter()
        .map(|event| event.wp_id.clone())
        .collect())
}

/// `agent mission branch-context`: the deterministic branch contract.
///
/// Read-only. It never creates, switches, or writes a branch.
pub fn branch_context(
    root: &Path,
    target_branch: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let repository = sk_git::GitRepository::open(root).map_err(|_| {
        CompatError::domain("Not in a git repository. Branch context requires git.")
    })?;
    let current = repository
        .current_branch()
        .map_err(|error| CompatError::domain(format!("could not read HEAD: {error}")))?
        .filter(|branch| branch != "HEAD")
        .ok_or_else(|| {
            CompatError::domain(
                "Must be on a branch to resolve branch context (detached HEAD detected).",
            )
        })?;

    let primary = if current == "main" || current == "master" {
        current.clone()
    } else if repository
        .read_branch_tip(
            &sk_git::BranchName::parse("main")
                .map_err(|error| CompatError::domain(format!("invalid branch name: {error}")))?,
        )
        .ok()
        .flatten()
        .is_some()
    {
        "main".to_owned()
    } else if repository
        .read_branch_tip(
            &sk_git::BranchName::parse("master")
                .map_err(|error| CompatError::domain(format!("invalid branch name: {error}")))?,
        )
        .ok()
        .flatten()
        .is_some()
    {
        "master".to_owned()
    } else {
        current.clone()
    };

    let target = target_branch
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&current)
        .to_owned();
    let mut payload = Map::new();
    payload.insert("result".into(), json!("success"));
    payload.insert("repo_root".into(), json!(path_string(repository.root())));
    payload.insert(
        "target_branch_source".into(),
        json!(if target_branch.is_some() {
            "cli_arg"
        } else {
            "current_branch"
        }),
    );
    payload.insert(
        "next_step".into(),
        json!("Use this deterministic branch contract during specify/plan prompts; do not rediscover branch state inside the LLM."),
    );
    inject_branch_contract(&mut payload, &target, &current, &target);
    let current_is_primary = current == primary;
    payload.insert("primary_branch".into(), json!(primary));
    payload.insert("PRIMARY_BRANCH".into(), json!(primary));
    payload.insert("current_is_primary".into(), json!(current_is_primary));
    payload.insert("CURRENT_IS_PRIMARY".into(), json!(current_is_primary));
    let (strategy, reason) = if current_is_primary {
        (
            "feature-branch",
            format!(
                "You are on the primary branch '{primary}'. PR-bound missions should start on a dedicated topic branch so planning artifacts are not committed to '{primary}'."
            ),
        )
    } else {
        (
            "stay",
            format!(
                "You are on '{current}', which is not the primary branch '{primary}'; staying on it is fine."
            ),
        )
    };
    payload.insert("recommended_strategy".into(), json!(strategy));
    payload.insert("RECOMMENDED_STRATEGY".into(), json!(strategy));
    payload.insert("branch_recommendation_reason".into(), json!(reason));
    if let Some(branch_context) = payload
        .get_mut("branch_context")
        .and_then(Value::as_object_mut)
    {
        branch_context.insert("primary_branch".into(), json!(primary));
        branch_context.insert("current_is_primary".into(), json!(current_is_primary));
        branch_context.insert("recommended_strategy".into(), json!(strategy));
        branch_context.insert("reason".into(), json!(reason));
    }
    if let Some(runtime_vars) = payload
        .get_mut("runtime_vars")
        .and_then(Value::as_object_mut)
    {
        runtime_vars.insert("primary_branch".into(), json!(primary));
        runtime_vars.insert("current_is_primary".into(), json!(current_is_primary));
        runtime_vars.insert("recommended_strategy".into(), json!(strategy));
    }
    let value = Value::Object(payload);
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("Current branch: {current}");
        println!("Planning/base branch: {target}");
        println!("Merge target: {target}");
        println!("Primary branch: {primary}");
        println!("Recommended strategy: {strategy}");
        println!("{reason}");
    }
    Ok(value)
}

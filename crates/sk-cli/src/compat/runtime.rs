//! Persistent governed-run routes over `sk-run-store`.
//!
//! `next` is the upstream-shaped loop: query, advance, answer. The `runtime`
//! group exposes the substrate contracts directly (a native extension, not an
//! upstream route) so operators and tests can inspect durable run state.
//!
//! # What `next` composes
//!
//! The upstream command is not a bare DAG projection. It combines four
//! authorities:
//!
//! 1. the **finalized work-package board** (`tasks/` + the canonical status
//!    log) which overrides a stale early DAG phase with `implement`/`review`/
//!    `accept`/`done`/a named blocked sentinel;
//! 2. the **step guards** for the currently issued step (software-dev artifact
//!    presence, analysis-report freshness, work-package advance readiness),
//!    which re-issue the step with `guard_failures` instead of advancing;
//! 3. the **runtime planner** over the frozen template (step /
//!    decision-required / blocked / terminal);
//! 4. the **durable run store** (cursor, journal, prompt artifacts, answers).
//!
//! Query mode never writes: with no run it plans from the live template and an
//! empty cursor in memory, and it reports `run_id: null`. Advancing mode starts
//! or resumes the run, answers any pending decision as the human operator, and
//! advances under the run cursor lock.
//!
//! 5. the **composed step contract** (`StepContractExecutor` over the real
//!    pack contracts), which for a composition-eligible action executes every
//!    contract step through `ProfileInvocationExecutor` — one started and one
//!    completed Op record each, best-effort committed — then runs the
//!    post-action guard and commits the run's advance through the canonical
//!    `plan_advance` → `commit_advance`, never the legacy `advance`.
//!
//! # What it does not compose (explicit gaps)
//!
//! The decision-log Git commit, the retrospective completion gate and
//! coordination-topology workspaces are **not** implemented here. Only the
//! software-dev composed-guard family is ported; every other mission family
//! takes the reference's unregistered-family neutral (empty) guard result.
//! Custom `contract_ref` bindings resolve through the mission/action contract
//! rather than the referenced id. `capabilities` records the same state; this
//! module never reports a success it did not perform.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use std::str::FromStr;

use serde_json::{Value, json};
use sk_run_store::{
    ActorIdentity, ActorType, AdvanceRequest, AnswerRequest, NextDecisionExt, RunRef, RunState,
    RunStore, StartRequest, StepResult,
};
use sk_runtime::{
    AnalysisVerdict, BoardFacts, BoardStep, CliDecisionKind, DecisionEnvelope, NextDecision,
    Origin, WpProgress, finalized_board_step, plan_next, state_alias, wp_progress,
};

use super::invocation;
use super::support::{
    CompatError, emit_json, emit_json_pretty, path_string, project_root, resolve_mission,
};

/// The runtime steps that iterate work packages rather than mission phases.
const WP_ITERATION_STEPS: [&str; 2] = ["implement", "review"];

/// The mission family whose CLI guard vocabulary this facade implements.
const SOFTWARE_DEV: &str = "software-dev";

/// Upstream guard messages, reproduced byte-for-byte where the gate is
/// implemented natively.
const MISSING_ARTIFACT_MESSAGE: &str = "Required artifact missing: ";
const MISSING_TASK_FILES_MESSAGE: &str = "Required: at least one tasks/WP*.md file";
const WP_IMPLEMENT_GUARD_MESSAGE: &str =
    "Not all work packages have required status (for_review, approved, or done)";
const WP_REVIEW_GUARD_MESSAGE: &str = "Not all work packages are approved or done";
/// Upstream's canonical runtime tool registry (the compact governance block's
/// `Tools:` baseline when the project does not declare `available_tools`).
const DEFAULT_TOOL_REGISTRY: [&str; 2] = ["git", "spec-kitty"];

/// The reference's WP-review anti-pattern checklist
/// (`specify_cli.review.antipattern_checklist`).
const REVIEW_ANTIPATTERN_CHECKLIST: &str =
    "## Anti-pattern checklist (WP-level cheap version of mission-review)

For each item below, state PASS / FAIL / N/A in your verdict. A FAIL on any
item blocks approval.

1. **Dead code**: every new public function, class, or module created by this
   WP has at least one live caller from production code, excluding tests. For
   each new module, run targeted import/call-site greps, for example
   `grep -r -e \"from <new_module> import\" -e \"import <new_module>\" src/ --include=\"*.<ext>\"`.
   Zero production hits means dead code.

2. **Synthetic-fixture test**: every test marked as covering an FR listed in
   this WP's frontmatter actually invokes the production code path that would
   produce the asserted shape. A test that constructs a literal dict matching
   the assertion is a synthetic fixture. Ask: if I delete the implementation
   code, does this test still pass? If yes, the FR is untested.

3. **Silent empty return**: search every new code path for `except ...:
   return \"\"`, `return None`, `return []`, `return {}`, or `pass`. Each hit
   must have a documented reason; absent that, it is a silent failure
   candidate.

4. **FR coverage**: every functional requirement in `requirement_refs`
   (`FR-###`, including a letter-suffixed `FR-###a`) has at least one test
   assertion that references the behavior it names, not just a comment or
   frontmatter entry. `SC-###` refs are tracked, not gated, and a
   `<mission-slug>#<ID>` ref cites another mission, so neither needs a test
   here.

5. **Frozen surface**: no commit in this WP modifies a file the spec,
   contract, or WP prompt marks as frozen or untouchable. For each frozen file,
   `git log --oneline <base>..HEAD -- <frozen-file>` must be empty.

6. **Locked decision**: no new code path contradicts a `MUST NOT` clause in
   `spec.md`, `plan.md`, or `contracts/`. Grep the diff for forbidden patterns
   named by those clauses.

7. **Shared-file ownership**: any file modified by this WP that is also
   modified by another WP, visible in `lanes.json`, shared lane metadata, or
   the same mission merge, has an explicit coordination note in the move-task
   reason or review feedback.

8. **Production fragility**: any new `raise` in a production code path has a
   documented fail-loud rationale. A bare `raise` in a request handler, worker,
   or CLI path that can fire on a transient race is a fragility risk.";

fn map_store_error(error: sk_run_store::RunStoreError) -> CompatError {
    let info = error.info();
    CompatError::domain(info.message).with("details", json!({ "code": info.code }))
}

fn map_query_error(error: sk_run_store::RunStoreError) -> CompatError {
    let info = error.info();
    CompatError::coded("QUERY_MODE_VALIDATION_FAILED", info.message)
        .with("details", json!({ "code": info.code }))
}

fn parse_result(value: &str) -> Result<StepResult, CompatError> {
    match value.trim() {
        "success" => Ok(StepResult::Success),
        "failed" => Ok(StepResult::Failed),
        "blocked" => Ok(StepResult::Blocked),
        other => Err(CompatError::usage(format!(
            "invalid --result {other:?}: expected success, failed, or blocked"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Pack, mission-type and prompt-offering resolution
// ---------------------------------------------------------------------------

/// The canonical packs root: the **parent** of `built-in`.
///
/// One spelling is resolved once and shared by runtime-template resolution and
/// the doctrine context, which is what removes the two-conventions defect. The
/// documented value is `SPEC_KITTY_PACKS_ROOT=<packs>`; a value that names the
/// `built-in` tree itself is normalized to its parent rather than silently
/// meaning something else. Without the environment override the installed tree
/// is discovered from the running executable and the project root — never from
/// a compile-time checkout path, which an installed binary does not have.
///
/// The org required floor is resolved by `sk_doctrine::required_floor`, whose
/// own pack search reads the documented parent value; the leaf normalization
/// here serves this adapter's template and governance-context reads.
pub(crate) fn packs_root(repo_root: &Path) -> Option<PathBuf> {
    if let Some(value) = env::var_os("SPEC_KITTY_PACKS_ROOT") {
        let value = PathBuf::from(value);
        let parent = if value.file_name() == Some(OsStr::new("built-in")) {
            value.parent().map(Path::to_path_buf)?
        } else {
            value
        };
        return parent
            .join("built-in")
            .join("missions")
            .is_dir()
            .then_some(parent);
    }
    let mut anchors: Vec<PathBuf> = vec![repo_root.to_path_buf()];
    if let Ok(executable) = env::current_exe() {
        anchors.push(executable);
    }
    for anchor in anchors {
        for ancestor in anchor.ancestors() {
            let candidate = ancestor.join("packs").join("built-in");
            if candidate.join("missions").is_dir() {
                return Some(ancestor.join("packs"));
            }
        }
    }
    None
}

/// The built-in pack root (`<packs>/built-in`), when it is present.
fn built_in_pack_root(repo_root: &Path) -> Option<PathBuf> {
    packs_root(repo_root).map(|root| root.join("built-in"))
}

/// The project's activated mission types, in declared order.
///
/// This is the reference's `PackContext.activated_mission_types` gate: the
/// provisioned `mission_type_activations` key, with no implicit "all built-ins"
/// backfill when it is absent.
fn activated_mission_types(repo_root: &Path) -> Vec<String> {
    let path = repo_root.join(".kittify").join("config.yaml");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_yaml_ng::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value
        .get("mission_type_activations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The project + org overlay roots used by the mission-type/prompt authority.
fn offering_roots(repo_root: &Path) -> (Vec<PathBuf>, Option<PathBuf>) {
    let (_, org_roots, project) = sk_doctrine::context_roots(repo_root)
        .unwrap_or_else(|_| (PathBuf::new(), Vec::new(), None));
    (org_roots, project)
}

/// Whether a registered mission type has a loadable mission-type definition.
fn mission_type_definition_exists(repo_root: &Path, mission_type: &str) -> bool {
    let mut candidates =
        vec![root_mission_types_dir(repo_root).join(format!("{mission_type}.yaml"))];
    let (org_roots, _) = offering_roots(repo_root);
    candidates.extend(org_roots.iter().map(|root| {
        root.join("mission_types")
            .join(format!("{mission_type}.yaml"))
    }));
    if let Some(pack) = built_in_pack_root(repo_root) {
        candidates.push(
            pack.join("missions")
                .join("mission_types")
                .join(format!("{mission_type}.yaml")),
        );
    }
    candidates.iter().any(|path| path.is_file())
}

fn root_mission_types_dir(repo_root: &Path) -> PathBuf {
    repo_root
        .join(".kittify")
        .join("missions")
        .join("mission_types")
}

/// The reference's `UnknownMissionTypeError` message for an activated type with
/// no loadable definition anywhere in the layered roster.
fn unknown_mission_type_error(repo_root: &Path, mission_type: &str) -> Option<String> {
    if mission_type.is_empty() {
        return None;
    }
    let activated = activated_mission_types(repo_root);
    if !activated.iter().any(|item| item == mission_type) {
        return None;
    }
    if mission_type_definition_exists(repo_root, mission_type) {
        return None;
    }
    let others: Vec<&str> = activated
        .iter()
        .map(String::as_str)
        .filter(|item| *item != mission_type)
        .collect();
    let mut message =
        format!("Mission type {mission_type:?} is activated but has no loadable profile.");
    if !others.is_empty() {
        message.push_str(&format!(
            " Other activated mission types: {}.",
            others.join(", ")
        ));
    }
    Some(format!("UnknownMissionTypeError: {message}"))
}

/// One mission-config tier (`origin.mission_tier` + the directory it names).
fn resolve_mission_config(repo_root: &Path, mission_type: &str) -> Option<(PathBuf, &'static str)> {
    let mut candidates: Vec<(PathBuf, &'static str)> = vec![
        (
            repo_root
                .join(".kittify")
                .join("overrides")
                .join("missions")
                .join(mission_type)
                .join("mission.yaml"),
            "override",
        ),
        (
            repo_root
                .join(".kittify")
                .join("missions")
                .join(mission_type)
                .join("mission.yaml"),
            "legacy",
        ),
    ];
    let (org_roots, _) = offering_roots(repo_root);
    candidates.extend(org_roots.iter().map(|root| {
        (
            root.join("missions")
                .join(mission_type)
                .join("mission.yaml"),
            "org",
        )
    }));
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        candidates.push((
            home.join(".kittify")
                .join("missions")
                .join(mission_type)
                .join("mission.yaml"),
            "global_mission",
        ));
    }
    if let Some(pack) = built_in_pack_root(repo_root) {
        candidates.push((
            pack.join("missions")
                .join(mission_type)
                .join("mission.yaml"),
            "package_default",
        ));
    }
    candidates
        .into_iter()
        .find(|(path, _)| path.is_file())
        .map(|(path, tier)| {
            let directory = path
                .parent()
                .and_then(|parent| parent.canonicalize().ok())
                .unwrap_or_else(|| {
                    path.parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| path.clone())
                });
            (directory, tier)
        })
}

/// The canonical command-template path for a mission offering, or `None`.
///
/// Mirrors the reference's six-tier `resolve_command` chain and its
/// mission-step prompt layout. `name` is a filename such as `review.md`.
fn resolve_command_template(repo_root: &Path, mission_type: &str, name: &str) -> Option<PathBuf> {
    let stem = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_owned());
    let file = format!("{stem}.md");
    let mut candidates: Vec<PathBuf> = vec![
        repo_root
            .join(".kittify")
            .join("overrides")
            .join("command-templates")
            .join(&file),
        repo_root
            .join(".kittify")
            .join("command-templates")
            .join(&file),
    ];
    let (org_roots, _) = offering_roots(repo_root);
    candidates.extend(org_roots.iter().map(|root| {
        root.join("missions")
            .join(mission_type)
            .join("command-templates")
            .join(&file)
    }));
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        candidates.push(
            home.join(".kittify")
                .join("missions")
                .join(mission_type)
                .join("command-templates")
                .join(&file),
        );
        candidates.push(home.join(".kittify").join("command-templates").join(&file));
    }
    if let Some(pack) = built_in_pack_root(repo_root) {
        candidates.push(
            pack.join("missions")
                .join(mission_type)
                .join("command-templates")
                .join(&file),
        );
        candidates.push(
            pack.join("missions")
                .join("mission-steps")
                .join(mission_type)
                .join(&stem)
                .join("prompt.md"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// A resolved runtime template and the tier that supplied it.
#[derive(Debug, Clone)]
pub struct ResolvedRuntimeTemplate {
    /// The template file.
    pub path: PathBuf,
    /// `override`, `legacy`, or `package`.
    pub tier: &'static str,
}

/// Resolve the canonical runtime template for a mission type, with its tier.
///
/// Precedence is the documented chain: project override, project mission tree,
/// then the pinned pack tree. Nothing is written and no template is invented.
pub fn resolve_runtime_template(
    root: &Path,
    mission_type: &str,
) -> Result<ResolvedRuntimeTemplate, CompatError> {
    if mission_type.trim().is_empty() {
        return Err(CompatError::coded(
            "TEMPLATE_NOT_FOUND",
            "the mission records no mission_type, so no runtime template can be resolved",
        ));
    }
    let mut candidates: Vec<(PathBuf, &'static str)> = vec![
        (
            root.join(".kittify")
                .join("overrides")
                .join("missions")
                .join(mission_type)
                .join("mission-runtime.yaml"),
            "override",
        ),
        (
            root.join(".kittify")
                .join("missions")
                .join(mission_type)
                .join("mission-runtime.yaml"),
            "legacy",
        ),
    ];
    if let Some(pack) = built_in_pack_root(root) {
        candidates.push((
            pack.join("missions")
                .join(mission_type)
                .join("mission-runtime.yaml"),
            "package",
        ));
    }
    for (path, tier) in candidates {
        if path.is_file() {
            return Ok(ResolvedRuntimeTemplate { path, tier });
        }
    }
    Err(CompatError::coded(
        "TEMPLATE_NOT_FOUND",
        format!(
            "could not resolve mission-runtime.yaml for mission type '{mission_type}'; the built-in pack must be present"
        ),
    ))
}

/// The canonical runtime template path for a mission type.
pub fn runtime_template(root: &Path, mission_type: &str) -> Result<PathBuf, CompatError> {
    resolve_runtime_template(root, mission_type).map(|template| template.path)
}

/// The mission's recorded type, or the neutral empty string.
///
/// Upstream stopped defaulting a typeless mission to `software-dev`; a neutral
/// value fails template resolution loudly instead of silently changing the
/// mission family.
fn mission_type_of(mission: &sk_mission::Mission) -> String {
    mission
        .mission_type()
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

/// The display mission number: recorded number, else the slug's numeric prefix.
fn mission_number_of(mission: &sk_mission::Mission) -> String {
    if let Some(display) = mission.mission_number_display() {
        return display;
    }
    let slug = mission.slug().as_str();
    let prefix: String = slug.chars().take_while(char::is_ascii_digit).collect();
    prefix
}

/// The repository root that owns `mission`, CWD-invariantly when possible.
fn repo_root_for(root: &Path, mission: &sk_mission::Mission) -> PathBuf {
    sk_workspace::ProjectWorkspace::open(mission.spec_dir())
        .map(|workspace| workspace.primary_root().to_path_buf())
        .unwrap_or_else(|_| root.to_path_buf())
}

fn open_store(root: &Path) -> Result<RunStore, CompatError> {
    RunStore::open(root).map_err(map_store_error)
}

fn existing_run(
    root: &Path,
    mission: &sk_mission::Mission,
) -> Result<(RunStore, RunRef), CompatError> {
    let store = open_store(root)?;
    let run = store
        .run_for_mission(mission.slug().as_str(), Some(mission.id().as_str()))
        .map_err(map_store_error)?
        .ok_or_else(|| {
            CompatError::coded(
                "RUN_NOT_FOUND",
                "no durable run exists for this mission; `runtime start` creates one (query mode never does)",
            )
        })?;
    Ok((store, run))
}

/// Start or resolve the mission's durable run.
pub fn ensure_run(
    root: &Path,
    mission: &sk_mission::Mission,
) -> Result<(RunStore, RunRef), CompatError> {
    let store = open_store(root)?;
    if let Some(run) = store
        .run_for_mission(mission.slug().as_str(), Some(mission.id().as_str()))
        .map_err(map_store_error)?
    {
        return Ok((store, run));
    }
    let template = resolve_runtime_template(root, &mission_type_of(mission))?;
    let request = StartRequest {
        mission_slug: mission.slug().as_str().to_owned(),
        mission_id: Some(mission.id().as_str().to_owned()),
        template_path: template.path,
        inputs: Default::default(),
        policy_snapshot: Default::default(),
    };
    let outcome = store.start(&request).map_err(map_store_error)?;
    Ok((store, outcome.run))
}

// ---------------------------------------------------------------------------
// Work-package board facts
// ---------------------------------------------------------------------------

/// The finalized work-package board, read once per decision.
#[derive(Debug, Clone)]
struct BoardView {
    /// Number of `WP*.md` files.
    facts: BoardFacts,
    /// Reduced lane per work package (absent log ⇒ `genesis`).
    lanes: BTreeMap<String, String>,
    /// Reduced snapshot slots per work package, for operator provenance.
    provenance: BTreeMap<String, bool>,
    /// Lane-weighted progress.
    progress: WpProgress,
    /// Parsed work-package files, in id order.
    packages: Vec<sk_mission::WorkPackage>,
    /// The canonical task-read error, when a status work package lost its file.
    task_error: Option<String>,
    /// Whether the mission carries a `tasks.md` artifact.
    ///
    /// The reference's finalized-board override requires BOTH `tasks.md` and
    /// `tasks/`; a pre-finalize board must never override the DAG.
    tasks_md_present: bool,
    /// Whether the canonical status event log exists at all.
    has_status_log: bool,
}

impl BoardView {
    /// Whether this board may override the DAG (the reference's gate).
    fn can_override(&self) -> bool {
        self.tasks_md_present && self.task_error.is_none()
    }

    fn lane_of(&self, wp_id: &str) -> &str {
        self.lanes
            .get(wp_id)
            .map(String::as_str)
            .unwrap_or("genesis")
    }

    fn has_operator_provenance(&self, wp_id: &str) -> bool {
        self.provenance.get(wp_id).copied().unwrap_or(false)
    }
}

/// The canonical task-read error for any work package present in status state.
///
/// Messages match upstream `prompt_builder.read_wp_task`; canceled work
/// packages are exempt because canceling is the recorded way to retire a task
/// file on purpose.
fn wp_task_surface_error(
    tasks_dir: &Path,
    lanes: &BTreeMap<String, String>,
    mission_slug: &str,
) -> Option<String> {
    for (wp_id, lane) in lanes {
        if lane == "canceled" {
            continue;
        }
        let expected = tasks_dir.join(format!("{wp_id}.md"));
        let recovery = format!(
            "Restore or regenerate the primary task for {wp_id} at {} (or a titled {wp_id} file under {}), then rerun `spec-kitty next --mission {mission_slug}`. If {wp_id} was removed on purpose, move it to canceled instead: `spec-kitty agent tasks move-task {wp_id} --to canceled --mission {mission_slug}`.",
            expected.display(),
            tasks_dir.display()
        );
        if !tasks_dir.is_dir() {
            return Some(format!(
                "Canonical WORK_PACKAGE_TASK directory for {wp_id} is missing at {}; expected {}. {recovery}",
                tasks_dir.display(),
                expected.display()
            ));
        }
        let found = fs::read_dir(tasks_dir).ok().is_some_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let Some(stem) = name.strip_suffix(".md") else {
                    return false;
                };
                name.starts_with("WP") && (stem == wp_id || stem.starts_with(&format!("{wp_id}-")))
            })
        });
        if !found {
            return Some(format!(
                "Canonical WORK_PACKAGE_TASK file for {wp_id} is missing at {}. {recovery}",
                expected.display()
            ));
        }
    }
    None
}

/// Read the board for a mission without writing anything.
///
/// Returns `None` when the mission has no finalized task board (no `tasks/`
/// directory or no work-package files), which is the upstream
/// "no override yet" signal.
fn gather_board(mission: &sk_mission::Mission) -> Result<Option<BoardView>, CompatError> {
    let tasks_dir = super::support::tasks_dir(mission);
    if !tasks_dir.is_dir() {
        return Ok(None);
    }
    let packages = super::support::work_packages(mission)?;
    if packages.is_empty() {
        return Ok(None);
    }
    let lanes = super::support::canonical_lanes(mission)?;

    // Operator-authored cancellation provenance, from the reduced snapshot.
    let mut provenance = BTreeMap::new();
    let events = super::support::events_path(mission);
    if let Ok(rows) = sk_status::read_raw_events(&events)
        && !rows.is_empty()
        && let Ok(snapshot) = sk_status::reduce_rows(&rows)
    {
        for (wp_id, state) in &snapshot.work_packages {
            let operator = state.get("reason_source").and_then(Value::as_str) == Some("operator");
            provenance.insert(wp_id.clone(), operator);
        }
    }

    let state_map: sk_mission::WpStateMap = packages
        .iter()
        .filter_map(|package| {
            let lane = lanes
                .get(package.id().as_str())
                .map(String::as_str)
                .unwrap_or("genesis");
            sk_mission::WpState::ALL
                .iter()
                .find(|state| state.as_str() == lane)
                .map(|state| (package.id().clone(), *state))
        })
        .collect();

    let mut facts = BoardFacts {
        total_wps: packages.len() as u64,
        ..BoardFacts::default()
    };
    for package in &packages {
        let wp_id = package.id().as_str();
        let lane = lanes.get(wp_id).map(String::as_str).unwrap_or("genesis");
        if lane == "planned" {
            let readiness = sk_mission::readiness_for_dependencies(
                package.id(),
                package.dependencies(),
                &state_map,
            );
            if readiness.satisfied() {
                facts.claimable_planned = true;
            }
        }
        let acceptable = matches!(lane, "approved" | "done")
            || (lane == "canceled" && provenance.get(wp_id).copied().unwrap_or(false));
        if lane == "done" {
            facts.done_endings += 1;
        }
        if acceptable {
            facts.acceptable_endings += 1;
        }
        match lane {
            "claimed" if facts.first_claimed.is_none() => {
                facts.first_claimed = Some(wp_id.to_string());
            }
            "in_progress" if facts.first_in_progress.is_none() => {
                facts.first_in_progress = Some(wp_id.to_string());
            }
            "for_review" if facts.first_for_review.is_none() => {
                facts.first_for_review = Some(wp_id.to_string());
            }
            "in_review" if facts.first_in_review.is_none() => {
                facts.first_in_review = Some(wp_id.to_string());
            }
            _ => {}
        }
    }

    // Bucket counts come from the WP files (a work package with no status event
    // counts as `genesis`), but the weighted percentage comes from the reduced
    // status snapshot only — exactly the split `_compute_wp_progress` makes.
    let file_lanes: Vec<&str> = packages
        .iter()
        .map(|package| {
            lanes
                .get(package.id().as_str())
                .map(String::as_str)
                .unwrap_or("genesis")
        })
        .collect();
    let mut progress = wp_progress(file_lanes);
    progress.weighted_percentage =
        sk_runtime::weighted_percentage(lanes.values().map(String::as_str));
    let task_error = wp_task_surface_error(&tasks_dir, &lanes, mission.slug().as_str());

    Ok(Some(BoardView {
        facts,
        lanes,
        provenance,
        progress,
        packages,
        task_error,
        tasks_md_present: mission.spec_dir().join("tasks.md").is_file(),
        has_status_log: super::support::events_path(mission).exists(),
    }))
}

/// Whether the work-package board has finished the given iteration step.
///
/// Mirrors upstream `_should_advance_wp_step` / `_wp_blocks_step`.
fn board_advances_step(board: &BoardView, step: &str) -> bool {
    board.packages.iter().all(|package| {
        let wp_id = package.id().as_str();
        let lane = board.lane_of(wp_id);
        // An acceptable ending never blocks: `approved`/`done`
        // unconditionally, `canceled` only with operator provenance.
        if matches!(lane, "approved" | "done")
            || (lane == "canceled" && board.has_operator_provenance(wp_id))
        {
            return true;
        }
        match step {
            // Advance past implement only once every work package has been
            // handed off (`for_review`); pending, blocked and in-review work
            // still holds the run in the step.
            "implement" => lane == "for_review",
            "review" => false,
            _ => true,
        }
    })
}

/// The reference's fail-loud "canonical status missing" error, when it applies.
///
/// Upstream's WP-advance guard (`_should_advance_wp_step` → `wp_ending`) raises
/// when the mission has a `tasks/` board but no committed status log, and names
/// an unresolved dependency cycle as the root cause when one exists. The board
/// selector's *query* path intentionally treats an absent log as "all genesis";
/// only the WP-iteration advance path fails loud.
fn canonical_status_missing_error(ctx: &DecisionContext<'_>) -> Option<String> {
    let board = ctx.board.as_ref()?;
    if board.has_status_log {
        return None;
    }
    if !super::support::tasks_dir(ctx.mission).is_dir() || board.packages.is_empty() {
        return None;
    }
    let slug = ctx.mission.slug().as_str();
    match dependency_cycle_root_cause(board) {
        Some(cause) => Some(format!(
            "Canonical status not found for mission '{slug}': {cause}"
        )),
        None => Some(format!(
            "Canonical status not found for mission '{slug}'. Run 'spec-kitty agent mission finalize-tasks --mission {slug}' to bootstrap the event log."
        )),
    }
}

/// The reference's cycle-aware root-cause sentence, when the board has a cycle.
fn dependency_cycle_root_cause(board: &BoardView) -> Option<String> {
    let error = sk_mission::WorkPackageGraph::build(board.packages.clone()).err()?;
    match error {
        sk_mission::MissionError::DependencyCycle { cycle } => Some(format!(
            "`finalize-tasks` cannot initialize canonical status while a circular WP dependency exists: {cycle}. Resolve the dependency cycle in tasks.md / WP frontmatter, then re-run finalize-tasks."
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// A guard-diagnostic path, relative to the repository when it is contained.
///
/// Upstream reports the path a guard actually read as a repository-relative
/// token; an out-of-repo path stays absolute so the diagnostic is still
/// actionable.
fn guard_path(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path_string(path))
}

fn artifact_present(mission: &sk_mission::Mission, name: &str) -> bool {
    mission.spec_dir().join(name).is_file()
}

fn missing_artifact(name: &str) -> String {
    format!("{MISSING_ARTIFACT_MESSAGE}{name}")
}

/// The live analysis-report freshness verdict for a mission.
fn analysis_verdict(
    repo_root: &Path,
    mission: &sk_mission::Mission,
) -> (AnalysisVerdict, BTreeMap<String, String>) {
    let spec_dir = mission.spec_dir();
    let mut live: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut tags = BTreeMap::new();
    for name in ["spec.md", "plan.md", "tasks.md"] {
        let path = spec_dir.join(name);
        let hash = if name == "tasks.md" {
            fs::read_to_string(&path).ok().map(|text| {
                sk_kernel::content_digest::sha256_hex(
                    sk_runtime::normalize_tasks_md(&text).as_bytes(),
                )
            })
        } else {
            fs::read(&path)
                .ok()
                .map(|bytes| sk_kernel::content_digest::sha256_hex(&bytes))
        };
        if hash.is_some() {
            tags.insert(name.to_string(), path_string(&path));
        }
        live.insert(name.to_string(), hash);
    }
    let charter = [
        ".kittify/charter/charter.yaml",
        ".kittify/charter/charter.md",
    ]
    .iter()
    .map(|relative| repo_root.join(relative))
    .find(|path| path.is_file());
    let charter_hash = charter
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| sk_kernel::content_digest::sha256_hex(&bytes));
    if let Some(path) = &charter
        && charter_hash.is_some()
    {
        tags.insert("charter".to_string(), path_string(path));
    }
    live.insert("charter".to_string(), charter_hash);

    let report_path = spec_dir.join(sk_runtime::ANALYSIS_REPORT_FILENAME);
    let report = fs::read_to_string(&report_path).ok();
    if report.is_some() {
        tags.insert(
            sk_runtime::ANALYSIS_REPORT_FILENAME.to_string(),
            path_string(&report_path),
        );
    }
    (
        sk_runtime::check_analysis_report(report.as_deref(), &live),
        tags,
    )
}

/// The analysis-report gate the finalized-board override applies before it
/// hands out `implement` (upstream B3).
///
/// Returns the guard failures and the typed code only when the mission's
/// runtime template declares an `analyze` step and the recorded report is not
/// current. A mission that does not declare `analyze` never gets this gate.
fn analysis_gate(ctx: &DecisionContext<'_>) -> Option<(Vec<String>, &'static str)> {
    let template = resolve_runtime_template(ctx.root, &ctx.mission_type).ok()?;
    let template = sk_runtime::load_template(&template.path).ok()?;
    if !template.steps.iter().any(|step| step.id == "analyze") {
        return None;
    }
    let (verdict, _tags) = analysis_verdict(&ctx.repo_root, ctx.mission);
    if verdict.is_current() {
        return None;
    }
    Some((
        verdict.guard_failures(),
        verdict
            .error_code()
            .unwrap_or(sk_runtime::ANALYSIS_CURRENCY_UNAVAILABLE),
    ))
}

// ---------------------------------------------------------------------------
// Requirement-mapping and occurrence gates (the `tasks` guard)
// ---------------------------------------------------------------------------

/// The reference's `_check_bare_prose_requirements_ready` guard failures.
///
/// Read first and unconditionally by the `tasks` guard, before any
/// tasks-directory readiness short-circuit.
fn bare_prose_requirement_failures(mission: &sk_mission::Mission) -> Vec<String> {
    let path = mission.spec_dir().join("spec.md");
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let candidates = sk_runtime::find_bare_prose_requirement_ids(&content);
    sk_runtime::evaluate_bare_prose_requirements(&candidates)
}

/// Read `wps.yaml` (the WP manifest) and its raw requirement refs.
///
/// `Ok(None)` means the manifest is legitimately absent (the legacy
/// `tasks.md` fallback applies). A present-but-corrupt manifest is an
/// actionable failure string, never a silent empty result.
fn wps_manifest_refs(spec_dir: &Path) -> Result<Option<BTreeMap<String, Vec<String>>>, String> {
    let path = spec_dir.join("wps.yaml");
    if !path.exists() {
        return Ok(None);
    }
    let read = fs::read_to_string(&path).map_err(|error| {
        format!("Requirement mapping preflight failed: wps.yaml is corrupt: {error}")
    })?;
    let parsed: Value = serde_yaml_ng::from_str(&read).map_err(|error| {
        format!("Requirement mapping preflight failed: wps.yaml is corrupt: {error}")
    })?;
    let Some(object) = parsed.as_object() else {
        return Err(
            "Requirement mapping preflight failed: wps.yaml is corrupt: top level must be a mapping"
                .to_string(),
        );
    };
    let Some(entries) = object.get("work_packages").and_then(Value::as_array) else {
        return Err(
            "Requirement mapping preflight failed: wps.yaml is corrupt: 'work_packages' must be a list"
                .to_string(),
        );
    };
    let mut refs = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            return Err(format!(
                "Requirement mapping preflight failed: wps.yaml is corrupt: work_packages[{index}] must be a mapping"
            ));
        };
        let Some(wp_id) = entry.get("id").and_then(Value::as_str) else {
            return Err(format!(
                "Requirement mapping preflight failed: wps.yaml is corrupt: work_packages[{index}] missing 'id'"
            ));
        };
        let tokens = entry
            .get("requirement_refs")
            .map(sk_runtime::tokenize_refs)
            .unwrap_or_default();
        refs.insert(wp_id.to_string(), tokens);
    }
    Ok(Some(refs))
}

/// The reference's `_check_requirement_mapping_ready` guard failures.
fn requirement_mapping_failures(
    mission: &sk_mission::Mission,
    board: Option<&BoardView>,
    mission_type: &str,
) -> Vec<String> {
    let spec_dir = mission.spec_dir();
    if !spec_dir.join("spec.md").exists() {
        return Vec::new();
    }
    if !spec_dir.join("tasks").is_dir() {
        return Vec::new();
    }
    let Ok(spec_content) = fs::read_to_string(spec_dir.join("spec.md")) else {
        return vec!["Requirement mapping preflight failed: spec.md could not be read".to_string()];
    };
    let parsed = sk_runtime::parse_requirement_ids_from_spec_md(&spec_content, mission_type);

    let manifest = match wps_manifest_refs(spec_dir) {
        Ok(manifest) => manifest,
        Err(failure) => return vec![failure],
    };

    let mut wp_requirement_refs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(board) = board {
        for package in &board.packages {
            let tokens = package
                .frontmatter_field("requirement_refs")
                .map(sk_runtime::tokenize_refs)
                .unwrap_or_default();
            wp_requirement_refs.insert(package.id().as_str().to_string(), tokens);
        }
    }

    if manifest.is_none()
        && let Ok(tasks_md) = fs::read_to_string(spec_dir.join("tasks.md"))
    {
        let fallback = sk_runtime::parse_requirement_refs_from_tasks_md(&tasks_md, mission_type);
        for (wp_id, refs) in fallback {
            if !refs.is_empty() && wp_requirement_refs.get(&wp_id).is_none_or(Vec::is_empty) {
                wp_requirement_refs.insert(wp_id, refs);
            }
        }
    }

    let wp_ids: Vec<String> = board
        .map(|board| {
            board
                .packages
                .iter()
                .map(|package| package.id().as_str().to_string())
                .collect()
        })
        .unwrap_or_default();
    let facts = sk_runtime::RequirementMappingFacts {
        spec_requirement_ids: parsed.all.iter().cloned().collect(),
        functional_requirement_ids: parsed.functional.iter().cloned().collect(),
        wp_ids,
        wp_requirement_refs,
        feature_dir_name: mission.slug().as_str().to_string(),
        mission_type: mission_type.to_string(),
    };
    sk_runtime::evaluate_requirement_mapping(&facts)
}

/// The reference's `_occurrence_gate_failures` guard failures.
fn occurrence_gate_failures(mission: &sk_mission::Mission) -> Vec<String> {
    let change_mode = mission.meta_field("change_mode").and_then(Value::as_str);
    if change_mode != Some("bulk_edit") {
        return Vec::new();
    }
    let path = mission.spec_dir().join("occurrence_map.yaml");
    let map = match fs::read_to_string(&path) {
        Ok(text) => match serde_yaml_ng::from_str::<Value>(&text) {
            Ok(value) if value.is_null() => None,
            Ok(value) => Some(value),
            Err(error) => {
                return vec![format!(
                    "Occurrence map could not be read: {}: {error}",
                    path_string(&path)
                )];
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return vec![format!(
                "Occurrence map could not be read: {}: {error}",
                path_string(&path)
            )];
        }
    };
    sk_runtime::evaluate_occurrence_gate(change_mode, map.as_ref(), &path_string(&path))
}

/// The software-dev CLI guards for one issued step.
///
/// Returns `(failures, guard_failure_paths)`. Only the artifact-presence and
/// work-package-advance gates are implemented natively; the two governance
/// gates this facade does not implement fail closed with a named refusal so a
/// run can never silently advance past them.
fn software_dev_guards(
    repo_root: &Path,
    mission: &sk_mission::Mission,
    board: Option<&BoardView>,
    mission_type: &str,
    step: &str,
) -> (Vec<String>, BTreeMap<String, String>) {
    let mut failures = Vec::new();
    let mut paths = BTreeMap::new();
    let spec_dir = mission.spec_dir();
    match step {
        "specify" => {
            if !artifact_present(mission, "spec.md") {
                failures.push(missing_artifact("spec.md"));
                paths.insert(
                    "spec.md".to_string(),
                    guard_path(repo_root, &spec_dir.join("spec.md")),
                );
            }
        }
        "plan" => {
            if !artifact_present(mission, "plan.md") {
                failures.push(missing_artifact("plan.md"));
                paths.insert(
                    "plan.md".to_string(),
                    guard_path(repo_root, &spec_dir.join("plan.md")),
                );
            }
        }
        "tasks" => {
            // Upstream order: bare-prose requirement fact first (it must fire
            // even when no WP file exists), then artifact presence, then the
            // tasks-directory readiness short-circuit, then the mapping and
            // dependency gates, then the occurrence gate.
            failures.extend(bare_prose_requirement_failures(mission));
            if !artifact_present(mission, "tasks.md") {
                failures.push(missing_artifact("tasks.md"));
                paths.insert(
                    "tasks.md".to_string(),
                    guard_path(repo_root, &spec_dir.join("tasks.md")),
                );
            }
            let tasks_dir = spec_dir.join("tasks");
            if !tasks_dir.is_dir() {
                failures.push("Required: tasks/ directory with finalized WP files".to_string());
                paths.insert("tasks".to_string(), guard_path(repo_root, &tasks_dir));
            } else if board.is_none() {
                failures.push(MISSING_TASK_FILES_MESSAGE.to_string());
                paths.insert("tasks".to_string(), guard_path(repo_root, &tasks_dir));
            } else {
                failures.extend(requirement_mapping_failures(mission, board, mission_type));
                if let Some(board) = board
                    && let Some(package) = board
                        .packages
                        .iter()
                        .find(|package| !package.frontmatter().contains_key("dependencies"))
                {
                    let stem = package
                        .source()
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| package.id().as_str().to_string());
                    failures.push(format!(
                        "WP {stem} missing 'dependencies' in frontmatter (run 'spec-kitty agent mission finalize-tasks')"
                    ));
                    paths.insert(
                        package.id().as_str().to_string(),
                        guard_path(repo_root, package.source()),
                    );
                }
            }
            failures.extend(occurrence_gate_failures(mission));
        }
        "analyze" => {
            let (verdict, tags) = analysis_verdict(repo_root, mission);
            if !verdict.is_current() {
                failures.extend(verdict.guard_failures());
                paths.extend(tags);
            }
        }
        "implement" | "review" => {
            let complete = board.is_some_and(|board| board_advances_step(board, step));
            if !complete {
                failures.push(if step == "implement" {
                    WP_IMPLEMENT_GUARD_MESSAGE.to_string()
                } else {
                    WP_REVIEW_GUARD_MESSAGE.to_string()
                });
                if let Some(board) = board {
                    let tasks_dir = super::support::tasks_dir(mission);
                    for package in &board.packages {
                        paths.insert(
                            package.id().as_str().to_string(),
                            guard_path(repo_root, package.source()),
                        );
                    }
                    paths.insert("tasks".to_string(), path_string(&tasks_dir));
                }
            }
        }
        _ => {}
    }
    (failures, paths)
}

// ---------------------------------------------------------------------------
// Work-package workspace + prompt authoring
// ---------------------------------------------------------------------------

/// A work package's real execution workspace.
#[derive(Debug, Clone)]
struct WpWorkspace {
    /// The lane worktree, or the repository root for a planning artifact.
    path: PathBuf,
    /// The lane id, when one is recorded and it is a code lane.
    lane_id: Option<String>,
    /// Work packages sharing the lane.
    lane_wp_ids: Vec<String>,
    /// The lane's base branch, when recorded.
    base_branch: Option<String>,
    /// The mission's target branch (the review diff fallback).
    target_branch: String,
}

/// Resolve the real lane worktree for a work package.
///
/// A planning-artifact work package runs at the repository root; a code lane
/// resolves through `lanes.json`. A mission without a lane manifest falls back
/// to the repository root (the upstream "no lane allocated yet" answer).
fn wp_workspace(repo_root: &Path, mission: &sk_mission::Mission, wp_id: &str) -> WpWorkspace {
    let target_branch = mission
        .meta_field("target_branch")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_string();
    let mut workspace = WpWorkspace {
        path: repo_root.to_path_buf(),
        lane_id: None,
        lane_wp_ids: Vec::new(),
        base_branch: None,
        target_branch,
    };
    let Ok(wp) = sk_kernel::WpId::from_str(wp_id) else {
        return workspace;
    };
    let manifest = sk_workspace::LanesStore::new(mission.spec_dir())
        .read()
        .ok()
        .flatten();
    let Some(manifest) = manifest else {
        return workspace;
    };
    let Some(lane) = manifest.lane_for_wp(&wp) else {
        return workspace;
    };
    if lane.is_planning() {
        return workspace;
    }
    workspace.path =
        sk_workspace::worktree_path(repo_root, manifest.mission_slug.as_str(), &lane.lane_id)
            .unwrap_or_else(|_| repo_root.to_path_buf());
    workspace.lane_id = Some(lane.lane_id.clone());
    workspace.lane_wp_ids = lane
        .wp_ids
        .iter()
        .map(|id| id.as_str().to_string())
        .collect();
    workspace.base_branch = Some(manifest.target_branch.clone());
    workspace.target_branch = manifest.target_branch.clone();
    workspace
}

fn workspace_for_wp(repo_root: &Path, mission: &sk_mission::Mission, wp_id: &str) -> PathBuf {
    wp_workspace(repo_root, mission, wp_id).path
}

/// The full authored work-package text, including frontmatter and whitespace.
fn work_package_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

// ---------------------------------------------------------------------------
// Governance context
// ---------------------------------------------------------------------------

/// The mission-type governance profile (mission-offering data).
fn mission_type_profile(repo_root: &Path, mission_type: &str) -> Option<Value> {
    let mut candidates = vec![
        repo_root
            .join(".kittify")
            .join("charter-packs")
            .join("mission_types")
            .join(mission_type)
            .join("governance-profile.yaml"),
    ];
    if let Some(pack) = built_in_pack_root(repo_root) {
        candidates.push(
            pack.join("missions")
                .join(mission_type)
                .join("governance-profile.yaml"),
        );
    }
    let path = candidates.into_iter().find(|path| path.is_file())?;
    let text = fs::read_to_string(path).ok()?;
    serde_yaml_ng::from_str::<Value>(&text).ok()
}

fn profile_list(profile: Option<&Value>, key: &str) -> Vec<String> {
    profile
        .and_then(|profile| profile.get(key))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn profile_scalar(profile: Option<&Value>, key: &str) -> Option<String> {
    profile
        .and_then(|profile| profile.get(key))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The mission-type governance block spliced into a WP prompt.
fn mission_type_governance_block(
    repo_root: &Path,
    mission_type: &str,
    profile: Option<&Value>,
) -> String {
    let rendered = |values: Vec<String>| {
        if values.is_empty() {
            "(none)".to_string()
        } else {
            values.join(", ")
        }
    };
    let mut lines = vec![format!("Mission-Type Governance Profile: {mission_type}")];
    for key in [
        "selected_directives",
        "selected_tactics",
        "selected_paradigms",
        "selected_styleguides",
        "selected_toolguides",
        "selected_procedures",
        "selected_agent_profiles",
        "selected_mission_step_contracts",
    ] {
        let values = profile_list(profile, key);
        let label = &key["selected_".len()..];
        lines.push(format!("  {label}: {}", rendered(values)));
    }
    // `template_set` is rendered after the selections by the reference? It
    // renders `template_set` first; keep that order.
    let template_set = profile_scalar(profile, "template_set").unwrap_or_default();
    lines.insert(1, format!("  template_set: {template_set}"));
    let _ = repo_root;
    lines.join("\n")
}

/// The gathered governance facts for one action.
pub(crate) struct GovernanceFacts {
    pub(crate) context: sk_doctrine::CharterContext,
    pub(crate) paradigms: Vec<String>,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) template_set: String,
    pub(crate) tools: Vec<String>,
}

/// The advisory governance block for an action, or the reason it is absent.
///
/// Reads the pack graph and the activation ledger through `sk-doctrine`. A
/// missing or malformed pack is reported in the prompt text instead of being
/// swallowed: the agent sees that governance context was unavailable.
fn governance_block(
    repo_root: &Path,
    mission_type: &str,
    action: &str,
    profile_id: Option<&str>,
) -> String {
    match action_context(repo_root, mission_type, action, profile_id) {
        Ok(facts) => render_governance(&facts),
        Err(reason) => format!("(_governance context unavailable: {reason}_)"),
    }
}

/// Render the compact governance block the reference prompt carries.
pub(crate) fn render_governance(facts: &GovernanceFacts) -> String {
    let rendered = |values: &[String]| {
        if values.is_empty() {
            "(none)".to_string()
        } else {
            values.join(", ")
        }
    };
    let mut lines = vec![
        "Governance:".to_string(),
        format!("  - Template set: {}", facts.template_set),
        format!("  - Paradigms: {}", rendered(&facts.paradigms)),
        format!("  - Tools: {}", rendered(&facts.tools)),
    ];
    let mut append = |heading: &str, values: &[String], always: bool| {
        if always || !values.is_empty() {
            lines.push(format!("{heading}:"));
            if values.is_empty() {
                lines.push("  - (none)".to_string());
            } else {
                lines.extend(values.iter().map(|value| format!("  - {value}")));
            }
        }
    };
    let directive_ids: Vec<String> = facts
        .context
        .all_directives
        .iter()
        .map(|directive| directive.id.clone())
        .collect();
    append("Directive IDs", &directive_ids, true);
    let tactic_ids: Vec<String> = facts
        .context
        .tactics
        .iter()
        .map(|tactic| tactic.id.clone())
        .collect();
    append("Tactic IDs", &tactic_ids, true);
    append(
        "Styleguide IDs",
        &entry_ids(&facts.context.styleguides),
        false,
    );
    append(
        "Toolguide IDs",
        &entry_ids(&facts.context.toolguides),
        false,
    );
    append(
        "Procedure IDs",
        &entry_ids(&facts.context.procedures),
        false,
    );
    let profile_links: Vec<String> = facts
        .context
        .profile_links
        .iter()
        .map(|link| format!("{} -> {}", link.profile, link.urn))
        .collect();
    append("Profile Links", &profile_links, false);
    append("Section Anchors", &[], true);
    if !facts.diagnostics.is_empty() {
        lines.push(format!(
            "  - Diagnostics: {}",
            facts.diagnostics.join(" | ")
        ));
    }
    lines.join("\n")
}

fn entry_ids(entries: &[sk_doctrine::ArtifactEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id.clone()).collect()
}

// ---------------------------------------------------------------------------
// Active project languages (the profile `applies_to_languages` filter)
// ---------------------------------------------------------------------------
//
// The signal itself is the canonical `sk_doctrine` language-scope service: the
// doctrine-derived vocabulary, the configured -> compiled -> interview tiers
// and the no-signal / explicit-empty / declared projection all live there. This
// adapter only binds that service to the CLI's repository and the already
// resolved pack root, and it propagates a real service failure instead of
// inventing a no-signal result.

pub(crate) fn active_project_languages(
    service: &mut sk_doctrine::LanguageScopeService,
    repo_root: &Path,
    packs_root: Option<&Path>,
) -> Result<Option<Vec<String>>, String> {
    let request = sk_doctrine::InferRequest {
        repo_root: Some(repo_root),
        interview: None,
        prefer_interview: false,
        packs_root,
    };
    let inferred = service.infer(&request).map_err(|error| error.to_string())?;
    Ok(inferred.into_option())
}

/// The one doctrine load the CLI's governance reads share.
///
/// Built once, then reused by the action-context builder, the step-contract
/// delegation graph and the profile catalogue so those surfaces cannot disagree
/// about the layers, the activation ledger or the required floor.
pub(crate) struct DoctrineBundle {
    /// The merged built-in + org + project DRG.
    pub graph: sk_doctrine::DrgGraph,
    /// The same graph after the activation filter.
    pub filtered: sk_doctrine::DrgGraph,
    /// The activation ledger the filter and the contract admission read.
    pub ledger: sk_doctrine::ActivationLedger,
    /// The scanned identity index.
    pub identity: sk_doctrine::IdentityIndex,
    /// The language-scoped profile repository.
    pub profiles: sk_doctrine::ProfileRepository,
    /// The org pack's required floor per kind.
    pub floor: BTreeMap<sk_charter::ArtifactKind, Vec<String>>,
}

/// Load the merged graph, its activation-filtered view, and the governance
/// inputs every CLI surface shares.
pub(crate) fn doctrine_bundle(repo_root: &Path) -> Result<DoctrineBundle, CompatError> {
    fn domain(error: impl std::fmt::Display) -> CompatError {
        CompatError::domain(error.to_string())
    }
    let packs_override = packs_root(repo_root);
    let (built_in, org_packs, project) =
        doctrine_roots(repo_root, packs_override.as_deref()).map_err(domain)?;
    let org_roots: Vec<PathBuf> = org_packs.iter().map(|pack| pack.root.clone()).collect();
    let mut built_in_graph = sk_doctrine::load_graph_or_dir(&built_in).map_err(domain)?;
    let mut fragments = Vec::new();
    for (index, pack) in org_packs.iter().enumerate() {
        let root = &pack.root;
        // Root-level charter graphs and org-shaped fragments are distinct
        // inputs in the reference. A root graph must not gate the fragment,
        // nor imply that a fragment exists. Both can contribute to one pack.
        if sk_doctrine::has_graph_files(root) {
            let root_graph = sk_doctrine::load_graph_or_dir(root).map_err(domain)?;
            built_in_graph =
                sk_doctrine::merge_layers(&built_in_graph, Some(&root_graph)).map_err(domain)?;
        }
        if root.join(sk_doctrine::ORG_FRAGMENT).exists() {
            let fragment =
                sk_doctrine::load_org_pack(&pack.name, root, index + 1).map_err(domain)?;
            fragments.push(fragment);
        }
    }
    let project_graph = match &project {
        Some(root) if sk_doctrine::has_graph_files(root) => {
            Some(sk_doctrine::load_graph_or_dir(root).map_err(domain)?)
        }
        _ => None,
    };
    let merged =
        sk_doctrine::merge_three_layers(&built_in_graph, &fragments, project_graph.as_ref())
            .map_err(domain)?;
    let ledger = sk_doctrine::load_activation_ledger(repo_root).map_err(domain)?;
    let mut identity =
        sk_doctrine::IdentityIndex::from_roots(&built_in, &org_roots, project.as_deref());
    sk_doctrine::scan_all(&mut identity);
    let mut language_service = sk_doctrine::LanguageScopeService::new();
    let active_languages =
        active_project_languages(&mut language_service, repo_root, packs_override.as_deref())
            .map_err(domain)?;
    let profiles = sk_doctrine::ProfileRepository::load_with_languages(
        &built_in,
        &org_roots,
        project.as_deref(),
        active_languages.as_deref(),
    )
    .map_err(domain)?;
    // The org charter's required floor is a real input to the activation state:
    // it nets a stale deny so an org-required artifact is still delivered. An
    // empty map here would make the floor unenforceable on this surface.
    let floor = sk_doctrine::required_floor(repo_root, &identity)
        .map_err(domain)?
        .into_map();
    let activation =
        sk_doctrine::ActivationState::from_ledger(&ledger, &identity, &floor).map_err(domain)?;
    let filtered = sk_doctrine::filter_graph_by_activation(&merged.graph, &activation);
    Ok(DoctrineBundle {
        graph: merged.graph,
        filtered,
        ledger,
        identity,
        profiles,
        floor,
    })
}

/// The activation-filtered merged DRG for `repo_root`.
pub(crate) fn filtered_graph(repo_root: &Path) -> Result<sk_doctrine::DrgGraph, CompatError> {
    Ok(doctrine_bundle(repo_root)?.filtered)
}

pub(crate) fn action_context(
    repo_root: &Path,
    mission_type: &str,
    action: &str,
    profile_id: Option<&str>,
) -> Result<GovernanceFacts, String> {
    let bundle = doctrine_bundle(repo_root).map_err(|error| error.message().to_string())?;
    let graph = &bundle.graph;
    let ledger = &bundle.ledger;
    let identity = &bundle.identity;
    let profiles = &bundle.profiles;
    let floor = &bundle.floor;
    let inputs = sk_doctrine::ContextInputs {
        graph,
        ledger,
        identity,
        profiles,
        project_root: repo_root,
        required_floor: floor,
        project_warnings: &[],
    };
    let mut request = sk_doctrine::ContextRequest::new(action);
    request.mission_type = Some(mission_type);
    request.profile = profile_id;
    let context =
        sk_doctrine::build_action_context(&inputs, &request).map_err(|error| error.to_string())?;

    let profile = mission_type_profile(repo_root, mission_type);
    let template_set = profile_scalar(profile.as_ref(), "template_set")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "software-dev-default".to_string());
    let mut tools: Vec<String> = profile_list(profile.as_ref(), "available_tools");
    let charter_tools = !tools.is_empty();
    tools.extend(DEFAULT_TOOL_REGISTRY.iter().map(|tool| (*tool).to_string()));
    tools.sort();
    tools.dedup();

    // Paradigms: the activated set when configured, else the built-in catalog.
    let activated_paradigms = ledger.allow_for(sk_charter::ArtifactKind::Paradigm);
    let mut paradigms: Vec<String> = match activated_paradigms {
        Some(tokens) => tokens
            .iter()
            .map(|token| token.strip_prefix("paradigm:").unwrap_or(token).to_string())
            .collect(),
        None => graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == sk_doctrine::NodeKind::Paradigm)
            .filter_map(|node| node.urn().strip_prefix("paradigm:").map(str::to_string))
            .collect(),
    };
    paradigms.sort();
    paradigms.dedup();

    let mut diagnostics = Vec::new();
    if activated_paradigms.is_none() {
        diagnostics.push(format!(
            "No activated paradigm set configured; using built-in catalog default ({} paradigms).",
            paradigms.len()
        ));
    }
    if !charter_tools {
        diagnostics.push(
            "No available_tools selection provided; using runtime tool registry fallback."
                .to_string(),
        );
    }
    if ledger
        .allow_for(sk_charter::ArtifactKind::Directive)
        .is_none()
    {
        diagnostics.push(format!(
            "No activated directive set configured; using built-in catalog default ({} directives).",
            context.all_directives.len()
        ));
    }
    if profile_scalar(profile.as_ref(), "template_set").is_none_or(|value| value.is_empty()) {
        diagnostics.push(format!(
            "Template set not selected in charter; fallback '{template_set}' applied."
        ));
    }
    // Surface profile-source failures and the language-scope drop the same way
    // the repository records them: a skip must never be silent on this surface.
    for skipped in profiles.skipped() {
        let label = skipped
            .profile_id
            .clone()
            .unwrap_or_else(|| path_string(&skipped.path));
        diagnostics.push(format!(
            "Agent profile source is invalid: {label} ({})",
            skipped.summary
        ));
    }
    for profile_id in profiles.scope_filtered_ids() {
        diagnostics.push(format!(
            "Agent profile {profile_id} was excluded by the active language scope."
        ));
    }

    diagnostics.extend(
        context
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone()),
    );
    Ok(GovernanceFacts {
        context,
        paradigms,
        diagnostics,
        template_set,
        tools,
    })
}

/// Doctrine roots resolved with the same pack-root convention as templates.
pub(crate) fn doctrine_roots(
    repo_root: &Path,
    packs_root: Option<&Path>,
) -> Result<(PathBuf, Vec<sk_charter::OrgPackRoot>, Option<PathBuf>), String> {
    let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default())
        .map_err(|error| error.to_string())?;
    let expansion = sk_charter::PathExpansion::from_env();
    let mut search = sk_charter::BuiltInSearch::from_env(repo_root);
    if let Some(root) = packs_root {
        search = search.with_packs_root_env(root);
    }
    let roots = sk_charter::PackRoots::resolve(repo_root, &config, &expansion, Some(&search))
        .map_err(|error| error.to_string())?;
    let built_in = roots
        .built_in
        .ok_or_else(|| "no built-in pack root could be resolved".to_string())?;
    Ok((built_in, roots.organizations, roots.project))
}

// ---------------------------------------------------------------------------
// Prompt authoring
// ---------------------------------------------------------------------------

/// The outcome of resolving one step's prompt.
struct AuthoredPrompt {
    path: Option<PathBuf>,
    error: Option<String>,
}

impl AuthoredPrompt {
    fn failed(error: String) -> Self {
        Self {
            path: None,
            error: Some(error),
        }
    }
}

/// The template-backed prompt body for a non-WP step, when one resolves.
fn template_prompt_body(
    repo_root: &Path,
    mission_type: &str,
    step: &str,
    action: &str,
    prompt_template: Option<&str>,
) -> Option<(PathBuf, String)> {
    let mut names: Vec<String> = Vec::new();
    if let Some(template) = prompt_template {
        names.push(template.to_string());
    }
    names.push(format!("{action}.md"));
    if action != step {
        names.push(format!("{step}.md"));
    }
    names.dedup();
    for name in names {
        if let Some(path) = resolve_command_template(repo_root, mission_type, &name) {
            let content = fs::read_to_string(&path).ok()?;
            return Some((path, content));
        }
    }
    None
}

/// The short actionable prompt for a non-WP step with no template.
fn templateless_step_prompt(
    mission_type: &str,
    action: &str,
    mission_slug: &str,
    agent: &str,
) -> String {
    format!(
        "# {mission_type} — {action}\n\nPerform the `{action}` step for this Mission according to its charter and spec.\n\nWhen the step is done, advance with:\n\n    spec-kitty next --agent {agent} --mission {mission_slug}\n"
    )
}

/// Compose the reference's non-WP step prompt: header, governance, body.
fn compose_template_prompt(
    repo_root: &Path,
    mission: &sk_mission::Mission,
    mission_type: &str,
    step: &str,
    action: &str,
    prompt_template: Option<&str>,
    agent: &str,
) -> String {
    let header = format!(
        "{}\nMission: {}\nAgent: {}\nMission directory: {}\n{}",
        "=".repeat(80),
        mission.slug().as_str(),
        agent,
        path_string(mission.spec_dir()),
        "=".repeat(80)
    );
    let governance = governance_block(repo_root, mission_type, action, None);
    match template_prompt_body(repo_root, mission_type, step, action, prompt_template) {
        Some((_path, body)) => format!("{header}\n\n{governance}\n\n{body}"),
        None => templateless_step_prompt(mission_type, action, mission.slug().as_str(), agent),
    }
}

fn review_command_lines(
    workspace: &WpWorkspace,
    wp_meta: &sk_mission::WorkPackage,
    mission_slug: &str,
) -> Vec<String> {
    let mut lines = vec!["REVIEW COMMANDS:".to_string()];
    if workspace.lane_id.is_some() {
        let base = workspace
            .base_branch
            .clone()
            .unwrap_or_else(|| workspace.target_branch.clone());
        lines.push(format!("  git log {base}..HEAD --oneline"));
        lines.push(format!("  git diff {base}..HEAD --stat"));
    } else {
        let pathspecs: Vec<String> = wp_meta
            .frontmatter_field("owned_files")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mut review_paths = String::new();
        if !pathspecs.is_empty() {
            let mission_root = format!("kitty-specs/{mission_slug}/");
            let mut scoped = pathspecs.clone();
            if pathspecs.iter().any(|path| path.starts_with(&mission_root)) {
                for excluded in ["tasks/**", "tasks.md", "status.events.jsonl", "status.json"] {
                    scoped.push(format!(":(exclude){mission_root}{excluded}"));
                }
            }
            review_paths = format!(" -- {}", scoped.join(" "));
        }
        lines.push(format!(
            "  unavailable: no deterministic implementation claim commit found for this WP{review_paths}"
        ));
    }
    lines.push(String::new());
    lines.push(REVIEW_ANTIPATTERN_CHECKLIST.to_string());
    lines.push(String::new());
    lines
}

/// Compose the reference's WP prompt: workspace header, governance, isolation,
/// WP body and the exact completion commands.
fn compose_wp_prompt(
    repo_root: &Path,
    mission: &sk_mission::Mission,
    mission_type: &str,
    action: &str,
    wp_id: &str,
    agent: &str,
) -> Result<String, String> {
    let tasks_dir = super::support::tasks_dir(mission);
    let packages =
        super::support::work_packages(mission).map_err(|error| error.message().to_string())?;
    let package = packages
        .iter()
        .find(|package| package.id().as_str() == wp_id)
        .ok_or_else(|| {
            format!(
                "Canonical WORK_PACKAGE_TASK file for {wp_id} is missing at {}",
                tasks_dir.join(format!("{wp_id}.md")).display()
            )
        })?;
    let workspace = wp_workspace(repo_root, mission, wp_id);
    let profile_id = package
        .frontmatter_field("agent_profile")
        .and_then(Value::as_str);
    let profile = mission_type_profile(repo_root, mission_type);

    let mut lines = vec![
        "=".repeat(80),
        format!("{}: {wp_id}", action.to_uppercase()),
        "=".repeat(80),
        String::new(),
        format!("Agent: {agent}"),
        format!("Mission: {}", mission.slug().as_str()),
        format!("Mission Type: {mission_type}"),
        format!("Workspace: {}", path_string(&workspace.path)),
    ];
    match &workspace.lane_id {
        Some(lane_id) => {
            let shared = if workspace.lane_wp_ids.is_empty() {
                wp_id.to_string()
            } else {
                workspace.lane_wp_ids.join(", ")
            };
            lines.push(format!(
                "Workspace contract: lane {lane_id} shared by {shared}"
            ));
        }
        None => lines.push("Workspace contract: repository root planning workspace".to_string()),
    }
    lines.push(String::new());
    lines.push(mission_type_governance_block(
        repo_root,
        mission_type,
        profile.as_ref(),
    ));
    lines.push(String::new());
    lines.push(governance_block(
        repo_root,
        mission_type,
        action,
        profile_id,
    ));
    lines.push(
        "Authority references: project glossary `docs/context/`; architecture ADRs `docs/adr/`."
            .to_string(),
    );
    lines.push(String::new());
    lines.extend([
        "=".repeat(78),
        "  CRITICAL: WORK PACKAGE ISOLATION RULES".to_string(),
        "=".repeat(78),
        format!(
            "  YOU ARE {}: {wp_id}",
            if action == "implement" {
                "IMPLEMENTING"
            } else {
                "REVIEWING"
            }
        ),
        String::new(),
        "  DO:".to_string(),
        format!("    - Only modify status of {wp_id}"),
        "    - Ignore git commits and status changes from other agents".to_string(),
        String::new(),
        "  DO NOT:".to_string(),
        format!("    - Change status of any WP other than {wp_id}"),
        "    - React to or investigate other WPs' status changes".to_string(),
        "=".repeat(78),
        String::new(),
    ]);
    lines.push("WORKING DIRECTORY:".to_string());
    lines.push(format!("  cd {}", path_string(&workspace.path)));
    if workspace.lane_id.is_none() {
        lines.push(
            "  # Planning-artifact work for this WP happens in the repository root".to_string(),
        );
    }
    lines.push(String::new());
    if action == "review" {
        lines.extend(review_command_lines(
            &workspace,
            package,
            mission.slug().as_str(),
        ));
    }
    let body = work_package_text(package.source()).ok_or_else(|| {
        format!(
            "Could not read canonical WORK_PACKAGE_TASK file {}",
            package.source().display()
        )
    })?;
    lines.extend([
        "=".repeat(78),
        "  WORK PACKAGE PROMPT BEGINS".to_string(),
        "=".repeat(78),
        String::new(),
    ]);
    lines.push(body);
    lines.push(String::new());
    lines.extend([
        "=".repeat(78),
        "  WORK PACKAGE PROMPT ENDS".to_string(),
        "=".repeat(78),
        String::new(),
    ]);
    lines.push("WHEN DONE:".to_string());
    let mission_slug = mission.slug().as_str();
    if action == "implement" {
        let subtask_ids: Vec<String> = package
            .frontmatter_field("subtasks")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if subtask_ids.is_empty() {
            lines.push(
                "  No subtask completion command is needed; this work package declares no subtasks."
                    .to_string(),
            );
        } else {
            lines.push(format!(
                "  spec-kitty agent tasks mark-status {} --status done --mission {mission_slug}",
                subtask_ids.join(" ")
            ));
        }
        lines.push(format!(
            "  spec-kitty agent tasks move-task {wp_id} --to for_review --mission {mission_slug} --note \"Ready for review\""
        ));
    } else {
        lines.push(format!(
            "  APPROVE: spec-kitty agent tasks move-task {wp_id} --to approved --mission {mission_slug} --note \"Review passed\""
        ));
        lines.push(
            "           approved means review-passed; merge will later record done".to_string(),
        );
        lines.push(format!(
            "  REJECT:  spec-kitty agent tasks move-task {wp_id} --to planned --review-feedback-file <feedback-file> --mission {mission_slug}"
        ));
    }
    Ok(lines.join("\n"))
}

/// Resolve and write the prompt artifact for a composed step decision.
fn author_decision_prompt(
    store: &RunStore,
    run: &RunRef,
    ctx: &DecisionContext<'_>,
    agent: &str,
    envelope: &DecisionEnvelope,
) -> AuthoredPrompt {
    if envelope.kind() != CliDecisionKind::Step {
        return AuthoredPrompt {
            path: None,
            error: None,
        };
    }
    let Some(step) = envelope.step_id.as_deref() else {
        return AuthoredPrompt::failed("no step id to resolve a prompt for".to_string());
    };
    let action = envelope
        .action
        .clone()
        .unwrap_or_else(|| state_alias(step).unwrap_or(step).to_string());
    let content = if WP_ITERATION_STEPS.contains(&step)
        && let Some(wp_id) = envelope.wp_id.as_deref()
    {
        // This authority belongs to WP governance. Non-WP custom steps retain
        // their template/templateless path without requiring a type profile.
        if let Some(error) = unknown_mission_type_error(&ctx.repo_root, &ctx.mission_type) {
            return AuthoredPrompt::failed(format!(
                "prompt resolution failed for action '{action}': {error}"
            ));
        }
        match compose_wp_prompt(
            &ctx.repo_root,
            ctx.mission,
            &ctx.mission_type,
            &action,
            wp_id,
            agent,
        ) {
            Ok(content) => content,
            Err(error) => {
                return AuthoredPrompt::failed(format!(
                    "prompt resolution failed for action '{action}': {error}"
                ));
            }
        }
    } else {
        let prompt_template = resolve_runtime_template(ctx.root, &ctx.mission_type)
            .ok()
            .and_then(|template| sk_runtime::load_template(&template.path).ok())
            .and_then(|template| {
                template
                    .steps
                    .iter()
                    .find(|candidate| candidate.id == step)
                    .and_then(|candidate| candidate.prompt_template.clone())
            });
        compose_template_prompt(
            &ctx.repo_root,
            ctx.mission,
            &ctx.mission_type,
            step,
            &action,
            prompt_template.as_deref(),
            agent,
        )
    };
    let key = match envelope.wp_id.as_deref() {
        Some(wp_id) if WP_ITERATION_STEPS.contains(&step) => format!("{step}--{wp_id}"),
        _ => step.to_string(),
    };
    match store.write_derived_prompt(run, &key, &content) {
        Ok(path) => {
            // Keep the store's own issued-step artifact in sync so no stale
            // placeholder remains on disk for the same step.
            if key != step {
                let _ = store.write_derived_prompt(run, step, &content);
            }
            AuthoredPrompt {
                path: Some(path),
                error: None,
            }
        }
        Err(error) => AuthoredPrompt::failed(format!(
            "prompt artifact could not be written: {}",
            error.info().message
        )),
    }
}

// ---------------------------------------------------------------------------
// Decision assembly
// ---------------------------------------------------------------------------

/// Everything the decision builders need, resolved once per invocation.
struct DecisionContext<'a> {
    root: &'a Path,
    repo_root: PathBuf,
    mission: &'a sk_mission::Mission,
    mission_type: String,
    mission_number: String,
    board: Option<BoardView>,
    timestamp: String,
}

impl DecisionContext<'_> {
    fn progress(&self) -> Option<WpProgress> {
        self.board.as_ref().map(|board| board.progress.clone())
    }

    /// The mission-template tier/path, when it resolves.
    ///
    /// Query-mode payloads leave `origin` empty (upstream only fills it on the
    /// advancing path), so callers that need it ask explicitly.
    fn origin(&self) -> Origin {
        resolve_mission_config(&self.repo_root, &self.mission_type)
            .map(|(directory, tier)| Origin::resolved(tier, path_string(&directory)))
            .unwrap_or_else(Origin::unknown)
    }

    fn envelope(&self, kind: CliDecisionKind) -> DecisionEnvelope {
        DecisionEnvelope {
            kind: Some(kind),
            mission_slug: self.mission.slug().as_str().to_string(),
            mission_number: self.mission_number.clone(),
            mission_type: self.mission_type.clone(),
            timestamp: self.timestamp.clone(),
            progress: self.progress(),
            origin: (kind != CliDecisionKind::Query).then(|| self.origin()),
            is_query: kind == CliDecisionKind::Query,
            ..DecisionEnvelope::default()
        }
    }
}

/// The single board-authority-backed action selector.
///
/// Mirrors upstream `_resolve_wp_board_action`: a `dispatch` needs a
/// dependency-ready `planned` work package (`implement`) or a `for_review` one
/// (`review`); a board that has work but no dispatchable package becomes a
/// `blocked` floor with the upstream reason; `accept`/`done` decline (the DAG's
/// leave-step rule owns those transitions).
#[derive(Debug, Clone, PartialEq, Eq)]
enum BoardAction {
    Dispatch {
        step: &'static str,
        wp_id: String,
    },
    Blocked {
        step: Option<String>,
        reason: String,
    },
    Decline,
}

fn inspect_board_hint(ctx: &DecisionContext<'_>) -> String {
    format!(
        "Inspect the board: `spec-kitty agent tasks status --mission {}`.",
        ctx.mission.slug().as_str()
    )
}

fn resolve_board_action(ctx: &DecisionContext<'_>) -> BoardAction {
    let Some(board) = ctx.board.as_ref() else {
        return BoardAction::Decline;
    };
    // The reference requires BOTH `tasks.md` and `tasks/` before a board may
    // override the DAG (`_finalized_task_board_override_step`).
    if !board.can_override() {
        return BoardAction::Decline;
    }
    if let Some(error) = board.task_error.clone() {
        return BoardAction::Blocked {
            step: None,
            reason: error,
        };
    }
    let Some(step) = finalized_board_step(&board.facts) else {
        return BoardAction::Decline;
    };
    match step {
        BoardStep::Implement => match claimable(ctx).wp_id {
            Some(wp_id) => BoardAction::Dispatch {
                step: "implement",
                wp_id,
            },
            None => BoardAction::Blocked {
                step: Some("implement".to_string()),
                reason: format!(
                    "{}. {}",
                    claimable(ctx)
                        .selection_reason
                        .unwrap_or("no claimable work package"),
                    inspect_board_hint(ctx)
                ),
            },
        },
        BoardStep::Review => match board.facts.first_for_review.clone() {
            Some(wp_id) => BoardAction::Dispatch {
                step: "review",
                wp_id,
            },
            None => BoardAction::Blocked {
                step: Some("review".to_string()),
                reason: format!(
                    "Board reported a reviewable work package but none was found on re-read. {}",
                    inspect_board_hint(ctx)
                ),
            },
        },
        BoardStep::Blocked(sentinel) => BoardAction::Blocked {
            step: Some(format!("blocked:{sentinel}")),
            reason: format!(
                "No actionable work package ({}). {}",
                sentinel.replace('_', " "),
                inspect_board_hint(ctx)
            ),
        },
        BoardStep::Accept | BoardStep::Done => BoardAction::Decline,
    }
}

/// The query-mode preview of the finalized board.
///
/// Query reports the board's own answer without issuing anything: `wp_id` is the
/// claimable work package (or none, with the upstream selection reason), the
/// analysis-currency gate downgrades an `implement` preview to `analyze`, and a
/// `blocked:*` board is reported as `mission_state: blocked` under the
/// structural `kind: query` invariant.
fn board_query_decision(ctx: &DecisionContext<'_>, step: BoardStep) -> DecisionEnvelope {
    let mut envelope = ctx.envelope(CliDecisionKind::Query);
    if let Some(error) = ctx
        .board
        .as_ref()
        .and_then(|board| board.task_error.clone())
    {
        envelope.mission_state = "blocked".to_string();
        envelope.reason = Some(error);
        return envelope;
    }
    match step {
        BoardStep::Implement | BoardStep::Review => {
            envelope.mission_state = step.step_id().to_string();
            envelope.preview_step = Some(step.step_id().to_string());
            if step == BoardStep::Implement
                && let Some((failures, code)) = analysis_gate(ctx)
            {
                envelope.mission_state = "analyze".to_string();
                envelope.preview_step = Some("analyze".to_string());
                envelope.guard_failures = failures;
                envelope.error_code = Some(code.to_string());
                return envelope;
            }
            // The reference fills `override_wp_id` only for `implement`;
            // review/accept previews leave `wp_id` null.
            if step == BoardStep::Implement {
                let preview = claimable(ctx);
                envelope.wp_id = preview.wp_id;
                if envelope.wp_id.is_none() {
                    envelope.reason = preview.selection_reason.map(str::to_string);
                }
            }
        }
        BoardStep::Accept => {
            envelope.mission_state = "accept".to_string();
            envelope.preview_step = Some("accept".to_string());
        }
        BoardStep::Done => {
            envelope.mission_state = "done".to_string();
            envelope.reason = Some("All work packages are done".to_string());
        }
        BoardStep::Blocked(sentinel) => {
            // Upstream renders the sentinel alone here (`blocked:review_in_progress`
            // → "review in progress").
            envelope.mission_state = "blocked".to_string();
            envelope.reason = Some(sentinel.replace('_', " "));
        }
    }
    envelope
}

/// The advancing-mode board override.
///
/// Only a dispatch or a `blocked:*` sentinel pre-empts the DAG; every other
/// board answer (including a `blocked` floor naming `implement`/`review`) falls
/// through unchanged, exactly as upstream's `_dn_finalized_board_override`.
fn board_advance_decision(ctx: &DecisionContext<'_>) -> Option<DecisionEnvelope> {
    match resolve_board_action(ctx) {
        BoardAction::Dispatch { step, wp_id } => {
            if step == "implement"
                && let Some((failures, code)) = analysis_gate(ctx)
            {
                let mut envelope = ctx.envelope(CliDecisionKind::Step);
                envelope.mission_state = "analyze".to_string();
                envelope.step_id = Some("analyze".to_string());
                envelope.action = Some("analyze".to_string());
                envelope.guard_failures = failures;
                envelope.error_code = Some(code.to_string());
                return Some(envelope);
            }
            let mut envelope = ctx.envelope(CliDecisionKind::Step);
            envelope.mission_state = step.to_string();
            envelope.step_id = Some(step.to_string());
            envelope.action = Some(step.to_string());
            let workspace = workspace_for_wp(&ctx.repo_root, ctx.mission, &wp_id);
            envelope.workspace_path = Some(path_string(&workspace));
            envelope.wp_id = Some(wp_id);
            Some(envelope)
        }
        BoardAction::Blocked { step, reason } => {
            if !step
                .as_deref()
                .is_some_and(|step| step.starts_with("blocked:"))
            {
                return None;
            }
            let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
            envelope.mission_state = "blocked".to_string();
            envelope.reason = Some(reason);
            Some(envelope)
        }
        BoardAction::Decline => None,
    }
}

/// The claimable work package preview for this board.
fn claimable(ctx: &DecisionContext<'_>) -> sk_runtime::ClaimablePreview {
    let Some(board) = ctx.board.as_ref() else {
        return sk_runtime::ClaimablePreview {
            wp_id: None,
            selection_reason: Some(sk_runtime::NO_WP_FILES),
        };
    };
    let candidates: Vec<String> = board
        .packages
        .iter()
        .map(|package| package.id().as_str().to_string())
        .collect();
    let state_map: sk_mission::WpStateMap = board
        .packages
        .iter()
        .filter_map(|package| {
            sk_mission::WpState::ALL
                .iter()
                .find(|state| state.as_str() == board.lane_of(package.id().as_str()))
                .map(|state| (package.id().clone(), *state))
        })
        .collect();
    let mut dependency_ready = BTreeMap::new();
    for package in &board.packages {
        let ready = sk_mission::readiness_for_dependencies(
            package.id(),
            package.dependencies(),
            &state_map,
        )
        .satisfied();
        dependency_ready.insert(package.id().as_str().to_string(), ready);
    }
    sk_runtime::preview_claimable_wp(&candidates, &board.lanes, &dependency_ready)
}

/// Map a runtime planner decision onto the CLI envelope.
fn runtime_decision_envelope(
    ctx: &DecisionContext<'_>,
    run: &RunRef,
    decision: &NextDecision,
    agent: Option<&str>,
) -> DecisionEnvelope {
    let kind = CliDecisionKind::from_runtime(decision.kind());
    let mut envelope = ctx.envelope(kind);
    envelope.agent = agent.map(str::to_owned);
    envelope.run_id = Some(run.run_id.clone());
    match decision {
        NextDecision::Terminal(_) => {
            envelope.mission_state = "done".to_string();
            envelope.reason = Some(
                decision
                    .reason()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Mission complete".to_string()),
            );
        }
        NextDecision::Blocked(blocked) => {
            // Upstream maps an advancing blocked decision from the *planner*
            // step id, which a blocked decision does not carry.
            envelope.mission_state = decision
                .step_id()
                .map(str::to_owned)
                .unwrap_or_else(|| "unknown".to_string());
            envelope.step_id = decision.step_id().map(str::to_owned);
            envelope.reason = Some(blocked.reason.clone());
        }
        NextDecision::DecisionRequired(required) => {
            let step_id = decision.step_id().map(str::to_owned);
            envelope.mission_state = step_id.clone().unwrap_or_else(|| "unknown".to_string());
            envelope.step_id = step_id;
            envelope.reason = Some(
                decision
                    .reason()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Decision required".to_string()),
            );
            envelope.decision_id = decision.decision_id().map(str::to_owned);
            envelope.input_key = decision.input_key().map(str::to_owned);
            match &required.requirement {
                sk_runtime::DecisionRequirement::PendingDecision {
                    question, options, ..
                } => {
                    envelope.question = Some(question.clone());
                    envelope.options = (!options.is_empty()).then(|| options.clone());
                }
                sk_runtime::DecisionRequirement::MissingRequiredInput { question, .. } => {
                    envelope.question = Some(question.clone());
                }
                sk_runtime::DecisionRequirement::AuditCheckpoint {
                    question, options, ..
                } => {
                    envelope.question = Some(question.clone());
                    envelope.options = (!options.is_empty()).then(|| options.clone());
                }
            }
        }
        NextDecision::Step(step) => {
            envelope.mission_state = step.step_id.clone();
            envelope.step_id = Some(step.step_id.clone());
            if WP_ITERATION_STEPS.contains(&step.step_id.as_str()) {
                match resolve_board_action(ctx) {
                    BoardAction::Dispatch {
                        step: board_step,
                        wp_id,
                    } => {
                        envelope.action = Some(board_step.to_string());
                        envelope.mission_state = board_step.to_string();
                        let workspace = workspace_for_wp(&ctx.repo_root, ctx.mission, &wp_id);
                        envelope.workspace_path = Some(path_string(&workspace));
                        envelope.wp_id = Some(wp_id);
                    }
                    BoardAction::Blocked { reason, .. } => {
                        envelope.kind = Some(CliDecisionKind::Blocked);
                        envelope.reason = Some(reason);
                        envelope.action = None;
                    }
                    BoardAction::Decline => {
                        envelope.action = Some(step.step_id.clone());
                    }
                }
            } else {
                envelope.action = Some(
                    state_alias(&step.step_id)
                        .unwrap_or(step.step_id.as_str())
                        .to_string(),
                );
            }
            envelope.prompt_file = None;
        }
    }
    envelope
}

/// The issued step when there is one, else the planner's step, else `blocked`.
fn en_issued_or(state: &RunState, planner_step: Option<&str>) -> String {
    state
        .issued_step_id
        .clone()
        .or_else(|| planner_step.map(str::to_owned))
        .unwrap_or_else(|| "blocked".to_string())
}

/// The human-readable rendering the CLI prints in non-JSON mode.
fn print_human(value: &Value) {
    if value["is_query"].as_bool().unwrap_or(false) {
        println!("[QUERY — no result provided, state not advanced]");
        println!(
            "  Mission: {} @ {}",
            value["mission_slug"].as_str().unwrap_or(""),
            value["mission_state"].as_str().unwrap_or("")
        );
        if let Some(step) = value["preview_step"].as_str() {
            println!("  Next step: {step}");
        }
        if let Some(question) = value["question"].as_str() {
            println!("  Question: {question}");
            if let Some(options) = value["options"].as_array() {
                let rendered: Vec<&str> = options.iter().filter_map(Value::as_str).collect();
                if !rendered.is_empty() {
                    println!("  Options: {}", rendered.join(", "));
                }
            }
            if let Some(decision_id) = value["decision_id"].as_str() {
                println!("  Decision ID: {decision_id}");
            }
        } else if let Some(reason) = value["reason"].as_str() {
            println!("  Reason: {reason}");
        }
    } else {
        println!(
            "[{}] {} @ {}",
            value["kind"].as_str().unwrap_or("").to_uppercase(),
            value["mission_slug"].as_str().unwrap_or(""),
            value["mission_state"].as_str().unwrap_or("")
        );
        if let Some(action) = value["action"].as_str() {
            match value["wp_id"].as_str() {
                Some(wp_id) => println!("  Action: {action} {wp_id}"),
                None => println!("  Action: {action}"),
            }
        }
        if let Some(workspace) = value["workspace_path"].as_str() {
            println!("  Workspace: {workspace}");
        }
        if let Some(failures) = value["guard_failures"].as_array()
            && !failures.is_empty()
        {
            let rendered: Vec<&str> = failures.iter().filter_map(Value::as_str).collect();
            println!("  Guards pending: {}", rendered.join(", "));
        }
        if let Some(reason) = value["reason"].as_str() {
            println!("  Reason: {reason}");
        }
        if let Some(question) = value["question"].as_str() {
            println!("  Question: {question}");
        }
        if let Some(prompt) = value["prompt_file"].as_str() {
            println!();
            println!("  Next step: read the prompt file:");
            println!("    cat {prompt}");
        }
    }
    if let Some(progress) = value["progress"].as_object()
        && let Some(total) = progress.get("total_wps").and_then(Value::as_u64)
        && total > 0
    {
        let done = progress
            .get("done_wps")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let percentage = progress
            .get("weighted_percentage")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        println!("  Progress: {}% ({done}/{total} done)", percentage as i64);
    }
    if let Some(run_id) = value["run_id"].as_str() {
        println!("  Run ID: {run_id}");
    }
}

/// Finish a decision: enforce the step/prompt contract, render, and return the
/// payload plus the exit code.
fn finish_decision(
    envelope: DecisionEnvelope,
    json_output: bool,
    prompt_file: Option<PathBuf>,
    prompt_error: Option<String>,
    answered: Option<(&str, &str)>,
) -> Result<(Value, i32), CompatError> {
    let mut envelope = envelope;
    if envelope.kind() == CliDecisionKind::Step {
        envelope.prompt_file = prompt_file.as_ref().map(|path| path_string(path));
    }
    let exists = prompt_file.as_ref().is_some_and(|path| path.is_file());
    let reason = prompt_error
        .or_else(|| envelope.reason.clone())
        .unwrap_or_else(|| "prompt_file_not_resolvable".to_string());
    let envelope = envelope.materialize(exists, reason);
    // Upstream `next` does not join the branch-context contract (that belongs to
    // `specify`/`plan`/`branch-context`), so the payload stays exactly the
    // `Decision.to_dict()` key set plus the answered-decision pair, applied
    // before the document is emitted.
    let mut payload = envelope.to_json();
    if let (Some((answered_id, answer)), Some(object)) = (answered, payload.as_object_mut()) {
        object.insert("answered".to_string(), json!(answered_id));
        object.insert("answer".to_string(), json!(answer));
    }
    if json_output {
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        print_human(&payload);
    }
    let exit = if envelope.kind() == CliDecisionKind::Blocked {
        1
    } else {
        0
    };
    Ok((payload, exit))
}

/// The `next` command.
#[allow(clippy::too_many_arguments)]
pub fn next(
    handle: Option<&str>,
    agent: Option<&str>,
    result: Option<&str>,
    answer: Option<&str>,
    decision_id: Option<&str>,
    json_output: bool,
) -> Result<i32, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;

    // Argument contract first: query mode is read-only, so an answer without a
    // result is refused before any run is created or any cursor is touched.
    if answer.is_some() && result.is_none() {
        return Err(CompatError::domain(
            "--answer requires --result because query mode is read-only",
        ));
    }
    if answer.is_some() && agent.is_none() {
        return Err(CompatError::domain(
            "--agent is required when --answer is provided",
        ));
    }
    if result.is_some() && agent.is_none() {
        return Err(CompatError::domain(
            "--agent is required when --result is provided",
        ));
    }
    let step_result = result.map(parse_result).transpose()?;

    let ctx = DecisionContext {
        root: &root,
        repo_root: repo_root_for(&root, &mission),
        mission: &mission,
        mission_type: mission_type_of(&mission),
        mission_number: mission_number_of(&mission),
        board: gather_board(&mission)?,
        timestamp: sk_kernel::clock::now_utc_iso(),
    };

    if step_result.is_none() {
        return query_mode(&ctx, agent, json_output);
    }
    let step_result = step_result.expect("checked above");
    advance_mode(
        &ctx,
        agent.expect("checked above for a result"),
        step_result,
        answer,
        decision_id,
        json_output,
    )
}

/// Whether the run's journal ends in a torn (non-newline-terminated) line.
fn journal_has_torn_tail(run_dir: &Path) -> bool {
    let path = run_dir.join(sk_run_store::EVENTS_FILENAME);
    let Ok(bytes) = fs::read(&path) else {
        return false;
    };
    !bytes.is_empty() && !bytes.ends_with(b"\n")
}

/// The read-only query-mode call.
///
/// Order mirrors upstream: a canonical task-surface error, then the finalized
/// board override, then the untouched-run preview, then the planner decision.
/// The durable run is read (never started) before the override so the reported
/// `run_id` is truthful in every branch.
fn query_mode(
    ctx: &DecisionContext<'_>,
    agent: Option<&str>,
    json_output: bool,
) -> Result<i32, CompatError> {
    let mut envelope = ctx.envelope(CliDecisionKind::Query);
    envelope.agent = agent.map(str::to_owned);

    let store = open_store(ctx.root)?;
    let run = store
        .run_for_mission(ctx.mission.slug().as_str(), Some(ctx.mission.id().as_str()))
        .map_err(map_query_error)?;
    let (state, decision, issued_step) = match &run {
        Some(run) => {
            envelope.run_id = Some(run.run_id.clone());
            match store.query(run) {
                Ok(query) => (
                    query.state.clone(),
                    query.decision.clone(),
                    query.issued_step.clone(),
                ),
                Err(_) if journal_has_torn_tail(&run.run_dir) => {
                    // The reference read path tolerates a torn final journal
                    // line (it consumes complete lines); only a mutation must
                    // refuse until recovery. Plan from the durable cursor
                    // without claiming the journal is consistent.
                    let (state, decision) = store.plan_from_cursor(run).map_err(map_query_error)?;
                    (state, decision, None)
                }
                Err(error) => return Err(map_query_error(error)),
            }
        }
        None => {
            // No durable run: plan against the live template and an empty
            // cursor in memory. Nothing is written (upstream bootstraps an
            // ephemeral run store here and deletes it; the native facade never
            // needs one).
            let template =
                resolve_runtime_template(ctx.root, &ctx.mission_type).map_err(|error| {
                    CompatError::coded(
                        "QUERY_MODE_VALIDATION_FAILED",
                        format!(
                            "Could not read query state for mission '{}': {}",
                            ctx.mission.slug().as_str(),
                            error.message()
                        ),
                    )
                })?;
            let template_path = path_string(&template.path);
            let mission = sk_runtime::load_template(&template.path).map_err(|error| {
                CompatError::coded(
                    "QUERY_MODE_VALIDATION_FAILED",
                    format!(
                        "Could not read query state for mission '{}': {error}",
                        ctx.mission.slug().as_str()
                    ),
                )
            })?;
            let snapshot = sk_runtime::RunSnapshot {
                run_id: "query-preview".to_string(),
                mission_key: mission.mission.key.clone(),
                template_path,
                template_hash: String::new(),
                policy_snapshot: Default::default(),
                completed_steps: Vec::new(),
                issued_step_id: None,
                inputs: BTreeMap::new(),
                decisions: BTreeMap::new(),
                pending_decisions: BTreeMap::new(),
                blocked_reason: None,
                mission_id: None,
                mission_slug: Some(ctx.mission.slug().as_str().to_string()),
                actor_context: BTreeMap::new(),
            };
            let decision = plan_next(&mission, &snapshot, "").map_err(|error| {
                CompatError::coded(
                    "QUERY_MODE_VALIDATION_FAILED",
                    format!(
                        "Could not read query state for mission '{}': {error}",
                        ctx.mission.slug().as_str()
                    ),
                )
            })?;
            (RunState::default(), decision, None)
        }
    };

    // 1. A board whose status names a work package with no task file cannot be
    //    turned into truthful progress: report the canonical task-read error.
    if let Some(error) = ctx
        .board
        .as_ref()
        .and_then(|board| board.task_error.clone())
    {
        envelope.mission_state = "blocked".to_string();
        envelope.reason = Some(error);
        let (_, exit) = finish_decision(envelope, json_output, None, None, None)?;
        return Ok(exit);
    }

    // 2. The finalized board overrides a stale DAG phase.
    if let Some(step) = ctx
        .board
        .as_ref()
        .filter(|board| board.can_override())
        .and_then(|board| finalized_board_step(&board.facts))
    {
        let mut board = board_query_decision(ctx, step);
        board.run_id = envelope.run_id.clone();
        board.agent = envelope.agent.clone();
        let (_, exit) = finish_decision(board, json_output, None, None, None)?;
        return Ok(exit);
    }

    // 3. Untouched run (or no run at all): the truthful first-step preview.
    let untouched = state.completed_steps.is_empty()
        && state.pending_decisions.is_empty()
        && state.decisions.is_empty();
    if untouched
        && matches!(
            decision.kind(),
            sk_runtime::DecisionKind::Step | sk_runtime::DecisionKind::DecisionRequired
        )
    {
        envelope.mission_state = "not_started".to_string();
        envelope.preview_step = decision.step_id().map(str::to_owned);
        let (_, exit) = finish_decision(envelope, json_output, None, None, None)?;
        return Ok(exit);
    }

    match &decision {
        NextDecision::DecisionRequired(required) => {
            let step_id = decision.step_id().map(str::to_owned);
            envelope.mission_state = en_issued_or(&state, step_id.as_deref());
            envelope.step_id = state.issued_step_id.clone().or(step_id);
            envelope.decision_id = decision.decision_id().map(str::to_owned);
            envelope.input_key = decision.input_key().map(str::to_owned);
            match &required.requirement {
                sk_runtime::DecisionRequirement::PendingDecision {
                    question, options, ..
                }
                | sk_runtime::DecisionRequirement::AuditCheckpoint {
                    question, options, ..
                } => {
                    envelope.question = Some(question.clone());
                    envelope.options = (!options.is_empty()).then(|| options.clone());
                }
                sk_runtime::DecisionRequirement::MissingRequiredInput { question, .. } => {
                    envelope.question = Some(question.clone());
                }
            }
            let (_, exit) = finish_decision(envelope, json_output, None, None, None)?;
            return Ok(exit);
        }
        NextDecision::Blocked(_) => {
            let planner_step = decision.step_id().map(str::to_owned);
            envelope.mission_state = en_issued_or(&state, planner_step.as_deref());
            envelope.step_id = state.issued_step_id.clone().or(planner_step);
            envelope.reason = state
                .blocked_reason
                .clone()
                .or_else(|| decision.reason().map(str::to_owned));
        }
        NextDecision::Terminal(_) => {
            envelope.mission_state = "done".to_string();
        }
        NextDecision::Step(step) => {
            envelope.mission_state = step.step_id.clone();
            envelope.step_id = issued_step
                .as_ref()
                .map(|issued| issued.step_id.clone())
                .or_else(|| decision.step_id().map(str::to_owned));
        }
    }
    let (_, exit) = finish_decision(envelope, json_output, None, None, None)?;
    Ok(exit)
}

/// The advancing-mode call: answer, gate, advance, compose.
fn advance_mode(
    ctx: &DecisionContext<'_>,
    agent: &str,
    step_result: StepResult,
    answer: Option<&str>,
    decision_id: Option<&str>,
    json_output: bool,
) -> Result<i32, CompatError> {
    let (store, run) = ensure_run(ctx.root, ctx.mission)?;

    // 1. Persist any pending answer first, as the human operator.
    let answered_id = match answer {
        Some(answer) => Some(answer_decision(&store, &run, answer, decision_id, agent)?),
        None => None,
    };
    // Upstream reads the cursor only after the answer is durable, so every gate
    // below sees the answered run.
    let state = store.read_state(&run).map_err(map_store_error)?;

    // 2. Finalized-board first contact: a board dispatch pre-empts a stale DAG
    //    phase, exactly as upstream's `_dn_finalized_board_override`.
    let untouched = state.completed_steps.is_empty()
        && state.pending_decisions.is_empty()
        && state.decisions.is_empty();
    if step_result == StepResult::Success
        && untouched
        && let Some(mut envelope) = board_advance_decision(ctx)
    {
        envelope.run_id = Some(run.run_id.clone());
        envelope.agent = Some(agent.to_string());
        let authored = author_decision_prompt(&store, &run, ctx, agent, &envelope);
        let (_, exit) = finish_decision(
            envelope,
            json_output,
            authored.path,
            authored.error,
            answered_pair(answered_id.as_deref(), answer),
        )?;
        return Ok(exit);
    }

    // 3. The dependency/guard gate: hold the run in place instead of advancing.
    if step_result == StepResult::Success
        && let Some(current) = state.issued_step_id.clone()
    {
        if WP_ITERATION_STEPS.contains(&current.as_str()) {
            // The reference's WP-advance guard reads the canonical status log
            // fail-loud: an absent log is a blocked decision naming the
            // `finalize-tasks` bootstrap remedy, never a silent "all genesis".
            if let Some(error) = canonical_status_missing_error(ctx) {
                let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
                envelope.mission_state = current.clone();
                envelope.step_id = Some(current);
                envelope.reason = Some(error.clone());
                envelope.guard_failures = vec![error];
                envelope.run_id = Some(run.run_id.clone());
                envelope.agent = Some(agent.to_string());
                let (_, exit) = finish_decision(
                    envelope,
                    json_output,
                    None,
                    None,
                    answered_pair(answered_id.as_deref(), answer),
                )?;
                return Ok(exit);
            }
            let advances = ctx.board.as_ref().is_some_and(|board| {
                board.task_error.is_none() && board_advances_step(board, &current)
            });
            if !advances {
                let mut envelope = wp_iteration_decision(ctx, &run, &current)?;
                envelope.agent = Some(agent.to_string());
                let authored = author_decision_prompt(&store, &run, ctx, agent, &envelope);
                let (_, exit) = finish_decision(
                    envelope,
                    json_output,
                    authored.path,
                    authored.error,
                    answered_pair(answered_id.as_deref(), answer),
                )?;
                return Ok(exit);
            }
        } else if ctx.mission_type == SOFTWARE_DEV {
            let (failures, paths) = software_dev_guards(
                &ctx.repo_root,
                ctx.mission,
                ctx.board.as_ref(),
                &ctx.mission_type,
                &current,
            );
            if !failures.is_empty() {
                let mut envelope = reissue_step_decision(ctx, &run, &current, failures, paths)?;
                envelope.agent = Some(agent.to_string());
                let authored = author_decision_prompt(&store, &run, ctx, agent, &envelope);
                let (_, exit) = finish_decision(
                    envelope,
                    json_output,
                    authored.path,
                    authored.error,
                    answered_pair(answered_id.as_deref(), answer),
                )?;
                return Ok(exit);
            }
        }
    }

    // 3b. Composition dispatch (single dispatch, FR-001/FR-002): a
    //     composition-eligible issued step executes its real step contract, runs
    //     the post-action guard, then commits the engine's own advance plan. The
    //     legacy `store.advance` below is never reached for these steps.
    if step_result == StepResult::Success
        && let Some(current) = state.issued_step_id.clone()
        && let Some(exit) = composition_dispatch(
            ctx,
            &store,
            &run,
            agent,
            &state,
            &current,
            answered_id.as_deref(),
            answer,
            json_output,
        )?
    {
        return Ok(exit);
    }

    // 4. Advance the durable run.
    let outcome = match store.advance(
        &run,
        &AdvanceRequest {
            agent_id: agent.to_string(),
            result: step_result,
            expected_issued_step: state.issued_step_id.clone(),
        },
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            // The reference materializes an engine failure as a `blocked`
            // decision document (exit 1), not a bare error envelope: the
            // machine contract stays "always a decision".
            let info = error.info();
            let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
            envelope.mission_state = state
                .issued_step_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            envelope.reason = Some(format!("Runtime engine error: {}", info.message));
            envelope.agent = Some(agent.to_string());
            let (_, exit) = finish_decision(
                envelope,
                json_output,
                None,
                None,
                answered_pair(answered_id.as_deref(), answer),
            )?;
            return Ok(exit);
        }
    };
    // The decision to report is the one this advance produced. Querying again
    // would report the *following* step, because the planner skips the step it
    // just issued.
    let envelope = runtime_decision_envelope(ctx, &run, &outcome.decision, Some(agent));
    let authored = author_decision_prompt(&store, &run, ctx, agent, &envelope);
    let (_, exit) = finish_decision(
        envelope,
        json_output,
        authored.path,
        authored.error,
        answered_pair(answered_id.as_deref(), answer),
    )?;
    Ok(exit)
}

/// Dispatch one composition-eligible step through the real step contract.
///
/// Order mirrors the reference exactly: execute the contract (invoking and
/// closing each step exactly once), then the post-action guard, then the
/// engine's canonical `plan_advance` → `commit_advance` once. A failure at any
/// stage returns a structured `blocked` decision and leaves the run cursor
/// untouched; no legacy DAG dispatch runs as a fallback.
///
/// `Ok(None)` means the step is not composition-eligible and the caller must
/// continue on the legacy path.
#[allow(clippy::too_many_arguments)]
fn composition_dispatch(
    ctx: &DecisionContext<'_>,
    store: &RunStore,
    run: &RunRef,
    agent: &str,
    state: &RunState,
    current: &str,
    answered_id: Option<&str>,
    answer: Option<&str>,
    json_output: bool,
) -> Result<Option<i32>, CompatError> {
    if !invocation::should_dispatch_via_composition(
        &ctx.repo_root,
        &ctx.mission_type,
        current,
        Some(&run.run_dir),
    ) {
        return Ok(None);
    }
    let services = match invocation::InvocationServices::load(&ctx.repo_root, &ctx.mission_type) {
        Ok(services) => services,
        Err(error) => {
            // A registry/data load failure is the reference's structured composed
            // failure surface, not a traceback: block without advancing.
            let envelope =
                composition_blocked_envelope(ctx, current, &[error.message().to_string()]);
            let (_, exit) = finish_decision(
                envelope,
                json_output,
                None,
                None,
                answered_pair(answered_id, answer),
            )?;
            return Ok(Some(exit));
        }
    };
    let composed_action = invocation::normalize_action_for_composition(current).to_string();
    // Built-in actions resolve their profile through the shipped default table
    // (and its availability fallback); only a custom step's frozen-template
    // binding is threaded as a hint (the reference's PR #797 / FR-003 order).
    let profile_hint = if sk_invocation::step_contract::action_profile_default(
        &ctx.mission_type,
        &composed_action,
    )
    .is_some()
    {
        None
    } else {
        invocation::frozen_step_binding(&run.run_dir, current).0
    };
    let record_root = sk_invocation::resolve_op_record_root(&ctx.repo_root, ctx.root);
    let composition = invocation::compose_action(
        &services,
        &composed_action,
        agent,
        &record_root,
        profile_hint.as_deref(),
        None,
    );
    let outcome = match composition {
        Ok(outcome) => outcome,
        Err(failure) => {
            let envelope = composition_blocked_envelope(ctx, current, &[failure]);
            let (_, exit) = finish_decision(
                envelope,
                json_output,
                None,
                None,
                answered_pair(answered_id, answer),
            )?;
            return Ok(Some(exit));
        }
    };

    // Post-action guard. The reference degrades an unregistered mission family to
    // a neutral (empty) result with a WARNING; only software-dev has a ported
    // composed-guard family today, so every other family takes that same neutral
    // path rather than being judged by software-dev's rules.
    let guard_failures = if ctx.mission_type == SOFTWARE_DEV {
        software_dev_guards(
            &ctx.repo_root,
            ctx.mission,
            ctx.board.as_ref(),
            &ctx.mission_type,
            current,
        )
        .0
    } else {
        Vec::new()
    };
    if !guard_failures.is_empty() {
        let envelope = composition_blocked_envelope(ctx, current, &guard_failures);
        let (_, exit) = finish_decision(
            envelope,
            json_output,
            None,
            None,
            answered_pair(answered_id, answer),
        )?;
        return Ok(Some(exit));
    }

    // Plan first, commit second: one canonical advance, no second dispatch.
    let request = AdvanceRequest {
        agent_id: agent.to_string(),
        result: StepResult::Success,
        expected_issued_step: state.issued_step_id.clone(),
    };
    let plan = match store.plan_advance(run, &request) {
        Ok(plan) => plan,
        Err(error) => {
            let envelope = advance_failed_envelope(ctx, current, &error.to_string());
            let (_, exit) = finish_decision(
                envelope,
                json_output,
                None,
                None,
                answered_pair(answered_id, answer),
            )?;
            return Ok(Some(exit));
        }
    };
    let advanced = match store.commit_advance(&plan) {
        Ok(advanced) => advanced,
        Err(error) => {
            let envelope = advance_failed_envelope(ctx, current, &error.to_string());
            let (_, exit) = finish_decision(
                envelope,
                json_output,
                None,
                None,
                answered_pair(answered_id, answer),
            )?;
            return Ok(Some(exit));
        }
    };
    // The composed run's audit trail is the Op records themselves (one started +
    // one completed per step, each best-effort committed); the reference logs the
    // invocation-id chain and adds no extra decision key, so the payload contract
    // stays exactly `Decision.to_dict()`.
    let _ = &outcome;
    let mut envelope = runtime_decision_envelope(ctx, run, &advanced.decision, Some(agent));
    if envelope.step_id.is_none() {
        envelope.step_id = Some(current.to_string());
    }
    let authored = author_decision_prompt(store, run, ctx, agent, &envelope);
    let (_, exit) = finish_decision(
        envelope,
        json_output,
        authored.path,
        authored.error,
        answered_pair(answered_id, answer),
    )?;
    Ok(Some(exit))
}

fn composition_blocked_envelope(
    ctx: &DecisionContext<'_>,
    step: &str,
    failures: &[String],
) -> DecisionEnvelope {
    let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
    envelope.mission_state = step.to_string();
    envelope.step_id = Some(step.to_string());
    envelope.reason = failures.first().cloned();
    envelope.guard_failures = failures.to_vec();
    envelope
}

fn advance_failed_envelope(
    ctx: &DecisionContext<'_>,
    step: &str,
    detail: &str,
) -> DecisionEnvelope {
    let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
    envelope.mission_state = step.to_string();
    envelope.step_id = Some(step.to_string());
    envelope.reason = Some(format!(
        "Run-state advancement after composition failed: {detail}"
    ));
    envelope
}

/// The answered-decision pair, when this call persisted an answer.
fn answered_pair<'a>(
    answered_id: Option<&'a str>,
    answer: Option<&'a str>,
) -> Option<(&'a str, &'a str)> {
    Some((answered_id?, answer?))
}

fn answer_decision(
    store: &RunStore,
    run: &RunRef,
    answer: &str,
    decision_id: Option<&str>,
    agent: &str,
) -> Result<String, CompatError> {
    let decision = match decision_id {
        Some(decision_id) => decision_id.to_owned(),
        None => {
            let query = store.query(run).map_err(map_store_error)?;
            match query.pending.as_slice() {
                [] => {
                    return Err(CompatError::domain(
                        "no pending decision to answer in this run",
                    ));
                }
                [only] => only.decision_id.clone(),
                many => {
                    return Err(CompatError::coded(
                        "DECISION_SELECTION_REQUIRED",
                        "multiple decisions are pending; pass --decision-id",
                    )
                    .with(
                        "pending",
                        json!(
                            many.iter()
                                .map(|decision| decision.decision_id.clone())
                                .collect::<Vec<_>>()
                        ),
                    ));
                }
            }
        }
    };
    // CLI answers are human-authored even though the invocation carries the
    // agent identity for the surrounding mission loop.
    let actor = ActorIdentity::new(agent, ActorType::Human);
    store
        .answer(
            run,
            &AnswerRequest {
                decision_id: decision.clone(),
                answer: answer.to_owned(),
                actor,
            },
        )
        .map_err(map_store_error)?;
    Ok(decision)
}

/// Build a work-package iteration decision for an in-progress step.
///
/// Routed through the single board selector: a dispatch re-issues
/// `implement`/`review` for the resolved work package; the board's own blocked
/// floor becomes a blocked decision; a board with no opinion yet falls back to
/// the step's mapped action.
fn wp_iteration_decision(
    ctx: &DecisionContext<'_>,
    run: &RunRef,
    step: &str,
) -> Result<DecisionEnvelope, CompatError> {
    match resolve_board_action(ctx) {
        BoardAction::Dispatch {
            step: board_step,
            wp_id,
        } => {
            let workspace = workspace_for_wp(&ctx.repo_root, ctx.mission, &wp_id);
            let mut envelope = ctx.envelope(CliDecisionKind::Step);
            envelope.mission_state = board_step.to_string();
            envelope.step_id = Some(board_step.to_string());
            envelope.action = Some(board_step.to_string());
            envelope.wp_id = Some(wp_id);
            envelope.workspace_path = Some(path_string(&workspace));
            envelope.run_id = Some(run.run_id.clone());
            Ok(envelope)
        }
        BoardAction::Blocked {
            step: _board_step,
            reason,
        } => {
            // The board's `blocked:*` sentinel is an internal selector value.
            // Upstream emits the *issued* step id in `mission_state`/`step_id`
            // and reports the sentinel-derived reason only.
            let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
            envelope.mission_state = step.to_string();
            envelope.step_id = Some(step.to_string());
            envelope.reason = Some(reason);
            envelope.run_id = Some(run.run_id.clone());
            Ok(envelope)
        }
        BoardAction::Decline => {
            let mut envelope = ctx.envelope(CliDecisionKind::Blocked);
            envelope.mission_state = step.to_string();
            envelope.step_id = Some(step.to_string());
            envelope.reason = Some(format!("No action mapped for step '{step}'"));
            envelope.run_id = Some(run.run_id.clone());
            Ok(envelope)
        }
    }
}

/// Re-issue the current step with its guard diagnostics (no advance).
fn reissue_step_decision(
    ctx: &DecisionContext<'_>,
    run: &RunRef,
    step: &str,
    failures: Vec<String>,
    paths: BTreeMap<String, String>,
) -> Result<DecisionEnvelope, CompatError> {
    let mut envelope = ctx.envelope(CliDecisionKind::Step);
    envelope.mission_state = step.to_string();
    envelope.step_id = Some(step.to_string());
    envelope.action = Some(state_alias(step).unwrap_or(step).to_string());
    envelope.guard_failures = failures;
    envelope.guard_failure_paths = paths;
    envelope.run_id = Some(run.run_id.clone());
    if WP_ITERATION_STEPS.contains(&step)
        && let Some(board) = ctx.board.as_ref()
    {
        let wp_id = match step {
            "implement" => claimable(ctx).wp_id,
            _ => board.facts.first_for_review.clone(),
        };
        if let Some(wp_id) = wp_id {
            let workspace = workspace_for_wp(&ctx.repo_root, ctx.mission, &wp_id);
            envelope.wp_id = Some(wp_id);
            envelope.workspace_path = Some(path_string(&workspace));
        }
    }
    Ok(envelope)
}

// ---------------------------------------------------------------------------
// Native extension routes
// ---------------------------------------------------------------------------

/// Native extension: start a governed run.
pub fn start(
    handle: Option<&str>,
    template: Option<&Path>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let store = open_store(&root)?;
    let template = match template {
        Some(path) => path.to_path_buf(),
        None => runtime_template(&root, &mission_type_of(&mission))?,
    };
    let outcome = store
        .start(&StartRequest {
            mission_slug: mission.slug().as_str().to_owned(),
            mission_id: Some(mission.id().as_str().to_owned()),
            template_path: template,
            inputs: Default::default(),
            policy_snapshot: Default::default(),
        })
        .map_err(map_store_error)?;
    let value = serde_json::to_value(&outcome)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "{} run {} for {}",
            if outcome.started { "Started" } else { "Reused" },
            outcome.run.run_id,
            mission.slug().as_str()
        );
    }
    Ok(value)
}

/// Native extension: query a governed run (never starts one).
pub fn query(handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = existing_run(&root, &mission)?;
    let result = store.query(&run).map_err(map_store_error)?;
    let value = serde_json::to_value(&result)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("run {}: {:?}", run.run_id, result.decision.kind());
    }
    Ok(value)
}

/// Native extension: advance a governed run.
pub fn advance(
    handle: Option<&str>,
    agent: &str,
    result: &str,
    expected_issued_step: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = ensure_run(&root, &mission)?;
    let outcome = store
        .advance(
            &run,
            &AdvanceRequest {
                agent_id: agent.to_owned(),
                result: parse_result(result)?,
                expected_issued_step: expected_issued_step.map(str::to_owned),
            },
        )
        .map_err(map_store_error)?;
    let value = serde_json::to_value(&outcome)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("advanced run {}: {:?}", run.run_id, outcome.decision.kind());
    }
    Ok(value)
}

/// Native extension: answer a pending decision.
pub fn answer(
    handle: Option<&str>,
    decision_id: &str,
    answer: &str,
    actor: &str,
    json_output: bool,
) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = ensure_run(&root, &mission)?;
    let outcome = store
        .answer(
            &run,
            &AnswerRequest {
                decision_id: decision_id.to_owned(),
                answer: answer.to_owned(),
                actor: ActorIdentity::new(actor, ActorType::Human),
            },
        )
        .map_err(map_store_error)?;
    let value = serde_json::to_value(&outcome)
        .map_err(|error| CompatError::domain(format!("could not serialize run: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("answered {decision_id}");
    }
    Ok(value)
}

/// Native extension: reconcile a run's cursor and journal.
pub fn recover(handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let root = project_root()?;
    let mission = resolve_mission(&root, handle)?;
    let (store, run) = ensure_run(&root, &mission)?;
    let report = store.recover(&run).map_err(map_store_error)?;
    let value = serde_json::to_value(&report)
        .map_err(|error| CompatError::domain(format!("could not serialize recovery: {error}")))?;
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("run {} recovered", run.run_id);
    }
    Ok(value)
}

/// True when the decision is runnable (a step or an open decision).
pub fn decision_is_runnable(decision: &NextDecision) -> bool {
    matches!(
        decision.kind(),
        sk_runtime::DecisionKind::Step | sk_runtime::DecisionKind::DecisionRequired
    )
}

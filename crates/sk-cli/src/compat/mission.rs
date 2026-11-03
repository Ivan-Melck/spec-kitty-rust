//! Mission authoring routes: `agent mission create`, `specify`, `plan`,
//! `agent mission setup-plan`, `tasks` and `agent mission finalize-tasks`.
//!
//! Creation and dependency finalization are executed by `sk-mission` plans;
//! this module resolves canonical templates, supplies the request, and renders
//! the compatibility payload. It never commits to Git (the reviewed
//! safe-commit path is not yet integrated) and says so in the payload.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_mission::{
    CanonicalTemplate, DependencyFinalizePlan, MissionCreateOptions, MissionCreatePlan,
    MissionCreateRequest, MissionIdentity, MissionRetention, MissionTemplateKind, MissionTemplates,
    MissionTopology,
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

/// Finalize `tasks.md` dependencies into WP frontmatter.
pub fn finalize_tasks(
    root: &Path,
    handle: Option<&str>,
    validate_only: bool,
    json_output: bool,
) -> Result<Value, CompatError> {
    let handle = handle.map(str::trim).filter(|value| !value.is_empty());
    let Some(handle) = handle else {
        return Err(CompatError::domain("--mission <slug> is required"));
    };
    let mission = resolve_mission(root, Some(handle))?;
    let tasks_dir = super::support::tasks_dir(&mission);
    if !tasks_dir.is_dir() {
        return Err(CompatError::domain(format!(
            "Tasks directory not found: {}",
            tasks_dir.display()
        )));
    }
    let tasks_md = mission.spec_dir().join("tasks.md");
    if !tasks_md.is_file() {
        return Err(CompatError::domain(format!(
            "tasks.md not found: {}",
            tasks_md.display()
        )));
    }

    let plan = DependencyFinalizePlan::compute(&tasks_md, &tasks_dir)
        .map_err(|error| CompatError::domain(format!("could not parse tasks.md: {error}")))?;

    let mut dependencies = Map::new();
    for (wp, deps) in plan.effective_dependencies() {
        let values: Vec<Value> = deps.iter().map(|dep| json!(dep.as_str())).collect();
        dependencies.insert(wp.as_str().to_owned(), Value::Array(values));
    }

    if validate_only {
        let payload = json!({
            "result": "validation_passed",
            "mission_slug": mission.slug().as_str(),
            "validate_only": true,
            "updated_wps": plan.updated_wps(),
            "preserved_wps": plan.preserved_wps(),
            "unchanged_wps": plan.unchanged_wps(),
            "dependencies": dependencies,
            "committed": false,
            "spec_kitty_version": env!("CARGO_PKG_VERSION"),
        });
        if json_output {
            emit_json(&payload)
                .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
        } else {
            println!("tasks validation passed: {}", mission.slug().as_str());
        }
        return Ok(payload);
    }

    let receipt = plan
        .apply(&tasks_dir)
        .map_err(|error| CompatError::domain(format!("could not finalize tasks: {error}")))?;
    let updated: Vec<Value> = receipt
        .updated()
        .iter()
        .map(|path| json!(path_string(path)))
        .collect();
    let payload = json!({
        "result": "success",
        "mission_slug": mission.slug().as_str(),
        "updated_wps": plan.updated_wps(),
        "preserved_wps": plan.preserved_wps(),
        "unchanged_wps": plan.unchanged_wps(),
        "updated_files": updated,
        "dependencies": dependencies,
        "committed": false,
        "spec_kitty_version": env!("CARGO_PKG_VERSION"),
    });
    if json_output {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "✓ Finalized {}: {} updated, {} preserved, {} unchanged",
            mission.slug().as_str(),
            plan.updated_wps().len(),
            plan.preserved_wps().len(),
            plan.unchanged_wps().len()
        );
    }
    Ok(payload)
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

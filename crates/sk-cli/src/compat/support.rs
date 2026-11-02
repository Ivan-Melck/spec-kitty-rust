//! Shared resolution, template and output helpers for the compatibility CLI.
//!
//! These helpers are deliberately boring: the domain crates own identifiers,
//! mission inventory, replay and mutation planning. This module only answers
//! "which project root?", "which mission does this handle name?", "where is the
//! canonical template?" and "how is a refusal rendered?".

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::discover_project_root;
use sk_mission::{Mission, MissionTemplateKind, list_missions};

/// A typed compatibility-route failure.
///
/// `exit` mirrors the upstream process class: `1` for a domain/data refusal,
/// `2` for a malformed invocation or an unsupported required option. The
/// machine payload is emitted as exactly one JSON document in `--json` mode.
#[derive(Debug)]
pub struct CompatError {
    message: String,
    code: Option<&'static str>,
    exit: u8,
    extra: Map<String, Value>,
}

impl CompatError {
    /// A domain/data refusal (exit 1).
    pub fn domain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            exit: 1,
            extra: Map::new(),
        }
    }

    /// A domain refusal with a stable machine code.
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
            exit: 1,
            extra: Map::new(),
        }
    }

    /// A usage refusal (exit 2), used for unsupported required options.
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            exit: 2,
            extra: Map::new(),
        }
    }

    /// An unsupported surface with a stable machine code (exit 2).
    pub fn unsupported(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
            exit: 2,
            extra: Map::new(),
        }
    }

    /// Attach a machine-readable field.
    pub fn with(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// Process exit code.
    pub fn exit(&self) -> u8 {
        self.exit
    }

    /// JSON error document, matching the upstream `{"error": ...}` shape and
    /// adding `error_code` only when the route defines one.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("error".to_owned(), Value::String(self.message.clone()));
        if let Some(code) = self.code {
            map.insert("error_code".to_owned(), Value::String(code.to_owned()));
        }
        for (key, value) in &self.extra {
            map.entry(key.clone()).or_insert_with(|| value.clone());
        }
        Value::Object(map)
    }

    /// Human-readable diagnostic.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Print one compact JSON document to stdout.
pub fn emit_json(value: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    match serde_json::to_writer(&mut stdout, value) {
        Ok(()) => {}
        Err(error) if error.io_error_kind() == Some(io::ErrorKind::BrokenPipe) => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    match stdout.write_all(b"\n") {
        Ok(()) => stdout.flush(),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}

/// Print one indented JSON document to stdout (the upstream `indent=2` sites).
pub fn emit_json_pretty(value: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    match serde_json::to_writer_pretty(&mut stdout, value) {
        Ok(()) => {}
        Err(error) if error.io_error_kind() == Some(io::ErrorKind::BrokenPipe) => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    match stdout.write_all(b"\n") {
        Ok(()) => stdout.flush(),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}

/// Discover the project root from the current directory.
///
/// The compatibility surface has no `--project-root`; it follows the same
/// discovery contract as upstream (walk upward for `.kittify`, `kitty-specs`
/// or `.git`, honouring `SPECIFY_REPO_ROOT`).
pub fn project_root() -> Result<PathBuf, CompatError> {
    let current = env::current_dir()
        .map_err(|error| CompatError::domain(format!("cannot read current directory: {error}")))?;
    discover_project_root(&current)
        .map_err(|_| CompatError::coded("PROJECT_ROOT_NOT_FOUND", "Could not locate project root"))
}

/// Discover the root, requiring `.kittify/` (an initialised project).
pub fn initialized_root() -> Result<PathBuf, CompatError> {
    let root = project_root()?;
    if !root.join(".kittify").is_dir() {
        return Err(CompatError::coded(
            "SPEC_KITTY_REPO_NOT_INITIALIZED",
            format!(
                "{} is not an initialized Spec Kitty project (missing .kittify/); run `spec-kitty init` first",
                root.display()
            ),
        ));
    }
    Ok(root)
}

/// Mission selection failure with the upstream discovery payload.
pub fn mission_selection_error(root: &Path, handle: Option<&str>) -> CompatError {
    let missions = list_missions(root).unwrap_or_default();
    match handle {
        Some(handle) => CompatError::coded(
            "MISSION_NOT_FOUND",
            format!("Mission not found: '{handle}'"),
        )
        .with(
            "next_step",
            json!("Run 'spec-kitty native mission list' to see available missions."),
        )
        .with(
            "remediation",
            json!("Run 'spec-kitty native mission list' to see available missions."),
        ),
        None => {
            let available: Vec<Value> = missions
                .iter()
                .map(|mission| {
                    json!({
                        "mission_slug": mission.slug().as_str(),
                        "mid8": mission.mid8(),
                        "friendly_name": mission.friendly_name(),
                    })
                })
                .collect();
            if available.is_empty() {
                CompatError::coded(
                    "NO_MISSIONS_FOUND",
                    "No missions exist in this repository; create one with `spec-kitty specify <name>`.",
                )
                .with("next_step", json!("Run 'spec-kitty specify <name>'."))
            } else {
                CompatError::coded(
                    "MISSION_SELECTION_REQUIRED",
                    "Multiple missions found; pass --mission <handle> to select one",
                )
                .with("available_missions", Value::Array(available))
                .with("next_step", json!("Re-run with --mission <handle>."))
            }
        }
    }
}

/// Resolve a mission selected by full directory name, slug, mid8 prefix, full
/// ULID, or `NNN-` numeric prefix. Ambiguity is a refusal, never a guess.
pub fn resolve_mission(root: &Path, handle: Option<&str>) -> Result<Mission, CompatError> {
    let missions = list_missions(root).map_err(|error| {
        CompatError::domain(format!("could not read mission inventory: {error}"))
    })?;

    let Some(handle) = handle.map(str::trim).filter(|value| !value.is_empty()) else {
        return match missions.as_slice() {
            [] => Err(mission_selection_error(root, None)),
            [only] => Ok(only.clone()),
            _ => Err(mission_selection_error(root, None)),
        };
    };

    let mut matches: Vec<&Mission> = Vec::new();
    for mission in &missions {
        let slug = mission.slug().as_str();
        if slug == handle
            || slug.starts_with(&format!("{handle}-"))
            || mission.mid8() == handle
            || mission.id().as_str() == handle
            || mission.mid8().starts_with(handle)
            || mission.id().as_str().starts_with(handle)
        {
            matches.push(mission);
        }
    }
    matches.sort_by(|left, right| left.slug().as_str().cmp(right.slug().as_str()));
    matches.dedup_by(|left, right| left.spec_dir() == right.spec_dir());

    match matches.as_slice() {
        [] => Err(mission_selection_error(root, Some(handle))),
        [only] => Ok((*only).clone()),
        many => {
            let candidates: Vec<Value> = many
                .iter()
                .map(|mission| {
                    json!({
                        "mission_slug": mission.slug().as_str(),
                        "mid8": mission.mid8(),
                        "friendly_name": mission.friendly_name(),
                    })
                })
                .collect();
            Err(CompatError::coded(
                "MISSION_SELECTOR_AMBIGUOUS",
                format!("Mission handle '{handle}' matches more than one mission"),
            )
            .with("handle", json!(handle))
            .with("candidates", Value::Array(candidates)))
        }
    }
}

/// The mission's work-package task directory (`tasks/`).
pub fn tasks_dir(mission: &Mission) -> PathBuf {
    mission.spec_dir().join("tasks")
}

/// The canonical status event-log path.
pub fn events_path(mission: &Mission) -> PathBuf {
    mission.spec_dir().join(sk_status::EVENTS_FILENAME)
}

/// Read the mission's frontmatter work-package files.
pub fn work_packages(mission: &Mission) -> Result<Vec<sk_mission::WorkPackage>, CompatError> {
    let directory = tasks_dir(mission);
    match fs::metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CompatError::domain(format!(
                "Tasks directory is not a directory: {}",
                directory.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CompatError::domain(format!(
                "could not inspect {}: {error}",
                directory.display()
            )));
        }
    }
    let mut packages = Vec::new();
    let entries = fs::read_dir(&directory).map_err(|error| {
        CompatError::domain(format!("could not read {}: {error}", directory.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CompatError::domain(format!("could not read {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !name.starts_with("WP") || !name.ends_with(".md") {
            continue;
        }
        if name.eq_ignore_ascii_case("readme.md") {
            continue;
        }
        let package = sk_mission::WorkPackage::from_file(&path).map_err(|error| {
            CompatError::domain(format!(
                "could not parse work package {}: {error}",
                path.display()
            ))
        })?;
        packages.push(package);
    }
    packages.sort_by(|left, right| left.id().as_str().cmp(right.id().as_str()));
    Ok(packages)
}

/// Work-package lanes from the canonical event log.
///
/// An absent log is "no canonical lanes yet" (upstream reports every WP as
/// `planned`); a corrupt log is a refusal, never a silent fallback.
pub fn canonical_lanes(
    mission: &Mission,
) -> Result<std::collections::BTreeMap<String, String>, CompatError> {
    let path = events_path(mission);
    let rows = match sk_status::read_raw_events(&path) {
        Ok(rows) => rows,
        Err(sk_status::SnapshotError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Default::default());
        }
        Err(error) => {
            return Err(CompatError::domain(format!(
                "could not read {}: {error}",
                path.display()
            )));
        }
    };
    if rows.is_empty() {
        return Ok(Default::default());
    }
    let snapshot = sk_status::reduce_rows(&rows).map_err(|error| {
        CompatError::domain(format!("could not reduce {}: {error}", path.display()))
    })?;
    let mut lanes = std::collections::BTreeMap::new();
    for (wp_id, state) in &snapshot.work_packages {
        if let Some(lane) = state.lane_string() {
            lanes.insert(wp_id.clone(), lane.to_owned());
        }
    }
    Ok(lanes)
}

/// The human-friendly title upstream derives from a slug when none is given.
pub fn humanize_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The current Git branch of `root`, if the directory is a repository.
pub fn current_branch(root: &Path) -> Option<String> {
    sk_git::GitRepository::open(root)
        .ok()
        .and_then(|repository| repository.current_branch().ok().flatten())
}

/// A canonical mission template resolved through the documented tiers.
#[derive(Clone, Debug)]
pub struct ResolvedMissionTemplate {
    /// Absolute template path.
    pub path: PathBuf,
    /// Template text, newline-normalized.
    pub text: String,
    /// Which precedence tier supplied it (`override`, `legacy`, `org`,
    /// `global_mission`, `global`, `package`).
    pub tier: &'static str,
}

impl ResolvedMissionTemplate {
    /// The resolver id recorded in authoring plans.
    pub fn id(&self) -> String {
        format!("{}:{}", self.tier, self.path.display())
    }
}

fn template_file_name(kind: MissionTemplateKind, mission_type: &str) -> String {
    let base = match kind {
        MissionTemplateKind::Spec => "spec-template.md",
        MissionTemplateKind::Plan => "plan-template.md",
        MissionTemplateKind::Tasks => "tasks-template.md",
        MissionTemplateKind::WorkPackage => "task-prompt-template.md",
    };
    // The pack rename rule: a non-default mission type ships
    // `<type>-spec-template.md` / `<type>-plan-template.md`.
    if mission_type != "software-dev"
        && matches!(kind, MissionTemplateKind::Spec | MissionTemplateKind::Plan)
    {
        format!("{mission_type}-{base}")
    } else {
        base.to_owned()
    }
}

/// The built-in pack root used for package-tier template resolution.
///
/// `SPEC_KITTY_PACKS_ROOT` wins (the same environment hook the installer and
/// the existing acceptance tests use). Otherwise the checkout-local
/// `packs/built-in` tree is used, then the directory recorded at build time.
pub fn pack_root() -> Option<PathBuf> {
    if let Some(value) = env::var_os("SPEC_KITTY_PACKS_ROOT") {
        let path = PathBuf::from(value);
        if path.join("missions").is_dir() {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(current) = env::current_dir() {
        let mut cursor = Some(current.as_path());
        while let Some(directory) = cursor {
            candidates.push(directory.join("packs/built-in"));
            cursor = directory.parent();
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packs/built-in"));
    candidates
        .into_iter()
        .find(|candidate| candidate.join("missions").is_dir())
}

fn home_candidates(kind: MissionTemplateKind, mission_type: &str) -> Vec<(PathBuf, &'static str)> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let name = template_file_name(kind, mission_type);
    vec![
        (
            home.join(".kittify")
                .join("missions")
                .join(mission_type)
                .join("templates")
                .join(&name),
            "global_mission",
        ),
        (
            home.join(".kittify").join("templates").join(&name),
            "global",
        ),
    ]
}

/// Resolve a mission template through the upstream precedence chain.
///
/// Tiers implemented: project override, project legacy, global mission,
/// global, package (the built-in pack). The org-pack tier is not yet wired
/// (missions from `.kittify/config.yaml` are not read here) and is recorded as
/// an inventory gap rather than silently skipped.
pub fn resolve_mission_template(
    root: &Path,
    mission_type: &str,
    kind: MissionTemplateKind,
) -> Result<ResolvedMissionTemplate, CompatError> {
    let name = template_file_name(kind, mission_type);
    let mut candidates: Vec<(PathBuf, &'static str)> = vec![
        (
            root.join(".kittify")
                .join("overrides")
                .join("templates")
                .join(&name),
            "override",
        ),
        (
            root.join(".kittify").join("templates").join(&name),
            "legacy",
        ),
    ];
    candidates.extend(home_candidates(kind, mission_type));
    if let Some(pack) = pack_root() {
        candidates.push((
            pack.join("missions")
                .join(mission_type)
                .join("templates")
                .join(&name),
            "package",
        ));
        // The canonical software-dev pack uses the unsuffixed name; a
        // non-default type may only carry the suffixed file.
        candidates.push((
            pack.join("missions")
                .join(mission_type)
                .join("templates")
                .join(match kind {
                    MissionTemplateKind::Spec => "spec-template.md",
                    MissionTemplateKind::Plan => "plan-template.md",
                    MissionTemplateKind::Tasks => "tasks-template.md",
                    MissionTemplateKind::WorkPackage => "task-prompt-template.md",
                }),
            "package",
        ));
    }

    for (path, tier) in candidates {
        match sk_kernel::read_bounded(&path, 4 * 1024 * 1024) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).map_err(|error| {
                    CompatError::domain(format!(
                        "template {} is not UTF-8: {error}",
                        path.display()
                    ))
                })?;
                return Ok(ResolvedMissionTemplate {
                    path,
                    text: text.replace("\r\n", "\n").replace('\r', "\n"),
                    tier,
                });
            }
            Err(sk_kernel::KernelError::Io { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => {
                return Err(CompatError::domain(format!(
                    "could not read template {}: {error}",
                    path.display()
                )));
            }
        }
    }

    Err(CompatError::coded(
        "TEMPLATE_NOT_FOUND",
        format!(
            "could not resolve the canonical {kind} template for mission type '{mission_type}'; \
             looked in the project overrides, the global ~/.kittify tiers, and the built-in pack"
        ),
    )
    .with("template_name", json!(name))
    .with("mission_type", json!(mission_type)))
}

/// Build the branch contract upstream injects into create/plan/next payloads.
#[allow(clippy::too_many_arguments)]
pub fn inject_branch_contract(
    payload: &mut Map<String, Value>,
    target_branch: &str,
    current_branch: &str,
    expected_checkout_branch: &str,
) {
    let now = sk_kernel::clock::now_utc_iso();
    let resolved_current = if current_branch.trim().is_empty() {
        target_branch
    } else {
        current_branch
    };
    let expected = if expected_checkout_branch.trim().is_empty() {
        target_branch
    } else {
        expected_checkout_branch
    };
    let matches = resolved_current == expected;
    let summary = format!(
        "Current branch at workflow start: {resolved_current}. Planning/base branch for this mission: {target_branch}. Completed changes must merge into {target_branch}."
    );
    let mut runtime_vars = Map::new();
    runtime_vars.insert("now_utc_iso".into(), json!(now));
    runtime_vars.insert("current_branch".into(), json!(resolved_current));
    runtime_vars.insert("target_branch".into(), json!(target_branch));
    runtime_vars.insert("base_branch".into(), json!(target_branch));
    runtime_vars.insert("planning_base_branch".into(), json!(target_branch));
    runtime_vars.insert("merge_target_branch".into(), json!(target_branch));
    runtime_vars.insert("branch_matches_target".into(), json!(matches));
    runtime_vars.insert("branch_strategy_summary".into(), json!(summary));

    let mut branch_context = Map::new();
    branch_context.insert("current_branch".into(), json!(resolved_current));
    branch_context.insert("target_branch".into(), json!(target_branch));
    branch_context.insert("base_branch".into(), json!(target_branch));
    branch_context.insert("planning_base_branch".into(), json!(target_branch));
    branch_context.insert("merge_target_branch".into(), json!(target_branch));
    branch_context.insert("expected_checkout_branch".into(), json!(expected));
    branch_context.insert("matches_target".into(), json!(matches));
    branch_context.insert("branch_strategy_summary".into(), json!(summary));

    payload.insert("current_branch".into(), json!(resolved_current));
    payload.insert("CURRENT_BRANCH".into(), json!(resolved_current));
    payload.insert("target_branch".into(), json!(target_branch));
    payload.insert("base_branch".into(), json!(target_branch));
    payload.insert("TARGET_BRANCH".into(), json!(target_branch));
    payload.insert("BASE_BRANCH".into(), json!(target_branch));
    payload.insert("planning_base_branch".into(), json!(target_branch));
    payload.insert("PLANNING_BASE_BRANCH".into(), json!(target_branch));
    payload.insert("merge_target_branch".into(), json!(target_branch));
    payload.insert("MERGE_TARGET_BRANCH".into(), json!(target_branch));
    payload.insert("EXPECTED_TARGET_BRANCH".into(), json!(target_branch));
    payload.insert("EXPECTED_BASE_BRANCH".into(), json!(target_branch));
    payload.insert("branch_matches_target".into(), json!(matches));
    payload.insert("BRANCH_MATCHES_TARGET".into(), json!(matches));
    payload.insert("branch_strategy_summary".into(), json!(summary));
    payload.insert("runtime_vars".into(), Value::Object(runtime_vars));
    payload.insert("NOW_UTC_ISO".into(), json!(now));
    payload.insert("branch_context".into(), Value::Object(branch_context));
}

/// Render a path losslessly enough for JSON (diagnostics, not a transport).
pub fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Parse an `--ai` selection the way upstream does: lower-case, `;` or `,`
/// separated, de-duplicated, and validated against the install crate's roster.
pub fn parse_agents(raw: Option<&str>) -> Result<Vec<String>, CompatError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut agents = Vec::new();
    let mut invalid = Vec::new();
    for part in raw.replace(';', ",").split(',').map(str::trim) {
        if part.is_empty() {
            continue;
        }
        let key = part.to_ascii_lowercase();
        if sk_install::agent_surface(&key).is_err() {
            invalid.push(key);
            continue;
        }
        if !agents.contains(&key) {
            agents.push(key);
        }
    }
    if !invalid.is_empty() {
        return Err(CompatError::usage(format!(
            "Invalid AI assistant(s): {}. Choose from: {}",
            invalid.join(", "),
            sk_install::valid_keys()
        )));
    }
    if agents.is_empty() {
        return Err(CompatError::usage(
            "--ai flag did not contain any valid agent identifiers",
        ));
    }
    Ok(agents)
}

/// A stable string for an `OsString` argument list (only used in messages).
pub fn display_args(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_slug_matches_the_upstream_title_shape() {
        assert_eq!(humanize_slug("user-authentication"), "User Authentication");
        assert_eq!(humanize_slug("a_b__c"), "A B C");
    }

    #[test]
    fn template_file_name_uses_the_pack_rename_rule() {
        assert_eq!(
            template_file_name(MissionTemplateKind::Spec, "software-dev"),
            "spec-template.md"
        );
        assert_eq!(
            template_file_name(MissionTemplateKind::Spec, "research"),
            "research-spec-template.md"
        );
        assert_eq!(
            template_file_name(MissionTemplateKind::WorkPackage, "research"),
            "task-prompt-template.md"
        );
    }
}

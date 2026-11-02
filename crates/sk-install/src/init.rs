//! Native project initialization: fresh scaffold, manifest-owned refresh,
//! interrupted-delivery recovery, and the full delivery surface.
//!
//! `.kittify/config.yaml` is the idempotency boundary (upstream `init.py`): its
//! presence means "already initialized", so a re-run verifies and repairs
//! managed surfaces instead of re-scaffolding. A fresh run writes one recovery
//! record (`.kittify/init-command-skills.pending.json`) *before* the first
//! managed byte and clears it only after the whole delivery is durable.
//!
//! A **full** fresh run (when [`ProjectAssets`] is supplied) performs the
//! upstream sequence:
//!
//! 1. install the verified template tree (`memory/`, `templates/`, `missions/`,
//!    `AGENTS.md`, `.claudeignore`), backing up operator subtrees;
//! 2. write the authored configuration (`config.yaml`);
//! 3. plan and commit the command surfaces (existing contract);
//! 4. install the charter skill catalog (native roots + shared manifest);
//! 5. protect runtime/agent paths in `.gitignore`;
//! 6. seed `.gitattributes` merge-driver attributes and expose the clone-local
//!    git-config wiring plan to the Git owner;
//! 7. provision mission-type activations from the built-in `default` preset;
//! 8. save/stamp `.kittify/metadata.yaml` additively (comments preserved);
//! 9. mint/persist project identity and append the `ProjectInitialized` event;
//! 10. seed the migration ledger (tracked, plus proven clone-local effects);
//! 11. restore the Vibe skill-path pointer when Vibe is selected;
//! 12. run the managed-skill surface repair, then remove only proven
//!     run-created scratch.
//!
//! Optional steps report warnings; the required ones (template tree when
//! configured, command commit, config, `.gitignore`, activation provisioning)
//! fail closed and leave the pending record for a safe resume.
//!
//! Without [`ProjectAssets`], the previous tranche's behavior is retained: the
//! command/manifest/config/metadata core runs and every asset-dependent step is
//! skipped. This is additive, so an existing caller keeps compiling and a
//! concurrent CLI integrator can adopt the full path at its own pace.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_yaml_ng::Value as YamlValue;

use crate::agent::{AgentSurface, agent_surface};
use crate::error::InstallError;
use crate::ledger::{MigrationCatalog, MigrationLedger, clone_local_effect_in_place};
use crate::metadata::{ProjectMetadata, stamp_schema_metadata};
use crate::plan::{ApplicationReport, InstallPlan, PlanOptions, PlanSummary, now_utc_iso};
use crate::render::{RenderedAsset, TemplateSource, render_asset};
use crate::skills::{SkillCatalog, SkillInstallOptions};
use crate::template_tree::{
    TemplateTreeOptions, TemplateTreeSource, cleanup_init_scratch, install_template_tree,
};

/// Project-relative path of the recovery record written by a fresh init.
pub const PENDING_DELIVERY_REL: &str = ".kittify/init-command-skills.pending.json";
/// Project-relative path of the authored project configuration.
pub const CONFIG_REL: &str = ".kittify/config.yaml";
/// Project-relative path of the stamped project metadata.
pub const METADATA_REL: &str = ".kittify/metadata.yaml";
/// Project-relative memory directory created for a fresh project.
pub const MEMORY_REL: &str = ".kittify/memory";

/// The project schema version a fresh init stamps (upstream `CURRENT_SCHEMA_VERSION`).
pub const CURRENT_SCHEMA_VERSION: u32 = crate::metadata::CURRENT_SCHEMA_VERSION as u32;
/// The capability flags a fresh init stamps (upstream `CURRENT_SCHEMA_CAPABILITIES`).
pub const CURRENT_SCHEMA_CAPABILITIES: [&str; 4] = [
    "canonical_context",
    "event_log_authority",
    "ownership_manifest",
    "thin_shims",
];

/// The environment provenance stamped into `metadata.yaml`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectEnvironment {
    /// The interpreter that initialized the project, when known.
    pub python_version: String,
    /// The platform name.
    pub platform: String,
    /// The platform release.
    pub platform_version: String,
}

impl ProjectEnvironment {
    /// Probe the local environment without any external process.
    pub fn detect() -> Self {
        let platform = std::env::consts::OS.to_string();
        let release = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let platform_version = if release.is_empty() {
            platform.clone()
        } else {
            format!("{platform}-{release}")
        };
        Self {
            python_version: String::new(),
            platform,
            platform_version,
        }
    }
}

/// The optional verified inputs for a full init.
///
/// Every asset is explicit. There is no bundled fallback: an absent catalog or
/// template tree is skipped with a warning (or refused when the selected agents
/// need it), never invented.
#[derive(Clone, Debug)]
pub struct ProjectAssets {
    /// The verified template tree.
    pub template_tree: TemplateTreeSource,
    /// Template copy options.
    pub template_options: TemplateTreeOptions,
    /// The charter skill catalog, when available.
    pub skills: Option<SkillCatalog>,
    /// The built-in `default` activation preset (`presets/default.yaml`).
    pub activation_preset: Option<PathBuf>,
    /// Optional missions pack pin and its lockfile path, enforced before copy.
    pub missions_pin: Option<(PathBuf, crate::pin::PackPin)>,
    /// Environment provenance stamped into metadata.
    pub environment: ProjectEnvironment,
}

impl ProjectAssets {
    /// Build a full asset set from a checkout/install layout.
    ///
    /// Requires `src/charter/offering/templates` and `packs/built-in/missions`;
    /// discovers `src/charter/offering/skills` and
    /// `packs/built-in/presets/default.yaml` when present.
    pub fn discover(checkout: &Path) -> Result<Self, InstallError> {
        let template_tree = TemplateTreeSource::discover(checkout)?;
        let skills = {
            let root = checkout.join("src/charter/offering/skills");
            if root.is_dir() {
                Some(SkillCatalog::discover(&root)?)
            } else {
                None
            }
        };
        let preset = checkout.join("packs/built-in/presets/default.yaml");
        Ok(Self {
            template_tree,
            template_options: TemplateTreeOptions::default(),
            skills,
            activation_preset: preset.is_file().then_some(preset),
            missions_pin: None,
            environment: ProjectEnvironment::detect(),
        })
    }

    /// Attach a missions pack pin enforced before the tree is copied.
    pub fn with_missions_pin(
        mut self,
        pin_path: impl Into<PathBuf>,
        pin: crate::pin::PackPin,
    ) -> Self {
        self.missions_pin = Some((pin_path.into(), pin));
        self
    }

    /// Resolve shared assets from `kittify_home` when it is populated, falling
    /// back to a full copy otherwise (upstream's `use_global` decision).
    pub fn with_global_runtime_home(mut self, kittify_home: impl Into<PathBuf>) -> Self {
        self.template_tree = self.template_tree.with_global_home(kittify_home);
        self
    }

    /// Attach an explicit skill catalog.
    pub fn with_skills(mut self, catalog: SkillCatalog) -> Self {
        self.skills = Some(catalog);
        self
    }

    /// Attach the activation preset path.
    pub fn with_activation_preset(mut self, path: impl Into<PathBuf>) -> Self {
        self.activation_preset = Some(path.into());
        self
    }
}

/// A complete initialization request.
#[derive(Clone, Debug)]
pub struct InitRequest {
    /// Project root to initialize (must already exist as a directory).
    pub project_root: PathBuf,
    /// Requested agent keys; empty means "use the saved configuration, else error".
    pub agents: Vec<String>,
    /// The installed CLI version recorded in ownership manifests.
    pub version: String,
    /// Template source (a fetched/verified pack's `missions/` root).
    pub templates: Option<TemplateSource>,
    /// Plan options (timestamp, regenerate/adopt policy).
    pub options: PlanOptions,
    /// How long to wait for the project lock.
    pub lock_wait: Duration,
    /// Optional full-delivery assets (template tree, skills, preset, pin).
    pub assets: Option<ProjectAssets>,
    /// The running core version, for `requires_core` enforcement.
    pub core_version: Option<String>,
    /// The environment provenance to stamp.
    pub environment: Option<ProjectEnvironment>,
}

impl InitRequest {
    /// Build a fresh request with production defaults.
    pub fn new(
        project_root: impl Into<PathBuf>,
        agents: Vec<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            project_root: project_root.into(),
            agents,
            version: version.into(),
            templates: None,
            options: PlanOptions::new(),
            lock_wait: Duration::from_secs(10),
            assets: None,
            core_version: None,
            environment: None,
        }
    }

    /// Attach the template source used for prompt-backed commands.
    pub fn with_templates(mut self, templates: TemplateSource) -> Self {
        self.templates = Some(templates);
        self
    }

    /// Attach plan options.
    pub fn with_options(mut self, options: PlanOptions) -> Self {
        self.options = options;
        self
    }

    /// Set the project-lock wait budget.
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    /// Attach the full-delivery assets.
    pub fn with_project_assets(mut self, assets: ProjectAssets) -> Self {
        self.assets = Some(assets);
        self
    }

    /// Set the running core version used for `requires_core` enforcement.
    pub fn with_core_version(mut self, version: impl Into<String>) -> Self {
        self.core_version = Some(version.into());
        self
    }

    /// Override the stamped environment provenance.
    pub fn with_environment(mut self, environment: ProjectEnvironment) -> Self {
        self.environment = Some(environment);
        self
    }

    fn effective_templates(&self) -> Option<TemplateSource> {
        self.templates.clone().or_else(|| {
            self.assets.as_ref().and_then(|assets| {
                assets
                    .template_tree
                    .missions_root()
                    .map(TemplateSource::new)
            })
        })
    }
}

/// The recovery record naming the agent set whose delivery is incomplete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDelivery {
    agents: Vec<String>,
}

impl PendingDelivery {
    /// Read the record for `project_root`, validating it strictly.
    ///
    /// An absent record returns `Ok(None)`. A non-regular file (symlink,
    /// directory), unknown schema, non-canonical ordering, or an agent that is
    /// not a command-skill agent is a typed failure: recovery never guesses.
    pub fn load(project_root: &Path) -> Result<Option<Self>, InstallError> {
        let path = project_root.join(PENDING_DELIVERY_REL);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(InstallError::io("inspect", &path, source)),
        };
        if !metadata.file_type().is_file() {
            return Err(InstallError::PendingDeliveryInvalid {
                path,
                detail: "record is not a regular file".to_string(),
            });
        }
        let raw = sk_kernel::read_bounded(&path, 64 * 1024)
            .map_err(|error| InstallError::io("read", &path, std::io::Error::other(error)))?;
        let value: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|error| InstallError::PendingDeliveryInvalid {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        let object = value
            .as_object()
            .ok_or_else(|| InstallError::PendingDeliveryInvalid {
                path: path.clone(),
                detail: "record must be a JSON object".to_string(),
            })?;
        if object.len() != 2
            || !object.contains_key("schema_version")
            || !object.contains_key("agents")
        {
            return Err(InstallError::PendingDeliveryInvalid {
                path,
                detail: "record must contain exactly schema_version and agents".to_string(),
            });
        }
        let version = object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| InstallError::PendingDeliveryInvalid {
                path: path.clone(),
                detail: "schema_version must be an integer".to_string(),
            })?;
        if version != 1 {
            return Err(InstallError::PendingDeliveryInvalid {
                path,
                detail: format!("unsupported schema_version {version}"),
            });
        }
        let raw_agents = object
            .get("agents")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| InstallError::PendingDeliveryInvalid {
                path: path.clone(),
                detail: "agents must be an array".to_string(),
            })?;
        let mut agents = Vec::with_capacity(raw_agents.len());
        for agent in raw_agents {
            let key = agent
                .as_str()
                .ok_or_else(|| InstallError::PendingDeliveryInvalid {
                    path: path.clone(),
                    detail: "every agents entry must be a string".to_string(),
                })?;
            let surface = agent_surface(key).map_err(|_| InstallError::PendingDeliveryInvalid {
                path: path.clone(),
                detail: format!("{key:?} is not a configured command-skill agent"),
            })?;
            if !surface.installs_command_skills() {
                return Err(InstallError::PendingDeliveryInvalid {
                    path: path.clone(),
                    detail: format!("{key:?} is not a command-skill agent"),
                });
            }
            agents.push(key.to_string());
        }
        let mut canonical = agents.clone();
        canonical.sort();
        canonical.dedup();
        if agents.is_empty() || agents != canonical {
            return Err(InstallError::PendingDeliveryInvalid {
                path,
                detail: "agents must be a non-empty sorted, duplicate-free list".to_string(),
            });
        }
        Ok(Some(Self { agents }))
    }

    /// Write (or confirm) the recovery record for `agents`.
    ///
    /// Writing an identical record is a no-op; writing a different one over an
    /// existing record is refused, because that would silently re-target a
    /// half-finished delivery.
    pub fn write(project_root: &Path, agents: &[String]) -> Result<Self, InstallError> {
        let mut canonical: Vec<String> = agents.to_vec();
        canonical.sort();
        canonical.dedup();
        if canonical.is_empty() {
            return Err(InstallError::NoAgentsSelected);
        }
        let record = Self { agents: canonical };
        if let Some(existing) = Self::load(project_root)? {
            if existing == record {
                return Ok(record);
            }
            return Err(InstallError::PendingDeliveryInvalid {
                path: project_root.join(PENDING_DELIVERY_REL),
                detail: format!(
                    "an interrupted delivery for {:?} is pending; re-run init without changing --ai to resume it",
                    existing.agents
                ),
            });
        }
        let path = project_root.join(PENDING_DELIVERY_REL);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        let mut document = serde_json::Map::new();
        document.insert("schema_version".to_string(), serde_json::Value::from(1_u64));
        document.insert(
            "agents".to_string(),
            serde_json::Value::Array(
                record
                    .agents
                    .iter()
                    .map(|agent| serde_json::Value::from(agent.clone()))
                    .collect(),
            ),
        );
        let mut bytes = serde_json::to_vec(&serde_json::Value::Object(document))
            .expect("pending record serialization cannot fail");
        bytes.push(b'\n');
        sk_kernel::atomic_write(&path, &bytes).map_err(|error| {
            InstallError::io(
                "write pending delivery",
                &path,
                std::io::Error::other(error),
            )
        })?;
        Ok(record)
    }

    /// Remove the recovery record after a completed delivery.
    pub fn clear(project_root: &Path) -> Result<(), InstallError> {
        let path = project_root.join(PENDING_DELIVERY_REL);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(InstallError::io("remove pending delivery", &path, source)),
        }
    }

    /// The recorded agent keys.
    pub fn agents(&self) -> &[String] {
        &self.agents
    }
}

/// The outcome of an initialization run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitOutcome {
    /// A fresh project scaffold was created.
    Initialized,
    /// The project was already initialized; managed surfaces were verified/refreshed.
    AlreadyInitialized,
    /// An interrupted command delivery was completed from the saved configuration.
    ResumedDelivery,
}

impl InitOutcome {
    /// Stable spelling for CLI output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initialized => "initialized",
            Self::AlreadyInitialized => "already_initialized",
            Self::ResumedDelivery => "resumed_delivery",
        }
    }
}

/// The result of an initialization run.
#[derive(Clone, Debug)]
pub struct InitReport {
    /// Which lifecycle outcome occurred.
    pub outcome: InitOutcome,
    /// Counts of the plan that was committed.
    pub summary: PlanSummary,
    /// What the commit actually changed.
    pub application: ApplicationReport,
    /// Whether a fresh `config.yaml` was created.
    pub config_written: bool,
    /// Whether a fresh `metadata.yaml` was created.
    pub metadata_written: bool,
    /// Warnings collected while planning (preserved edits, collisions, retired paths).
    pub warnings: Vec<String>,
}

/// The detailed effects of a full initialization (additive report).
#[derive(Clone, Debug, Default)]
pub struct InitDetails {
    /// The template mode used, when assets were supplied.
    pub template_mode: Option<crate::template_tree::TemplateMode>,
    /// Project-relative subtrees copied.
    pub template_copied: Vec<String>,
    /// Backup directories created.
    pub backups: Vec<String>,
    /// Whether this run created `.kittify/templates` (cleanup provenance).
    pub templates_created: bool,
    /// Skill files written / adopted / drifted (reported, not repaired).
    pub skills_written: Vec<String>,
    /// Skill files adopted because identical bytes were already present.
    pub skills_adopted: Vec<String>,
    /// Skill files whose managed bytes drifted.
    pub skills_drifted: Vec<String>,
    /// Whether `.gitignore` changed.
    pub gitignore_modified: bool,
    /// `.gitignore` entries added.
    pub gitignore_added: Vec<String>,
    /// Whether `.gitattributes` changed.
    pub attributes_written: bool,
    /// The typed clone-local merge-driver wiring plan (Git-owner work).
    pub merge_driver_plan: Option<crate::attributes::MergeDriverWiringPlan>,
    /// Whether mission-type activations were provisioned.
    pub activation_provisioned: bool,
    /// The resolved identity, when minted/loaded.
    pub identity: Option<crate::identity::ProjectIdentity>,
    /// Whether the `ProjectInitialized` event was appended.
    pub event_written: bool,
    /// Migration ids seeded into the ledger.
    pub ledger_seeded: Vec<String>,
    /// The tool-surface repair summary.
    pub surface: crate::skills::SurfaceRepairSummary,
    /// Scratch paths preserved because removal could not be proven.
    pub scratch_preserved: Vec<String>,
    /// Agent-folder security notice entries for the CLI to render.
    pub security_notice: Vec<crate::gitignore::AgentFolderNotice>,
}

/// Initialize, refresh, or resume a project.
pub fn initialize(request: &InitRequest) -> Result<InitReport, InstallError> {
    initialize_with_details(request).map(|(report, _)| report)
}

/// Initialize and return the detailed effect report.
pub fn initialize_with_details(
    request: &InitRequest,
) -> Result<(InitReport, InitDetails), InstallError> {
    let root = &request.project_root;
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        _ => {
            return Err(InstallError::RootNotDirectory { root: root.clone() });
        }
    }
    if crate::version::Version::parse(&request.version).is_err() {
        return Err(InstallError::VersionInvalid {
            value: request.version.clone(),
            detail: "the CLI version must be a PEP 440 version".to_string(),
        });
    }

    let config_path = root.join(CONFIG_REL);

    if config_path.exists() {
        let configured = read_configured_agents(root)?;
        let requested = if request.agents.is_empty() {
            Vec::new()
        } else {
            resolve_requested_agents(&request.agents)?
        };
        return refresh_or_resume(request, &configured, &requested);
    }

    if request.agents.is_empty() {
        return Err(InstallError::NoAgentsSelected);
    }
    let requested = resolve_requested_agents(&request.agents)?;
    fresh_initialize(request, &requested)
}

fn enforce_assets_pin(request: &InitRequest) -> Result<(), InstallError> {
    if let Some(assets) = &request.assets
        && let Some((pin_path, pin)) = &assets.missions_pin
    {
        if let Some(requires_core) = &pin.requires_core {
            let core_version = request.core_version.as_deref().ok_or_else(|| {
                InstallError::UpgradeRefused {
                    detail: format!(
                        "pack pin {} declares requires_core {requires_core}; init needs the running core version",
                        pin_path.display()
                    ),
                }
            })?;
            crate::upgrade::enforce_requires_core(pin_path, Some(requires_core), core_version)?;
        }
        let missions = assets.template_tree.missions_root().ok_or_else(|| {
            InstallError::TemplateTreeMissing {
                path: PathBuf::from("<missions>"),
                detail: "the pinned asset set has no mission tree".to_string(),
            }
        })?;
        let pack_root = missions
            .parent()
            .ok_or_else(|| InstallError::TemplateTreeUnverified {
                path: missions.to_path_buf(),
                detail: "mission root has no parent pack directory".to_string(),
            })?;
        pin.verify(pin_path, pack_root)?;
    }
    Ok(())
}

fn fresh_initialize(
    request: &InitRequest,
    agents: &[&'static AgentSurface],
) -> Result<(InitReport, InitDetails), InstallError> {
    let root = &request.project_root;
    let agent_keys: Vec<String> = agents.iter().map(|agent| agent.key.to_string()).collect();
    enforce_assets_pin(request)?;
    let templates = request.effective_templates();
    // Full delivery follows the current upstream surface: only the command-skill
    // roster installs command skills (into `.agents/skills/`); slash-command
    // files are no longer generated by init. The legacy path keeps its previous
    // 12-family command-file behavior for API compatibility.
    let rendered = render_assets(
        agents,
        &request.version,
        templates.as_ref(),
        request.assets.is_none(),
    )?;

    // Recovery record first: if this run dies, the next one resumes exactly this
    // agent set instead of re-scaffolding with a different selection. In full
    // mode the record names the command-skill roster only (the upstream
    // contract); native/shared charter skills are restored by surface repair on
    // the next run. A selection with no command-skill agent has no record,
    // exactly as upstream writes none.
    let pending_agents: Vec<String> = if request.assets.is_some() {
        agents
            .iter()
            .filter(|agent| agent.installs_command_skills())
            .map(|agent| agent.key.to_string())
            .collect()
    } else {
        agent_keys.clone()
    };
    let pending = if pending_agents.is_empty() {
        None
    } else {
        Some(PendingDelivery::write(root, &pending_agents)?)
    };

    let mut details = InitDetails::default();
    if let Some(assets) = &request.assets {
        let report = install_template_tree(root, &assets.template_tree, &assets.template_options)?;
        details.template_mode = report.mode;
        details.template_copied = report.copied;
        details.backups = report.backups;
        details.templates_created = report.templates_created;
    }

    let config_written = write_fresh_config(root, agents)?;
    std::fs::create_dir_all(root.join(MEMORY_REL)).map_err(|source| {
        InstallError::io("create memory directory", root.join(MEMORY_REL), source)
    })?;

    let plan = InstallPlan::plan(root, &request.version, &rendered, &request.options)?;
    let summary = plan.summary();
    let application = plan.commit(request.lock_wait)?;

    let mut warnings = plan.warnings().to_vec();
    warnings.extend(application.warnings.iter().cloned());

    let environment = request
        .environment
        .clone()
        .or_else(|| {
            request
                .assets
                .as_ref()
                .map(|assets| assets.environment.clone())
        })
        .unwrap_or_else(ProjectEnvironment::detect);
    let metadata = ProjectMetadata::for_new_project(
        &request.version,
        now_utc_iso(),
        &environment.python_version,
        &environment.platform,
        &environment.platform_version,
        None,
    );
    let metadata_written = match metadata.save(&root.join(".kittify")) {
        Ok(written) => written,
        Err(error) => {
            warnings.push(format!("Could not save project metadata: {error}"));
            false
        }
    };
    if let Err(error) = stamp_schema_metadata(&root.join(".kittify")) {
        warnings.push(format!("Could not stamp schema metadata: {error}"));
    }

    if let Some(assets) = &request.assets {
        if let Some(catalog) = &assets.skills {
            let options = SkillInstallOptions::default();
            match crate::skills::install_skills_for_agents(root, &agent_keys, catalog, &options) {
                Ok(report) => {
                    details.skills_written = report.written;
                    details.skills_adopted = report.adopted;
                    details.skills_drifted = report.drifted;
                }
                Err(error) => warnings.push(format!("Could not install charter skills: {error}")),
            }
        }
        let builtin_skill_names = assets
            .skills
            .as_ref()
            .map(SkillCatalog::names)
            .unwrap_or_default();
        let result = crate::gitignore::protect_all_agents(root, &agent_keys, &builtin_skill_names)?;
        details.gitignore_modified = result.modified;
        details.gitignore_added = result.entries_added;
        warnings.extend(result.warnings);
    } else {
        let result = crate::gitignore::protect_all_agents(root, &agent_keys, &[])?;
        details.gitignore_modified = result.modified;
        details.gitignore_added = result.entries_added;
        warnings.extend(result.warnings);
    }

    match crate::attributes::ensure_attribute_lines(root) {
        Ok(changed) => details.attributes_written = changed,
        Err(error) => warnings.push(format!("Could not seed .gitattributes: {error}")),
    }
    details.merge_driver_plan = crate::attributes::MergeDriverWiringPlan::plan(root).ok();

    if let Some(preset) = request
        .assets
        .as_ref()
        .and_then(|assets| assets.activation_preset.as_ref())
    {
        details.activation_provisioned =
            crate::activation::provision_mission_type_activations(root, preset)?;
    }

    match crate::identity::ensure_identity(root, request.lock_wait) {
        Ok(identity) => {
            if let Some(uuid) = identity.project_uuid {
                let payload = crate::lifecycle::payload_for_init(
                    uuid.to_string(),
                    identity.project_slug.clone(),
                    Some(request.version.clone()),
                );
                match crate::lifecycle::emit_project_initialized(root, &payload, request.lock_wait)
                {
                    Ok(event) => details.event_written = event.is_some(),
                    Err(error) => {
                        warnings.push(format!("Could not emit ProjectInitialized: {error}"))
                    }
                }
            }
            details.identity = Some(identity);
        }
        Err(error) => warnings.push(format!("Could not persist project identity: {error}")),
    }

    match MigrationCatalog::embedded() {
        Ok(catalog) => {
            let ledger = MigrationLedger::new(root);
            match ledger.seed_for_init(catalog, &request.version, |spec| {
                clone_local_effect_in_place(root, spec)
            }) {
                Ok(seeded) => details.ledger_seeded = seeded,
                Err(error) => warnings.push(format!("Could not seed migration ledger: {error}")),
            }
        }
        Err(error) => warnings.push(format!("Migration catalog unavailable: {error}")),
    }

    if agent_keys.iter().any(|key| key == "vibe") {
        match crate::vibe::ensure_project_skill_path(root) {
            Ok(_changed) => {}
            Err(error) => warnings.push(format!(
                "Could not write the Vibe skill-path pointer: {error}"
            )),
        }
    }

    if let Some(catalog) = request
        .assets
        .as_ref()
        .and_then(|assets| assets.skills.as_ref())
    {
        match crate::skills::run_surface_repair(root, &agent_keys, catalog, false) {
            Ok(summary) => details.surface = summary,
            Err(error) => warnings.push(format!("Tool surface repair failed: {error}")),
        }
    }

    let mut run_created: BTreeSet<PathBuf> = BTreeSet::new();
    if details.templates_created {
        run_created.insert(PathBuf::from(".kittify/templates"));
    }
    match cleanup_init_scratch(root, &run_created) {
        Ok(preserved) => details.scratch_preserved = preserved,
        Err(error) => warnings.push(format!("Could not clean init scratch: {error}")),
    }

    details.security_notice = crate::gitignore::agent_folder_notice(
        &agent_keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    // Only now is the delivery durable.
    PendingDelivery::clear(root)?;
    if let Some(pending) = &pending {
        debug_assert!(
            pending
                .agents()
                .iter()
                .all(|agent| agent_keys.contains(agent))
        );
    }

    let report = InitReport {
        outcome: InitOutcome::Initialized,
        summary,
        application,
        config_written,
        metadata_written,
        warnings,
    };
    Ok((report, details))
}

fn refresh_or_resume(
    request: &InitRequest,
    configured: &[String],
    requested: &[&'static AgentSurface],
) -> Result<(InitReport, InitDetails), InstallError> {
    let root = &request.project_root;
    let templates = request.effective_templates();
    let mut details = InitDetails::default();
    let mut warnings: Vec<String> = Vec::new();

    if let Some(pending) = PendingDelivery::load(root)? {
        // Resume exactly the interrupted delivery, from the saved configuration.
        // A request that names a different agent set would re-target the recovery
        // record, so it is refused rather than silently ignored.
        if !requested.is_empty() {
            let mut requested_keys: Vec<String> = requested
                .iter()
                .filter(|agent| request.assets.is_none() || agent.installs_command_skills())
                .map(|agent| agent.key.to_string())
                .collect();
            requested_keys.sort();
            if requested_keys != pending.agents() {
                return Err(InstallError::PendingDeliveryInvalid {
                    path: root.join(PENDING_DELIVERY_REL),
                    detail: format!(
                        "an interrupted delivery for {:?} is pending; re-run init without changing --ai to resume it",
                        pending.agents()
                    ),
                });
            }
        }
        let missing: Vec<&str> = pending
            .agents()
            .iter()
            .filter(|agent| !configured.iter().any(|known| known == *agent))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(InstallError::PendingDeliveryInvalid {
                path: root.join(PENDING_DELIVERY_REL),
                detail: format!(
                    "pending agents {missing:?} are not in the saved configuration; \
                     inspect {} before retrying",
                    CONFIG_REL
                ),
            });
        }
        let surfaces = resolve_requested_agents(pending.agents())?;
        let rendered = render_assets(
            &surfaces,
            &request.version,
            templates.as_ref(),
            request.assets.is_none(),
        )?;
        let plan = InstallPlan::plan(root, &request.version, &rendered, &request.options)?;
        let summary = plan.summary();
        let application = plan.commit(request.lock_wait)?;
        warnings.extend(plan.warnings().iter().cloned());
        warnings.extend(application.warnings.iter().cloned());
        // A pending record names the command-skill roster, but a crash can also
        // leave a native/shared charter-skill file missing: repair every
        // configured agent's managed skill surface.
        let agent_keys: Vec<String> = configured.to_vec();
        if let Some(assets) = &request.assets {
            if let Some(catalog) = &assets.skills {
                match crate::skills::run_surface_repair(root, &agent_keys, catalog, false) {
                    Ok(summary) => details.surface = summary,
                    Err(error) => warnings.push(format!("Tool surface repair failed: {error}")),
                }
            }
            let builtin_skill_names = assets
                .skills
                .as_ref()
                .map(SkillCatalog::names)
                .unwrap_or_default();
            let result = crate::gitignore::protect_all_agents_with(
                root,
                &agent_keys,
                &builtin_skill_names,
                false,
            )?;
            details.gitignore_modified = result.modified;
            details.gitignore_added = result.entries_added;
            warnings.extend(result.warnings);
        }
        PendingDelivery::clear(root)?;
        return Ok((
            InitReport {
                outcome: InitOutcome::ResumedDelivery,
                summary,
                application,
                config_written: false,
                metadata_written: false,
                warnings,
            },
            details,
        ));
    }

    // Refresh: an already-initialized project is never re-scaffolded. Requested
    // agents must already be configured (upstream: "run agent config add").
    let refresh_agents: Vec<&'static AgentSurface> = if request.agents.is_empty() {
        resolve_requested_agents(configured)?
    } else {
        let unconfigured: Vec<&str> = requested
            .iter()
            .filter(|agent| !configured.iter().any(|known| known == agent.key))
            .map(|agent| agent.key)
            .collect();
        if !unconfigured.is_empty() {
            return Err(InstallError::AlreadyInitialized {
                root: root.clone(),
                detail: format!(
                    "requested agents {unconfigured:?} are not configured; add them with \
                     `spec-kitty agent config add` first"
                ),
            });
        }
        requested.to_vec()
    };

    let rendered = render_assets(
        &refresh_agents,
        &request.version,
        templates.as_ref(),
        request.assets.is_none(),
    )?;
    let plan = InstallPlan::plan(root, &request.version, &rendered, &request.options)?;
    let summary = plan.summary();
    let application = plan.commit(request.lock_wait)?;
    warnings.extend(plan.warnings().iter().cloned());
    warnings.extend(application.warnings.iter().cloned());

    let agent_keys: Vec<String> = refresh_agents
        .iter()
        .map(|agent| agent.key.to_string())
        .collect();
    if let Some(assets) = &request.assets {
        if let Some(catalog) = &assets.skills {
            match crate::skills::run_surface_repair(root, &agent_keys, catalog, false) {
                Ok(summary) => details.surface = summary,
                Err(error) => warnings.push(format!("Tool surface repair failed: {error}")),
            }
        }
        let builtin_skill_names = assets
            .skills
            .as_ref()
            .map(SkillCatalog::names)
            .unwrap_or_default();
        let result = crate::gitignore::protect_all_agents_with(
            root,
            &agent_keys,
            &builtin_skill_names,
            false,
        )?;
        details.gitignore_modified = result.modified;
        details.gitignore_added = result.entries_added;
        warnings.extend(result.warnings);
    }
    if agent_keys.iter().any(|key| key == "vibe")
        && !crate::vibe::skill_path_configured(root)
        && let Err(error) = crate::vibe::ensure_project_skill_path(root)
    {
        warnings.push(format!(
            "Could not restore the Vibe skill-path pointer: {error}"
        ));
    }
    details.merge_driver_plan = crate::attributes::MergeDriverWiringPlan::plan(root).ok();
    details.security_notice = crate::gitignore::agent_folder_notice(
        &agent_keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    Ok((
        InitReport {
            outcome: InitOutcome::AlreadyInitialized,
            summary,
            application,
            config_written: false,
            metadata_written: false,
            warnings,
        },
        details,
    ))
}

fn resolve_requested_agents(keys: &[String]) -> Result<Vec<&'static AgentSurface>, InstallError> {
    if keys.is_empty() {
        return Err(InstallError::NoAgentsSelected);
    }
    let mut surfaces: Vec<&'static AgentSurface> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for key in keys {
        match agent_surface(key) {
            Ok(surface) => {
                if !surfaces.iter().any(|known| known.key == surface.key) {
                    surfaces.push(surface);
                }
            }
            Err(_) => unknown.push(key.clone()),
        }
    }
    if !unknown.is_empty() {
        return Err(InstallError::UnknownAgent {
            key: unknown.join(", "),
            valid: crate::agent::valid_keys(),
        });
    }
    Ok(surfaces)
}

fn render_assets(
    agents: &[&'static AgentSurface],
    version: &str,
    templates: Option<&TemplateSource>,
    include_command_files: bool,
) -> Result<Vec<RenderedAsset>, InstallError> {
    // Slash-command harnesses are template-driven: the pack's template set is the
    // command set (upstream `generate_agent_assets` walks the directory). Command
    // *skills* always cover the canonical set, rendering thin wrappers for the
    // commands that have no prompt of their own.
    let slash_commands: Option<Vec<String>> =
        if include_command_files && agents.iter().any(|agent| !agent.installs_command_skills()) {
            let source = templates.ok_or_else(|| InstallError::TemplateSourceUnavailable {
                detail: "installing slash-command surfaces requires a command template source"
                    .to_string(),
            })?;
            let discovered: Vec<String> = source
                .available_commands()?
                .into_iter()
                .filter(|command| crate::agent::is_canonical_command(command))
                .collect();
            if discovered.is_empty() {
                return Err(InstallError::TemplateSourceUnavailable {
                    detail: format!(
                        "no canonical command templates found under {}",
                        source.missions_root().display()
                    ),
                });
            }
            Some(discovered)
        } else {
            None
        };

    let mut rendered = Vec::new();
    for agent in agents {
        if agent.installs_command_skills() {
            for command in crate::agent::CANONICAL_COMMANDS {
                rendered.push(render_asset(agent, command, version, templates)?);
            }
        } else if include_command_files && let Some(commands) = &slash_commands {
            for command in commands {
                rendered.push(render_asset(agent, command, version, templates)?);
            }
        }
    }
    Ok(rendered)
}

/// The agent keys recorded in `.kittify/config.yaml`, or a typed failure.
pub fn read_configured_agents(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let path = project_root.join(CONFIG_REL);
    let raw = sk_kernel::read_bounded(&path, 4 * 1024 * 1024)
        .map_err(|error| InstallError::io("read", &path, std::io::Error::other(error)))?;
    let text = std::str::from_utf8(&raw).map_err(|error| InstallError::ConfigInvalid {
        path: path.clone(),
        detail: error.to_string(),
    })?;
    let value: YamlValue =
        serde_yaml_ng::from_str(text).map_err(|error| InstallError::ConfigInvalid {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    let mapping = value
        .as_mapping()
        .ok_or_else(|| InstallError::ConfigInvalid {
            path: path.clone(),
            detail: "config root must be a mapping".to_string(),
        })?;
    let agents = mapping
        .get(YamlValue::from("agents"))
        .and_then(|agents| agents.as_mapping())
        .and_then(|agents| agents.get(YamlValue::from("available")))
        .and_then(|available| available.as_sequence())
        .ok_or_else(|| InstallError::ConfigInvalid {
            path: path.clone(),
            detail: "config must declare agents.available as a list".to_string(),
        })?;
    let mut configured = Vec::with_capacity(agents.len());
    let mut seen = BTreeSet::new();
    for entry in agents {
        let key = entry.as_str().ok_or_else(|| InstallError::ConfigInvalid {
            path: path.clone(),
            detail: "agents.available entries must be strings".to_string(),
        })?;
        agent_surface(key)?;
        if seen.insert(key.to_string()) {
            configured.push(key.to_string());
        }
    }
    Ok(configured)
}

/// Whether `project_root` is already initialized (`.kittify/config.yaml` exists).
pub fn is_initialized(project_root: &Path) -> bool {
    project_root.join(CONFIG_REL).is_file()
}

/// Write the fresh `config.yaml`; never overwrite an authored one.
fn write_fresh_config(
    project_root: &Path,
    agents: &[&'static AgentSurface],
) -> Result<bool, InstallError> {
    let path = project_root.join(CONFIG_REL);
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    let mut text = String::from("agents:\n  available:\n");
    for agent in agents {
        text.push_str(&format!("  - {}\n", agent.key));
    }
    text.push_str("  auto_commit: true\n  lint_on_edit: false\nvcs:\n  type: git\n");
    sk_kernel::atomic_write(&path, text.as_bytes())
        .map_err(|error| InstallError::io("write config", &path, std::io::Error::other(error)))?;
    Ok(true)
}

/// The recovery record for `project_root`, if a delivery is pending.
pub fn pending_delivery(project_root: &Path) -> Result<Option<PendingDelivery>, InstallError> {
    PendingDelivery::load(project_root)
}

/// A deterministic timestamp helper for tests and receipts.
pub fn timestamp() -> String {
    now_utc_iso()
}

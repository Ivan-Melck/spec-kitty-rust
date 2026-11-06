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
//!    and the packaged `AGENTS.md`/`.claudeignore` templates), backing up
//!    operator subtrees;
//! 2. write the authored configuration (`config.yaml`);
//! 3. plan and commit the command surfaces (existing contract);
//! 4. install the charter skill catalog (native roots + shared manifest);
//! 5. run the managed-skill surface repair;
//! 6. write session-presence orientation (before profile projection, matching
//!    the pinned `SessionPresenceManager.install()` before surface repair);
//! 7. project the charter agent profiles and record ownership;
//! 8. protect runtime/agent paths in `.gitignore`;
//! 9. seed `.gitattributes` merge-driver attributes and expose the clone-local
//!    git-config wiring plan to the Git owner;
//! 10. provision mission-type activations from the built-in `default` preset
//!     (only when the activation key is still absent; the preset is validated
//!     before the first managed write);
//! 11. save/stamp `.kittify/metadata.yaml` additively (comments preserved);
//! 12. mint/persist project identity and append the `ProjectInitialized` event;
//! 13. seed the migration ledger (tracked, plus proven clone-local effects);
//! 14. restore the Vibe skill-path pointer when Vibe is selected;
//! 15. remove only proven run-created scratch.
//!
//! Optional steps report warnings; the required ones (template tree when
//! configured, command commit, config, `.gitignore`, activation provisioning)
//! fail closed and leave the pending record for a safe resume.
//!
//! **One lock authority.** The whole delivery runs under the canonical
//! [`ProjectLock::for_project`] (machine state directory, keyed by the resolved
//! project root), taken *before* the fresh/refresh/resume decision so two
//! concurrent inits cannot both scaffold, back up each other's copies, or write
//! the same managed surfaces. Nested commits (plan, identity, lifecycle event)
//! re-enter the same lock on one thread, which the kernel explicitly supports.
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
use crate::lock::ProjectLock;
use crate::metadata::{ProjectMetadata, stamp_schema_metadata};
use crate::plan::{ApplicationReport, InstallPlan, PlanOptions, PlanSummary, now_utc_iso};
use crate::render::{RenderedAsset, TemplateSource, render_asset};
use crate::skills::SkillCatalog;
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
    /// The charter agent-profile catalog (`packs/built-in/agent_profiles`).
    pub profiles: Option<crate::profiles::ProfileCatalog>,
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
        let preset = crate::template_tree::discover_pack_presets(checkout)
            .map(|root| root.join("default.yaml"))
            .filter(|path| path.is_file());
        let profiles = match crate::template_tree::discover_pack_agent_profiles(checkout) {
            Some(root) => crate::profiles::ProfileCatalog::discover(&root)?,
            None => None,
        };
        Ok(Self {
            template_tree,
            template_options: TemplateTreeOptions::default(),
            skills,
            activation_preset: preset,
            profiles,
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

    /// Attach an explicit agent-profile catalog.
    pub fn with_profiles(mut self, profiles: crate::profiles::ProfileCatalog) -> Self {
        self.profiles = Some(profiles);
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
        self.templates
            .clone()
            .or_else(|| {
                self.assets.as_ref().and_then(|assets| {
                    assets
                        .template_tree
                        .missions_root()
                        .map(TemplateSource::new)
                })
            })
            .map(|source| source.for_project(&self.project_root))
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
    /// Missing managed skill files restored by surface repair.
    pub skills_restored: Vec<String>,
    /// Agent-profile files projected (written).
    pub profiles_written: Vec<String>,
    /// Agent-profile files already holding the canonical render.
    pub profiles_kept: Vec<String>,
    /// Agent-profile files preserved because an operator owns them.
    pub profiles_preserved: Vec<String>,
    /// Orientation files written.
    pub orientation_written: Vec<String>,
    /// Orientation files preserved because an operator edited the block.
    pub orientation_preserved: Vec<String>,
    /// Whether `.gitignore` changed.
    pub gitignore_modified: bool,
    /// `.gitignore` entries added.
    pub gitignore_added: Vec<String>,
    /// Whether `.gitattributes` changed.
    pub attributes_written: bool,
    /// The typed clone-local merge-driver wiring plan.
    pub merge_driver_plan: Option<crate::attributes::MergeDriverWiringPlan>,
    /// What applying the clone-local merge-driver config did.
    pub merge_driver_wiring: Option<crate::attributes::MergeDriverWiringState>,
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
///
/// ## Recovery contract
///
/// Every required effect (template tree when assets are configured, command
/// commit, `config.yaml`, charter skills, activation, identity, the
/// `ProjectInitialized` event and the migration-ledger seed) is completed
/// **idempotently** on every entry point:
///
/// * a fresh run performs all of them;
/// * a resumed run — a pending record with or without `config.yaml` — performs
///   all of them again, so an interruption at any point is completed rather than
///   reported as success;
/// * a refresh of an initialized project completes the ones that can be missing
///   (skills, activation, identity, event, ledger, metadata) without
///   re-scaffolding operator content.
///
/// A failure in any required effect returns the typed error and leaves the
/// pending record in place. The record is cleared only after the whole delivery
/// is durable.
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

    // One lock authority for the whole delivery, held *before* the
    // fresh/refresh/resume decision. A second concurrent init therefore waits
    // here and then observes the first run's `config.yaml` (refresh) instead of
    // scaffolding again, moving the first run's freshly copied trees into a
    // `.backup-*` directory. Plan/identity/event commits below re-enter this
    // same lock on this thread.
    let _delivery_lock = ProjectLock::for_project(root, request.lock_wait)?;

    let config_path = root.join(CONFIG_REL);
    let pending = PendingDelivery::load(root)?;
    let configured = if config_path.exists() {
        read_configured_agents(root)?
    } else {
        Vec::new()
    };
    let requested = if request.agents.is_empty() {
        Vec::new()
    } else {
        resolve_requested_agents(&request.agents)?
    };

    if config_path.exists() {
        if let Some(pending) = &pending {
            return resume_delivery(request, pending, &configured, &requested);
        }
        return refresh_delivery(request, &configured, &requested);
    }

    // No `config.yaml`: a fresh run, or a fresh run interrupted before the
    // configuration was written. The pending record still names the delivery.
    let mut agents = requested;
    if let Some(pending) = &pending {
        validate_pending_selection(request, pending, &agents)?;
        for key in pending.agents() {
            if !agents.iter().any(|agent| agent.key == key) {
                agents.push(crate::agent::agent_surface(key)?);
            }
        }
        if agents.is_empty() {
            return Err(InstallError::NoAgentsSelected);
        }
        return run_delivery(request, &agents, DeliveryMode::ResumeFresh, Some(pending));
    }
    if agents.is_empty() {
        return Err(InstallError::NoAgentsSelected);
    }
    run_delivery(request, &agents, DeliveryMode::Fresh, None)
}

/// Whether this project still needs the built-in `default` activation preset.
///
/// The preset is only required while the `mission_type_activations` key is
/// entirely absent. An already-activated project (a refresh, or a resume whose
/// activation committed before the interruption) needs no preset at all; asking
/// for one there rejects a legitimate refresh when the source file is gone.
fn activation_preset_required(project_root: &Path) -> Result<bool, InstallError> {
    let activation_path = project_root.join(crate::activation::ACTIVATION_REL);
    let invalid = |error: sk_doctrine::DoctrineError| InstallError::MetadataInvalid {
        path: activation_path.clone(),
        detail: error.to_string(),
    };
    let transaction = sk_doctrine::ActivationTransaction::begin(project_root).map_err(invalid)?;
    let ledger = transaction.ledger().map_err(invalid)?;
    Ok(ledger.mission_type_activations.is_none())
}

/// Resolve and validate the activation preset before the first managed write.
///
/// Returns the preset path that the delivery must seed from, or `None` when the
/// activation key already exists and provisioning is a no-op. A fresh project
/// whose asset set has no preset fails here, before the template tree, config,
/// skills or metadata are written.
fn preflight_activation(request: &InitRequest) -> Result<Option<PathBuf>, InstallError> {
    let Some(assets) = &request.assets else {
        return Ok(None);
    };
    if !activation_preset_required(&request.project_root)? {
        return Ok(None);
    }
    let preset =
        assets
            .activation_preset
            .clone()
            .ok_or_else(|| InstallError::ActivationPresetMissing {
                path: PathBuf::from("packs/built-in/presets/default.yaml"),
                detail: "the full asset set has no built-in `default` activation preset; \
                     run the pack sync or pass one explicitly"
                    .to_string(),
            })?;
    // Fail closed on a missing/malformed/empty preset *now*, not after the
    // template tree and configuration have already landed.
    crate::activation::default_preset_mission_types(&preset)?;
    Ok(Some(preset))
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

/// How a delivery run was entered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryMode {
    /// A brand-new project.
    Fresh,
    /// A pending delivery whose `config.yaml` was not written yet.
    ResumeFresh,
    /// A pending delivery on an initialized project.
    Resume,
    /// An already-initialized project being verified/refreshed.
    Refresh,
}

impl DeliveryMode {
    fn is_fresh(self) -> bool {
        matches!(self, Self::Fresh | Self::ResumeFresh)
    }

    fn outcome(self) -> InitOutcome {
        match self {
            Self::Fresh => InitOutcome::Initialized,
            Self::ResumeFresh | Self::Resume => InitOutcome::ResumedDelivery,
            Self::Refresh => InitOutcome::AlreadyInitialized,
        }
    }
}

fn validate_pending_selection(
    request: &InitRequest,
    pending: &PendingDelivery,
    requested: &[&'static AgentSurface],
) -> Result<(), InstallError> {
    if requested.is_empty() {
        return Ok(());
    }
    // In full mode the record names the command-skill roster only, so compare
    // against the requested command-skill subset; the legacy path records every
    // selected agent.
    let mut keys: Vec<String> = requested
        .iter()
        .filter(|agent| request.assets.is_none() || agent.installs_command_skills())
        .map(|agent| agent.key.to_string())
        .collect();
    keys.sort();
    keys.dedup();
    if keys == pending.agents() {
        return Ok(());
    }
    Err(InstallError::PendingDeliveryInvalid {
        path: request.project_root.join(PENDING_DELIVERY_REL),
        detail: format!(
            "an interrupted delivery for {:?} is pending; re-run init without changing --ai to resume it",
            pending.agents()
        ),
    })
}

fn resume_delivery(
    request: &InitRequest,
    pending: &PendingDelivery,
    configured: &[String],
    requested: &[&'static AgentSurface],
) -> Result<(InitReport, InitDetails), InstallError> {
    validate_pending_selection(request, pending, requested)?;
    let missing: Vec<&str> = pending
        .agents()
        .iter()
        .filter(|agent| !configured.iter().any(|known| known == *agent))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(InstallError::PendingDeliveryInvalid {
            path: request.project_root.join(PENDING_DELIVERY_REL),
            detail: format!(
                "pending agents {missing:?} are not in the saved configuration; \
                 inspect {} before retrying",
                CONFIG_REL
            ),
        });
    }
    let mut agents: Vec<&'static AgentSurface> = configured
        .iter()
        .map(|key| crate::agent::agent_surface(key))
        .collect::<Result<_, _>>()?;
    for key in pending.agents() {
        if !agents.iter().any(|agent| agent.key == key) {
            agents.push(crate::agent::agent_surface(key)?);
        }
    }
    run_delivery(request, &agents, DeliveryMode::Resume, Some(pending))
}

fn refresh_delivery(
    request: &InitRequest,
    configured: &[String],
    requested: &[&'static AgentSurface],
) -> Result<(InitReport, InitDetails), InstallError> {
    // Refresh: an already-initialized project is never re-scaffolded. Requested
    // agents must already be configured (upstream: "run agent config add").
    let agents: Vec<&'static AgentSurface> = if requested.is_empty() {
        configured
            .iter()
            .map(|key| crate::agent::agent_surface(key))
            .collect::<Result<_, _>>()?
    } else {
        let unconfigured: Vec<&str> = requested
            .iter()
            .filter(|agent| !configured.iter().any(|known| known == agent.key))
            .map(|agent| agent.key)
            .collect();
        if !unconfigured.is_empty() {
            return Err(InstallError::AlreadyInitialized {
                root: request.project_root.clone(),
                detail: format!(
                    "requested agents {unconfigured:?} are not configured; add them with \
                     `spec-kitty agent config add` first"
                ),
            });
        }
        requested.to_vec()
    };
    run_delivery(request, &agents, DeliveryMode::Refresh, None)
}

fn run_delivery(
    request: &InitRequest,
    agents: &[&'static AgentSurface],
    mode: DeliveryMode,
    pending: Option<&PendingDelivery>,
) -> Result<(InitReport, InitDetails), InstallError> {
    let root = &request.project_root;
    let agent_keys: Vec<String> = agents.iter().map(|agent| agent.key.to_string()).collect();
    enforce_assets_pin(request)?;
    let templates = request.effective_templates();
    // Full delivery follows the current upstream surface: only the command-skill
    // roster installs command skills (into `.agents/skills/`); slash-command
    // files are no longer generated by init. The legacy path keeps its previous
    // 12-family command-file behavior for API compatibility.
    // Native's no-assets API validates legacy command templates before writes.
    // Full Source delivery instead installs final command skills after config
    // publication: activation must see that actual file, not the pre-init absence.
    let legacy_rendered = if request.assets.is_none() {
        Some(render_assets(
            agents,
            &request.version,
            templates.as_ref(),
            true,
        )?)
    } else {
        None
    };

    // Recovery record first on a fresh run: if this run dies, the next one
    // resumes exactly this agent set instead of re-scaffolding with a different
    // selection. In full mode the record names the command-skill roster only
    // (the upstream contract); native/shared charter skills are restored by
    // surface repair on the next run. A selection with no command-skill agent
    // has no record, exactly as upstream writes none.
    let pending_agents: Vec<String> = if request.assets.is_some() {
        agents
            .iter()
            .filter(|agent| agent.installs_command_skills())
            .map(|agent| agent.key.to_string())
            .collect()
    } else {
        agent_keys.clone()
    };
    if mode.is_fresh() {
        if pending_agents.is_empty() {
            if pending.is_some() {
                return Err(InstallError::NoAgentsSelected);
            }
        } else {
            PendingDelivery::write(root, &pending_agents)?;
        }
    }

    let mut details = InitDetails::default();
    let mut warnings: Vec<String> = Vec::new();
    let activation_preset = preflight_activation(request)?;

    if let Some(assets) = &request.assets {
        let should_install = match mode {
            DeliveryMode::Fresh => true,
            DeliveryMode::ResumeFresh => !template_tree_present(root),
            DeliveryMode::Resume | DeliveryMode::Refresh => false,
        };
        if should_install {
            let report =
                install_template_tree(root, &assets.template_tree, &assets.template_options)?;
            details.template_mode = report.mode;
            details.template_copied = report.copied;
            details.backups = report.backups;
            details.templates_created = report.templates_created;
            warnings.extend(report.warnings);
        } else if !root.join(MEMORY_REL).is_dir() && !root.join(".kittify/missions").is_dir() {
            // Refresh/resume never re-scaffolds (upstream early-returns), but the
            // missing tree is reported instead of claiming it was verified.
            warnings.push(
                "the template tree is absent from this project; init does not re-scaffold an \
                 initialized project (run `spec-kitty upgrade`)"
                    .to_string(),
            );
        }
    }

    let config_written = if mode.is_fresh() {
        write_fresh_config(root, agents)?
    } else {
        false
    };
    if mode.is_fresh() {
        std::fs::create_dir_all(root.join(MEMORY_REL)).map_err(|source| {
            InstallError::io("create memory directory", root.join(MEMORY_REL), source)
        })?;
    }

    let rendered = match legacy_rendered {
        Some(rendered) => rendered,
        None => render_assets(agents, &request.version, templates.as_ref(), false)?,
    };
    let plan = InstallPlan::plan(root, &request.version, &rendered, &request.options)?;
    let summary = plan.summary();
    let application = plan.commit(request.lock_wait)?;
    warnings.extend(plan.warnings().iter().cloned());
    warnings.extend(application.warnings.iter().cloned());

    // ---- metadata (additive, non-fatal upstream) -------------------------
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
    let metadata_path = root.join(METADATA_REL);
    let write_metadata = mode.is_fresh() || !metadata_path.is_file();
    let mut metadata_written = false;
    if write_metadata {
        let metadata = ProjectMetadata::for_new_project(
            &request.version,
            now_utc_iso(),
            &environment.python_version,
            &environment.platform,
            &environment.platform_version,
            None,
        );
        metadata_written = match metadata.save(&root.join(".kittify")) {
            Ok(written) => written,
            Err(error) => {
                warnings.push(format!("Could not save project metadata: {error}"));
                false
            }
        };
    }
    if let Err(error) = stamp_schema_metadata(&root.join(".kittify")) {
        warnings.push(format!("Could not stamp schema metadata: {error}"));
    }

    // ---- charter skills --------------------------------------------------
    if let Some(assets) = &request.assets {
        let catalog = assets
            .skills
            .as_ref()
            .ok_or_else(|| InstallError::SkillsCatalogMissing {
                path: root.to_path_buf(),
                detail: "the full asset set has no charter skill catalog".to_string(),
            })?;
        let manifest_present = crate::skills::ManagedSkillManifest::load_strict(root)?
            .is_some_and(|manifest| !manifest.entries.is_empty());
        if mode.is_fresh() || !manifest_present {
            let options = crate::skills::SkillInstallOptions::with_version(&request.version);
            match crate::skills::install_skills_for_agents(root, &agent_keys, catalog, &options) {
                Ok(report) => {
                    details.skills_written = report.written;
                    details.skills_adopted = report.adopted;
                    details.skills_drifted = report.drifted;
                    warnings.extend(report.warnings);
                }
                // An operator-owned file at a managed skill path is preserved
                // (upstream logs a warning and continues). Every other failure
                // is a real delivery failure and keeps the pending record.
                Err(error @ InstallError::SkillCollision { .. }) => warnings.push(format!(
                    "Charter skill installation was incomplete: {error}"
                )),
                Err(error) => return Err(error),
            }
        }
        let repair = crate::skills::run_surface_repair(root, &agent_keys, catalog, false)?;
        details.skills_restored = repair.restored.clone();
        warnings.extend(repair.warnings.clone());
        details.surface = repair;

        // ---- session presence (before profile projection) ----------------
        // Upstream runs `SessionPresenceManager.install()` before the tool-surface
        // repair that projects profiles. That ordering is observable: a
        // copilot-only fresh init has no `.github/` yet, so its `check_dir`
        // guard skips the orientation file, which profile projection then adds.
        let slug = crate::identity::resolve_identity(root)
            .project_slug
            .unwrap_or_else(|| crate::identity::derive_project_slug(root));
        let session =
            crate::session::apply_healthy_orientation(root, &agent_keys, &request.version, &slug)?;
        details.orientation_written = session.written;
        details.orientation_preserved = session.preserved;
        warnings.extend(session.warnings);

        // ---- agent-profile projection -----------------------------------
        let profiles =
            assets
                .profiles
                .as_ref()
                .ok_or_else(|| InstallError::AgentProfileCatalogMissing {
                    path: root.to_path_buf(),
                    detail: "the full asset set has no agent-profile catalog".to_string(),
                })?;
        let report =
            crate::profiles::project_agent_profiles(root, &agent_keys, profiles, "built-in")?;
        details.profiles_written = report.written;
        details.profiles_kept = report.kept;
        details.profiles_preserved = report.preserved;
        warnings.extend(report.warnings);
    }

    // ---- .gitignore (after the profile manifest exists) -------------------
    let builtin_skill_names = request
        .assets
        .as_ref()
        .and_then(|assets| assets.skills.as_ref())
        .map(crate::skills::SkillCatalog::names)
        .unwrap_or_default();
    if request.assets.is_some() {
        let result = crate::gitignore::protect_all_agents_with(
            root,
            &agent_keys,
            &builtin_skill_names,
            mode.is_fresh(),
        )?;
        details.gitignore_modified = result.modified;
        details.gitignore_added = result.entries_added;
        warnings.extend(result.warnings);
    } else {
        let result =
            crate::gitignore::protect_all_agents_with(root, &agent_keys, &[], mode.is_fresh())?;
        details.gitignore_modified = result.modified;
        details.gitignore_added = result.entries_added;
        warnings.extend(result.warnings);
    }

    // ---- .gitattributes + clone-local git config -------------------------
    details.attributes_written = crate::attributes::ensure_attribute_lines(root)?;
    let wiring_plan = crate::attributes::MergeDriverWiringPlan::plan(root)?;
    details.merge_driver_plan = Some(wiring_plan.clone());
    match wiring_plan.apply_git_config() {
        Ok(state) => {
            // The clone-local keys are delivered, but this build has no
            // `spec-kitty merge-driver-*` route: never report the drivers as
            // functional while that command is absent.
            if state.changed() {
                warnings.push(format!(
                    "Installed clone-local merge-driver config for {} key(s); this build does not implement the `spec-kitty merge-driver-*` commands, so the drivers are configured but not functional yet",
                    state.installed.len()
                ));
            }
            for customization in &state.customized {
                warnings.push(format!(
                    "Kept your own {} setting ({}); the shipped value was not applied",
                    customization.key, customization.value
                ));
            }
            if let Some(reason) = &state.skip_reason
                && wiring_plan.needs_git_config()
                && !state.changed()
            {
                warnings.push(format!(
                    "Clone-local merge-driver config was not written: {reason}"
                ));
            }
            details.merge_driver_wiring = Some(state);
        }
        Err(error) => {
            // Upstream treats git wiring as best-effort; the merge paths
            // self-heal it later. The tracked `.gitattributes` half is durable.
            warnings.push(format!(
                "Could not configure Spec Kitty merge drivers; continuing without local git configuration: {error}"
            ));
        }
    }

    // ---- mission-type activation (required only when absent) --------------
    // The preset was resolved and validated by `preflight_activation` before the
    // first managed write. When the activation key already exists no preset is
    // needed and `provision` would be a no-op, so it is not called at all.
    if let Some(preset) = &activation_preset {
        details.activation_provisioned =
            crate::activation::provision_mission_type_activations(root, preset)?;
    }

    // ---- identity + canonical event (required) ---------------------------
    let identity = crate::identity::ensure_identity(root, request.lock_wait)?;
    let project_uuid = identity
        .project_uuid
        .ok_or_else(|| InstallError::IdentityInvalid {
            path: root.join(CONFIG_REL),
            detail: "project identity has no uuid after ensure_identity".to_string(),
        })?;
    let payload = crate::lifecycle::payload_for_init(
        project_uuid.to_string(),
        identity.project_slug.clone(),
        Some(request.version.clone()),
    );
    let event = crate::lifecycle::emit_project_initialized(root, &payload, request.lock_wait)?;
    details.event_written = event.is_some();
    details.identity = Some(identity);

    // ---- migration-ledger seed (required) -------------------------------
    let catalog = MigrationCatalog::embedded()?;
    let ledger = MigrationLedger::new(root);
    details.ledger_seeded = ledger.seed_for_init(catalog, &request.version, |spec| {
        clone_local_effect_in_place(root, spec)
    })?;

    // ---- legacy compatibility surfaces ----------------------------------
    if agent_keys.iter().any(|key| key == "vibe") {
        match crate::vibe::ensure_project_skill_path(root) {
            Ok(_changed) => {}
            Err(error) => warnings.push(format!(
                "Could not write the Vibe skill-path pointer: {error}"
            )),
        }
    }

    if mode.is_fresh() {
        let mut run_created: BTreeSet<PathBuf> = BTreeSet::new();
        if details.templates_created {
            run_created.insert(PathBuf::from(".kittify/templates"));
        }
        match cleanup_init_scratch(root, &run_created) {
            Ok(preserved) => details.scratch_preserved = preserved,
            Err(error) => warnings.push(format!("Could not clean init scratch: {error}")),
        }
    }

    details.security_notice = crate::gitignore::agent_folder_notice(
        &agent_keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    // Only now is the delivery durable.
    if mode.is_fresh() || pending.is_some() {
        PendingDelivery::clear(root)?;
    }

    let report = InitReport {
        outcome: mode.outcome(),
        summary,
        application,
        config_written,
        metadata_written,
        warnings,
    };
    Ok((report, details))
}

/// Whether a full template tree is already installed (not a partial copy).
///
/// `.kittify/templates` is removed at the end of a successful fresh init, so it
/// cannot be a sentinel; the mission tree's `software-dev/mission.yaml` and the
/// memory tree's `README.md` are stable files a partial copy cannot fake.
fn template_tree_present(project_root: &Path) -> bool {
    project_root
        .join(".kittify/missions/software-dev/mission.yaml")
        .is_file()
        && project_root.join(".kittify/memory/README.md").is_file()
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
    // Section order matches upstream's saved configuration: `project` is
    // inserted at the top by `ensure_identity`, so `vcs` must precede `agents`.
    let mut text = String::from("vcs:\n  type: git\nagents:\n  available:\n");
    for agent in agents {
        text.push_str(&format!("  - {}\n", agent.key));
    }
    text.push_str("  auto_commit: true\n  lint_on_edit: false\n");
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

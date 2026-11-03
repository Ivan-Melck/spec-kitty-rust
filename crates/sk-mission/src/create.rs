//! Typed planning, validation, and failure-atomic writing of a Mission scaffold.
//!
//! # What a plan is — and is not
//!
//! A [`MissionCreatePlan`] is a complete, inspectable set of planning-artifact
//! writes: the canonical ULID identity, the canonical `<slug>-<mid8>` directory,
//! `meta.json`, and the `spec.md` / `tasks/README.md` / empty status-log
//! scaffold. Building and validating a plan writes nothing.
//!
//! Committing a plan writes those planning artifacts only. It does **not**
//! allocate lanes, mint or check out a branch, create a worktree, commit Git
//! state, or append a status event. The receipt carries
//! [`MissionAllocationState::PlanningArtifactsOnly`] so no caller can mistake a
//! filesystem plan for an allocated, executable Mission. The workspace owner
//! consumes the plan's identity and coordination-branch requirement and
//! performs those operations separately.
//!
//! # Identity and display number
//!
//! The Mission id is a caller-injected canonical ULID ([`MissionIdentity`]);
//! `mid8` is its first eight characters. `mission_number` is display-only and
//! is always `null` for a new Mission: the dense number is assigned later by
//! the single-writer merge context.
//!
//! # Conservation
//!
//! Planning is fail-closed: corrupt inventory, identity collisions, live
//! same-key duplicates, unsafe paths, and absent canonical templates refuse
//! before any write. A failed multi-file commit rolls back every file and
//! directory it created, and restores any replaced bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use sk_kernel::locks::LockWait;
use sk_kernel::{MissionSlug, atomic_write, read_bounded};
use thiserror::Error;

use crate::error::MissionError;
use crate::identity::{MissionId, MissionIdError};
use crate::meta::{DEFAULT_META_BYTE_LIMIT, META_FILE_NAME, SPECS_DIR_NAME, json_kind};
use crate::mission::{Mission, list_missions};
use crate::paths::{
    CHECKLISTS_DIR_NAME, PLAN_FILE_NAME, RESEARCH_DIR_NAME, SPEC_FILE_NAME,
    STATUS_EVENTS_FILE_NAME, TASKS_DIR_NAME, TASKS_FILE_NAME, TASKS_README_NAME,
};
use crate::template::{MissionTemplateKind, MissionTemplates, TemplateError, render_tasks_readme};
use crate::topology::{MissionTopology, routes_through_coordination};

/// The default `mission_type` when the request omits one.
pub const DEFAULT_MISSION_TYPE: &str = "software-dev";

const TASKS_GITKEEP: &str = ".gitkeep";
const DEFAULT_LOCK_TIMEOUT: Duration = crate::write_lock::MISSION_WRITE_LOCK_TIMEOUT;

/// A validated kebab-case Mission slug body supplied by the operator.
///
/// This is the *request* slug, not the canonical directory name. Creation
/// strips a legacy three-digit numeric prefix and appends `-<mid8>`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct MissionCreateSlug(String);

impl MissionCreateSlug {
    /// Validate a kebab-case Mission slug.
    ///
    /// Grammar: `^[a-z0-9][a-z0-9]*(-[a-z0-9]+)*$`. Bare numeric slugs such as
    /// `069` are accepted; the canonical directory name still appends the mid8.
    pub fn parse(value: &str) -> Result<Self, MissionCreateError> {
        if value.is_empty() || !value.is_ascii() {
            return Err(MissionCreateError::InvalidSlug {
                value: value.to_owned(),
                reason: "expected lowercase ASCII kebab-case".to_owned(),
            });
        }
        let bytes = value.as_bytes();
        if !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || value.contains("--")
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        {
            return Err(MissionCreateError::InvalidSlug {
                value: value.to_owned(),
                reason: "expected lowercase letters, digits, and single internal hyphens"
                    .to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// The request slug exactly as supplied.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The slug with one leading three-digit `NNN-` prefix removed.
    pub fn stripped(&self) -> &str {
        strip_numeric_prefix(&self.0)
    }

    /// Canonical directory name `<stripped-slug>-<mid8>`, idempotent when the
    /// supplied slug already ends in that mid8.
    pub fn directory_name(&self, mid8: &str) -> String {
        let suffix = format!("-{mid8}");
        if self.0.ends_with(&suffix) {
            self.0.clone()
        } else {
            format!("{}{suffix}", self.stripped())
        }
    }
}

impl std::fmt::Display for MissionCreateSlug {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn strip_numeric_prefix(slug: &str) -> &str {
    let bytes = slug.as_bytes();
    if bytes.len() > 4
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3] == b'-'
    {
        &slug[4..]
    } else {
        slug
    }
}

/// Canonical Mission identity inputs, injected so callers (and tests) control
/// the ULID and creation timestamp.
#[derive(Clone, Debug)]
pub struct MissionIdentity {
    id: MissionId,
    created_at: String,
}

impl MissionIdentity {
    /// Build an identity from an existing canonical ULID and timestamp.
    pub fn new(id: &str, created_at: impl Into<String>) -> Result<Self, MissionCreateError> {
        let id =
            id.parse::<MissionId>()
                .map_err(|source| MissionCreateError::InvalidMissionId {
                    value: id.to_owned(),
                    source,
                })?;
        let created_at = created_at.into();
        if created_at.trim().is_empty() || created_at.chars().any(char::is_control) {
            return Err(MissionCreateError::InvalidCreatedAt { value: created_at });
        }
        Ok(Self { id, created_at })
    }

    /// Mint a fresh canonical ULID and read the canonical clock.
    pub fn mint() -> Result<Self, MissionCreateError> {
        let id = ulid::Ulid::new().to_string();
        Self::new(&id, sk_kernel::clock::now_utc_iso())
    }

    /// Canonical identity.
    pub fn id(&self) -> &MissionId {
        &self.id
    }

    /// Canonical `mission_id[..8]`.
    pub fn mid8(&self) -> &str {
        self.id.mid8()
    }

    /// Injected creation timestamp.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

/// Create-time retention/binding flags.
///
/// Each flag is written only when `true`; `false` leaves the field absent,
/// matching the reference contract.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MissionRetention {
    /// Bind the Mission to a pull request.
    pub pr_bound: bool,
    /// Retain Mission branches after merge.
    pub retain_branches: bool,
    /// Retain lane worktrees after merge.
    pub retain_worktrees: bool,
    /// Commit directly to the target instead of minting a Mission branch.
    pub commit_to_target: bool,
}

/// A Mission creation request, before planning.
#[derive(Clone, Debug)]
pub struct MissionCreateRequest {
    slug: MissionCreateSlug,
    target_branch: String,
    mission_type: String,
    friendly_name: Option<String>,
    purpose_tldr: Option<String>,
    purpose_context: Option<String>,
    topology: MissionTopology,
    retention: MissionRetention,
    coordination_branch: Option<String>,
}

impl MissionCreateRequest {
    /// Build a request. `target_branch` must be a non-empty branch name.
    pub fn new(
        slug: impl AsRef<str>,
        target_branch: impl Into<String>,
    ) -> Result<Self, MissionCreateError> {
        let slug = MissionCreateSlug::parse(slug.as_ref())?;
        let target_branch = target_branch.into();
        if target_branch.trim().is_empty()
            || target_branch.trim() != target_branch
            || target_branch.chars().any(char::is_whitespace)
            || target_branch.chars().any(char::is_control)
            || target_branch == "HEAD"
            || target_branch.contains("..")
        {
            return Err(MissionCreateError::InvalidBranch {
                value: target_branch,
            });
        }
        Ok(Self {
            slug,
            target_branch,
            mission_type: DEFAULT_MISSION_TYPE.to_owned(),
            friendly_name: None,
            purpose_tldr: None,
            purpose_context: None,
            topology: MissionTopology::Coord,
            retention: MissionRetention::default(),
            coordination_branch: None,
        })
    }

    /// Set the human-friendly title.
    pub fn with_friendly_name(mut self, friendly_name: impl Into<String>) -> Self {
        self.friendly_name = Some(friendly_name.into());
        self
    }

    /// Set the purpose TL;DR and context.
    pub fn with_purpose(mut self, tldr: impl Into<String>, context: impl Into<String>) -> Self {
        self.purpose_tldr = Some(tldr.into());
        self.purpose_context = Some(context.into());
        self
    }

    /// Set the mission type (default `software-dev`).
    pub fn with_mission_type(mut self, mission_type: impl Into<String>) -> Self {
        self.mission_type = mission_type.into();
        self
    }

    /// Set the stored topology.
    pub fn with_topology(mut self, topology: MissionTopology) -> Self {
        self.topology = topology;
        self
    }

    /// Set retention/binding flags.
    pub fn with_retention(mut self, retention: MissionRetention) -> Self {
        self.retention = retention;
        self
    }

    /// Supply the coordination branch the workspace owner minted/pinned.
    ///
    /// Required for `coord` and `lanes_with_coord`; forbidden otherwise. The
    /// crate never mints or checks out this branch.
    pub fn with_coordination_branch(mut self, branch: impl Into<String>) -> Self {
        self.coordination_branch = Some(branch.into());
        self
    }

    /// The request slug.
    pub fn slug(&self) -> &MissionCreateSlug {
        &self.slug
    }

    /// The planning/target branch.
    pub fn target_branch(&self) -> &str {
        &self.target_branch
    }

    /// The requested mission type.
    pub fn mission_type(&self) -> &str {
        &self.mission_type
    }

    /// The requested topology.
    pub fn topology(&self) -> MissionTopology {
        self.topology
    }

    /// The explicit friendly name, if supplied.
    pub fn friendly_name(&self) -> Option<&str> {
        self.friendly_name.as_deref()
    }

    /// The explicit purpose TL;DR, if supplied.
    pub fn purpose_tldr(&self) -> Option<&str> {
        self.purpose_tldr.as_deref()
    }

    /// The explicit purpose context, if supplied.
    pub fn purpose_context(&self) -> Option<&str> {
        self.purpose_context.as_deref()
    }

    /// Create-time retention/binding flags.
    pub fn retention(&self) -> MissionRetention {
        self.retention
    }

    /// The workspace-owner coordination branch, if supplied.
    pub fn coordination_branch(&self) -> Option<&str> {
        self.coordination_branch.as_deref()
    }
}

/// Caller-supplied classification of an existing same-key Mission.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorMissionActivity {
    /// The prior Mission is real, live work; creation is refused.
    #[default]
    Live,
    /// The prior Mission was canceled or never worked; an in-place repair is
    /// allowed. The caller owns this evidence (status replay or Git tracking).
    Abandoned,
}

/// Options controlling creation safety and locking.
#[derive(Clone, Debug)]
pub struct MissionCreateOptions {
    allow_duplicate: bool,
    abandoned: BTreeSet<MissionId>,
    lock_timeout: Duration,
}

impl Default for MissionCreateOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl MissionCreateOptions {
    /// Default options: duplicates refused, every prior treated as live.
    pub fn new() -> Self {
        Self {
            allow_duplicate: false,
            abandoned: BTreeSet::new(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Permit a second live Mission with the same base slug and mission type.
    pub fn allow_duplicate(mut self, allow: bool) -> Self {
        self.allow_duplicate = allow;
        self
    }

    /// Mark specific prior Mission ids as abandoned (repairable).
    pub fn mark_abandoned(mut self, ids: impl IntoIterator<Item = MissionId>) -> Self {
        self.abandoned.extend(ids);
        self
    }

    /// Bound how long the creation lock may wait.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    fn activity(&self, mission: &Mission) -> PriorMissionActivity {
        if self.abandoned.contains(mission.id()) {
            PriorMissionActivity::Abandoned
        } else {
            PriorMissionActivity::Live
        }
    }
}

/// A planned scaffold file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedFileMode {
    /// Write the planned bytes. Existing differing bytes are a stale plan.
    Write,
    /// Create only when absent; preserve any existing user content.
    CreateIfAbsent,
}

/// What one planned file is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedFileKind {
    /// `meta.json`
    Meta,
    /// `spec.md`
    Spec,
    /// `tasks/README.md`
    TasksReadme,
    /// `tasks/.gitkeep`
    TasksGitkeep,
    /// Empty `status.events.jsonl`
    StatusEvents,
}

/// One inspectable planned write.
#[derive(Clone, Debug)]
pub struct PlannedFile {
    path: PathBuf,
    relative: PathBuf,
    kind: PlannedFileKind,
    mode: PlannedFileMode,
    bytes: Vec<u8>,
    template_id: Option<String>,
}

impl PlannedFile {
    /// Absolute destination path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path relative to the Mission directory (or `kitty-specs/` for the
    /// directory itself).
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    /// Artifact kind.
    pub fn kind(&self) -> PlannedFileKind {
        self.kind
    }

    /// Write mode.
    pub fn mode(&self) -> PlannedFileMode {
        self.mode
    }

    /// Exact planned bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Resolver id of the injected template that supplied these bytes, if any.
    pub fn template_id(&self) -> Option<&str> {
        self.template_id.as_deref()
    }
}

/// Whether a committed scaffold is an allocated, executable Mission.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionAllocationState {
    /// Planning artifacts exist; lanes, branches, worktrees, Git commits and
    /// status events are owned elsewhere and have not been performed here.
    PlanningArtifactsOnly,
}

/// A fully validated, read-only Mission creation plan.
#[derive(Clone, Debug)]
pub struct MissionCreatePlan {
    root: PathBuf,
    identity: MissionIdentity,
    request: MissionCreateRequest,
    directory_name: MissionSlug,
    spec_dir: PathBuf,
    meta_json: String,
    meta: BTreeMap<String, Value>,
    files: Vec<PlannedFile>,
    directories: Vec<PathBuf>,
    repair: bool,
    existing_meta: Option<Vec<u8>>,
    allow_duplicate: bool,
    abandoned: BTreeSet<MissionId>,
    lock_timeout: Duration,
}

impl MissionCreatePlan {
    /// Plan a Mission without writing anything.
    pub fn plan(
        root: &Path,
        request: MissionCreateRequest,
        identity: MissionIdentity,
        templates: &MissionTemplates,
        options: &MissionCreateOptions,
    ) -> Result<Self, MissionCreateError> {
        let validated = ValidatedRequest::new(&request)?;
        ensure_root(root)?;
        let root = canonical_root(root)?;

        let inventory = list_missions(&root).map_err(|source| MissionCreateError::Inventory {
            path: root.to_path_buf(),
            source: Box::new(source),
        })?;

        let directory_name = MissionSlug::from_str(&request.slug().directory_name(identity.mid8()))
            .map_err(|source| MissionCreateError::InvalidDirectoryName {
                value: request.slug().directory_name(identity.mid8()),
                source,
            })?;
        let spec_dir = root.join(SPECS_DIR_NAME).join(directory_name.as_str());

        // A symlinked target is never repaired or followed.
        if let Ok(metadata) = fs::symlink_metadata(&spec_dir) {
            if metadata.file_type().is_symlink() {
                return Err(MissionCreateError::SymlinkedTarget { path: spec_dir });
            }
            if !metadata.is_dir() {
                return Err(MissionCreateError::DirectoryExists { path: spec_dir });
            }
        }

        let mut target_mission: Option<&Mission> = None;
        for mission in &inventory {
            if mission.spec_dir() == spec_dir {
                target_mission = Some(mission);
            } else if mission.id() == identity.id() {
                return Err(MissionCreateError::MissionIdCollision {
                    id: identity.id().clone(),
                    path: mission.spec_dir().to_path_buf(),
                });
            }
        }

        let repair = match target_mission {
            Some(mission) => {
                if options.activity(mission) == PriorMissionActivity::Live {
                    return Err(MissionCreateError::AlreadyExists {
                        base_slug: request.slug().stripped().to_owned(),
                        mission_type: request.mission_type().to_owned(),
                        path: mission.spec_dir().to_path_buf(),
                        mid8: mission.mid8().to_owned(),
                    });
                }
                true
            }
            None => {
                if fs::symlink_metadata(&spec_dir).is_ok() {
                    return Err(MissionCreateError::DirectoryExists { path: spec_dir });
                }
                false
            }
        };

        if !options.allow_duplicate {
            let base_slug = request.slug().stripped();
            for mission in &inventory {
                if target_mission.is_some_and(|target| target.id() == mission.id()) {
                    continue;
                }
                if !candidate_name_matches(mission.slug().as_str(), base_slug) {
                    continue;
                }
                if mission.mission_type().unwrap_or(DEFAULT_MISSION_TYPE) != request.mission_type()
                {
                    continue;
                }
                if options.activity(mission) == PriorMissionActivity::Abandoned {
                    continue;
                }
                return Err(MissionCreateError::AlreadyExists {
                    base_slug: base_slug.to_owned(),
                    mission_type: request.mission_type().to_owned(),
                    path: mission.spec_dir().to_path_buf(),
                    mid8: mission.mid8().to_owned(),
                });
            }
        }

        let existing_meta = if repair {
            let meta_path = spec_dir.join(META_FILE_NAME);
            let bytes = read_bounded(&meta_path, DEFAULT_META_BYTE_LIMIT).map_err(|source| {
                MissionCreateError::MetaRead {
                    path: meta_path.clone(),
                    source,
                }
            })?;
            Some(bytes)
        } else {
            None
        };

        let existing_map = existing_meta
            .as_deref()
            .map(|bytes| decode_meta_object(bytes, &spec_dir.join(META_FILE_NAME)))
            .transpose()?;

        let meta = compose_meta(
            &validated,
            &request,
            &identity,
            existing_map.as_ref(),
            repair,
        )?;
        let meta_json = serialize_meta(&meta)?;

        let spec = templates.spec();
        if spec.kind() != MissionTemplateKind::Spec {
            return Err(MissionCreateError::Template(
                TemplateError::MismatchedKind {
                    id: spec.id().to_owned(),
                    expected: MissionTemplateKind::Spec,
                    actual: spec.kind(),
                },
            ));
        }
        let readme = render_tasks_readme(&request.target_branch);
        let status_log = !routes_through_coordination(request.topology);

        let mut files = vec![
            PlannedFile {
                path: spec_dir.join(META_FILE_NAME),
                relative: PathBuf::from(META_FILE_NAME),
                kind: PlannedFileKind::Meta,
                mode: PlannedFileMode::Write,
                bytes: meta_json.clone().into_bytes(),
                template_id: None,
            },
            PlannedFile {
                path: spec_dir.join(SPEC_FILE_NAME),
                relative: PathBuf::from(SPEC_FILE_NAME),
                kind: PlannedFileKind::Spec,
                mode: PlannedFileMode::CreateIfAbsent,
                bytes: spec.content().as_bytes().to_vec(),
                template_id: Some(spec.id().to_owned()),
            },
            PlannedFile {
                path: spec_dir.join(TASKS_DIR_NAME).join(TASKS_README_NAME),
                relative: PathBuf::from(TASKS_DIR_NAME).join(TASKS_README_NAME),
                kind: PlannedFileKind::TasksReadme,
                mode: PlannedFileMode::CreateIfAbsent,
                bytes: readme.into_bytes(),
                template_id: None,
            },
            PlannedFile {
                path: spec_dir.join(TASKS_DIR_NAME).join(TASKS_GITKEEP),
                relative: PathBuf::from(TASKS_DIR_NAME).join(TASKS_GITKEEP),
                kind: PlannedFileKind::TasksGitkeep,
                mode: PlannedFileMode::CreateIfAbsent,
                bytes: Vec::new(),
                template_id: None,
            },
        ];
        if status_log {
            files.push(PlannedFile {
                path: spec_dir.join(STATUS_EVENTS_FILE_NAME),
                relative: PathBuf::from(STATUS_EVENTS_FILE_NAME),
                kind: PlannedFileKind::StatusEvents,
                mode: PlannedFileMode::CreateIfAbsent,
                bytes: Vec::new(),
                template_id: None,
            });
        }

        let directories = vec![
            root.join(SPECS_DIR_NAME),
            spec_dir.clone(),
            spec_dir.join(CHECKLISTS_DIR_NAME),
            spec_dir.join(RESEARCH_DIR_NAME),
            spec_dir.join(TASKS_DIR_NAME),
        ];

        let plan = Self {
            root: root.to_path_buf(),
            identity,
            request,
            directory_name,
            spec_dir,
            meta_json,
            meta,
            files,
            directories,
            repair,
            existing_meta,
            allow_duplicate: options.allow_duplicate,
            abandoned: options.abandoned.clone(),
            lock_timeout: options.lock_timeout,
        };
        plan.validate(&root)?;
        Ok(plan)
    }

    /// Canonical Mission identity.
    pub fn mission_id(&self) -> &MissionId {
        self.identity.id()
    }

    /// Canonical eight-character identity prefix.
    pub fn mid8(&self) -> &str {
        self.identity.mid8()
    }

    /// Canonical `kitty-specs/<slug>-<mid8>` directory name.
    pub fn directory_name(&self) -> &MissionSlug {
        &self.directory_name
    }

    /// Absolute Mission directory this plan targets.
    pub fn spec_dir(&self) -> &Path {
        &self.spec_dir
    }

    /// Display-only mission number: `None` for a new Mission, preserved for a
    /// repaired prior that already had one.
    pub fn mission_number(&self) -> Option<u64> {
        self.meta.get("mission_number").and_then(Value::as_u64)
    }

    /// Exact `meta.json` bytes the commit writes.
    pub fn meta_json(&self) -> &str {
        &self.meta_json
    }

    /// All planned `meta.json` fields.
    pub fn meta(&self) -> &BTreeMap<String, Value> {
        &self.meta
    }

    /// The request this plan was built from.
    pub fn request(&self) -> &MissionCreateRequest {
        &self.request
    }

    /// Stored topology.
    pub fn topology(&self) -> MissionTopology {
        self.request.topology()
    }

    /// Coordination branch supplied by the workspace owner.
    pub fn coordination_branch(&self) -> Option<&str> {
        self.request.coordination_branch.as_deref()
    }

    /// Whether this plan repairs an abandoned in-place scaffold.
    pub fn is_repair(&self) -> bool {
        self.repair
    }

    /// The planned files in deterministic commit order.
    pub fn planned_files(&self) -> &[PlannedFile] {
        &self.files
    }

    /// Directories the commit ensures exist, parents first.
    pub fn planned_directories(&self) -> &[PathBuf] {
        &self.directories
    }

    /// Revalidate the plan against the current filesystem and inventory.
    pub fn validate(&self, root: &Path) -> Result<(), MissionCreateError> {
        ensure_root(root)?;
        let root = canonical_root(root)?;
        if root != self.root {
            return Err(MissionCreateError::StalePlan {
                path: root,
                detail: "plan was built for a different project root".to_owned(),
            });
        }
        let inventory = list_missions(&root).map_err(|source| MissionCreateError::Inventory {
            path: root.clone(),
            source: Box::new(source),
        })?;

        let mut target_exists = false;
        for mission in &inventory {
            if mission.spec_dir() == self.spec_dir {
                target_exists = true;
                if mission.id() != self.identity.id() {
                    return Err(MissionCreateError::StalePlan {
                        path: self.spec_dir.clone(),
                        detail: format!(
                            "target now records mission_id {}, not {}",
                            mission.id(),
                            self.identity.id()
                        ),
                    });
                }
            } else if mission.id() == self.identity.id() {
                return Err(MissionCreateError::MissionIdCollision {
                    id: self.identity.id().clone(),
                    path: mission.spec_dir().to_path_buf(),
                });
            }
        }

        if !self.repair {
            if fs::symlink_metadata(&self.spec_dir).is_ok() && !target_exists {
                return Err(MissionCreateError::StalePlan {
                    path: self.spec_dir.clone(),
                    detail: "target directory appeared after the plan was built".to_owned(),
                });
            }
            if target_exists {
                // Re-committing the same plan is an idempotent repair: the
                // inventory identity matches and meta.json must already be the
                // exact planned bytes. Any other divergence is a stale plan.
                let path = self.spec_dir.join(META_FILE_NAME);
                let current = read_bounded(&path, DEFAULT_META_BYTE_LIMIT).map_err(|source| {
                    MissionCreateError::MetaRead {
                        path: path.clone(),
                        source,
                    }
                })?;
                if current != self.meta_json.as_bytes() {
                    return Err(MissionCreateError::StalePlan {
                        path,
                        detail: "target meta.json differs from the planned bytes".to_owned(),
                    });
                }
            }
        }
        if self.repair {
            match &self.existing_meta {
                Some(expected) => {
                    let path = self.spec_dir.join(META_FILE_NAME);
                    let current =
                        read_bounded(&path, DEFAULT_META_BYTE_LIMIT).map_err(|source| {
                            MissionCreateError::MetaRead {
                                path: path.clone(),
                                source,
                            }
                        })?;
                    if &current != expected && current != self.meta_json.as_bytes() {
                        return Err(MissionCreateError::StalePlan {
                            path,
                            detail: "meta.json changed after the repair plan was built".to_owned(),
                        });
                    }
                }
                None => {
                    return Err(MissionCreateError::StalePlan {
                        path: self.spec_dir.clone(),
                        detail: "repair plan has no captured prior metadata".to_owned(),
                    });
                }
            }
        }

        for file in &self.files {
            if !file.path.starts_with(&self.spec_dir) {
                return Err(MissionCreateError::UnsafePath {
                    path: file.path.clone(),
                });
            }
            match fs::symlink_metadata(&file.path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(MissionCreateError::SymlinkedTarget {
                        path: file.path.clone(),
                    });
                }
                Ok(metadata) if metadata.is_dir() => {
                    return Err(MissionCreateError::PathIsDirectory {
                        path: file.path.clone(),
                    });
                }
                Ok(_) => {
                    if file.mode == PlannedFileMode::Write {
                        let current = read_bounded(&file.path, DEFAULT_META_BYTE_LIMIT).map_err(
                            |source| MissionCreateError::MetaRead {
                                path: file.path.clone(),
                                source,
                            },
                        )?;
                        let accepted = if self.repair && file.kind == PlannedFileKind::Meta {
                            current == file.bytes
                                || self.existing_meta.as_deref() == Some(current.as_slice())
                        } else {
                            current == file.bytes
                        };
                        if !accepted {
                            return Err(MissionCreateError::StalePlan {
                                path: file.path.clone(),
                                detail: "planned bytes differ from the current file".to_owned(),
                            });
                        }
                    }
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(MissionCreateError::Io {
                        action: "inspect",
                        path: file.path.clone(),
                        source,
                    });
                }
            }
        }

        if !self.repair && !self.allow_duplicate {
            let base_slug = self.request.slug().stripped();
            for mission in &inventory {
                if mission.id() == self.identity.id() || self.abandoned.contains(mission.id()) {
                    continue;
                }
                if !candidate_name_matches(mission.slug().as_str(), base_slug) {
                    continue;
                }
                if mission.mission_type().unwrap_or(DEFAULT_MISSION_TYPE)
                    != self.request.mission_type()
                {
                    continue;
                }
                return Err(MissionCreateError::StalePlan {
                    path: mission.spec_dir().to_path_buf(),
                    detail: "a live same-key Mission appeared after the plan was built".to_owned(),
                });
            }
        }

        Ok(())
    }

    /// Commit the plan, returning what was created, preserved, and unchanged.
    pub fn commit(&self, root: &Path) -> Result<MissionCreateReceipt, MissionCreateError> {
        self.commit_with_observer(root, &mut ())
    }

    /// Commit with a write observer, used by recovery and failure-injection
    /// tests. The observer runs immediately before each atomic write.
    pub fn commit_with_observer(
        &self,
        root: &Path,
        observer: &mut dyn MissionWriteObserver,
    ) -> Result<MissionCreateReceipt, MissionCreateError> {
        self.validate(root)?;
        // The create's `meta.json` birth write takes the documented birth key
        // (the directory name): there is no `meta.json` to resolve yet, so the
        // canonical primary/coordination key cascade does not apply here.
        let _lock = crate::write_lock::lock_mission_birth(
            &self.spec_dir,
            &self.root,
            LockWait::Bounded(self.lock_timeout),
        )
        .map_err(|source| MissionCreateError::Lock {
            path: self.spec_dir.clone(),
            source,
        })?;
        // Revalidate under the lock: another writer may have won the race.
        self.validate(root)?;

        let mut transaction = WriteTransaction::new();
        if let Err(error) = self.write_all(observer, &mut transaction) {
            if let Err(rollback) = transaction.rollback() {
                return Err(MissionCreateError::RollbackFailed {
                    detail: format!("{error}; rollback also failed: {rollback}"),
                });
            }
            return Err(error);
        }
        transaction.commit();
        Ok(MissionCreateReceipt {
            spec_dir: self.spec_dir.clone(),
            created: transaction.created,
            preserved: transaction.preserved,
            unchanged: transaction.unchanged,
            allocation: MissionAllocationState::PlanningArtifactsOnly,
        })
    }

    fn write_all(
        &self,
        observer: &mut dyn MissionWriteObserver,
        transaction: &mut WriteTransaction,
    ) -> Result<(), MissionCreateError> {
        for directory in &self.directories {
            transaction.ensure_directory(directory)?;
        }
        for file in &self.files {
            transaction.write(file.path(), file.bytes(), file.mode, self.repair, observer)?;
        }
        Ok(())
    }
}

/// The result of a committed creation plan.
#[derive(Clone, Debug)]
pub struct MissionCreateReceipt {
    spec_dir: PathBuf,
    created: Vec<PathBuf>,
    preserved: Vec<PathBuf>,
    unchanged: Vec<PathBuf>,
    allocation: MissionAllocationState,
}

impl MissionCreateReceipt {
    /// Mission directory.
    pub fn spec_dir(&self) -> &Path {
        &self.spec_dir
    }

    /// Files created by this commit.
    pub fn created(&self) -> &[PathBuf] {
        &self.created
    }

    /// Existing user files preserved untouched.
    pub fn preserved(&self) -> &[PathBuf] {
        &self.preserved
    }

    /// Files already byte-identical to the plan.
    pub fn unchanged(&self) -> &[PathBuf] {
        &self.unchanged
    }

    /// Explicit statement that this receipt is not an allocation.
    pub fn allocation(&self) -> MissionAllocationState {
        self.allocation
    }
}

/// Observe each planned write. Implemented for `()` as a no-op.
pub trait MissionWriteObserver {
    /// Called immediately before an atomic write; returning an error aborts
    /// and triggers rollback.
    fn before_write(&mut self, path: &Path) -> io::Result<()>;
}

impl MissionWriteObserver for () {
    fn before_write(&mut self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

struct WriteTransaction {
    created: Vec<PathBuf>,
    preserved: Vec<PathBuf>,
    unchanged: Vec<PathBuf>,
    backups: Vec<(PathBuf, Option<Vec<u8>>)>,
    created_dirs: Vec<PathBuf>,
}

impl WriteTransaction {
    fn new() -> Self {
        Self {
            created: Vec::new(),
            preserved: Vec::new(),
            unchanged: Vec::new(),
            backups: Vec::new(),
            created_dirs: Vec::new(),
        }
    }

    fn ensure_directory(&mut self, path: &Path) -> Result<(), MissionCreateError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(MissionCreateError::SymlinkedTarget {
                    path: path.to_path_buf(),
                })
            }
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(MissionCreateError::DirectoryExists {
                path: path.to_path_buf(),
            }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(path).map_err(|source| MissionCreateError::Io {
                    action: "create directory",
                    path: path.to_path_buf(),
                    source,
                })?;
                self.created_dirs.push(path.to_path_buf());
                Ok(())
            }
            Err(source) => Err(MissionCreateError::Io {
                action: "inspect",
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn write(
        &mut self,
        path: &Path,
        bytes: &[u8],
        mode: PlannedFileMode,
        repair: bool,
        observer: &mut dyn MissionWriteObserver,
    ) -> Result<(), MissionCreateError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(MissionCreateError::SymlinkedTarget {
                    path: path.to_path_buf(),
                });
            }
            Ok(metadata) if metadata.is_dir() => {
                return Err(MissionCreateError::PathIsDirectory {
                    path: path.to_path_buf(),
                });
            }
            Ok(_) => {
                let current = fs::read(path).map_err(|source| MissionCreateError::Io {
                    action: "read",
                    path: path.to_path_buf(),
                    source,
                })?;
                if current == bytes {
                    self.unchanged.push(path.to_path_buf());
                    return Ok(());
                }
                match mode {
                    PlannedFileMode::CreateIfAbsent => {
                        self.preserved.push(path.to_path_buf());
                        return Ok(());
                    }
                    PlannedFileMode::Write => {
                        if !repair {
                            return Err(MissionCreateError::StalePlan {
                                path: path.to_path_buf(),
                                detail: "file appeared with different bytes".to_owned(),
                            });
                        }
                        self.backups.push((path.to_path_buf(), Some(current)));
                    }
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                self.backups.push((path.to_path_buf(), None));
            }
            Err(source) => {
                return Err(MissionCreateError::Io {
                    action: "inspect",
                    path: path.to_path_buf(),
                    source,
                });
            }
        }

        observer
            .before_write(path)
            .map_err(|source| MissionCreateError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        atomic_write(path, bytes).map_err(|source| MissionCreateError::Kernel {
            path: path.to_path_buf(),
            source,
        })?;
        self.created.push(path.to_path_buf());
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), MissionCreateError> {
        let mut failure = None;
        for (path, previous) in self.backups.iter().rev() {
            let result = match previous {
                Some(bytes) => atomic_write(path, bytes).map(|_| ()),
                None => match fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(source) => Err(sk_kernel::KernelError::Io {
                        action: "remove",
                        path: path.clone(),
                        source,
                    }),
                },
            };
            if let Err(error) = result {
                failure = Some(format!("{error}"));
            }
        }
        for directory in self.created_dirs.iter().rev() {
            let _ = fs::remove_dir(directory);
        }
        match failure {
            Some(detail) => Err(MissionCreateError::RollbackFailed { detail }),
            None => Ok(()),
        }
    }

    fn commit(&mut self) {
        self.backups.clear();
        self.created_dirs.clear();
    }
}

struct ValidatedRequest {
    friendly_name: String,
    purpose_tldr: String,
    purpose_context: String,
    mission_type: String,
}

impl ValidatedRequest {
    fn new(request: &MissionCreateRequest) -> Result<Self, MissionCreateError> {
        let mission_type = request.mission_type.trim();
        if mission_type.is_empty()
            || MissionSlug::from_str(mission_type).is_err()
            || mission_type.contains('/')
            || mission_type.contains('\\')
        {
            return Err(MissionCreateError::InvalidMissionType {
                value: request.mission_type.clone(),
            });
        }

        let friendly_name = match &request.friendly_name {
            Some(value) => {
                let normalized = collapse_whitespace(value);
                if normalized.is_empty() {
                    return Err(MissionCreateError::EmptyFriendlyName);
                }
                normalized
            }
            None => default_display_name(request.slug.as_str()),
        };

        let purpose_tldr = match &request.purpose_tldr {
            Some(value) => {
                // The reference normalizes whitespace first, so a multi-line
                // input becomes the single line it validates.
                let normalized = collapse_whitespace(value);
                if normalized.is_empty() {
                    return Err(MissionCreateError::InvalidPurposeTldr {
                        detail: "purpose_tldr is required and non-empty".to_owned(),
                    });
                }
                normalized
            }
            None => friendly_name.clone(),
        };

        let purpose_context = match &request.purpose_context {
            Some(value) => {
                let normalized = collapse_whitespace(value);
                if normalized.is_empty() {
                    return Err(MissionCreateError::EmptyPurposeContext);
                }
                normalized
            }
            None => default_purpose_context(&friendly_name, &request.target_branch),
        };

        if request.retention.commit_to_target && request.topology != MissionTopology::SingleBranch {
            return Err(MissionCreateError::CommitToTargetRequiresSingleBranch {
                topology: request.topology,
            });
        }
        let routing = routes_through_coordination(request.topology);
        match (&request.coordination_branch, routing) {
            (None, true) => {
                return Err(MissionCreateError::CoordinationBranchRequired {
                    topology: request.topology,
                });
            }
            (Some(_), false) => {
                return Err(MissionCreateError::UnexpectedCoordinationBranch {
                    topology: request.topology,
                });
            }
            (Some(branch), true) => {
                if branch.trim().is_empty()
                    || branch.trim() != branch
                    || branch.chars().any(char::is_control)
                    || branch.chars().any(char::is_whitespace)
                {
                    return Err(MissionCreateError::InvalidBranch {
                        value: branch.clone(),
                    });
                }
            }
            (None, false) => {}
        }

        Ok(Self {
            friendly_name,
            purpose_tldr,
            purpose_context,
            mission_type: mission_type.to_owned(),
        })
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf, MissionCreateError> {
    fs::canonicalize(root).map_err(|source| MissionCreateError::Io {
        action: "canonicalize",
        path: root.to_path_buf(),
        source,
    })
}

fn ensure_root(root: &Path) -> Result<(), MissionCreateError> {
    match fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(MissionCreateError::RootNotDirectory {
            root: root.to_path_buf(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Err(MissionCreateError::RootNotDirectory {
                root: root.to_path_buf(),
            })
        }
        Err(source) => Err(MissionCreateError::Io {
            action: "inspect",
            path: root.to_path_buf(),
            source,
        }),
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn default_display_name(mission_slug: &str) -> String {
    let parts: Vec<&str> = mission_slug
        .trim()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        "Mission".to_owned()
    } else {
        parts.join(" ")
    }
}

fn default_purpose_context(display_name: &str, target_branch: &str) -> String {
    format!(
        "This mission advances {display_name} on {target_branch} so stakeholders can track the work from mission creation onward."
    )
}

fn candidate_name_matches(name: &str, base_slug: &str) -> bool {
    if name == base_slug {
        return true;
    }
    let Some(tail) = name.strip_prefix(base_slug) else {
        return false;
    };
    let Some(tail) = tail.strip_prefix('-') else {
        return false;
    };
    tail.len() == 8 && tail.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn decode_meta_object(
    bytes: &[u8],
    path: &Path,
) -> Result<serde_json::Map<String, Value>, MissionCreateError> {
    let text = std::str::from_utf8(bytes).map_err(|error| MissionCreateError::Meta {
        path: path.to_path_buf(),
        detail: format!("not valid UTF-8: {error}"),
    })?;
    let value: Value = serde_json::from_str(text).map_err(|error| MissionCreateError::Meta {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    match value {
        Value::Object(map) => Ok(map),
        other => Err(MissionCreateError::Meta {
            path: path.to_path_buf(),
            detail: format!("must be a JSON object, not {}", json_kind(&other)),
        }),
    }
}

fn compose_meta(
    validated: &ValidatedRequest,
    request: &MissionCreateRequest,
    identity: &MissionIdentity,
    existing: Option<&serde_json::Map<String, Value>>,
    repair: bool,
) -> Result<BTreeMap<String, Value>, MissionCreateError> {
    let mut meta: BTreeMap<String, Value> = existing
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();

    if repair {
        for (field, requested) in [
            (
                "topology",
                Value::String(request.topology.as_str().to_owned()),
            ),
            (
                "coordination_branch",
                request
                    .coordination_branch
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            ),
        ] {
            if let Some(existing) = meta.get(field)
                && existing != &requested
            {
                return Err(MissionCreateError::IdentityConflict {
                    field,
                    detail: format!(
                        "repair cannot change recorded routing from {existing} to {requested}"
                    ),
                });
            }
        }
    }

    if let Some(value) = meta.get("mission_id")
        && value.as_str() != Some(identity.id().as_str())
    {
        return Err(MissionCreateError::IdentityConflict {
            field: "mission_id",
            detail: format!("existing value {value} does not match {}", identity.id()),
        });
    }
    if let Some(value) = meta.get("mid8")
        && value.as_str() != Some(identity.mid8())
    {
        return Err(MissionCreateError::IdentityConflict {
            field: "mid8",
            detail: format!("existing value {value} does not match {}", identity.mid8()),
        });
    }

    let set_default = |meta: &mut BTreeMap<String, Value>, key: &str, value: Value| {
        meta.entry(key.to_owned()).or_insert(value);
    };

    set_default(
        &mut meta,
        "mission_id",
        Value::String(identity.id().as_str().to_owned()),
    );
    set_default(&mut meta, "mid8", Value::String(identity.mid8().to_owned()));
    set_default(&mut meta, "mission_number", Value::Null);
    set_default(
        &mut meta,
        "slug",
        Value::String(request.slug().directory_name(identity.mid8())),
    );
    set_default(
        &mut meta,
        "mission_slug",
        Value::String(request.slug().directory_name(identity.mid8())),
    );
    set_default(
        &mut meta,
        "friendly_name",
        Value::String(validated.friendly_name.clone()),
    );
    set_default(
        &mut meta,
        "purpose_tldr",
        Value::String(validated.purpose_tldr.clone()),
    );
    set_default(
        &mut meta,
        "purpose_context",
        Value::String(validated.purpose_context.clone()),
    );
    set_default(
        &mut meta,
        "mission_type",
        Value::String(validated.mission_type.clone()),
    );
    set_default(
        &mut meta,
        "target_branch",
        Value::String(request.target_branch.clone()),
    );
    set_default(
        &mut meta,
        "created_at",
        Value::String(identity.created_at.clone()),
    );

    if request.retention.pr_bound {
        meta.insert("pr_bound".to_owned(), Value::Bool(true));
    }
    if request.retention.retain_branches {
        meta.insert("retain_branches".to_owned(), Value::Bool(true));
    }
    if request.retention.retain_worktrees {
        meta.insert("retain_worktrees".to_owned(), Value::Bool(true));
    }
    if request.retention.commit_to_target {
        meta.insert("commit_to_target".to_owned(), Value::Bool(true));
    }

    if let Some(branch) = &request.coordination_branch {
        meta.insert(
            "coordination_branch".to_owned(),
            Value::String(branch.clone()),
        );
    } else {
        meta.remove("coordination_branch");
    }
    meta.insert(
        "topology".to_owned(),
        Value::String(request.topology.as_str().to_owned()),
    );
    set_default(&mut meta, "flattened", Value::Bool(false));

    Ok(meta)
}

fn serialize_meta(meta: &BTreeMap<String, Value>) -> Result<String, MissionCreateError> {
    let mut json =
        serde_json::to_string_pretty(meta).map_err(|error| MissionCreateError::Meta {
            path: PathBuf::from(META_FILE_NAME),
            detail: error.to_string(),
        })?;
    json.push('\n');
    Ok(json)
}

/// Typed creation failures.
#[derive(Debug, Error)]
pub enum MissionCreateError {
    /// The project root is missing or not a directory.
    #[error("project root {root} does not exist or is not a directory")]
    RootNotDirectory { root: PathBuf },

    /// The requested slug is not canonical kebab-case.
    #[error("invalid mission slug {value:?}: {reason}")]
    InvalidSlug { value: String, reason: String },

    /// The target branch is not a usable branch name.
    #[error("invalid target branch {value:?}")]
    InvalidBranch { value: String },

    /// The mission type is not a safe identifier.
    #[error("invalid mission type {value:?}")]
    InvalidMissionType { value: String },

    /// An explicitly supplied friendly name normalizes to empty.
    #[error("mission creation requires a non-empty friendly_name")]
    EmptyFriendlyName,

    /// The purpose TL;DR is empty or spans lines.
    #[error("invalid purpose_tldr: {detail}")]
    InvalidPurposeTldr { detail: String },

    /// The purpose context is empty.
    #[error("purpose_context is required and must be a non-empty string")]
    EmptyPurposeContext,

    /// The injected creation timestamp is empty or malformed.
    #[error("invalid created_at timestamp {value:?}")]
    InvalidCreatedAt { value: String },

    /// The injected identity is not a canonical ULID.
    #[error("invalid mission_id {value:?}: {source}")]
    InvalidMissionId {
        value: String,
        #[source]
        source: MissionIdError,
    },

    /// The canonical directory name is not a safe segment.
    #[error("invalid Mission directory name {value:?}: {source}")]
    InvalidDirectoryName {
        value: String,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// `commit_to_target` is only meaningful for `single_branch`.
    #[error("commit_to_target requires topology single_branch, not {topology}")]
    CommitToTargetRequiresSingleBranch { topology: MissionTopology },

    /// A coordination topology needs the workspace owner's branch.
    #[error("topology {topology} requires a coordination_branch supplied by the workspace owner")]
    CoordinationBranchRequired { topology: MissionTopology },

    /// A non-coordination topology must not carry a coordination branch.
    #[error("topology {topology} must not carry a coordination_branch")]
    UnexpectedCoordinationBranch { topology: MissionTopology },

    /// A canonical template is missing or unusable.
    #[error(transparent)]
    Template(#[from] TemplateError),

    /// The Mission directory exists but is not the abandoned prior being repaired.
    #[error("Mission directory {path} already exists")]
    DirectoryExists { path: PathBuf },

    /// A live same-key Mission already exists.
    #[error(
        "a live mission named {base_slug:?} of type {mission_type:?} already exists at {path} (mid8 {mid8}); pass allow_duplicate to create another"
    )]
    AlreadyExists {
        base_slug: String,
        mission_type: String,
        path: PathBuf,
        mid8: String,
    },

    /// The freshly minted identity is already recorded by another Mission.
    #[error("mission_id {id} is already recorded by {path}")]
    MissionIdCollision { id: MissionId, path: PathBuf },

    /// Existing metadata contradicts the injected identity.
    #[error("existing metadata field {field} conflicts with the creation identity: {detail}")]
    IdentityConflict { field: &'static str, detail: String },

    /// The Mission inventory could not be read safely.
    #[error("cannot inspect Mission inventory under {path}: {source}")]
    Inventory {
        path: PathBuf,
        #[source]
        source: Box<MissionError>,
    },

    /// `meta.json` could not be decoded.
    #[error("cannot decode Mission metadata {path}: {detail}")]
    Meta { path: PathBuf, detail: String },

    /// `meta.json` could not be read.
    #[error("cannot read Mission metadata {path}: {source}")]
    MetaRead {
        path: PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// A path in the plan escaped the Mission directory.
    #[error("refusing unsafe planned path {path}")]
    UnsafePath { path: PathBuf },

    /// A target or planned path is a symlink.
    #[error("refusing to follow symlink {path}")]
    SymlinkedTarget { path: PathBuf },

    /// A file destination is a directory.
    #[error("planned file destination {path} is a directory")]
    PathIsDirectory { path: PathBuf },

    /// The plan no longer matches the filesystem.
    #[error("stale Mission creation plan for {path}: {detail}")]
    StalePlan { path: PathBuf, detail: String },

    /// The write lock could not be acquired.
    #[error("cannot acquire Mission write lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: sk_status::mutation::MutationError,
    },

    /// A filesystem operation failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A kernel primitive refused a write.
    #[error("kernel refused to write {path}: {source}")]
    Kernel {
        path: PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// A planned write failed.
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A failed commit could not be fully rolled back.
    #[error("Mission creation rollback failed: {detail}")]
    RollbackFailed { detail: String },
}

// Keep the artifact-name constants referenced so the plan's file set and the
// canonical path module cannot drift silently.
const _: (&str, &str, &str, &str) = (
    SPEC_FILE_NAME,
    PLAN_FILE_NAME,
    TASKS_FILE_NAME,
    TASKS_DIR_NAME,
);

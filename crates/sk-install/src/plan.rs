//! Typed plan → validate → commit for managed project assets.
//!
//! Every mutation this crate performs goes through three explicit phases:
//!
//! 1. **Plan** (pure, read-only): observe the filesystem without following
//!    symlinks, compare with the ownership manifest, and decide an action per
//!    destination. No byte is written while planning, so a caller can inspect
//!    (or refuse) the whole batch before anything changes.
//! 2. **Validate**: reject unsafe destinations, duplicate destinations, and any
//!    parent that is not a directory before the first write.
//! 3. **Commit**: take the project lock, re-check every observation, write in a
//!    deterministic order (files, then deletes, then the manifest), and on a
//!    mid-batch failure persist a *partial* manifest covering only the writes
//!    that succeeded. Recovery therefore never has to guess what landed.
//!
//! Ownership rules that make the batch safe:
//!
//! - an absent path is created;
//! - identical bytes are adopted or kept, never rewritten (idempotence);
//! - a manifest-owned file whose bytes still equal the recorded hash is
//!   regenerated to the canonical render;
//! - a *modified* manifest-owned file is regenerated only on the derived,
//!   git-ignored shared projection (`.agents/skills/`), and preserved untouched
//!   everywhere else (a native root can hold a committed user edit);
//! - a file that is neither owned nor canonical is a collision and is preserved;
//! - symlinks and directories are never written through.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;

use crate::agent::RenderFamily;
use crate::error::InstallError;
use crate::lock::ProjectLock;
use crate::manifest::{
    AssetManifest, MANIFEST_REL, ManifestEntry, content_hash, validate_relative_posix,
};
use crate::render::RenderedAsset;

/// The shared command-skill projection root (project-relative POSIX prefix).
pub const SHARED_PROJECTION_PREFIX: &str = ".agents/skills/";

/// Whether the shared command-skill projection is a DERIVED, git-ignored surface.
///
/// Mirrors `specify_cli.state.contract` → `shared_skills_projection`
/// (`authority = DERIVED`, `git_class = IGNORED`), read as a constant so the
/// regenerate-on-drift branch is self-documenting. If that surface were ever
/// reclassified, this flag must be turned off rather than the branch removed.
pub const SHARED_PROJECTION_IS_DERIVED: bool = true;

/// Current UTC timestamp in the canonical aware-UTC ISO form.
pub fn now_utc_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)
}

/// What a plan will do to one destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlannedAction {
    /// The path is absent and will be created.
    Create,
    /// An unmodified managed file is rewritten to the canonical render.
    Update,
    /// Identical bytes are on disk: record/extend ownership without writing bytes.
    Adopt,
    /// Identical bytes and identical ownership: nothing to do.
    Keep,
    /// A drifted derived projection is regenerated to the canonical render.
    Regenerate,
    /// A managed file is removed (retired path or removed agent).
    Delete,
    /// Existing content is preserved untouched (user edit, unsupported node, edit kept).
    Preserve,
    /// Unowned content occupies a canonical path; the path stays untouched and unclaimed.
    Collision,
}

impl PlannedAction {
    /// Whether this action writes bytes to disk.
    pub fn writes_bytes(self) -> bool {
        matches!(self, Self::Create | Self::Update | Self::Regenerate)
    }

    /// Whether this action deletes a file.
    pub fn deletes(self) -> bool {
        matches!(self, Self::Delete)
    }

    /// Whether this action records ownership in the manifest.
    pub fn claims_ownership(self) -> bool {
        matches!(
            self,
            Self::Create | Self::Update | Self::Adopt | Self::Keep | Self::Regenerate
        )
    }

    /// The stable report/JSON spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Adopt => "adopt",
            Self::Keep => "keep",
            Self::Regenerate => "regenerate",
            Self::Delete => "delete",
            Self::Preserve => "preserve",
            Self::Collision => "collision",
        }
    }
}

/// A filesystem observation made without following the final symlink.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Observed {
    /// Nothing at the path.
    Absent,
    /// A regular file with this content hash.
    File {
        /// SHA-256 of the file's current bytes.
        hash: String,
    },
    /// A directory.
    Directory,
    /// A symlink (target recorded for diagnostics; never followed for writes).
    Symlink {
        /// The link target as recorded.
        target: PathBuf,
    },
    /// A device, FIFO, socket or other special node.
    Other,
}

impl Observed {
    /// A short stable description used in plans and diagnostics.
    pub fn describe(&self) -> String {
        match self {
            Self::Absent => "absent".to_string(),
            Self::File { hash, .. } => format!("file:{hash}"),
            Self::Directory => "directory".to_string(),
            Self::Symlink { target } => format!("symlink:{}", target.display()),
            Self::Other => "special-file".to_string(),
        }
    }
}

/// Observe `path` without following a final symlink.
pub fn observe(path: &Path) -> Result<Observed, InstallError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Observed::Absent),
        Err(source) => return Err(InstallError::io("inspect", path, source)),
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = std::fs::read_link(path)
            .map_err(|source| InstallError::io("read link", path, source))?;
        return Ok(Observed::Symlink { target });
    }
    if file_type.is_dir() {
        return Ok(Observed::Directory);
    }
    if file_type.is_file() {
        let raw = sk_kernel::read_bounded(path, 16 * 1024 * 1024)
            .map_err(|error| InstallError::io("read", path, std::io::Error::other(error)))?;
        return Ok(Observed::File {
            hash: content_hash(&raw),
        });
    }
    Ok(Observed::Other)
}

/// One planned destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedAsset {
    /// Project-relative POSIX destination path.
    pub path: String,
    /// What the commit will do.
    pub action: PlannedAction,
    /// The exact bytes to write (only for writing actions).
    pub bytes: Option<Vec<u8>>,
    /// SHA-256 of `bytes`, or of the observed file for keep/preserve actions.
    pub content_hash: String,
    /// Owner agents recorded after commit.
    pub owners: Vec<String>,
    /// The renderer family, when the asset came from a renderer.
    pub family: Option<RenderFamily>,
    /// The canonical command the asset implements.
    pub command: Option<String>,
    /// SHA-256 of the source template at render time.
    pub source_hash: Option<String>,
    /// Source template path, when one was read.
    pub source_template: Option<PathBuf>,
    /// Human-readable diagnostic (preserve/collision reasons).
    pub note: Option<String>,
}

/// Counts per action, for CLI output and assertions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PlanSummary {
    /// Paths created.
    pub created: usize,
    /// Paths rewritten from an unmodified managed copy.
    pub updated: usize,
    /// Paths adopted without writing bytes.
    pub adopted: usize,
    /// Paths already correct.
    pub kept: usize,
    /// Drifted derived projections regenerated.
    pub regenerated: usize,
    /// Paths removed.
    pub deleted: usize,
    /// Existing content preserved untouched.
    pub preserved: usize,
    /// Unowned content that blocked a canonical path.
    pub collisions: usize,
}

impl PlanSummary {
    /// Whether the commit will change the filesystem.
    pub fn changes_files(&self) -> bool {
        self.created + self.updated + self.regenerated + self.deleted > 0
    }
}

/// A complete, validated-before-commit install plan.
#[derive(Debug)]
pub struct InstallPlan {
    project_root: PathBuf,
    version: String,
    assets: Vec<PlannedAsset>,
    observed: BTreeMap<String, Observed>,
    manifest_before: Option<AssetManifest>,
    manifest_after: AssetManifest,
    manifest_changed: bool,
    warnings: Vec<String>,
}

/// Options controlling plan decisions.
#[derive(Clone, Debug, Default)]
pub struct PlanOptions {
    /// Fixed timestamp (tests); `None` uses the wall clock.
    pub now: Option<String>,
    /// Regenerate drifted derived shared projections (default true).
    pub regenerate_shared: bool,
    /// Record ownership for identical unowned bytes instead of reporting a collision.
    pub adopt_identical: bool,
}

impl PlanOptions {
    /// Production defaults.
    pub fn new() -> Self {
        Self {
            now: None,
            regenerate_shared: SHARED_PROJECTION_IS_DERIVED,
            adopt_identical: true,
        }
    }
}

impl InstallPlan {
    /// Build a plan for `rendered` assets against the current project state.
    pub fn plan(
        project_root: &Path,
        version: &str,
        rendered: &[RenderedAsset],
        options: &PlanOptions,
    ) -> Result<Self, InstallError> {
        ensure_root(project_root)?;
        let manifest_before = AssetManifest::load(project_root)?;
        let timestamp = options.now.clone().unwrap_or_else(now_utc_iso);

        let mut assets: Vec<PlannedAsset> = Vec::new();
        let mut observed: BTreeMap<String, Observed> = BTreeMap::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut claimed_paths: BTreeSet<String> = BTreeSet::new();

        let grouped = group_rendered(rendered)?;
        for (path, group) in grouped {
            validate_relative_posix(&path, "planned asset")?;
            if !claimed_paths.insert(path.clone()) {
                return Err(InstallError::DuplicateDestination { path });
            }
            let owners = group_owners(&group);
            let canonical = group[0].clone();
            let existing = manifest_before
                .as_ref()
                .and_then(|manifest| manifest.find(&path))
                .cloned();
            let absolute = contained_path(project_root, &path)?;
            let state = observe(&absolute)?;
            observed.insert(path.clone(), state.clone());

            let decision = decide(
                &path,
                &state,
                existing.as_ref(),
                &canonical,
                &owners,
                options,
            )?;
            if let Some(note) = &decision.note {
                warnings.push(note.clone());
            }
            assets.push(decision);
        }

        // Retire legacy dotted entries recorded by an older manifest. Only
        // unmodified files this tool wrote are deleted; anything else is kept
        // on disk and simply no longer managed.
        if let Some(manifest) = &manifest_before {
            let legacy: Vec<ManifestEntry> = manifest
                .entries()
                .iter()
                .filter(|entry| is_legacy_dotted_path(&entry.path))
                .cloned()
                .collect();
            for entry in legacy {
                if claimed_paths.contains(&entry.path) {
                    continue;
                }
                let absolute = match contained_path(project_root, &entry.path) {
                    Ok(path) => path,
                    Err(error) => {
                        warnings.push(format!("Preserved legacy entry {}: {}", entry.path, error));
                        assets.push(preserved_entry(
                            &entry,
                            format!("unsafe legacy path preserved: {error}"),
                        ));
                        continue;
                    }
                };
                let state = observe(&absolute)?;
                observed.insert(entry.path.clone(), state.clone());
                let asset = match &state {
                    Observed::Absent => PlannedAsset {
                        path: entry.path.clone(),
                        action: PlannedAction::Delete,
                        bytes: None,
                        content_hash: entry.content_hash.clone(),
                        owners: Vec::new(),
                        family: None,
                        command: None,
                        source_hash: None,
                        source_template: None,
                        note: Some(format!("dropped legacy manifest entry {}", entry.path)),
                    },
                    Observed::File { hash } if hash == &entry.content_hash => PlannedAsset {
                        path: entry.path.clone(),
                        action: PlannedAction::Delete,
                        bytes: None,
                        content_hash: entry.content_hash.clone(),
                        owners: entry.agents.clone(),
                        family: None,
                        command: None,
                        source_hash: None,
                        source_template: None,
                        note: Some(format!("retired legacy command skill {}", entry.path)),
                    },
                    _ => {
                        let note = format!(
                            "Legacy dotted command skill {} has your edits or is not a regular file, so it was kept; it is no longer managed",
                            entry.path
                        );
                        warnings.push(note.clone());
                        PlannedAsset {
                            path: entry.path.clone(),
                            action: PlannedAction::Preserve,
                            bytes: None,
                            content_hash: entry.content_hash.clone(),
                            owners: entry.agents.clone(),
                            family: None,
                            command: None,
                            source_hash: None,
                            source_template: None,
                            note: Some(note),
                        }
                    }
                };
                claimed_paths.insert(entry.path.clone());
                assets.push(asset);
            }
        }

        let (manifest_after, manifest_changed) =
            build_manifest(&manifest_before, &assets, &timestamp, version);

        let plan = Self {
            project_root: project_root.to_path_buf(),
            version: version.to_string(),
            assets,
            observed,
            manifest_before,
            manifest_after,
            manifest_changed,
            warnings,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Build a plan that only removes one agent's ownership (reference counted).
    pub fn plan_agent_removal(
        project_root: &Path,
        version: &str,
        agent_key: &str,
        options: &PlanOptions,
    ) -> Result<Self, InstallError> {
        ensure_root(project_root)?;
        crate::agent::agent_surface(agent_key)?;
        let manifest_before = AssetManifest::load(project_root)?;
        let timestamp = options.now.clone().unwrap_or_else(now_utc_iso);
        let mut assets = Vec::new();
        let mut observed = BTreeMap::new();
        let mut warnings = Vec::new();

        if let Some(manifest) = &manifest_before {
            for entry in manifest.entries() {
                if !entry.agents.iter().any(|owner| owner == agent_key) {
                    continue;
                }
                let absolute = contained_path(project_root, &entry.path)?;
                let state = observe(&absolute)?;
                observed.insert(entry.path.clone(), state.clone());
                let remaining: Vec<String> = entry
                    .agents
                    .iter()
                    .filter(|owner| owner.as_str() != agent_key)
                    .cloned()
                    .collect();
                if !remaining.is_empty() {
                    assets.push(PlannedAsset {
                        path: entry.path.clone(),
                        action: PlannedAction::Adopt,
                        bytes: None,
                        content_hash: entry.content_hash.clone(),
                        owners: remaining,
                        family: None,
                        command: None,
                        source_hash: None,
                        source_template: None,
                        note: Some(format!("released {agent_key} from {}", entry.path)),
                    });
                    continue;
                }
                match &state {
                    Observed::Absent => assets.push(PlannedAsset {
                        path: entry.path.clone(),
                        action: PlannedAction::Delete,
                        bytes: None,
                        content_hash: entry.content_hash.clone(),
                        owners: Vec::new(),
                        family: None,
                        command: None,
                        source_hash: None,
                        source_template: None,
                        note: None,
                    }),
                    Observed::File { hash } if hash == &entry.content_hash => {
                        assets.push(PlannedAsset {
                            path: entry.path.clone(),
                            action: PlannedAction::Delete,
                            bytes: None,
                            content_hash: entry.content_hash.clone(),
                            owners: Vec::new(),
                            family: None,
                            command: None,
                            source_hash: None,
                            source_template: None,
                            note: None,
                        });
                    }
                    _ => {
                        let note = format!(
                            "Edited managed file {} was kept; it is no longer managed",
                            entry.path
                        );
                        warnings.push(note.clone());
                        assets.push(PlannedAsset {
                            path: entry.path.clone(),
                            action: PlannedAction::Preserve,
                            bytes: None,
                            content_hash: entry.content_hash.clone(),
                            owners: Vec::new(),
                            family: None,
                            command: None,
                            source_hash: None,
                            source_template: None,
                            note: Some(note),
                        });
                    }
                }
            }
        }

        let (manifest_after, manifest_changed) =
            build_manifest(&manifest_before, &assets, &timestamp, version);
        let plan = Self {
            project_root: project_root.to_path_buf(),
            version: version.to_string(),
            assets,
            observed,
            manifest_before,
            manifest_after,
            manifest_changed,
            warnings,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// The project root this plan targets.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// The planned assets, in canonical path order.
    pub fn assets(&self) -> &[PlannedAsset] {
        &self.assets
    }

    /// Human-readable warnings (preserved edits, collisions, retired paths).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The manifest as it will be after commit.
    pub fn manifest_after(&self) -> &AssetManifest {
        &self.manifest_after
    }

    /// Whether the commit will rewrite the manifest file.
    pub fn writes_manifest(&self) -> bool {
        self.manifest_changed
    }

    /// Counts per action.
    pub fn summary(&self) -> PlanSummary {
        let mut summary = PlanSummary::default();
        for asset in &self.assets {
            match asset.action {
                PlannedAction::Create => summary.created += 1,
                PlannedAction::Update => summary.updated += 1,
                PlannedAction::Adopt => summary.adopted += 1,
                PlannedAction::Keep => summary.kept += 1,
                PlannedAction::Regenerate => summary.regenerated += 1,
                PlannedAction::Delete => summary.deleted += 1,
                PlannedAction::Preserve => summary.preserved += 1,
                PlannedAction::Collision => summary.collisions += 1,
            }
        }
        summary
    }

    /// Whether committing would change anything at all.
    pub fn is_noop(&self) -> bool {
        !self.summary().changes_files() && !self.manifest_changed
    }

    /// Reject unsafe destinations before the first write.
    pub fn validate(&self) -> Result<(), InstallError> {
        let mut destinations: BTreeSet<&str> = BTreeSet::new();
        for asset in &self.assets {
            validate_relative_posix(&asset.path, "planned asset")?;
            if !destinations.insert(asset.path.as_str()) {
                return Err(InstallError::DuplicateDestination {
                    path: asset.path.clone(),
                });
            }
            if asset.action.writes_bytes() && asset.bytes.is_none() {
                return Err(InstallError::UnsupportedNode {
                    path: PathBuf::from(&asset.path),
                    detail: format!("{:?} action has no bytes", asset.action),
                });
            }
            if asset.action.writes_bytes() || asset.action.deletes() {
                let _ = contained_path(&self.project_root, &asset.path)?;
                let absolute = self.project_root.join(&asset.path);
                if let Some(parent) = absolute.parent() {
                    self.ensure_parent_chain(parent)?;
                }
            }
        }
        Ok(())
    }

    /// Apply the plan: lock, re-check, write, then persist the manifest.
    pub fn commit(&self, lock_wait: Duration) -> Result<ApplicationReport, InstallError> {
        self.validate()?;
        let lock = ProjectLock::for_project(&self.project_root, lock_wait)?;

        // Precondition re-check: the plan was built from these observations.
        for asset in &self.assets {
            let absolute = self.project_root.join(&asset.path);
            let expected = self
                .observed
                .get(&asset.path)
                .cloned()
                .unwrap_or(Observed::Absent);
            let current = observe(&absolute)?;
            if current != expected {
                return Err(InstallError::PreconditionChanged { path: absolute });
            }
        }

        let mut report = ApplicationReport::default();
        let mut succeeded: BTreeSet<String> = BTreeSet::new();

        // Deterministic order: writes first (sorted), then deletions (deepest
        // first so an emptied directory can be removed), then the manifest.
        let mut writes: Vec<&PlannedAsset> = self
            .assets
            .iter()
            .filter(|asset| asset.action.writes_bytes())
            .collect();
        writes.sort_by(|left, right| left.path.cmp(&right.path));
        let mut deletions: Vec<&PlannedAsset> = self
            .assets
            .iter()
            .filter(|asset| asset.action.deletes())
            .collect();
        deletions.sort_by(|left, right| right.path.cmp(&left.path));

        for asset in writes {
            let absolute = self.project_root.join(&asset.path);
            let bytes = asset
                .bytes
                .as_deref()
                .expect("validate() requires bytes for writes");
            if let Some(parent) = absolute.parent()
                && let Err(source) = std::fs::create_dir_all(parent)
            {
                return self.fail_partial(
                    &report,
                    &succeeded,
                    asset,
                    InstallError::io("create parent", parent, source),
                );
            }
            match sk_kernel::atomic_write(&absolute, bytes) {
                Ok(_) => {
                    succeeded.insert(asset.path.clone());
                    report.written.push(asset.path.clone());
                }
                Err(error) if error.mutation_may_have_committed() => {
                    // The replacement may already be visible (a durable rename
                    // followed by a failed directory fsync). Read the bytes back:
                    // an observed match is a success with a durability warning;
                    // anything else is indeterminate and must not be retried or
                    // deleted blindly.
                    let visible = std::fs::read(&absolute).unwrap_or_default();
                    if visible == bytes {
                        succeeded.insert(asset.path.clone());
                        report.written.push(asset.path.clone());
                        report.warnings.push(format!(
                            "{} was written but its directory durability is unconfirmed: {error}",
                            asset.path
                        ));
                    } else {
                        return Err(InstallError::IndeterminateCommit {
                            path: absolute,
                            detail: error.to_string(),
                        });
                    }
                }
                Err(error) => {
                    let failure =
                        InstallError::io("write", &absolute, std::io::Error::other(error));
                    return self.fail_partial(&report, &succeeded, asset, failure);
                }
            }
        }

        for asset in deletions {
            let absolute = self.project_root.join(&asset.path);
            if (absolute.exists() || std::fs::symlink_metadata(&absolute).is_ok())
                && let Err(source) = std::fs::remove_file(&absolute)
            {
                let failure = InstallError::io("remove", &absolute, source);
                return self.fail_partial(&report, &succeeded, asset, failure);
            }
            succeeded.insert(asset.path.clone());
            report.deleted.push(asset.path.clone());
            if let Some(parent) = absolute.parent()
                && parent != self.project_root
            {
                // Never recursive: rmdir succeeds only on an empty directory.
                let _ = std::fs::remove_dir(parent);
            }
        }

        if self.manifest_changed {
            let manifest_path = self.project_root.join(MANIFEST_REL);
            if let Some(parent) = manifest_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|source| InstallError::io("create parent", parent, source))?;
            }
            let manifest_bytes = self.manifest_after.to_bytes();
            match sk_kernel::atomic_write(&manifest_path, &manifest_bytes) {
                Ok(_) => report.manifest_written = true,
                Err(error) if error.mutation_may_have_committed() => {
                    let visible = std::fs::read(&manifest_path).unwrap_or_default();
                    if visible == manifest_bytes {
                        report.manifest_written = true;
                        report.warnings.push(format!(
                            "{} was written but its directory durability is unconfirmed: {error}",
                            MANIFEST_REL
                        ));
                    } else {
                        return Err(InstallError::IndeterminateCommit {
                            path: manifest_path,
                            detail: error.to_string(),
                        });
                    }
                }
                Err(error) => {
                    return Err(InstallError::io(
                        "write manifest",
                        &manifest_path,
                        std::io::Error::other(error),
                    ));
                }
            }
        }

        report.warnings = self.warnings.clone();
        report.preserved = self
            .assets
            .iter()
            .filter(|asset| {
                matches!(
                    asset.action,
                    PlannedAction::Preserve | PlannedAction::Collision
                )
            })
            .map(|asset| asset.path.clone())
            .collect();
        drop(lock);
        Ok(report)
    }

    fn ensure_parent_chain(&self, parent: &Path) -> Result<(), InstallError> {
        let mut components: Vec<&std::ffi::OsStr> = Vec::new();
        let mut cursor = parent;
        while cursor != self.project_root && cursor.starts_with(&self.project_root) {
            match cursor.file_name() {
                Some(name) => components.push(name),
                None => break,
            }
            cursor = match cursor.parent() {
                Some(next) => next,
                None => break,
            };
        }
        components.reverse();
        let mut current = self.project_root.clone();
        for component in components {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_dir() => continue,
                Ok(_) => {
                    return Err(InstallError::NonDirectoryParent {
                        path: self.project_root.join(parent),
                        parent: current,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(source) => return Err(InstallError::io("inspect", &current, source)),
            }
        }
        Ok(())
    }

    fn fail_partial(
        &self,
        report: &ApplicationReport,
        succeeded: &BTreeSet<String>,
        failed: &PlannedAsset,
        error: InstallError,
    ) -> Result<ApplicationReport, InstallError> {
        // A failed apply must leave a manifest that describes only what actually
        // landed, so the next run plans from the truth instead of re-deriving it.
        if self.manifest_changed
            && let Some(partial) = self.partial_manifest(succeeded)
        {
            let manifest_path = self.project_root.join(MANIFEST_REL);
            if let Some(parent) = manifest_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = sk_kernel::atomic_write(&manifest_path, &partial.to_bytes());
        }
        let _ = (report, failed);
        Err(error)
    }

    fn partial_manifest(&self, succeeded: &BTreeSet<String>) -> Option<AssetManifest> {
        let mut manifest = self
            .manifest_before
            .clone()
            .unwrap_or_else(AssetManifest::empty);
        for asset in &self.assets {
            if !succeeded.contains(&asset.path) {
                continue;
            }
            if asset.action.claims_ownership() {
                if let Some(entry) =
                    manifest_entry_for(asset, manifest.find(&asset.path), &self.version)
                {
                    manifest.upsert(entry);
                }
            } else if asset.action.deletes() {
                manifest.remove_path(&asset.path);
            }
        }
        if let Some(before) = &self.manifest_before
            && before.equals_ignoring_timestamps(&manifest)
        {
            return None;
        }
        Some(manifest)
    }

    /// The manifest that was current when the plan was built.
    pub fn manifest_before(&self) -> Option<&AssetManifest> {
        self.manifest_before.as_ref()
    }

    /// The version string recorded in manifest entries.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// The result of a successful commit.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApplicationReport {
    /// Paths whose bytes were written.
    pub written: Vec<String>,
    /// Paths deleted.
    pub deleted: Vec<String>,
    /// Paths preserved untouched (user content, collisions, kept edits).
    pub preserved: Vec<String>,
    /// Whether the ownership manifest was rewritten.
    pub manifest_written: bool,
    /// Warnings collected during planning.
    pub warnings: Vec<String>,
}

impl ApplicationReport {
    /// Whether the commit changed anything at all.
    pub fn is_noop(&self) -> bool {
        self.written.is_empty() && self.deleted.is_empty() && !self.manifest_written
    }
}

fn ensure_root(project_root: &Path) -> Result<(), InstallError> {
    match std::fs::metadata(project_root) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(InstallError::RootNotDirectory {
            root: project_root.to_path_buf(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(InstallError::RootNotDirectory {
                root: project_root.to_path_buf(),
            })
        }
        Err(source) => Err(InstallError::io("inspect root", project_root, source)),
    }
}

fn contained_path(project_root: &Path, relative: &str) -> Result<PathBuf, InstallError> {
    sk_kernel::resolve_within(project_root, Path::new(relative)).map_err(|error| match error {
        sk_kernel::KernelError::PathEscape { .. } => InstallError::PathEscape {
            path: project_root.join(relative),
            root: project_root.to_path_buf(),
        },
        sk_kernel::KernelError::AbsoluteRelativePath { .. }
        | sk_kernel::KernelError::ParentTraversal { .. } => InstallError::UnsafePath {
            path: relative.to_string(),
        },
        other => InstallError::io(
            "resolve",
            project_root.join(relative),
            std::io::Error::other(other),
        ),
    })
}

fn group_rendered(
    rendered: &[RenderedAsset],
) -> Result<BTreeMap<String, Vec<RenderedAsset>>, InstallError> {
    let mut grouped: BTreeMap<String, Vec<RenderedAsset>> = BTreeMap::new();
    for asset in rendered {
        grouped
            .entry(asset.path.clone())
            .or_default()
            .push(asset.clone());
    }
    for (path, group) in &grouped {
        let first = &group[0].bytes;
        if group.iter().any(|asset| &asset.bytes != first) {
            return Err(InstallError::SharedContentConflict { path: path.clone() });
        }
    }
    Ok(grouped)
}

fn group_owners(group: &[RenderedAsset]) -> Vec<String> {
    let owners: BTreeSet<String> = group.iter().map(|asset| asset.agent.to_string()).collect();
    owners.into_iter().collect()
}

fn decide(
    path: &str,
    state: &Observed,
    existing: Option<&ManifestEntry>,
    canonical: &RenderedAsset,
    owners: &[String],
    options: &PlanOptions,
) -> Result<PlannedAsset, InstallError> {
    let hash = content_hash(&canonical.bytes);
    let base = |action: PlannedAction| PlannedAsset {
        path: path.to_string(),
        action,
        bytes: None,
        content_hash: hash.clone(),
        owners: owners.to_vec(),
        family: Some(canonical.family),
        command: Some(canonical.command.clone()),
        source_hash: canonical.source_hash.clone(),
        source_template: canonical.source_template.clone(),
        note: None,
    };
    let with_bytes = |action: PlannedAction| PlannedAsset {
        bytes: Some(canonical.bytes.clone()),
        ..base(action)
    };

    Ok(match (state, existing) {
        // Never write through a symlink or a directory: those are user/3rd-party nodes.
        (Observed::Symlink { .. }, _) => PlannedAsset {
            note: Some(format!(
                "Preserved {path}: a symlink occupies this managed path"
            )),
            ..base(PlannedAction::Preserve)
        },
        (Observed::Directory, _) => PlannedAsset {
            note: Some(format!(
                "Preserved {path}: a directory occupies this managed path"
            )),
            ..base(PlannedAction::Preserve)
        },
        (Observed::Other, _) => PlannedAsset {
            note: Some(format!(
                "Preserved {path}: a special file occupies this managed path"
            )),
            ..base(PlannedAction::Preserve)
        },
        (Observed::Absent, _) => with_bytes(PlannedAction::Create),
        (Observed::File { hash: on_disk }, Some(entry)) => {
            if on_disk == &hash {
                if entry.content_hash == hash && entry.agents == owners {
                    base(PlannedAction::Keep)
                } else {
                    base(PlannedAction::Adopt)
                }
            } else if on_disk == &entry.content_hash {
                // Unmodified managed copy: the canonical render moved on.
                with_bytes(PlannedAction::Update)
            } else if path.starts_with(SHARED_PROJECTION_PREFIX) && options.regenerate_shared {
                PlannedAsset {
                    note: Some(format!(
                        "Regenerated drifted derived command-skill projection {path}"
                    )),
                    ..with_bytes(PlannedAction::Regenerate)
                }
            } else {
                PlannedAsset {
                    note: Some(format!(
                        "Preserved {path}: the managed file has your edits, so it was not overwritten"
                    )),
                    ..base(PlannedAction::Preserve)
                }
            }
        }
        (Observed::File { hash: on_disk }, None) => {
            if on_disk == &hash && options.adopt_identical {
                base(PlannedAction::Adopt)
            } else {
                PlannedAsset {
                    note: Some(format!(
                        "Your own file blocks command skill {path} — rename or remove it, then re-run init"
                    )),
                    ..base(PlannedAction::Collision)
                }
            }
        }
    })
}

fn preserved_entry(entry: &ManifestEntry, note: String) -> PlannedAsset {
    PlannedAsset {
        path: entry.path.clone(),
        action: PlannedAction::Preserve,
        bytes: None,
        content_hash: entry.content_hash.clone(),
        owners: entry.agents.clone(),
        family: None,
        command: None,
        source_hash: None,
        source_template: None,
        note: Some(note),
    }
}

fn is_legacy_dotted_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(SHARED_PROJECTION_PREFIX) else {
        return false;
    };
    let Some(name) = rest.strip_suffix("/SKILL.md") else {
        return false;
    };
    let Some(command) = name.strip_prefix(crate::render::LEGACY_COMMAND_SKILL_PREFIX) else {
        return false;
    };
    crate::agent::is_canonical_command(command)
}

fn manifest_entry_for(
    asset: &PlannedAsset,
    existing: Option<&ManifestEntry>,
    version: &str,
) -> Option<ManifestEntry> {
    if asset.owners.is_empty() {
        return None;
    }
    let installed_at = existing
        .map(|entry| entry.installed_at.clone())
        .unwrap_or_else(now_utc_iso);
    ManifestEntry::new(
        asset.path.clone(),
        asset.content_hash.clone(),
        asset.owners.clone(),
        installed_at,
        version.to_string(),
    )
    .ok()
}

fn build_manifest(
    before: &Option<AssetManifest>,
    assets: &[PlannedAsset],
    timestamp: &str,
    version: &str,
) -> (AssetManifest, bool) {
    let mut manifest = before.clone().unwrap_or_else(AssetManifest::empty);
    for asset in assets {
        if asset.action.claims_ownership() {
            if asset.owners.is_empty() {
                continue;
            }
            let installed_at = manifest
                .find(&asset.path)
                .map(|entry| entry.installed_at.clone())
                .unwrap_or_else(|| timestamp.to_string());
            if let Ok(entry) = ManifestEntry::new(
                asset.path.clone(),
                asset.content_hash.clone(),
                asset.owners.clone(),
                installed_at,
                version,
            ) {
                manifest.upsert(entry);
            }
        } else if asset.action.deletes() {
            manifest.remove_path(&asset.path);
        }
    }
    let changed = match before {
        Some(previous) => !previous.equals_ignoring_timestamps(&manifest),
        None => !manifest.entries().is_empty(),
    };
    (manifest, changed)
}

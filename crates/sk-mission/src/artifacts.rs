//! Authoring of `plan.md`, `tasks.md`, and work-package prompt scaffolds.
//!
//! This is the Mission-authoring companion to [`crate::create`]: creation
//! births `meta.json`, `spec.md`, and the tasks directory; this module authors
//! the remaining planning files from **injected** canonical templates.
//!
//! Nothing here invents spec/plan/tasks/work-package prose. The caller injects
//! the template the charter/template resolver selected, and this module copies
//! it byte-for-byte for `spec.md`/`plan.md`/`tasks.md` or renders the canonical
//! work-package placeholders for a prompt. Existing files are never replaced:
//! a re-run preserves every user-authored edit and reports it as preserved.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sk_kernel::locks::LockWait;
use sk_kernel::{MissionSlug, WpId, atomic_write};

use crate::paths::MissionPaths;
use thiserror::Error;

use crate::create::MissionWriteObserver;
use crate::paths::{
    PLAN_FILE_NAME, SPEC_FILE_NAME, TASKS_DIR_NAME, TASKS_FILE_NAME, TASKS_README_NAME,
};
use crate::template::{
    MissionTemplateKind, MissionTemplates, TemplateError, WorkPackageTemplateContext,
    render_tasks_readme,
};

const LOCK_TIMEOUT: std::time::Duration = crate::write_lock::MISSION_WRITE_LOCK_TIMEOUT;

/// A work-package prompt the caller wants authored.
#[derive(Clone, Debug)]
pub struct WorkPackageDraft {
    wp_id: String,
    title: String,
    dependencies: Vec<String>,
    subtasks: Vec<String>,
    phase: String,
    file_name: Option<String>,
}

impl WorkPackageDraft {
    /// Build a draft for `wp_id` (for example `WP01`) with a title.
    pub fn new(wp_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            wp_id: wp_id.into(),
            title: title.into(),
            dependencies: Vec::new(),
            subtasks: Vec::new(),
            phase: String::new(),
            file_name: None,
        }
    }

    /// Set declared dependencies.
    pub fn with_dependencies<I, S>(mut self, dependencies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.dependencies = dependencies.into_iter().map(Into::into).collect();
        self
    }

    /// Set subtask ids.
    pub fn with_subtasks<I, S>(mut self, subtasks: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.subtasks = subtasks.into_iter().map(Into::into).collect();
        self
    }

    /// Set the phase label.
    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = phase.into();
        self
    }

    /// Use an explicit `WP##-slug.md` filename.
    pub fn with_file_name(
        mut self,
        file_name: impl Into<String>,
    ) -> Result<Self, TaskArtifactError> {
        let file_name = file_name.into();
        validate_wp_file_name(&file_name, &self.wp_id)?;
        self.file_name = Some(file_name);
        Ok(self)
    }

    /// The canonical draft id.
    pub fn wp_id(&self) -> &str {
        &self.wp_id
    }

    /// The draft title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The canonical filename (`WP##-kebab-slug.md` or the explicit name).
    pub fn file_name(&self) -> Result<String, TaskArtifactError> {
        if let Some(file_name) = &self.file_name {
            return Ok(file_name.clone());
        }
        let wp_id = canonical_wp_id(&self.wp_id)?;
        let slug = slugify(&self.title);
        if slug.is_empty() {
            return Err(TaskArtifactError::InvalidDraft {
                wp: self.wp_id.clone(),
                detail: "a non-empty title is required to compose the prompt filename".to_owned(),
            });
        }
        Ok(format!("{wp_id}-{slug}.md"))
    }

    fn parsed_id(&self) -> Result<WpId, TaskArtifactError> {
        canonical_wp_id(&self.wp_id)
    }

    fn parsed_dependencies(&self) -> Result<Vec<WpId>, TaskArtifactError> {
        self.dependencies
            .iter()
            .map(|value| {
                WpId::from_str(value).map_err(|error| TaskArtifactError::InvalidDraft {
                    wp: self.wp_id.clone(),
                    detail: format!("invalid dependency {value:?}: {error}"),
                })
            })
            .collect()
    }
}

/// A request to author the remaining planning artifacts of one Mission.
#[derive(Clone, Debug)]
pub struct MissionArtifactRequest {
    planning_branch: String,
    target_branch: String,
    created_at: String,
    drafts: Vec<WorkPackageDraft>,
}

impl MissionArtifactRequest {
    /// Build a request with the branch and timestamp values the templates need.
    pub fn new(
        planning_branch: impl Into<String>,
        target_branch: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Self {
            planning_branch: planning_branch.into(),
            target_branch: target_branch.into(),
            created_at: created_at.into(),
            drafts: Vec::new(),
        }
    }

    /// Add a work-package draft.
    pub fn with_work_package(mut self, draft: WorkPackageDraft) -> Self {
        self.drafts.push(draft);
        self
    }

    /// Drafts in request order.
    pub fn work_packages(&self) -> &[WorkPackageDraft] {
        &self.drafts
    }
}

/// One planned artifact file.
#[derive(Clone, Debug)]
pub struct ArtifactFile {
    path: PathBuf,
    relative: PathBuf,
    kind: MissionTemplateKind,
    bytes: Vec<u8>,
    template_id: String,
}

impl ArtifactFile {
    /// Absolute destination.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path relative to the Mission directory.
    pub fn relative(&self) -> &Path {
        &self.relative
    }

    /// Artifact kind.
    pub fn kind(&self) -> MissionTemplateKind {
        self.kind
    }

    /// Planned bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Resolver id of the injected template.
    pub fn template_id(&self) -> &str {
        &self.template_id
    }
}

/// A validated, read-only artifact-authoring plan.
#[derive(Clone, Debug)]
pub struct MissionArtifactPlan {
    root: PathBuf,
    mission_dir: PathBuf,
    files: Vec<ArtifactFile>,
    directories: Vec<PathBuf>,
}

impl MissionArtifactPlan {
    /// Plan the artifact scaffolding for an existing Mission directory.
    pub fn plan(
        root: &Path,
        mission_slug: MissionSlug,
        templates: &MissionTemplates,
        request: &MissionArtifactRequest,
    ) -> Result<Self, TaskArtifactError> {
        if request.planning_branch.trim().is_empty()
            || request.target_branch.trim().is_empty()
            || request.created_at.trim().is_empty()
        {
            return Err(TaskArtifactError::InvalidRequest {
                detail: "planning branch, target branch, and created_at are required".to_owned(),
            });
        }
        let specs_dir = root.join("kitty-specs");
        if fs::symlink_metadata(&specs_dir).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(TaskArtifactError::SymlinkedEntry { path: specs_dir });
        }
        let lexical_mission_dir = specs_dir.join(mission_slug.as_str());
        if fs::symlink_metadata(&lexical_mission_dir)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(TaskArtifactError::SymlinkedEntry {
                path: lexical_mission_dir,
            });
        }
        let paths = MissionPaths::new(root, mission_slug.clone())?;
        let mission_dir = paths.spec_dir().to_path_buf();
        match fs::symlink_metadata(&mission_dir) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(TaskArtifactError::SymlinkedEntry { path: mission_dir });
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(TaskArtifactError::MissionDirectoryMissing { path: mission_dir });
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(TaskArtifactError::MissionDirectoryMissing { path: mission_dir });
            }
            Err(source) => {
                return Err(TaskArtifactError::Io {
                    action: "inspect",
                    path: mission_dir,
                    source,
                });
            }
        }

        let spec = require(templates, MissionTemplateKind::Spec)?;
        let plan = require(templates, MissionTemplateKind::Plan)?;
        let tasks = require(templates, MissionTemplateKind::Tasks)?;
        let wp_template = require(templates, MissionTemplateKind::WorkPackage)?;
        for (template, kind) in [
            (spec, MissionTemplateKind::Spec),
            (plan, MissionTemplateKind::Plan),
            (tasks, MissionTemplateKind::Tasks),
            (wp_template, MissionTemplateKind::WorkPackage),
        ] {
            if template.kind() != kind {
                return Err(TaskArtifactError::Template(TemplateError::MismatchedKind {
                    id: template.id().to_owned(),
                    expected: kind,
                    actual: template.kind(),
                }));
            }
        }

        let tasks_dir = mission_dir.join(TASKS_DIR_NAME);
        if fs::symlink_metadata(&tasks_dir).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(TaskArtifactError::SymlinkedEntry { path: tasks_dir });
        }
        let existing = existing_work_packages(&tasks_dir)?;
        let mut drafts: BTreeSet<WpId> = BTreeSet::new();
        let mut files = vec![
            artifact(
                &mission_dir,
                Path::new(SPEC_FILE_NAME),
                MissionTemplateKind::Spec,
                spec.content(),
                spec.id(),
            ),
            artifact(
                &mission_dir,
                Path::new(PLAN_FILE_NAME),
                MissionTemplateKind::Plan,
                plan.content(),
                plan.id(),
            ),
            artifact(
                &mission_dir,
                Path::new(TASKS_FILE_NAME),
                MissionTemplateKind::Tasks,
                tasks.content(),
                tasks.id(),
            ),
            artifact(
                &mission_dir,
                &Path::new(TASKS_DIR_NAME).join(TASKS_README_NAME),
                MissionTemplateKind::Tasks,
                &render_tasks_readme(&request.planning_branch),
                "scaffold/tasks-readme",
            ),
        ];

        for draft in &request.drafts {
            let wp_id = draft.parsed_id()?;
            if !drafts.insert(wp_id.clone()) {
                return Err(TaskArtifactError::DuplicateDraft { wp: wp_id });
            }
            let dependencies = draft.parsed_dependencies()?;
            if dependencies.iter().any(|dependency| dependency == &wp_id) {
                return Err(TaskArtifactError::SelfDependency { wp: wp_id });
            }
            let file_name = draft.file_name()?;
            if let Some(existing_path) = existing.get(&wp_id)
                && existing_path.file_name().and_then(|name| name.to_str()) != Some(&file_name)
            {
                return Err(TaskArtifactError::WorkPackageIdConflict {
                    wp: wp_id,
                    existing: existing_path.clone(),
                    planned: tasks_dir.join(&file_name),
                });
            }

            let context = WorkPackageTemplateContext::new(
                wp_id.clone(),
                draft.title.clone(),
                request.planning_branch.clone(),
                request.target_branch.clone(),
                request.created_at.clone(),
            )
            .with_phase(draft.phase.clone())
            .with_dependencies(dependencies)
            .with_subtasks(draft.subtasks.clone());
            let rendered = context.render(wp_template)?;
            files.push(artifact(
                &mission_dir,
                &Path::new(TASKS_DIR_NAME).join(&file_name),
                MissionTemplateKind::WorkPackage,
                &rendered,
                wp_template.id(),
            ));
        }

        let plan = Self {
            root: root.to_path_buf(),
            mission_dir: mission_dir.clone(),
            files,
            directories: vec![tasks_dir],
        };
        plan.validate(root)?;
        Ok(plan)
    }

    /// Absolute Mission directory.
    pub fn mission_dir(&self) -> &Path {
        &self.mission_dir
    }

    /// Planned artifact files.
    pub fn planned_files(&self) -> &[ArtifactFile] {
        &self.files
    }

    /// Revalidate against the current filesystem.
    pub fn validate(&self, root: &Path) -> Result<(), TaskArtifactError> {
        if root != self.root {
            return Err(TaskArtifactError::StalePlan {
                path: root.to_path_buf(),
                detail: "plan was built for a different project root".to_owned(),
            });
        }
        if fs::symlink_metadata(&self.mission_dir)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(TaskArtifactError::SymlinkedEntry {
                path: self.mission_dir.clone(),
            });
        }
        if !self.mission_dir.is_dir() {
            return Err(TaskArtifactError::MissionDirectoryMissing {
                path: self.mission_dir.clone(),
            });
        }
        for file in &self.files {
            if !file.path.starts_with(&self.mission_dir) {
                return Err(TaskArtifactError::UnsafePath {
                    path: file.path.clone(),
                });
            }
            match fs::symlink_metadata(&file.path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(TaskArtifactError::SymlinkedEntry {
                        path: file.path.clone(),
                    });
                }
                Ok(metadata) if metadata.is_dir() => {
                    return Err(TaskArtifactError::PathIsDirectory {
                        path: file.path.clone(),
                    });
                }
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(TaskArtifactError::Io {
                        action: "inspect",
                        path: file.path.clone(),
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    /// Commit the plan.
    pub fn commit(&self, root: &Path) -> Result<MissionArtifactReceipt, TaskArtifactError> {
        self.commit_with_observer(root, &mut ())
    }

    /// Commit with a write observer, used by recovery and failure-injection tests.
    pub fn commit_with_observer(
        &self,
        root: &Path,
        observer: &mut dyn MissionWriteObserver,
    ) -> Result<MissionArtifactReceipt, TaskArtifactError> {
        self.validate(root)?;
        // Plan/tasks authoring is a post-birth Mission write: take the shared
        // keyed Mission lock, so this writer serializes with the status writers
        // and with a coordination-routed Mission's primary/coordination surface.
        let _lock = crate::write_lock::lock_mission(
            &self.mission_dir,
            root,
            LockWait::Bounded(LOCK_TIMEOUT),
        )
        .map_err(|source| TaskArtifactError::Lock {
            path: self.mission_dir.clone(),
            source,
        })?;
        self.validate(root)?;

        let mut transaction = ArtifactTransaction::new();
        if let Err(error) = self.write_all(observer, &mut transaction) {
            if let Err(rollback) = transaction.rollback() {
                return Err(TaskArtifactError::RollbackFailed {
                    detail: format!("{error}; rollback also failed: {rollback}"),
                });
            }
            return Err(error);
        }
        Ok(MissionArtifactReceipt {
            mission_dir: self.mission_dir.clone(),
            created: transaction.created,
            preserved: transaction.preserved,
            unchanged: transaction.unchanged,
        })
    }

    fn write_all(
        &self,
        observer: &mut dyn MissionWriteObserver,
        transaction: &mut ArtifactTransaction,
    ) -> Result<(), TaskArtifactError> {
        for directory in &self.directories {
            transaction.ensure_directory(directory)?;
        }
        for file in &self.files {
            transaction.write(&file.path, &file.bytes, observer)?;
        }
        Ok(())
    }
}

/// The result of committing an artifact plan.
#[derive(Clone, Debug)]
pub struct MissionArtifactReceipt {
    mission_dir: PathBuf,
    created: Vec<PathBuf>,
    preserved: Vec<PathBuf>,
    unchanged: Vec<PathBuf>,
}

impl MissionArtifactReceipt {
    /// Mission directory.
    pub fn mission_dir(&self) -> &Path {
        &self.mission_dir
    }

    /// Files created by this commit.
    pub fn created(&self) -> &[PathBuf] {
        &self.created
    }

    /// User-authored files preserved untouched.
    pub fn preserved(&self) -> &[PathBuf] {
        &self.preserved
    }

    /// Files already byte-identical to the plan.
    pub fn unchanged(&self) -> &[PathBuf] {
        &self.unchanged
    }
}

fn require(
    templates: &MissionTemplates,
    kind: MissionTemplateKind,
) -> Result<&crate::template::CanonicalTemplate, TaskArtifactError> {
    templates.require(kind).map_err(|source| match source {
        TemplateError::Missing { kind } => TaskArtifactError::MissingTemplate { kind },
        other => TaskArtifactError::Template(other),
    })
}

fn artifact(
    mission_dir: &Path,
    relative: &Path,
    kind: MissionTemplateKind,
    content: &str,
    template_id: &str,
) -> ArtifactFile {
    ArtifactFile {
        path: mission_dir.join(relative),
        relative: relative.to_path_buf(),
        kind,
        bytes: content.as_bytes().to_vec(),
        template_id: template_id.to_owned(),
    }
}

fn existing_work_packages(tasks_dir: &Path) -> Result<BTreeMap<WpId, PathBuf>, TaskArtifactError> {
    if fs::symlink_metadata(tasks_dir).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(TaskArtifactError::SymlinkedEntry {
            path: tasks_dir.to_path_buf(),
        });
    }
    let mut found = BTreeMap::new();
    let entries = match fs::read_dir(tasks_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(found),
        Err(source) => {
            return Err(TaskArtifactError::Io {
                action: "list",
                path: tasks_dir.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| TaskArtifactError::Io {
            action: "list",
            path: tasks_dir.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| TaskArtifactError::Io {
            action: "inspect",
            path: entry.path(),
            source,
        })?;
        if file_type.is_symlink() {
            return Err(TaskArtifactError::SymlinkedEntry { path: entry.path() });
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(TaskArtifactError::NonUtf8FileName { path: entry.path() });
        };
        if !name.ends_with(".md") {
            continue;
        }
        let Some(rest) = name.strip_prefix("WP") else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.len() < 2 {
            continue;
        }
        let wp_id = WpId::from_str(&format!("WP{digits}")).map_err(|error| {
            TaskArtifactError::InvalidDraft {
                wp: format!("WP{digits}"),
                detail: error.to_string(),
            }
        })?;
        if let Some(previous) = found.insert(wp_id.clone(), entry.path()) {
            return Err(TaskArtifactError::DuplicateWorkPackage {
                wp: wp_id,
                first: previous,
                second: entry.path(),
            });
        }
    }
    Ok(found)
}

fn canonical_wp_id(value: &str) -> Result<WpId, TaskArtifactError> {
    WpId::from_str(value).map_err(|error| TaskArtifactError::InvalidDraft {
        wp: value.to_owned(),
        detail: error.to_string(),
    })
}

fn validate_wp_file_name(file_name: &str, wp_id: &str) -> Result<(), TaskArtifactError> {
    let wp_id = canonical_wp_id(wp_id)?;
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || !file_name.ends_with(".md")
        || file_name == ".md"
    {
        return Err(TaskArtifactError::InvalidDraft {
            wp: wp_id.to_string(),
            detail: format!("invalid work-package filename {file_name:?}"),
        });
    }
    let Some(rest) = file_name.strip_prefix("WP") else {
        return Err(TaskArtifactError::InvalidDraft {
            wp: wp_id.to_string(),
            detail: format!("work-package filename {file_name:?} must start with WP"),
        });
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if format!("WP{digits}") != wp_id.as_str() {
        return Err(TaskArtifactError::InvalidDraft {
            wp: wp_id.to_string(),
            detail: format!("work-package filename {file_name:?} does not name {wp_id}"),
        });
    }
    Ok(())
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }
    slug.trim_end_matches('-').to_owned()
}

struct ArtifactTransaction {
    created: Vec<PathBuf>,
    preserved: Vec<PathBuf>,
    unchanged: Vec<PathBuf>,
    created_dirs: Vec<PathBuf>,
    backups: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl ArtifactTransaction {
    fn new() -> Self {
        Self {
            created: Vec::new(),
            preserved: Vec::new(),
            unchanged: Vec::new(),
            created_dirs: Vec::new(),
            backups: Vec::new(),
        }
    }

    fn ensure_directory(&mut self, path: &Path) -> Result<(), TaskArtifactError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(TaskArtifactError::SymlinkedEntry {
                    path: path.to_path_buf(),
                })
            }
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(TaskArtifactError::PathIsDirectory {
                path: path.to_path_buf(),
            }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(path).map_err(|source| TaskArtifactError::Io {
                    action: "create directory",
                    path: path.to_path_buf(),
                    source,
                })?;
                self.created_dirs.push(path.to_path_buf());
                Ok(())
            }
            Err(source) => Err(TaskArtifactError::Io {
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
        observer: &mut dyn MissionWriteObserver,
    ) -> Result<(), TaskArtifactError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(TaskArtifactError::SymlinkedEntry {
                    path: path.to_path_buf(),
                });
            }
            Ok(metadata) if metadata.is_dir() => {
                return Err(TaskArtifactError::PathIsDirectory {
                    path: path.to_path_buf(),
                });
            }
            Ok(_) => {
                let current = fs::read(path).map_err(|source| TaskArtifactError::Io {
                    action: "read",
                    path: path.to_path_buf(),
                    source,
                })?;
                if current == bytes {
                    self.unchanged.push(path.to_path_buf());
                } else {
                    self.preserved.push(path.to_path_buf());
                }
                return Ok(());
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(TaskArtifactError::Io {
                    action: "inspect",
                    path: path.to_path_buf(),
                    source,
                });
            }
        }

        if let Err(source) = observer.before_write(path) {
            return Err(TaskArtifactError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        // Register intent first: a failed directory sync can follow a successful
        // rename, so an error does not imply that this path was left untouched.
        self.backups.push((path.to_path_buf(), None));
        atomic_write(path, bytes).map_err(|source| TaskArtifactError::Kernel {
            path: path.to_path_buf(),
            source,
        })?;
        self.created.push(path.to_path_buf());
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), String> {
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
                failure = Some(error.to_string());
            }
        }
        for directory in self.created_dirs.iter().rev() {
            let _ = fs::remove_dir(directory);
        }
        match failure {
            Some(detail) => Err(detail),
            None => Ok(()),
        }
    }
}

/// Typed artifact-authoring failures.
#[derive(Debug, Error)]
pub enum TaskArtifactError {
    /// The Mission directory does not exist.
    #[error("Mission directory {path} does not exist")]
    MissionDirectoryMissing { path: PathBuf },

    /// The request is missing a required branch or timestamp.
    #[error("invalid artifact request: {detail}")]
    InvalidRequest { detail: String },

    /// A required canonical template was not injected.
    #[error("no canonical {kind} template was supplied")]
    MissingTemplate { kind: MissionTemplateKind },

    /// A canonical template is unusable.
    #[error(transparent)]
    Template(#[from] TemplateError),

    /// A draft is malformed.
    #[error("invalid work-package draft {wp}: {detail}")]
    InvalidDraft { wp: String, detail: String },

    /// Two drafts claim the same work-package id.
    #[error("work package draft {wp} is declared more than once")]
    DuplicateDraft { wp: WpId },

    /// A draft declares itself as a dependency.
    #[error("work package draft {wp} depends on itself")]
    SelfDependency { wp: WpId },

    /// An existing prompt for this id uses another filename.
    #[error("work package {wp} already exists at {existing}; refusing to create {planned}")]
    WorkPackageIdConflict {
        wp: WpId,
        existing: PathBuf,
        planned: PathBuf,
    },

    /// Two prompt files claim the same id.
    #[error("work package {wp} is declared by both {first} and {second}")]
    DuplicateWorkPackage {
        wp: WpId,
        first: PathBuf,
        second: PathBuf,
    },

    /// A path is a symlink.
    #[error("refusing to follow symlink {path}")]
    SymlinkedEntry { path: PathBuf },

    /// A tasks entry has a non-UTF-8 name.
    #[error("tasks entry {path} has a non-UTF-8 filename")]
    NonUtf8FileName { path: PathBuf },

    /// A planned file destination is a directory.
    #[error("planned file destination {path} is a directory")]
    PathIsDirectory { path: PathBuf },

    /// A planned path escaped the Mission directory.
    #[error("refusing unsafe planned path {path}")]
    UnsafePath { path: PathBuf },

    /// The plan no longer matches the filesystem.
    #[error("stale artifact plan for {path}: {detail}")]
    StalePlan { path: PathBuf, detail: String },

    /// A canonical planning path was refused.
    #[error(transparent)]
    Path(#[from] crate::paths::MissionPathError),

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
    #[error("kernel refused {path}: {source}")]
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
    #[error("artifact-completion rollback failed: {detail}")]
    RollbackFailed { detail: String },
}

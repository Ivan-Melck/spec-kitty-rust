//! Full `.kittify` template-tree installation and the scratch prover.
//!
//! Two modes exist upstream and both are represented here:
//!
//! - **Full copy** — the package/local-checkout path copies `.kittify/memory`,
//!   `.kittify/templates` (the offering template tree, plus `AGENTS.md` and the
//!   `.claudeignore` template) and `.kittify/missions` into the project.
//! - **Global runtime** — when the user's `~/.kittify` runtime is populated,
//!   `init` writes only the minimal project skeleton (`memory/`); missions and
//!   templates resolve from the global home at runtime.
//!
//! Both modes preserve operator content: an existing `memory/`, `templates/` or
//! `missions/` subtree is **moved** into a timestamped, collision-safe
//! `.kittify/.backup-<UTC>/` directory before the scaffold writes, never
//! recursively deleted. `templates_created` records provenance for *this run*
//! (the #4931 lesson): a caller may only treat `.kittify/templates` as
//! run-owned when this invocation actually created it, and the scratch prover
//! ([`guard_destructive_removal`]) refuses to remove anything not proven.
//!
//! Sources must be explicit and verified. A pack root is checked against its
//! pin before any byte is copied; there is no bundled fallback template tree
//! and no invented mission set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::pack::{copy_tree_bounded, remove_tree_bounded};
use crate::pin::PackPin;

/// Default upper bound for a template-tree copy (256 MiB).
pub const MAX_TEMPLATE_TREE_BYTES: u64 = 256 * 1024 * 1024;

/// The mode a template-tree install used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateMode {
    /// Copy memory, templates and missions into the project.
    FullCopy,
    /// Write the minimal project skeleton; resolve shared assets globally.
    GlobalRuntime,
}

impl TemplateMode {
    /// Stable CLI spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FullCopy => "full_copy",
            Self::GlobalRuntime => "global_runtime",
        }
    }
}

/// A verified source for the project's `.kittify` assets.
#[derive(Clone, Debug)]
pub struct TemplateTreeSource {
    memory_root: Option<PathBuf>,
    templates_root: Option<PathBuf>,
    missions_root: Option<PathBuf>,
    agents_md: Option<PathBuf>,
    claudeignore_template: Option<PathBuf>,
    global_home: Option<PathBuf>,
}

impl TemplateTreeSource {
    /// Build a source from explicit roots; every root is optional and absence is
    /// reported rather than replaced by a bundled copy.
    pub fn new(
        memory_root: Option<PathBuf>,
        templates_root: Option<PathBuf>,
        missions_root: Option<PathBuf>,
    ) -> Self {
        let agents_md = templates_root.as_ref().map(|root| root.join("AGENTS.md"));
        let claudeignore_template = templates_root
            .as_ref()
            .map(|root| root.join("claudeignore-template"));
        Self {
            memory_root,
            templates_root,
            missions_root,
            agents_md,
            claudeignore_template,
            global_home: None,
        }
    }

    /// Discover the canonical checkout layout (`src/charter/offering/templates`,
    /// `packs/built-in/missions`, `src/specify_cli/memory`).
    pub fn discover(checkout: &Path) -> Result<Self, InstallError> {
        let templates = checkout.join("src/charter/offering/templates");
        let missions = checkout.join("packs/built-in/missions");
        let memory = checkout.join("src/specify_cli/memory");
        if !templates.is_dir() {
            return Err(InstallError::TemplateTreeMissing {
                path: templates,
                detail: "offering templates root is absent".to_string(),
            });
        }
        if !missions.is_dir() {
            return Err(InstallError::TemplateTreeMissing {
                path: missions,
                detail: "the built-in mission tree is absent; run the pack sync first".to_string(),
            });
        }
        Ok(Self::new(
            memory.is_dir().then_some(memory),
            Some(templates),
            Some(missions),
        ))
    }

    /// The memory source, when configured.
    pub fn memory_root(&self) -> Option<&Path> {
        self.memory_root.as_deref()
    }

    /// The offering templates source, when configured.
    pub fn templates_root(&self) -> Option<&Path> {
        self.templates_root.as_deref()
    }

    /// The mission tree source, when configured.
    pub fn missions_root(&self) -> Option<&Path> {
        self.missions_root.as_deref()
    }

    /// Mark this source as the global runtime home (`~/.kittify`) for a minimal install.
    pub fn with_global_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.global_home = Some(home.into());
        self
    }

    /// Override the `AGENTS.md` source.
    pub fn with_agents_md(mut self, path: impl Into<PathBuf>) -> Self {
        self.agents_md = Some(path.into());
        self
    }

    /// Override the `.claudeignore` template source.
    pub fn with_claudeignore_template(mut self, path: impl Into<PathBuf>) -> Self {
        self.claudeignore_template = Some(path.into());
        self
    }

    /// Verify the missions tree against its pack pin (no copy happens here).
    pub fn verify_missions_pin(&self, pin: &PackPin, pin_path: &Path) -> Result<(), InstallError> {
        let missions =
            self.missions_root
                .as_ref()
                .ok_or_else(|| InstallError::TemplateTreeMissing {
                    path: PathBuf::from("<missions>"),
                    detail: "no mission tree is configured".to_string(),
                })?;
        // The pin covers the pack root; the mission tree lives inside it.
        let pack_root = missions
            .parent()
            .ok_or_else(|| InstallError::TemplateTreeUnverified {
                path: missions.clone(),
                detail: "mission root has no parent pack directory".to_string(),
            })?;
        pin.verify(pin_path, pack_root)
    }

    /// Whether a global runtime is populated (missions contains a directory).
    pub fn global_runtime_available(&self) -> bool {
        self.global_home
            .as_ref()
            .map(|home| global_runtime_available(home))
            .unwrap_or(false)
    }
}

/// Whether `kittify_home/missions` exists and holds at least one directory.
pub fn global_runtime_available(kittify_home: &Path) -> bool {
    let missions = kittify_home.join("missions");
    match std::fs::read_dir(&missions) {
        Ok(entries) => entries.flatten().any(|entry| {
            entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false)
        }),
        Err(_) => false,
    }
}

/// Options for a template-tree install.
#[derive(Clone, Debug)]
pub struct TemplateTreeOptions {
    /// Use the global-runtime minimal mode when the source's global home is populated.
    pub prefer_global_runtime: bool,
    /// Byte limit for the copy.
    pub limit: u64,
    /// Fixed timestamp for backup-directory names (tests); `None` uses the clock.
    pub backup_stamp: Option<String>,
}

impl Default for TemplateTreeOptions {
    fn default() -> Self {
        Self {
            prefer_global_runtime: true,
            limit: MAX_TEMPLATE_TREE_BYTES,
            backup_stamp: None,
        }
    }
}

/// What a template-tree install did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemplateTreeReport {
    /// The mode used.
    pub mode: Option<TemplateMode>,
    /// Project-relative subtrees copied this run.
    pub copied: Vec<String>,
    /// Backup directories created (project-relative).
    pub backups: Vec<String>,
    /// Whether this run created `.kittify/templates` (provenance for cleanup).
    pub templates_created: bool,
    /// Whether `.kittify/AGENTS.md` was written.
    pub agents_md_written: bool,
    /// Whether `.claudeignore` was written.
    pub claudeignore_written: bool,
}

/// Install the template tree into `project_root`.
pub fn install_template_tree(
    project_root: &Path,
    source: &TemplateTreeSource,
    options: &TemplateTreeOptions,
) -> Result<TemplateTreeReport, InstallError> {
    let kittify = project_root.join(".kittify");
    std::fs::create_dir_all(&kittify)
        .map_err(|error| InstallError::io("create directory", &kittify, error))?;
    let mut report = TemplateTreeReport::default();

    let use_global = options.prefer_global_runtime && source.global_runtime_available();
    if use_global {
        let memory = kittify.join("memory");
        std::fs::create_dir_all(&memory)
            .map_err(|error| InstallError::io("create directory", &memory, error))?;
        report.mode = Some(TemplateMode::GlobalRuntime);
        return Ok(report);
    }
    report.mode = Some(TemplateMode::FullCopy);

    let stamp = options
        .backup_stamp
        .clone()
        .unwrap_or_else(compact_utc_stamp);
    for (name, root) in [
        ("memory", source.memory_root.as_ref()),
        ("templates", source.templates_root.as_ref()),
        ("missions", source.missions_root.as_ref()),
    ] {
        let Some(root) = root else {
            continue;
        };
        if !root.is_dir() {
            return Err(InstallError::TemplateTreeMissing {
                path: root.clone(),
                detail: format!("{name} source is not a directory"),
            });
        }
        let destination = kittify.join(name);
        if destination.exists() {
            let backup = allocate_backup_dir(&kittify, &stamp)?;
            std::fs::rename(&destination, backup.join(name)).map_err(|error| {
                InstallError::io("back up operator subtree", &destination, error)
            })?;
            report.backups.push(format!(
                ".kittify/{}",
                backup.file_name().unwrap().to_string_lossy()
            ));
        }
        copy_tree_bounded(root, &destination, options.limit)?;
        report.copied.push(format!(".kittify/{name}"));
        if name == "templates" {
            report.templates_created = true;
        }
    }

    if let Some(agents_md) = &source.agents_md
        && agents_md.is_file()
    {
        let destination = kittify.join("AGENTS.md");
        if !destination.exists() {
            copy_file_bounded(agents_md, &destination, options.limit)?;
            report.agents_md_written = true;
        }
    }
    if let Some(template) = &source.claudeignore_template
        && template.is_file()
    {
        let destination = project_root.join(".claudeignore");
        if !destination.exists() {
            copy_file_bounded(template, &destination, options.limit)?;
            report.claudeignore_written = true;
        }
    }
    Ok(report)
}

fn copy_file_bounded(source: &Path, destination: &Path, limit: u64) -> Result<(), InstallError> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| InstallError::io("inspect", source, error))?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::TemplatePathUnsafe {
            path: source.to_path_buf(),
            detail: "template is not a regular file".to_string(),
        });
    }
    if metadata.len() > limit {
        return Err(InstallError::ArchiveTooLarge {
            limit,
            detail: format!("template {} is {} bytes", source.display(), metadata.len()),
        });
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    std::fs::copy(source, destination)
        .map_err(|error| InstallError::io("copy", destination, error))?;
    Ok(())
}

fn allocate_backup_dir(kittify: &Path, stamp: &str) -> Result<PathBuf, InstallError> {
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!(".backup-{stamp}")
        } else {
            format!(".backup-{stamp}-{suffix}")
        };
        let candidate = kittify.join(name);
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(InstallError::io(
                    "create backup directory",
                    &candidate,
                    source,
                ));
            }
        }
    }
    Err(InstallError::TemplateTreeUnverified {
        path: kittify.to_path_buf(),
        detail: "could not allocate a unique backup directory".to_string(),
    })
}

fn compact_utc_stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// The ownership verdict for a destructive removal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemovalVerdict {
    /// The path is proven run-created and safe to remove.
    Owned,
    /// Nothing exists at the path.
    Absent,
    /// The path is not proven run-created; keep it and report the reason.
    Unproven { diagnostic: String },
}

/// Prove whether `path` may be removed.
///
/// Ownership is never inferred from a name. A path is owned only when the
/// caller recorded that *this run* created it and the tree holds no `.git`
/// entry. A legacy `.kittify/command-templates` or `.kittify/.scratch` is
/// therefore preserved, and a populated operator `.kittify/templates` is never
/// mistaken for this run's copy.
pub fn guard_destructive_removal(
    path: &Path,
    project_root: &Path,
    run_created: &BTreeSet<PathBuf>,
) -> RemovalVerdict {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => RemovalVerdict::Absent,
        Err(_) => RemovalVerdict::Unproven {
            diagnostic: format!("Could not inspect {}; kept it", path.display()),
        },
        Ok(_) => {
            let relative = path.strip_prefix(project_root).unwrap_or(path);
            if !run_created.contains(relative) && !run_created.contains(path) {
                return RemovalVerdict::Unproven {
                    diagnostic: format!(
                        "Kept {}: it was not created by this run, so removal is not proven safe",
                        path.display()
                    ),
                };
            }
            if path.join(".git").exists() {
                return RemovalVerdict::Unproven {
                    diagnostic: format!(
                        "Kept {}: it contains a .git entry and may hold the only copy of someone's work",
                        path.display()
                    ),
                };
            }
            RemovalVerdict::Owned
        }
    }
}

/// Remove proven-run-created init scratch paths, returning one diagnostic per
/// preserved path (empty when everything was owned or absent).
pub fn cleanup_init_scratch(
    project_root: &Path,
    run_created: &BTreeSet<PathBuf>,
) -> Result<Vec<String>, InstallError> {
    let mut diagnostics = Vec::new();
    let kittify = project_root.join(".kittify");
    let mut candidates = vec![
        kittify.join("templates"),
        kittify.join("command-templates"),
        kittify.join(".scratch"),
    ];
    if let Ok(entries) = std::fs::read_dir(&kittify) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".resolved-") || name.starts_with(".merged-") {
                candidates.push(entry.path());
            }
        }
    }
    for candidate in candidates {
        match guard_destructive_removal(&candidate, project_root, run_created) {
            RemovalVerdict::Owned => remove_tree_bounded(&candidate)?,
            RemovalVerdict::Absent => {}
            RemovalVerdict::Unproven { diagnostic } => diagnostics.push(diagnostic),
        }
    }
    Ok(diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn source(root: &Path) -> TemplateTreeSource {
        write(&root.join("memory/MEMORY.md"), "memory\n");
        write(&root.join("templates/AGENTS.md"), "# agents\n");
        write(&root.join("templates/claudeignore-template"), "ignore\n");
        write(&root.join("templates/sets/one.md"), "set\n");
        write(
            &root.join("missions/software-dev/mission.yaml"),
            "mission\n",
        );
        TemplateTreeSource::new(
            Some(root.join("memory")),
            Some(root.join("templates")),
            Some(root.join("missions")),
        )
    }

    #[test]
    fn full_copy_installs_every_tree_and_agents_files() {
        let directory = tempfile::tempdir().expect("temp");
        let source_root = tempfile::tempdir().expect("temp");
        let source = source(source_root.path());
        let report =
            install_template_tree(directory.path(), &source, &TemplateTreeOptions::default())
                .expect("install");
        assert_eq!(report.mode, Some(TemplateMode::FullCopy));
        assert!(report.templates_created);
        assert!(report.agents_md_written);
        assert!(report.claudeignore_written);
        assert!(directory.path().join(".kittify/memory/MEMORY.md").is_file());
        assert!(
            directory
                .path()
                .join(".kittify/templates/sets/one.md")
                .is_file()
        );
        assert!(
            directory
                .path()
                .join(".kittify/missions/software-dev/mission.yaml")
                .is_file()
        );
        assert!(directory.path().join(".kittify/AGENTS.md").is_file());
        assert!(directory.path().join(".claudeignore").is_file());
    }

    #[test]
    fn existing_operator_subtrees_are_backed_up_not_deleted() {
        let directory = tempfile::tempdir().expect("temp");
        let source_root = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(".kittify/missions/custom/mine.yaml"),
            "mine\n",
        );
        let source = source(source_root.path());
        let options = TemplateTreeOptions {
            backup_stamp: Some("20260101T000000Z".to_string()),
            ..TemplateTreeOptions::default()
        };
        let report = install_template_tree(directory.path(), &source, &options).expect("install");
        assert_eq!(
            report.backups,
            vec![".kittify/.backup-20260101T000000Z".to_string()]
        );
        assert!(
            directory
                .path()
                .join(".kittify/.backup-20260101T000000Z/missions/custom/mine.yaml")
                .is_file()
        );
        assert!(
            directory
                .path()
                .join(".kittify/missions/software-dev/mission.yaml")
                .is_file()
        );
    }

    #[test]
    fn global_runtime_mode_is_minimal() {
        let directory = tempfile::tempdir().expect("temp");
        let source_root = tempfile::tempdir().expect("temp");
        let home = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(home.path().join("missions/software-dev")).unwrap();
        let source = source(source_root.path()).with_global_home(home.path());
        let report =
            install_template_tree(directory.path(), &source, &TemplateTreeOptions::default())
                .expect("install");
        assert_eq!(report.mode, Some(TemplateMode::GlobalRuntime));
        assert!(!report.templates_created);
        assert!(directory.path().join(".kittify/memory").is_dir());
        assert!(!directory.path().join(".kittify/templates").exists());
        assert!(!directory.path().join(".kittify/missions").exists());
    }

    #[test]
    fn scratch_cleanup_never_removes_unproven_paths() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(".kittify/templates/x.md"),
            "operator\n",
        );
        write(
            &directory.path().join(".kittify/command-templates/old.md"),
            "legacy\n",
        );
        let diagnostics =
            cleanup_init_scratch(directory.path(), &BTreeSet::new()).expect("cleanup");
        assert_eq!(diagnostics.len(), 2);
        assert!(directory.path().join(".kittify/templates/x.md").is_file());
        assert!(
            directory
                .path()
                .join(".kittify/command-templates/old.md")
                .is_file()
        );

        let run_created: BTreeSet<PathBuf> =
            [PathBuf::from(".kittify/templates")].into_iter().collect();
        let diagnostics = cleanup_init_scratch(directory.path(), &run_created).expect("cleanup");
        assert_eq!(diagnostics.len(), 1);
        assert!(!directory.path().join(".kittify/templates").exists());
        assert!(
            directory
                .path()
                .join(".kittify/command-templates/old.md")
                .is_file()
        );
    }

    #[test]
    fn a_tree_with_git_is_never_removed() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(".kittify/templates/.git/config"),
            "x\n",
        );
        let run_created: BTreeSet<PathBuf> =
            [PathBuf::from(".kittify/templates")].into_iter().collect();
        assert!(matches!(
            guard_destructive_removal(
                &directory.path().join(".kittify/templates"),
                directory.path(),
                &run_created
            ),
            RemovalVerdict::Unproven { .. }
        ));
    }
}

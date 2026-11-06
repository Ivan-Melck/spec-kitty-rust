//! Canonical, contained planning-path layout for one Mission.
//!
//! Every path below is composed from the project root, the `kitty-specs/`
//! directory, and a validated single-segment Mission slug. Existing paths are
//! resolved with [`sk_kernel::resolve_within`], so a symlinked Mission
//! directory that escapes the project root is refused rather than followed.
//! Nothing in this module writes, creates, or allocates anything.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use sk_kernel::{MissionSlug, resolve_within};
use thiserror::Error;

use crate::meta::{META_FILE_NAME, SPECS_DIR_NAME};
use crate::work_package::DEFAULT_WORK_PACKAGE_BYTE_LIMIT;

/// Canonical Markdown artifacts authored inside a Mission directory.
pub const SPEC_FILE_NAME: &str = "spec.md";
/// Canonical implementation-plan artifact.
pub const PLAN_FILE_NAME: &str = "plan.md";
/// Canonical task-outline artifact.
pub const TASKS_FILE_NAME: &str = "tasks.md";
/// Directory holding one prompt file per work package.
pub const TASKS_DIR_NAME: &str = "tasks";
/// Canonical tasks-directory README.
pub const TASKS_README_NAME: &str = "README.md";
/// Canonical empty-status placeholder written at non-coordination birth.
pub const STATUS_EVENTS_FILE_NAME: &str = "status.events.jsonl";
/// Scratch directory scaffolds.
pub const CHECKLISTS_DIR_NAME: &str = "checklists";
/// Research scratch directory scaffolds.
pub const RESEARCH_DIR_NAME: &str = "research";

/// Why a Mission path request cannot be served safely.
#[derive(Debug, Error)]
pub enum MissionPathError {
    /// The project root does not exist or is not a directory.
    #[error("project root {root} does not exist or is not a directory")]
    RootNotDirectory { root: PathBuf },

    /// The Mission directory name is not a safe single path segment.
    #[error("invalid Mission directory name {value:?}: {source}")]
    InvalidMissionSlug {
        value: String,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// Containment resolution refused the path.
    #[error("contained path resolution failed for {path}: {source}")]
    Containment {
        path: PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// A planning entry has a non-UTF-8 filename.
    #[error("planning entry {path} has a non-UTF-8 filename")]
    NonUtf8FileName { path: PathBuf },
}

/// The canonical paths of one Mission directory.
///
/// The Mission directory itself may or may not exist; this is a layout value,
/// not proof of creation. `spec_dir` is the resolved `kitty-specs/<slug>` path
/// when the directory exists, and the lexical join otherwise.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissionPaths {
    root: PathBuf,
    slug: MissionSlug,
    spec_dir: PathBuf,
}

impl MissionPaths {
    /// Compose the canonical layout for `root` + `slug`.
    pub fn new(root: &Path, slug: MissionSlug) -> Result<Self, MissionPathError> {
        let metadata = std::fs::metadata(root).map_err(|_| MissionPathError::RootNotDirectory {
            root: root.to_path_buf(),
        })?;
        if !metadata.is_dir() {
            return Err(MissionPathError::RootNotDirectory {
                root: root.to_path_buf(),
            });
        }

        let relative = Path::new(SPECS_DIR_NAME).join(slug.as_str());
        let spec_dir = match resolve_within(root, &relative) {
            Ok(path) => path,
            Err(source) => {
                // An absent target is not a containment failure: resolve_within
                // canonicalizes the existing parent and appends the leaf. Only a
                // genuine escape (symlink/traversal) reaches this arm.
                if source_is_missing(&source) {
                    root.join(relative)
                } else {
                    return Err(MissionPathError::Containment {
                        path: relative,
                        source,
                    });
                }
            }
        };

        Ok(Self {
            root: root.to_path_buf(),
            slug,
            spec_dir,
        })
    }

    /// Parse `slug` and compose the layout.
    pub fn parse(root: &Path, slug: &str) -> Result<Self, MissionPathError> {
        let slug =
            MissionSlug::from_str(slug).map_err(|source| MissionPathError::InvalidMissionSlug {
                value: slug.to_owned(),
                source,
            })?;
        Self::new(root, slug)
    }

    /// Project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `kitty-specs/` root.
    pub fn specs_dir(&self) -> PathBuf {
        self.root.join(SPECS_DIR_NAME)
    }

    /// Mission directory.
    pub fn spec_dir(&self) -> &Path {
        &self.spec_dir
    }

    /// The validated Mission slug (directory name).
    pub fn slug(&self) -> &MissionSlug {
        &self.slug
    }

    /// `meta.json`.
    pub fn meta_file(&self) -> PathBuf {
        self.spec_dir.join(META_FILE_NAME)
    }

    /// `spec.md`.
    pub fn spec_file(&self) -> PathBuf {
        self.spec_dir.join(SPEC_FILE_NAME)
    }

    /// `plan.md`.
    pub fn plan_file(&self) -> PathBuf {
        self.spec_dir.join(PLAN_FILE_NAME)
    }

    /// `tasks.md`.
    pub fn tasks_file(&self) -> PathBuf {
        self.spec_dir.join(TASKS_FILE_NAME)
    }

    /// `tasks/`.
    pub fn tasks_dir(&self) -> PathBuf {
        self.spec_dir.join(TASKS_DIR_NAME)
    }

    /// `tasks/README.md`.
    pub fn tasks_readme(&self) -> PathBuf {
        self.tasks_dir().join(TASKS_README_NAME)
    }

    /// `status.events.jsonl`.
    pub fn status_events_file(&self) -> PathBuf {
        self.spec_dir.join(STATUS_EVENTS_FILE_NAME)
    }

    /// The default byte budget for one work-package prompt.
    pub fn work_package_byte_limit() -> u64 {
        DEFAULT_WORK_PACKAGE_BYTE_LIMIT
    }
}

fn source_is_missing(error: &sk_kernel::KernelError) -> bool {
    matches!(error, sk_kernel::KernelError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

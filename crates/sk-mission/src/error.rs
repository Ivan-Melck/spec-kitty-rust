use std::io;
use std::path::PathBuf;

use sk_kernel::KernelError;
use thiserror::Error;

use crate::identity::{MissionId, MissionIdError};
use sk_kernel::WpId;

/// Typed failures returned by Mission inventory, work-package, graph, and
/// readiness operations.
///
/// Every fallible public operation in this crate reports a variant of this
/// enum with the path or identifier that caused the refusal. Corrupt or
/// ambiguous input is refused, never repaired or guessed.
#[derive(Debug, Error)]
pub enum MissionError {
    /// A supplied project root is missing or is not a directory.
    #[error("mission root {root} does not exist or is not a directory")]
    RootNotDirectory { root: PathBuf },

    /// `kitty-specs` exists but is not a directory.
    #[error("{path} exists but is not a directory")]
    SpecsDirectoryNotDirectory { path: PathBuf },

    /// A `tasks` path exists but is not a directory.
    #[error("{path} exists but is not a directory")]
    TasksDirectoryNotDirectory { path: PathBuf },

    /// A filesystem operation failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A kernel primitive rejected a path or read.
    #[error("kernel rejected {path}: {source}")]
    Kernel {
        path: PathBuf,
        #[source]
        source: KernelError,
    },

    /// A mission-relevant entry is a symlink. The inventory deliberately does
    /// not follow links out of (or inside) `kitty-specs`.
    #[error("{path} is a symlink; read-only Mission inspection refuses to follow it")]
    SymlinkedEntry { path: PathBuf },

    /// `meta.json` exceeded the caller's byte budget.
    #[error("mission metadata {path} is larger than the {limit}-byte limit")]
    MetadataTooLarge { path: PathBuf, limit: u64 },

    /// `meta.json` bytes are not UTF-8-encoded JSON.
    #[error("mission metadata {path} is malformed: {detail}")]
    MalformedMetadata { path: PathBuf, detail: String },

    /// `meta.json` decoded to something other than a JSON object.
    #[error("mission metadata {path} must be a JSON object, not {kind}")]
    MetadataNotObject { path: PathBuf, kind: &'static str },

    /// `meta.json` is not a regular file.
    #[error("mission metadata {path} is not a regular file")]
    MetadataNotFile { path: PathBuf },

    /// A mission directory carries no `mission_id`.
    #[error("mission directory {path} has no mission_id in meta.json")]
    MissingMissionId { path: PathBuf },

    /// `mission_id` is not a canonical 26-character ULID.
    #[error("mission {path} records invalid mission_id {value:?}: {source}")]
    InvalidMissionId {
        path: PathBuf,
        value: String,
        #[source]
        source: MissionIdError,
    },

    /// `mission_id` is present but is not a string at all.
    #[error("mission {path} records mission_id as {kind}; a ULID string is required")]
    InvalidMissionIdType { path: PathBuf, kind: &'static str },

    /// `mission_number` cannot be coerced to a display number.
    #[error("mission {path} records unusable mission_number {value}: {detail}")]
    InvalidMissionNumber {
        path: PathBuf,
        value: String,
        detail: String,
    },

    /// A `kitty-specs` child that claims to be a Mission has an unsafe or
    /// non-canonical directory name.
    #[error("mission directory {path} has an invalid slug: {source}")]
    InvalidMissionDirectoryName {
        path: PathBuf,
        #[source]
        source: KernelError,
    },

    /// A planning entry has a non-UTF-8 name and cannot be safely indexed.
    #[error("planning entry {path} has a non-UTF-8 filename")]
    NonUtf8FileName { path: PathBuf },

    /// Two mission directories declare the same canonical identity.
    #[error("duplicate mission_id {id} is declared by both {first} and {second}")]
    DuplicateMissionId {
        id: MissionId,
        first: PathBuf,
        second: PathBuf,
    },

    /// A selector is not a full mission id, an 8-character mid8, or an exact
    /// mission slug.
    #[error(
        "selector {selector:?} is not a full 26-character mission id, an 8-character mid8, or an exact mission slug"
    )]
    InvalidSelector { selector: String },

    /// No Mission matched a syntactically valid selector.
    #[error("no mission matches selector {selector:?}")]
    MissionNotFound { selector: String },

    /// More than one Mission matched a selector; the caller must disambiguate.
    #[error("selector {selector:?} is ambiguous: it matches {count} missions")]
    AmbiguousMission { selector: String, count: usize },

    /// A work-package Markdown file exceeded the byte budget.
    #[error("work package file {path} is larger than the {limit}-byte limit")]
    WorkPackageTooLarge { path: PathBuf, limit: u64 },

    /// A work-package file does not begin with a frontmatter block.
    #[error("work package file {path} does not begin with a YAML frontmatter block")]
    MissingFrontmatter { path: PathBuf },

    /// A work-package frontmatter block is never closed.
    #[error("work package file {path} has an unterminated frontmatter block")]
    UnterminatedFrontmatter { path: PathBuf },

    /// Frontmatter YAML could not be decoded into a mapping.
    #[error("work package file {path} has invalid frontmatter: {detail}")]
    InvalidFrontmatter { path: PathBuf, detail: String },

    /// Frontmatter is missing the canonical `work_package_id`.
    #[error("work package file {path} has no work_package_id in its frontmatter")]
    MissingWorkPackageId { path: PathBuf },

    /// Frontmatter `work_package_id` is not a canonical `WP<digits>` id.
    #[error("work package file {path} declares invalid work_package_id {value:?}: {source}")]
    InvalidWorkPackageId {
        path: PathBuf,
        value: String,
        #[source]
        source: KernelError,
    },

    /// Frontmatter `dependencies` has an unsupported shape or entry.
    #[error("work package file {path} declares unusable dependencies: {detail}")]
    InvalidDependencies { path: PathBuf, detail: String },

    /// The filename id and the frontmatter id disagree.
    #[error(
        "work package file {path} has filename id {filename:?} but frontmatter work_package_id {frontmatter:?}"
    )]
    WorkPackageIdMismatch {
        path: PathBuf,
        filename: String,
        frontmatter: String,
    },

    /// A work package lists itself as a dependency.
    #[error("work package {wp} depends on itself")]
    SelfDependency { wp: WpId },

    /// A work package depends on an id that is not in the graph.
    #[error("work package {wp} depends on unknown work package {dependency}")]
    UnknownDependency { wp: WpId, dependency: WpId },

    /// Two parsed work packages share an id.
    #[error("work package {wp} is declared more than once")]
    DuplicateWorkPackage { wp: WpId },

    /// The declared dependencies contain a cycle.
    #[error("dependency cycle detected: {cycle}")]
    DependencyCycle { cycle: String },

    /// A readiness query named a work package absent from the graph.
    #[error("work package {wp} is not part of the graph")]
    UnknownWorkPackage { wp: WpId },

    /// A caller-supplied lane name is not a recognised work-package state.
    #[error("unknown work package state {value:?}")]
    UnknownWorkPackageState { value: String },
}

impl MissionError {
    /// Build an [`MissionError::Io`] with the failing action and path.
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }

    /// Wrap a kernel failure with the path that triggered it.
    pub(crate) fn kernel(path: impl Into<PathBuf>, source: KernelError) -> Self {
        Self::Kernel {
            path: path.into(),
            source,
        }
    }
}

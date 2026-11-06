//! Typed failures for initialization, managed-asset install and pack acquisition.
//!
//! Every variant carries the identity of what failed (path, agent, member,
//! digest) so a caller can branch without parsing a message, and no operation
//! in this crate falls through to a silent default. `code()` is the stable
//! machine-readable spelling used by the CLI and by tests.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// A fail-closed install/init/acquisition error.
#[derive(Debug, Error)]
pub enum InstallError {
    // ---------------------------------------------------------------- agents
    /// An agent key is not part of the configured agent surface registry.
    #[error("unknown agent {key:?}; configured agents: {valid}")]
    UnknownAgent { key: String, valid: String },

    /// No agents were selected for an operation that needs at least one.
    #[error("no agents selected; initialization needs at least one configured agent")]
    NoAgentsSelected,

    /// An agent has no slash-command surface but one was requested.
    #[error("agent {key:?} has no slash-command surface (wrapper-only agent)")]
    AgentHasNoCommandSurface { key: String },

    /// An agent has no installable skill root but one was requested.
    #[error("agent {key:?} has no installable skill root (wrapper-only agent)")]
    AgentHasNoSkillRoot { key: String },

    // -------------------------------------------------------------- manifest
    /// The managed-asset manifest exists but is not valid JSON.
    #[error("managed manifest {path} is corrupt: {detail}")]
    ManifestCorrupt { path: PathBuf, detail: String },

    /// The managed-asset manifest declares an unsupported schema version.
    #[error("managed manifest {path} has unsupported schema_version {found}")]
    ManifestUnsupportedVersion { path: PathBuf, found: String },

    /// Two manifest entries claim the same installed path.
    #[error("managed manifest {path} records duplicate path {entry:?}")]
    ManifestDuplicatePath { path: PathBuf, entry: String },

    /// A manifest entry is malformed.
    #[error("managed manifest {path} has an invalid entry: {detail}")]
    ManifestInvalidEntry { path: PathBuf, detail: String },

    /// The manifest file itself is not a regular file.
    #[error("managed manifest {path} is not a regular file")]
    ManifestNotRegularFile { path: PathBuf },

    /// `.kittify/config.yaml` exists but cannot be read as a valid configuration.
    #[error("invalid project config {path}: {detail}")]
    ConfigInvalid { path: PathBuf, detail: String },

    /// Two rendered agents produced different bytes for one shared destination.
    #[error("agents rendered different bytes for shared path {path}")]
    SharedContentConflict { path: String },

    /// A `.gitignore`-style ignore file cannot be read or written safely.
    #[error("unsafe ignore file {path}: {detail}")]
    IgnoreFileUnsafe { path: PathBuf, detail: String },

    /// The Spec Kitty agent-files block in `.gitignore` is damaged.
    #[error("{detail}")]
    IgnoreBlockDamaged { detail: String },

    /// A pending command-delivery recovery record is unsafe or unrecognized.
    #[error("unsafe pending command-delivery record {path}: {detail}")]
    PendingDeliveryInvalid { path: PathBuf, detail: String },

    // ---------------------------------------------------------------- render
    /// A command template could not be located for a prompt-backed command.
    #[error("no command template for {command:?} under {root}")]
    TemplateNotFound { command: String, root: PathBuf },

    /// The configured template source has no prompt for the command.
    #[error("command template source is unavailable: {detail}")]
    TemplateSourceUnavailable { detail: String },

    /// A template has no `## User Input` section, so a skill body cannot be produced.
    #[error("template {path} has no '## User Input' section")]
    MissingUserInputBlock { path: PathBuf },

    /// A literal `$ARGUMENTS` token survived the User Input rewrite.
    #[error("rendered skill {path} still contains $ARGUMENTS on line {line}")]
    StrayArgumentsToken { path: PathBuf, line: usize },

    /// A template's SPDD/REASONS block markers are unbalanced or nested.
    #[error("malformed SPDD/REASONS block: {detail}")]
    SpddBlockMalformed { detail: String },

    /// The canonical project SPDD selector failed; preserve its original cause.
    #[error(transparent)]
    SpddActivation(#[from] Box<sk_doctrine::SpddActivationError>),

    /// A skill name escaped the required Agent Skills grammar.
    #[error("invalid skill name {name:?}")]
    InvalidSkillName { name: String },

    // ------------------------------------------------------------ plan/apply
    /// A managed path is absolute or contains a `..` component.
    #[error("unsafe managed path {path:?}")]
    UnsafePath { path: String },

    /// A managed path resolves outside the project root.
    #[error("managed path {path} resolves outside project root {root}")]
    PathEscape { path: PathBuf, root: PathBuf },

    /// A parent of a managed path exists but is not a directory.
    #[error("managed path {path} has a non-directory parent {parent}")]
    NonDirectoryParent { path: PathBuf, parent: PathBuf },

    /// Two planned assets target the same destination.
    #[error("plan targets {path} more than once")]
    DuplicateDestination { path: String },

    /// An existing node is not a regular file/directory and cannot be managed.
    #[error("unsupported node kind at {path}: {detail}")]
    UnsupportedNode { path: PathBuf, detail: String },

    /// A file the plan depends on changed between planning and applying.
    #[error("precondition changed for {path}")]
    PreconditionChanged { path: PathBuf },

    /// The project already contains authored configuration that forbids this operation.
    #[error("project {root} is already initialized and {detail}")]
    AlreadyInitialized { root: PathBuf, detail: String },

    /// The project root is missing or is not a directory.
    #[error("project root {root} does not exist or is not a directory")]
    RootNotDirectory { root: PathBuf },

    /// Writing a planned asset failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The project lock could not be acquired within the wait budget.
    #[error("could not acquire {path} within {waited_ms}ms: {source}")]
    LockUnavailable {
        path: PathBuf,
        waited_ms: u64,
        #[source]
        source: io::Error,
    },

    // ----------------------------------------------------------- pin/pack
    /// A pack pin lockfile is missing, malformed, or violates the pin contract.
    #[error("invalid pack pin {path}: {detail}")]
    PinInvalid { path: PathBuf, detail: String },

    /// A pin cannot be verified at all (unknown digest algorithm, no snapshot digest).
    #[error("pack pin {path} cannot be verified: {detail}")]
    PinUnverifiable { path: PathBuf, detail: String },

    /// The fetched/staged tree does not match the pin's digests.
    #[error(
        "pack tree at {path} does not match the pin (expected clean {expected_clean}, got {actual_clean})"
    )]
    PackDigestMismatch {
        path: PathBuf,
        expected_clean: String,
        actual_clean: String,
    },

    /// The pinned commit could not be resolved by the transport.
    #[error("pinned commit {commit} is not available from {repository}: {detail}")]
    PackCommitUnresolvable {
        repository: String,
        commit: String,
        detail: String,
    },

    /// No network transport is configured/available and the cache holds no verified tree.
    #[error("pack {commit} is unavailable offline: {detail}")]
    PackUnavailable { commit: String, detail: String },

    /// The pinned tree contains a symlink, which is never acceptable.
    #[error("refusing packed tree containing a symlink: {path}")]
    PackSymlink { path: PathBuf },

    /// The pinned tree contains a non-regular, non-directory file.
    #[error("refusing packed tree containing unsupported file type: {path}")]
    PackSpecialFile { path: PathBuf },

    // ------------------------------------------------------- yaml/metadata
    /// A byte-preserving YAML edit could not be proven safe.
    #[error("cannot edit YAML {path}: {detail}")]
    YamlEdit { path: PathBuf, detail: String },

    /// `.kittify/metadata.yaml` is malformed or cannot be merged.
    #[error("invalid project metadata {path}: {detail}")]
    MetadataInvalid { path: PathBuf, detail: String },

    // ------------------------------------------------------------- versions
    /// A PEP 440 version or version specifier could not be parsed.
    #[error("invalid version value {value:?}: {detail}")]
    VersionInvalid { value: String, detail: String },

    /// The installed core version does not satisfy a pin's `requires_core`.
    #[error("pack pin {pin} requires core {requires_core}, but core is {core_version}")]
    RequiresCoreUnsatisfied {
        pin: PathBuf,
        requires_core: String,
        core_version: String,
    },

    // -------------------------------------------------------------- templates
    /// The built-in activation preset is missing, malformed or empty.
    #[error("default activation preset {path} cannot seed mission types: {detail}")]
    ActivationPresetMissing { path: PathBuf, detail: String },

    /// A template-tree source is absent or incomplete.
    #[error("template tree source {path} is unavailable: {detail}")]
    TemplateTreeMissing { path: PathBuf, detail: String },

    /// A template-tree source does not match the verified pack pin.
    #[error("template tree source {path} is not the verified pack: {detail}")]
    TemplateTreeUnverified { path: PathBuf, detail: String },

    /// A template-tree path is unsafe (absolute, traversal, or a symlink).
    #[error("unsafe template tree path {path}: {detail}")]
    TemplatePathUnsafe { path: PathBuf, detail: String },

    // -------------------------------------------------------------------- git
    /// A Git-owned operation needs the Git owner; this crate only plans it.
    #[error("Git operation {operation} requires the Git owner and was not executed")]
    GitOperationDelegated { operation: String },

    /// The project's Git directory shape is unsupported for a file-level edit.
    #[error("unsupported Git directory at {path}: {detail}")]
    GitDirectoryUnsupported { path: PathBuf, detail: String },

    // -------------------------------------------------------------- migration
    /// The embedded migration catalog is malformed.
    #[error("migration catalog is invalid: {detail}")]
    MigrationCatalogInvalid { detail: String },

    /// The migration ledger is malformed or has an unsupported shape.
    #[error("invalid migration ledger {path}: {detail}")]
    LedgerInvalid { path: PathBuf, detail: String },

    /// The per-ledger lock could not be acquired within its bounded wait.
    #[error(
        "migration ledger lock {path} was not acquired within {waited_ms}ms; nothing was appended"
    )]
    LedgerLockUnavailable { path: PathBuf, waited_ms: u64 },

    /// A selected migration has no native implementation, so the upgrade
    /// refuses *before* mutating anything rather than silently dropping it.
    #[error("upgrade cannot apply {count} migration(s) with no native implementation: {ids}")]
    MigrationNotImplemented { count: usize, ids: String },

    /// The project version is newer than this CLI (a downgrade is refused).
    #[error("project version {project} is newer than this CLI {cli}; downgrade is not supported")]
    ProjectVersionNewer { project: String, cli: String },

    /// The requested `--target` version is older than the project.
    #[error("target version {target} is older than project version {project}")]
    TargetVersionOlder { target: String, project: String },

    /// The project's schema version is newer than this CLI understands.
    #[error("project schema version {found} is newer than supported {supported}")]
    SchemaVersionNewer { found: i64, supported: i64 },

    /// An upgrade was refused because a precondition failed.
    #[error("upgrade refused: {detail}")]
    UpgradeRefused { detail: String },

    /// A migration could not read the project state it must evaluate.
    ///
    /// Source `MigrationStateUnreadableError`: `detect()` returning `False` means
    /// "the project is already in the target state", so a migration whose input
    /// cannot be parsed has not proven that. The runner therefore records a
    /// FAILURE (never a skip) and leaves the version stamp untouched, the same
    /// fail-closed contract `GitignorePathError` has. This is a distinct typed
    /// cause, not a string-matched `UpgradeRefused`: the runner's main and
    /// worktree phases treat these two Source classes differently from every
    /// other error, and `detail` retains the original cause family.
    #[error("{detail}")]
    MigrationStateUnreadable {
        /// The file whose state could not be read, when the premise names one.
        path: Option<PathBuf>,
        /// The Source-shaped message; the retained cause is part of it.
        detail: String,
    },

    /// An uncaught Source writer exception, retaining its typed cause and phase.
    #[error(transparent)]
    SourceWrite(#[from] Box<sk_kernel::source_writer::SourceWriteError>),

    /// An agent choice or agent-check payload was invalid.
    #[error("invalid agent upgrade choice {choice:?}: {detail}")]
    AgentChoiceInvalid { choice: String, detail: String },

    // ----------------------------------------------------------------- skills
    /// The managed-skills manifest is malformed.
    #[error("invalid skills manifest {path}: {detail}")]
    SkillsManifestInvalid { path: PathBuf, detail: String },

    /// A skills catalog root is absent or contains no valid skill.
    #[error("skills catalog {path} is unavailable: {detail}")]
    SkillsCatalogMissing { path: PathBuf, detail: String },

    /// The asset set has no charter agent-profile catalog.
    #[error("agent-profile catalog {path} is unavailable: {detail}")]
    AgentProfileCatalogMissing { path: PathBuf, detail: String },

    /// An unowned file occupies a managed skill destination.
    #[error("cannot install skill file {path}: destination is unowned or modified")]
    SkillCollision { path: PathBuf },

    /// A managed skill file is missing or drifted.
    #[error("managed skill file {path} is {state}")]
    SkillDrift { path: PathBuf, state: &'static str },

    /// The project's charter-pack skills cannot be resolved into the catalog.
    ///
    /// Decided in memory before the staging directory is touched, so a refusal
    /// leaves the project tree byte-identical.
    #[error("pack skills cannot be resolved into the catalog: {detail}")]
    PackSkillCatalog { detail: String },

    // ------------------------------------------------------------- identity
    /// The recorded project identity cannot be understood.
    #[error("invalid project identity in {path}: {detail}")]
    IdentityInvalid { path: PathBuf, detail: String },

    /// A project lifecycle event could not be appended safely.
    #[error("cannot append project lifecycle event to {path}: {detail}")]
    LifecycleEventInvalid { path: PathBuf, detail: String },

    // --------------------------------------------------------------- commit
    /// A write may already be visible but its durability is unconfirmed.
    ///
    /// Callers must read back and recover; they must not delete or retry
    /// blindly (a retry can duplicate an observed mutation).
    #[error("{path} may already be committed ({detail}); read back before retrying")]
    IndeterminateCommit { path: PathBuf, detail: String },

    // --------------------------------------------------------------- archive
    /// An archive exceeded the allowed member count.
    #[error("archive exceeds the {limit}-member limit")]
    ArchiveTooManyMembers { limit: usize },

    /// An archive (compressed or extracted) exceeded the allowed byte budget.
    #[error("archive exceeds the {limit}-byte limit ({detail})")]
    ArchiveTooLarge { limit: u64, detail: String },

    /// An archive member would escape the extraction root.
    #[error("refusing archive member outside the extraction root: {member}")]
    ArchiveTraversal { member: String },

    /// An archive member is a symlink or hardlink.
    #[error("refusing archive link member: {member}")]
    ArchiveLink { member: String },

    /// An archive member is a device, FIFO, socket or other special file.
    #[error("refusing archive special-file member: {member}")]
    ArchiveSpecialFile { member: String },

    /// The archive itself could not be parsed.
    #[error("invalid archive: {detail}")]
    ArchiveInvalid { detail: String },

    /// An archive declares a path that is not valid UTF-8/portable.
    #[error("archive member name is not a portable path: {member:?}")]
    ArchiveInvalidPath { member: String },
}

impl InstallError {
    /// The stable machine-readable code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownAgent { .. } => "unknown_agent",
            Self::NoAgentsSelected => "no_agents_selected",
            Self::AgentHasNoCommandSurface { .. } => "agent_has_no_command_surface",
            Self::AgentHasNoSkillRoot { .. } => "agent_has_no_skill_root",
            Self::ManifestCorrupt { .. } => "manifest_corrupt_json",
            Self::ManifestUnsupportedVersion { .. } => "manifest_unsupported_schema_version",
            Self::ManifestDuplicatePath { .. } => "manifest_duplicate_path",
            Self::ManifestInvalidEntry { .. } => "manifest_invalid_entry",
            Self::ManifestNotRegularFile { .. } => "manifest_not_regular_file",
            Self::ConfigInvalid { .. } => "config_invalid",
            Self::SharedContentConflict { .. } => "shared_content_conflict",
            Self::PendingDeliveryInvalid { .. } => "pending_delivery_invalid",
            Self::IgnoreFileUnsafe { .. } => "ignore_file_unsafe",
            Self::IgnoreBlockDamaged { .. } => "gitignore_block_damaged",
            Self::TemplateNotFound { .. } => "template_not_found",
            Self::TemplateSourceUnavailable { .. } => "template_source_unavailable",
            Self::MissingUserInputBlock { .. } => "user_input_block_missing",
            Self::StrayArgumentsToken { .. } => "stray_arguments_token",
            Self::InvalidSkillName { .. } => "invalid_skill_name",
            Self::SpddBlockMalformed { .. } => "spdd_reasons_block_malformed",
            Self::SpddActivation(_) => "spdd_activation_failed",
            Self::UnsafePath { .. } => "unsafe_path",
            Self::PathEscape { .. } => "unsafe_path",
            Self::NonDirectoryParent { .. } => "non_directory_parent",
            Self::DuplicateDestination { .. } => "duplicate_destination",
            Self::UnsupportedNode { .. } => "unsupported_node",
            Self::PreconditionChanged { .. } => "precondition_changed",
            Self::AlreadyInitialized { .. } => "already_initialized",
            Self::RootNotDirectory { .. } => "root_not_directory",
            Self::Io { .. } => "io_failed",
            Self::LockUnavailable { .. } => "lock_unavailable",
            Self::PinInvalid { .. } => "PACK_PIN_INVALID",
            Self::PinUnverifiable { .. } => "PACK_PIN_INVALID",
            Self::PackDigestMismatch { .. } => "PACK_DIGEST_MISMATCH",
            Self::PackCommitUnresolvable { .. } => "PACK_COMMIT_UNRESOLVABLE",
            Self::PackUnavailable { .. } => "PACK_SYNC_UNAVAILABLE",
            Self::PackSymlink { .. } => "PACK_DIGEST_MISMATCH",
            Self::PackSpecialFile { .. } => "PACK_DIGEST_MISMATCH",
            Self::ArchiveTooManyMembers { .. } => "archive_too_many_members",
            Self::ArchiveTooLarge { .. } => "archive_too_large",
            Self::ArchiveTraversal { .. } => "archive_traversal",
            Self::ArchiveLink { .. } => "archive_link",
            Self::ArchiveSpecialFile { .. } => "archive_special_file",
            Self::ArchiveInvalid { .. } => "archive_invalid",
            Self::ArchiveInvalidPath { .. } => "archive_invalid_path",
            Self::YamlEdit { .. } => "yaml_not_editable",
            Self::MetadataInvalid { .. } => "metadata_invalid",
            Self::VersionInvalid { .. } => "version_invalid",
            Self::RequiresCoreUnsatisfied { .. } => "PACK_REQUIRES_CORE",
            Self::ActivationPresetMissing { .. } => "DEFAULT_PRESET_MISSING",
            Self::TemplateTreeMissing { .. } => "template_tree_missing",
            Self::TemplateTreeUnverified { .. } => "PACK_DIGEST_MISMATCH",
            Self::TemplatePathUnsafe { .. } => "unsafe_path",
            Self::GitOperationDelegated { .. } => "git_operation_delegated",
            Self::GitDirectoryUnsupported { .. } => "git_directory_unsupported",
            Self::MigrationCatalogInvalid { .. } => "migration_catalog_invalid",
            Self::LedgerInvalid { .. } => "migration_ledger_invalid",
            Self::MigrationNotImplemented { .. } => "migration_not_implemented",
            Self::LedgerLockUnavailable { .. } => "migration_ledger_lock_unavailable",
            Self::ProjectVersionNewer { .. } => "project_version_newer",
            Self::TargetVersionOlder { .. } => "target_version_older",
            Self::SchemaVersionNewer { .. } => "schema_version_newer",
            Self::UpgradeRefused { .. } => "upgrade_refused",
            Self::MigrationStateUnreadable { .. } => "migration_state_unreadable",
            Self::SourceWrite(_) => "source_write",
            Self::AgentChoiceInvalid { .. } => "invalid_agent_choice",
            Self::SkillsManifestInvalid { .. } => "skills_manifest_invalid",
            Self::SkillsCatalogMissing { .. } => "skills_catalog_missing",
            Self::AgentProfileCatalogMissing { .. } => "agent_profile_catalog_missing",
            Self::SkillCollision { .. } => "skill_collision",
            Self::SkillDrift { .. } => "skill_drift",
            Self::PackSkillCatalog { .. } => "pack_skill_catalog",
            Self::IdentityInvalid { .. } => "identity_invalid",
            Self::LifecycleEventInvalid { .. } => "lifecycle_event_invalid",
            Self::IndeterminateCommit { .. } => "commit_indeterminate",
        }
    }

    /// Build an I/O error for `path` (the one constructor every surface uses).
    pub fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

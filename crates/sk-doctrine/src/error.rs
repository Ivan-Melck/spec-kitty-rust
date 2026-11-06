//! Typed, fail-closed failures for governance execution.
//!
//! Every fallible operation reports what failed and where. No operation
//! returns a silently empty graph, drops an edge, or rewrites a user document
//! while reporting success.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::graph::{NodeKind, Relation};

/// A fail-closed governance execution error.
#[derive(Debug, Error)]
pub enum DoctrineError {
    /// A URN is not syntactically valid (`<lower_kind>:<body>`).
    #[error("invalid DRG URN {value:?}: expected <lower_case_kind>:<body> with no spaces")]
    InvalidUrn { value: String },

    /// A node's URN prefix disagrees with its declared kind.
    #[error("URN prefix {prefix:?} does not match declared kind {kind:?} ({urn})")]
    UrnKindMismatch {
        urn: String,
        prefix: String,
        kind: NodeKind,
    },

    /// An artifact kind token is outside the taxonomy (delegated to sk-charter).
    #[error(transparent)]
    Charter(#[from] sk_charter::CharterError),

    /// A graph document is missing.
    #[error("graph file not found: {path}")]
    GraphNotFound { path: PathBuf },

    /// A path is neither a readable graph file nor a graph directory.
    #[error("{path} is neither a graph file nor a directory")]
    NotAGraphSource { path: PathBuf },

    /// A directory contains no recognised graph document.
    #[error("no DRG graph files found in directory: {path}")]
    NoGraphFiles { path: PathBuf },

    /// A graph document failed YAML parsing.
    #[error("YAML parse error in {path}: {message}")]
    InvalidYaml { path: PathBuf, message: String },

    /// A graph document failed strict schema validation.
    #[error("{path}: {message}")]
    InvalidGraphDocument { path: PathBuf, message: String },

    /// A document declared a top-level key the schema does not define.
    #[error(
        "{path}: graph document declares unknown top-level key(s) {keys:?}; \
         DRGGraph accepts only schema_version, generated_at, generated_by, nodes, edges"
    )]
    UnknownTopLevelKey { path: PathBuf, keys: Vec<String> },

    /// A node or edge carries an undeclared field.
    #[error("{path}: {location} declares unknown field {field:?}")]
    UnknownField {
        path: PathBuf,
        location: String,
        field: String,
    },

    /// Graph integrity validation failed.
    #[error("DRG integrity validation failed: {0}")]
    InvalidGraph(String),

    /// A bare edge endpoint resolved to more than one kind.
    #[error(
        "ambiguous DRG edge endpoint {raw:?}: it matches built-in {matches:?}; \
         qualify the endpoint with its kind prefix"
    )]
    AmbiguousEndpoint { raw: String, matches: Vec<String> },

    /// A bare edge endpoint resolved to nothing.
    #[error("unresolved DRG edge endpoint {raw:?} in {origin}")]
    UnresolvedEndpoint { raw: String, origin: String },

    /// An edge relation label is not canonical and has no declared alias.
    #[error("unknown DRG relation {relation:?} in {origin}")]
    UnknownRelation { relation: String, origin: String },

    /// An org pack directory is missing.
    #[error("org pack {pack:?} configured at {path} not found")]
    OrgPackMissing { pack: String, path: PathBuf },

    /// An org pack has no `drg/fragment.yaml`.
    #[error("org pack {pack:?} has no DRG fragment at {path}")]
    OrgFragmentMissing { pack: String, path: PathBuf },

    /// An org fragment failed schema validation.
    #[error("org pack {pack:?}: invalid fragment {path}: {message}")]
    InvalidOrgFragment {
        pack: String,
        path: PathBuf,
        message: String,
    },

    /// A pack-supplied body path escapes its pack root.
    #[error("org pack {pack:?}: node {node:?} body_path {body_path:?} escapes {root}")]
    BodyPathEscape {
        pack: String,
        node: String,
        body_path: String,
        root: PathBuf,
    },

    /// An org node reaches across the architectural layer boundary.
    #[error(
        "org node {node:?} in {pack} references the runtime layer ({token:?}); \
         pack doctrine must not reach into src/specify_cli"
    )]
    LayerRuleViolation {
        pack: String,
        node: String,
        token: String,
    },

    /// A same-URN override changed a node's kind.
    #[error(
        "kind-drift override refused for {urn}: built-in kind {built_in:?} \
         cannot be replaced by {attempted:?}"
    )]
    KindDriftOverride {
        urn: String,
        built_in: NodeKind,
        attempted: NodeKind,
    },

    /// Two nodes share a URN that does not admit layered override.
    #[error("duplicate {kind} URN {urn} ({count} declarations across layers)")]
    DuplicateUrn {
        kind: NodeKind,
        urn: String,
        count: usize,
    },

    /// An `enhances` overlay would drop a step from the base action sequence.
    #[error(
        "enhances overlay silently drops {field} step(s) {dropped:?}; \
         declare 'overrides' for a full replacement instead"
    )]
    TopologyStepDropped { field: String, dropped: Vec<String> },

    /// An `enhances` overlay would strip a step's I/O contract.
    #[error("enhances overlay strips {io_field:?} from {field} step {step:?}")]
    TopologyIoStripped {
        field: String,
        step: String,
        io_field: String,
    },

    /// A topology merge was requested with a relation that is not an overlay.
    #[error("topology merge supports enhances/overrides, not {relation:?}")]
    TopologyModeInvalid { relation: Relation },

    /// An activation ledger key is not part of the vocabulary.
    #[error("unknown activation ledger key {key:?} in {path}; allowed: {allowed}")]
    UnknownLedgerKey {
        path: PathBuf,
        key: String,
        allowed: String,
    },

    /// An activation value is not a list of string ids.
    #[error("activation key {key:?} in {path} must be a list of string ids, got {message}")]
    InvalidLedgerValue {
        path: PathBuf,
        key: String,
        message: String,
    },

    /// A legacy activation surface cannot carry a deny write.
    #[error(
        "Refused: activation keys {keys} in {files} have not been moved to \
         .kittify/charter/activation.yaml, and a deactivation (deactivated_<kind>) is recorded only there. \
         Run `spec-kitty upgrade` (or `spec-kitty migrate activation-ledger`), then retry. \
         Nothing was written."
    )]
    UnmigratedDenyWrite { keys: String, files: String },

    /// A ledger write would change a deny-list without an explicit choice.
    #[error(
        "Refused to change {key} without an explicit choice (added: {added:?}, removed: {removed:?}). \
         Only charter activate/deactivate, --include-deactivated, --preset --force and the \
         project-intent rename change a deny-list. Nothing was written."
    )]
    DenyChangeRefused {
        key: String,
        added: Vec<String>,
        removed: Vec<String>,
    },

    /// The requested deny choice cannot authorise this write.
    #[error("deny-list key {key:?} has no deny list (kind is not charter-activatable)")]
    NoDenyList { key: String },

    /// An artifact id is unknown for its kind, before any post-state is computed.
    #[error(
        "Unknown {kind} ID {artifact_id:?}. No artifact with that ID is available for kind '{kind}'. \
         Run `charter list --show-available` to inspect available IDs, \
         or `doctor charter-packs` to verify the doctrine corpus is intact."
    )]
    UnknownActivationId { kind: String, artifact_id: String },

    /// A deactivation is refused by the required floor.
    #[error(
        "cannot deactivate {kind} {artifact_id:?}: required by {requiring} ({policy_key}). Nothing was recorded."
    )]
    RequiredArtifact {
        kind: String,
        artifact_id: String,
        policy_key: String,
        requiring: String,
    },

    /// An activation list value is present but malformed.
    #[error("activation key {key:?} must be a list, got {actual}")]
    MalformedActivationList { key: String, actual: String },

    /// A config or charter YAML document is not a mapping.
    #[error("{path} root must be a mapping")]
    YamlRootNotMapping { path: PathBuf },

    /// A YAML document could not be read or parsed.
    #[error("invalid YAML in {path}: {message}")]
    Yaml { path: PathBuf, message: String },

    /// A YAML target is not a regular file (or is a symlink).
    #[error("unsafe YAML target {path}: expected a regular, non-symlink file")]
    UnsafeYamlTarget { path: PathBuf },

    /// The file changed between prepare and apply.
    #[error("precondition_changed: {path}")]
    PreconditionChanged { path: PathBuf },

    /// A locked file could not be locked within the timeout.
    #[error("timed out after {millis}ms waiting for the lock on {path}")]
    LockTimeout { path: PathBuf, millis: u64 },

    /// A filesystem operation failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A contained resolution refused a pack/user path.
    #[error(transparent)]
    Kernel(#[from] sk_kernel::KernelError),

    /// An agent profile document is malformed.
    #[error("invalid agent profile {path}: {message}")]
    InvalidProfile { path: PathBuf, message: String },

    /// A profile document makes the reference's loader raise an uncaught Python
    /// exception, or a Native capability failure ends the read.
    ///
    /// The reference catches only `(YAMLError, OSError)` per file, so a real Python
    /// exception is a *load* failure, not a skipped file: the command exits nonzero
    /// and prints no profile list. The typed cause keeps the real Source class and
    /// message (or the CF83 Native capability/start-up cause); its rendered view is
    /// `path: class: message`, not the data authority.
    #[error("agent profile load aborted by {path}: {cause}")]
    ProfileLoadAborted {
        path: PathBuf,
        #[source]
        cause: Box<crate::profile::ProfileLoadAbortCause>,
    },

    /// An agent profile does not exist in any layer.
    #[error("agent profile {profile:?} was not found in the built-in, org or project layer")]
    ProfileNotFound { profile: String },

    /// An agent profile's lineage names a parent that does not exist.
    #[error("profile {profile:?} references missing lineage parent {parent:?}")]
    MissingLineageParent { profile: String, parent: String },

    /// A profile lineage chain loops.
    #[error("cycle in specializes_from lineage: {chain:?}")]
    LineageCycle { chain: Vec<String> },

    /// A profile declares more than one lineage parent.
    #[error(
        "profile {profile:?} declares ambiguous lineage parents {parents:?}; \
         a profile may specialize from exactly one parent"
    )]
    AmbiguousLineage {
        profile: String,
        parents: Vec<String>,
    },

    /// An activation identity token is ambiguous within one layer.
    ///
    /// Lookups resolve a shared stem by layer precedence (project, then org
    /// packs last-to-first, then built-in) exactly as the reference index does,
    /// so this is only raised when a caller explicitly asks about one layer.
    #[error(
        "activation token {token:?} for {kind:?} is ambiguous: {candidates:?} \
         declare or answer to it in {layer}"
    )]
    AmbiguousIdentity {
        kind: String,
        token: String,
        layer: PathBuf,
        candidates: Vec<String>,
    },

    /// A declared id is known, but no activation stem can represent it.
    #[error(
        "Cannot store {kind} {artifact_id:?}: its file name {file_name:?} is used by files in more than one layer \
         that declare different IDs ({declarations:?}), so the file name cannot stand for {artifact_id:?}. \
         Rename one of them so each ID has its own file name."
    )]
    ActivationIdCollision {
        kind: String,
        artifact_id: String,
        file_name: String,
        declarations: Vec<String>,
    },

    /// An org-charter policy file is malformed or unreadable.
    #[error("invalid org charter {path}: {message}")]
    InvalidOrgCharter { path: PathBuf, message: String },

    /// A raw org-charter document makes the reference's per-document catch
    /// useless: its `except (OSError, YAMLError, ValueError)` does not cover this
    /// exception, so the whole read aborts instead of skipping or refusing the
    /// pack.
    ///
    /// `exception` is the Python class name (`KeyError` for an invalid `!!bool`
    /// spelling, `TypeError` for an unhashable `!!set` key, `AssertionError` for a
    /// duplicate `!!omap` key) and `message` its message. This is deliberately a
    /// distinct variant from [`DoctrineError::InvalidOrgCharter`]: a caught
    /// failure is a skip (lenient) or a typed refusal (strict), while this one
    /// escapes both postures exactly as the reference's exception does.
    #[error("org charter raw read aborted by {path}: {exception}: {message}")]
    OrgCharterReadAborted {
        path: PathBuf,
        exception: String,
        message: String,
    },

    /// An org-charter `extends:` chain is invalid.
    #[error("org charter {path}: invalid extends chain: {message}")]
    OrgCharterExtends { path: PathBuf, message: String },

    /// An `org-charter.yaml` still carries a field the charter-pack cutover
    /// retired.
    ///
    /// The reference's typed loader raises `RetiredPackFieldError` (code
    /// `RETIRED_PACK_FIELD`) naming the field and its replacement, and its
    /// cross-pack fold **re-raises** that error instead of skipping the pack:
    /// dropping the pack would silently lose its required ids, governance
    /// policies and activations (fail closed, FR-003). This variant is that
    /// refusal, located at the file that carries the field. It is deliberately
    /// distinct from [`DoctrineError::InvalidOrgCharter`], which the fold skips
    /// exactly as the reference skips a schema-invalid pack.
    #[error(
        "org charter {path}: field '{field}' was removed. Replacement: {replacement}. \
         See {runbook}."
    )]
    OrgCharterRetiredField {
        /// The `org-charter.yaml` that still carries the retired field.
        path: PathBuf,
        /// The retired field name (`doctrine_pack_id`).
        field: String,
        /// The field that replaced it (`charter_pack_id`).
        replacement: String,
        /// The migration runbook the reference names.
        runbook: &'static str,
    },

    /// An org pack declares an invalid `skill_namespace`.
    ///
    /// The reference's `read_org_skill_namespace` validates **every** value it
    /// encounters while folding (`require_valid_skill_namespace`) and raises
    /// `SkillPreparationError` for the first invalid one — even when a later
    /// pack would override it — so this is a refusal, not a diagnostic. `pack`
    /// is the pack whose document was being read (the reference names that pack
    /// in its remedy text) and `message` is the canonical grammar violation plus
    /// that remedy.
    #[error("org skill namespace: {message}")]
    OrgSkillNamespaceInvalid { pack: String, message: String },

    /// The charter synthesis lock could not be acquired within the bounded wait.
    #[error(
        "another `spec-kitty` run in this worktree holds the charter synthesis lock {path} \
         (waited {millis}ms). Nothing was written. Retry after the other run finishes."
    )]
    SynthesisLockHeld { path: PathBuf, millis: u64 },

    /// The charter synthesis lock could not be placed.
    #[error(
        "{root}: cannot place the charter synthesis lock because git could not be invoked ({reason}). \
         Make git runnable in this checkout, then retry."
    )]
    SynthesisLockUnavailable { root: PathBuf, reason: String },

    /// A context action token is not a valid action URN or action name.
    #[error("invalid action {action:?}: expected an action name or action:<mission>/<step> URN")]
    InvalidAction { action: String },

    /// The authored charter still carries the retired `governance.doctrine`
    /// selection key.
    ///
    /// The pinned reference refuses this shape with
    /// `ActiveCharterConfigError` / `ACTIVE_CHARTER_CONFIG_INVALID` instead of
    /// silently ignoring the key (`sync.require_canonical_governance`): the
    /// retired block's selections would otherwise be dropped in silence. This
    /// is the native equivalent, raised from the same charter-config read.
    #[error(
        "{path}: `governance.doctrine` is the retired selection key and is no longer read. \
         Run `spec-kitty upgrade` to rewrite it as `governance.charter`. \
         See {runbook}."
    )]
    RetiredGovernanceSelection {
        /// The authored charter file.
        path: PathBuf,
        /// The migration runbook named by the reference refusal.
        runbook: &'static str,
    },

    /// A charter pack root could not be resolved.
    #[error("no pack root could be resolved for {tier}")]
    PackRootUnavailable { tier: &'static str },

    /// A required content file is missing (never invented as a built-in).
    #[error("missing pinned pack content: {path} does not exist")]
    MissingPinnedContent { path: PathBuf },

    /// An exact activation-read failure that has no existing doctrine variant.
    ///
    /// The exact reader's own doctrinal refusals arrive here already unwrapped
    /// (the SAME existing variant, never a nested cause); this carries only the
    /// two remaining typed causes: a checked scalar projection of a Source-valid
    /// token that has no scalar form, and a runtime constructor abort the
    /// reference loader does not catch. The real phase, path, Python class and
    /// message are kept — no fake IO error, no stringified cause and no healthy
    /// fallback.
    #[error("activation read failed: {source}")]
    ActivationRead {
        /// The exact reader's typed cause.
        #[source]
        source: Box<crate::ledger::ActivationReadError>,
    },

    /// A checked identity operation could not produce its Source result.
    ///
    /// Keeps the existing Source/Native failure authority as a standard cause;
    /// a scalar view never substitutes empty text for an exact filename stem.
    #[error("identity operation failed: {source}")]
    IdentityQuery {
        /// The original checked query or Native scalar-view failure.
        #[source]
        source: Box<crate::identity::IdentityFailure>,
    },

    /// A lazy progressive record delivery phase failed.
    ///
    /// The reference reads a delivered artifact's metadata from its model
    /// properties and dumps the model only for an inline body; a setup failure,
    /// a raising layered repository, or a payload the reference's own dump
    /// refuses is not a healthy miss. This is the ONE carrier for those phases
    /// (see [`crate::context::ProgressiveDeliveryError`]).
    #[error("progressive record delivery failed: {source}")]
    ProgressiveDelivery {
        /// The phase and its real cause.
        #[source]
        source: Box<crate::context::ProgressiveDeliveryError>,
    },
}

/// The migration runbook the reference names in its retired-key refusal.
pub const RETIRED_GOVERNANCE_RUNBOOK: &str = "docs/migrations/charter-pack-cutover.md";

impl DoctrineError {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }

    /// The pinned reference's stable machine code for this failure, when the
    /// reference defines one.
    ///
    /// Most doctrine failures are native refusals with no reference code. The
    /// retired `governance.doctrine` selection key is a ported reference
    /// refusal, whose code (`ACTIVE_CHARTER_CONFIG_INVALID`) consumers may
    /// switch on instead of matching the message text.
    pub fn reference_code(&self) -> Option<&'static str> {
        match self {
            Self::RetiredGovernanceSelection { .. } => Some("ACTIVE_CHARTER_CONFIG_INVALID"),
            _ => None,
        }
    }
}

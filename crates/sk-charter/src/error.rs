//! Typed failures for every fallible charter, pack and asset operation.
//!
//! The error type is deliberately exhaustive and carries the identity of what
//! failed (path, tier, kind, asset id) so a CLI never has to parse a message to
//! branch, and no operation falls through to a silent default.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::kind::ArtifactKind;
use crate::tier::PackTier;

/// A fail-closed charter inspection error.
#[derive(Debug, Error)]
pub enum CharterError {
    /// An operator kind token is not part of the artifact-kind taxonomy.
    #[error("unknown artifact kind token {token:?}; valid operator tokens: {valid}")]
    UnknownArtifactKind { token: String, valid: String },

    /// `mission-type` is a charter kind but not an [`ArtifactKind`].
    #[error(
        "'mission-type' is part of the charter kind universe but is not an artifact kind; \
         route it through the mission-tier handler"
    )]
    MissionTypeNotAnArtifactKind,

    /// A plural directory name does not map to an artifact kind.
    #[error("unknown artifact kind plural {plural:?}")]
    UnknownPlural { plural: String },

    /// A tier token is not one of `built-in` / `org` / `project`.
    #[error("unknown pack tier token {token:?}; expected built-in, org or project")]
    UnknownPackTier { token: String },

    /// No root can be resolved for a pack tier.
    #[error("no pack root could be resolved for tier {tier}")]
    PackRootNotFound { tier: PackTier },

    /// A source checkout pins the built-in pack but its tree was never fetched.
    #[error(
        "the built-in pack tree is missing: this checkout pins it in \
         packs/built-in.source.yaml but it was never fetched. Run `make sync-packs` \
         (network required once; later runs use the cache)"
    )]
    BuiltInPackMissing,

    /// The kind has no shipped `packs/built-in/<plural>/` content directory.
    #[error(
        "artifact kind {kind:?} has no packs/built-in/ content directory \
         (package-resource/graph-only kind)"
    )]
    BuiltInContentDirNotAvailable { kind: ArtifactKind },

    /// The project root itself is missing or not a directory.
    #[error("project root {root} does not exist or is not a directory")]
    ProjectRootNotDirectory { root: PathBuf },

    /// A containment root is missing or not a directory.
    #[error("containment root {root} does not exist or is not a directory")]
    RootNotDirectory { root: PathBuf },

    /// An inspection path cannot be represented by the UTF-8 JSON contract.
    #[error("charter inspection path {path} is not UTF-8")]
    NonUtf8Path { path: PathBuf },

    /// A filesystem read failed.
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A bounded read found a file larger than the inspection budget.
    #[error("file {path} is larger than the {limit}-byte inspection limit")]
    TooLarge { path: PathBuf, limit: u64 },

    /// A YAML document could not be parsed.
    #[error("failed to parse YAML at {path}: {source}")]
    InvalidYaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },

    /// A YAML document parsed but its top level is not a mapping.
    #[error("expected a YAML mapping at {path}")]
    ExpectedMapping { path: PathBuf },

    /// `.kittify/config.yaml` is semantically invalid.
    #[error("invalid config at {path}: {reason}")]
    InvalidConfig { path: PathBuf, reason: String },

    /// A path supplied by a pack or user is empty after normalization.
    #[error("path {path:?} is empty")]
    EmptyPath { path: String },

    /// A path supplied by a pack or user is absolute.
    #[error("path {path} is absolute and must be relative")]
    AbsolutePath { path: String },

    /// A path supplied by a pack or user contains a `..` component.
    #[error("path {path} must not contain '..' components")]
    ParentTraversal { path: String },

    /// A path contains a NUL byte.
    #[error("path {path} contains a NUL byte")]
    NulInPath { path: String },

    /// A resolved candidate escaped its containment root (including a symlink).
    #[error("path {path} resolves outside root {root}")]
    PathEscape { path: PathBuf, root: PathBuf },

    /// A `${VAR}` / `$VAR` / `~` token could not be expanded.
    #[error("unresolved path token {token} in {path}")]
    UnresolvedPathToken { path: PathBuf, token: String },

    /// An org pack declared in config has no directory on disk.
    #[error("org pack {name:?} declared at {path} has no pack root at {root}")]
    OrgPackMissing {
        name: String,
        path: PathBuf,
        root: PathBuf,
    },

    /// A pack tier directory has no `pack.yaml` descriptor.
    #[error("pack descriptor pack.yaml is missing at {root} for tier {tier}")]
    PackDescriptorMissing { tier: PackTier, root: PathBuf },

    /// A `pack.yaml` is schema-invalid.
    #[error("invalid pack descriptor at {path}: {reason}")]
    InvalidPackDescriptor { path: PathBuf, reason: String },

    /// A descriptor's declared tier disagrees with its configured position.
    #[error("pack descriptor {path} declares tier {declared} but is loaded as {expected}")]
    PackLevelMismatch {
        path: PathBuf,
        declared: PackTier,
        expected: PackTier,
    },

    /// A `pack_id` is not a canonical ULID.
    #[error("pack descriptor {path} has invalid pack_id {value:?}")]
    InvalidPackId { path: PathBuf, value: String },

    /// Two manifests with the same id compete at the same precedence.
    #[error("asset {id:?} is ambiguous; competing manifests: {sources:?}")]
    AmbiguousAsset { id: String, sources: Vec<PathBuf> },

    /// No asset manifest resolves the requested id.
    #[error("no asset manifest resolves id {id:?}")]
    AssetNotFound { id: String },

    /// An asset blob path escapes its anchoring root.
    #[error("asset {id:?} blob path {blob_path:?} escapes its root {root}")]
    AssetPathEscape {
        id: String,
        blob_path: String,
        root: PathBuf,
    },

    /// An `*.asset.yaml` sidecar is schema-invalid.
    #[error("invalid asset manifest at {path}: {reason}")]
    InvalidAssetManifest { path: PathBuf, reason: String },

    /// More asset manifests were discovered than the budget allows.
    #[error("more than {limit} asset manifests were found")]
    TooManyAssets { limit: usize },

    /// The kernel containment primitive refused a path for a non-path reason.
    #[error("kernel containment refused {path}: {reason}")]
    ContainmentRefused { path: PathBuf, reason: String },

    /// A charter document carries a shape or value this build does not accept
    /// (an unknown artifact kind, a malformed synthesis manifest, an
    /// unhashable bundle member).
    #[error("unsupported charter state in {path}: {detail}")]
    UnsupportedNode { path: PathBuf, detail: String },
}

//! The charter-bundle freshness gate and the retired sync lifecycle
//! (reference `charter.activation.sync`, `charter.hasher.is_stale`,
//! `charter.bundle.CANONICAL_MANIFEST`).
//!
//! The reference keeps two charter-bundle entry points alive after the IC-04
//! retirement of the prose→triad scrape:
//!
//! * `sync()` — the retired report-only sync. It reads `charter.md` through the
//!   ONE encoding-provenance chokepoint, compares the resulting text against
//!   `metadata.yaml::charter_hash`, and reports the staleness verdict. It no
//!   longer extracts prose, so it always answers `synced=False` with an empty
//!   `files_written` and an empty `extraction_mode`.
//! * `ensure_charter_bundle_fresh()` — the freshness gate every charter read
//!   goes through. It resolves the canonical (main-checkout) charter read root,
//!   treats an absent `charter.md` as a healthy "there is nothing to refresh"
//!   (`None`), forces the sync when the manifest's required derived files are
//!   missing, otherwise consults `is_stale`, and calls `sync()` only when the
//!   bundle is incomplete or stale. A complete, fresh bundle returns a
//!   non-synced result and reads **no** provenance record.
//!
//! ## Owners this module reuses instead of copying
//!
//! | Concern | Owner |
//! | --- | --- |
//! | canonical read root | [`sk_kernel::git_topology`] (the one `--git-common-dir` probe) |
//! | BOM/newline-normalising digest | [`sk_charter::hash_content`] |
//! | proven read + provenance append | [`sk_kernel::text_decode::read_text_with_provenance`] → [`sk_kernel::encoding_provenance::EncodingProvenanceJournal`] |
//! | record shape, routing, lock, writer | the same Kernel journal + [`sk_kernel::event_log_append`] |
//! | `Path.exists()` policy | [`sk_kernel::paths::source_path_exists`] |
//! | bundle path constants | [`sk_charter::CHARTER_MD`], [`sk_charter::CHARTER_YAML`] |
//!
//! There is no second decoder, hasher, YAML model, journal, lock or writer here.
//!
//! ## Reference phases reproduced
//!
//! `ensure_charter_bundle_fresh`'s observable order is: resolve the read root
//! (a refusal escapes — the reference's C-001 "raise loudly, never fall back"),
//! `charter.md` existence, required-derived-file inventory, `is_stale`, and only
//! then the sync. The sync's own order is: read through the provenance chokepoint
//! (which appends its record), re-check staleness with that already-read content
//! (no second read, no TOCTOU window), and report. Every failure of that inner
//! sequence is *caught* by the reference's broad `except Exception`, producing an
//! error carrier rather than an exception; the freshness-evaluation failure is
//! likewise downgraded to a warning plus a forced sync.
//!
//! ## Deliberate, disclosed divergences
//!
//! * The reference has a `logging` facility and sends its two `logger.warning`
//!   bodies plus one `logger.info` line to it. Native has no logger, so the two
//!   warning bodies are carried on [`CharterSyncResult::warnings`] (the field the
//!   reference's own result type declares but leaves unused) and the info-level
//!   line is not modelled at all. Nothing reaches stdout.
//! * `CharterSyncResult::error` carries the Native rendering of the failure
//!   (`str(e)` in the reference). A Python `UnicodeDecodeError`/`AttributeError`
//!   body has no exact Rust spelling, and a non-UTF-8 charter that the reference's
//!   charset tie-break would recover refuses here instead — the recorded CF64
//!   charset-ingestion gap, not a new semantic exemption.
//! * The inventory is a same-authority view of sk-charter's canonical bundle
//!   manifest. The retired derived set is empty, not a private parallel list.
//! * Metadata reads select the existing Kernel ruamel byte/graph authority and
//!   project only the requested hash field. Round-trip-only custom tags and the
//!   existing parser/depth/cyclic-value projection limits remain unapproved gaps.

use std::io;
use std::path::{Path, PathBuf};

use sk_kernel::KernelError;
use sk_kernel::PythonString;
use sk_kernel::encoding_provenance::EncodingProvenanceJournal;
use sk_kernel::python_value::PythonValue;
use sk_kernel::python_yaml::{self, Dialect};
use sk_kernel::text_decode::{ProvenanceReadError, read_text_with_provenance};

/// The `.kittify/charter/` relative directory holding the bundle.
pub const CHARTER_DIR_RELATIVE: &str = ".kittify/charter";

/// The staleness metadata file name inside the charter directory.
pub const METADATA_FILENAME: &str = "metadata.yaml";

/// The metadata key holding the digest of the synced `charter.md`.
pub const CHARTER_HASH_KEY: &str = "charter_hash";

/// The pinned reference's required derived-file inventory.
///
/// `charter.bundle.CANONICAL_MANIFEST.derived_files` is empty: the IC-04
/// retirement removed `governance.yaml`/`directives.yaml`/`metadata.yaml`
/// *derivation*, leaving `charter.md` and `charter.yaml` as the only tracked
/// members. The inventory is kept as a named, declared set so the "manifest owns
/// what must exist, and a missing member forces the sync" phase is still a real
/// phase rather than an inlined `false`.
pub const CANONICAL_DERIVED_FILES: &[&str] = sk_charter::CANONICAL_BUNDLE_MANIFEST.derived_files;

/// The reference's `SyncResult`.
///
/// Field meanings follow the reference exactly: `files_written` entries are file
/// *names* relative to `<canonical_root>/.kittify/charter/`, `canonical_root` is
/// the resolved main-checkout root (patched onto the result by
/// [`ensure_charter_bundle_fresh`], never by [`sync_charter_bundle`]), and
/// staleness is the verdict *before* the sync ran.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharterSyncResult {
    /// True only if extraction ran. The extraction is retired, so this is
    /// always `false`.
    pub synced: bool,
    /// The staleness verdict observed before the sync ran.
    pub stale_before: bool,
    /// Derived file names (never absolute paths) the sync wrote. Always empty.
    pub files_written: Vec<String>,
    /// The extraction mode (`"deterministic"`/`"hybrid"` before the retirement).
    /// Always empty.
    pub extraction_mode: String,
    /// The sync failure the reference catches and reports in this field.
    pub error: Option<String>,
    /// The resolved canonical (main-checkout) charter read root.
    pub canonical_root: Option<PathBuf>,
    /// Warning bodies the reference writes to its logger.
    ///
    /// The reference declares this field and leaves it empty because it logs;
    /// Native has no logger, so the same texts land here instead of on a stream.
    pub warnings: Vec<String>,
}

impl CharterSyncResult {
    /// A reporting-only sync outcome (the retired contract's only shape).
    fn report(stale_before: bool) -> Self {
        Self {
            synced: false,
            stale_before,
            files_written: Vec::new(),
            extraction_mode: String::new(),
            error: None,
            canonical_root: None,
            warnings: Vec::new(),
        }
    }

    /// The caught-failure shape: `stale_before` is `false` and no file was written.
    fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::report(false)
        }
    }
}

/// The reference's `(is_stale, current_hash, stored_hash)` triple.
#[derive(Clone, Debug, PartialEq)]
pub struct CharterStaleness {
    /// Whether the authored `charter.md` differs from the recorded digest.
    pub stale: bool,
    /// The digest of the content under test (`sha256:<hex>`).
    pub current_hash: String,
    /// The actual requested YAML value, not non-string-to-None projection.
    /// Source returns an empty string for absent metadata/key or a falsy root.
    /// Cyclic selected-value projection remains a typed Native capability gap.
    pub stored_hash: PythonValue,
}

/// A fail-closed charter-bundle failure the reference raises.
#[derive(Debug, thiserror::Error)]
pub enum CharterSyncError {
    /// The canonical read root could not be resolved.
    ///
    /// The reference raises `NotInsideRepositoryError` (never inside a Git
    /// repository) or `GitCommonDirUnavailableError` (the probe itself failed);
    /// both escape `ensure_charter_bundle_fresh` unchanged (C-001: no fallback),
    /// and the CLI renders them as git-resolution failures.
    #[error("{message}")]
    Root {
        /// The path that could not be resolved.
        path: PathBuf,
        /// Whether the failure is "not a repository" rather than "probe failed".
        not_a_repository: bool,
        /// The underlying cause, rendered.
        message: String,
    },

    /// An existence probe before a read failed.
    #[error("failed to stat {path}: {source}")]
    Stat {
        /// The probed path.
        path: PathBuf,
        /// The canonical IO cause.
        #[source]
        source: io::Error,
    },

    /// The ordinary strict-UTF-8 `charter.md` read used by the staleness check
    /// failed (including an invalid-UTF-8 body, which the reference raises out of
    /// `read_text`).
    #[error("failed to read {path}: {source}")]
    StaleRead {
        /// The charter path.
        path: PathBuf,
        /// The canonical IO cause.
        #[source]
        source: io::Error,
    },

    /// `metadata.yaml` could not be read or is not a mapping.
    #[error("invalid charter metadata {path}: {message}")]
    Metadata {
        /// The metadata path.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },

    /// The reference's `ValueError("Either charter_path or content must be provided")`.
    #[error("Either charter_path or content must be provided")]
    NoInput,

    /// The provenance-chokepoint read refused (unreadable or unprovable bytes,
    /// or a record that could not be minted).
    #[error(transparent)]
    Read(#[from] ProvenanceReadError),

    /// The Kernel Git-topology probe failed.
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

/// Resolve the charter read root: an explicit scope root wins, otherwise the
/// canonical (main-checkout) root.
///
/// This is the reference's `_charter_read_root`: a caller-supplied
/// `CharterScope.root` is honoured verbatim, and every other caller gets
/// [`resolve_canonical_repo_root`]. It never invents a second Git probe.
pub fn charter_read_root(
    repo_root: &Path,
    scope: Option<&Path>,
) -> Result<PathBuf, CharterSyncError> {
    match scope {
        Some(root) => Ok(root.to_path_buf()),
        None => resolve_canonical_repo_root(repo_root),
    }
}

/// Port of `charter.resolution.resolve_canonical_repo_root` onto the one
/// Kernel Git-topology probe.
///
/// * a common dir named `.git` sits inside its working tree → its parent;
/// * otherwise the common dir's configured `core.worktree` (a submodule's own
///   working tree);
/// * with no `core.worktree` (a separate git dir) → the common dir's parent.
///
/// `.git` interiors and bare repositories are refusals, matching
/// [`sk_kernel::git_topology::git_common_dir`].
pub fn resolve_canonical_repo_root(path: &Path) -> Result<PathBuf, CharterSyncError> {
    let common_dir = sk_kernel::git_topology::git_common_dir(path).map_err(|source| {
        let not_a_repository = matches!(source, KernelError::NotAGitRepository { .. });
        CharterSyncError::Root {
            path: path.to_path_buf(),
            not_a_repository,
            message: source.to_string(),
        }
    })?;
    if common_dir.file_name().is_some_and(|name| name == ".git")
        && let Some(parent) = common_dir.parent()
    {
        return Ok(parent.to_path_buf());
    }
    match sk_kernel::git_topology::git_configured_worktree(&common_dir) {
        Ok(Some(worktree)) => Ok(worktree),
        Ok(None) => match common_dir.parent() {
            Some(parent) => Ok(parent.to_path_buf()),
            None => Err(CharterSyncError::Root {
                path: path.to_path_buf(),
                not_a_repository: false,
                message: format!("{} has no parent directory", common_dir.display()),
            }),
        },
        Err(source) => Err(CharterSyncError::Root {
            path: path.to_path_buf(),
            not_a_repository: false,
            message: source.to_string(),
        }),
    }
}

/// The `.kittify/charter/` directory under `root`.
pub fn charter_directory(root: &Path) -> PathBuf {
    root.join(CHARTER_DIR_RELATIVE)
}

/// The authored `charter.md` path under `root`.
pub fn charter_markdown_path(root: &Path) -> PathBuf {
    root.join(sk_charter::CHARTER_MD)
}

/// Whether `path` exists, with the reference's `Path.exists()` policy.
fn exists(path: &Path) -> Result<bool, CharterSyncError> {
    sk_kernel::paths::source_path_exists(path).map_err(|source| CharterSyncError::Stat {
        path: path.to_path_buf(),
        source,
    })
}

/// Port of `charter.hasher.is_stale`.
///
/// `content` (already-read text) wins when supplied, so a caller that just read
/// the file cannot race a second read; otherwise `charter_path` is read with the
/// reference's ordinary strict-UTF-8 `read_text` (`hash_charter`).
///
/// An absent `metadata.yaml` is `(stale = true, current, "")` — the reference's
/// "never synced" signal — never a healthy verdict and never an error.
pub fn charter_bundle_staleness(
    charter_path: Option<&Path>,
    metadata_path: &Path,
    content: Option<&str>,
) -> Result<CharterStaleness, CharterSyncError> {
    let current_hash = match (content, charter_path) {
        (Some(text), _) => sk_charter::hash_content(text),
        (None, Some(path)) => {
            let text =
                std::fs::read_to_string(path).map_err(|source| CharterSyncError::StaleRead {
                    path: path.to_path_buf(),
                    source,
                })?;
            sk_charter::hash_content(&text)
        }
        (None, None) => return Err(CharterSyncError::NoInput),
    };

    if !exists(metadata_path)? {
        return Ok(CharterStaleness {
            stale: true,
            current_hash,
            stored_hash: PythonValue::Str(PythonString::default()),
        });
    }

    let metadata_error = |message: String| CharterSyncError::Metadata {
        path: metadata_path.to_path_buf(),
        message,
    };
    let bytes = sk_kernel::read_source_bytes(metadata_path)
        .map_err(|error| metadata_error(error.to_string()))?;
    let text = python_yaml::read_document_text(&bytes, Dialect::RuamelSafe12)
        .map_err(|error| metadata_error(error.to_string()))?;
    let graph = python_yaml::decode_graph(&text, Dialect::RuamelSafe12)
        .map_err(|error| metadata_error(error.to_string()))?;
    let root = graph.root_view();
    // Source tests truth before `.get`: false/0/empty string/list/dict all use
    // the empty stored hash. Unrelated fields, even cycles, are never projected.
    let stored_hash = if root.is_falsy() {
        PythonValue::Str(PythonString::default())
    } else {
        if root.as_mapping().is_none() {
            return Err(metadata_error(format!(
                "'{}' object has no attribute 'get'",
                root.type_name()
            )));
        }
        match root.lookup(CHARTER_HASH_KEY) {
            Some(value) => value
                .to_python_value()
                .map_err(|error| metadata_error(error.to_string()))?,
            None => PythonValue::Str(PythonString::default()),
        }
    };
    let fresh = stored_hash
        .as_str()
        .is_some_and(|stored| *stored == PythonString::from_scalars(&current_hash));
    Ok(CharterStaleness {
        stale: !fresh,
        current_hash,
        stored_hash,
    })
}

/// Port of `charter.activation.sync.sync` — the retired report-only sync.
///
/// Reads `charter.md` through the ONE encoding-provenance chokepoint (which
/// appends the record), re-checks staleness with that already-read content, and
/// returns `synced = false` with an empty `files_written`/`extraction_mode`.
/// `force` is accepted for signature compatibility and changes nothing: there is
/// no extraction left to force, and no derived artifact is ever written.
///
/// Every failure of the inner sequence is caught into
/// [`CharterSyncResult::error`], exactly like the reference's `except Exception`:
/// a broken charter is a reported sync failure, not a raised one.
pub fn sync_charter_bundle(
    charter_path: &Path,
    output_dir: Option<&Path>,
    journal: &EncodingProvenanceJournal,
    force: bool,
) -> CharterSyncResult {
    let _ = force;
    let output_dir = match output_dir {
        Some(dir) => dir,
        None => match charter_path.parent() {
            Some(parent) => parent,
            None => {
                return CharterSyncResult::failed(format!(
                    "{} has no parent directory",
                    charter_path.display()
                ));
            }
        },
    };
    let metadata_path = output_dir.join(METADATA_FILENAME);
    match sync_report(charter_path, &metadata_path, journal) {
        Ok(result) => result,
        Err(error) => CharterSyncResult::failed(error.to_string()),
    }
}

/// The fallible core of [`sync_charter_bundle`]: read, then re-check staleness.
fn sync_report(
    charter_path: &Path,
    metadata_path: &Path,
    journal: &EncodingProvenanceJournal,
) -> Result<CharterSyncResult, CharterSyncError> {
    let proven = read_text_with_provenance(charter_path, journal)?;
    let text = proven.text().to_scalar_string().map_err(|code| {
        // A proven BOM/UTF-8 decode cannot carry a surrogate; keep this an honest
        // refusal rather than a lossy replacement if that premise ever changes.
        CharterSyncError::Metadata {
            path: charter_path.to_path_buf(),
            message: format!(
                "decoded charter text holds U+{code:04X}, which has no UTF-8 text form"
            ),
        }
    })?;
    let staleness = charter_bundle_staleness(None, metadata_path, Some(&text))?;
    let mut result = CharterSyncResult::report(staleness.stale);
    result.warnings = proven
        .warnings()
        .iter()
        .map(|warning| warning.message().to_owned())
        .collect();
    Ok(result)
}

/// Port of `charter.activation.sync.ensure_charter_bundle_fresh`.
///
/// * Resolves the read root ([`charter_read_root`]); a resolver failure escapes.
/// * `None` when `charter.md` is absent under that root — there is no charter to
///   refresh, and no provenance record is written. An existing *directory* or
///   other non-file object at that path is **not** healthy absence: it reaches the
///   sync and becomes its reported failure, exactly like the reference.
/// * A missing required derived file ([`CANONICAL_DERIVED_FILES`]) or a stale
///   bundle triggers [`sync_charter_bundle`]; a staleness-evaluation failure is a
///   warning plus a forced sync, never a healthy verdict.
/// * A complete, fresh bundle returns an unsynced result with `stale_before =
///   false` and reads no provenance record.
/// * The returned result always carries `canonical_root`.
///
/// `journal` is supplied by the caller: the reference's project-level provenance
/// path is resolved against the **process CWD**, not against this read root, so the
/// base is a caller decision ([`source_cwd_journal`] is the Source placement).
pub fn ensure_charter_bundle_fresh(
    repo_root: &Path,
    scope: Option<&Path>,
    journal: &EncodingProvenanceJournal,
) -> Result<Option<CharterSyncResult>, CharterSyncError> {
    let canonical_root = charter_read_root(repo_root, scope)?;
    let charter_dir = charter_directory(&canonical_root);
    let charter_path = charter_markdown_path(&canonical_root);
    if !exists(&charter_path)? {
        return Ok(None);
    }

    let metadata_path = charter_dir.join(METADATA_FILENAME);
    // The manifest is authoritative for "what files must exist".
    let mut missing_files: Vec<String> = Vec::new();
    for &name in CANONICAL_DERIVED_FILES {
        let path = canonical_root.join(name);
        if !exists(&path)? {
            missing_files.push(name.to_owned());
        }
    }
    let mut should_force = !missing_files.is_empty();
    let mut stale = false;
    let mut warnings: Vec<String> = Vec::new();

    if !should_force {
        match charter_bundle_staleness(Some(&charter_path), &metadata_path, None) {
            Ok(staleness) => stale = staleness.stale,
            Err(error) => {
                warnings.push(format!(
                    "Failed to evaluate charter bundle freshness: {error}"
                ));
                should_force = true;
            }
        }
    }

    if !should_force && !stale {
        let mut result = CharterSyncResult::report(false);
        result.canonical_root = Some(canonical_root);
        return Ok(Some(result));
    }

    let mut result = sync_charter_bundle(&charter_path, Some(&charter_dir), journal, should_force);
    result.canonical_root = Some(canonical_root);
    if let Some(error) = result.error.clone() {
        warnings.push(format!(
            "Charter auto-sync failed while refreshing extracted artifacts: {error}"
        ));
    }
    result.warnings.splice(0..0, warnings);
    Ok(Some(result))
}

/// A journal placed exactly where the reference places it: the **process CWD**.
///
/// `charter.activation._io._route_provenance_path` returns the literal relative
/// path `.kittify/encoding-provenance/global.jsonl`, so the record lands beside
/// whatever directory the operator ran the command in — which need not be the
/// project root, and is never the resolved read root. A caller that passes this
/// journal therefore reproduces the reference placement; a caller that passes
/// [`EncodingProvenanceJournal::new`] with some other base is choosing a
/// deliberate divergence.
pub fn source_cwd_journal() -> EncodingProvenanceJournal {
    EncodingProvenanceJournal::new(".")
}

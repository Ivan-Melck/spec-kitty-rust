//! Canonical persisted merge-state read view — Source `specify_cli.consolidation.state`.
//!
//! # What this module owns
//!
//! Exactly one reader for the persisted per-mission merge record at
//! `.kittify/runtime/merge/<mission_id>/state.json`, plus the enumeration seam the
//! coordination safety net needs (`iter_pending_coord_reconcile_markers`).
//!
//! Source's `consolidation/state.py` owns that path shape; the coordination doctor
//! consumes *this* iterator instead of re-implementing the scan, so there is one
//! runtime-path authority and one state decoder (the `#2786` DIR-044 rule recorded
//! in Source's own docstring). Nothing here creates a directory, takes a lock,
//! writes a byte, or repairs anything: this is a read-only view.
//!
//! # What is deliberately reused, not cloned
//!
//! * [Kernel `python_json`](sk_kernel::python_json) is the ONE decoder for authored
//!   JSON: exact code-point strings (lone surrogates included), Python `dict`
//!   insertion/duplicate-key semantics, and `NaN`/`Infinity` acceptance. There is
//!   no second JSON parser here.
//! * [`sk_kernel::read_source_bytes`] is Source's unbounded
//!   `Path.read_bytes()` policy (no invented size cap on this record).
//! * [`sk_kernel::clock::now_utc_iso`] supplies the reference constructor's clock
//!   default for an absent `started_at` / `updated_at`.
//!
//! # Constructor and failure semantics (Source `from_dict` + `_load_state_file`)
//!
//! Source rehydrates the record through the dataclass constructor:
//!
//! * unknown keys are **dropped** (the known-field filter),
//! * the ref map/list fields are coerced with fail-closed projections,
//! * a missing *required* field raises `TypeError`, which `_load_state_file`
//!   collapses into `ConsolidationStateReadError`,
//! * an absent file is `None` (never an error), while a present-but-undecodable
//!   file (non-UTF-8, malformed JSON, non-object, missing required field) is
//!   `ConsolidationStateReadError` — Source's D5 fail-closed rule, never a
//!   silent "no merge in progress".
//!
//! The one deliberate narrowing: Source's `from_dict` would raise `AttributeError`
//! for a top-level JSON array/scalar (`.items()` on a list), and its enumerator
//! only catches `ConsolidationStateReadError`, so one such file would abort the
//! whole safety-net scan. Here that shape is classified as the same undecodable
//! record the docstring promises to skip. See the lane report's remaining-debt
//! list.
//!
//! # Read-only by construction
//!
//! [`ConsolidationState`] exposes the authored values it decoded; it never invents
//! a mission id, timestamp or ref. There is no `save_state`, no marker clearing and
//! no lock helper in this module — the lifecycle writers stay where they are.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use sk_kernel::clock::now_utc_iso;
use sk_kernel::python_json::{PythonJsonDict, PythonJsonValue, is_falsy, parse, python_repr_value};
use sk_kernel::{PythonString, read_source_bytes};

/// The persisted merge-record filename (`consolidation/workspace.py::STATE_FILENAME`).
pub const STATE_FILENAME: &str = "state.json";

/// The `.kittify` project-metadata directory name.
pub const KITTIFY_DIR_NAME: &str = ".kittify";

/// The runtime-state directory name under `.kittify`.
pub const RUNTIME_DIR_NAME: &str = "runtime";

/// The merge subdirectory name under `.kittify/runtime`.
pub const MERGE_DIR_NAME: &str = "merge";

/// Marker key recording the strand's own status-log commit SHAs (`#5572`).
///
/// Optional on the marker: a marker written before the key existed reads as "no
/// commits recorded" (see [`marker_strand_shas`]).
pub const STRAND_SHAS_KEY: &str = "strand_shas";

/// `unsettled_refs` sentinel meaning "every run-movable branch is unsettled".
pub const UNSETTLED_ALL: &str = "*";

/// The marker's coordination-ref key.
pub const COORD_REF_KEY: &str = "coord_ref";
/// The marker's pre-bake coordination-tip key.
pub const CAPTURED_SHA_KEY: &str = "captured_sha";
/// The marker's coordination-worktree key.
pub const COORD_WORKTREE_KEY: &str = "coord_worktree";
/// The marker's stranded write-set key.
pub const STRANDED_WP_IDS_KEY: &str = "stranded_wp_ids";
/// The marker's failure-text key.
pub const REVERT_ERROR_KEY: &str = "revert_error";
/// The marker's detection-timestamp key.
pub const DETECTED_AT_KEY: &str = "detected_at";

/// Every field Source's `ConsolidationState` dataclass declares, in declaration
/// order (the order its `to_dict()` emits).
pub const KNOWN_FIELDS: [&str; 33] = [
    "mission_id",
    "mission_slug",
    "target_branch",
    "wp_order",
    "completed_wps",
    "current_wp",
    "has_pending_conflicts",
    "strategy",
    "workspace_path",
    "started_at",
    "updated_at",
    "mission_number_baked",
    "push_requested",
    "delete_branch",
    "remove_worktree",
    "pending_coord_reconcile",
    "skip_lanes",
    "dependency_refs",
    "dependency_ref_pins",
    "pre_mutation_target_sha",
    "pre_mutation_coord_sha",
    "pre_mutation_coord_ref",
    "pre_interrupt_lane_tips",
    "reconciliation_passed_target_sha",
    "pre_mutation_refs",
    "post_mutation_refs",
    "restore_targets",
    "snapshot_lane_branches",
    "resume_seeded_refs",
    "unsettled_refs",
    "advance_intents",
    "released_refs",
    "release_reasons",
];

/// The fields without a dataclass default — absent means an undecodable record.
pub const REQUIRED_FIELDS: [&str; 4] = ["mission_id", "mission_slug", "target_branch", "wp_order"];

/// `ref -> SHA` / `branch -> SHA` map fields, fail-closed to `{}` when malformed.
const REF_MAP_FIELDS: [&str; 6] = [
    "pre_mutation_refs",
    "post_mutation_refs",
    "restore_targets",
    "released_refs",
    "release_reasons",
    "dependency_ref_pins",
];

/// `list[str]` fields, fail-closed to `[]` when malformed.
const REF_LIST_FIELDS: [&str; 3] = [
    "snapshot_lane_branches",
    "resume_seeded_refs",
    "dependency_refs",
];

/// Raised when a present `state.json` (or a decoded record) cannot be read.
///
/// Source's `ConsolidationStateReadError(GuardedReadError, RuntimeError)`; like the
/// guarded-read authority it subclasses, `Display` is the underlying reason and the
/// offending path is carried as its own attribute. Never returned for an absent
/// file (D5).
#[derive(Clone, Debug, PartialEq)]
pub struct ConsolidationStateReadError {
    path: PathBuf,
    reason: String,
}

impl ConsolidationStateReadError {
    fn new(path: &Path, reason: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }

    /// The record that could not be decoded.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The actionable reason (Source's `GuardedReadError.reason`).
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for ConsolidationStateReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for ConsolidationStateReadError {}

/// A decoded persisted merge record.
///
/// The view holds the authored values that survived Source's known-field filter and
/// fail-closed coercions, in dataclass declaration order. Typed accessors exist for
/// the fields the coordination consumers read; every other field is reachable raw
/// through [`ConsolidationState::field`], so nothing is dropped or re-typed on the
/// caller's behalf.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsolidationState {
    fields: PythonJsonDict,
}

impl ConsolidationState {
    /// Decode one persisted record, mirroring Source's `ConsolidationState.from_dict`.
    ///
    /// `path` is only used to anchor the error (Source's guarded read supplies it);
    /// it is never touched.
    ///
    /// # Errors
    ///
    /// [`ConsolidationStateReadError`] when the value is not a JSON object, or when a
    /// required field (`mission_id`, `mission_slug`, `target_branch`, `wp_order`) is
    /// absent — Source's `TypeError` arm.
    pub fn from_json(
        path: &Path,
        value: &PythonJsonValue,
    ) -> Result<Self, ConsolidationStateReadError> {
        let Some(object) = normalize_object(value) else {
            return Err(ConsolidationStateReadError::new(
                path,
                format!(
                    "merge state must be a JSON object, got {}",
                    python_type_name(value)
                ),
            ));
        };
        let mut fields = PythonJsonDict::new();
        for name in KNOWN_FIELDS {
            let stored = match object.get(name) {
                Some(item) => coerce_field(name, item),
                None if REQUIRED_FIELDS.contains(&name) => {
                    return Err(ConsolidationStateReadError::new(
                        path,
                        format!("merge state is missing the required field {name:?}"),
                    ));
                }
                None => default_field_value(name),
            };
            fields.insert(PythonString::from_scalars(name), stored);
        }
        Ok(Self { fields })
    }

    /// One persisted field by its authored name, or `None` when it is not a known field.
    pub fn field(&self, name: &str) -> Option<&PythonJsonValue> {
        self.fields.get(name)
    }

    /// The record's `mission_id` (always `Some` on a decoded record).
    ///
    /// Source's constructor refuses a record without it, so the accessor is `Option`
    /// only to keep the invariant visible instead of panicking.
    pub fn mission_id(&self) -> Option<&PythonJsonValue> {
        self.field("mission_id")
    }

    /// The record's `mission_slug` (always `Some` on a decoded record).
    pub fn mission_slug(&self) -> Option<&PythonJsonValue> {
        self.field("mission_slug")
    }

    /// The record's `target_branch` (always `Some` on a decoded record).
    pub fn target_branch(&self) -> Option<&PythonJsonValue> {
        self.field("target_branch")
    }

    /// The record's `wp_order` (always `Some` on a decoded record).
    pub fn wp_order(&self) -> Option<&PythonJsonValue> {
        self.field("wp_order")
    }

    /// The `pending_coord_reconcile` marker, or `None` when it is absent or JSON `null`.
    ///
    /// Source's `state.pending_coord_reconcile is not None`: any non-null value is
    /// returned as authored, including the empty mapping `{}` and any shape a
    /// producer wrote (validation is the consumer's job — see
    /// [`ConsolidationState::parse_pending_coord_reconcile`]).
    pub fn pending_coord_reconcile(&self) -> Option<&PythonJsonValue> {
        self.field("pending_coord_reconcile")
            .filter(|value| !matches!(value, PythonJsonValue::Null))
    }

    /// The strand commit SHAs this record's marker recorded (§5572).
    ///
    /// `None` means the marker recorded none (a legacy marker, a malformed value, or
    /// no marker at all); an explicitly recorded empty list is `Some([])`.
    pub fn marker_strand_shas(&self) -> Option<Vec<PythonString>> {
        marker_strand_shas(self.pending_coord_reconcile()?)
    }

    /// Validate the marker into the repair inputs the coordination consumers need.
    ///
    /// This is Source's `_parse_reconcile_marker`: `None` when the marker is absent or
    /// malformed (missing `coord_ref` / `captured_sha` / `coord_worktree`, or an empty
    /// `stranded_wp_ids` — an empty strand is not a strand).
    pub fn parse_pending_coord_reconcile(&self) -> Option<PendingCoordReconcile> {
        let marker = self.pending_coord_reconcile()?;
        parse_reconcile_marker(marker)
    }
}

/// The validated repair inputs of a `pending_coord_reconcile` marker.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingCoordReconcile {
    coord_ref: String,
    captured_sha: String,
    coord_worktree: String,
    stranded_wp_ids: Vec<PythonString>,
    strand_shas: Option<Vec<PythonString>>,
}

impl PendingCoordReconcile {
    /// The coordination ref the marker names (Source `coord_ref`).
    pub fn coord_ref(&self) -> &str {
        &self.coord_ref
    }

    /// The coordination tip captured before the pre-target `done` bake.
    pub fn captured_sha(&self) -> &str {
        &self.captured_sha
    }

    /// The coordination worktree the strand commit was made in.
    pub fn coord_worktree(&self) -> &str {
        &self.coord_worktree
    }

    /// The stranded write-set, exactly as authored (`str(w)` per item).
    pub fn stranded_wp_ids(&self) -> &[PythonString] {
        &self.stranded_wp_ids
    }

    /// The recorded strand commit SHAs, when the marker recorded any.
    pub fn strand_shas(&self) -> Option<&[PythonString]> {
        self.strand_shas.as_deref()
    }

    /// Project [`PendingCoordReconcile::stranded_wp_ids`] to Rust text.
    ///
    /// # Errors
    ///
    /// The first lone-surrogate code point, because a Rust `String` cannot carry it.
    /// There is no lossy fallback: callers must report the refusal rather than
    /// compare a silently rewritten work-package id.
    pub fn stranded_wp_ids_as_strings(&self) -> Result<Vec<String>, u32> {
        self.stranded_wp_ids
            .iter()
            .map(PythonString::to_scalar_string)
            .collect()
    }
}

/// The strand SHAs a marker recorded, or `None` when it recorded none.
///
/// Source `marker_strand_shas`: a falsy marker, an absent key, and a malformed value
/// all read as "nothing recorded"; an explicit empty list reads as `Some([])`.
pub fn marker_strand_shas(marker: &PythonJsonValue) -> Option<Vec<PythonString>> {
    if is_falsy(marker) {
        return None;
    }
    let value = dict_get(marker, STRAND_SHAS_KEY)?;
    let PythonJsonValue::Array(items) = value else {
        return None;
    };
    let mut shas = Vec::with_capacity(items.len());
    for item in items {
        let PythonJsonValue::String(text) = item else {
            return None;
        };
        shas.push(text.clone());
    }
    Some(shas)
}

/// Validate a raw marker value into [`PendingCoordReconcile`].
///
/// Source `_parse_reconcile_marker`, including its `str(w)` projection of the
/// stranded write-set: a JSON string keeps its exact code points, every other value
/// uses the canonical Python value `repr` (the single `str()`/`repr()` authority in
/// [`sk_kernel::python_json`]).
pub fn parse_reconcile_marker(marker: &PythonJsonValue) -> Option<PendingCoordReconcile> {
    if is_falsy(marker) {
        return None;
    }
    let coord_ref = nonempty_string(dict_get(marker, COORD_REF_KEY)?)?;
    let captured_sha = nonempty_string(dict_get(marker, CAPTURED_SHA_KEY)?)?;
    let coord_worktree = nonempty_string(dict_get(marker, COORD_WORKTREE_KEY)?)?;
    let stranded = dict_get(marker, STRANDED_WP_IDS_KEY)?;
    let PythonJsonValue::Array(items) = stranded else {
        return None;
    };
    if items.is_empty() {
        return None;
    }
    let stranded_wp_ids = items
        .iter()
        .map(|item| match item {
            PythonJsonValue::String(text) => text.clone(),
            other => PythonString::from_scalars(&python_repr_value(other)),
        })
        .collect();
    Some(PendingCoordReconcile {
        coord_ref,
        captured_sha,
        coord_worktree,
        stranded_wp_ids,
        strand_shas: marker_strand_shas(marker),
    })
}

/// `.kittify/runtime/merge` under `repo_root`.
pub fn merge_runtime_dir(repo_root: &Path) -> PathBuf {
    repo_root
        .join(KITTIFY_DIR_NAME)
        .join(RUNTIME_DIR_NAME)
        .join(MERGE_DIR_NAME)
}

/// The per-mission runtime directory `.kittify/runtime/merge/<mission_id>`.
pub fn mission_runtime_dir(repo_root: &Path, mission_id: &str) -> PathBuf {
    merge_runtime_dir(repo_root).join(mission_id)
}

/// The persisted record path `.kittify/runtime/merge/<mission_id>/state.json`.
///
/// Deliberately mirrors Source's `get_state_path(repo_root, mission_id)`; the legacy
/// `mission_id=None` location (`.kittify/merge-state.json`) is not reproduced here —
/// no read in this lane uses it, and keeping it would be a second path authority.
pub fn state_path(repo_root: &Path, mission_id: &str) -> PathBuf {
    mission_runtime_dir(repo_root, mission_id).join(STATE_FILENAME)
}

/// Load one mission's persisted record.
///
/// # Errors
///
/// [`ConsolidationStateReadError`] when the file exists but cannot be read or
/// decoded. An absent file is `Ok(None)` (Source's D5 rule).
pub fn load_state(
    repo_root: &Path,
    mission_id: &str,
) -> Result<Option<ConsolidationState>, ConsolidationStateReadError> {
    load_state_file(&state_path(repo_root, mission_id))
}

/// Load one `state.json` path.
///
/// # Errors
///
/// [`ConsolidationStateReadError`] for every unreadable or undecodable present file
/// — including a top-level value that is not an object. Callers that need Source's
/// *two* failure phases (a declared read error is skipped; a top-level non-object
/// escapes and aborts the sweep) use [`classify_state_file`] instead.
pub fn load_state_file(
    path: &Path,
) -> Result<Option<ConsolidationState>, ConsolidationStateReadError> {
    match classify_state_file(path) {
        StateFileOutcome::Absent => Ok(None),
        StateFileOutcome::Loaded(state) => Ok(Some(*state)),
        StateFileOutcome::Skipped(reason) | StateFileOutcome::Escaped(reason) => {
            Err(ConsolidationStateReadError::new(path, reason))
        }
    }
}

/// What one present `state.json` produced, keeping Source's failure phases apart.
///
/// Source `_load_state_file` reads through `read_guarded(..., errors=(JSONDecodeError,
/// TypeError, KeyError))`, which also declares `OSError`/`UnicodeDecodeError`. Those
/// collapse into `ConsolidationStateReadError`, which the enumerator **skips**. A JSON
/// top level that is not an object is *not* in that set: `from_dict` calls
/// `data.items()`, raising `AttributeError`, which escapes `read_guarded` and aborts
/// the whole sweep. Reporting either phase as the other would hide a real failure or
/// invent a skip, so they are distinct here.
#[derive(Debug)]
pub enum StateFileOutcome {
    /// The path does not exist (Source's `if not state_path.exists(): return None`).
    Absent,
    /// A declared read failure: unreadable bytes, invalid UTF-8, invalid JSON, or a
    /// missing required field. Source skips the record.
    Skipped(String),
    /// The decoded JSON top level is not an object — Source's escaped `AttributeError`.
    Escaped(String),
    /// A decoded record.
    Loaded(Box<ConsolidationState>),
}

/// Classify one `state.json` path into Source's own failure phases.
///
/// Read-only: no lock, no write, no repair. The record itself is decoded by
/// [`ConsolidationState::from_json`] — this function adds no model, coercion or
/// default of its own.
pub fn classify_state_file(path: &Path) -> StateFileOutcome {
    match std::fs::metadata(path) {
        // Source `Path.exists()`: a missing path and a non-directory ancestor are
        // both "absent", never an error.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return StateFileOutcome::Absent;
        }
        // `read_guarded` declares `OSError`.
        Err(error) => return StateFileOutcome::Skipped(error.to_string()),
        Ok(_) => {}
    }
    let bytes = match read_source_bytes(path) {
        Ok(bytes) => bytes,
        Err(error) => return StateFileOutcome::Skipped(error.to_string()),
    };
    // `read_guarded` declares `UnicodeDecodeError`.
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) => return StateFileOutcome::Skipped(error.to_string()),
    };
    // `read_guarded` declares `JSONDecodeError`.
    let value = match parse(text) {
        Ok(value) => value,
        Err(error) => return StateFileOutcome::Skipped(error.to_string()),
    };
    if !matches!(
        value,
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_)
    ) {
        // `from_dict`'s `data.items()` on a list/scalar/None raises AttributeError,
        // which `read_guarded` does not catch: the sweep aborts.
        return StateFileOutcome::Escaped(format!(
            "merge state must be a JSON object, got {}",
            python_type_name(&value)
        ));
    }
    match ConsolidationState::from_json(path, &value) {
        // `cls(**filtered)` raises TypeError for a missing required field.
        Ok(state) => StateFileOutcome::Loaded(Box::new(state)),
        Err(error) => StateFileOutcome::Skipped(error.to_string()),
    }
}

/// Every persisted merge record under `.kittify/runtime/merge/*/state.json` that
/// carries a non-`null` `pending_coord_reconcile` marker.
///
/// Source `iter_pending_coord_reconcile_markers`. This is the **enumeration** seam
/// the coordination safety net must use: `load_state` with no mission id raises
/// `MergeAmbiguousStateError` as soon as two active records exist, so it cannot
/// enumerate, and a re-implemented runtime-path scan would be a second path
/// authority (DIR-044).
///
/// Contract, matching Source:
///
/// * deterministic order — entries are visited in Python `sorted(iterdir())` order
///   (code-point order of the surrogate-escaped directory name);
/// * a record whose marker is `null`/absent is skipped; an empty mapping `{}` IS a
///   marker and is yielded (the consumer decides it is unparseable);
/// * a present-but-undecodable `state.json` is skipped, so one mission's corrupt
///   record never aborts the sweep over every other mission;
/// * nothing is created, locked or written; a missing `.kittify/runtime/merge`
///   directory yields an empty result.
pub fn iter_pending_coord_reconcile_markers(repo_root: &Path) -> Vec<ConsolidationState> {
    // This name keeps its original "skip anything unreadable" contract for its
    // existing callers. The doctor's safety net uses
    // [`iter_pending_coord_reconcile_markers_source`], which preserves Source's
    // escaped phase; a sweep that fails for the strict caller must not silently
    // collapse to "no markers" here either, so it degrades to the same skip.
    scan_pending_markers(repo_root, false).unwrap_or_default()
}

/// Why the marker sweep could not finish.
#[derive(Debug, thiserror::Error)]
pub enum ConsolidationSweepError {
    /// A `state.json` decoded to a non-object top level.
    ///
    /// Source's `from_dict` calls `data.items()` on that value, raising
    /// `AttributeError`, which `read_guarded` does not declare and the enumerator
    /// does not catch — the sweep aborts instead of skipping the record.
    #[error("{path}: {reason}")]
    EscapedTopLevel {
        /// The offending record.
        path: PathBuf,
        /// The observed value's type.
        reason: String,
    },
}

/// Every pending marker, preserving Source's caught/escaped failure phases.
///
/// Same traversal, order, skip rules and "an empty mapping IS a marker" rule as
/// [`iter_pending_coord_reconcile_markers`], but a `state.json` whose top level is
/// not an object is a typed **error** here rather than a silent skip: that is the
/// one failure the reference lets escape, and a diagnostic must not report a sweep
/// it could not complete as a clean one.
pub fn iter_pending_coord_reconcile_markers_source(
    repo_root: &Path,
) -> Result<Vec<ConsolidationState>, ConsolidationSweepError> {
    scan_pending_markers(repo_root, true)
}

/// One traversal, two contracts: skip a non-object top level, or report it.
fn scan_pending_markers(
    repo_root: &Path,
    escape_on_non_object: bool,
) -> Result<Vec<ConsolidationState>, ConsolidationSweepError> {
    let runtime_merge_dir = merge_runtime_dir(repo_root);
    let Ok(entries) = std::fs::read_dir(&runtime_merge_dir) else {
        return Ok(Vec::new());
    };
    let mut candidates: Vec<(Vec<u32>, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        candidates.push((python_fsdecode(&name), entry.path().join(STATE_FILENAME)));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    let mut states = Vec::new();
    for (_, state_file) in candidates {
        match classify_state_file(&state_file) {
            StateFileOutcome::Absent | StateFileOutcome::Skipped(_) => continue,
            StateFileOutcome::Escaped(reason) => {
                if escape_on_non_object {
                    return Err(ConsolidationSweepError::EscapedTopLevel {
                        path: state_file,
                        reason,
                    });
                }
                continue;
            }
            StateFileOutcome::Loaded(state) => {
                if state.pending_coord_reconcile().is_some() {
                    states.push(*state);
                }
            }
        }
    }
    Ok(states)
}

// ---------------------------------------------------------------------------
// Source `from_dict` projections
// ---------------------------------------------------------------------------

/// Normalize either dict view of the canonical value model into an exact object.
fn normalize_object(value: &PythonJsonValue) -> Option<PythonJsonDict> {
    match value {
        PythonJsonValue::ExactObject(object) => Some(object.clone()),
        PythonJsonValue::Object(entries) => {
            let mut object = PythonJsonDict::new();
            for (key, item) in entries {
                object.insert(PythonString::from_scalars(key), item.clone());
            }
            Some(object)
        }
        _ => None,
    }
}

/// `dict.get(key)` over either dict view of the canonical value model.
fn dict_get<'a>(value: &'a PythonJsonValue, key: &str) -> Option<&'a PythonJsonValue> {
    match value {
        PythonJsonValue::ExactObject(object) => object.get(key),
        PythonJsonValue::Object(entries) => entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, item)| item),
        _ => None,
    }
}

/// Source's `str` type name for a decoded JSON value (used in the refusal text).
fn python_type_name(value: &PythonJsonValue) -> &'static str {
    match value {
        PythonJsonValue::Null => "NoneType",
        PythonJsonValue::Bool(_) => "bool",
        PythonJsonValue::Number(_) | PythonJsonValue::Integer(_) => "int",
        PythonJsonValue::Float(_) => "float",
        PythonJsonValue::String(_) => "str",
        PythonJsonValue::Array(_) => "list",
        PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => "dict",
    }
}

/// A non-empty JSON string, exactly as authored.
fn nonempty_string(value: &PythonJsonValue) -> Option<String> {
    let PythonJsonValue::String(text) = value else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    text.to_scalar_string().ok()
}

/// Apply Source's per-field fail-closed projection.
fn coerce_field(name: &str, value: &PythonJsonValue) -> PythonJsonValue {
    if REF_MAP_FIELDS.contains(&name) {
        return string_map_or_empty(value);
    }
    if REF_LIST_FIELDS.contains(&name) {
        return string_list_or_default(value, Vec::new());
    }
    match name {
        "unsettled_refs" => string_list_or_default(
            value,
            vec![PythonJsonValue::String(PythonString::from_scalars(
                UNSETTLED_ALL,
            ))],
        ),
        "advance_intents" => intent_chains_or_empty(value),
        _ => value.clone(),
    }
}

/// `dict[str, str]` or `{}`.
fn string_map_or_empty(value: &PythonJsonValue) -> PythonJsonValue {
    let all_strings = match value {
        PythonJsonValue::ExactObject(object) => object
            .iter()
            .all(|(_, item)| matches!(item, PythonJsonValue::String(_))),
        PythonJsonValue::Object(entries) => entries
            .iter()
            .all(|(_, item)| matches!(item, PythonJsonValue::String(_))),
        _ => false,
    };
    if all_strings {
        value.clone()
    } else {
        PythonJsonValue::ExactObject(PythonJsonDict::new())
    }
}

/// `list[str]`, else `fallback`.
fn string_list_or_default(
    value: &PythonJsonValue,
    fallback: Vec<PythonJsonValue>,
) -> PythonJsonValue {
    match value {
        PythonJsonValue::Array(items)
            if items
                .iter()
                .all(|item| matches!(item, PythonJsonValue::String(_))) =>
        {
            value.clone()
        }
        _ => PythonJsonValue::Array(fallback),
    }
}

/// `dict[str, list[str]]` with every chain at least two SHAs long, else `{}`.
///
/// Source returns `{}` on the FIRST malformed entry, so chains that preceded it do
/// not survive either.
fn intent_chains_or_empty(value: &PythonJsonValue) -> PythonJsonValue {
    let valid = match value {
        PythonJsonValue::ExactObject(object) => object.iter().all(|(_, chain)| {
            matches!(
                chain,
                PythonJsonValue::Array(items)
                    if items.len() >= 2
                        && items.iter().all(|sha| matches!(sha, PythonJsonValue::String(_)))
            )
        }),
        PythonJsonValue::Object(entries) => entries.iter().all(|(_, chain)| {
            matches!(
                chain,
                PythonJsonValue::Array(items)
                    if items.len() >= 2
                        && items.iter().all(|sha| matches!(sha, PythonJsonValue::String(_)))
            )
        }),
        _ => false,
    };
    if valid {
        value.clone()
    } else {
        PythonJsonValue::ExactObject(PythonJsonDict::new())
    }
}

/// The dataclass default for an absent field with a default.
///
/// `started_at` / `updated_at` default to the reference constructor's
/// `now_utc_iso()`, reproduced here through the canonical Kernel clock.
fn default_field_value(name: &str) -> PythonJsonValue {
    match name {
        "completed_wps"
        | "dependency_refs"
        | "snapshot_lane_branches"
        | "resume_seeded_refs"
        | "unsettled_refs" => PythonJsonValue::Array(Vec::new()),
        "has_pending_conflicts" | "mission_number_baked" | "push_requested" | "skip_lanes" => {
            PythonJsonValue::Bool(false)
        }
        "strategy" => PythonJsonValue::String(PythonString::from_scalars("merge")),
        "started_at" | "updated_at" => {
            PythonJsonValue::String(PythonString::from_scalars(&now_utc_iso()))
        }
        "dependency_ref_pins"
        | "pre_interrupt_lane_tips"
        | "pre_mutation_refs"
        | "post_mutation_refs"
        | "restore_targets"
        | "advance_intents"
        | "released_refs"
        | "release_reasons" => PythonJsonValue::ExactObject(PythonJsonDict::new()),
        // `current_wp`, `workspace_path`, `delete_branch`, `remove_worktree`,
        // `pending_coord_reconcile` and the optional SHA anchors all default to None.
        _ => PythonJsonValue::Null,
    }
}

// ---------------------------------------------------------------------------
// Directory ordering (Python `sorted(Path.iterdir())`)
// ---------------------------------------------------------------------------

/// Python's filesystem-decoder code points for a directory name (`os.fsdecode`).
///
/// Python sorts `Path` objects by their surrogate-escaped text, so an undecodable
/// name byte becomes `U+DC80..U+DCFF` and sorts above ordinary code points. Sorting
/// by raw bytes would order the same directory differently on that boundary.
fn python_fsdecode(name: &OsStr) -> Vec<u32> {
    let bytes = name.as_encoded_bytes();
    match std::str::from_utf8(bytes) {
        Ok(text) => text.chars().map(u32::from).collect(),
        Err(_) => {
            let mut codepoints = Vec::with_capacity(bytes.len());
            let mut rest = bytes;
            while !rest.is_empty() {
                match std::str::from_utf8(rest) {
                    Ok(text) => {
                        codepoints.extend(text.chars().map(u32::from));
                        break;
                    }
                    Err(error) => {
                        let valid = &rest[..error.valid_up_to()];
                        if let Ok(text) = std::str::from_utf8(valid) {
                            codepoints.extend(text.chars().map(u32::from));
                        }
                        let width = error
                            .error_len()
                            .unwrap_or(rest.len() - error.valid_up_to());
                        for byte in &rest[error.valid_up_to()..error.valid_up_to() + width] {
                            codepoints.push(0xDC00 + u32::from(*byte));
                        }
                        rest = &rest[error.valid_up_to() + width..];
                    }
                }
            }
            codepoints
        }
    }
}

//! Committed candidate-DONE reducer — Source `coordination.coherence.coord_incoherent_done_wps`.
//!
//! # What this module answers
//!
//! "Which of the work packages THIS merge writes `done` still reduce to `done` on the
//! **committed** coordination ref?" That is the strand authority (`#2786` / `#2367-B`
//! FR-009): a marker-persist site, a resume heal gate and the coordination doctor all
//! consume this one derivation, so the three can never drift.
//!
//! # The three load-bearing rules (copied from the reference's contract)
//!
//! 1. **Candidates only.** The caller passes this merge's pre-target `done` write-set.
//!    This function never enumerates work packages: passing every WP id would, on a
//!    resume, re-strand a work package a previous attempt legitimately baked `done`.
//!    A genuinely pre-existing `done` WP is excluded *by construction* because it is
//!    not in the write-set.
//! 2. **The committed ref, never a worktree.** The log is read out of the branch tree
//!    with the canonical Git reader — no checkout, index, lock or temporary file — so
//!    a rollback that restored the primary checkout cannot change the answer.
//! 3. **Reorder and duplicates are the caller's.** The result is `candidate_wps`
//!    filtered in place, so order and repeated ids survive exactly.
//!
//! # What is reused, not cloned
//!
//! * [`sk_git`] is the ONE ref/blob reader: [`RefName`] / [`BranchName`] validate the
//!   ref, [`GitRepository::read_ref`] decides "no such branch", and
//!   [`GitRepository::read_blob_at_bounded`] reads the exact tree entry.
//! * [`sk_status::mutation_log::parse_log_rows`] is the canonical JSONL-text → object
//!   rows reader (blank lines skipped, a non-object or malformed line refuses with its
//!   line number).
//! * [`sk_status::parse_diary`] + [`sk_status::replay::reduce_diary`] are the canonical
//!   diary partition and deterministic fold; the lane comparison is the canonical
//!   [`sk_status::Lane`] model. No second status parser, reducer or lane table exists
//!   here.
//!
//! # Admission before reduction (CF49)
//!
//! The committed log is admitted through
//! [`sk_status::replay::read_source_event_stream_from_text`], the ONE owner of the
//! reference's `status.store.read_events_from_text` contract, **before** the fold runs.
//! JSON syntactic success is not model admission: a row the reference's event store
//! refuses (a missing `to_lane`, an unknown lane, a malformed annotation) or a row the
//! reference's own fold cannot key on (a container-valued `wp_id`/`at`) is a typed
//! refusal here, never the healthy "no strand" answer. Both refusal buckets keep the
//! phases the reference has: a row-syntax failure is [`CoordDoneReadError::MalformedLog`],
//! an event-model failure is [`CoordDoneReadError::UndecodableDiary`].
//!
//! # Read-only
//!
//! Nothing in this module reverts, clears a marker, writes a ref, materialises a
//! worktree or repairs anything. The repair itself stays with its owning authority.
//!
//! # Ref spelling
//!
//! Source shells `git show <destination_ref>:<path>`, so any revision spelling git
//! accepts resolves there. The documented contract for this parameter is the
//! coordination **branch** the marker records. [`crate::coord::read_status_events_at_ref`]
//! accepts that spelling — a fully-qualified ref (`refs/...`) as-is, or a short
//! branch name (`kitty/mission-…`) qualified through the canonical ref model — and
//! reports any other revision spelling as
//! [`CoordDoneReadError::UnresolvableCoordRef`] rather than guessing. Widening that
//! resolver is a named Git-capability gap; it is not an "empty event list".

use std::path::Path;

use sk_git::GitRepository;
use sk_status::source_event::{SourceFoldError, SourceSnapshot};
use sk_status::source_execution::SourceExecutionError;
use sk_status::{EVENTS_FILENAME, Lane};
use thiserror::Error;

use crate::coord::{CoordStatusRefReadError, read_status_events_at_ref};
use crate::workspace::SPECS_DIR_NAME;

/// Why a committed-ref strand derivation could not be completed.
///
/// Source raises for the structured failures — an invalid JSON line or an event that
/// does not decode into a lane diary (`StoreError`) — and this carries the same two
/// phases. The reference's *caught* arm (a non-zero `git show`: no branch, no log
/// path, an unreadable repository) is `Ok(None)` at the reader and `Ok(vec![])` here,
/// exactly as the reference returns an empty event list.
///
/// The first three variants are Native-only limits of the canonical reader. They are
/// errors, never an empty answer: a committed log this adapter cannot read must not
/// be reported as "no strand".
///
/// # Caught versus escaped (CF62)
///
/// `MalformedLog`/`UndecodableDiary` are the reference's *caught* `StoreError`
/// phases. The reference's store does **not** catch every failure: an annotation's
/// `int(Infinity)` `OverflowError`, `json.loads`'s digit-ceiling `ValueError`, and the
/// metadata resolver's escaped `OSError`/`RecursionError` escape to the caller. Those
/// are [`Self::EscapedSourceException`] — the class and phase survive — and a native
/// adapter limit with no Source counterpart is [`Self::SourceOperationalLimit`].
/// Neither can be read as a refused row or as "no strand".
#[derive(Debug, Error)]
pub enum CoordDoneReadError {
    /// The ref is neither a fully-qualified ref nor a branch name.
    #[error("coordination ref {value:?} is neither a fully-qualified ref nor a branch name")]
    UnresolvableCoordRef {
        /// The offending ref text.
        value: String,
    },
    /// The Mission directory cannot be addressed as a Git tree path.
    #[error("mission directory {detail} cannot be addressed as a Git tree path")]
    MissionDirName {
        /// What was wrong (no final component, invalid path bytes, …).
        detail: String,
    },
    /// The committed log is larger than the canonical reader's bounded blob read.
    #[error("{reference}: the committed status log exceeds the {limit}-byte read bound")]
    OverBound {
        /// The `<ref>:<path>` the read came from.
        reference: String,
        /// The adapter's bound.
        limit: u64,
    },
    /// The committed log is not JSONL object rows.
    #[error("{reference}: line {line}: {message}")]
    MalformedLog {
        /// The `<ref>:<path>` the read came from.
        reference: String,
        /// The 1-based line number of the offending row.
        line: usize,
        /// The decoder's reason.
        message: String,
    },
    /// The rows are not a reducible lane diary.
    #[error("{reference}: the committed status log is not a reducible lane diary: {detail}")]
    UndecodableDiary {
        /// The `<ref>:<path>` the read came from.
        reference: String,
        /// The reducer's reason.
        detail: String,
    },
    /// A Python exception the reference's store (or fold) let ESCAPE.
    ///
    /// The reference's own caller does not catch it either, so this is not a
    /// "corrupt row" answer: the store stopped with this class at this phase.
    #[error("{reference}: {class} escaped the status store during {phase}: {message}")]
    EscapedSourceException {
        /// The `<ref>:<path>` the read came from.
        reference: String,
        /// The Python exception class (`OverflowError`, `ValueError`, …).
        class: String,
        /// The reference phase (`annotation_admission`, `mission_id_exists`, `fold`, …).
        phase: String,
        /// The reference's own message text.
        message: String,
    },
    /// A native adapter limit with no Source counterpart.
    ///
    /// The reference cannot produce this failure (its JSON decoder would either
    /// accept the input or raise a Python class), so it is labelled as a native
    /// limit rather than given a fabricated Python exception name.
    #[error("{reference}: native Source-adapter limit during {phase}: {detail}")]
    SourceOperationalLimit {
        /// The `<ref>:<path>` the read came from.
        reference: String,
        /// The execution phase.
        phase: String,
        /// What the native adapter refused.
        detail: String,
    },
}

impl From<CoordStatusRefReadError> for CoordDoneReadError {
    fn from(error: CoordStatusRefReadError) -> Self {
        match error {
            CoordStatusRefReadError::UnsupportedRef { value } => {
                Self::UnresolvableCoordRef { value }
            }
            CoordStatusRefReadError::MissionDirName { detail } => Self::MissionDirName { detail },
            CoordStatusRefReadError::OverBound { reference, limit } => {
                Self::OverBound { reference, limit }
            }
        }
    }
}

/// The subset of `candidate_wps` still reducing to `done` on the committed `coord_ref`.
///
/// # Arguments
///
/// * `coord_ref` — the coordination branch the marker recorded, either fully
///   qualified (`refs/heads/…`) or short (`kitty/mission-…`).
/// * `candidate_wps` — this merge's pre-target `done` write-set, in the caller's
///   order. Never "all work packages"; see the module docs.
/// * `repo_root` — the repository the branch ref belongs to.
/// * `feature_dir` — the Mission directory whose final component anchors the ref
///   path (`kitty-specs/<name>/status.events.jsonl`), exactly as the rollback's
///   placement resolved it (passed in, never re-resolved, so placement cannot drift).
///
/// # Returns
///
/// The candidates that reduce to `done`, in candidate order, duplicates preserved.
/// An empty candidate list short-circuits before any Git read; a branch or log that
/// does not exist is `Ok(vec![])` (Source's non-zero `git show` arm: there is no
/// strand to derive).
///
/// # Errors
///
/// [`CoordDoneReadError`] when the ref cannot be resolved to a branch, the Mission
/// directory cannot be a tree path, Git cannot read the ref/blob, or the committed
/// log exists but cannot be decoded. A readable log is required before this function
/// will claim a WP is coherent: an I/O-level failure is never reported as "no strand".
pub fn coord_incoherent_done_wps(
    coord_ref: &str,
    candidate_wps: &[String],
    repo_root: &Path,
    feature_dir: &Path,
) -> Result<Vec<String>, CoordDoneReadError> {
    if candidate_wps.is_empty() {
        return Ok(Vec::new());
    }
    let Some(text) = read_committed_event_text(coord_ref, repo_root, feature_dir)? else {
        return Ok(Vec::new());
    };
    let reference = format!("{coord_ref}:{}", readable_status_log_path(feature_dir));
    // The reference admits every row through its event store BEFORE folding; a log it
    // refuses (or whose identity its fold cannot key on) has no lane answer at all.
    let admitted = sk_status::replay::read_source_event_stream_from_text(feature_dir, &text)
        .map_err(|error| admission_error(&reference, error))?;
    if admitted.transitions.is_empty() {
        // Source's `if not events: return []` — no lane transition on the ref.
        return Ok(Vec::new());
    }
    // Annotations are deliberately not folded: this read is the transitions-only
    // view (`read_events_from_text`), so `role`/subtask deltas cannot move a lane.
    // The fold consumes the Source-valued identities/truth it was admitted with;
    // a fold refusal is the reference's own TypeError/AttributeError, never a
    // healthy "no strand" answer.
    let snapshot = sk_status::source_event::reduce_source_diary(&admitted.transitions, &[])
        .map_err(|error| fold_error(&reference, error))?;
    Ok(candidate_wps
        .iter()
        .filter(|wp_id| reduces_to_done(&snapshot, wp_id))
        .cloned()
        .collect())
}

/// True when the reduced snapshot puts `wp_id` in the `done` lane.
///
/// Source `wp_lane_actor_from_events(events, wp_id).lane == Lane.DONE`: an unseeded WP
/// (no events, or no snapshot entry) is `GENESIS`, and an unrecognised persisted lane
/// string falls back to `GENESIS` through its `ValueError` arm. Neither is `done`.
fn reduces_to_done(snapshot: &SourceSnapshot, wp_id: &str) -> bool {
    let key = sk_kernel::python_json::PythonJsonValue::scalar(wp_id);
    let Ok(Some(state)) = snapshot.work_package(&key) else {
        return false;
    };
    state.lane() == Some(Lane::Done)
}

/// Map a Source fold refusal onto the consumer's error space.
///
/// The reference's `reduce` runs after its store read returns, so its
/// `TypeError`/`AttributeError` escape the caller rather than becoming a
/// `StoreError`; the escaped class and the `fold` phase are preserved.
fn fold_error(reference: &str, error: SourceFoldError) -> CoordDoneReadError {
    match SourceExecutionError::from(error) {
        SourceExecutionError::Escaped(raised) => CoordDoneReadError::EscapedSourceException {
            reference: reference.to_owned(),
            class: raised.class().name().to_owned(),
            phase: raised.phase().name().to_owned(),
            message: raised.message().to_owned(),
        },
        other => CoordDoneReadError::UndecodableDiary {
            reference: reference.to_owned(),
            detail: other.to_string(),
        },
    }
}

/// Map one admission outcome onto the consumer's own phase buckets.
///
/// The reference's **caught** `StoreError` rows keep the split CF47 pinned: a
/// *row-syntax* failure is [`CoordDoneReadError::MalformedLog`] (it carries the
/// offending line), an *event-model* failure is
/// [`CoordDoneReadError::UndecodableDiary`] (the rows are not an admissible event
/// stream). Both retain the reference's own message text, which already names the
/// line, so nothing about the cause is lost. What the reference did **not** catch
/// (its escaping Python class) and a native adapter limit are their own variants —
/// this function never folds them into the refused-row buckets.
fn admission_error(reference: &str, error: SourceExecutionError) -> CoordDoneReadError {
    match &error {
        // The reference's caught `StoreError` phases keep their own buckets: a
        // row-syntax failure carries the offending line, an event-model failure
        // is "not a reducible lane diary".
        SourceExecutionError::Store(store) => match store {
            sk_status::replay::SourceEventLogError::InvalidJson { line, .. }
            | sk_status::replay::SourceEventLogError::NonObjectRow { line } => {
                CoordDoneReadError::MalformedLog {
                    reference: reference.to_owned(),
                    line: *line,
                    message: store.to_string(),
                }
            }
            other => CoordDoneReadError::UndecodableDiary {
                reference: reference.to_owned(),
                detail: other.to_string(),
            },
        },
        // The reference let this Python exception out — never a corrupt-row
        // answer and never "no strand".
        SourceExecutionError::Escaped(raised) => CoordDoneReadError::EscapedSourceException {
            reference: reference.to_owned(),
            class: raised.class().name().to_owned(),
            phase: raised.phase().name().to_owned(),
            message: raised.message().to_owned(),
        },
        // A native adapter limit with no Source counterpart.
        SourceExecutionError::Operational(operational) => {
            CoordDoneReadError::SourceOperationalLimit {
                reference: reference.to_owned(),
                phase: operational.phase().name().to_owned(),
                detail: operational.detail().to_owned(),
            }
        }
    }
}

/// The exact bytes of the committed status log, or `None` when there is none.
///
/// The ref/blob read is [`crate::coord::read_status_events_at_ref`] — the canonical
/// coordination committed-status read this checkout's own coordination path uses —
/// so this module authors no second ref reader, blob reader or path grammar. This
/// function only decodes what that owner returns.
fn read_committed_event_text(
    coord_ref: &str,
    repo_root: &Path,
    feature_dir: &Path,
) -> Result<Option<String>, CoordDoneReadError> {
    let mission_dir_name = mission_dir_name(feature_dir)?;
    // Source runs `git show` with `cwd=repo_root`; a repository it cannot open makes
    // git exit non-zero, which the reference's contract reads as "no events".
    let Ok(repository) = GitRepository::open(repo_root) else {
        return Ok(None);
    };
    let Some(bytes) = read_status_events_at_ref(&repository, coord_ref, &mission_dir_name)? else {
        // The reference's caught `git show` arm: absent ref, absent log path, or a
        // git-level refusal. There is no strand to derive.
        return Ok(None);
    };
    // Source: `subprocess.run(..., text=True, encoding="utf-8", errors="replace")` —
    // invalid bytes become U+FFFD instead of raising, so the refusal (if any) comes
    // from the JSON/event admission below, exactly where the reference's does.
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The final component of the Mission directory — the `kitty-specs/<name>/` anchor.
fn mission_dir_name(feature_dir: &Path) -> Result<String, CoordDoneReadError> {
    let Some(name) = feature_dir.file_name() else {
        return Err(CoordDoneReadError::MissionDirName {
            detail: format!("{feature_dir:?} has no final path component"),
        });
    };
    // A non-UTF-8 directory name cannot be spelled as a coordination tree path this
    // adapter can build; refuse rather than rewrite the identity lossily.
    name.to_str()
        .map(str::to_owned)
        .ok_or_else(|| CoordDoneReadError::MissionDirName {
            detail: format!("{name:?} is not valid UTF-8"),
        })
}

/// The status log path, for error text only (never for identity).
fn readable_status_log_path(feature_dir: &Path) -> String {
    match feature_dir.file_name().and_then(|name| name.to_str()) {
        Some(name) => format!("{SPECS_DIR_NAME}/{name}/{EVENTS_FILENAME}"),
        None => "<unresolvable status log path>".to_owned(),
    }
}

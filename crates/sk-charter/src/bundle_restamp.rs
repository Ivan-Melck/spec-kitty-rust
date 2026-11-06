//! Re-stamp the synthesis manifest's `bundle_content_hash` after a `charter.yaml` write.
//!
//! `bundle_content_hash` is the content identity of the activated charter recorded
//! in `.kittify/charter/synthesis-manifest.yaml`. Activation and the compiled-charter
//! refresh both rewrite the activation authority; without a re-stamp the committed
//! manifest drifts and every fresh clone inherits a stale-bundle preflight.
//!
//! This is the Native owner of `src/charter/activation/bundle_restamp.py`, moved
//! beside the two authorities it composes:
//!
//! * the digest is always [`crate::compute_bundle_content_hash`] — the one
//!   bundle content-identity recipe, never a second hash;
//! * the manifest is the canonical [`SynthesisManifest`]: the stamp is written
//!   from [`SynthesisManifest::finalize`]'s self-hash and
//!   [`SynthesisManifest::to_canonical_bytes`], so only `bundle_content_hash`
//!   and the derived `manifest_hash` can change value. As in the Source, the
//!   canonical serialization also materialises the model's own fields (so an
//!   authored `schema_version`/`built_in_only`/`mission_id` default is written
//!   in canonical key order);
//! * publication uses the kernel's Source `TextTruncate` policy, matching
//!   `Path.write_text`: follow links and update the existing inode in place,
//!   without durable replacement, permission bypass or observer notification.
//!
//! The run-wide charter synthesis lock lives in `sk-doctrine`
//! (`sk_doctrine::acquire_synthesis_lock`), which is the one cross-language lock
//! owner; this crate does not fork it. The caller therefore supplies the
//! acquire step, and the guard it returns is held for the whole
//! read-modify-write. That keeps the dependency direction (`sk-charter` depends
//! on `sk-kernel` only) and keeps ONE lock authority.
//!
//! No-op semantics are the Source's exactly: `false`/no write when there is no
//! manifest, when the manifest is a built-in-only seed, when it carries no
//! stamp, when the bundle cannot be hashed, or when the stamp already matches.
//! A lock the caller cannot take is *not* an abort — the activation change it
//! follows is already saved — so it yields one warning line
//! ([`RestampOutcome::warning_line`]) and no write, while a nested call inside a
//! synthesis run re-enters the same (re-entrant) lock and still restamps.
//!
//! Contract mirror (Python reference, never invoked at runtime):
//! `src/charter/activation/bundle_restamp.py`.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::error::CharterError;
use crate::hash::compute_bundle_content_hash;
use crate::synthesis::{SYNTHESIS_MANIFEST_PATH, SynthesisManifest};

/// One stderr line when the restamp is skipped; `{reason}` names why the lock
/// was not taken (the Source's `_RESTAMP_SKIPPED_WARNING`).
pub const RESTAMP_SKIPPED_WARNING: &str = "Warning: the change was saved, but the synthesis manifest's bundle hash was not refreshed: {reason}. The next activation change or synthesis run will restamp it.";

/// Reason text for a lock another writing run holds (the Source's `_HELD_REASON`).
pub const RESTAMP_HELD_REASON: &str =
    "a `charter synthesize`/`resynthesize` run in this worktree holds {lock_path}";

/// Reason text for a lock that cannot be placed at all (the Source's
/// `_UNAVAILABLE_REASON`).
pub const RESTAMP_UNAVAILABLE_REASON: &str = "git could not be invoked to place the synthesis lock";

/// Why the run-wide lock could not be taken.
///
/// Mirrors the Source's two typed exceptions, plus an explicit "other" arm so a
/// different acquire failure is propagated instead of being reported as the
/// "change was saved but not refreshed" warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestampLockFailure {
    /// Another writing run in this worktree holds the lock past the bounded wait.
    Held {
        /// The lock file the other run holds.
        lock_path: PathBuf,
    },
    /// Git could not be invoked to place the lock.
    Unavailable,
    /// Any other acquire failure: propagated, never reported as a skip.
    Other {
        /// The failure as the lock owner rendered it.
        detail: String,
    },
}

impl RestampLockFailure {
    /// The reason sentence the skip warning carries (the Source's wording).
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Held { lock_path } => {
                RESTAMP_HELD_REASON.replace("{lock_path}", &lock_path.display().to_string())
            }
            Self::Unavailable => RESTAMP_UNAVAILABLE_REASON.to_owned(),
            Self::Other { detail } => detail.clone(),
        }
    }
}

impl fmt::Display for RestampLockFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason())
    }
}

/// What [`restamp_bundle_hash`] observed and did.
///
/// The Source returns a bare `bool` ("the manifest was rewritten"). The typed
/// form keeps that observable (`is_restamped`) and names every no-op cause, so a
/// caller (and a test) can tell "nothing to keep in parity" apart from "the lock
/// was busy".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestampOutcome {
    /// `.kittify/charter/synthesis-manifest.yaml` does not exist.
    NoManifest,
    /// The manifest records a built-in-only seed: it carries no stamp to keep in parity.
    BuiltInOnly,
    /// The manifest carries no `bundle_content_hash` (a pre-stamp manifest).
    Unstamped,
    /// The bundle cannot be hashed (a missing or unreadable member).
    Unhashable,
    /// The recorded stamp already matches the bundle.
    AlreadyCurrent {
        /// The matching `sha256:...` stamp.
        bundle_content_hash: String,
    },
    /// The manifest was rewritten with the current stamp.
    Restamped {
        /// The stamp now recorded.
        bundle_content_hash: String,
        /// The stamp it replaced.
        previous_bundle_content_hash: String,
    },
    /// The run-wide lock could not be taken; nothing was written.
    Skipped(RestampLockFailure),
}

impl RestampOutcome {
    /// Whether the manifest was rewritten (the Source's `True`).
    #[must_use]
    pub fn is_restamped(&self) -> bool {
        matches!(self, Self::Restamped { .. })
    }

    /// The run-wide lock skip, when the restamp was skipped for that reason.
    #[must_use]
    pub fn skipped(&self) -> Option<&RestampLockFailure> {
        match self {
            Self::Skipped(failure) => Some(failure),
            _ => None,
        }
    }

    /// The one stderr warning line a skip emits, or `None` when nothing was skipped.
    #[must_use]
    pub fn warning_line(&self) -> Option<String> {
        self.skipped()
            .map(|failure| RESTAMP_SKIPPED_WARNING.replace("{reason}", &failure.reason()))
    }
}

/// A restamp that could not read or rewrite the manifest.
///
/// The Source lets the model-validation and IO errors escape; Native keeps the
/// same fail-closed posture with a typed error, so a corrupt manifest is never
/// reported as "restamped" or silently skipped.
#[derive(Debug, thiserror::Error)]
pub enum RestampError {
    /// The manifest exists but cannot be read or validated.
    #[error("the synthesis manifest {} cannot be read: {source}", path.display())]
    Manifest {
        /// The manifest path.
        path: PathBuf,
        /// The underlying charter failure.
        #[source]
        source: CharterError,
    },
    /// The restamped manifest could not be written.
    #[error("the synthesis manifest {} cannot be written: {source}", path.display())]
    Write {
        /// The manifest path.
        path: PathBuf,
        /// The underlying kernel failure.
        #[source]
        source: sk_kernel::source_writer::SourceWriteError,
    },
    /// The run-wide lock failed for a reason that is not the Source's two
    /// skip conditions.
    #[error("the charter synthesis lock could not be acquired: {0}")]
    Lock(String),
}

/// Bring the manifest's `bundle_content_hash` in line with the activated charter.
///
/// `acquire_lock` is the caller's run-wide lock owner (in this workspace
/// `sk_doctrine::acquire_synthesis_lock`); the guard it returns is held for the
/// whole read-modify-write and dropped before this function returns. A nested
/// call inside a synthesis run re-enters the same re-entrant guard.
///
/// Returns the typed [`RestampOutcome`]; only a real rewrite is
/// [`RestampOutcome::Restamped`]. A lock the caller could not take is
/// [`RestampOutcome::Skipped`] with the Source's reason, never an abort.
///
/// # Errors
///
/// [`RestampError::Manifest`] when the present manifest cannot be read or
/// validated, [`RestampError::Write`] when the rewritten bytes cannot be
/// published, and [`RestampError::Lock`] for a non-lock acquire failure.
pub fn restamp_bundle_hash<G>(
    repo_root: &Path,
    acquire_lock: impl FnOnce(&Path) -> Result<G, RestampLockFailure>,
) -> Result<RestampOutcome, RestampError> {
    let manifest_path = repo_root.join(SYNTHESIS_MANIFEST_PATH);
    if !manifest_path.is_file() {
        return Ok(RestampOutcome::NoManifest);
    }
    // The read-modify-write is one unit under the run-wide synthesis lock
    // (re-entrant, so the call from inside a synthesize run nests).
    let guard = match acquire_lock(repo_root) {
        Ok(guard) => guard,
        Err(RestampLockFailure::Other { detail }) => return Err(RestampError::Lock(detail)),
        Err(failure) => return Ok(RestampOutcome::Skipped(failure)),
    };
    let outcome = restamp_locked(repo_root, &manifest_path);
    drop(guard);
    outcome
}

fn restamp_locked(repo_root: &Path, manifest_path: &Path) -> Result<RestampOutcome, RestampError> {
    let manifest =
        SynthesisManifest::load(manifest_path).map_err(|source| RestampError::Manifest {
            path: manifest_path.to_path_buf(),
            source,
        })?;
    let Some(previous) = manifest.bundle_content_hash.clone() else {
        return Ok(if manifest.built_in_only {
            RestampOutcome::BuiltInOnly
        } else {
            RestampOutcome::Unstamped
        });
    };
    if manifest.built_in_only {
        return Ok(RestampOutcome::BuiltInOnly);
    }
    let Some(current) = compute_bundle_content_hash(repo_root) else {
        return Ok(RestampOutcome::Unhashable);
    };
    if current == previous {
        return Ok(RestampOutcome::AlreadyCurrent {
            bundle_content_hash: current,
        });
    }
    let mut updated = manifest;
    updated.bundle_content_hash = Some(current.clone());
    let finalized = updated.finalize();
    let bytes = finalized.to_canonical_bytes();
    sk_kernel::source_writer::write_source(
        sk_kernel::source_writer::WriterOperation::TextTruncate,
        manifest_path,
        sk_kernel::source_writer::SourcePayload::Bytes(&bytes),
        None,
    )
    .map_err(|source| RestampError::Write {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    Ok(RestampOutcome::Restamped {
        bundle_content_hash: current,
        previous_bundle_content_hash: previous,
    })
}

/// The manifest document a restamp would produce for `manifest`, as canonical
/// YAML — the exact bytes [`restamp_bundle_hash`] publishes.
///
/// Exposed so a caller (and a test) can compare the pre-image with the
/// post-image without a second serializer; it performs no IO.
#[must_use]
pub fn restamped_manifest_bytes(
    manifest: &SynthesisManifest,
    bundle_content_hash: &str,
) -> Vec<u8> {
    let mut updated = manifest.clone();
    updated.bundle_content_hash = Some(bundle_content_hash.to_owned());
    updated.finalize().to_canonical_bytes()
}

/// Parse a manifest document exactly like the restamp does (the one model).
///
/// # Errors
///
/// The model's own refusal for a malformed document.
pub fn parse_manifest_document(text: &str) -> Result<SynthesisManifest, RestampError> {
    let value: Value = serde_yaml_ng::from_str(text).map_err(|error| RestampError::Manifest {
        path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
        source: CharterError::UnsupportedNode {
            path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
            detail: error.to_string(),
        },
    })?;
    SynthesisManifest::from_value(&value).map_err(|source| RestampError::Manifest {
        path: PathBuf::from(SYNTHESIS_MANIFEST_PATH),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(directory: &Path, extra: &str) -> PathBuf {
        let path = directory
            .join(".kittify")
            .join("charter")
            .join("synthesis-manifest.yaml");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(
            &path,
            format!(
                "created_at: '2026-01-01T00:00:00+00:00'\n\
                 run_id: '01J00000000000000000000000'\n\
                 adapter_id: a\n\
                 adapter_version: '1'\n\
                 synthesizer_version: 4.0.0\n\
                 manifest_hash: '{}'\n\
                 artifacts: []\n{extra}",
                "0".repeat(64)
            ),
        )
        .expect("manifest");
        path
    }

    #[test]
    fn a_missing_manifest_is_a_no_op_before_any_lock() {
        let directory = tempfile::tempdir().expect("temp");
        let outcome =
            restamp_bundle_hash(directory.path(), |_| -> Result<(), RestampLockFailure> {
                panic!("no manifest: the lock must not be taken")
            })
            .expect("no-op");
        assert_eq!(outcome, RestampOutcome::NoManifest);
        assert!(!outcome.is_restamped());
        assert_eq!(outcome.warning_line(), None);
    }

    #[test]
    fn a_skip_carries_the_source_warning_line() {
        let held = RestampLockFailure::Held {
            lock_path: PathBuf::from("/tmp/x/charter-synthesis.lock"),
        };
        let line = RestampOutcome::Skipped(held)
            .warning_line()
            .expect("warning");
        assert_eq!(
            line,
            "Warning: the change was saved, but the synthesis manifest's bundle hash was not refreshed: \
             a `charter synthesize`/`resynthesize` run in this worktree holds /tmp/x/charter-synthesis.lock. \
             The next activation change or synthesis run will restamp it."
        );
        let line = RestampOutcome::Skipped(RestampLockFailure::Unavailable)
            .warning_line()
            .expect("warning");
        assert_eq!(
            line,
            "Warning: the change was saved, but the synthesis manifest's bundle hash was not refreshed: \
             git could not be invoked to place the synthesis lock. \
             The next activation change or synthesis run will restamp it."
        );
    }

    #[test]
    fn a_non_lock_acquire_failure_is_propagated_not_reported_as_a_skip() {
        let directory = tempfile::tempdir().expect("temp");
        seeded(directory.path(), "bundle_content_hash: sha256:stale\n");
        let error = restamp_bundle_hash(directory.path(), |_| -> Result<(), RestampLockFailure> {
            Err(RestampLockFailure::Other {
                detail: "disk on fire".to_owned(),
            })
        })
        .expect_err("propagated");
        assert!(matches!(error, RestampError::Lock(_)), "{error}");
    }
}

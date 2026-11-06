//! CF34 — the canonical synthesis-manifest bundle-hash re-stamp.
//!
//! Mirrors the pinned reference's `charter/activation/bundle_restamp.py` against
//! fresh temporary projects: the exact no-op cases, the two lock skips with the
//! reference's warning line, the nested call inside an already-held lock, an
//! independent sibling namespace, and the post-image (only the stamp and the
//! derived self-hash change value).
//!
//! The run-wide lock is the caller's (`sk_doctrine::acquire_synthesis_lock` in
//! this workspace); these tests supply a plain kernel file lock on the same
//! path family, so the re-entrancy and sibling behaviour under test is the
//! primitive's real behaviour, not a stub.
//!
//! Protocol note: every fixture here is a fresh `tempfile` project, never the
//! repository. Deep proof still owed (recorded in the lane report): a
//! differential two-process run against the pinned Python restamp, and Windows
//! lock behaviour.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_charter::bundle_restamp::{
    RestampLockFailure, RestampOutcome, parse_manifest_document, restamp_bundle_hash,
    restamped_manifest_bytes,
};
use sk_charter::{compute_bundle_content_hash, synthesis::SynthesisManifest};
use sk_kernel::FileLock;

const CHARTER_REL: &str = ".kittify/charter/charter.yaml";
const MANIFEST_REL: &str = ".kittify/charter/synthesis-manifest.yaml";
const LOCK_REL: &str = ".kittify/runtime/locks/charter-synthesis.lock";

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture file");
}

/// A project with the activated charter and a seeded manifest.
fn project(extra: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("fixture root");
    let root = directory.path();
    write(&root.join(CHARTER_REL), "catalog: {}\n");
    let manifest = format!(
        "created_at: '2026-01-01T00:00:00+00:00'\n\
         run_id: '01J00000000000000000000000'\n\
         adapter_id: a\n\
         adapter_version: '1'\n\
         synthesizer_version: 4.0.0\n\
         manifest_hash: '{}'\n\
         artifacts: []\n{extra}",
        "0".repeat(64)
    );
    write(&root.join(MANIFEST_REL), &manifest);
    directory
}

/// The caller's lock: the real kernel lock on the reference's non-git path.
fn acquire(root: &Path) -> Result<FileLock, RestampLockFailure> {
    FileLock::acquire(&root.join(LOCK_REL), Duration::from_secs(10)).map_err(|error| {
        RestampLockFailure::Other {
            detail: error.to_string(),
        }
    })
}

fn manifest_text(root: &Path) -> String {
    fs::read_to_string(root.join(MANIFEST_REL)).expect("the manifest")
}

#[test]
fn the_restamp_changes_only_the_stamp_and_the_derived_self_hash() {
    let fixture = project("bundle_content_hash: sha256:stale\n");
    let root = fixture.path();
    let before_text = manifest_text(root);
    let before = parse_manifest_document(&before_text).expect("the seeded manifest");
    let current = compute_bundle_content_hash(root).expect("a hashable bundle");

    let outcome = restamp_bundle_hash(root, acquire).expect("the restamp");
    assert_eq!(
        outcome,
        RestampOutcome::Restamped {
            bundle_content_hash: current.clone(),
            previous_bundle_content_hash: "sha256:stale".to_owned(),
        }
    );
    assert!(outcome.is_restamped());
    assert_eq!(outcome.warning_line(), None, "no skip, no warning");

    let after = parse_manifest_document(&manifest_text(root)).expect("the restamped manifest");
    assert_eq!(after.bundle_content_hash.as_deref(), Some(current.as_str()));
    assert!(
        after.verify_manifest_hash().is_ok(),
        "the derived manifest_hash verifies"
    );
    // Everything the model carries survives; the canonical serialization also
    // materialises the model's own defaults (measured against the reference).
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(after.run_id, before.run_id);
    assert_eq!(after.adapter_id, before.adapter_id);
    assert_eq!(after.adapter_version, before.adapter_version);
    assert_eq!(after.synthesizer_version, before.synthesizer_version);
    assert_eq!(after.artifacts, before.artifacts);
    assert_eq!(after.schema_version, "3");
    assert!(!after.built_in_only);
    assert_eq!(after.mission_id, None);
    // The bytes are exactly the canonical post-image the helper renders.
    assert_eq!(
        manifest_text(root).into_bytes(),
        restamped_manifest_bytes(&before, &current)
    );

    let again = restamp_bundle_hash(root, acquire).expect("the second restamp");
    assert_eq!(
        again,
        RestampOutcome::AlreadyCurrent {
            bundle_content_hash: current
        }
    );
    assert!(
        !again.is_restamped(),
        "a matching stamp is a no-op, never a rewrite"
    );
}

#[test]
fn every_no_op_case_leaves_the_manifest_bytes_untouched() {
    let no_manifest = tempfile::tempdir().expect("fixture root");
    write(&no_manifest.path().join(CHARTER_REL), "catalog: {}\n");
    let outcome = restamp_bundle_hash(no_manifest.path(), |_| -> Result<(), RestampLockFailure> {
        panic!("a missing manifest must not take the lock")
    })
    .expect("no manifest");
    assert_eq!(outcome, RestampOutcome::NoManifest);

    for (extra, expected) in [
        (
            "built_in_only: true\nbundle_content_hash: sha256:stale\n",
            RestampOutcome::BuiltInOnly,
        ),
        ("", RestampOutcome::Unstamped),
    ] {
        let fixture = project(extra);
        let before = manifest_text(fixture.path());
        let outcome = restamp_bundle_hash(fixture.path(), acquire).expect("a skip");
        assert_eq!(outcome, expected);
        assert!(!outcome.is_restamped());
        assert_eq!(manifest_text(fixture.path()), before);
    }

    let fixture = project("bundle_content_hash: sha256:stale\n");
    let before = manifest_text(fixture.path());
    fs::remove_file(fixture.path().join(CHARTER_REL)).expect("drop the charter");
    let outcome = restamp_bundle_hash(fixture.path(), acquire).expect("unhashable");
    assert_eq!(outcome, RestampOutcome::Unhashable);
    assert_eq!(manifest_text(fixture.path()), before);
}

#[test]
fn a_lock_skip_is_one_reference_warning_line_and_writes_nothing() {
    let fixture = project("bundle_content_hash: sha256:stale\n");
    let root = fixture.path();
    let before = manifest_text(root);

    let held_path = root.join(LOCK_REL);
    let outcome = restamp_bundle_hash(root, |_| -> Result<(), RestampLockFailure> {
        Err(RestampLockFailure::Held {
            lock_path: held_path.clone(),
        })
    })
    .expect("held");
    assert_eq!(manifest_text(root), before);
    assert_eq!(
        outcome.warning_line().expect("warning"),
        format!(
            "Warning: the change was saved, but the synthesis manifest's bundle hash was not refreshed: \
             a `charter synthesize`/`resynthesize` run in this worktree holds {}. \
             The next activation change or synthesis run will restamp it.",
            held_path.display()
        )
    );

    let outcome = restamp_bundle_hash(root, |_| -> Result<(), RestampLockFailure> {
        Err(RestampLockFailure::Unavailable)
    })
    .expect("unavailable");
    assert_eq!(manifest_text(root), before);
    assert_eq!(
        outcome.warning_line().expect("warning"),
        "Warning: the change was saved, but the synthesis manifest's bundle hash was not refreshed: \
         git could not be invoked to place the synthesis lock. \
         The next activation change or synthesis run will restamp it."
    );

    // The change is still absent: a later run with a free lock restamps.
    let outcome = restamp_bundle_hash(root, acquire).expect("restamp");
    assert!(outcome.is_restamped());
    assert_ne!(manifest_text(root), before);
}

#[test]
fn a_nested_call_reenters_the_lock_a_synthesis_run_already_holds() {
    let fixture = project("bundle_content_hash: sha256:stale\n");
    let root = fixture.path();
    let guard = acquire(root).expect("hold the run-wide lock");
    let outcome = restamp_bundle_hash(root, acquire).expect("nested restamp");
    assert!(
        outcome.is_restamped(),
        "a nested call inside a synthesis run still restamps"
    );
    drop(guard);
}

#[test]
fn a_sibling_project_holds_its_own_lock_namespace() {
    let holder = project("bundle_content_hash: sha256:stale\n");
    let sibling = project("bundle_content_hash: sha256:stale\n");
    assert_ne!(
        holder.path().join(LOCK_REL),
        sibling.path().join(LOCK_REL),
        "the run-wide lock is per worktree, not per machine"
    );
    let guard = acquire(holder.path()).expect("hold the sibling's neighbour");
    let outcome = restamp_bundle_hash(sibling.path(), acquire).expect("independent restamp");
    assert!(
        outcome.is_restamped(),
        "another worktree's lock never blocks this one"
    );
    drop(guard);
}

#[test]
fn an_unreadable_manifest_is_a_typed_failure_not_a_no_op() {
    let fixture = project("bundle_content_hash: sha256:stale\nextra_field: 1\n");
    let before = manifest_text(fixture.path());
    let error = restamp_bundle_hash(fixture.path(), acquire).expect_err("refused");
    assert!(error.to_string().contains("cannot be read"), "{error}");
    assert_eq!(
        manifest_text(fixture.path()),
        before,
        "a refused restamp writes nothing"
    );
}

/// The manifest type is the one the restamp composes; a hand-edited model change
/// must never smuggle a second serializer into this module.
#[test]
fn the_restamp_uses_the_canonical_manifest_model() {
    let fixture = project("bundle_content_hash: sha256:stale\n");
    let root = fixture.path();
    restamp_bundle_hash(root, acquire).expect("restamp");
    let text = manifest_text(root);
    let manifest = SynthesisManifest::load(&root.join(MANIFEST_REL)).expect("loads");
    assert_eq!(text.into_bytes(), manifest.to_canonical_bytes());
    let expected: PathBuf = root.join(MANIFEST_REL);
    assert!(expected.is_file());
}

//! Source `charter.activation.pack_context.load_denied`: the canonical read-only
//! effective-deny reader.
//!
//! `load_denied` is the one place a reader's effective deny
//! `D*(k) = resolve_deny(D(k)) − R(k)` is computed for every kind that actually
//! carries a deny list. It is the reader seam, not the activation writer, not
//! the inspection `ActivationState`, and not a second ledger/deny model:
//!
//! * the ledger is the existing typed [`ActivationLedger`] (source resolution,
//!   legacy projection, L1/L2 validation and the unknown-deny-key carriage are
//!   all its own);
//! * the fast path is the existing [`ledger_may_hold_deny`] key probe, so a
//!   legacy surface with no `deactivated_<kind>` key never pays a typed load,
//!   an identity index or a required-floor read (NFR-003);
//! * the required floor is the canonical **strict raw** token reader, i.e. the
//!   read-path spelling of the floor, not the coercing selection overlay and not
//!   the writer's typed-provenance fold;
//! * the identity index is the existing layered [`IdentityIndex`], built once
//!   and only when at least one kind is pending;
//! * the deny algebra and membership are the existing [`effective_deny`] /
//!   [`DenySet`] authority;
//! * the result is sorted by the **plural spelling**, never by enum order or a
//!   map accident, and only the non-empty sets are returned.
//!
//! Source phases preserved here: a supplied ledger bypasses the probe, the
//! source read and the stray-key reporting; an explicit `kinds` filter is
//! applied before the "is anything pending?" test (so an empty filter means no
//! identity and no floor); `org_roots: Some([])` is distinct from `None`
//! (supplied roots are indexed as given, while the strict floor read keeps its
//! own `repo_root` semantics).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::identity::{IdentityFailure, IdentityIndex};
use crate::ledger::{
    ActivationReadError, ExactActivationLedger, ExactDenySet, activatable_kinds,
    effective_deny_exact, ledger_may_hold_deny, load_activation_ledger_exact,
};

/// Options of [`load_denied`]: the reference's keyword arguments, in its shape.
///
/// The three-state distinctions are load-bearing and mirror Source:
///
/// * `ledger: None` runs the [`ledger_may_hold_deny`] fast path and reads the
///   source, delivering the reader's own Source warnings (FR-020 stray keys, and
///   the legacy deny report) as it reads; `Some(ledger)` is used as given — no
///   probe, no read, no reporting, so a caller that already read the ledger does
///   not warn twice or fabricate a read.
/// * `kinds: None` resolves every kind; `Some(&[])` resolves none (no identity,
///   no floor), which is not the same as "no deny".
/// * `org_roots: None` takes the canonical configured org-pack chain;
///   `Some(&[])` indexes the supplied (empty) chain while the strict floor read
///   still answers from `repo_root`.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadDeniedOptions<'a> {
    /// An already-read activation ledger, exactly.
    pub ledger: Option<&'a ExactActivationLedger>,
    /// Supplied org pack roots for the identity index (not the floor read).
    pub org_roots: Option<&'a [PathBuf]>,
    /// The kinds to resolve; `None` means every kind.
    pub kinds: Option<&'a [ArtifactKind]>,
}

/// `D*(k)` per plural kind, sorted by plural, for the kinds that deny anything.
///
/// # Errors
///
/// The ledger read (malformed/unknown keys, bad values, a runtime construction
/// abort), the key probe, the strict org-required floor read of a pending kind,
/// the identity root resolution, or the identity scan abort of a pending kind.
/// An unreadable floor of a denied kind refuses; an unrelated malformed required
/// field is never read, and an abort never becomes an unresolved healthy key.
pub fn load_denied(
    repo_root: &Path,
    options: &LoadDeniedOptions<'_>,
) -> Result<Vec<(String, ExactDenySet)>, ActivationReadError> {
    let owned;
    let ledger = match options.ledger {
        Some(ledger) => ledger,
        None => {
            if !ledger_may_hold_deny(repo_root)? {
                return Ok(Vec::new());
            }
            owned = load_activation_ledger_exact(repo_root)?;
            &owned
        }
    };
    // The ledger keeps every activatable kind; Source's pending list is only the
    // kinds that actually carry a deny entry, in the ledger's (enum) order.
    let pending: Vec<ArtifactKind> = activatable_kinds()
        .into_iter()
        .filter(|kind| options.kinds.is_none_or(|kinds| kinds.contains(kind)))
        .filter(|kind| !ledger.deny_for(*kind).is_empty())
        .collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    // Source builds its own identity here. Reusing an earlier caller's index
    // changes roots, snapshots and failure phases, even if it seems cheaper.
    let (built_in, org_roots, project) = identity_roots(repo_root, options.org_roots)?;
    let identity = IdentityIndex::from_roots(&built_in, &org_roots, project.as_deref());
    let mut denied: Vec<(String, ExactDenySet)> = Vec::new();
    for kind in pending {
        // The exact raw floor: a Source-admitted lone surrogate in
        // `required_<kind>` must reach the algebra, not be filtered away.
        let floor = crate::org_charter::read_org_required_tokens_exact(repo_root, kind, true)?;
        let deny = effective_deny_exact(
            kind,
            ledger.deny_for(kind).iter().cloned(),
            floor,
            &identity,
        )
        .map_err(|failure| match failure {
            IdentityFailure::Constructor(abort) => ActivationReadError::Aborted {
                phase: format!("identity scan of {}", kind.plural()),
                path: repo_root.to_path_buf(),
                python: abort.python.to_owned(),
                message: abort.message,
            },
            IdentityFailure::Native(native) => ActivationReadError::Native(native),
        })?;
        if !deny.is_empty() {
            denied.push((kind.plural().to_owned(), deny));
        }
    }
    denied.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(denied)
}

/// Map the reader's plural-keyed result onto artifact kinds.
///
/// Every plural the reader can emit is an [`ArtifactKind::plural`], so a failure
/// is a real taxonomy mismatch and stays a typed refusal.
pub(crate) fn deny_by_kind(
    denied: &[(String, ExactDenySet)],
) -> Result<BTreeMap<ArtifactKind, ExactDenySet>, ActivationReadError> {
    let mut by_kind = BTreeMap::new();
    for (plural, deny) in denied {
        by_kind.insert(
            ArtifactKind::from_plural(plural)
                .map_err(|error| ActivationReadError::Doctrine(error.into()))?,
            deny.clone(),
        );
    }
    Ok(by_kind)
}

/// The identity layers: the configured chain, or the supplied org roots.
///
/// The built-in root and the project overlay are resolved through the canonical
/// catalogue-root authority either way; supplied org roots replace the
/// configured chain and, like Source's `IdentityIndex.build`, only existing
/// directories are indexed.
fn identity_roots(
    repo_root: &Path,
    supplied: Option<&[PathBuf]>,
) -> Result<(PathBuf, Vec<PathBuf>, Option<PathBuf>), DoctrineError> {
    match supplied {
        Some(org_roots) => Ok((
            crate::language_vocabulary::built_in_root(Some(repo_root), None)?,
            org_roots
                .iter()
                .filter(|root| root.is_dir())
                .cloned()
                .collect(),
            crate::language_vocabulary::existing_project_pack_root(repo_root),
        )),
        None => {
            let roots = crate::language_vocabulary::catalogue_roots(repo_root)?;
            Ok((roots.built_in, roots.org_roots, roots.project))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{ACTIVATION_LEDGER_REL, CONFIG_YAML_REL};
    use std::fs;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    #[test]
    fn empty_kinds_filter_is_not_the_same_as_every_kind() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(ACTIVATION_LEDGER_REL),
            "deactivated_directives: [900-unknown]\n",
        );
        // An explicit empty filter is "nothing pending", answered before any
        // identity or required-floor read.
        assert_eq!(
            load_denied(
                root,
                &LoadDeniedOptions {
                    kinds: Some(&[]),
                    ..LoadDeniedOptions::default()
                }
            )
            .unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn modern_empty_ledger_reads_no_deny() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join(ACTIVATION_LEDGER_REL), "{}\n");
        assert_eq!(
            load_denied(root, &LoadDeniedOptions::default()).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn legacy_allow_only_surface_is_answered_by_the_key_probe() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(CONFIG_YAML_REL),
            "activated_directives: [010-specification-fidelity-requirement]\n",
        );
        assert!(!ledger_may_hold_deny(root).unwrap());
        assert_eq!(
            load_denied(root, &LoadDeniedOptions::default()).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn legacy_deny_key_is_probed_reported_but_never_applied() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(
            &root.join(CONFIG_YAML_REL),
            "activated_directives: []\ndeactivated_directives: [900-unknown]\n",
        );
        assert!(ledger_may_hold_deny(root).unwrap());
        assert_eq!(
            load_denied(root, &LoadDeniedOptions::default()).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn malformed_legacy_surface_refuses_instead_of_answering_no_deny() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(&root.join(CONFIG_YAML_REL), "activated_directives: [\n");
        assert!(ledger_may_hold_deny(root).is_err());
        assert!(load_denied(root, &LoadDeniedOptions::default()).is_err());
    }

    #[test]
    fn deny_by_kind_maps_every_plural_spelling() {
        let denied = vec![(
            "directives".to_owned(),
            ExactDenySet {
                stems: [sk_kernel::PythonString::from_scalars("900-org-only")].into(),
                ..ExactDenySet::empty()
            },
        )];
        let by_kind = deny_by_kind(&denied).unwrap();
        assert_eq!(by_kind.len(), 1);
        assert!(by_kind.contains_key(&ArtifactKind::Directive));
    }
}

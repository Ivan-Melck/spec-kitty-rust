//! The typed org **pack context** and its canonical `extends:` chain resolution.
//!
//! Mirrors `charter.activation.pack_context.PackContext`'s pack-root slice and
//! `charter.activation.org_charter._resolve_chain` — the second typed door into
//! `org-charter.yaml`, next to the flat registry-order fold in [`super::fold`]:
//!
//! ```text
//! _build_pack_set(pack_context)      # typed-load each pack_root, key = dir name
//!   -> _resolve_chain(name, pack_set)  # base-first, cycle/dangling == refusal
//!   -> _merge_chain(chain)             # strict schema_version equality
//! _fold_policies(resolved, lenient)    # the cross-pack fold
//! ```
//!
//! ## One authority per fact (nothing is cloned here)
//!
//! * **Roots/registry**: [`OrgPackContext::from_repo`] asks the existing registry
//!   authority ([`super::configured_org_roots`], i.e. `charter_packs.org.packs[]`
//!   through `OrgPackConfig::effective_root`) and the existing built-in anchor
//!   ([`super::built_in_search`]). There is no second config reader.
//! * **Documents/schema**: [`super::load_org_charter_policy`] is the only typed
//!   loader; this module never re-parses an `org-charter.yaml`.
//! * **Topology walk**: [`source::chain_members`] is the crate's ONE `extends:`
//!   walk (the raw reader uses it too); this module reverses its overlay-first
//!   result for the reference's base-first fold order and re-words its failures
//!   as the reference's *typed* exceptions.
//!
//! ## Errors (the reference's typed classes, in its order)
//!
//! A per-pack typed load failure is **skipped** exactly like the reference's
//! `_build_pack_set` (`continue`), except a retired field, which is re-raised;
//! a base that then has no document is a **dangling** chain and is refused, not
//! silently dropped. There are two refusals:
//!
//! * cycle → `OrgCharterCycleError`:
//!   `Cycle detected in extends: chain: A → B → A` (the repeated node is kept so
//!   the operator sees the loop close);
//! * dangling base → `OrgCharterExtensionError`:
//!   `Base pack 'ghost' not found. Chain: child` (the overlay-first chain walked
//!   so far).
//!
//! Both are [`DoctrineError::OrgCharterExtends`], the crate's existing
//! `extends:`-chain variant; the class is carried by the exact message text.
//! The per-chain strict `schema_version` mismatch is the fold's own refusal (see
//! [`super::fold`]).
//!
//! ## Not this module's job
//!
//! Merging. Chain resolution returns names; the one fold body in
//! [`super::fold`] turns them into the resolved policy, so the raw and typed
//! doors cannot disagree about union/replacement semantics.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;

use crate::error::DoctrineError;

use super::source::{ChainFailure, chain_members};

/// A caller-supplied, pre-validated pack context narrowed to what the typed org
/// fold reads: the ordered pack roots.
///
/// The reference's `PackContext` carries the whole activation snapshot; the typed
/// org fold reads exactly one field of it (`pack_roots`: built-in first, then the
/// configured org packs in declaration order) and keys the typed pack set on each
/// root's **directory name**, which is what an `extends:` edge names. Passing this
/// narrower value keeps that contract explicit: the fold can never start reading
/// activation state it does not own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgPackContext {
    pack_roots: Vec<PathBuf>,
}

impl OrgPackContext {
    /// A context over the given ordered pack roots (the caller's own snapshot).
    ///
    /// Order matters: it is the reference's fold order, so the LAST root wins a
    /// colliding `interview_defaults` key and a later root's `org_name` wins.
    pub fn from_roots<I>(pack_roots: I) -> Self
    where
        I: IntoIterator<Item = PathBuf>,
    {
        Self {
            pack_roots: pack_roots.into_iter().collect(),
        }
    }

    /// Resolve the context the way the reference's `PackContext.from_config` does
    /// for the typed org door: the built-in pack anchor first, then the declared
    /// org packs in `charter_packs.org.packs[]` order.
    ///
    /// The built-in anchor is the packaged resource root (the reference's
    /// `charter/offering` package directory); it ships no `org-charter.yaml`, so
    /// it contributes nothing, but it stays in the list so a pack that declares
    /// `extends:` naming it resolves instead of dangling. When the built-in pack
    /// cannot be found at all the anchor is omitted rather than failing the whole
    /// read — the org tier is registry-born and must never be erased by an
    /// unavailable built-in tier.
    ///
    /// A declared org pack that cannot be resolved is a refusal: the reference's
    /// `_read_org_packs` re-raises the subdir-escape/unset-env-var errors and the
    /// registry read is fail-closed on this path.
    pub fn from_repo(repo_root: &Path) -> Result<Self, DoctrineError> {
        let mut pack_roots = Vec::new();
        if let Ok(built_in) = super::built_in_search(repo_root).find_root() {
            pack_roots.push(built_in);
        }
        for (_, root) in super::configured_org_roots(repo_root)? {
            pack_roots.push(root);
        }
        Ok(Self { pack_roots })
    }

    /// The ordered pack roots this context folds over.
    pub fn pack_roots(&self) -> &[PathBuf] {
        &self.pack_roots
    }

    /// Whether the context has no roots at all (the reference's
    /// `pack_roots=()`: an empty typed policy, never an error).
    pub fn is_empty(&self) -> bool {
        self.pack_roots.is_empty()
    }
}

/// The typed pack set's key for one pack root: its **directory name**, exactly as
/// an `extends:` edge names it.
///
/// `None` when the final path component has no scalar spelling. The reference
/// would carry such a directory name as a `str` with surrogate escapes; this
/// crate has no lossless `OsStr`-to-`PythonString` path on this door, so the root
/// is excluded from the typed pack set instead of being keyed by a lossy or
/// invented name (see the module report's disclosed limitation).
pub(crate) fn pack_key(root: &Path) -> Option<String> {
    root.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
}

/// Resolve `start`'s `extends:` chain, **base first** (the reference's
/// `resolve_extends_order` order: `[root_base, ..., overlay]`).
///
/// The whole topology is validated before any order is returned, so a caller
/// never folds a half-resolved chain. `start_path` is only the document the
/// Native refusal names; the reference's exceptions carry no path.
pub(crate) fn chain_base_first(
    start: &str,
    edges: &BTreeMap<String, Option<PythonString>>,
    start_path: &Path,
) -> Result<Vec<String>, DoctrineError> {
    match chain_members(start, edges) {
        Ok(mut overlay_first) => {
            overlay_first.reverse();
            Ok(overlay_first)
        }
        Err(failure) => Err(DoctrineError::OrgCharterExtends {
            path: start_path.to_path_buf(),
            message: typed_chain_message(&failure),
        }),
    }
}

/// The reference's typed chain wording for a walk failure.
fn typed_chain_message(failure: &ChainFailure) -> String {
    match failure {
        ChainFailure::MissingBase { missing, chain } => {
            // `f"Base pack '{missing_pack}' not found. Chain: {' → '.join(chain)}"`
            let missing = match missing.to_scalar_string() {
                Ok(name) => name,
                // A base name with no scalar spelling (a lone surrogate) cannot be
                // spliced into a `String`; the canonical Python repr keeps it exact
                // and unambiguous instead of inventing or dropping the name.
                Err(_) => missing.repr(),
            };
            format!(
                "Base pack '{missing}' not found. Chain: {}",
                chain.join(" → ")
            )
        }
        ChainFailure::Cycle { cycle } => {
            format!("Cycle detected in extends: chain: {}", cycle.join(" → "))
        }
    }
}

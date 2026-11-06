//! The ONE `catalog.references` catalogue reader (Source `_load_references`).
//!
//! The reference's `charter context` bootstrap rail counts and names its
//! `Reference Docs:` pointers from the **derived catalogue projection** inside
//! the authoritative `charter.yaml` (`catalog.references`) — not from the
//! delivered action grain, not from a `references.yaml` sibling file, and not
//! from the set of pointers that happen to resolve on disk:
//!
//! ```python
//! charter_yaml_path = canonical_root / CHARTER_YAML
//! if not charter_yaml_path.exists():
//!     return []
//! try:
//!     document = load_charter_yaml(charter_yaml_path)
//! except (YAMLError, UnicodeDecodeError, OSError):
//!     return []
//! catalog = document.get("catalog") if isinstance(document, dict) else None
//! raw_references = catalog.get("references") if isinstance(catalog, dict) else []
//! if not isinstance(raw_references, list):
//!     return []
//! denied = _denied_by_kind(canonical_root)          # raises (fail closed)
//! for item in raw_references:
//!     if not isinstance(item, dict) or not _reference_admitted(item, denied):
//!         continue
//!     refs.append({"id": str(item.get("id", "")), ...})
//! ```
//!
//! So this module owns exactly three decisions and no others:
//!
//! * **presence/parse policy** — Source catches read/decode/YAML errors, not
//!   unignored existence errors or constructor runtime failures. Wrong-shaped
//!   catalogues are empty; deny failures remain outside the read catch;
//! * **the deny filter** — [`reference_admitted`]'s per-kind effective deny,
//!   composed by [`denied_by_kind`], which the `--include` annotation path
//!   shares so there is one deny composition in the crate;
//! * **exact field text** — `str(item.get(field, ""))` for the five catalogue
//!   fields, through the canonical object graph's Python `str` authority.
//!
//! Everything else is reused: the canonical YAML object-graph constructor, the
//! activation ledger, the caller's identity index, the org-required floor and
//! the effective-deny algebra. There is no second YAML parser, deny resolver,
//! identity index or pointer resolver here, and no writer at all.

use std::path::Path;

use sk_charter::ArtifactKind;
use sk_kernel::{
    PythonString,
    python_yaml::{self, Dialect, ErrorClass, GraphDocument, NodeRef, YamlError},
};

/// Failures outside Source's catalogue read/decode/YAML catch.
#[derive(Debug, thiserror::Error)]
pub enum ReferenceCatalogError {
    /// Source's existence probe precedes its read catch.
    #[error("could not inspect reference catalogue {path}: {source}")]
    Exists {
        /// Authoritative charter path.
        path: std::path::PathBuf,
        /// Actual filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// Retain constructor runtime, transport and process causes without turning
    /// them into an empty catalogue or fabricating a Source exception class.
    #[error("could not construct reference catalogue {path}: {source}")]
    Construct {
        /// Authoritative charter path.
        path: std::path::PathBuf,
        /// ONE canonical constructor's typed cause.
        #[source]
        source: Box<YamlError>,
    },
    /// Deny loading is outside the read catch; the canonical reader's typed
    /// refusal (including an identity scan abort) is carried unchanged.
    #[error(transparent)]
    Deny(Box<crate::ledger::ActivationReadError>),
}

use crate::identity::IdentityIndex;
use crate::ledger::{ExactDenySet, resolve_deny};

/// One `catalog.references` entry, as the reference's `str()` projection spells
/// its five fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceEntry {
    /// `str(item.get("id", ""))` — the catalogue id (`directive:DIRECTIVE_001`).
    pub id: PythonString,
    /// `str(item.get("title", ""))`.
    pub title: PythonString,
    /// `str(item.get("local_path", ""))` — the pack-relative document path.
    pub local_path: PythonString,
    /// `str(item.get("kind", ""))` — the singular artifact kind.
    pub kind: PythonString,
    /// `str(item.get("summary", ""))`.
    pub summary: PythonString,
}

/// Source `_load_references`: the deny-admitted `catalog.references` entries.
///
/// `canonical_root` is the canonical read root the caller already resolved (the
/// reference reads `charter.yaml` there, never at the repository root), and
/// `identity` is that caller's already-built identity index — the same index the
/// deny spellings resolve through, so no second pack scan happens here.
///
/// # Errors
///
/// An unignored existence error, constructor runtime/operational failure, or
/// deny/floor failure. Source's caught read/decode/YAML cases remain empty.
pub fn load_reference_catalog(
    canonical_root: &Path,
    _identity: &IdentityIndex,
) -> Result<Vec<ReferenceEntry>, ReferenceCatalogError> {
    let Some(document) = read_catalog_document(canonical_root)? else {
        return Ok(Vec::new());
    };
    let Some(raw_references) = document
        .root()
        .lookup("catalog")
        .and_then(|catalog| catalog.lookup("references"))
        .and_then(NodeRef::as_sequence)
    else {
        return Ok(Vec::new());
    };

    let denied = denied_by_kind(canonical_root, None)
        .map_err(|error| ReferenceCatalogError::Deny(Box::new(error)))?;
    let mut entries = Vec::new();
    for &id in raw_references {
        let item = document.view(id);
        if item.as_mapping().is_none() || !reference_admitted(item, &denied) {
            continue;
        }
        entries.push(ReferenceEntry {
            id: python_str(item, "id"),
            title: python_str(item, "title"),
            local_path: python_str(item, "local_path"),
            kind: python_str(item, "kind"),
            summary: python_str(item, "summary"),
        });
    }
    Ok(entries)
}

/// Source load_charter_yaml opens UTF8 text, not a byte stream with UTF16 BOM
/// autodetection. Its existence probe is outside the caught read/load block.
fn read_catalog_document(
    canonical_root: &Path,
) -> Result<Option<GraphDocument>, ReferenceCatalogError> {
    let path = canonical_root.join(sk_charter::CHARTER_YAML);
    if !sk_kernel::paths::source_path_exists(&path).map_err(|source| {
        ReferenceCatalogError::Exists {
            path: path.clone(),
            source,
        }
    })? {
        return Ok(None);
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let Ok(text) = sk_kernel::python_utf8::decode_utf8(&bytes) else {
        return Ok(None);
    };
    let text = sk_kernel::python_text::universal_newlines(text);
    match python_yaml::decode_graph(&text, Dialect::RuamelSafe12) {
        Ok(graph) => Ok(Some(GraphDocument::new(graph))),
        Err(error) if error.class() == ErrorClass::Yaml => Ok(None),
        Err(source) => Err(ReferenceCatalogError::Construct {
            path,
            source: Box::new(source),
        }),
    }
}

/// `str(item.get(field, ""))` for one catalogue field.
///
/// Project only the requested field. Unrelated cycles remain graph data;
/// strings, bytes, dates and containers use ONE exact Python str authority.
fn python_str(item: NodeRef<'_>, field: &str) -> PythonString {
    item.lookup(field)
        .map(NodeRef::python_str)
        .unwrap_or_default()
}

/// Source `_reference_admitted`: the entry's target survives its kind's deny.
///
/// `denied` is keyed by **singular** kind; an entry whose `kind` names no deny
/// set is admitted. The target is `str(item.get("id", "")).split(":", 1)[-1]`,
/// so a bare id and a `KIND:id` spelling agree.
pub fn reference_admitted(item: NodeRef<'_>, denied: &[(ArtifactKind, ExactDenySet)]) -> bool {
    let kind_text = python_str(item, "kind");
    let Some((_, deny)) = denied
        .iter()
        .find(|(kind, _)| kind_text == PythonString::from_scalars(kind.canonical()))
    else {
        return true;
    };
    let id = python_str(item, "id");
    let start = id
        .codepoints()
        .iter()
        .position(|&code| code == u32::from(':'))
        .map_or(0, |position| position + 1);
    let target = PythonString::from_codepoints(&id.codepoints()[start..])
        .expect("substring of an already validated PythonString");
    deny.admits_exact(&target)
}

/// Source `_denied_by_kind` / `load_denied`: the effective deny per kind, in
/// plural order.
///
/// This is a thin mapping over the ONE canonical source-deny reader
/// ([`crate::source_denied::load_denied`]): the reader owns the ledger read, the
/// key-probe fast path, the single identity build, the exact strict raw floor and
/// the exact deny algebra. This function only maps each emitted plural back to its
/// artifact kind, so it cannot drift into a second floor composition.
///
/// # Errors
///
/// The reader's typed refusal, including the identity scan abort the reference
/// raises.
pub fn denied_by_kind(
    repo_root: &Path,
    kinds: Option<&[ArtifactKind]>,
) -> Result<Vec<(ArtifactKind, ExactDenySet)>, crate::ledger::ActivationReadError> {
    let denied = crate::source_denied::load_denied(
        repo_root,
        &crate::source_denied::LoadDeniedOptions {
            kinds,
            ..crate::source_denied::LoadDeniedOptions::default()
        },
    )?;
    let mut mapped = Vec::with_capacity(denied.len());
    for (plural, deny) in denied {
        mapped.push((
            ArtifactKind::from_plural(&plural)
                .map_err(|error| crate::ledger::ActivationReadError::Doctrine(error.into()))?,
            deny,
        ));
    }
    Ok(mapped)
}

/// The reference `_denied_kind` answer over an already-composed deny map: the
/// first kind (in plural order) whose effective deny holds `identifier`.
#[must_use]
pub fn denied_kind_of(
    identifier: &str,
    denied: &[(ArtifactKind, ExactDenySet)],
    identity: &IdentityIndex,
) -> Option<ArtifactKind> {
    denied.iter().find_map(|(kind, deny)| {
        let spelled = resolve_deny(*kind, [identifier.to_owned()], identity);
        // The identifier is a scalar spelling, so only the exact set's scalar
        // projections can equal it; an exact-only (surrogate) spelling never
        // matches a scalar identifier and is not dropped from the set itself.
        let scalar_spellings: std::collections::BTreeSet<String> = deny
            .spellings()
            .into_iter()
            .filter_map(|token| token.to_scalar_string().ok())
            .collect();
        (!spelled.spellings().is_disjoint(&scalar_spellings)).then_some(*kind)
    })
}

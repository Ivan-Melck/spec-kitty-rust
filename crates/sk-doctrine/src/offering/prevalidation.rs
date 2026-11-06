//! The reference's `BaseArtifactRepository._pre_validate` phase.
//!
//! The reference calls `self._pre_validate(data, yaml_file)` after the
//! non-mapping guard and **before** `model_validate` — and, on an overlay, before
//! `data.get("id")` is even read — inside the same per-file block whose
//! `except (YAMLError, ValidationError, OSError)` covers only those three
//! families. Anything else a hook raises therefore escapes the per-file skip and
//! aborts the whole repository construction; the effective-set pass then reports
//! (or propagates) the abort instead of an absent artifact.
//!
//! `DirectiveRepository._pre_validate` is
//! `directives.validation.reject_directive_inline_refs`, which raises
//! `InlineReferenceRejectedError` (a `ValueError`) for a document that still
//! carries a retired inline-reference field. That is the one hook this module
//! implements: the abort is a Source abort, carried through the crate's existing
//! [`ConstructorAbort`] metadata and [`OfferingLoadError::from_abort`] path, not
//! a new exception hierarchy, class-string parser or fake Native class.
//!
//! Every other kind is an explicit no-op — the reference's own default for a
//! repository without a hook override. The remaining kinds' Source hooks are
//! deliberately not inferred or generalised here: a non-mapping document still
//! warns-and-skips before this phase, and schema/OS/YAML failures are still
//! caught after it, so nothing about malformed input is blanket-fatalised.

use std::path::Path;

use sk_charter::ArtifactKind;
use sk_kernel::python_os_encoding::OsEncodingContext;
use sk_kernel::{PythonString, source_path_text};

use super::value::ArtifactDocument;
use super::{ConstructorAbort, NativeOfferingFailure, OfferingLoadError};

/// The retired inline-reference fields Source rejects at the top level of an
/// artifact document (`charter.offering.shared.errors.FORBIDDEN_TOP_LEVEL_FIELDS`).
///
/// ONE canonical copy of the tuple, in Source's order (the first present field is
/// the one the abort names). The agent-profile repository's own post-validation
/// skip reads this list too, so the two policies cannot drift apart.
pub(crate) const FORBIDDEN_INLINE_REFERENCE_FIELDS: [&str; 3] =
    ["tactic_refs", "paradigm_refs", "applies_to"];

/// Run the reference's `_pre_validate` hook for `kind` over one parsed document.
///
/// Call sites mirror the reference's exactly: the built-in layer runs it after
/// the non-mapping guard and before model admission; an overlay runs it after the
/// non-mapping guard and before its id checks (so an inline reference aborts even
/// when the overlay has no usable id).
///
/// # Errors
///
/// [`OfferingLoadError::from_abort`] carrying the Python exception the
/// reference's per-file catch does not cover (`InlineReferenceRejectedError` for
/// a Directive inline reference), or a typed Native capability failure when
/// the current scalar exception carrier cannot preserve the Source diagnostic.
pub(crate) fn prevalidate(
    kind: ArtifactKind,
    document: &ArtifactDocument,
    path: &Path,
) -> Result<(), OfferingLoadError> {
    match kind {
        ArtifactKind::Directive => reject_directive_inline_refs(document, path),
        _ => Ok(()),
    }
}

/// `directives.validation.reject_directive_inline_refs` over the constructed
/// document.
fn reject_directive_inline_refs(
    document: &ArtifactDocument,
    path: &Path,
) -> Result<(), OfferingLoadError> {
    let Some(mapping) = document.value.as_mapping() else {
        // The loader's non-mapping guard runs before the hook, so a non-mapping
        // document never reaches this phase (and is never fatalised here).
        return Ok(());
    };
    let Some(field) = FORBIDDEN_INLINE_REFERENCE_FIELDS
        .into_iter()
        .find(|field| mapping.get(*field).is_some())
    else {
        return Ok(());
    };
    // `str(data.get("id", "?"))` — Python `str()` of the raw value through the
    // canonical graph authority, so a structured id keeps its Python spelling
    // and an absent one is the reference's placeholder.
    let id = match document.root().lookup("id") {
        Some(node) => document.python_str(node),
        None => PythonString::from_scalars("?"),
    };
    let id = id
        .to_scalar_string()
        .map_err(|codepoint| unextractable(path, "the id value", id.clone(), codepoint))?;
    let exact_path =
        source_path_text(path, &OsEncodingContext::from_environment()).map_err(|cause| {
            OfferingLoadError::from_native(NativeOfferingFailure::DiagnosticPathDecode {
                path: path.to_path_buf(),
                cause,
            })
        })?;
    let path_text = exact_path
        .to_scalar_string()
        .map_err(|codepoint| unextractable(path, "the file path", exact_path.clone(), codepoint))?;
    Err(OfferingLoadError::from_abort(ConstructorAbort {
        python: "InlineReferenceRejectedError",
        message: format!(
            "Inline reference rejected in {path_text}:\n  artifact_kind: directive\n  \
             forbidden_field: {field}\n  migration: Remove {field} from YAML; add edge \
             {{source: directive:{id}, target: <target-kind>:<target-id>, relation: requires}} \
             to packs/built-in/directive.graph.yaml"
        ),
    }))
}

/// A typed Native capability failure for an abort text Rust cannot carry exactly.
///
/// `ConstructorAbort.message` is a Rust `String`, so a file path with undecodable
/// bytes (surrogateescape) or an id carrying a lone surrogate has no exact
/// scalar spelling. The document is still refused — never skipped, never
/// reported as absent — but the reason is a Native capability limit rather than
/// invented Source-class text.
fn unextractable(
    path: &Path,
    part: &'static str,
    exact: PythonString,
    codepoint: u32,
) -> OfferingLoadError {
    OfferingLoadError::from_native(NativeOfferingFailure::DiagnosticProjection {
        path: path.to_path_buf(),
        part,
        exact,
        codepoint,
    })
}

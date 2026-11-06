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
//! Six Source repositories override the hook with the shared inline-reference
//! rejection (`charter.offering.shared.errors`):
//!
//! * `DirectiveRepository` → [`ArtifactKind::Directive`]
//! * `TacticRepository` → [`ArtifactKind::Tactic`]
//! * `ParadigmRepository` → [`ArtifactKind::Paradigm`]
//! * `StyleguideRepository` → [`ArtifactKind::Styleguide`]
//! * `ToolguideRepository` → [`ArtifactKind::Toolguide`]
//! * `ProcedureRepository` → [`ArtifactKind::Procedure`], which additionally
//!   scans each `steps[i]` entry for the step-level fields.
//!
//! Every one of them raises `InlineReferenceRejectedError` (a `ValueError`) for a
//! document that still carries a retired inline-reference field. That abort is a
//! Source abort, carried through the crate's existing [`ConstructorAbort`]
//! metadata and [`OfferingLoadError::from_abort`] path, not a new exception
//! hierarchy, class-string parser or fake Native class.
//!
//! Every other kind is an explicit no-op — the reference's own default for a
//! repository without a hook override (the agent-profile repository has its own,
//! deliberately non-fatal, caught-skip policy; it is NOT generalised here). A
//! non-mapping document still warns-and-skips before this phase, and
//! schema/OS/YAML failures are still caught after it, so nothing about malformed
//! input is blanket-fatalised.
//!
//! # Source ordering of the abort text
//!
//! `reject_inline_refs` computes `str(data.get("id", "?"))` **before** it walks
//! the forbidden-field tuple, so the id spelling is settled even for a document
//! that turns out to carry no forbidden field, and `reject_inline_refs_in_procedure_steps`
//! computes it again before its own scan. This module reproduces that order with
//! the one `str()` authority ([`ArtifactDocument::python_str`]): the value is
//! rendered eagerly, and only its exact scalar projection (which can fail for a
//! lone surrogate) is deferred to the raise path.
//!
//! One Source eager failure is **not** expressible here: CPython's `str()`/
//! `repr()` raises `RecursionError` ("maximum recursion depth exceeded") for a
//! pathologically nested structure (measured at ~900 nested flow sequences),
//! even when no forbidden field is present, and that escapes the per-file catch.
//! The Kernel graph `str` authority returns text instead of a recursion-limit
//! error, so this phase cannot observe that failure. The ASCII RecursionError
//! message itself fits [`ConstructorAbort`]; the missing seam is a typed,
//! caller-appropriate recursion-limit result from the Kernel text authority.
//! This remains a required gap, separate from non-scalar diagnostic rendering.

use std::path::Path;

use sk_charter::ArtifactKind;
use sk_kernel::python_os_encoding::OsEncodingContext;
use sk_kernel::{PythonString, source_path_text};

use super::value::ArtifactDocument;
use super::{ConstructorAbort, NativeOfferingFailure, OfferingLoadError, python_truthy};

/// The retired inline-reference fields Source rejects at the top level of an
/// artifact document (`charter.offering.shared.errors.FORBIDDEN_TOP_LEVEL_FIELDS`).
///
/// ONE canonical copy of the tuple, in Source's order (the first present field is
/// the one the abort names). The agent-profile repository's own post-validation
/// skip reads this list too, so the two policies cannot drift apart.
pub(crate) const FORBIDDEN_INLINE_REFERENCE_FIELDS: [&str; 3] =
    ["tactic_refs", "paradigm_refs", "applies_to"];

/// The retired inline-reference fields Source rejects on a procedure `steps[i]`
/// entry (`charter.offering.shared.errors.FORBIDDEN_STEP_FIELDS`).
///
/// Source order again, and independent of the authored key order: a step carrying
/// both fields names `tactic_refs`.
const FORBIDDEN_PROCEDURE_STEP_FIELDS: [&str; 2] = ["tactic_refs", "paradigm_refs"];

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
/// a retired inline reference), or a typed Native capability failure when the
/// current scalar exception carrier cannot preserve the Source diagnostic.
pub(crate) fn prevalidate(
    kind: ArtifactKind,
    document: &ArtifactDocument,
    path: &Path,
) -> Result<(), OfferingLoadError> {
    if !hooks(kind) {
        return Ok(());
    }
    if document.value.as_mapping().is_none() {
        // The loader's non-mapping guard runs before the hook, so a non-mapping
        // document never reaches this phase (and is never fatalised here).
        return Ok(());
    }
    // `str(data.get("id", "?"))`, eagerly and before the tuple walk.
    let id = artifact_id(document);
    if let Some(field) = FORBIDDEN_INLINE_REFERENCE_FIELDS
        .into_iter()
        .find(|field| document.value.get(*field).is_some())
    {
        return Err(inline_reference_abort(kind, path, field, &id));
    }
    if kind != ArtifactKind::Procedure {
        return Ok(());
    }
    // The separate Source step helper renders the id again before even reading
    // `steps`; do not merge that eager call with the top-level helper's render.
    let id = artifact_id(document);
    // `steps = data.get("steps") or []`: the Python truthiness fallback, then the
    // `isinstance(steps, list)` guard, then the `isinstance(step, dict)` skip.
    let Some(steps) = document
        .value
        .get("steps")
        .filter(|steps| python_truthy(steps))
    else {
        return Ok(());
    };
    let Some(steps) = steps.as_sequence() else {
        return Ok(());
    };
    for step in steps {
        if step.as_mapping().is_none() {
            continue;
        }
        if let Some(field) = FORBIDDEN_PROCEDURE_STEP_FIELDS
            .into_iter()
            .find(|field| step.get(*field).is_some())
        {
            return Err(inline_reference_abort(kind, path, field, &id));
        }
    }
    Ok(())
}

/// Whether the Source repository for `kind` overrides `_pre_validate`.
///
/// The agent-profile repository is deliberately absent: it calls the shared
/// helper itself and **catches** the rejection into a surfaced skip. Treating it
/// as fatal here would change that policy.
const fn hooks(kind: ArtifactKind) -> bool {
    matches!(
        kind,
        ArtifactKind::Directive
            | ArtifactKind::Tactic
            | ArtifactKind::Paradigm
            | ArtifactKind::Styleguide
            | ArtifactKind::Toolguide
            | ArtifactKind::Procedure
    )
}

/// `str(data.get("id", "?"))` through the canonical graph authority.
///
/// A structured id keeps its Python spelling (a container renders its `repr`), an
/// absent one is the reference's placeholder, and an authored `null` is `"None"`.
fn artifact_id(document: &ArtifactDocument) -> PythonString {
    match document.root().lookup("id") {
        Some(node) => document.python_str(node),
        None => PythonString::from_scalars("?"),
    }
}

/// Build the Source `InlineReferenceRejectedError` abort for one document.
fn inline_reference_abort(
    kind: ArtifactKind,
    path: &Path,
    field: &'static str,
    id: &PythonString,
) -> OfferingLoadError {
    let source_kind = kind.canonical();
    let id = id
        .to_scalar_string()
        .map_err(|codepoint| unextractable(path, "the id value", id.clone(), codepoint));
    let id = match id {
        Ok(id) => id,
        Err(error) => return error,
    };
    let exact_path =
        match source_path_text(path, &OsEncodingContext::from_environment()).map_err(|cause| {
            OfferingLoadError::from_native(NativeOfferingFailure::DiagnosticPathDecode {
                path: path.to_path_buf(),
                cause,
            })
        }) {
            Ok(text) => text,
            Err(error) => return error,
        };
    let path_text = match exact_path.to_scalar_string() {
        Ok(text) => text,
        Err(codepoint) => {
            return unextractable(path, "the file path", exact_path.clone(), codepoint);
        }
    };
    // The graph fragment is the one the loader names: `<kind>.graph.yaml` beside
    // the other `packs/built-in/` fragments, from the loader's own suffix.
    let graph_fragment = format!("{source_kind}{}", crate::loader::FRAGMENT_SUFFIX);
    OfferingLoadError::from_abort(ConstructorAbort {
        python: "InlineReferenceRejectedError",
        message: format!(
            "Inline reference rejected in {path_text}:\n  artifact_kind: {source_kind}\n  \
             forbidden_field: {field}\n  migration: Remove {field} from YAML; add edge \
             {{source: {source_kind}:{id}, target: <target-kind>:<target-id>, relation: requires}} \
             to packs/built-in/{graph_fragment}"
        ),
    })
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

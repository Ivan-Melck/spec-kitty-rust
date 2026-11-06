//! `charter context --include <kind>:<id>` — the fetch-deferred governance selectors.
//!
//! The reference surface is
//! `charter.activation.context.build_charter_context_include` plus the renderers in
//! `charter.activation.context_renderers.template_include`
//! (`_render_directive_include`, `_render_tactic_include`,
//! `_render_offering_artifact_include`, `_render_generic_artifact_include`,
//! `_render_agent_profile_include_selector`, `_render_catalog_kind_include_selector`).
//! This module owns the non-`section:` half of that dispatch: `section:` stays with the
//! charter CLI adapter, which renders it from `charter.md`.
//!
//! # One selector grammar, one kind vocabulary
//!
//! The selector is split exactly as the reference splits it — `kind, separator,
//! identifier = selector.partition(":")`, then `kind.strip().lower()` and
//! `identifier.strip()`, with an empty separator, kind or identifier refused as
//! `Expected --include selector in '<kind>:<id>' form.` The kind then goes through the
//! canonical [`ArtifactKind::from_operator_token`] resolver (the single kind authority in
//! `sk-charter`), so `agent-profile` and `agent_profile` agree and an unknown token fails
//! closed with the reference's `Unsupported --include selector kind '…'.` message instead
//! of a private per-surface kind table.
//!
//! # One identity, one merged record, one exact payload
//!
//! An artifact token is resolved to its declared id through the crate's
//! [`IdentityIndex`] (`try_declared_id`: stem, declared id, then the directive id
//! normaliser), and the *record* through the canonical
//! [`CanonicalOfferingService`] ([`CanonicalOfferingService::artifact`]) — the layered
//! repository the reference's `getattr(service, <plural>).get(lookup_id)` reaches.
//! That record is the **merged** one: an org/project overlay replaces the fields it
//! declares and inherits the rest from the built-in/earlier layer.
//!
//! The printed payload is the CF38 exact model dump
//! ([`crate::canonical_artifact_body_exact`]) over that merged record's own canonical
//! graph ([`crate::offering::MergedArtifact::exact_root`]), never the raw authored YAML
//! of the declaring file and never a scalar projection of it. The heading, the inline
//! body ([`crate::context_render::include_body_lines_exact`]) and the `Full artifact:`
//! block ([`crate::context_render::include_full_payload_lines_exact`]) all read that
//! one payload, so an authored code point cannot be spelled one way inline and
//! another way in the payload. The scalar `render_include` entry point is an explicit
//! checked adapter over [`render_include_exact`], not a second route.
//!
//! Identity, record and projection are all **lazy per queried kind**: asking for a
//! `directive` never scans `tactics`, never parses the profile catalogue and never
//! merges the DRG, so a malformed file of an unrelated kind cannot change the answer.
//! The roots, the inferred languages and the activation gate are constructed by
//! [`IncludeAccess`] on first demand, matching the reference's build order (deny
//! annotation first, then the service, then the queried repository).
//!
//! # Effective deny is an annotation, not a refusal
//!
//! Every activatable selector is checked against the effective deny
//! (`_denied_kind` → `load_denied` → `resolve_deny`/`effective_deny`). A hit renders the
//! reference's note
//! `'<kind>:<id> is deactivated in this project (charter activate <restore-kind> <id> restores it)'`
//! instead of the body — the reference annotates, it does not refuse. The
//! `artifact:` probe is gated against every kind, and names the denied kind's
//! hyphenated canonical token as the restore argument.
//!
//! # Known gaps (explicit, not papered over)
//!
//! * `agent-profile` has no [`crate::body`] `Field` schema: its payload is the exact
//!   model-JSON view of the canonical model dump
//!   ([`crate::profile::ProfileBody::model_json_exact`]), the same
//!   `_jsonable_artifact_value(model_dump(...))` projection the reference applies.
//!   [`IncludeError::PendingProjection`] stays only for a record this crate cannot
//!   canonicalise at all (a capability boundary, never "artifact missing").
//! * A payload the exact value model cannot carry faithfully refuses with
//!   [`IncludeError::Payload`], carrying the reference's real cause: `!!binary` bytes
//!   that are not UTF-8 (pydantic's `UnicodeDecodeError`), a mapping key with no UTF-8
//!   form (`UnicodeEncodeError`), an object Python refuses as a mapping key, or a
//!   typed member the model validation refuses. Every *other* non-`str` mapping key
//!   is **not** a refusal: pydantic's JSON codec rewrites it (`1` → `"1"`, `True` →
//!   `"true"`, `None` → `"None"`, a tuple → `"a,b"`), and that one codec lives in the
//!   Kernel ([`sk_kernel::model_json`]). The refusal never claims the artifact is
//!   missing and never invents the Python exception type or traceback.
//! * `directive` is served from the ONE layered repository
//!   ([`CanonicalOfferingService::artifact`]): whole-document admission through the
//!   canonical Directive model, built-in → org → project field merge, and the CF38
//!   model dump for the payload. The old declaring-document fallback (which printed an
//!   org/project shadowed built-in field, and rendered documents the reference model
//!   rejects) is deleted. Two reference behaviours stay unimplemented and are reported
//!   as debt rather than approximated: the repository's `_pre_validate` inline-reference
//!   rejection (`InlineReferenceRejectedError`, an *aborting* ValueError) and the
//!   reference's warning text for a skipped document.
//! * The reference's `ArtifactLayerCollisionWarning` line for a shadowed record is not
//!   reproduced; the merge result itself is (see the paired CF43 proof).
//!
//! # `template:<mission>/<name>`
//!
//! A template selector is NOT an artifact-document lookup: it resolves through the ONE
//! canonical six-tier template resolver in `sk_charter::template_catalog` (the Native
//! owner below this layer), which is also what the CLI's template fast path calls. The
//! rendered text is that owner's `Template <id> (tier: <Source spelling>):` heading plus
//! the template text with trailing newlines stripped. This branch never consults the
//! identity index, an artifact document or a profile repository.

use std::path::Path;

use sk_charter::{ArtifactKind, CharterError};
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

use crate::context_render::{
    include_body_lines_exact_with_steps, include_full_payload_lines_exact,
};
use crate::identity::IdentityIndex;

pub use crate::offering::include_access::{IncludeAccess, IncludeAccessError};

/// The stable machine code of a selector this Native surface cannot render yet.
pub const CONTEXT_INCLUDE_PENDING: &str = "CONTEXT_INCLUDE_PENDING";

/// The reference `ActiveCharterConfigError` machine code, used when the deny
/// annotation's ledger/org-floor read fails.
pub const CONTEXT_INCLUDE_CONFIG_INVALID: &str = "ACTIVE_CHARTER_CONFIG_INVALID";

/// The canonical inputs one include render needs.
///
/// `access` is the lazily constructed authority set
/// ([`IncludeAccess`]): one identity index, the canonical offering service built
/// only when a selector reaches it, the canonical activation gate built only for
/// the two gated kinds, and the effective-deny decision. Keeping a single access
/// value (rather than an eagerly built identity + profile repository) is what
/// makes a `directive` include independent of the profile catalogue and of every
/// unrelated artifact kind.
pub struct IncludeInputs<'a> {
    /// The project root (the directory holding `.kittify/`).
    pub project_root: &'a Path,
    /// The lazily built canonical authorities for this invocation.
    pub access: &'a IncludeAccess,
}

/// One parsed `--include` selector: the normalized kind and the trimmed identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncludeSelector {
    /// The lower-cased, trimmed selector kind (`directive`, `agent-profile`, `artifact`, …).
    pub kind: String,
    /// The trimmed identifier (`<id>`, or `<mission>/<name>` for `template`).
    pub identifier: String,
}

/// Why one `--include` selector could not be rendered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncludeError {
    /// The selector is not `<kind>:<id>`.
    MalformedSelector,
    /// The kind is not an addressable artifact kind.
    UnknownKind {
        /// The normalized kind token, as the reference's message spells it.
        kind: String,
    },
    /// `mission-type` is an activation roster, not an addressable artifact.
    MissionType {
        /// The raw selector.
        selector: String,
    },
    /// The reference's `ValueError` for a selector that resolves to no artifact, or for a
    /// `ValueError` raised while resolving one (a malformed `template:` id, a malformed
    /// org-pack declaration).
    Missing {
        /// The rendered message (already in the reference's wording).
        message: String,
    },
    /// The kind's document scan raised the Python exception the reference does not catch.
    Aborted {
        /// `<Python exception>: <message>`.
        message: String,
    },
    /// The activation-ledger / org-required-floor / pack-config read that decides the
    /// deny annotation failed (the reference's `ActiveCharterConfigError`).
    Config {
        /// The rendered reason.
        message: String,
    },
    /// Constructing the canonical access authorities (pack roots, inferred
    /// languages, the offering service) failed.
    Setup {
        /// The rendered reason.
        message: String,
    },
    /// The reference renders this kind, but this crate has no canonical model dump for it.
    PendingProjection {
        /// The canonical kind.
        kind: ArtifactKind,
    },
    /// The record's exact value has no faithful JSON form.
    ///
    /// This is **not** the reference's not-found path and not a Native capability
    /// gap: the artifact resolution succeeded, and the reference's own model dump
    /// (or its key codec) raises on the same value. `cause` names that real
    /// failure — a non-UTF-8 `!!binary` member (`UnicodeDecodeError`), a mapping
    /// key with no UTF-8 form (`UnicodeEncodeError`), an object Python refuses as
    /// a mapping key, or a typed member the model validation refuses. The message
    /// says so instead of claiming the artifact is missing; it does not invent the
    /// Python exception type or traceback.
    Payload {
        /// The canonical kind.
        kind: ArtifactKind,
        /// The reference's actual failure cause.
        cause: crate::body::PayloadFailure,
    },
    /// The exact text contains code points a UTF-8 byte stream cannot carry (a
    /// lone surrogate), so the scalar `String` view cannot hold it.
    ///
    /// The exact renderer does not lose the value; only this explicit scalar
    /// adapter refuses. `--json` escapes such code points exactly like the
    /// reference's `json.dumps`.
    Unencodable {
        /// The selector the operator asked for.
        selector: String,
    },
}

impl IncludeError {
    /// The stable machine code, when this is a Native capability gap or a reference
    /// `ActiveCharterConfigError` (a plain reference `ValueError` carries no code).
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::PendingProjection { .. } => Some(CONTEXT_INCLUDE_PENDING),
            Self::Config { .. } => Some(CONTEXT_INCLUDE_CONFIG_INVALID),
            Self::MalformedSelector
            | Self::UnknownKind { .. }
            | Self::MissionType { .. }
            | Self::Missing { .. }
            | Self::Aborted { .. }
            | Self::Setup { .. }
            | Self::Payload { .. }
            | Self::Unencodable { .. } => None,
        }
    }

    /// Whether this is a Native capability gap (see [`IncludeError::code`]).
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.code().is_some()
    }
}

impl std::fmt::Display for IncludeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedSelector => {
                formatter.write_str("Expected --include selector in '<kind>:<id>' form.")
            }
            Self::UnknownKind { kind } => {
                write!(formatter, "Unsupported --include selector kind '{kind}'.")
            }
            Self::MissionType { selector } => {
                let spelled = PythonString::from_scalars(selector).repr();
                write!(
                    formatter,
                    "--include does not support the 'mission-type' selector (selector {spelled}); \
                     mission types are not addressable governance artifacts."
                )
            }
            Self::Missing { message } | Self::Aborted { message } => formatter.write_str(message),
            Self::Config { message } | Self::Setup { message } => formatter.write_str(message),
            Self::PendingProjection { kind } => write!(
                formatter,
                "native `charter context --include` cannot render a {} artifact yet: this crate has \
                 no canonical model-dump schema for it, so the reference's `Full artifact:` payload \
                 cannot be produced without printing raw YAML.",
                kind.canonical()
            ),
            Self::Payload { kind, cause } => write!(
                formatter,
                "cannot render the {} `Full artifact:` payload: {cause}. The reference's own \
                 model dump refuses the same record, so this is a real payload-serialization \
                 phase; the artifact itself resolved and this is not a missing selector.",
                kind.canonical()
            ),
            Self::Unencodable { selector } => write!(
                formatter,
                "cannot print the rendered text of --include '{selector}': it contains code points \
                 with no UTF-8 byte encoding (a lone surrogate). The rendered value is complete; \
                 the reference fails at the same output-encoding step. Re-run with --json, which \
                 escapes those code points as \\uXXXX exactly like the reference's json.dumps."
            ),
        }
    }
}

impl std::error::Error for IncludeError {}

/// Split an `--include` selector exactly as the reference does.
///
/// # Errors
///
/// [`IncludeError::MalformedSelector`] when the selector has no separator, no kind or no
/// identifier (after trimming).
pub fn parse_include_selector(selector: &str) -> Result<IncludeSelector, IncludeError> {
    let (head, separator, tail) = match selector.find(':') {
        Some(index) => (&selector[..index], true, &selector[index + 1..]),
        None => (selector, false, ""),
    };
    let trimmed_kind = head.trim_matches(|character: char| {
        sk_kernel::python_text::is_whitespace(u32::from(character))
    });
    let kind = PythonString::from_units(sk_kernel::python_text::lower_codepoints(
        PythonString::from_scalars(trimmed_kind).codepoints(),
    ))
    .expect("pinned lowercase produces valid Python code points")
    .to_scalar_string()
    .expect("lowercase of scalar input remains scalar");
    let identifier = tail
        .trim_matches(|character: char| sk_kernel::python_text::is_whitespace(u32::from(character)))
        .to_owned();
    if !separator || kind.is_empty() || identifier.is_empty() {
        return Err(IncludeError::MalformedSelector);
    }
    Ok(IncludeSelector { kind, identifier })
}

/// Render one non-`section:` `--include` selector as exact Python text.
///
/// `raw` is the selector as the operator spelled it; the reference echoes it verbatim in
/// several messages, so it is not reconstructed from the parsed halves.
///
/// This is the ONE implementation. The returned [`PythonString`] holds the reference
/// text phase's exact code points, so an authored lone surrogate survives to the
/// caller's emit step instead of being lost in a Rust `String`.
///
/// The reference's build order is preserved: a `section:`/`template:` selector never
/// touches a doctrine authority; the effective-deny annotation is decided **before**
/// the offering service (and therefore before the language inference); and the service
/// itself is only built when a renderer needs a repository record.
///
/// # Errors
///
/// [`IncludeError`] — a reference refusal (`Missing`, `UnknownKind`, `MalformedSelector`,
/// `MissionType`, `Aborted`), a reference `ActiveCharterConfigError` (`Config`), a
/// construction failure (`Setup`), a real payload-serialization refusal (`Payload`) or a
/// declared Native capability gap (`PendingProjection`).
pub fn render_include_exact(
    inputs: &IncludeInputs<'_>,
    selector: &IncludeSelector,
    raw: &str,
) -> Result<PythonString, IncludeError> {
    match selector.kind.as_str() {
        "artifact" => {
            // D-21: the best-effort selector is gated too, against every kind's deny.
            if let Some(denied) = inputs
                .access
                .denied_kind(&selector.identifier, None)
                .map_err(access_error)?
            {
                let restore = denied.canonical().replace('_', "-");
                return Ok(deactivated_note("artifact", &selector.identifier, &restore));
            }
            render_generic_artifact(inputs, &selector.identifier)
        }
        "template" => render_template(inputs, &selector.identifier, raw),
        // The CLI adapter owns `section:` and never forwards it here.
        "section" => Err(IncludeError::MalformedSelector),
        kind => {
            let resolved = resolve_kind(kind, raw)?;
            // FR-016 / D-21: every activatable kind honours the effective deny, and the
            // reference *annotates* rather than refuses.
            if resolved.activatable()
                && inputs
                    .access
                    .denied_kind(&selector.identifier, Some(&[resolved]))
                    .map_err(access_error)?
                    .is_some()
            {
                return Ok(deactivated_note(kind, &selector.identifier, kind));
            }
            render_kind(inputs, resolved, selector, raw, true, true)
        }
    }
}

/// The scalar [`String`] view of [`render_include_exact`].
///
/// An explicit checked adapter, not the serving route: a rendered value whose code
/// points are not a scalar sequence (a lone surrogate) refuses with
/// [`IncludeError::Unencodable`] instead of replacing the code point. The exact
/// renderer is the authority; this adapter exists for scalar-only callers and
/// existing tests.
///
/// # Errors
///
/// As [`render_include_exact`], plus [`IncludeError::Unencodable`].
pub fn render_include(
    inputs: &IncludeInputs<'_>,
    selector: &IncludeSelector,
    raw: &str,
) -> Result<String, IncludeError> {
    render_include_exact(inputs, selector, raw)?
        .to_scalar_string()
        .map_err(|_| IncludeError::Unencodable {
            selector: raw.to_owned(),
        })
}

/// Dispatch one resolved kind to its renderer (shared with the `artifact:` probe).
///
/// `use_index` is the identity-index lookup the direct selector performs; `gated` is the
/// reference's `agent-profile`/`glossary-pack` path, which renders on the
/// activation-aware service. The generic `artifact:` probe passes `false` for both: the
/// reference hands it the plain service and no identity index.
fn render_kind(
    inputs: &IncludeInputs<'_>,
    kind: ArtifactKind,
    selector: &IncludeSelector,
    raw: &str,
    use_index: bool,
    gated: bool,
) -> Result<PythonString, IncludeError> {
    match kind {
        // A template is resolved by the ONE six-tier template resolver, never by a kind
        // repository. The generic `artifact:` probe cannot reach it (templates are not
        // charter-activatable), so this arm serves the explicit `template:` selector.
        ArtifactKind::Template => render_template(inputs, &selector.identifier, raw),
        // A non-activatable kind with no renderer: the reference returns `None` and the
        // orchestrator raises the "unsupported kind" error with the operator's token.
        ArtifactKind::Asset => Err(IncludeError::UnknownKind {
            kind: selector.kind.clone(),
        }),
        // Recognised, but ships no standalone artifact file (its nodes live in graph
        // fragments), so the reference resolves it to the standard not-found.
        ArtifactKind::AntiPattern => Err(not_found(kind, &selector.identifier, raw)),
        ArtifactKind::AgentProfile => {
            render_agent_profile(inputs, &selector.identifier, raw, use_index, gated)
        }
        // The reference renders a directive on the **plain** offering service
        // (`build_charter_context_include` wraps only `agent-profile` and
        // `glossary-pack` in the activation-aware service), so a directive is a
        // plain source-document lookup, never an activated-id provider: serving
        // it must not put the activation allow list between the operator and a
        // document that exists on disk. The effective *deny* still annotates it
        // (decided before this dispatch), exactly like every other activatable kind.
        ArtifactKind::Directive => {
            render_document_artifact(inputs, kind, &selector.identifier, raw, use_index, false)
        }
        // `glossary-pack` is the second (and last) activation-gated include path:
        // its fetch pointer must honour the same gate its delivery slot advertises.
        ArtifactKind::GlossaryPack => {
            render_document_artifact(inputs, kind, &selector.identifier, raw, use_index, gated)
        }
        // Every remaining document kind (skill, styleguide, toolguide, paradigm,
        // procedure, mission-step-contract) is served by the reference's PLAIN
        // `CharterOfferingService`: `build_charter_context_include` wraps only
        // `agent-profile` and `glossary-pack` in the activation-aware service.
        // Passing the gate here turned a Source-valid record into a structured
        // miss for a required-only kind — `activated_skills` is absent by
        // default, so the gate's in-force set is the org `required_skills`
        // floor and every authored project/org skill was dropped from
        // `charter context --include skill:<id>`.
        _ => render_document_artifact(inputs, kind, &selector.identifier, raw, use_index, false),
    }
}

/// The reference's `_deactivated_note` annotation for a deactivated selector.
fn deactivated_note(selector_kind: &str, identifier: &str, restore_kind: &str) -> PythonString {
    PythonString::from_scalars(&format!(
        "{selector_kind}:{identifier} is deactivated in this project \
         (charter activate {restore_kind} {identifier} restores it)"
    ))
}

/// Map one access-construction failure onto the include error family.
fn access_error(error: IncludeAccessError) -> IncludeError {
    if error.is_config() {
        IncludeError::Config {
            message: error.message().to_owned(),
        }
    } else {
        IncludeError::Setup {
            message: error.message().to_owned(),
        }
    }
}

/// Map one canonical offering load failure onto the include error family.
fn offering_error(error: crate::offering::OfferingLoadError) -> IncludeError {
    match error.abort() {
        // The reference's repository load aborts on this exception; the include path
        // surfaces it rather than pretending the file was skipped.
        Some(abort) => IncludeError::Aborted {
            message: abort.to_string(),
        },
        None => IncludeError::Setup {
            message: error.message().to_owned(),
        },
    }
}

/// Render a `template:<mission>/<name>` selector through the canonical template owner.
///
/// The reference delegates to `resolve_template_by_id` (the six-tier chain) and renders
/// `Template <id> (tier: <tier.value>):` plus the template text. `sk-charter` owns that
/// resolver and the rendering contract; this function only maps its failure phases onto
/// this module's error family. Nothing here touches the identity index, an artifact
/// document or a profile repository.
fn render_template(
    inputs: &IncludeInputs<'_>,
    identifier: &str,
    raw: &str,
) -> Result<PythonString, IncludeError> {
    use sk_charter::template_catalog::{TemplateIncludeError, render_source_template_include};

    match render_source_template_include(inputs.project_root, identifier, raw) {
        Ok(include) => Ok(PythonString::from_scalars(&include.text)),
        // The reference's renderer raises `ValueError` for a miss, a malformed id/org
        // declaration and a read failure; all three reach the CLI as `Error: <message>`.
        Err(
            error @ (TemplateIncludeError::Malformed { .. }
            | TemplateIncludeError::NotFound { .. }
            | TemplateIncludeError::Unreadable { .. }),
        ) => Err(IncludeError::Missing {
            message: error.to_string(),
        }),
        // A packaged-root or strict-decode failure is not caught by the reference's
        // renderer; it surfaces as that exception's own text.
        Err(error @ TemplateIncludeError::Uncaught { .. }) => Err(IncludeError::Aborted {
            message: error.to_string(),
        }),
    }
}

/// Resolve an operator kind token through the canonical kind authority.
fn resolve_kind(kind: &str, raw: &str) -> Result<ArtifactKind, IncludeError> {
    match ArtifactKind::from_operator_token(kind) {
        Ok(resolved) => Ok(resolved),
        Err(CharterError::MissionTypeNotAnArtifactKind) => Err(IncludeError::MissionType {
            selector: raw.to_owned(),
        }),
        Err(_) => Err(IncludeError::UnknownKind {
            kind: kind.to_owned(),
        }),
    }
}

/// The reference's `No {kind} found for selector …` message for `kind`.
fn not_found(kind: ArtifactKind, identifier: &str, raw: &str) -> IncludeError {
    let message = match kind {
        // The two dedicated renderers echo the *raw* selector instead.
        ArtifactKind::Directive => format!("No directive found for selector '{raw}'."),
        ArtifactKind::Tactic => format!("No tactic found for selector '{raw}'."),
        _ => {
            let kind_name = kind.canonical();
            format!("No {kind_name} found for selector '{kind_name}:{identifier}'.")
        }
    };
    IncludeError::Missing { message }
}

/// The repository key an include token names: its declared id when the index knows it.
fn lookup_id(
    inputs: &IncludeInputs<'_>,
    kind: ArtifactKind,
    identifier: &str,
    use_index: bool,
) -> Result<String, IncludeError> {
    if !use_index {
        // The best-effort `artifact:` probe passes no index, exactly like the
        // reference: the directive id normaliser, else the token verbatim.
        return Ok(IdentityIndex::unindexed_key(Some(kind), identifier));
    }
    match inputs.access.identity().try_declared_id(kind, identifier) {
        Ok(Some(declared)) => Ok(declared),
        Ok(None) => Ok(IdentityIndex::unindexed_key(Some(kind), identifier)),
        Err(abort) => Err(IncludeError::Aborted {
            message: abort.to_string(),
        }),
    }
}

/// Render a directive/tactic/styleguide/toolguide/procedure selector.
///
/// The record comes from the canonical offering service (the layered repository the
/// reference reaches), so an overlay record is served **merged** — including a
/// directive, whose repository admission is the same whole-model authority as every
/// other kind's and whose declaring file is never read directly.
fn render_document_artifact(
    inputs: &IncludeInputs<'_>,
    kind: ArtifactKind,
    identifier: &str,
    raw: &str,
    use_index: bool,
    gated: bool,
) -> Result<PythonString, IncludeError> {
    let key = lookup_id(inputs, kind, identifier, use_index)?;
    let offering = inputs.access.offering().map_err(access_error)?;
    let record = if gated {
        let gate = inputs.access.gate().map_err(access_error)?;
        offering.artifact_with_gate(kind, &key, gate)
    } else {
        offering.artifact(kind, &key)
    }
    .map_err(offering_error)?;
    let Some(record) = record else {
        return Err(not_found(kind, identifier, raw));
    };
    // The merged record's own canonical graph (inherited + overridden fields, exact
    // code points). Never the declaring file's raw document, and never a reopened or
    // re-parsed file: the repository already built both layers. A record without a
    // canonical graph is a Native projection gap, not a Source refusal.
    let Some(root) = record.exact_root() else {
        return Err(IncludeError::PendingProjection { kind });
    };
    let payload = match crate::body::exact_payload(kind, root) {
        Ok(payload) => payload,
        // Two distinct causes, never collapsed into "missing": no typed schema in
        // this crate (a Native capability gap), or an admitted document whose value
        // the reference's own model dump refuses (a real payload-serialization phase,
        // with its own cause).
        Err(crate::body::PayloadError::NoSchema) => {
            return Err(IncludeError::PendingProjection { kind });
        }
        Err(crate::body::PayloadError::Failed(cause)) => {
            return Err(IncludeError::Payload { kind, cause });
        }
    };
    Ok(render_payload(
        kind,
        &key,
        &payload,
        step_models(kind, root),
    ))
}

/// The canonical typed step models of a record with a canonical graph.
fn step_models(
    kind: ArtifactKind,
    root: sk_kernel::python_yaml::NodeRef<'_>,
) -> Option<Vec<crate::body::ModelObject>> {
    crate::body::model_list(kind, root, "steps").ok().flatten()
}

/// The reference's three-part render of one exact payload.
///
/// The heading, the inline body and the `Full artifact:` block all read the SAME
/// exact payload, so a value can never be spelled one way inline and another way in
/// the payload.
fn render_payload(
    kind: ArtifactKind,
    key: &str,
    payload: &PythonJsonValue,
    steps: Option<Vec<crate::body::ModelObject>>,
) -> PythonString {
    let mut lines = vec![heading(kind, key, payload)];
    lines.extend(include_body_lines_exact_with_steps(
        kind.canonical(),
        payload,
        steps.as_deref(),
    ));
    // The `Full artifact:` block is `json.dumps(..., ensure_ascii=True)`, so its text
    // is ASCII by construction: carrying it in a Rust `String` is lossless here (a
    // lone surrogate is escaped as `\uXXXX`, exactly like the reference).
    lines.extend(
        include_full_payload_lines_exact(payload)
            .into_iter()
            .map(|line| PythonString::from_scalars(&line)),
    );
    lines_text(lines)
}

/// `"\n".join(lines)`.
fn lines_text(lines: Vec<PythonString>) -> PythonString {
    let mut text = PythonString::default();
    for (position, line) in lines.into_iter().enumerate() {
        if position > 0 {
            text.push_scalars("\n");
        }
        text.append(&line);
    }
    text
}

/// The reference heading line for one artifact (`<Label> <id>: <title>`).
fn heading(kind: ArtifactKind, key: &str, payload: &PythonJsonValue) -> PythonString {
    let (label, title_field) = match kind {
        ArtifactKind::Directive => ("Directive", "title"),
        ArtifactKind::Tactic => ("Tactic", "name"),
        ArtifactKind::Styleguide => ("Styleguide", "title"),
        ArtifactKind::Toolguide => ("Toolguide", "title"),
        ArtifactKind::Procedure => ("Procedure", "name"),
        ArtifactKind::AgentProfile => ("Agent profile", "name"),
        ArtifactKind::GlossaryPack => ("Glossary pack", "id"),
        ArtifactKind::Paradigm => ("Paradigm", "name"),
        ArtifactKind::MissionStepContract => ("Mission step contract", "action"),
        ArtifactKind::Skill => ("Skill", "title"),
        // Non-rendering kinds never reach a heading; name them honestly if one does.
        ArtifactKind::Template | ArtifactKind::Asset | ArtifactKind::AntiPattern => {
            (kind.canonical(), "id")
        }
    };
    let title = match payload.field(title_field) {
        Some(PythonJsonValue::String(text)) => text.clone(),
        _ => PythonString::from_scalars(key),
    };
    let mut line = PythonString::from_scalars(&format!("{label} {key}: "));
    line.append(&title);
    line
}

/// Render an `agent-profile:<id>` selector from the canonical profile catalogue.
///
/// The direct selector uses the activation-aware path (the reference's gated
/// `agent_profiles` mapping); the `artifact:` probe passes `gated = false`.
fn render_agent_profile(
    inputs: &IncludeInputs<'_>,
    identifier: &str,
    raw: &str,
    use_index: bool,
    gated: bool,
) -> Result<PythonString, IncludeError> {
    let kind = ArtifactKind::AgentProfile;
    let key = lookup_id(inputs, kind, identifier, use_index)?;
    let offering = inputs.access.offering().map_err(access_error)?;
    let profile = if gated {
        let gate = inputs.access.gate().map_err(access_error)?;
        offering.profile_with_gate(&key, gate)
    } else {
        offering.profile(&key)
    }
    .map_err(offering_error)?;
    let Some(profile) = profile else {
        return Err(not_found(kind, identifier, raw));
    };
    // The reference dumps the model (`by_alias=True, mode="json", exclude_none=True`)
    // and then applies `_jsonable_artifact_value`. The canonical model dump body owns
    // that schema/default/lineage surface; the exact projection here keeps every code
    // point instead of a scalar projection of it.
    let dump = profile.model_dump();
    let payload = match dump.model_json_exact() {
        Ok(payload) => payload,
        // No canonical graph in this body: a Native capability boundary, not a
        // payload-serialization phase.
        Err(crate::body::PayloadError::NoSchema) => {
            return Err(IncludeError::PendingProjection { kind });
        }
        // The reference's own model dump refuses this record's value (a
        // non-UTF-8 `!!binary` member, or a mapping key with no UTF-8 form).
        Err(crate::body::PayloadError::Failed(cause)) => {
            return Err(IncludeError::Payload { kind, cause });
        }
    };
    Ok(render_payload(kind, &key, &payload, None))
}

/// Render the best-effort `artifact:<id>` probe across every addressable kind.
fn render_generic_artifact(
    inputs: &IncludeInputs<'_>,
    identifier: &str,
) -> Result<PythonString, IncludeError> {
    let raw = format!("artifact:{identifier}");
    let selector = IncludeSelector {
        kind: "artifact".to_owned(),
        identifier: identifier.to_owned(),
    };
    let mut matches: Vec<(ArtifactKind, PythonString)> = Vec::new();
    let mut pending: Vec<ArtifactKind> = Vec::new();
    for kind in ArtifactKind::ALL {
        // The reference's single exclusion set (`_NON_AUGMENTATION_ELIGIBLE_KINDS`) is
        // exactly the non-activatable kinds, derived from the canonical authority.
        if !kind.activatable() {
            continue;
        }
        match render_kind(inputs, kind, &selector, &raw, false, false) {
            Ok(text) => matches.push((kind, text)),
            Err(IncludeError::PendingProjection { .. }) => pending.push(kind),
            Err(IncludeError::Missing { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    if matches.len() + pending.len() > 1 {
        // A present artifact whose Native projection is pending still counts
        // as a match. Never silently claim uniqueness by dropping that kind.
        let kinds = ArtifactKind::ALL
            .into_iter()
            .filter(|kind| {
                pending.contains(kind) || matches.iter().any(|(found, _text)| found == kind)
            })
            .map(|kind| kind.canonical())
            .collect::<Vec<&str>>()
            .join(", ");
        return Err(IncludeError::Missing {
            message: format!(
                "Ambiguous artifact selector 'artifact:{identifier}' matched kinds: {kinds}."
            ),
        });
    }
    if let Some((_kind, text)) = matches.pop() {
        return Ok(text);
    }
    match pending.len() {
        0 => Err(IncludeError::Missing {
            message: format!("No artifact found for selector 'artifact:{identifier}'."),
        }),
        1 => Err(IncludeError::PendingProjection { kind: pending[0] }),
        _ => Err(IncludeError::PendingProjection { kind: pending[0] }),
    }
}

//! Action-scoped charter context resolution.
//!
//! Mirrors the resolution and JSON half of `src/charter/activation/context.py`,
//! `context_json.py`, `progressive_disclosure.py`, `context_renderers/
//! artifact_bodies.py` and the contract ledger in `context_contract.py`.
//!
//! The payload is the versioned `charter context --json` envelope:
//!
//! * five typed arrays (`directives`, `tactics`, `styleguides`, `toolguides`,
//!   `procedures`) whose entries carry provenance, an outbound `references[]`
//!   list, a `delivery` cadence (`inline` for the `requires` closure, `link` for
//!   `suggests`-reached artifacts, everything inline under `--include-all`) and
//!   the artifact body when it is inlined;
//! * the top-level `references[]` link set that names every delivered artifact
//!   (`{id, relation, when, reason}`), so the union of inlined and named ids is
//!   the delivered set with no cap;
//! * `all_directives` and `directives_source` from one project-governance
//!   resolution (`catalog_fallback` / `activation`, plus `+charter` and
//!   `+project_local` when those layers contribute);
//! * the real `project_charter` block (paths, bytes, byte-count metadata) and
//!   the imported `org_charter` policy block;
//! * `governance_references` status entries.
//!
//! A non-bootstrap action delivers no action grain, but every payload still
//! carries the governance blocks and diagnostics.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

use crate::activation::{ActivationState, filter_graph_by_activation};
use crate::body::{PayloadError, PayloadFailure, selected_text_field};
use crate::error::DoctrineError;
use crate::graph::{DrgEdge, DrgGraph, NodeKind, Relation, is_valid_urn};
use crate::identity::{IdentityIndex, IdentityLayer};
use crate::ledger::ActivationLedger;
use crate::offering::OfferingLoadError;
use crate::offering::include_access::IncludeAccess;
use crate::org_charter::{self, AuthoredScalar, ExactText, load_org_charter_policy};
use crate::profile::ProfileRepository;
use crate::project_governance::{
    CharterConfigSections, CharterDocument, CharterSectionsError, DirectiveSelectionOutcome,
    LocalDirective, read_charter_document, read_charter_sections, resolve_directive_selection,
};
use crate::query::deliver_action_context;

/// The top-level `charter context --json` contract version.
pub const CONTEXT_SCHEMA_VERSION: &str = "1.4.0";

/// The complete set of top-level keys the payload may carry.
pub const CONTEXT_CONTRACT_TOP_LEVEL_KEYS: [&str; 15] = [
    "context_schema_version",
    "action",
    "mode",
    "directives",
    "tactics",
    "styleguides",
    "toolguides",
    "procedures",
    "all_directives",
    "directives_source",
    "diagnostics",
    "references",
    "project_charter",
    "org_charter",
    "governance_references",
];

/// Actions that always use bootstrap delivery.
pub const BOOTSTRAP_ACTIONS: [&str; 4] = ["specify", "plan", "implement", "review"];

/// The `when` text rendered for a `suggests` edge that authored none.
pub const STATED_DEFAULT_WHEN: &str = "Fetch when this artefact's guidance applies to the work at hand (no specific trigger authored yet).";

/// A lazy progressive record phase that could not answer.
///
/// The reference reads a delivered artifact's metadata from its **model
/// properties** and only dumps the model (JSON mode) for an inline `body`.
/// A missing id is a healthy miss (`Ok(None)`), but a setup failure, a raising
/// layered repository, or a payload the reference's own dump refuses is not an
/// absence. This is the ONE narrow carrier for those phases; it is boxed by
/// [`DoctrineError::ProgressiveDelivery`] so the action build can propagate it
/// without a second global error hierarchy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProgressiveDeliveryError {
    /// The canonical offering access could not be built (the reference's
    /// raising service builder). `config` marks the reference's
    /// `ActiveCharterConfigError` family.
    Setup {
        /// The rendered reason.
        message: String,
        /// Whether this is the activation-config family.
        config: bool,
    },
    /// A layered repository load failed. `aborted` carries the Python exception
    /// the reference's per-file catch does not cover, when this is one.
    Repository {
        /// The repository's own message.
        message: String,
        /// The `(Python exception, message)` of a constructor abort.
        aborted: Option<(String, String)>,
    },
    /// This crate has no canonical model view for the merged record (a Native
    /// capability boundary, never a Source refusal).
    NativeBoundary {
        /// The artifact kind.
        kind: ArtifactKind,
        /// What projection is missing.
        detail: &'static str,
    },
    /// The reference's own model-JSON dump refuses this merged record.
    Payload {
        /// The artifact kind.
        kind: ArtifactKind,
        /// The reference's real cause.
        cause: PayloadFailure,
    },
}

impl std::fmt::Display for ProgressiveDeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup { message, .. } | Self::Repository { message, .. } => {
                formatter.write_str(message)
            }
            Self::NativeBoundary { kind, detail } => write!(
                formatter,
                "no canonical model view for {}: {detail}",
                kind.canonical()
            ),
            Self::Payload { kind, cause } => write!(
                formatter,
                "cannot serialize the {} record: {cause}",
                kind.canonical()
            ),
        }
    }
}

impl std::error::Error for ProgressiveDeliveryError {}

/// Box the one progressive-delivery carrier into the doctrine error channel.
fn progressive_delivery(error: ProgressiveDeliveryError) -> DoctrineError {
    DoctrineError::ProgressiveDelivery {
        source: Box::new(error),
    }
}

/// Turn a repository load failure into its phase, preserving a constructor abort.
fn repository_delivery_error(error: OfferingLoadError) -> ProgressiveDeliveryError {
    ProgressiveDeliveryError::Repository {
        message: error.message().to_owned(),
        aborted: error
            .abort()
            .map(|abort| (abort.python.to_owned(), abort.message.clone())),
    }
}

/// The delivery cadence of one artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Delivered inline (eager `requires` closure, or `--include-all`).
    Inline,
    /// Named as a link with its `when` clause.
    Link,
}

/// One outbound reference entry (`{id, relation, when, reason}`).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EdgeReference {
    /// The bare target id.
    pub id: String,
    /// The relation label.
    pub relation: String,
    /// The edge's `when` clause (a stated default for an uncovered `suggests`;
    /// `null` otherwise, matching the reference reference-DTO).
    pub when: Option<String>,
    /// The edge's authored rationale (`null` when none, matching the contract).
    pub reason: Option<String>,
}

impl EdgeReference {
    pub(crate) fn project(edge: &DrgEdge) -> Self {
        // The reference replaces a missing OR empty `when` on a `suggests` edge
        // with the stated default (`if not when and relation is SUGGESTS`).
        let when = edge
            .when()
            .filter(|when| !when.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                (edge.relation() == Relation::Suggests).then(|| STATED_DEFAULT_WHEN.to_owned())
            });
        Self {
            id: bare_id(edge.target()),
            relation: edge.relation().as_str().to_owned(),
            when,
            reason: edge.reason().map(str::to_owned),
        }
    }
}

/// Orthogonal to [`Delivery`]; part of the compact steady-state envelope.
///
/// The reference's `CharterContextResult.mode` has exactly three spellings:
/// `bootstrap`, `compact` and `missing`. `missing` is a **text-rail only**
/// result (neither `charter.yaml` nor `charter.md` exists under the canonical
/// read root); the structured `--json` builder never emits it, so a `--json`
/// run reports it on the first (text) stage's envelope and a `bootstrap`
/// structured payload beside it, exactly like the reference CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Action-scoped bootstrap governance is delivered.
    Bootstrap,
    /// The small steady-state payload (no action grain).
    Compact,
    /// No charter surface exists at all (text rail only).
    Missing,
}

/// Which of the reference's four **text** rails a context build took.
///
/// `build_charter_context` has four mutually exclusive branches, each with its
/// own first-load/depth/mark/prose-read/reference-count policy. The structured
/// `build_charter_context_json` builder has only two (`bootstrap` / `compact`):
/// it never consults charter presence and never applies the depth tier, so
/// [`TextRail`] belongs to the text stage alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextRail {
    /// A non-bootstrap action whose action node is not declared in the merged
    /// graph: compact, `first_load=false`, no state mark, and it returns
    /// *before* the charter-presence gate.
    NonBootstrap,
    /// A grain-delivering action with neither `charter.yaml` nor `charter.md`:
    /// the fixed missing-charter text, the state's own first-load/depth, no mark.
    Missing,
    /// A grain-delivering action below the minimum effective depth: the widened
    /// compact bundle; marks before rendering.
    CompactBundle,
    /// A grain-delivering action at or above the minimum effective depth: the
    /// full bootstrap text; renders, then marks.
    Bootstrap,
}

impl TextRail {
    /// The `mode` this rail reports (`compact` for both compact rails).
    #[must_use]
    pub fn mode(self) -> ContextMode {
        match self {
            Self::NonBootstrap | Self::CompactBundle => ContextMode::Compact,
            Self::Missing => ContextMode::Missing,
            Self::Bootstrap => ContextMode::Bootstrap,
        }
    }

    /// Whether the first-load mark may be written on this rail.
    ///
    /// Only the two grain-delivering, non-missing rails mark, and only when the
    /// prepared snapshot says this action is a first load.
    #[must_use]
    pub fn marks_first_load(self) -> bool {
        matches!(self, Self::Bootstrap | Self::CompactBundle)
    }

    /// Whether this rail reads the `charter.md` prose at all.
    #[must_use]
    pub fn reads_prose(self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// Select the reference's text rail in the reference's own order.
///
/// Order is load-bearing (`build_charter_context`):
///
/// 1. a non-bootstrap action whose action node is **not declared** in the merged
///    graph returns here, *before* the charter-presence gate — so a typeless or
///    undeclared non-bootstrap action still renders compact governance for a
///    project whose charter is absent, and it never marks state;
/// 2. otherwise the charter-presence gate (`charter.yaml` OR `charter.md`);
/// 3. otherwise the below-minimum-depth tier (the widened compact bundle);
/// 4. otherwise bootstrap.
///
/// `action_is_bootstrap` is the reference's 4-token fast path, which renders
/// bootstrap without consulting the predicate. `action_declared` is **node-URN
/// membership**, never an empty grain: a declared-but-activation-starved node is
/// declared and therefore bootstrap-capable.
#[must_use]
pub fn select_text_rail(
    action_is_bootstrap: bool,
    action_declared: bool,
    charter_present: bool,
    effective_depth: usize,
) -> TextRail {
    if !action_is_bootstrap && !action_declared {
        return TextRail::NonBootstrap;
    }
    if !charter_present {
        return TextRail::Missing;
    }
    if effective_depth < crate::context_state::MIN_EFFECTIVE_DEPTH {
        return TextRail::CompactBundle;
    }
    TextRail::Bootstrap
}

/// The reference's **prose**-presence gate: `charter.yaml` OR `charter.md`.
///
/// This is deliberately not the structured authority's yaml-only
/// `project_charter.present` signal: `charter.yaml` takes precedence for
/// governance and `charter.md` is the readable secondary surface, so a
/// `charter.md`-only project still renders. It is not an authority/presence
/// gate for any other reader — the catalogue read stays yaml-only.
///
/// Source's unignored stat errors escape, and yaml existence short-circuits
/// the md probe. The undeclared rail must not call this function at all.
///
/// # Errors
/// An existence failure Source does not ignore (for example EACCES).
pub fn charter_present(canonical_root: &Path) -> Result<bool, DoctrineError> {
    for relative in [sk_charter::CHARTER_YAML, sk_charter::CHARTER_MD] {
        let path = canonical_root.join(relative);
        if sk_kernel::paths::source_path_exists(&path)
            .map_err(|source| DoctrineError::io("inspect charter presence", path, source))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A closed-set context diagnostic code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticCode {
    /// A selection token named no artifact.
    UnknownSelectionDropped,
    /// The catalog default was applied.
    CatalogDefaultApplied,
    /// A fallback branch supplied the set.
    FallbackApplied,
    /// Selections were merged.
    SelectionMerged,
    /// A reference could not be resolved.
    UnresolvedReference,
}

/// One context diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    /// The closed-set code.
    pub code: DiagnosticCode,
    /// Human text (may be reworded; match on the code).
    pub message: String,
}

/// One delivered typed artifact, in the reference entry shape.
///
/// Carries `{id, source, title?, summary?, references, delivery, body?}` —
/// exactly the contract's per-entry keys, built by
/// [`CharterContext::to_json_exact`]. The URN, kind and `when` are API data and
/// never appear in the JSON.
///
/// `Eq` is deliberately absent: [`PythonJsonValue::Float`] is a float, so the
/// exact body has no total equality. `PartialEq` is order-sensitive for
/// `Object`/`ExactObject`, which is what the payload comparison needs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArtifactEntry {
    /// The bare artifact id.
    pub id: String,
    /// The full DRG URN (API data).
    #[serde(skip)]
    pub urn: String,
    /// The artifact kind (API data).
    #[serde(skip)]
    pub kind: NodeKind,
    /// Artifact origin (`built-in`, `org`, `project`).
    pub source: String,
    /// The display label, when one is declared.
    ///
    /// Exact code points (the merged model's own value). The text renderer
    /// projects it for the `String` line it must produce; the JSON payload keeps
    /// it exact.
    #[serde(skip)]
    pub title: Option<PythonString>,
    /// The one-line intent/purpose, when declared.
    #[serde(skip)]
    pub summary: Option<PythonString>,
    /// Every outbound DRG edge of this artifact.
    pub references: Vec<EdgeReference>,
    /// The delivery cadence.
    pub delivery: Delivery,
    /// The artifact body, for an inline delivery.
    ///
    /// The **merged** record's canonical model dump (inherited and overridden
    /// fields, exact code points, arbitrary-width integers, non-finite floats),
    /// produced by the one payload authority in [`crate::body`] over
    /// `MergedArtifact::exact_root`. It is never a reopened declaring file and
    /// never a scalar projection of one. Serialized by
    /// [`CharterContext::to_json_exact`], not by the derive (a `serde_json` value
    /// cannot carry a lone surrogate or an arbitrary-width integer).
    #[serde(skip)]
    pub body: Option<PythonJsonValue>,
    /// A `suggests` link's `when` clause (API data, rendered through
    /// `references[]` in the JSON contract).
    #[serde(skip)]
    pub when: Option<String>,
}

/// One `all_directives` entry: the plain provenance DTO (no disclosure fields).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DirectiveEntry {
    /// The bare directive id.
    pub id: String,
    /// Artifact origin (`built-in`, `org`, `project`).
    pub source: String,
    /// The display label, when one is declared (exact code points).
    #[serde(skip)]
    pub title: Option<PythonString>,
    /// The directive description, when declared (exact code points).
    #[serde(skip)]
    pub summary: Option<PythonString>,
}

/// A profile-cited artifact entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProfileLink {
    /// The profile that cites it.
    pub profile: String,
    /// The cited artifact URN.
    pub urn: String,
    /// The relation by which it is cited.
    pub relation: Relation,
    /// The edge's `when` clause, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// The project-charter presence/metadata block.
///
/// `present`/`path` describe the authoritative `charter.yaml`; the `charter.md`
/// fields are display companions. When `charter.yaml` exists, `bytes` is always
/// present and the metadata file contributes `hash`, `source_path`,
/// `bundle_schema_version` and `schema_version` when it declares them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProjectCharterBlock {
    /// Whether `charter.yaml` exists.
    pub present: bool,
    /// The repository-relative `charter.yaml` path.
    pub path: String,
    /// Whether `charter.md` is present.
    pub charter_md_present: bool,
    /// The repository-relative `charter.md` path.
    pub charter_md_path: String,
    /// Byte length of `charter.yaml`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The recorded charter digest, when `metadata.yaml` declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The recorded source path, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// The recorded bundle schema version, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_schema_version: Option<i64>,
    /// The recorded schema version, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
}

/// One governance policy in the org-charter block.
///
/// Every field is the authored value, exactly: `value` is the reference's
/// `str | bool` (a JSON boolean stays a boolean, never the string `'True'`), and
/// `field`/`enforcement` keep their exact code points. The derived `Serialize`
/// is the finite `serde_json` view; [`CharterContext::to_json_exact`] is the
/// payload authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrgGovernancePolicy {
    /// The policy field.
    pub field: ExactText,
    /// The policy value, as authored (`str | bool`).
    pub value: AuthoredScalar,
    /// The enforcement level.
    pub enforcement: ExactText,
    /// Always `org` (the block only describes org-tier policy).
    pub source: String,
}

/// One org pack's summary in the org-charter block.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrgCharterPack {
    /// The pack name (its org name when declared, else the pack directory name).
    pub pack_name: ExactText,
    /// The pack's governance policies.
    pub governance_policies: Vec<OrgGovernancePolicy>,
    /// The pack's `required_directives`, verbatim (exact code points, so a lone
    /// surrogate survives; the reference emits the authored list unchanged).
    pub required_directives: Vec<ExactText>,
}

/// The org-charter block.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct OrgCharterBlock {
    /// Whether any org charter policy is present.
    pub present: bool,
    /// One entry per pack that ships an `org-charter.yaml`.
    pub packs: Vec<OrgCharterPack>,
}

impl OrgCharterBlock {
    /// The block's **exact** payload (`{present, packs}`), in contract order.
    ///
    /// Every authored value is carried as authored: an `str | bool` policy value
    /// stays a JSON boolean when it was a Python `bool` (never the string
    /// `'True'`), and a string keeps its exact code points (a lone surrogate
    /// included). The derived `Serialize` is only the finite `serde_json` view.
    pub fn to_json_value(&self) -> PythonJsonValue {
        self.to_json_exact()
    }
}

/// One `governance_references` status entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GovernanceReferenceStatus {
    /// The declared repository-relative path.
    pub path: String,
    /// Whether it exists.
    pub exists: bool,
    /// Whether the path stays inside the repository root.
    pub safe: bool,
    /// A diagnostic when it is unsafe, missing or not a file (`null` otherwise,
    /// matching the reference status DTO).
    pub warning: Option<String>,
}

/// The action-scoped governance payload.
#[derive(Clone, Debug, Serialize)]
pub struct CharterContext {
    /// The top-level contract version.
    pub context_schema_version: String,
    /// The normalized action name.
    pub action: String,
    /// The delivery mode.
    pub mode: ContextMode,
    /// Eager/link directives.
    pub directives: Vec<ArtifactEntry>,
    /// Eager/link tactics.
    pub tactics: Vec<ArtifactEntry>,
    /// Eager/link styleguides.
    pub styleguides: Vec<ArtifactEntry>,
    /// Eager/link toolguides.
    pub toolguides: Vec<ArtifactEntry>,
    /// Eager/link procedures (first-class since schema 1.1.0).
    pub procedures: Vec<ArtifactEntry>,
    /// Every directive exposed by project governance.
    pub all_directives: Vec<DirectiveEntry>,
    /// Which branch resolved the directive set.
    pub directives_source: Option<String>,
    /// Resolution diagnostics, always present.
    pub diagnostics: Vec<Diagnostic>,
    /// The complete link set naming every delivered artifact.
    pub references: Vec<EdgeReference>,
    /// The project-charter block.
    pub project_charter: ProjectCharterBlock,
    /// The org-charter block.
    pub org_charter: OrgCharterBlock,
    /// Governance-reference status entries.
    pub governance_references: Vec<GovernanceReferenceStatus>,
    /// Profile-channel links (`requires`/`specializes_from`/`suggests`).
    ///
    /// API data, not part of the versioned top-level JSON contract.
    #[serde(skip)]
    pub profile_links: Vec<ProfileLink>,
    /// Co-delivered tension pairs and their reconcilers (API data).
    #[serde(skip)]
    pub tension_arbiters: Vec<(String, Vec<String>)>,
    /// Co-delivered tension pairs with no reconciler (API data).
    #[serde(skip)]
    pub unarbitrated_tensions: Vec<(String, String)>,
}

impl CharterContext {
    /// The contract keys this payload actually carries.
    pub fn top_level_keys(&self) -> BTreeSet<&'static str> {
        let value = self.to_json_exact();
        let PythonJsonValue::ExactObject(object) = &value else {
            return BTreeSet::new();
        };
        object
            .entries()
            .iter()
            .filter_map(|(key, _value)| key.to_scalar_string().ok())
            .filter_map(|key| {
                CONTEXT_CONTRACT_TOP_LEVEL_KEYS
                    .iter()
                    .copied()
                    .find(|known| *known == key)
            })
            .collect()
    }

    /// The payload as the canonical ordered value graph (the ONE payload owner).
    ///
    /// Every block is built here, in the contract's key order, and each delivered
    /// entry's `body` is inserted as the **exact** merged model dump, so a value
    /// the scalar JSON model cannot carry (a lone surrogate, an arbitrary-width
    /// integer, a non-finite float) still reaches the caller. A caller that must
    /// write JSON uses the canonical Kernel encoder over this value
    /// ([`PythonJsonValue::render_python_with`]); [`Self::to_json`] is the finite
    /// `serde_json` adapter for callers that can only hold scalars.
    pub fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            (
                "context_schema_version",
                PythonJsonValue::scalar(&self.context_schema_version),
            ),
            ("action", PythonJsonValue::scalar(&self.action)),
            ("mode", enum_text(&self.mode)),
            (
                "directives",
                PythonJsonValue::Array(
                    self.directives
                        .iter()
                        .map(ArtifactEntry::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "tactics",
                PythonJsonValue::Array(
                    self.tactics
                        .iter()
                        .map(ArtifactEntry::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "styleguides",
                PythonJsonValue::Array(
                    self.styleguides
                        .iter()
                        .map(ArtifactEntry::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "toolguides",
                PythonJsonValue::Array(
                    self.toolguides
                        .iter()
                        .map(ArtifactEntry::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "procedures",
                PythonJsonValue::Array(
                    self.procedures
                        .iter()
                        .map(ArtifactEntry::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "all_directives",
                PythonJsonValue::Array(
                    self.all_directives
                        .iter()
                        .map(DirectiveEntry::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "directives_source",
                PythonJsonValue::optional_scalar(self.directives_source.as_deref()),
            ),
            (
                "diagnostics",
                PythonJsonValue::Array(
                    self.diagnostics
                        .iter()
                        .map(Diagnostic::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "references",
                PythonJsonValue::Array(
                    self.references
                        .iter()
                        .map(EdgeReference::to_json_exact)
                        .collect(),
                ),
            ),
            ("project_charter", self.project_charter.to_json_exact()),
            ("org_charter", self.org_charter.to_json_exact()),
            (
                "governance_references",
                PythonJsonValue::Array(
                    self.governance_references
                        .iter()
                        .map(GovernanceReferenceStatus::to_json_exact)
                        .collect(),
                ),
            ),
        ])
    }

    /// The payload as a `serde_json` value — the explicit FINITE adapter.
    ///
    /// Returns [`serde_json::Value::Null`] only when the exact payload has no
    /// scalar JSON form at all (a lone-surrogate string/key or a non-finite
    /// float inside a body). Every CLI surface writes [`Self::to_json_exact`]
    /// through the canonical Kernel encoder, so this adapter is for in-process
    /// callers whose model cannot carry exact data; it never reformats one.
    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_exact()
            .to_serde()
            .unwrap_or(serde_json::Value::Null)
    }
}

/// One ordered JSON object from its fields, in the order given.
fn object(fields: Vec<(&str, PythonJsonValue)>) -> PythonJsonValue {
    PythonJsonValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// A closed-set enum's contract spelling, taken from its ONE `serde` rename
/// authority (never a second hand-written mapping).
fn enum_text<T: Serialize>(value: &T) -> PythonJsonValue {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => PythonJsonValue::scalar(&text),
        Ok(other) => PythonJsonValue::from_serde(&other),
        Err(_) => PythonJsonValue::Null,
    }
}

impl ArtifactEntry {
    /// The contract entry, in the reference's key order.
    fn to_json_exact(&self) -> PythonJsonValue {
        let mut fields = vec![
            ("id", PythonJsonValue::scalar(&self.id)),
            ("source", PythonJsonValue::scalar(&self.source)),
        ];
        push_optional_exact(&mut fields, "title", self.title.as_ref());
        push_optional_exact(&mut fields, "summary", self.summary.as_ref());
        fields.push((
            "references",
            PythonJsonValue::Array(
                self.references
                    .iter()
                    .map(EdgeReference::to_json_exact)
                    .collect(),
            ),
        ));
        fields.push(("delivery", enum_text(&self.delivery)));
        if let Some(body) = &self.body {
            fields.push(("body", body.clone()));
        }
        object(fields)
    }
}

impl DirectiveEntry {
    /// The plain provenance DTO, in the reference's key order.
    fn to_json_exact(&self) -> PythonJsonValue {
        let mut fields = vec![
            ("id", PythonJsonValue::scalar(&self.id)),
            ("source", PythonJsonValue::scalar(&self.source)),
        ];
        push_optional_exact(&mut fields, "title", self.title.as_ref());
        push_optional_exact(&mut fields, "summary", self.summary.as_ref());
        object(fields)
    }
}

impl EdgeReference {
    /// The reference DTO, in the reference's key order.
    fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            ("id", PythonJsonValue::scalar(&self.id)),
            ("relation", PythonJsonValue::scalar(&self.relation)),
            (
                "when",
                PythonJsonValue::optional_scalar(self.when.as_deref()),
            ),
            (
                "reason",
                PythonJsonValue::optional_scalar(self.reason.as_deref()),
            ),
        ])
    }
}

impl Diagnostic {
    /// `{code, message}`.
    fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            ("code", enum_text(&self.code)),
            ("message", PythonJsonValue::scalar(&self.message)),
        ])
    }
}

impl ProjectCharterBlock {
    /// The presence/metadata block, in declaration order.
    fn to_json_exact(&self) -> PythonJsonValue {
        let mut fields = vec![
            ("present", PythonJsonValue::Bool(self.present)),
            ("path", PythonJsonValue::scalar(&self.path)),
            (
                "charter_md_present",
                PythonJsonValue::Bool(self.charter_md_present),
            ),
            (
                "charter_md_path",
                PythonJsonValue::scalar(&self.charter_md_path),
            ),
        ];
        if let Some(bytes) = self.bytes {
            fields.push(("bytes", PythonJsonValue::Integer(bytes.to_string())));
        }
        push_optional_scalar(&mut fields, "hash", self.hash.as_deref());
        push_optional_scalar(&mut fields, "source_path", self.source_path.as_deref());
        if let Some(version) = self.bundle_schema_version {
            fields.push((
                "bundle_schema_version",
                PythonJsonValue::Integer(version.to_string()),
            ));
        }
        push_optional_scalar(
            &mut fields,
            "schema_version",
            self.schema_version.as_deref(),
        );
        object(fields)
    }
}

impl OrgCharterBlock {
    /// `{present, packs}`.
    fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            ("present", PythonJsonValue::Bool(self.present)),
            (
                "packs",
                PythonJsonValue::Array(
                    self.packs
                        .iter()
                        .map(OrgCharterPack::to_json_exact)
                        .collect(),
                ),
            ),
        ])
    }
}

impl OrgCharterPack {
    /// `{pack_name, governance_policies, required_directives}`.
    fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            (
                "pack_name",
                PythonJsonValue::String(self.pack_name.as_python().clone()),
            ),
            (
                "governance_policies",
                PythonJsonValue::Array(
                    self.governance_policies
                        .iter()
                        .map(OrgGovernancePolicy::to_json_exact)
                        .collect(),
                ),
            ),
            (
                "required_directives",
                PythonJsonValue::Array(
                    self.required_directives
                        .iter()
                        .map(|id| PythonJsonValue::String(id.as_python().clone()))
                        .collect(),
                ),
            ),
        ])
    }
}

impl OrgGovernancePolicy {
    /// `{field, value, enforcement, source}` — the reference's `model_dump()`
    /// entry plus its `source` tag, with the authored `str | bool` value exact.
    fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            (
                "field",
                PythonJsonValue::String(self.field.as_python().clone()),
            ),
            ("value", self.value.to_json_value()),
            (
                "enforcement",
                PythonJsonValue::String(self.enforcement.as_python().clone()),
            ),
            ("source", PythonJsonValue::scalar(&self.source)),
        ])
    }
}

impl GovernanceReferenceStatus {
    /// `{path, exists, safe, warning}`.
    fn to_json_exact(&self) -> PythonJsonValue {
        object(vec![
            ("path", PythonJsonValue::scalar(&self.path)),
            ("exists", PythonJsonValue::Bool(self.exists)),
            ("safe", PythonJsonValue::Bool(self.safe)),
            (
                "warning",
                PythonJsonValue::optional_scalar(self.warning.as_deref()),
            ),
        ])
    }
}

/// Append a declared string field, or nothing when it is absent (the
/// reference's `exclude_none=True`).
fn push_optional_scalar(
    fields: &mut Vec<(&'static str, PythonJsonValue)>,
    key: &'static str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        fields.push((key, PythonJsonValue::scalar(value)));
    }
}

/// Append a declared **exact** string field, or nothing when it is absent.
fn push_optional_exact(
    fields: &mut Vec<(&'static str, PythonJsonValue)>,
    key: &'static str,
    value: Option<&PythonString>,
) {
    if let Some(value) = value {
        fields.push((key, PythonJsonValue::String(value.clone())));
    }
}

/// A context request.
#[derive(Clone, Debug)]
pub struct ContextRequest<'a> {
    /// The action name (`implement`) or action URN (`action:software-dev/implement`).
    pub action: &'a str,
    /// The mission type, used to build a bare action's URN.
    pub mission_type: Option<&'a str>,
    /// Maximum `suggests` hops.
    pub depth: usize,
    /// An optional profile id whose channel is delivered.
    pub profile: Option<&'a str>,
    /// Inline the whole closure (`--include-all`).
    pub include_all: bool,
}

impl<'a> ContextRequest<'a> {
    /// A request for `action` with the compact steady-state depth.
    pub fn new(action: &'a str) -> Self {
        Self {
            action,
            mission_type: None,
            depth: 1,
            profile: None,
            include_all: false,
        }
    }
}

/// The resolved inputs to a context build.
pub struct ContextInputs<'a> {
    /// The merged, unfiltered DRG.
    pub graph: &'a DrgGraph,
    /// The activation ledger.
    pub ledger: &'a ActivationLedger,
    /// The identity index (already scanned).
    pub identity: &'a IdentityIndex,
    /// The profile repository.
    pub profiles: &'a ProfileRepository,
    /// The project root.
    pub project_root: &'a Path,
    /// Required-floor ids per kind (what an org pack mandates).
    pub required_floor: &'a BTreeMap<ArtifactKind, Vec<String>>,
    /// Non-fatal project-layer warnings to surface as diagnostics.
    pub project_warnings: &'a [String],
}

/// Which ordered Source stage a context build answers.
///
/// The reference `charter context` command executes two real builds for
/// `--json`: `build_charter_context` renders the text first (that builder has
/// **no** `include_all` parameter and is the only one that receives
/// `--profile`), then `build_charter_context_json` builds the structured
/// payload second (that builder has **no** profile parameter at all). A text
/// CLI run executes only the first stage. The two stages therefore disagree by
/// design about the profile channel, `--include-all` and record bodies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextPhase {
    /// One combined build, profile and `include_all` both applied and inline
    /// bodies serialized. This is the legacy single-build helper mode; it is
    /// **not** the reference CLI's two-stage contract.
    Single,
    /// The text stage: the profile channel applies, `--include-all` is not a
    /// text input, and no record body is serialized (the reference text
    /// renderers read model properties, never the JSON-mode model dump).
    Text,
    /// The structured stage: no profile channel, `include_all` applies, and
    /// inline bodies are serialized outside any lookup catch.
    Structured,
}

impl ContextPhase {
    /// Whether this stage contributes the profile channel (`--profile`).
    fn uses_profile(self) -> bool {
        self != Self::Structured
    }

    /// Whether this stage treats `--include-all` as a request parameter.
    fn uses_include_all(self) -> bool {
        self != Self::Text
    }

    /// Whether this stage serializes inline record bodies (the JSON-mode dump).
    fn serializes_bodies(self) -> bool {
        self != Self::Text
    }

    /// Whether this stage assembles the `all_directives` DTOs.
    ///
    /// The reference text build never produces that block; only the structured
    /// builder resolves it, so the text stage must not pay its record lookups.
    fn assembles_all_directives(self) -> bool {
        self != Self::Text
    }
}

/// Build the action-scoped governance payload in ONE combined build.
///
/// This is an explicitly **single-build helper**: it applies the profile and
/// `--include-all` together and serializes inline bodies, which is what the
/// pre-CF66 CLI happened to do. It is not the reference's ordered execution.
/// A CLI consumer that must follow the reference's phases calls
/// [`build_action_context_text`] first and, for `--json`, then
/// [`build_action_context_structured`] with the first result's depth.
pub fn build_action_context(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
) -> Result<CharterContext, DoctrineError> {
    build_action_context_phased(inputs, request, ContextPhase::Single, None)
}

/// Build the reference's **text** stage.
///
/// The profile channel applies; `--include-all` is ignored because the
/// reference text builder has no such parameter; no record body is serialized.
/// A failure here prevents the structured stage and its later side effects.
///
/// This is the pre-CF68 helper: it assumes the charter surface is present, so
/// it never takes the `missing` rail. A caller that owns the canonical read
/// root uses [`build_text_stage`], which takes the presence fact and reports
/// the rail it selected.
pub fn build_action_context_text(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
) -> Result<CharterContext, DoctrineError> {
    build_action_context_phased(inputs, request, ContextPhase::Text, None)
}

/// The reference's text stage plus the [`TextRail`] it took.
#[derive(Clone, Debug)]
pub struct TextStageBuild {
    /// The payload the text stage built (its `mode` is the rail's mode).
    pub context: CharterContext,
    /// The rail the reference's `build_charter_context` would have taken.
    pub rail: TextRail,
}

/// Build the reference's **text** stage on the rail the reference selects.
///
/// `charter_present` is the caller's [`charter_present`] answer for the
/// canonical read root the caller actually read (the reference's presence gate
/// runs against `sync_result.canonical_root`). Rail selection is
/// [`select_text_rail`] — the ONE place the reference's four-branch order lives.
/// The structured stage is deliberately **not** rail-gated: this function's
/// rail applies to the text stage only.
pub fn build_text_stage(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
    charter_present: bool,
) -> Result<TextStageBuild, DoctrineError> {
    build_text_stage_with_presence(inputs, request, || Ok(charter_present))
}

/// Build text with a fallible, deferred presence probe. The live CLI uses this
/// seam so an undeclared action returns before probing charter existence.
/// The bool-input adapter above remains an explicit already-observed fact view.
///
/// # Errors
/// Resolution/build failures, or presence errors only on grain-delivering rails.
pub fn build_text_stage_with_presence(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
    presence: impl FnOnce() -> Result<bool, DoctrineError>,
) -> Result<TextStageBuild, DoctrineError> {
    let normalized = normalize_context_action(request.action);
    let action_is_bootstrap = BOOTSTRAP_ACTIONS.contains(&normalized.as_str());
    // Declared means **node-URN membership in the merged graph**, not "the name
    // resolved to a URN": the reference's `_action_node_declared` tests
    // `f"action:{mission}/{action}" in bundle.merged.node_urns()`, and a
    // resolved-but-absent node is exactly the activation-starved case that must
    // still take the undeclared/non-bootstrap rail.
    let declared = declared_action_urn(inputs, request, &normalized, action_is_bootstrap)?
        .is_some_and(|urn| inputs.graph.node(&urn).is_some());
    let present = if action_is_bootstrap || declared {
        presence()?
    } else {
        false
    };
    let rail = select_text_rail(action_is_bootstrap, declared, present, request.depth);
    let context = build_action_context_phased(inputs, request, ContextPhase::Text, Some(rail))?;
    Ok(TextStageBuild { context, rail })
}

/// Build the reference's **structured** stage.
///
/// The profile channel is absent here (the reference JSON builder takes no
/// profile argument), `include_all` applies, and inline bodies are serialized
/// outside the lookup phase. The caller passes the text stage's depth.
pub fn build_action_context_structured(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
) -> Result<CharterContext, DoctrineError> {
    build_action_context_phased(inputs, request, ContextPhase::Structured, None)
}

/// The reference's action resolution for one build: the action node URN, or
/// `None` when the graph does not declare it (or, for a fast-path action, when
/// resolution failed and the reference tolerates the empty grain).
///
/// One seam for both stages and for the rail predicate, so "declared" can never
/// mean two different things in one invocation.
fn declared_action_urn(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
    normalized_action: &str,
    action_is_bootstrap: bool,
) -> Result<Option<String>, DoctrineError> {
    // A bootstrap action is delivered even when the project declares no such
    // action node (or declares several with different mission types): the
    // reference payload then simply carries the governance blocks with an
    // empty action grain. Every other action must resolve to one node.
    match resolve_action_urn(inputs.graph, normalized_action, request.mission_type) {
        Ok(urn) => Ok(Some(urn)),
        Err(error) if action_is_bootstrap => {
            let _ = error;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn build_action_context_phased(
    inputs: &ContextInputs<'_>,
    request: &ContextRequest<'_>,
    phase: ContextPhase,
    rail: Option<TextRail>,
) -> Result<CharterContext, DoctrineError> {
    // The reference's two builders differ by construction: the text builder
    // receives `profile` and never `include_all`; the JSON builder receives
    // `include_all` and has no profile parameter. Forcing the unused inputs off
    // here keeps one resolution path from pretending both stages are the same.
    let profile_id = request.profile.filter(|_| phase.uses_profile());
    let include_all = request.include_all && phase.uses_include_all();
    let normalized_action = normalize_context_action(request.action);
    let raw_action = normalized_action.as_str();
    if raw_action.is_empty() {
        return Err(DoctrineError::InvalidAction {
            action: request.action.to_owned(),
        });
    }
    let bootstrap_action = BOOTSTRAP_ACTIONS.contains(&normalized_action.as_str());
    let action_urn = declared_action_urn(inputs, request, raw_action, bootstrap_action)?;

    let state =
        ActivationState::from_ledger(inputs.ledger, inputs.identity, inputs.required_floor)?;
    let mut diagnostics = Vec::new();
    // `state.unknown_allow_tokens` stays API data (the activation surfaces
    // report it), but it is NOT a payload diagnostic: the reference logs an
    // unknown activation token at WARNING level in `_seed_sets` /
    // `_resolve_directive_base` and never emits `UNKNOWN_SELECTION_DROPPED` for
    // it. Only a charter `selected_*` token that no layer knows produces that
    // diagnostic (one entry, from `_drop_unknown`).
    for warning in inputs.project_warnings {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::UnresolvedReference,
            message: warning.clone(),
        });
    }

    let filtered = filter_graph_by_activation(inputs.graph, &state);
    let action_declared = action_urn
        .as_deref()
        .is_some_and(|urn| inputs.graph.node(urn).is_some());
    // The text stage reports the rail the reference selected (which can be
    // `missing` or either compact rail); the structured stage has only the two
    // envelope modes and never consults charter presence or the depth tier.
    let mode = match rail {
        Some(rail) => rail.mode(),
        None if bootstrap_action || action_declared => ContextMode::Bootstrap,
        None => ContextMode::Compact,
    };

    let mut delivery = action_urn
        .as_deref()
        .map(|urn| deliver_action_context(&filtered, urn, request.depth))
        .unwrap_or_default();
    // One merged-record source for every body/title this build delivers. It is
    // the canonical include authority, built on first demand: the action payload
    // answers "what does the repository hold for (kind, id)" the same way the
    // `--include` fetch does, instead of reopening the declaring file.
    let records = ActionRecords::new(inputs.project_root);
    // One canonical-root probe drives every charter-config read in this build:
    // the reference anchors the directive/local/charter reads on the same
    // `resolve_canonical_repo_root` facade.
    let charter_config = charter_config_read(inputs.project_root);
    // The authored `governance:` / `directives:` sections as the reference's
    // `load_governance_config` / `load_directives_config` see them. A retired
    // `governance.doctrine` key is a typed HARD refusal (raised here, exactly
    // as the reference's governance-reference read re-raises it); a malformed
    // shape is a best-effort degrade, never invented provenance.
    let charter_sections = charter_sections(inputs.project_root, charter_config.is_available())?;
    // Selected project doctrine widens the action's delivered set: the
    // reference walks the `requires`/`suggests` closure of the activated and
    // org-required directives/tactics/paradigms, minus any artifact another
    // action scopes. Without this an activated styleguide reached only from a
    // paradigm silently disappears from the typed arrays.
    let governance = resolve_directives(
        inputs.graph,
        inputs.identity,
        &records,
        inputs.ledger,
        &state,
        &charter_sections,
        phase.assembles_all_directives(),
    )?;
    // The reference resolves the project/org seed sets only inside
    // `_load_action_governance_bundle`'s `resolved_type is not None` branch. A
    // typeless or unresolvable action therefore contributes NO doctrine roots:
    // a bootstrap action still carries its governance blocks and an empty
    // action grain. Seeding here without a resolved mission type would deliver
    // the whole catalogue for `charter context --action review` on a typeless
    // project, which the reference never does.
    let seeds = if action_urn.is_some() {
        governance_seeds(
            inputs.project_root,
            inputs.graph,
            inputs.identity,
            inputs.ledger,
            &state,
            &charter_sections,
            charter_config.is_available(),
        )
    } else {
        GovernanceSeeds::default()
    };
    let mut roots: BTreeSet<String> = action_urn.iter().cloned().collect();
    roots.extend(seeds.roots.iter().cloned());

    let mut profile_links = Vec::new();
    if let Some(profile_id) = profile_id {
        if !state.admits_profile(profile_id) {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnresolvedReference,
                message: format!(
                    "profile {profile_id:?} is deactivated for this project; its cited doctrine was not delivered"
                ),
            });
        } else {
            match inputs.profiles.resolve_str(profile_id, inputs.graph) {
                Ok(profile) => {
                    // The DRG is a `String`-keyed graph, so only a scalar id
                    // can name one of its nodes. A non-scalar identity keeps its
                    // exact profile (and its exact lineage answer) without
                    // inventing a URN: the reference's own
                    // `graph.node(urn)` lookup finds nothing there either.
                    if let Some(id) = profile.scalar_id() {
                        let urn = format!("agent_profile:{id}");
                        if inputs.graph.node(&urn).is_some() {
                            delivery.eager.insert(urn.clone());
                            delivery.all_artifacts.insert(urn.clone());
                        }
                    }
                    for link in inputs
                        .profiles
                        .channel_deliveries_str(profile_id, inputs.graph)?
                    {
                        if link.relation == Relation::Suggests {
                            delivery.suggested.insert(link.urn.clone());
                            delivery.all_artifacts.insert(link.urn.clone());
                        } else {
                            delivery.eager.insert(link.urn.clone());
                            delivery.all_artifacts.insert(link.urn.clone());
                        }
                        profile_links.push(ProfileLink {
                            profile: link.profile,
                            urn: link.urn,
                            relation: link.relation,
                            when: link.when,
                        });
                    }
                }
                Err(error) => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::UnresolvedReference,
                    message: format!("profile {profile_id:?} could not be resolved: {error}"),
                }),
            }
        }
    }

    let mut directives = Vec::new();
    let mut tactics = Vec::new();
    let mut styleguides = Vec::new();
    let mut toolguides = Vec::new();
    let mut procedures = Vec::new();
    let mut delivered: BTreeSet<String> = BTreeSet::new();
    let mut references = Vec::new();

    if mode == ContextMode::Bootstrap {
        let inline_urns = requires_closure(&filtered, &roots);
        // The reference `_classify_artifact_urns` delivers the action resolution
        // URNs UNION the selected roots' `requires`/`suggests` closure. The
        // closure includes the roots themselves, so an activated/org-required
        // artifact the action bundle does not otherwise reach (and every
        // catalogue-default root) is delivered from its own root, not only when
        // some other artifact happens to point at it.
        let mut candidates: BTreeSet<String> = delivery.all_artifacts.clone();
        candidates.extend(selected_closure(
            &filtered,
            &seeds.roots,
            action_urn.as_deref(),
        ));
        for urn in &candidates {
            let Some(node) = filtered.node(urn) else {
                continue;
            };
            // Delivered slots are the five typed kinds plus the two
            // reference-only kinds the reference's delivery table flips in:
            // `asset` (ungated) and `glossary_pack` (activation-gated). Every
            // other kind -- notably `paradigm` -- is resolved but never
            // delivered, so it is never a link *target* either.
            let kind = node.kind();
            let deliverable = match kind {
                NodeKind::Asset => true,
                NodeKind::GlossaryPack => state.admits(kind.as_str(), &bare_id(urn)),
                _ => requested_kind(kind),
            };
            if !deliverable {
                continue;
            }
            // The reference's directive slot guard: only the selected /
            // project-local directive set is delivered, even when the action
            // graph reaches another directive.
            if kind == NodeKind::Directive && !seeds.directives.contains(&bare_id(urn)) {
                continue;
            }
            delivered.insert(urn.clone());
        }

        // Typed delivery: each delivered artifact of a typed kind is an entry;
        // the `requires` closure of the action + selected roots is inline and
        // everything else delivered is a link (all inline under --include-all).
        let mut typed: BTreeMap<NodeKind, Vec<ArtifactEntry>> = BTreeMap::new();
        for urn in &delivered {
            let Some(entry) = artifact_entry(
                &filtered,
                inputs.identity,
                &records,
                urn,
                &inline_urns,
                include_all,
                phase.serializes_bodies(),
            )?
            else {
                continue;
            };
            typed.entry(entry.kind).or_default().push(entry);
        }
        for (kind, mut entries) in typed {
            sort_entries(&mut entries);
            match kind {
                NodeKind::Directive => directives = entries,
                NodeKind::Tactic => tactics = entries,
                NodeKind::Styleguide => styleguides = entries,
                NodeKind::Toolguide => toolguides = entries,
                NodeKind::Procedure => procedures = entries,
                _ => {}
            }
        }
        // The complete link set over `roots ∪ delivered ∪ bridges`, where the
        // bridges are every URN the action resolution visited (including kinds
        // this payload never delivers) so a pass-through hop still names the
        // artifact on its far side.
        references = link_references(&filtered, &roots, &delivered, &delivery.all_artifacts);
    }

    diagnostics.extend(seeds.diagnostics.clone());
    diagnostics.extend(governance.diagnostics.clone());

    let project_charter = project_charter_block(inputs.project_root);
    let org_charter = org_charter_block(inputs.project_root);
    let governance_references =
        governance_reference_status(inputs.project_root, charter_sections.governance_valid);

    profile_links.sort_by(|left, right| {
        (&left.profile, &left.urn, left.relation, &left.when).cmp(&(
            &right.profile,
            &right.urn,
            right.relation,
            &right.when,
        ))
    });
    profile_links.dedup();

    if mode == ContextMode::Compact {
        directives.clear();
        tactics.clear();
        styleguides.clear();
        toolguides.clear();
        procedures.clear();
        references.clear();
    }

    Ok(CharterContext {
        context_schema_version: CONTEXT_SCHEMA_VERSION.to_owned(),
        action: normalized_action,
        mode,
        directives,
        tactics,
        styleguides,
        toolguides,
        procedures,
        all_directives: governance.entries,
        directives_source: governance.source,
        diagnostics,
        references,
        project_charter,
        org_charter,
        governance_references,
        profile_links,
        tension_arbiters: delivery.tension_arbiters,
        unarbitrated_tensions: delivery.unarbitrated_tensions,
    })
}

/// Source action/state key: Python str.strip().lower(), without selector shortening.
/// Uses the existing pinned Unicode whitespace and lower-case authorities.
pub fn normalize_context_action(action: &str) -> String {
    let stripped =
        action.trim_matches(|code: char| sk_kernel::python_text::is_whitespace(code as u32));
    let points: Vec<u32> = stripped.chars().map(u32::from).collect();
    PythonString::from_units(sk_kernel::python_text::lower_codepoints(&points))
        .expect("Unicode lowercase retains valid code points")
        .to_scalar_string()
        .expect("lowercase of scalar text remains scalar")
}

fn bare_id(urn: &str) -> String {
    urn.split_once(':')
        .map(|(_, body)| body)
        .unwrap_or(urn)
        .to_owned()
}

fn requested_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Directive
            | NodeKind::Tactic
            | NodeKind::Styleguide
            | NodeKind::Toolguide
            | NodeKind::Procedure
    )
}

fn sort_entries(entries: &mut Vec<ArtifactEntry>) {
    entries.sort_by(|left, right| {
        (left.kind, &left.id, &left.urn).cmp(&(right.kind, &right.id, &right.urn))
    });
    entries.dedup();
}

fn source_label(node: &crate::graph::DrgNode) -> String {
    match node.provenance() {
        Some("project") => "project".to_owned(),
        Some(provenance) if provenance.starts_with("org:") => "org".to_owned(),
        _ => "built-in".to_owned(),
    }
}

fn artifact_entry(
    graph: &DrgGraph,
    identity: &IdentityIndex,
    records: &ActionRecords,
    urn: &str,
    inline: &BTreeSet<String>,
    include_all: bool,
    serialize_body: bool,
) -> Result<Option<ArtifactEntry>, DoctrineError> {
    let Some(node) = graph.node(urn) else {
        return Ok(None);
    };
    let kind = node.kind();
    if !requested_kind(kind) {
        return Ok(None);
    }
    let Some(artifact_kind) = kind.artifact_kind() else {
        return Ok(None);
    };
    let artifact_id = bare_id(urn);
    // The reference `artifact_to_dict` reads the artifact MODEL's `title`
    // (then `name`) and `intent` (then `purpose`) and never the DRG node label.
    // The model property read is a different phase from the JSON-mode dump: a
    // bad unstructured member the reference never serializes for metadata must
    // not fail a link. `meta` walks only the selected text fields.
    let meta = records
        .meta(identity, artifact_kind, &artifact_id)
        .map_err(progressive_delivery)?;
    let references = graph
        .edges_from(urn, None)
        .into_iter()
        .map(EdgeReference::project)
        .collect();
    let is_inline = include_all || inline.contains(urn);
    // The body is requested only for an inline entry, exactly like the
    // reference `_decorate_entry` calling `body_of` only when `is_inline` — and
    // only in a stage that serializes bodies at all. The reference TEXT build
    // reads model properties (`title`/`name`, `intent`/`purpose`) and never
    // dumps the model, so a body that cannot be JSON-serialized is a structured-
    // stage failure, not a text-stage one.
    let body = if is_inline && serialize_body {
        artifact_payload(records, identity, artifact_kind, &artifact_id)?
    } else {
        None
    };
    // The entry's `source` names the layer that supplied the record actually
    // delivered, exactly like the reference's `repository.get_provenance(id)`: a
    // project/org overlay shadows the built-in file, so the DRG node's tag (built
    // from the graph layer that declared the node) is only the fallback for an
    // artifact with no layered file.
    let source = identity
        .source_layer(artifact_kind, &artifact_id)
        .map(|layer| layer.as_str().to_owned())
        .unwrap_or_else(|| {
            // The reference `collect_typed_artifacts` falls back to the
            // built-in tier when `repository.get_provenance(id)` is absent
            // (a genuinely missing record, including a graph-only node).
            if meta.present {
                source_label(node)
            } else {
                "built-in".to_owned()
            }
        });
    Ok(Some(ArtifactEntry {
        id: artifact_id,
        urn: urn.to_owned(),
        kind,
        source,
        title: meta.title,
        summary: meta.summary,
        references,
        delivery: if is_inline {
            Delivery::Inline
        } else {
            Delivery::Link
        },
        body,
        when: None,
    }))
}

/// One merged record's selected metadata fields (never its full model dump).
///
/// The reference `artifact_to_dict` uses the fallback chain
/// `title -> name` and `intent -> purpose`, so only the fields actually needed
/// by that chain are selected. A missing record yields `None` fields; a setup,
/// repository or selected-field failure is returned, never cached as absence.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RecordMeta {
    /// Whether the layered repository held a record (false is a true miss).
    present: bool,
    title: Option<PythonString>,
    summary: Option<PythonString>,
}

/// The canonical merged-record body payload of one delivered artifact.
///
/// This replaces the previous declaring-file read (a second `serde_yaml_ng`
/// parse feeding the finite `canonical_artifact_body`). The reference's action
/// payload dumps the *repository model*
/// (`_jsonable_artifact_value(model_dump(...))`) and that model is the **merged**
/// record: an org/project overlay replaces the fields it declares and inherits
/// the rest from the earlier layers. [`ActionRecords`] resolves that record
/// through the same canonical offering service the `--include` fetch reads, so
/// an inherited or overridden field cannot be spelled one way on one surface and
/// another way on the other.
///
/// A genuine absent record contributes no body; a setup/repository/payload
/// failure is a typed error, not a healthy miss.
fn artifact_payload(
    records: &ActionRecords,
    identity: &IdentityIndex,
    kind: ArtifactKind,
    artifact_id: &str,
) -> Result<Option<PythonJsonValue>, DoctrineError> {
    records
        .body(identity, kind, artifact_id)
        .map_err(progressive_delivery)
}

/// The selected-metadata cache: one phase result per `(kind, record key)`.
type RecordMetaCache =
    RefCell<BTreeMap<(ArtifactKind, String), Result<RecordMeta, ProgressiveDeliveryError>>>;

/// The requested-inline-body cache: one phase result per `(kind, record key)`.
type RecordBodyCache = RefCell<
    BTreeMap<(ArtifactKind, String), Result<Option<PythonJsonValue>, ProgressiveDeliveryError>>,
>;

/// The lazily built merged-record source one action build reads.
///
/// The offering service is the canonical [`IncludeAccess`] authority — the same one
/// the `--include` fetch path reads — built over **this build's own layer set**
/// (the roots the identity index and the merged graph came from), so a payload can
/// never mix records from one root set with identities from another. The service is
/// built on first demand (a compact build never needs it) and each selected field
/// set/body is memoised, which keeps a repeated id from being merged twice.
struct ActionRecords {
    project_root: PathBuf,
    access: OnceLock<Result<IncludeAccess, ProgressiveDeliveryError>>,
    meta_cache: RecordMetaCache,
    body_cache: RecordBodyCache,
}

impl ActionRecords {
    fn new(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            access: OnceLock::new(),
            meta_cache: RefCell::new(BTreeMap::new()),
            body_cache: RefCell::new(BTreeMap::new()),
        }
    }

    /// The selected metadata fields of one record.
    fn meta(
        &self,
        identity: &IdentityIndex,
        kind: ArtifactKind,
        artifact_id: &str,
    ) -> Result<RecordMeta, ProgressiveDeliveryError> {
        let key = (kind, artifact_id.to_owned());
        if let Some(cached) = self.meta_cache.borrow().get(&key) {
            return cached.clone();
        }
        let computed = self.load_meta(identity, kind, artifact_id);
        self.meta_cache.borrow_mut().insert(key, computed.clone());
        computed
    }

    /// The requested inline body of one record (evaluated on demand only).
    fn body(
        &self,
        identity: &IdentityIndex,
        kind: ArtifactKind,
        artifact_id: &str,
    ) -> Result<Option<PythonJsonValue>, ProgressiveDeliveryError> {
        let key = (kind, artifact_id.to_owned());
        if let Some(cached) = self.body_cache.borrow().get(&key) {
            return cached.clone();
        }
        let computed = self.load_body(identity, kind, artifact_id);
        self.body_cache.borrow_mut().insert(key, computed.clone());
        computed
    }

    fn load_meta(
        &self,
        identity: &IdentityIndex,
        kind: ArtifactKind,
        artifact_id: &str,
    ) -> Result<RecordMeta, ProgressiveDeliveryError> {
        // The graph/resource-only kinds have no merged document record here.
        // The reference's repository lookup yields no model, so this is the
        // true-miss branch rather than a failure.
        if !crate::offering::CanonicalOfferingService::serves_documents(kind) {
            return Ok(RecordMeta::default());
        }
        let access = self.access(identity)?;
        let offering = access
            .offering()
            .map_err(|error| ProgressiveDeliveryError::Setup {
                message: error.message().to_owned(),
                config: error.is_config(),
            })?;
        let key = record_key(identity, kind, artifact_id);
        let Some(record) = offering
            .artifact(kind, &key)
            .map_err(repository_delivery_error)?
        else {
            return Ok(RecordMeta::default());
        };
        let Some(root) = record.exact_root() else {
            return Err(ProgressiveDeliveryError::NativeBoundary {
                kind,
                detail: "the merged record has no canonical graph",
            });
        };
        Ok(RecordMeta {
            present: true,
            title: speech_field(kind, root, "title", "name")?,
            summary: speech_field(kind, root, "intent", "purpose")?,
        })
    }

    fn load_body(
        &self,
        identity: &IdentityIndex,
        kind: ArtifactKind,
        artifact_id: &str,
    ) -> Result<Option<PythonJsonValue>, ProgressiveDeliveryError> {
        if !crate::offering::CanonicalOfferingService::serves_documents(kind) {
            return Ok(None);
        }
        let access = self.access(identity)?;
        let offering = access
            .offering()
            .map_err(|error| ProgressiveDeliveryError::Setup {
                message: error.message().to_owned(),
                config: error.is_config(),
            })?;
        let key = record_key(identity, kind, artifact_id);
        let Some(record) = offering
            .artifact(kind, &key)
            .map_err(repository_delivery_error)?
        else {
            return Ok(None);
        };
        let Some(root) = record.exact_root() else {
            return Err(ProgressiveDeliveryError::NativeBoundary {
                kind,
                detail: "the merged record has no canonical graph",
            });
        };
        match crate::body::exact_payload(kind, root) {
            Ok(payload) => Ok(Some(payload)),
            Err(PayloadError::NoSchema) => Err(ProgressiveDeliveryError::NativeBoundary {
                kind,
                detail: "this crate has no typed model for the kind",
            }),
            Err(PayloadError::Failed(cause)) => {
                Err(ProgressiveDeliveryError::Payload { kind, cause })
            }
        }
    }

    /// The offering access over the identity index's own layer set.
    ///
    /// A missing built-in tier is the reference builder's raising setup phase,
    /// not a healthy absence; the same value is cached either way so a repeated
    /// read cannot turn a caught failure into a miss.
    fn access(&self, identity: &IdentityIndex) -> Result<&IncludeAccess, ProgressiveDeliveryError> {
        let slot = self.access.get_or_init(|| {
            if identity.built_in_root.as_os_str().is_empty() {
                return Err(ProgressiveDeliveryError::Setup {
                    message: "no pack root could be resolved for built-in".to_owned(),
                    config: false,
                });
            }
            let roots = crate::effective_set::OfferingRoots {
                built_in: Some(identity.built_in_root.clone()),
                org_roots: identity.org_roots.clone(),
                project: identity.project_root.clone(),
            };
            IncludeAccess::with_roots(&self.project_root, roots).map_err(|error| {
                ProgressiveDeliveryError::Setup {
                    message: error.message().to_owned(),
                    config: error.is_config(),
                }
            })
        });
        match slot {
            Ok(access) => Ok(access),
            Err(error) => Err(error.clone()),
        }
    }
}

/// The reference `artifact_to_dict` speech-field chain: the first selected text
/// field that `entry_text` accepts, never a full model dump.
fn speech_field(
    kind: ArtifactKind,
    root: sk_kernel::python_yaml::NodeRef<'_>,
    primary: &str,
    fallback: &str,
) -> Result<Option<PythonString>, ProgressiveDeliveryError> {
    for name in [primary, fallback] {
        let selected = selected_text_field(kind, root, name).map_err(|error| match error {
            PayloadError::NoSchema => ProgressiveDeliveryError::NativeBoundary {
                kind,
                detail: "this crate has no typed model for the kind",
            },
            PayloadError::Failed(cause) => ProgressiveDeliveryError::Payload { kind, cause },
        })?;
        if let Some(text) = entry_text(selected.as_ref()) {
            return Ok(Some(text.clone()));
        }
    }
    Ok(None)
}

/// The repository key an action-delivered id names: its declared id when the
/// index knows it, else the id verbatim, exactly like the include path's
/// `lookup_id`.
fn record_key(identity: &IdentityIndex, kind: ArtifactKind, artifact_id: &str) -> String {
    match identity.try_declared_id(kind, artifact_id) {
        Ok(Some(declared)) => declared,
        _ => IdentityIndex::unindexed_key(Some(kind), artifact_id),
    }
}

/// The authored speech field when Python string truthiness is nonempty.
///
/// Source preserves whitespace-only text and exact surrogate units here;
/// this is not str.strip() or a scalar projection.
pub fn entry_text(value: Option<&PythonString>) -> Option<&PythonString> {
    value.filter(|text| !text.is_empty())
}

/// The complete link set naming every delivered artifact.
///
/// Mirrors the reference `progressive_disclosure.link_references`: one entry per
/// `(target id, relation)` for every edge whose source is a traversal root, a
/// delivered artifact, or an action-resolution *bridge* URN, and whose target is
/// delivered. Sources are walked in sorted order and edges in graph order, which
/// is the reference's deterministic order; deduplication is by `(id, relation)`.
pub(crate) fn link_references(
    graph: &DrgGraph,
    roots: &BTreeSet<String>,
    delivered: &BTreeSet<String>,
    bridge_urns: &BTreeSet<String>,
) -> Vec<EdgeReference> {
    let mut sources: BTreeSet<&str> = BTreeSet::new();
    sources.extend(roots.iter().map(String::as_str));
    sources.extend(delivered.iter().map(String::as_str));
    sources.extend(bridge_urns.iter().map(String::as_str));
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut references = Vec::new();
    for source in sources {
        for edge in graph.edges_from(source, None) {
            if !delivered.contains(edge.target()) {
                continue;
            }
            let key = (bare_id(edge.target()), edge.relation().as_str().to_owned());
            if !seen.insert(key) {
                continue;
            }
            references.push(EdgeReference::project(edge));
        }
    }
    references
}

/// The traversal seeds selected project doctrine contributes.
#[derive(Default)]
struct GovernanceSeeds {
    /// Directive/tactic/paradigm URNs the closure starts from.
    roots: BTreeSet<String>,
    /// The delivered directive id set (the activation selection, or the
    /// catalogue default, plus project-local directives). The reference's
    /// directive slot guard admits no other directive.
    directives: BTreeSet<String>,
    /// Resolution diagnostics produced while seeding.
    diagnostics: Vec<Diagnostic>,
}

/// Resolve the activated + org-required directive/tactic/paradigm seeds.
///
/// Mirrors the reference `_seed_sets`: an absent activation key falls back to
/// `_graph_and_catalog_default_ids` — every node of that kind the merged graph
/// declares (built-in, org AND project) unioned with the built-in catalogue —
/// an explicitly empty list stays empty, org `required_<kind>` unions on top,
/// and the effective deny subtracts last (it never holds a required id). An
/// absent key is the default project state, so excluding org/project graph
/// nodes there silently under-delivered org-pack governance. Project-local
/// directives declared in `charter.yaml` join the directive seeds only when the
/// project has an explicit directive allow list and a loadable `directives:`
/// section, exactly as the reference guards it.
fn governance_seeds(
    project_root: &Path,
    graph: &DrgGraph,
    identity: &IdentityIndex,
    ledger: &ActivationLedger,
    state: &ActivationState,
    charter_sections: &CharterConfigSections,
    resolution_available: bool,
) -> GovernanceSeeds {
    let mut roots = BTreeSet::new();
    let mut directive_ids: BTreeSet<String> = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for (kind, node_kind) in [
        (ArtifactKind::Directive, NodeKind::Directive),
        (ArtifactKind::Tactic, NodeKind::Tactic),
        (ArtifactKind::Paradigm, NodeKind::Paradigm),
    ] {
        // `_graph_and_catalog_default_ids`: the catalogue default is every id
        // the ACTIVE graph declares for this kind, unioned with the built-in
        // catalogue. In a merged production graph the built-in layer already
        // supplies the catalogue, so the graph ids are the union; a node's
        // provenance never excludes it.
        let graph_ids: Vec<String> = graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == node_kind)
            .map(|node| bare_id(node.urn()))
            .collect();
        // The built-in catalogue alone is what the resolver's diagnostic
        // reports ("using built-in catalog default (N ...)"), exactly as the
        // reference's `_resolve_paradigm_base` counts `offering_catalog`.
        let catalog_len = graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == node_kind)
            .filter(|node| {
                // A raw built-in graph carries no provenance; a merged one tags
                // it "built-in".
                !node.provenance().is_some_and(|provenance| {
                    provenance == "project" || provenance.starts_with("org:")
                })
            })
            .count();
        let mut warnings = Vec::new();
        let mut ids: Vec<String> = match ledger.allow_for(kind) {
            None => {
                // The paradigm catalogue-default diagnostic is emitted by the
                // REFERENCE RESOLVER (`_resolve_paradigm_base`), not by the
                // action-bundle seed walk. When the canonical-root gate makes
                // that resolution fail, the reference reports no diagnostic at
                // all, so the seed path must not invent this one either. The
                // catalogue roots themselves are unchanged (the seed walk is a
                // separate seam that does not need Git).
                if kind == ArtifactKind::Paradigm
                    && resolution_available
                    && charter_sections.governance_valid
                    && charter_sections.directives_valid
                {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::CatalogDefaultApplied,
                        message: format!(
                            "No activated paradigm set configured; using built-in catalog default ({catalog_len} paradigms)."
                        ),
                    });
                }
                graph_ids
            }
            Some(tokens) => tokens
                .iter()
                .map(|token| identity.activation_id(kind, token, &mut warnings))
                .collect(),
        };
        // The org `required_<plural>` overlay is the reference's COERCING
        // selection reader (`_read_org_required_selections` /
        // `read_org_required_ids`: `isinstance(value, list)` then
        // `str(item).strip()`), NOT the strict required-floor reader
        // (`_required_values`, which drops a whole field containing one
        // non-string member). The two answer different questions and both stay
        // distinct APIs. A token with no scalar projection cannot name a
        // `String`-keyed graph node, so it simply reaches no root — never a
        // substituted spelling.
        for token in
            org_charter::read_org_required_ids_exact(project_root, kind).unwrap_or_default()
        {
            let declared = identity.activation_id_exact(kind, &token);
            if let Ok(id) = declared.to_scalar_string() {
                ids.push(id);
            }
        }
        // Unknown activation tokens are logged by the reference, never added
        // to the payload diagnostics (`_drop_unknown` is for charter
        // selections only). `warnings` is intentionally not surfaced here.
        let _ = warnings;
        let denied = state.deny_for(kind);
        for id in ids {
            let urn = format!("{}:{id}", kind.canonical());
            let denied = denied.is_some_and(|deny| !deny.admits(&id) || !deny.admits(&urn));
            if !denied {
                if kind == ArtifactKind::Directive {
                    directive_ids.insert(id.clone());
                }
                roots.insert(urn);
            }
        }
    }
    if ledger.allow_for(ArtifactKind::Directive).is_some() && charter_sections.directives_valid {
        for local in charter_sections.local_directives() {
            directive_ids.insert(local.id.clone());
            roots.insert(format!("directive:{}", local.id));
        }
    }
    GovernanceSeeds {
        roots,
        directives: directive_ids,
        diagnostics,
    }
}

/// The `requires` closure of `roots`, including the roots themselves.
pub(crate) fn requires_closure(graph: &DrgGraph, roots: &BTreeSet<String>) -> BTreeSet<String> {
    crate::query::walk_edges(graph, roots, &BTreeSet::from([Relation::Requires]), None)
}

/// The `requires`/`suggests` closure of the selected roots, minus artifacts a
/// *different* action scopes.
///
/// Mirrors the reference `_classify_artifact_urns` closure contribution: the
/// start URNs themselves are part of the closure (`walk_edges` visits its
/// roots), so a selected root is delivered even when no other artifact points
/// at it. Only the four closure kinds widen; a procedure/asset reached from a
/// selected root is not delivered by this seam. The ownership gate applies to
/// the roots too, exactly as the reference applies it to every closure member.
fn selected_closure(
    graph: &DrgGraph,
    roots: &BTreeSet<String>,
    action_urn: Option<&str>,
) -> BTreeSet<String> {
    if roots.is_empty() {
        return BTreeSet::new();
    }
    let forked: BTreeSet<String> = crate::query::walk_edges(
        graph,
        roots,
        &BTreeSet::from([Relation::Requires, Relation::Suggests]),
        None,
    );
    let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for edge in graph.edges() {
        if edge.relation() == Relation::Scope && edge.source().starts_with("action:") {
            owners
                .entry(edge.target())
                .or_default()
                .insert(edge.source());
        }
    }
    forked
        .into_iter()
        .filter(|urn| {
            let kind = urn
                .split_once(':')
                .and_then(|(prefix, _)| NodeKind::from_prefix(prefix));
            matches!(
                kind,
                Some(
                    NodeKind::Directive
                        | NodeKind::Tactic
                        | NodeKind::Styleguide
                        | NodeKind::Toolguide
                )
            )
        })
        .filter(|urn| match owners.get(urn.as_str()) {
            None => true,
            Some(sources) => action_urn.is_some_and(|action| sources.contains(action)),
        })
        .collect()
}

/// The result of one project-governance directive resolution.
struct DirectiveResolution {
    entries: Vec<DirectiveEntry>,
    source: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

/// Whether the reference's charter-config reads can run for a project root.
///
/// The reference (`charter.activation.sync::_charter_read_root`) anchors
/// `load_directives_config`, `load_governance_config` and
/// `resolve_project_governance` on
/// `charter.resolution.resolve_canonical_repo_root`, whose primitive is
/// `kernel.git_topology.git_common_dir` — the same canonical kernel seam this
/// crate already uses for lock placement, not a second Git resolver. Outside a
/// Git repository that facade raises `NotInsideRepositoryError`.
///
/// `context_json._load_project_directives_with_diagnostics` catches resolver
/// failures broadly, and `_load_governance_charter_config` is best-effort with
/// an equally broad catch. An ordinary non-repository and a genuine topology
/// failure therefore take the SAME documented degrade path in the reference;
/// the variants stay distinct here only so a later caller cannot mistake a
/// discovery failure for "this project simply is not a Git checkout".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CharterConfigRead {
    /// The canonical root resolved; the charter-config reads run normally.
    Available,
    /// The root is not inside a Git repository (the ordinary non-Git input).
    NotARepository,
    /// The canonical root could not be discovered for another reason.
    Unavailable,
}

impl CharterConfigRead {
    fn is_available(self) -> bool {
        self == Self::Available
    }
}

fn charter_config_read(project_root: &Path) -> CharterConfigRead {
    match sk_kernel::git_topology::git_common_dir(project_root) {
        Ok(_) => CharterConfigRead::Available,
        Err(sk_kernel::KernelError::NotAGitRepository { .. }) => CharterConfigRead::NotARepository,
        Err(_) => CharterConfigRead::Unavailable,
    }
}

/// Read the authored charter sections for a project root.
///
/// The action build's broad catch maps an unreadable file and a retired pack
/// field to its documented degrade; the retired `governance.doctrine` key stays
/// a typed hard refusal. The project resolver classifies the same outcomes
/// through its own typed error, so neither caller invents provenance.
fn charter_sections(
    project_root: &Path,
    resolution_available: bool,
) -> Result<CharterConfigSections, DoctrineError> {
    if !resolution_available {
        // Every reference charter-config read shares the canonical-root gate,
        // so a non-Git root degrades instead of refusing: nothing was loaded.
        return Ok(CharterConfigSections {
            charter: None,
            governance_valid: false,
            directives_valid: false,
            path: project_root.join(sk_charter::CHARTER_YAML),
        });
    }
    match read_charter_sections(project_root) {
        Ok(sections) => Ok(sections),
        Err(CharterSectionsError::Unreadable { .. })
        | Err(CharterSectionsError::RetiredPackField { .. }) => Ok(CharterConfigSections {
            charter: None,
            governance_valid: false,
            directives_valid: false,
            path: project_root.join(sk_charter::CHARTER_YAML),
        }),
        Err(CharterSectionsError::RetiredGovernanceSelection { path }) => {
            Err(DoctrineError::RetiredGovernanceSelection {
                path,
                runbook: crate::error::RETIRED_GOVERNANCE_RUNBOOK,
            })
        }
    }
}

/// Resolve the effective directive set and its provenance label.
///
/// Mirrors the reference resolver: the base is the activated set (or the
/// built-in catalog default when the key is absent), the charter's
/// `selected_directives` union onto it (`+charter`), and the project-local
/// `charter.yaml` `directives:` section unions on last (`+project_local`).
/// Unknown selections are dropped with a diagnostic instead of erasing the
/// resolution.
///
/// A shape failure in either charter section is the reference's
/// resolver-failure degrade: the DIRECTLY LOADED local directives are returned
/// with a `null` source label and NO diagnostics (never the activation base
/// plus an invented provenance). A retired `governance.doctrine` key was
/// already refused in [`charter_sections`].
fn resolve_directives(
    graph: &DrgGraph,
    identity: &IdentityIndex,
    records: &ActionRecords,
    ledger: &ActivationLedger,
    state: &ActivationState,
    sections: &CharterConfigSections,
    assemble_entries: bool,
) -> Result<DirectiveResolution, DoctrineError> {
    // The graph's built-in provenance is this caller's catalogue source; the
    // selection/union/diagnostic policy is the one shared owner.
    let catalog_ids: BTreeSet<String> = graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::Directive && node.provenance() == Some("built-in"))
        .map(|node| crate::org::normalize_directive_id(&bare_id(node.urn())))
        .collect();
    match resolve_directive_selection(
        sections,
        &catalog_ids,
        identity,
        ledger,
        state.deny_for(ArtifactKind::Directive),
    ) {
        DirectiveSelectionOutcome::Degraded { local } => Ok(DirectiveResolution {
            entries: if assemble_entries {
                local
                    .iter()
                    .map(|entry| local_directive_entry(&entry.id, entry))
                    .collect()
            } else {
                Vec::new()
            },
            source: None,
            diagnostics: Vec::new(),
        }),
        DirectiveSelectionOutcome::Resolved {
            ids,
            source,
            diagnostics,
        } => {
            // The reference text stage never resolves `all_directives`; only the
            // JSON builder does. Skipping the DTO assembly keeps a text-stage
            // run from paying record lookups the reference never performs.
            if !assemble_entries {
                return Ok(DirectiveResolution {
                    entries: Vec::new(),
                    source: Some(source),
                    diagnostics,
                });
            }
            let local = sections.local_directives();
            let local_by_id: BTreeMap<&str, &LocalDirective> = local
                .iter()
                .map(|entry| (entry.id.as_str(), entry))
                .collect();
            let entries = ids
                .iter()
                .map(|id| match local_by_id.get(id.as_str()) {
                    Some(local) => Ok(local_directive_entry(id, local)),
                    None => {
                        // The reference reads the directive model's
                        // `title`/`intent` properties here, never its JSON-mode
                        // body; a bad unrelated unstructured member must not
                        // break `all_directives`.
                        let meta = records
                            .meta(identity, ArtifactKind::Directive, id)
                            .map_err(progressive_delivery)?;
                        Ok(DirectiveEntry {
                            id: id.clone(),
                            source: identity
                                .source_layer(ArtifactKind::Directive, id)
                                .map(IdentityLayer::as_str)
                                .unwrap_or("built-in")
                                .to_owned(),
                            title: meta.title,
                            summary: meta.summary,
                        })
                    }
                })
                .collect::<Result<Vec<_>, DoctrineError>>()?;
            Ok(DirectiveResolution {
                entries,
                source: Some(source),
                diagnostics,
            })
        }
    }
}

/// The reference `_local_directive_entry` DTO: id + `project` origin + the
/// authored display fields (never a title fallback).
fn local_directive_entry(id: &str, local: &LocalDirective) -> DirectiveEntry {
    DirectiveEntry {
        id: id.to_owned(),
        source: "project".to_owned(),
        title: local.title.as_deref().map(PythonString::from_scalars),
        summary: local.description.as_deref().map(PythonString::from_scalars),
    }
}

fn project_charter_block(project_root: &Path) -> ProjectCharterBlock {
    let charter_yaml = project_root.join(sk_charter::CHARTER_YAML);
    let charter_md = project_root.join(sk_charter::CHARTER_MD);
    let present = charter_yaml.is_file();
    let mut block = ProjectCharterBlock {
        present,
        path: sk_charter::CHARTER_YAML.to_owned(),
        charter_md_present: charter_md.is_file(),
        charter_md_path: sk_charter::CHARTER_MD.to_owned(),
        ..ProjectCharterBlock::default()
    };
    if !present {
        return block;
    }
    block.bytes = std::fs::metadata(&charter_yaml).ok().map(|meta| meta.len());
    let Ok(metadata_path) =
        sk_kernel::resolve_within(project_root, Path::new(".kittify/charter/metadata.yaml"))
    else {
        return block;
    };
    let Ok(text) = std::fs::read_to_string(&metadata_path) else {
        return block;
    };
    let Ok(metadata) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
        return block;
    };
    block.hash = metadata
        .get("charter_hash")
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    block.source_path = metadata
        .get("source_path")
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    block.bundle_schema_version = metadata
        .get("bundle_schema_version")
        .and_then(serde_yaml_ng::Value::as_i64);
    block.schema_version = metadata
        .get("schema_version")
        .and_then(serde_yaml_ng::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    block
}

/// Import the configured org packs' `org-charter.yaml` policies into the JSON
/// block shape (the reference `load_org_charter_json_block`).
pub fn org_charter_block(project_root: &Path) -> OrgCharterBlock {
    let Ok(roots) = org_charter::configured_org_roots(project_root) else {
        return OrgCharterBlock::default();
    };
    let mut packs = Vec::new();
    for (declared, root) in roots {
        if !root.is_dir() {
            continue;
        }
        let Ok(Some(policy)) = load_org_charter_policy(&root) else {
            continue;
        };
        packs.push(OrgCharterPack {
            // The reference falls back to the pack DIRECTORY basename
            // (`getattr(policy, "org_name", None) or org_root.name`), never the
            // configured display name. `org_name` truthiness is Python `str`
            // truthiness (non-empty), and the chosen name keeps its exact code
            // points; only a non-UTF-8 directory name has no exact `str` spelling
            // and keeps the previous scalar fallback.
            pack_name: ExactText::new(
                policy
                    .org_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        PythonString::from_scalars(
                            root.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or(declared.as_str()),
                        )
                    }),
            ),
            governance_policies: policy
                .governance_policies
                .iter()
                .map(|policy| OrgGovernancePolicy {
                    field: ExactText::new(policy.field.clone()),
                    value: policy.value.clone(),
                    enforcement: ExactText::new(policy.enforcement.clone()),
                    source: "org".to_owned(),
                })
                .collect(),
            // The reference emits the authored list unchanged (`list(...)`), so an
            // id carrying a lone surrogate stays in the payload instead of being
            // dropped by a scalar filter.
            required_directives: policy
                .required_for(ArtifactKind::Directive)
                .iter()
                .cloned()
                .map(ExactText::new)
                .collect(),
        });
    }
    OrgCharterBlock {
        present: !packs.is_empty(),
        packs,
    }
}

fn governance_reference_status(
    project_root: &Path,
    governance_valid: bool,
) -> Vec<GovernanceReferenceStatus> {
    // The reference's `_load_governance_charter_config` is best-effort: it
    // swallows the canonical-root failure AND every malformed-governance
    // shape, collapsing to the default (empty) selection. Only the retired
    // shapes (a `governance.doctrine` key or a retired pack field) re-raise —
    // and the retired key was already refused in `charter_sections`. So a
    // non-Git root and a malformed governance section both report no
    // governance references, even when `charter.yaml` carries them on disk;
    // the written file is still described by `project_charter`, which reads
    // the root directly.
    if !governance_valid || !charter_config_read(project_root).is_available() {
        return Vec::new();
    }
    let (CharterDocument::Parsed(charter), _) = read_charter_document(project_root) else {
        return Vec::new();
    };
    let references = charter
        .get("governance")
        .and_then(|governance| governance.get("charter"))
        .and_then(|selection| selection.get("governance_references"))
        .and_then(|value| value.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(serde_yaml_ng::Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let mut statuses = Vec::new();
    for raw in references {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        let relative = Path::new(path);
        let unsafe_reason = if relative.is_absolute() {
            Some(format!(
                "Unsafe governance reference {path}: paths must be repository-relative, not absolute."
            ))
        } else if relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            Some(format!(
                "Unsafe governance reference {path}: parent-directory traversal is not allowed."
            ))
        } else {
            let candidate = root.join(relative);
            let resolved = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            (!resolved.starts_with(&root)).then(|| {
                format!(
                    "Unsafe governance reference {path}: resolved path escapes the repository root."
                )
            })
        };
        if let Some(warning) = unsafe_reason {
            statuses.push(GovernanceReferenceStatus {
                path: path.to_owned(),
                exists: false,
                safe: false,
                warning: Some(warning),
            });
            continue;
        }
        let candidate = root.join(relative);
        let exists = candidate.exists();
        let warning = if !exists {
            Some(format!(
                "Missing governance reference {path}. Create it under the repository root or remove it from governance_references in .kittify/charter/charter.md."
            ))
        } else if !candidate.is_file() {
            Some(format!(
                "Governance reference {path} is not a file. Point governance_references at a markdown or text document under the repository root."
            ))
        } else {
            None
        };
        statuses.push(GovernanceReferenceStatus {
            path: path.to_owned(),
            exists,
            safe: true,
            warning,
        });
    }
    statuses
}

/// Normalize an action token to a full `action:<mission>/<step>` URN.
pub fn resolve_action_urn(
    graph: &DrgGraph,
    action: &str,
    mission_type: Option<&str>,
) -> Result<String, DoctrineError> {
    if action.starts_with("action:") {
        if !is_valid_urn(action) {
            return Err(DoctrineError::InvalidAction {
                action: action.to_owned(),
            });
        }
        return Ok(action.to_owned());
    }
    if action.contains(':') || action.trim().is_empty() {
        return Err(DoctrineError::InvalidAction {
            action: action.to_owned(),
        });
    }
    if let Some(mission_type) = mission_type {
        let urn = format!("action:{mission_type}/{action}");
        if !is_valid_urn(&urn) {
            return Err(DoctrineError::InvalidAction {
                action: action.to_owned(),
            });
        }
        return Ok(urn);
    }
    let matches: Vec<String> = graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == NodeKind::Action)
        .filter(|node| {
            let id = node.urn().split_once(':').map(|(_, id)| id).unwrap_or("");
            id.split('/').next_back() == Some(action)
        })
        .map(|node| node.urn().to_owned())
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(DoctrineError::InvalidAction {
            action: action.to_owned(),
        }),
        _ => Err(DoctrineError::InvalidAction {
            action: format!(
                "{action:?} is ambiguous without --mission-type; candidates: {}",
                matches.join(", ")
            ),
        }),
    }
}

/// The pack roots and identity index a context build needs.
pub fn context_roots(
    project_root: &Path,
) -> Result<(PathBuf, Vec<PathBuf>, Option<PathBuf>), DoctrineError> {
    context_roots_with_packs_root(project_root, org_charter::packs_root_override().as_deref())
}

/// [`context_roots`] resolved against an explicit packs-root override.
///
/// `packs_root` accepts the documented parent (`<...>/packs`) or the built-in
/// leaf (`<...>/packs/built-in`), normalized through the one shared helper, so
/// the context build and the org required floor always resolve the same tiers.
pub fn context_roots_with_packs_root(
    project_root: &Path,
    packs_root: Option<&Path>,
) -> Result<(PathBuf, Vec<PathBuf>, Option<PathBuf>), DoctrineError> {
    let config = sk_charter::load_project_config(project_root, &sk_charter::Limits::default())?;
    let expansion = sk_charter::PathExpansion::from_env();
    let search = match packs_root {
        Some(root) => sk_charter::BuiltInSearch::from_env(project_root)
            .with_packs_root_env(org_charter::normalize_packs_root(root)),
        None => org_charter::built_in_search(project_root),
    };
    let roots = sk_charter::PackRoots::resolve(project_root, &config, &expansion, Some(&search))?;
    let built_in = roots
        .built_in
        .ok_or(DoctrineError::PackRootUnavailable { tier: "built-in" })?;
    let org = roots
        .organizations
        .iter()
        .map(|pack| pack.root.clone())
        .collect();
    Ok((built_in, org, roots.project))
}

/// Discover the artifact kinds the context contract currently delivers.
pub fn typed_kinds() -> [ArtifactKind; 5] {
    [
        ArtifactKind::Directive,
        ArtifactKind::Tactic,
        ArtifactKind::Styleguide,
        ArtifactKind::Toolguide,
        ArtifactKind::Procedure,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DrgEdge, DrgNode};
    use crate::identity::{IdentityIndex, scan_all};
    use crate::profile::ProfileRepository;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Make `root` a real Git checkout.
    ///
    /// The reference resolves `charter.yaml` through
    /// `charter.resolution.resolve_canonical_repo_root`, so the directive
    /// resolution, its provenance label and the governance-reference status
    /// are Git-dependent contracts: a bare temporary directory would exercise
    /// the documented non-Git degrade path instead. Tests that assert those
    /// metadata blocks need the realistic setup; deny/delivery assertions do
    /// not depend on it and stay unchanged.
    fn git_root(root: &Path) {
        let output = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .output()
            .expect("run git init");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn profile_yaml(id: &str) -> String {
        format!(
            "profile-id: {id}\nname: {id}\npurpose: review\nroles: [reviewer]\nspecialization:\n  primary-focus: review\n"
        )
    }

    fn graph() -> DrgGraph {
        DrgGraph::new(
            "1.0",
            "x",
            "built-in",
            vec![
                DrgNode::new(
                    "action:software-dev/implement",
                    NodeKind::Action,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "directive:DIRECTIVE_001",
                    NodeKind::Directive,
                    Some("Arch".to_owned()),
                    Vec::new(),
                )
                .unwrap()
                .with_provenance(Some("built-in".to_owned())),
                DrgNode::new(
                    "directive:DIRECTIVE_002",
                    NodeKind::Directive,
                    Some("Style".to_owned()),
                    Vec::new(),
                )
                .unwrap()
                .with_provenance(Some("built-in".to_owned())),
                DrgNode::new("tactic:hard", NodeKind::Tactic, None, Vec::new())
                    .unwrap()
                    .with_provenance(Some("built-in".to_owned())),
                DrgNode::new("tactic:soft", NodeKind::Tactic, None, Vec::new())
                    .unwrap()
                    .with_provenance(Some("built-in".to_owned())),
                DrgNode::new(
                    "glossary_scope:terms",
                    NodeKind::GlossaryScope,
                    None,
                    Vec::new(),
                )
                .unwrap(),
                DrgNode::new(
                    "agent_profile:reviewer",
                    NodeKind::AgentProfile,
                    None,
                    Vec::new(),
                )
                .unwrap(),
            ],
            vec![
                DrgEdge::new(
                    "action:software-dev/implement",
                    "directive:DIRECTIVE_001",
                    Relation::Scope,
                )
                .unwrap(),
                DrgEdge::new("directive:DIRECTIVE_001", "tactic:hard", Relation::Requires).unwrap(),
                DrgEdge::new("directive:DIRECTIVE_001", "tactic:soft", Relation::Suggests)
                    .unwrap()
                    .with_when("risky change"),
                DrgEdge::new(
                    "directive:DIRECTIVE_001",
                    "glossary_scope:terms",
                    Relation::Vocabulary,
                )
                .unwrap(),
                DrgEdge::new("agent_profile:reviewer", "tactic:soft", Relation::Suggests)
                    .unwrap()
                    .with_when("risky change"),
            ],
        )
        .unwrap()
    }

    fn inputs<'a>(
        graph: &'a DrgGraph,
        identity: &'a IdentityIndex,
        ledger: &'a ActivationLedger,
        profiles: &'a ProfileRepository,
        root: &'a Path,
        floor: &'a BTreeMap<ArtifactKind, Vec<String>>,
    ) -> ContextInputs<'a> {
        ContextInputs {
            graph,
            ledger,
            identity,
            profiles,
            project_root: root,
            required_floor: floor,
            project_warnings: &[],
        }
    }

    fn fixture(root: &Path) -> IdentityIndex {
        let built_in = root.join("packs/built-in");
        // Model-admitted documents: the action payload now reads the repository
        // record (the merged model), so a fixture the canonical Directive/Tactic
        // model refuses would be skipped exactly as the reference skips it.
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\nschema_version: '1.0'\ntitle: Arch\nintent: Keep seams\nenforcement: required\nscope: x\n",
        );
        write(
            &built_in.join("directives/002-style.directive.yaml"),
            "id: DIRECTIVE_002\nschema_version: '1.0'\ntitle: Style\nintent: Be clear\nenforcement: advisory\n",
        );
        write(
            &built_in.join("tactics/hard.tactic.yaml"),
            "id: hard\nschema_version: '1.0'\nname: Hard\npurpose: do it\nsteps:\n  - title: one\n",
        );
        write(
            &built_in.join("tactics/soft.tactic.yaml"),
            "id: soft\nschema_version: '1.0'\nname: Soft\npurpose: maybe\nsteps:\n  - title: one\n",
        );
        let mut identity = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut identity);
        identity
    }

    #[test]
    fn context_is_deterministic_and_separates_inline_from_link() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        // An explicit selection keeps the catalogue defaults out of the seed
        // set, so the `suggests`-reached tactic is a link, not a root.
        let mut ledger = ActivationLedger::default();
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(vec!["001-arch".to_owned()]));
        ledger
            .allow
            .insert(ArtifactKind::Tactic, Some(vec!["hard".to_owned()]));
        let floor = BTreeMap::new();
        let graph = graph();
        let request = ContextRequest::new("action:software-dev/implement");
        let first = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        let second = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        assert_eq!(first.to_json(), second.to_json());
        assert_eq!(first.mode, ContextMode::Bootstrap);
        assert_eq!(first.directives.len(), 1);
        assert_eq!(first.directives[0].delivery, Delivery::Inline);
        assert_eq!(
            first.directives[0]
                .title
                .as_ref()
                .and_then(|text| text.to_scalar_string().ok()),
            Some("Arch".to_owned())
        );
        assert_eq!(
            first.directives[0]
                .summary
                .as_ref()
                .and_then(|text| text.to_scalar_string().ok()),
            Some("Keep seams".to_owned())
        );
        // The inline body is the merged record's canonical model dump.
        assert_eq!(
            first.directives[0]
                .body
                .as_ref()
                .and_then(|body| body.field("scope"))
                .and_then(|value| match value {
                    PythonJsonValue::String(text) => text.to_scalar_string().ok(),
                    _ => None,
                }),
            Some("x".to_owned())
        );
        // The explicit tactic allow list gates `soft` out of the filtered graph,
        // so the `suggests` edge to it is not delivered at all.
        assert_eq!(first.tactics.len(), 1);
        assert_eq!(first.tactics[0].id, "hard");
        assert_eq!(first.tactics[0].delivery, Delivery::Inline);
        assert!(first.tactics[0].body.is_some());
        // The top-level link set names only *delivered* targets. A glossary
        // scope reached by `vocabulary` is not a delivered bundle artifact in the
        // reference (the legacy scope kind is excluded from the delivery table),
        // so it is not named here either.
        assert!(first.references.iter().all(|reference| {
            first
                .tactics
                .iter()
                .chain(first.directives.iter())
                .any(|entry| entry.id == reference.id)
        }));
        for key in first.to_json().as_object().unwrap().keys() {
            assert!(CONTEXT_CONTRACT_TOP_LEVEL_KEYS.contains(&key.as_str()));
        }
        let entry = &first.to_json()["directives"][0];
        let mut keys: Vec<&str> = entry
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "body",
                "delivery",
                "id",
                "references",
                "source",
                "summary",
                "title"
            ]
        );
    }

    #[test]
    fn include_all_inlines_suggests_and_profile_channel() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        write(
            &root.join("packs/built-in/agent_profiles/reviewer.agent.yaml"),
            &profile_yaml("reviewer"),
        );
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let graph = graph();
        let request = ContextRequest {
            action: "action:software-dev/implement",
            mission_type: None,
            depth: 1,
            profile: Some("reviewer"),
            include_all: true,
        };
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        let soft = context
            .tactics
            .iter()
            .find(|entry| entry.id == "soft")
            .unwrap();
        assert_eq!(soft.delivery, Delivery::Inline);
        assert!(soft.body.is_some());
        assert_eq!(context.profile_links.len(), 1);
        assert_eq!(
            context.profile_links[0].when.as_deref(),
            Some("risky change")
        );
    }

    #[test]
    fn bare_bootstrap_action_succeeds_without_a_unique_node() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        // `directives_source`/`all_directives` are Git-dependent reference
        // contracts (see `git_root`).
        git_root(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let mut graph = graph();
        let nodes = graph.nodes().to_vec();
        let edges = graph.edges().to_vec();
        let mut nodes = nodes;
        nodes.push(DrgNode::new("action:plan/review", NodeKind::Action, None, Vec::new()).unwrap());
        nodes.push(
            DrgNode::new(
                "action:software-dev/review",
                NodeKind::Action,
                None,
                Vec::new(),
            )
            .unwrap(),
        );
        graph = DrgGraph::new("1.0", "x", "t", nodes, edges).unwrap();
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("review"),
        )
        .unwrap();
        assert_eq!(context.mode, ContextMode::Bootstrap);
        assert_eq!(context.action, "review");
        assert!(context.directives.is_empty());
        // Governance blocks are still resolved.
        assert!(context.directives_source.is_some());
        assert!(!context.all_directives.is_empty());
    }

    #[test]
    fn activation_filter_removes_deactivated_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let mut ledger = ActivationLedger::default();
        ledger
            .deny
            .insert(ArtifactKind::Tactic, vec!["hard".to_owned()]);
        let floor = BTreeMap::new();
        let graph = graph();
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:software-dev/implement"),
        )
        .unwrap();
        assert!(context.tactics.iter().all(|entry| entry.id != "hard"));
    }

    #[test]
    fn malformed_and_unknown_profiles_are_diagnosed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        write(
            &root.join("packs/built-in/agent_profiles/reviewer.agent.yaml"),
            &profile_yaml("reviewer"),
        );
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let graph = graph();
        let request = ContextRequest {
            action: "action:software-dev/implement",
            mission_type: None,
            depth: 1,
            profile: Some("nobody"),
            include_all: false,
        };
        let context = build_action_context(
            &inputs(&graph, &identity, &ledger, &profiles, root, &floor),
            &request,
        )
        .unwrap();
        assert!(
            context
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UnresolvedReference)
        );
    }

    #[test]
    fn compact_mode_carries_no_action_grain_but_keeps_governance() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        // The governance provenance asserted below resolves only through the
        // reference's canonical (Git) root gate.
        git_root(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let ledger = ActivationLedger::default();
        let floor = BTreeMap::new();
        let context = build_action_context(
            &inputs(&graph(), &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:research/unlisted"),
        )
        .unwrap();
        assert_eq!(context.mode, ContextMode::Compact);
        assert!(context.directives.is_empty());
        assert!(context.references.is_empty());
        assert!(!context.all_directives.is_empty());
        assert_eq!(
            context.directives_source.as_deref(),
            Some("catalog_fallback")
        );
    }

    #[test]
    fn project_governance_labels_and_local_directives() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        // The charter selection/local-directive union is a Git-dependent
        // reference contract (see `git_root`).
        git_root(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let mut ledger = ActivationLedger::default();
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(vec!["001-arch".to_owned()]));
        let floor = BTreeMap::new();
        write(
            &root.join(sk_charter::CHARTER_YAML),
            "governance:\n  charter:\n    selected_directives: [DIRECTIVE_002, NOPE]\ndirectives:\n  directives:\n    - id: DIRECTIVE_009\n      title: Local\n      description: Authored here\n",
        );
        let context = build_action_context(
            &inputs(&graph(), &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:software-dev/implement"),
        )
        .unwrap();
        assert_eq!(
            context.directives_source.as_deref(),
            Some("activation+charter+project_local")
        );
        let ids: Vec<&str> = context
            .all_directives
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["DIRECTIVE_001", "DIRECTIVE_002", "DIRECTIVE_009"]);
        let local = context
            .all_directives
            .iter()
            .find(|entry| entry.id == "DIRECTIVE_009")
            .unwrap();
        assert_eq!(local.source, "project");
        assert_eq!(
            local
                .title
                .as_ref()
                .and_then(|text| text.to_scalar_string().ok()),
            Some("Local".to_owned())
        );
        assert!(
            context
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UnknownSelectionDropped)
        );
    }

    #[test]
    fn project_charter_block_always_carries_paths_and_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let block = project_charter_block(root);
        assert!(!block.present);
        assert_eq!(block.path, sk_charter::CHARTER_YAML);
        assert_eq!(block.charter_md_path, sk_charter::CHARTER_MD);

        write(&root.join(sk_charter::CHARTER_YAML), "charter:\n  x: 1\n");
        write(
            &root.join(".kittify/charter/metadata.yaml"),
            "charter_hash: sha256:abc\nsource_path: .kittify/charter/charter.md\nbundle_schema_version: 2\nschema_version: 1.0.0\n",
        );
        let block = project_charter_block(root);
        assert!(block.present);
        assert_eq!(block.bytes, Some(16));
        assert_eq!(block.hash.as_deref(), Some("sha256:abc"));
        assert_eq!(
            block.source_path.as_deref(),
            Some(".kittify/charter/charter.md")
        );
        assert_eq!(block.bundle_schema_version, Some(2));
        assert_eq!(block.schema_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn org_charter_block_summarises_configured_packs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let pack = root.join("packs/acme");
        write(
            &pack.join(org_charter::ORG_CHARTER),
            "org_name: Acme\nrequired_directives: [DIRECTIVE_001]\ngovernance_policies:\n  - field: commits\n    value: signed\n",
        );
        write(
            &root.join("packs/built-in/pack.yaml"),
            "name: built-in\npack_level: built-in\n",
        );
        write(
            &root.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                pack.display()
            ),
        );
        let block = org_charter_block(root);
        assert!(block.present);
        assert_eq!(block.packs.len(), 1);
        assert_eq!(
            block.packs[0]
                .pack_name
                .as_python()
                .to_scalar_string()
                .unwrap(),
            "Acme"
        );
        assert_eq!(
            block.packs[0]
                .required_directives
                .iter()
                .map(|id| id.as_python().to_scalar_string().unwrap())
                .collect::<Vec<_>>(),
            vec!["DIRECTIVE_001".to_owned()]
        );
        assert_eq!(block.packs[0].governance_policies[0].source, "org");
        assert_eq!(
            block.packs[0].governance_policies[0].value,
            AuthoredScalar::Text(PythonString::from_scalars("signed"))
        );
        let payload = block.to_json_exact();
        let PythonJsonValue::Object(fields) = &payload else {
            panic!("the block is an object");
        };
        assert_eq!(fields[0].0, "present");
        let PythonJsonValue::Array(packs) = &fields[1].1 else {
            panic!("packs is a list");
        };
        let PythonJsonValue::Object(entry) = &packs[0] else {
            panic!("a pack is an object");
        };
        let keys: Vec<&str> = entry.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            ["pack_name", "governance_policies", "required_directives"]
        );
        assert_eq!(
            entry[0].1,
            PythonJsonValue::String(PythonString::from_scalars("Acme"))
        );
        let PythonJsonValue::Array(policies) = &entry[1].1 else {
            panic!("governance_policies is a list");
        };
        let PythonJsonValue::Object(policy) = &policies[0] else {
            panic!("a policy is an object");
        };
        let policy_keys: Vec<&str> = policy.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(policy_keys, ["field", "value", "enforcement", "source"]);
        assert_eq!(
            policy[1].1,
            PythonJsonValue::String(PythonString::from_scalars("signed")),
            "an authored string stays a JSON string"
        );
        assert_eq!(policy[3].1, PythonJsonValue::scalar("org"));
    }

    #[test]
    fn org_charter_block_keeps_authored_booleans_and_exact_ids() {
        // Probed against the pinned Source: the block's `value` is the reference
        // `model_dump()` value, so an authored `true` is a JSON boolean and is
        // never collapsed into the string `'True'`; `required_directives` is the
        // authored list unchanged, so an id holding a lone surrogate survives.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let pack = root.join("packs/acme");
        write(
            &pack.join(org_charter::ORG_CHARTER),
            "governance_policies:\n  - {field: f, value: true}\n  - {field: g, value: 'True'}\n  - {field: h, value: false}\nrequired_directives: [\"author\\ud800\", ok-id]\n",
        );
        write(
            &root.join("packs/built-in/pack.yaml"),
            "name: built-in\npack_level: built-in\n",
        );
        write(
            &root.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                pack.display()
            ),
        );
        let block = org_charter_block(root);
        let values: Vec<AuthoredScalar> = block.packs[0]
            .governance_policies
            .iter()
            .map(|policy| policy.value.clone())
            .collect();
        assert_eq!(
            values,
            vec![
                AuthoredScalar::Bool(true),
                AuthoredScalar::Text(PythonString::from_scalars("True")),
                AuthoredScalar::Bool(false),
            ],
            "authored bools stay bools and the text 'True' stays text"
        );
        assert_eq!(
            block.packs[0].required_directives[0]
                .as_python()
                .codepoints(),
            &[0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800],
            "the surrogate-bearing id is not dropped"
        );

        let payload = block.to_json_exact();
        let rendered = payload.render_python(None, true);
        let rendered = rendered.to_scalar_string().unwrap();
        assert!(
            rendered.contains("\"value\": true"),
            "the boolean renders as JSON true: {rendered}"
        );
        assert!(
            rendered.contains("\"value\": \"True\""),
            "the text renders as a JSON string: {rendered}"
        );
        assert!(
            rendered.contains("author\\ud800"),
            "the surrogate id renders as a JSON escape: {rendered}"
        );
    }

    #[test]
    fn action_resolution_refuses_unknown_and_ambiguous_tokens() {
        let graph = graph();
        assert_eq!(
            resolve_action_urn(&graph, "implement", None).unwrap(),
            "action:software-dev/implement"
        );
        assert!(matches!(
            resolve_action_urn(&graph, "nope", None),
            Err(DoctrineError::InvalidAction { .. })
        ));
        assert!(matches!(
            resolve_action_urn(&graph, "not a urn", Some("m")),
            Err(DoctrineError::InvalidAction { .. })
        ));
    }

    #[test]
    fn governance_references_report_unsafe_and_missing_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // The reference reads the charter's governance selection through the
        // canonical (Git) root gate; without it the best-effort loader reports
        // no references at all.
        git_root(root);
        write(&root.join("docs/context.md"), "x");
        write(
            &root.join(sk_charter::CHARTER_YAML),
            "governance:\n  charter:\n    governance_references: [docs/context.md, missing.md, ../escape.md, /abs.md]\n",
        );
        let statuses = governance_reference_status(root, true);
        assert_eq!(statuses.len(), 4);
        assert!(statuses[0].exists && statuses[0].safe);
        assert!(!statuses[1].exists && statuses[1].safe);
        assert!(!statuses[2].safe);
        assert!(!statuses[3].safe);
    }

    #[test]
    fn typed_kinds_match_the_five_typed_arrays() {
        let kinds = typed_kinds();
        assert_eq!(kinds.len(), 5);
        assert!(kinds.contains(&ArtifactKind::Procedure));
    }

    #[test]
    fn explicit_empty_directive_allow_denies_directives_but_not_the_tactic_catalogue() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let profiles = ProfileRepository::load(&root.join("packs/built-in"), &[], None).unwrap();
        let mut ledger = ActivationLedger::default();
        // An explicit, present, EMPTY directive allow list deactivates every
        // directive — the one state that stays empty (a distinct state from an
        // absent key). It must not admit the filtered-out directive by falling
        // back to the unfiltered graph.
        ledger
            .allow
            .insert(ArtifactKind::Directive, Some(Vec::new()));
        let floor = BTreeMap::new();
        let context = build_action_context(
            &inputs(&graph(), &identity, &ledger, &profiles, root, &floor),
            &ContextRequest::new("action:software-dev/implement"),
        )
        .unwrap();
        assert!(context.directives.is_empty());
        assert!(
            context
                .references
                .iter()
                .all(|reference| reference.id != "DIRECTIVE_001"
                    && reference.id != "DIRECTIVE_002")
        );
        // The tactic key is ABSENT, and the three-state collapse is per kind:
        // it resolves to the catalogue default, so the built-in tactics are
        // selection roots even though the directive that `requires` them was
        // deactivated. The reference `_seed_sets` computes each kind
        // independently, so asserting them away here would be wrong, not a
        // deny guard.
        let ids: Vec<&str> = context
            .tactics
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["hard", "soft"]);
        assert!(
            context
                .tactics
                .iter()
                .all(|entry| entry.delivery == Delivery::Inline)
        );
    }

    #[test]
    fn context_roots_accept_the_built_in_leaf_packs_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // Not `<ancestor>/packs/built-in`: only the explicit packs root can
        // resolve the built-in tier, so a leaf value must be normalized.
        let packs = root.join("packstore");
        write(
            &packs.join("built-in/pack.yaml"),
            "name: built-in\npack_level: built-in\n",
        );
        let org = packs.join("acme");
        write(&org.join("pack.yaml"), "name: acme\npack_level: org\n");
        write(&org.join("org-charter.yaml"), "org_name: Acme\n");
        write(
            &root.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                org.display()
            ),
        );

        // The CLI adapter accepts either the packs parent or the built-in leaf
        // and normalizes it. The doctrine root read must agree, otherwise the
        // org floor/context sees a different tier than the adapter.
        let (parent_built_in, parent_org, parent_project) =
            context_roots_with_packs_root(root, Some(&packs)).expect("parent root");
        let (leaf_built_in, leaf_org, leaf_project) =
            context_roots_with_packs_root(root, Some(&packs.join("built-in"))).expect("leaf root");
        assert_eq!(leaf_built_in, parent_built_in);
        assert_eq!(leaf_org, parent_org);
        assert_eq!(leaf_project, parent_project);
        assert_eq!(
            leaf_built_in,
            packs.join("built-in").canonicalize().unwrap()
        );
        assert_eq!(leaf_org.len(), 1);
        assert!(leaf_project.is_none());
    }

    #[test]
    fn record_setup_failure_is_not_a_healthy_miss() {
        // An unresolved built-in tier is the reference builder's raising setup
        // phase, never "this artifact is absent". The record source must return
        // the phase and keep returning it (cached), not a healthy `None`.
        let identity = IdentityIndex::from_roots(Path::new(""), &[], None);
        let records = ActionRecords::new(Path::new("/nonexistent-cf61"));
        for _ in 0..2 {
            let error = records
                .meta(&identity, ArtifactKind::Directive, "DIRECTIVE_001")
                .expect_err("setup failure is not a miss");
            assert!(
                matches!(error, ProgressiveDeliveryError::Setup { .. }),
                "unexpected phase: {error:?}"
            );
        }
    }

    #[test]
    fn repository_abort_is_not_a_missing_record() {
        // A constructor abort the reference's per-file catch does not cover
        // aborts the layered repository load. It must surface as a repository
        // phase, not as an entry with no title/body.
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let built_in = root.join("packs/built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "schema_version: '1.0'\nid: DIRECTIVE_001\ntitle: T\nintent: I\nenforcement: required\nscope: x\n",
        );
        write(
            &built_in.join("directives/999-abort.directive.yaml"),
            "schema_version: '1.0'\nid: DIRECTIVE_999\ntitle: T\nintent: I\nenforcement: required\nscope: x\nreferences: !!omap\n  - a: 1\n  - a: 2\n",
        );
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let records = ActionRecords::new(root);
        let error = records
            .meta(&identity, ArtifactKind::Directive, "DIRECTIVE_001")
            .expect_err("the repository abort is not a miss");
        assert!(
            matches!(
                error,
                ProgressiveDeliveryError::Repository {
                    aborted: Some(_),
                    ..
                }
            ),
            "unexpected phase: {error:?}"
        );
    }

    #[test]
    fn unknown_record_key_is_a_healthy_miss() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let identity = fixture(root);
        let records = ActionRecords::new(root);
        let meta = records
            .meta(&identity, ArtifactKind::Directive, "DIRECTIVE_404")
            .expect("a missing id is not a failure");
        assert!(!meta.present);
        assert!(meta.title.is_none());
        assert!(
            records
                .body(&identity, ArtifactKind::Directive, "DIRECTIVE_404")
                .expect("a missing body is not a failure")
                .is_none()
        );
    }
}

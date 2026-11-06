//! The raw SPDD/REASONS activation selector
//! (`charter.offering.spdd_reasons.activation`).
//!
//! One question: *is the SPDD/REASONS charter content active for this project?*
//! The reference answers it from the project's **real activation authority** —
//! the same raw `activated_<kind>` keys `PackContext.from_config` reads, not from
//! the compiled charter and not from a shipped id list — and returns true iff at
//! least one of four selectors is present:
//!
//! * paradigm `structured-prompt-driven-development`;
//! * tactic `reasons-canvas-fill`;
//! * tactic `reasons-canvas-review`;
//! * directive `DIRECTIVE_038` (or a slug carrying the `038` numeric hint).
//!
//! # What this is, and what it is not
//!
//! The reference keeps this reader deliberately independent of the activation
//! engine (`charter.offering` may not import `charter.activation`), and its
//! documented posture is **not** the compiled charter's:
//!
//! * **No org required floor.** A denied id is off; an absent allow key is not
//!   narrowed by an org `required_<plural>` list.
//! * **No `project_configured` gate.** `compile_charter` narrows an omitted kind
//!   to the empty set once a project has configured *any* activation key; this
//!   selector keeps the raw per-kind three-state reading instead.
//! * **Three-state per kind, never a truthiness collapse.** An absent key (or a
//!   YAML `null`) means "every built-in of that kind is available", so the
//!   selector is *admitted*; an explicitly present empty list means "nothing
//!   selected", so it is not. Collapsing those two through `x or set()` is the
//!   exact bug class the reference's comments forbid.
//! * **Deny is ledger-only.** A deny list is read from the activation ledger; a
//!   legacy `deactivated_<kind>` key is never applied (a legacy source denies
//!   nothing). The org floor is not consulted here.
//! * **A missing `.kittify/config.yaml` pins `False`** — the reference's
//!   evidence-based carve-out for the pre-`init` call path — while a present but
//!   *empty* `config.yaml` is the allow-all state and therefore `True`.
//!
//! Malformed input raises: invalid YAML, a non-mapping root, a dangling
//! `charter:` pointer, a present-but-non-list `activated_<kind>` value, and an
//! unreadable ledger all become [`SpddActivationError`] — never a silent
//! `false`/`true`.
//!
//! # Shared authorities
//!
//! Nothing here re-implements activation: the source decision is
//! [`crate::ledger::resolve_activation_source`], the ledger path is
//! [`crate::ledger::activation_ledger_path`], the documents are read through the
//! one constructed-YAML authority ([`crate::offering::ArtifactDocument`]), and
//! `str()` of a list item is the Kernel object graph's own `python_str`. The
//! activation keys themselves are derived from the canonical ledger vocabulary
//! ([`crate::ledger::allow_key`] / [`crate::ledger::deny_key`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;

use crate::ledger::{
    ActivationSource, CONFIG_YAML_REL, ResolvedActivationSource, activation_ledger_path, deny_key,
    resolve_activation_source,
};
use crate::offering::ArtifactDocument;

/// The paradigm selector id.
pub const SPDD_PARADIGM_ID: &str = "structured-prompt-driven-development";
/// The fill-tactic selector id.
pub const SPDD_TACTIC_FILL_ID: &str = "reasons-canvas-fill";
/// The review-tactic selector id.
pub const SPDD_TACTIC_REVIEW_ID: &str = "reasons-canvas-review";
/// The directive selector id.
pub const SPDD_DIRECTIVE_ID: &str = "DIRECTIVE_038";
/// The numeric hint any directive slug may carry instead of the id.
pub const SPDD_DIRECTIVE_NUMERIC_HINT: &str = "038";

/// The reference's `_SpddActivationConfigError`: a `ValueError` whose body is the
/// operator-facing reason the activation source is malformed or unreachable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpddActivationError {
    body: String,
}

impl SpddActivationError {
    /// Capture one reason body.
    #[must_use]
    pub fn new(body: impl Into<String>) -> Self {
        Self { body: body.into() }
    }

    /// The reason, without any prefix.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl std::fmt::Display for SpddActivationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.body)
    }
}

impl std::error::Error for SpddActivationError {}

/// The per-selector answer one resolution pass produced.
///
/// [`SpddActivationSelection::active`] is the reference's single boolean; the
/// individual selectors are exposed so a migration or rescue caller can report
/// *why* the pack is (not) in force without re-reading the activation source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpddActivationSelection {
    /// Where the allow keys were read from, when source resolution was reached.
    /// Missing `config.yaml` returns before consulting either source (`None`).
    pub source: Option<ActivationSource>,
    /// The `charter:` pointer, when the legacy surface names one.
    pub pointer: Option<PathBuf>,
    /// The paradigm selector is admitted.
    pub paradigm: bool,
    /// The `reasons-canvas-fill` tactic selector is admitted.
    pub tactic_fill: bool,
    /// The `reasons-canvas-review` tactic selector is admitted.
    pub tactic_review: bool,
    /// The `DIRECTIVE_038` selector is admitted.
    pub directive: bool,
}

impl SpddActivationSelection {
    /// Whether the SPDD/REASONS pack is active (the reference's disjunction).
    ///
    /// A tactic matches if either tactic id is admitted.
    #[must_use]
    pub fn active(&self) -> bool {
        self.paradigm || self.tactic_fill || self.tactic_review || self.directive
    }

    /// The observed selectors, in the reference's evaluation order.
    #[must_use]
    pub fn observed(&self) -> Vec<&'static str> {
        let mut observed = Vec::new();
        if self.paradigm {
            observed.push(SPDD_PARADIGM_ID);
        }
        if self.tactic_fill {
            observed.push(SPDD_TACTIC_FILL_ID);
        }
        if self.tactic_review {
            observed.push(SPDD_TACTIC_REVIEW_ID);
        }
        if self.directive {
            observed.push(SPDD_DIRECTIVE_ID);
        }
        observed
    }
}

/// Return true iff the SPDD/REASONS pack is active for `repo_root`.
///
/// The single facade the reference exports; [`spdd_activation_selection`] is the
/// same decision with its per-selector evidence.
///
/// # Errors
///
/// [`SpddActivationError`] for malformed or unreachable activation input. A
/// missing `.kittify/config.yaml` is **not** an error: it is the reference's
/// `False`.
pub fn is_spdd_reasons_active(repo_root: &Path) -> Result<bool, SpddActivationError> {
    Ok(spdd_activation_selection(repo_root)?.active())
}

/// The SPDD/REASONS decision with its per-selector evidence.
///
/// # Errors
///
/// [`SpddActivationError`] for malformed or unreachable activation input.
pub fn spdd_activation_selection(
    repo_root: &Path,
) -> Result<SpddActivationSelection, SpddActivationError> {
    let config_path = repo_root.join(CONFIG_YAML_REL);
    if !config_path.exists() {
        // The reference's FR-004 carve-out: an absent `.kittify/config.yaml`
        // pins `False` (the renderer can run before `init` writes it).
        return Ok(SpddActivationSelection {
            source: None,
            pointer: None,
            paradigm: false,
            tactic_fill: false,
            tactic_review: false,
            directive: false,
        });
    }

    // The reference reads (and so validates) `config.yaml` before it resolves
    // the source, even when the ledger turns out to be the authority.
    let config = RawMapping::load(&config_path)?;
    let resolved = resolve_activation_source(repo_root)
        .map_err(|error| SpddActivationError::new(error.to_string()))?;
    let (allow, deny) = activation_sources(repo_root, &config, &resolved)?;

    // All three allow keys are read before any deny key or selector decision, so
    // the first malformed key wins exactly as it does in the reference.
    let activated_paradigms = allow.three_state(&allow_key(ArtifactKind::Paradigm))?;
    let activated_tactics = allow.three_state(&allow_key(ArtifactKind::Tactic))?;
    let activated_directives = allow.three_state(&allow_key(ArtifactKind::Directive))?;
    let denied_paradigms = deny.three_state(&deny_key_of(ArtifactKind::Paradigm))?;
    let denied_tactics = deny.three_state(&deny_key_of(ArtifactKind::Tactic))?;
    let denied_directives = deny.three_state(&deny_key_of(ArtifactKind::Directive))?;

    let paradigm = selector_admitted(
        activated_paradigms.as_ref(),
        denied_paradigms.as_ref(),
        |entry| entry == &PythonString::from_scalars(SPDD_PARADIGM_ID),
    );
    let tactic_fill = selector_admitted(
        activated_tactics.as_ref(),
        denied_tactics.as_ref(),
        |entry| entry == &PythonString::from_scalars(SPDD_TACTIC_FILL_ID),
    );
    let tactic_review = selector_admitted(
        activated_tactics.as_ref(),
        denied_tactics.as_ref(),
        |entry| entry == &PythonString::from_scalars(SPDD_TACTIC_REVIEW_ID),
    );
    let directive = selector_admitted(
        activated_directives.as_ref(),
        denied_directives.as_ref(),
        is_directive_038,
    );

    Ok(SpddActivationSelection {
        source: Some(resolved.source()),
        pointer: resolved.pointer.clone(),
        paradigm,
        tactic_fill,
        tactic_review,
        directive,
    })
}

/// The `(allow, deny)` mappings, read from the source the kernel resolver names.
///
/// * `LEDGER` → the raw activation ledger for both halves (`{}` before the first
///   write), so a deny list is honoured.
/// * `LEGACY` → the allow keys follow the pointer rule (no pointer: the parsed
///   `config.yaml`; a pointer: the pointed `charter.yaml`, where a dangling
///   pointer raises), and the deny half is empty: a legacy
///   `deactivated_<kind>` key is never applied.
fn activation_sources(
    repo_root: &Path,
    config: &RawMapping,
    resolved: &ResolvedActivationSource,
) -> Result<(RawMapping, RawMapping), SpddActivationError> {
    if resolved.source() == ActivationSource::Ledger {
        let path = activation_ledger_path(repo_root);
        let ledger = if path.is_file() {
            RawMapping::load(&path)?
        } else {
            // `read_activation_ledger_raw(...) or {}`.
            RawMapping::absent()
        };
        return Ok((ledger.clone(), ledger));
    }
    match resolved.pointer.clone() {
        None => Ok((config.clone(), RawMapping::absent())),
        Some(pointer) => {
            if !resolved.pointer_exists {
                return Err(SpddActivationError::new(format!(
                    ".kittify/config.yaml 'charter:' pointer names {}, which does not exist.",
                    pointer.display()
                )));
            }
            Ok((RawMapping::load(&pointer)?, RawMapping::absent()))
        }
    }
}

/// One top-level activation mapping read through the constructed-YAML authority.
///
/// `absent()` and an empty document are both the reference's `{}`: an absent
/// ledger, an empty file, and a document whose root is YAML `null` are absence,
/// never malformed input.
#[derive(Clone, Debug)]
struct RawMapping {
    document: Option<ArtifactDocument>,
}

impl RawMapping {
    fn absent() -> Self {
        Self { document: None }
    }

    /// Read *path* as one top-level mapping.
    ///
    /// # Errors
    ///
    /// `Invalid YAML in <path>: …` for any read/parse/construct failure, and
    /// `<path> root must be a mapping.` for a non-null, non-mapping root — the
    /// reference's two fail-loud messages.
    fn load(path: &Path) -> Result<Self, SpddActivationError> {
        let document = ArtifactDocument::read(path).map_err(|error| {
            SpddActivationError::new(format!("Invalid YAML in {}: {error}", path.display()))
        })?;
        let root = document.root();
        if !root.is_null() && root.as_mapping().is_none() {
            return Err(SpddActivationError::new(format!(
                "{} root must be a mapping.",
                path.display()
            )));
        }
        Ok(Self {
            document: Some(document),
        })
    }

    /// The three-state reading of one `activated_*` / `deactivated_*` key.
    ///
    /// `None` means the key is absent (or explicitly YAML `null`): "all built-ins
    /// available". `Some(set)` is the explicit selection, which may be empty.
    ///
    /// # Errors
    ///
    /// `Activation key '<key>' must be a list, got <type>.` when the key is
    /// present with a non-list value — never a silent character-by-character
    /// iteration of a bare string.
    fn three_state(
        &self,
        key: &str,
    ) -> Result<Option<BTreeSet<PythonString>>, SpddActivationError> {
        let Some(document) = self.document.as_ref() else {
            return Ok(None);
        };
        let root = document.root();
        let Some(value) = root.lookup(key) else {
            return Ok(None);
        };
        // Python's `data.get(key) is None` covers an explicit YAML null too.
        if value.is_null() {
            return Ok(None);
        }
        let Some(items) = value.as_sequence() else {
            return Err(SpddActivationError::new(format!(
                "Activation key '{key}' must be a list, got {}.",
                value.python_type_name()
            )));
        };
        // `frozenset(str(item) for item in raw)`: the one Kernel `str()`/`repr()`
        // authority, so a numeric, boolean, null or container entry coerces
        // exactly as the reference coerces it.
        Ok(Some(
            items
                .iter()
                .map(|id| document.python_str(root.view(*id)))
                .collect(),
        ))
    }
}

/// The ledger allow key of `kind` (`activated_<plural>`), from the ledger vocabulary.
fn allow_key(kind: ArtifactKind) -> String {
    crate::ledger::allow_key(kind).unwrap_or_default()
}

/// The ledger deny key of `kind` (`deactivated_<plural>`), from the ledger vocabulary.
fn deny_key_of(kind: ArtifactKind) -> String {
    deny_key(kind).unwrap_or_default()
}

/// `allow and not deny` for one selector.
///
/// A denied match wins whatever the allow list says; an **absent** allow list
/// (`None`) admits every selector; a present list admits only what it names.
fn selector_admitted(
    allow: Option<&BTreeSet<PythonString>>,
    deny: Option<&BTreeSet<PythonString>>,
    matches: impl Fn(&PythonString) -> bool,
) -> bool {
    if deny.is_some_and(|entries| entries.iter().any(&matches)) {
        return false;
    }
    allow.is_none_or(|entries| entries.iter().any(&matches))
}

/// `DIRECTIVE_038`, a slug carrying the `038` numeric hint, or its upper-cased id.
fn is_directive_038(raw: &PythonString) -> bool {
    if raw == &PythonString::from_scalars(SPDD_DIRECTIVE_ID) {
        return true;
    }
    if numeric_hint_is_038(raw) {
        return true;
    }
    // `raw.upper() == DIRECTIVE_ID.upper()`.
    raw.to_scalar_string()
        .is_ok_and(|text| text.to_uppercase() == SPDD_DIRECTIVE_ID)
}

/// Whether the leading run of Python decimal digits (`re`'s `\d`), zero-filled to
/// width 3, is `038`.
fn numeric_hint_is_038(raw: &PythonString) -> bool {
    let mut digits: Vec<u32> = Vec::new();
    for point in raw.codepoints() {
        if !sk_kernel::python_text::is_decimal(*point) {
            break;
        }
        digits.push(*point);
    }
    if digits.is_empty() {
        return false;
    }
    let mut filled: Vec<u32> = Vec::new();
    while filled.len() + digits.len() < SPDD_DIRECTIVE_NUMERIC_HINT.len() {
        filled.push(u32::from(b'0'));
    }
    filled.extend(digits);
    filled == PythonString::from_scalars(SPDD_DIRECTIVE_NUMERIC_HINT).codepoints()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(config: Option<&str>, ledger: Option<&str>) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("tempdir");
        let kittify = directory.path().join(".kittify");
        fs::create_dir_all(kittify.join("charter")).expect("dirs");
        if let Some(text) = config {
            fs::write(kittify.join("config.yaml"), text).expect("config");
        }
        if let Some(text) = ledger {
            fs::write(kittify.join("charter/activation.yaml"), text).expect("ledger");
        }
        directory
    }

    #[test]
    fn a_missing_config_pins_false() {
        let directory = project(None, None);
        assert_eq!(is_spdd_reasons_active(directory.path()), Ok(false));
        let selection = spdd_activation_selection(directory.path()).expect("early return");
        assert_eq!(selection.source, None, "neither source was consulted");
    }

    #[test]
    fn a_present_empty_config_is_the_allow_all_state() {
        // No activation key of any kind: every selector's allow list is absent,
        // which the reference reads as "all built-ins available" — not as "off".
        for config in ["", "null\n", "agents:\n  available: [codex]\n"] {
            let directory = project(Some(config), None);
            let selection = spdd_activation_selection(directory.path()).expect("selection");
            assert_eq!(selection.source, Some(ActivationSource::Ledger));
            assert!(selection.active(), "config {config:?}");
            assert!(selection.paradigm && selection.tactic_fill && selection.tactic_review);
        }
    }

    #[test]
    fn an_explicitly_empty_list_selects_nothing_for_that_kind() {
        let directory = project(
            Some("activated_paradigms: []\nactivated_tactics: []\nactivated_directives: []\n"),
            None,
        );
        let selection = spdd_activation_selection(directory.path()).expect("selection");
        assert!(!selection.active());
        assert!(!selection.paradigm && !selection.tactic_fill && !selection.directive);
    }

    #[test]
    fn only_the_named_selector_is_admitted() {
        let directory = project(
            Some(
                "activated_paradigms: []\nactivated_tactics: [reasons-canvas-review]\nactivated_directives: []\n",
            ),
            None,
        );
        let selection = spdd_activation_selection(directory.path()).expect("selection");
        assert!(selection.active());
        assert!(!selection.paradigm);
        assert!(!selection.tactic_fill);
        assert!(selection.tactic_review);
        assert!(!selection.directive);
    }

    #[test]
    fn the_directive_matches_its_id_its_numeric_hint_and_its_slug() {
        for value in [
            "DIRECTIVE_038",
            "038",
            "38",
            "038-structured-prompt-boundary",
            "directive_038",
            // YAML reads a bare `0038` as the integer 38, and the reference
            // coerces list items with `str()`, so it names the selector too.
            "0038",
        ] {
            let directory = project(
                Some(&format!(
                    "activated_paradigms: []\nactivated_tactics: []\nactivated_directives: [{value}]\n"
                )),
                None,
            );
            let selection = spdd_activation_selection(directory.path()).expect("selection");
            assert!(selection.directive, "{value}");
        }
        for value in ["DIRECTIVE_039", "039", "39", "'0038'", "directive-38"] {
            let directory = project(
                Some(&format!(
                    "activated_paradigms: []\nactivated_tactics: []\nactivated_directives: [{value}]\n"
                )),
                None,
            );
            let selection = spdd_activation_selection(directory.path()).expect("selection");
            assert!(!selection.directive, "{value}");
        }
    }

    #[test]
    fn a_ledger_deny_wins_over_an_absent_allow_list() {
        // The ledger is the authority, and its deny list removes the selector even
        // though the absent allow key would otherwise admit everything.
        let directory = project(
            Some("agents: {available: [codex]}\n"),
            Some(
                "deactivated_paradigms: [structured-prompt-driven-development]\ndeactivated_tactics: [reasons-canvas-fill, reasons-canvas-review]\n",
            ),
        );
        let selection = spdd_activation_selection(directory.path()).expect("selection");
        assert_eq!(selection.source, Some(ActivationSource::Ledger));
        assert!(!selection.paradigm && !selection.tactic_fill && !selection.tactic_review);
        // The directive kind is still un-denied and un-narrowed.
        assert!(selection.directive);
    }

    #[test]
    fn a_legacy_source_applies_no_deny_list() {
        // `.kittify/config.yaml` with legacy keys and no ledger resolves LEGACY;
        // a hand-written legacy `deactivated_*` key is never applied there.
        let directory = project(
            Some(
                "activated_paradigms: [structured-prompt-driven-development]\ndeactivated_paradigms: [structured-prompt-driven-development]\n",
            ),
            None,
        );
        let selection = spdd_activation_selection(directory.path()).expect("selection");
        assert_eq!(selection.source, Some(ActivationSource::Legacy));
        assert!(selection.paradigm);
    }

    #[test]
    fn a_present_non_list_activation_key_is_refused_loudly() {
        let directory = project(
            Some("activated_paradigms: structured-prompt-driven-development\n"),
            None,
        );
        let error = spdd_activation_selection(directory.path()).expect_err("refused");
        assert_eq!(
            error.body(),
            "Activation key 'activated_paradigms' must be a list, got str."
        );
    }

    #[test]
    fn malformed_and_non_mapping_documents_are_refused_loudly() {
        let broken = project(Some("a: [broken\n"), None);
        let error = spdd_activation_selection(broken.path()).expect_err("refused");
        assert!(
            error.body().starts_with("Invalid YAML in "),
            "{}",
            error.body()
        );

        let list = project(Some("- a\n- b\n"), None);
        let error = spdd_activation_selection(list.path()).expect_err("refused");
        assert!(
            error.body().ends_with("root must be a mapping."),
            "{}",
            error.body()
        );
    }

    #[test]
    fn a_dangling_legacy_pointer_is_refused() {
        let directory = project(Some("charter: .kittify/charter/missing.yaml\n"), None);
        // Without a ledger and with a pointer that does not exist the resolver still
        // names LEGACY, and this reader refuses the dangling pointer.
        let error = spdd_activation_selection(directory.path()).expect_err("refused");
        assert!(
            error.body().contains("which does not exist."),
            "{}",
            error.body()
        );
    }

    #[test]
    fn list_items_are_coerced_with_the_one_python_str() {
        // `frozenset(str(item) for item in raw)`: a bool and an int coerce to
        // `True` / `1`, so they name no selector but must not be dropped or crash.
        let directory = project(
            Some("activated_paradigms: [true, 1, 1.5]\nactivated_tactics: [reasons-canvas-fill]\n"),
            None,
        );
        let selection = spdd_activation_selection(directory.path()).expect("selection");
        assert!(!selection.paradigm);
        assert!(selection.tactic_fill);
    }
}

//! Catalogue-miss diagnosis — the ONE owner of the structured miss stanza,
//! the difflib-compatible typo classification and the warning policy.
//!
//! Mirrors the reference `charter.activation._catalog_miss` (public helpers) plus
//! `charter.activation.context_renderers.catalog_diagnosis._diagnose_catalog_miss`
//! (the repository-aware entry point). Every renderer that reports a catalogue
//! miss (profile-cited sections here) uses this module instead of inventing a
//! generic missing placeholder or a broader abort.
//!
//! # Exact policy
//!
//! * `classify_catalog_miss` is a faithful port of
//!   `difflib.get_close_matches(missing, candidates, n=1, cutoff=0.75)` over
//!   Python code points, including CPython 3.11's `SequenceMatcher` autojunk
//!   heuristic and stable first-maximum tie selection. It never returns
//!   `SchemaValidationSuspected` (only callers with ground truth construct it).
//! * `diagnose_catalog_miss` checks the repository's own scope-filtered ids
//!   before falling back to the fuzzy classifier; that is what distinguishes a
//!   language-filtered artifact from a typo/absence.
//! * `emit_catalog_miss_warning` mirrors the once-per-process latch and the
//!   quiet `agent_profile` scope-filtered rule. The reference also raises a
//!   Python `UserWarning`; Native has no warnings module, so the caller supplies
//!   a sink and the physical stderr formatting is a disclosed Native boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;

use crate::context_render::CHARTER_EXTENSION_ADVISORY;
use crate::language_scope::lacks_specialist_guidance;
use crate::offering::CanonicalOfferingService;

/// `difflib.get_close_matches`' tightened typo cutoff (the reference's 0.75).
const TYPO_SIMILARITY_CUTOFF: f64 = 0.75;

/// CPython `SequenceMatcher`'s autojunk threshold.
const AUTOJUNK_MINIMUM_LENGTH: usize = 200;

/// Why a catalogue lookup missed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogMissCause {
    /// A close-match candidate exists at or above the typo cutoff.
    TypoSuspected,
    /// No close match; the artifact may never have existed or was schema-dropped.
    MissingArtifact,
    /// Reserved for callers that already know the loader rejected the document.
    SchemaValidationSuspected,
    /// Present on disk but excluded by its `applies_to_languages` scope.
    ScopeFiltered,
}

impl CatalogMissCause {
    /// The reference enum's serialized spelling (the prompt's `Cause:` line).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TypoSuspected => "typo_suspected",
            Self::MissingArtifact => "missing_artifact",
            Self::SchemaValidationSuspected => "schema_validation_suspected",
            Self::ScopeFiltered => "scope_filtered",
        }
    }
}

/// One classified catalogue miss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogMissDiagnosis {
    /// The inferred cause.
    pub cause: CatalogMissCause,
    /// The closest-match id, only for [`CatalogMissCause::TypoSuspected`] and the
    /// scope-filtered remediation hint.
    pub suggestion: Option<PythonString>,
}

impl CatalogMissDiagnosis {
    /// A diagnosis without a suggestion.
    #[must_use]
    pub fn new(cause: CatalogMissCause) -> Self {
        Self {
            cause,
            suggestion: None,
        }
    }
}

/// The process-wide once-per-`(kind, id)` warning latch (the reference `_EMITTED`).
static EMITTED: Mutex<BTreeSet<(String, Vec<u32>)>> = Mutex::new(BTreeSet::new());

/// Clear the once-per-process warning latch (the reference test hook).
pub fn reset_emitted_warnings() {
    if let Ok(mut emitted) = EMITTED.lock() {
        emitted.clear();
    }
}

/// The reference `classify_catalog_miss` over exact code points.
///
/// Candidates are compared with `difflib.SequenceMatcher.ratio()` (no junk,
/// autojunk on) and the reference's `_nlargest(1, [(score, text), ...])` selection
/// is reproduced: on an equal score the Python tuple comparison breaks the tie by
/// the candidate string itself (code-point order, largest wins).
#[must_use]
pub fn classify_catalog_miss(
    missing_id: &PythonString,
    available_ids: &[PythonString],
) -> CatalogMissDiagnosis {
    if available_ids.is_empty() {
        return CatalogMissDiagnosis::new(CatalogMissCause::MissingArtifact);
    }
    let mut best: Option<(f64, &PythonString)> = None;
    for candidate in available_ids {
        // get_close_matches fixes seq2 to the missing word, then assigns
        // each candidate to seq1. SequenceMatcher is intentionally asymmetric.
        let score = sequence_ratio(candidate, missing_id);
        if score < TYPO_SIMILARITY_CUTOFF {
            continue;
        }
        let replace = match &best {
            None => true,
            Some((current_score, current)) => {
                score > *current_score
                    || (score == *current_score && candidate.codepoints() > current.codepoints())
            }
        };
        if replace {
            best = Some((score, candidate));
        }
    }
    match best {
        Some((_, suggestion)) => CatalogMissDiagnosis {
            cause: CatalogMissCause::TypoSuspected,
            suggestion: Some(suggestion.clone()),
        },
        None => CatalogMissDiagnosis::new(CatalogMissCause::MissingArtifact),
    }
}

/// The reference `classify_scope_filtered_miss`.
#[must_use]
pub fn classify_scope_filtered_miss(
    artifact_id: &PythonString,
    active_languages: Option<&[String]>,
    repo_root: Option<&Path>,
) -> CatalogMissDiagnosis {
    let active: Vec<String> = active_languages.map(<[String]>::to_vec).unwrap_or_default();
    let lacking = lacks_specialist_guidance(active_languages, repo_root, None).unwrap_or(false);
    let hint = if lacking {
        let mut text = PythonString::from_scalars("artifact '");
        text.append(artifact_id);
        text.push_scalars(
            "' is scoped to specific languages and does not apply to a project whose language \
             Spec Kitty does not recognise. ",
        );
        text.push_scalars(CHARTER_EXTENSION_ADVISORY);
        text
    } else if !active.is_empty() {
        let mut quoted = PythonString::default();
        for (position, language) in active.iter().enumerate() {
            if position > 0 {
                quoted.push_scalars(", ");
            }
            quoted.push_scalars(&PythonString::from_scalars(language).repr());
        }
        let mut text = PythonString::from_scalars("artifact '");
        text.append(artifact_id);
        text.push_scalars(
            "' is present but its applies_to_languages scope does not include the active language \
             set (",
        );
        text.append(&quoted);
        text.push_scalars(
            "). Add the active language to the artifact's applies_to_languages field, or remove the \
             field to make it always-applicable.",
        );
        text
    } else {
        let mut text = PythonString::from_scalars("artifact '");
        text.append(artifact_id);
        text.push_scalars(
            "' is present but its applies_to_languages scope was filtered out. Check the \
             artifact's applies_to_languages field, or remove it to make the artifact \
             always-applicable.",
        );
        text
    };
    CatalogMissDiagnosis {
        cause: CatalogMissCause::ScopeFiltered,
        suggestion: Some(hint),
    }
}

/// The reference `_diagnose_catalog_miss` over one kind's canonical repository.
///
/// A repository load failure while reading the ids/scope set degrades to the
/// fuzzy classifier's empty catalogue (the reference's best-effort listing).
/// A scope-filtered hit is classified as `scope_filtered` first.
#[must_use]
pub fn diagnose_catalog_miss(
    missing_id: &PythonString,
    service: &CanonicalOfferingService,
    kind: ArtifactKind,
    repo_root: Option<&Path>,
) -> CatalogMissDiagnosis {
    // `agent_profile` is served by its own canonical catalogue, not by the
    // layered document repositories: the reference's `_diagnose_catalog_miss`
    // receives that repository, so its scope-filtered ids and its id listing are
    // the ones the classification must read. A load failure keeps the reference's
    // `MISSING_ARTIFACT` answer (the repository raised while being constructed, so
    // the caller's lookups already produced a miss).
    if kind == ArtifactKind::AgentProfile {
        let Ok(repository) = service.profile_repository() else {
            return CatalogMissDiagnosis::new(CatalogMissCause::MissingArtifact);
        };
        if repository
            .scope_filtered_exact_ids()
            .iter()
            .any(|id| id == missing_id)
        {
            let active = service.active_languages();
            let derived_root = repo_root.or_else(|| repository_repo_root(service));
            return classify_scope_filtered_miss(missing_id, active, derived_root);
        }
        return classify_catalog_miss(missing_id, &repository.ids());
    }
    let scope_filtered = service
        .artifact_scope_filtered_ids(kind)
        .unwrap_or_default();
    if scope_filtered.iter().any(|id| id == missing_id) {
        let active = service.active_languages();
        let derived_root = repo_root.or_else(|| repository_repo_root(service));
        return classify_scope_filtered_miss(missing_id, active, derived_root);
    }
    let available = service.artifact_ids(kind).unwrap_or_default();
    classify_catalog_miss(missing_id, &available)
}

/// The reference `_repository_repo_root`: nearest ancestor of the project layer
/// holding `.kittify`.
fn repository_repo_root(service: &CanonicalOfferingService) -> Option<&Path> {
    let project = service.context().roots.project.as_deref()?;
    project
        .ancestors()
        .find(|candidate| candidate.join(".kittify").is_dir())
}

/// The reference `format_catalog_miss_stanza`.
#[must_use]
pub fn format_catalog_miss_stanza(
    selector_kind: &str,
    artifact_id: &PythonString,
    diagnosis: &CatalogMissDiagnosis,
    indent: &str,
) -> Vec<PythonString> {
    let mut lines = Vec::new();
    let mut first = PythonString::from_scalars(indent);
    first.push_scalars("(catalog entry not found for ");
    first.push_scalars(selector_kind);
    first.push_scalars(":");
    first.append(artifact_id);
    first.push_scalars(")");
    lines.push(first);
    let mut cause = PythonString::from_scalars(indent);
    cause.push_scalars("  Cause: ");
    cause.push_scalars(diagnosis.cause.as_str());
    lines.push(cause);
    let suggestion: Option<PythonString> = match diagnosis.cause {
        CatalogMissCause::TypoSuspected => diagnosis.suggestion.as_ref().map(|suggestion| {
            let mut text = PythonString::from_scalars("  Suggestion: did you mean '");
            text.append(suggestion);
            text.push_scalars("'? Update the charter selection to match the canonical ID.");
            text
        }),
        CatalogMissCause::SchemaValidationSuspected => Some(PythonString::from_scalars(
            "  Suggestion: the artifact YAML failed Pydantic validation and was dropped by the \
             loader. Run `spec-kitty charter validate` to surface the schema error.",
        )),
        CatalogMissCause::ScopeFiltered => Some(match diagnosis.suggestion.as_ref() {
            Some(hint) => {
                let mut text = PythonString::from_scalars("  Suggestion: ");
                text.append(hint);
                text
            }
            None => PythonString::from_scalars(
                "  Suggestion: the artifact exists but was excluded by its applies_to_languages \
                 scope filter. Remove the field to make it always-applicable, or add the active \
                 language to it.",
            ),
        }),
        CatalogMissCause::MissingArtifact => Some(PythonString::from_scalars(
            "  Suggestion: confirm the artifact exists (check project, org, and built-in layers) \
             or run `spec-kitty charter validate` to check for a silent schema validation drop.",
        )),
    };
    if let Some(suggestion) = suggestion {
        let mut line = PythonString::from_scalars(indent);
        line.append(&suggestion);
        lines.push(line);
    }
    lines
}

/// The reference `emit_catalog_miss_warning` policy.
///
/// Returns whether the warning was delivered. A repeated `(kind, id)` in this
/// process emits nothing; a scope-filtered `agent_profile` miss emits nothing.
/// The caller's `sink` receives the reference's structured message (the same
/// fields the reference attaches to `warnings.warn` and the logger).
pub fn emit_catalog_miss_warning(
    selector_kind: &str,
    artifact_id: &PythonString,
    diagnosis: &CatalogMissDiagnosis,
    context: Option<&str>,
    sink: &dyn Fn(&str),
) -> bool {
    if diagnosis.cause == CatalogMissCause::ScopeFiltered && selector_kind == "agent_profile" {
        return false;
    }
    let key = (selector_kind.to_owned(), artifact_id.codepoints().to_vec());
    match EMITTED.lock() {
        Ok(mut emitted) => {
            if !emitted.insert(key) {
                return false;
            }
        }
        Err(_) => return false,
    }
    let id_text = artifact_id
        .to_scalar_string()
        .unwrap_or_else(|_| artifact_id.repr());
    let mut message = format!("Charter catalog miss for {selector_kind}:{id_text}");
    message.push_str(&format!("; cause={}", diagnosis.cause.as_str()));
    if let Some(suggestion) = &diagnosis.suggestion {
        message.push_str(&format!("; suggestion={}", suggestion.repr()));
    }
    if let Some(context) = context {
        message.push_str(&format!(
            "; context={}",
            PythonString::from_scalars(context).repr()
        ));
    }
    sink(&message);
    true
}

// ---------------------------------------------------------------------------
// difflib.SequenceMatcher / get_close_matches over code points
// ---------------------------------------------------------------------------

/// `difflib.SequenceMatcher(None, a, b, autojunk=True).ratio()`.
fn sequence_ratio(left: &PythonString, right: &PythonString) -> f64 {
    let a = left.codepoints();
    let b = right.codepoints();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let index = SequenceMatcherIndex::new(b);
    let matched = index.matched_size(a, b);
    // Source: `2.0 * matches / (la + lb)`; the same f64 operations in Rust.
    2.0 * (matched as f64) / (total as f64)
}

/// The `b2j` index one `b` sequence builds (autojunk applied).
struct SequenceMatcherIndex {
    positions: BTreeMap<u32, Vec<usize>>,
}

impl SequenceMatcherIndex {
    fn new(b: &[u32]) -> Self {
        let mut positions: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (index, code) in b.iter().enumerate() {
            positions.entry(*code).or_default().push(index);
        }
        if b.len() >= AUTOJUNK_MINIMUM_LENGTH {
            let popular_threshold = b.len() / 100 + 1;
            positions.retain(|_, indexes| indexes.len() <= popular_threshold);
        }
        Self { positions }
    }

    /// `sum(size for (_, _, size) in get_matching_blocks())`, without materialising
    /// the block list (the reference only consumes the sizes for `ratio()`).
    fn matched_size(&self, a: &[u32], b: &[u32]) -> usize {
        let mut total = 0usize;
        let mut queue: Vec<(usize, usize, usize, usize)> = vec![(0, a.len(), 0, b.len())];
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, size) = self.find_longest_match(a, b, alo, ahi, blo, bhi);
            total += size;
            if size == 0 {
                continue;
            }
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + size < ahi && j + size < bhi {
                queue.push((i + size, ahi, j + size, bhi));
            }
        }
        total
    }

    /// `SequenceMatcher.find_longest_match` (no junk; the junk extension loops
    /// are inert because `isjunk` is `None`).
    fn find_longest_match(
        &self,
        a: &[u32],
        b: &[u32],
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (mut best_i, mut best_j, mut best_size) = (alo, blo, 0usize);
        let mut j2len: BTreeMap<usize, usize> = BTreeMap::new();
        for (i, &code) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut new_j2len: BTreeMap<usize, usize> = BTreeMap::new();
            if let Some(indexes) = self.positions.get(&code) {
                for &j in indexes {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let previous = if j == 0 {
                        0
                    } else {
                        j2len.get(&(j - 1)).copied().unwrap_or(0)
                    };
                    let size = previous + 1;
                    new_j2len.insert(j, size);
                    if size > best_size {
                        best_i = i + 1 - size;
                        best_j = j + 1 - size;
                        best_size = size;
                    }
                }
            }
            j2len = new_j2len;
        }
        // The reference extends a match left and right outside the longest block
        // when the neighbouring elements are not junk; `isjunk=None` means they
        // always extend while the elements agree.
        while best_i > alo && best_j > blo && a[best_i - 1] == b[best_j - 1] {
            best_i -= 1;
            best_j -= 1;
            best_size += 1;
        }
        while best_i + best_size < ahi
            && best_j + best_size < bhi
            && a[best_i + best_size] == b[best_j + best_size]
        {
            best_size += 1;
        }
        (best_i, best_j, best_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<PythonString> {
        values
            .iter()
            .map(|value| PythonString::from_scalars(value))
            .collect()
    }

    #[test]
    fn close_matches_pick_the_first_maximum_at_the_source_cutoff() {
        let diagnosis = classify_catalog_miss(
            &PythonString::from_scalars("DIRECTIVE_01O"),
            &ids(&["DIRECTIVE_010", "DIRECTIVE_999"]),
        );
        assert_eq!(diagnosis.cause, CatalogMissCause::TypoSuspected);
        assert_eq!(
            diagnosis.suggestion,
            Some(PythonString::from_scalars("DIRECTIVE_010"))
        );
        let far = classify_catalog_miss(
            &PythonString::from_scalars("zzzzzzzzzzzzzzzz"),
            &ids(&["DIRECTIVE_010"]),
        );
        assert_eq!(far.cause, CatalogMissCause::MissingArtifact);
        let empty = classify_catalog_miss(&PythonString::from_scalars("anything"), &[]);
        assert_eq!(empty.cause, CatalogMissCause::MissingArtifact);
        // CPython 3.11.15's `_nlargest(1, [(score, text)])` breaks an equal-score
        // tie by the larger string (DIRECTIVE_024 over DIRECTIVE_010).
        let tied = classify_catalog_miss(
            &PythonString::from_scalars("DIRECTIVE_999"),
            &ids(&["DIRECTIVE_010", "DIRECTIVE_024"]),
        );
        assert_eq!(
            tied.suggestion,
            Some(PythonString::from_scalars("DIRECTIVE_024"))
        );
    }

    #[test]
    fn sequence_ratio_matches_difflib_reference_values() {
        let ratio = |a: &str, b: &str| {
            sequence_ratio(
                &PythonString::from_scalars(a),
                &PythonString::from_scalars(b),
            )
        };
        assert!((ratio("abcd", "bcde") - 0.75).abs() < f64::EPSILON);
        assert!((ratio("", "") - 1.0).abs() < f64::EPSILON);
        assert!((ratio("abc", "abc") - 1.0).abs() < f64::EPSILON);
        assert!((ratio("abc", "") - 0.0).abs() < f64::EPSILON);
    }
}

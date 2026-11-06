//! The inline glossary chokepoint: tokenise request text, match against the
//! active glossary, classify semantic conflicts, and return an observation
//! bundle — the port of upstream `src/glossary/chokepoint.py`.
//!
//! The scanner is lazy: the glossary store is loaded on the first [`GlossaryChokepoint::run`] and
//! cached. A scan never propagates an error; any failure becomes an
//! error-bundle (`error_msg` set, every collection empty), exactly like the
//! reference's outer `except Exception` envelope. Tokenisation, the common-word
//! filter, the lemmatiser and the glossary URN derivation are byte-for-byte
//! ports of `glossary/chokepoint.py`, `glossary/drg_builder.py` and
//! `glossary/extraction.py` (`COMMON_WORDS` is generated from the reference
//! module; see `chokepoint/common_words.rs`).
//!
//! The reference also emits a best-effort `TermCandidateObserved` event per
//! unmatched token. That persisted event write is **not** ported here: this
//! crate stays read-only, so the bundle is produced without touching an event
//! ledger. The caller sees the same observation data; the candidate-event trail
//! remains a declared gap (see the crate report).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use sk_kernel::content_digest::sha256_hex;

use crate::models::TermSense;
use crate::resolution::{ConflictType, Severity};
use crate::scope::GlossaryScope;
use crate::store::Glossary;

mod common_words;

pub use common_words::COMMON_WORDS;

/// The scopes the chokepoint scans by default (`DEFAULT_APPLICABLE_SCOPES`).
pub const DEFAULT_APPLICABLE_SCOPES: [GlossaryScope; 2] =
    [GlossaryScope::SpecKittyCore, GlossaryScope::TeamDomain];

/// Minimum raw/normalised token length the scanner considers.
pub const MIN_TOKEN_LEN: usize = 3;

/// One candidate sense attached to a conflict.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConflictCandidate {
    /// The sense's canonical surface text.
    pub surface: String,
    /// The sense's glossary scope.
    pub scope: GlossaryScope,
    /// The sense's definition.
    pub definition: String,
    /// The sense's authored confidence.
    pub confidence: f64,
}

/// The `term` object of a serialised conflict (`{"surface_text": ...}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConflictTerm {
    /// The normalised token that matched.
    pub surface_text: String,
}

/// A semantic conflict detected during a scan.
///
/// Serialises with the reference's key order (`term`, `conflict_type`,
/// `severity`, `confidence`, `candidate_senses`, `context`); the enums
/// serialise to the reference's `value` spellings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GlossaryConflict {
    /// The matched term.
    pub term: ConflictTerm,
    /// The conflict classification.
    pub conflict_type: ConflictType,
    /// The scored severity.
    pub severity: Severity,
    /// The extraction confidence (`1.0` for request-text tokens).
    pub confidence: f64,
    /// The matching senses, in glossary order.
    pub candidate_senses: Vec<ConflictCandidate>,
    /// The usage context (`request_text` for this scanner).
    pub context: String,
}

impl GlossaryConflict {
    /// The normalised token that produced this conflict.
    pub fn term_surface(&self) -> &str {
        &self.term.surface_text
    }
}

/// The immutable result of one chokepoint scan.
///
/// Serialises with the reference's key order (`matched_urns`, `high_severity`,
/// `all_conflicts`, `tokens_checked`, `duration_ms`, `error_msg`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GlossaryObservationBundle {
    /// `glossary:<8 hex>` URNs for every token found in the index.
    pub matched_urns: Vec<String>,
    /// The subset of `all_conflicts` scored `HIGH`.
    pub high_severity: Vec<GlossaryConflict>,
    /// Every conflict detected during the scan.
    pub all_conflicts: Vec<GlossaryConflict>,
    /// The number of distinct normalised tokens examined.
    pub tokens_checked: usize,
    /// Wall-clock scan duration in milliseconds.
    pub duration_ms: f64,
    /// Set only when an unexpected failure produced the error envelope.
    pub error_msg: Option<String>,
}

impl GlossaryObservationBundle {
    /// An empty bundle (used for the error envelope and for a no-op scan).
    pub fn empty() -> Self {
        Self {
            matched_urns: Vec::new(),
            high_severity: Vec::new(),
            all_conflicts: Vec::new(),
            tokens_checked: 0,
            duration_ms: 0.0,
            error_msg: None,
        }
    }
}

/// The lazy, stateless inline glossary scanner.
pub struct GlossaryChokepoint {
    repo_root: PathBuf,
    scopes: Vec<GlossaryScope>,
    index: Option<GlossaryIndex>,
}

/// The pre-built lookup index (`GlossaryTermIndex`).
///
/// The canonical surface and its lemmatised alias both map to the same
/// `glossary:<id>` URN, so a token finds the term either way.
#[derive(Clone, Debug, Default)]
pub struct GlossaryIndex {
    /// Normalised surface → `glossary:<id>` URN.
    pub surface_to_urn: BTreeMap<String, String>,
    /// Normalised surface → the active senses under that surface.
    pub surface_to_senses: BTreeMap<String, Vec<TermSense>>,
    /// The scopes that contributed senses.
    pub applicable_scopes: BTreeSet<GlossaryScope>,
    /// The number of distinct URNs (= distinct canonical surfaces).
    pub term_count: usize,
}

impl GlossaryChokepoint {
    /// Build a scanner over `repo_root`'s `.kittify/glossaries/` seeds.
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        Self::with_scopes(repo_root, DEFAULT_APPLICABLE_SCOPES.to_vec())
    }

    /// Build a scanner over an explicit applicability set.
    pub fn with_scopes(repo_root: impl AsRef<Path>, scopes: Vec<GlossaryScope>) -> Self {
        Self {
            repo_root: repo_root.as_ref().to_path_buf(),
            scopes,
            index: None,
        }
    }

    /// The project root the scanner reads seeds from.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Scan `request_text` for glossary conflicts. Never fails.
    pub fn run(
        &mut self,
        request_text: &str,
        invocation_id: &str,
        actor_id: &str,
    ) -> GlossaryObservationBundle {
        let started = Instant::now();
        match self.run_inner(request_text, invocation_id, actor_id) {
            Ok(bundle) => bundle,
            Err(message) => GlossaryObservationBundle {
                duration_ms: elapsed_ms(started),
                error_msg: Some(message),
                ..GlossaryObservationBundle::empty()
            },
        }
    }

    /// Build (and cache) the index from the seeds on disk.
    pub fn index(&mut self) -> Result<&GlossaryIndex, String> {
        if self.index.is_none() {
            self.index = Some(build_index(&self.repo_root, &self.scopes)?);
        }
        Ok(self.index.as_ref().expect("built above"))
    }

    fn run_inner(
        &mut self,
        request_text: &str,
        _invocation_id: &str,
        _actor_id: &str,
    ) -> Result<GlossaryObservationBundle, String> {
        let started = Instant::now();
        let tokens = checked_tokens(request_text);
        // The borrow of the cached index ends before the measurements below.
        let mut matched_urns: Vec<String> = Vec::new();
        let mut all_conflicts: Vec<GlossaryConflict> = Vec::new();
        for token in &tokens {
            let Some((urn, senses)) = self.lookup(token)? else {
                // An unmatched token is an unknown-term candidate in the
                // reference; the persisted candidate event is not ported.
                continue;
            };
            matched_urns.push(urn.clone());
            if let Some(conflict_type) = classify_conflict(&senses) {
                let severity = score_severity(conflict_type, 1.0, false);
                all_conflicts.push(GlossaryConflict {
                    term: ConflictTerm {
                        surface_text: token.clone(),
                    },
                    conflict_type,
                    severity,
                    confidence: 1.0,
                    candidate_senses: senses.iter().map(candidate_of).collect(),
                    context: "request_text".to_string(),
                });
            }
        }
        let high_severity: Vec<GlossaryConflict> = all_conflicts
            .iter()
            .filter(|conflict| conflict.severity == Severity::High)
            .cloned()
            .collect();
        Ok(GlossaryObservationBundle {
            matched_urns,
            high_severity,
            all_conflicts,
            tokens_checked: tokens.len(),
            duration_ms: elapsed_ms(started),
            error_msg: None,
        })
    }

    /// Look one normalised token up: `(urn, active senses)` when it matches.
    fn lookup(&mut self, token: &str) -> Result<Option<(String, Vec<TermSense>)>, String> {
        let index = self.index()?;
        let Some(senses) = index.surface_to_senses.get(token) else {
            return Ok(None);
        };
        let Some(urn) = index.surface_to_urn.get(token) else {
            return Ok(None);
        };
        Ok(Some((urn.clone(), senses.clone())))
    }
}

/// The reference `ConflictType.value` spelling.
pub fn conflict_type_str(conflict_type: ConflictType) -> &'static str {
    match conflict_type {
        ConflictType::Unknown => "unknown",
        ConflictType::Ambiguous => "ambiguous",
        ConflictType::UnresolvedCritical => "unresolved_critical",
    }
}

/// The reference `Severity.value` spelling.
pub fn severity_str(severity: Severity) -> &'static str {
    match severity {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn candidate_of(sense: &TermSense) -> ConflictCandidate {
    ConflictCandidate {
        surface: sense.surface().as_str().to_string(),
        scope: sense.scope(),
        definition: sense.definition().to_string(),
        confidence: sense.confidence(),
    }
}

/// `classify_conflict` for the request-text path: `AMBIGUOUS` on 2+ active
/// senses, `UNKNOWN` on none (the scanner only calls this with a match, so
/// `AMBIGUOUS` is the only non-`None` outcome this path can produce).
pub fn classify_conflict(senses: &[TermSense]) -> Option<ConflictType> {
    if senses.is_empty() {
        return Some(ConflictType::Unknown);
    }
    let active = senses
        .iter()
        .filter(|sense| sense.status().is_active())
        .count();
    if active > 1 {
        return Some(ConflictType::Ambiguous);
    }
    None
}

/// `score_severity` for the request-text path (never a critical step).
pub fn score_severity(conflict_type: ConflictType, confidence: f64, critical: bool) -> Severity {
    if conflict_type == ConflictType::Ambiguous {
        return if critical {
            Severity::High
        } else {
            Severity::Medium
        };
    }
    if conflict_type == ConflictType::Unknown {
        if confidence >= 0.8 {
            return Severity::Low;
        }
        if confidence >= 0.5 {
            return Severity::Medium;
        }
        return if critical {
            Severity::High
        } else {
            Severity::Medium
        };
    }
    if confidence < 0.5 && critical {
        return Severity::High;
    }
    Severity::Low
}

/// The reference's `glossary_urn`: the first eight hex digits of SHA-256 of the
/// canonical surface text.
pub fn glossary_urn(surface_text: &str) -> String {
    let digest = sha256_hex(surface_text.as_bytes());
    format!("glossary:{}", &digest[..8])
}

/// Tokenise, filter common words, lemmatise and de-duplicate.
///
/// The reference splits `request_text.lower()` on `[\s\W]+` (Unicode `\w`:
/// alphanumerics plus underscore), drops raw tokens under three characters,
/// drops common words, then normalises and de-duplicates. Order of first
/// appearance is preserved.
pub fn checked_tokens(request_text: &str) -> Vec<String> {
    let lowered = request_text.to_lowercase();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut checked: Vec<String> = Vec::new();
    for raw in lowered.split(is_word_separator) {
        if raw.is_empty() {
            continue;
        }
        if raw.chars().count() < MIN_TOKEN_LEN {
            continue;
        }
        if COMMON_WORDS.contains(&raw) {
            continue;
        }
        let normalized = normalize(raw);
        if normalized.is_empty() || normalized.chars().count() < MIN_TOKEN_LEN {
            continue;
        }
        if !seen.insert(normalized.clone()) {
            continue;
        }
        checked.push(normalized);
    }
    checked
}

/// `true` for the characters the reference's `[\s\W]+` separator class matches.
fn is_word_separator(character: char) -> bool {
    !(character.is_alphanumeric() || character == '_')
}

/// The reference's suffix-stripping lemmatiser.
pub fn normalize(token: &str) -> String {
    let token = token.trim().to_lowercase();
    for suffix in SUFFIX_RULES {
        if let Some(stem) = token.strip_suffix(suffix)
            && stem.chars().count() >= MIN_TOKEN_LEN
            && stem != token
        {
            return stem.to_string();
        }
    }
    token
}

/// The reference's ordered suffix rules (the first that leaves ≥3 chars wins).
const SUFFIX_RULES: [&str; 11] = [
    "ments", "ment", "tions", "tion", "ness", "ings", "ing", "ers", "ed", "er", "s",
];

/// Build the lookup index from the seeds under `.kittify/glossaries/`.
pub fn build_index(repo_root: &Path, scopes: &[GlossaryScope]) -> Result<GlossaryIndex, String> {
    let glossary = Glossary::load(repo_root).map_err(|error| error.to_string())?;
    let applicable: BTreeSet<GlossaryScope> = scopes.iter().copied().collect();
    let mut index = GlossaryIndex {
        applicable_scopes: applicable.clone(),
        ..GlossaryIndex::default()
    };
    let mut urn_to_canonical: BTreeMap<String, String> = BTreeMap::new();
    for sense in glossary.iter() {
        if !sense.status().is_active() {
            continue;
        }
        if !applicable.contains(&sense.scope()) {
            continue;
        }
        let canonical = sense.surface().as_str().to_string();
        let urn = glossary_urn(&canonical);
        urn_to_canonical.insert(urn.clone(), canonical.clone());
        index
            .surface_to_urn
            .entry(canonical.clone())
            .or_insert_with(|| urn.clone());
        index
            .surface_to_senses
            .entry(canonical.clone())
            .or_default()
            .push(sense.clone());
        let normalized = normalize(&canonical);
        if normalized != canonical {
            index
                .surface_to_urn
                .entry(normalized.clone())
                .or_insert(urn);
            index
                .surface_to_senses
                .entry(normalized)
                .or_default()
                .push(sense.clone());
        }
    }
    // Deterministic sense order: the canonical scope precedence, then the
    // canonical surface. The reference's candidate order depends on a
    // `frozenset` iteration and is therefore not stable across processes; a
    // stable order here keeps the payload reproducible without changing which
    // senses are reported.
    for senses in index.surface_to_senses.values_mut() {
        senses.sort_by_key(|sense| {
            let rank = crate::scope::SCOPE_RESOLUTION_ORDER
                .iter()
                .position(|scope| *scope == sense.scope())
                .unwrap_or(usize::MAX);
            (rank, sense.surface().as_str().to_string())
        });
    }
    index.term_count = urn_to_canonical.len();
    Ok(index)
}

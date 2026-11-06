//! Deterministic scope resolution and ambiguity/conflict classification.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    Glossary, GlossaryError, GlossaryScope, SCOPE_RESOLUTION_ORDER, TermSense, TermStatus,
    TermSurface,
};

/// Outcome of a glossary lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LookupStatus {
    /// No sense matched in the requested scopes.
    Unknown,
    /// Exactly one active sense matched.
    Resolved,
    /// Two or more active senses matched.
    Ambiguous,
    /// One or more senses matched, but none are active.
    NoActiveSense,
}

/// Conflict classes produced by resolution.
///
/// `Inconsistent` from the upstream model is intentionally absent: detecting it
/// requires LLM output comparison, which this read-only crate does not perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictType {
    /// The term has no matching sense.
    Unknown,
    /// The term has two or more active senses.
    Ambiguous,
    /// The term is unknown in a critical step with low extraction confidence.
    UnresolvedCritical,
}

/// Severity assigned to a semantic conflict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational.
    Low,
    /// Warn and, under medium strictness, allow generation.
    Medium,
    /// Blocking under medium and max strictness.
    High,
}

/// A serializable view of one matching sense.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SenseMatch {
    /// Normalized surface text.
    pub surface: String,
    /// Scope that owns the sense.
    pub scope: GlossaryScope,
    /// Definition for this sense.
    pub definition: String,
    /// Confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Lifecycle status.
    pub status: TermStatus,
    /// Cross-reference metadata.
    pub see_also: Vec<BTreeMap<String, String>>,
    /// Terms that should not be used for this sense.
    pub synonyms_to_avoid: Vec<String>,
    /// Mission that introduced the term, when annotated.
    pub introduced_in_mission: Option<String>,
}

impl From<&TermSense> for SenseMatch {
    fn from(sense: &TermSense) -> Self {
        Self {
            surface: sense.surface().as_str().to_owned(),
            scope: sense.scope(),
            definition: sense.definition().to_owned(),
            confidence: sense.confidence(),
            status: sense.status(),
            see_also: sense.see_also().to_vec(),
            synonyms_to_avoid: sense.synonyms_to_avoid().to_vec(),
            introduced_in_mission: sense.introduced_in_mission().map(str::to_owned),
        }
    }
}

/// A serializable semantic conflict produced by a lookup.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SemanticConflict {
    /// Normalized term surface.
    pub term: String,
    /// Conflict classification.
    pub conflict_type: ConflictType,
    /// Severity assigned from the conflict class and lookup context.
    pub severity: Severity,
    /// Confidence supplied to the lookup.
    pub confidence: f64,
    /// All matching senses for an ambiguity, in precedence order.
    pub candidate_senses: Vec<SenseMatch>,
    /// Optional caller context. Lookups leave this empty.
    pub context: String,
}

/// The serializable result of resolving one term.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct LookupResult {
    /// The normalized queried surface.
    pub surface: String,
    /// Resolution outcome.
    pub status: LookupStatus,
    /// Conflict classification, when one applies.
    pub conflict: Option<SemanticConflict>,
    /// All matches in requested scope precedence order.
    pub matches: Vec<SenseMatch>,
}

/// Options controlling scope order and conflict severity.
#[derive(Clone, Debug, PartialEq)]
pub struct LookupOptions {
    /// Scopes to search, in the caller's precedence order.
    pub scopes: Vec<GlossaryScope>,
    /// Confidence supplied to conflict classification. Must be `0.0..=1.0`.
    pub confidence: f64,
    /// Whether the term was used in a critical step.
    pub critical_step: bool,
}

impl Default for LookupOptions {
    fn default() -> Self {
        Self {
            scopes: SCOPE_RESOLUTION_ORDER.to_vec(),
            confidence: 1.0,
            critical_step: false,
        }
    }
}

impl LookupOptions {
    /// Default options: all scopes, confidence `1.0`, non-critical.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the scope search order.
    pub fn with_scopes(mut self, scopes: impl IntoIterator<Item = GlossaryScope>) -> Self {
        self.scopes = scopes.into_iter().collect();
        self
    }

    /// Replace the extraction confidence used for conflict severity.
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    /// Mark the lookup as occurring in a critical step.
    pub fn in_critical_step(mut self, critical_step: bool) -> Self {
        self.critical_step = critical_step;
        self
    }
}

/// Classify a term against its matches using the upstream lookup rules.
///
/// No matches yield `Unknown`, except a critical step with confidence below
/// `0.5`, which yields `UnresolvedCritical`. Two or more active senses yield
/// `Ambiguous`. Draft and deprecated senses do not count as active.
pub fn classify_conflict(
    matches: &[TermSense],
    confidence: f64,
    critical_step: bool,
) -> Option<ConflictType> {
    let active = matches
        .iter()
        .filter(|sense| sense.status().is_active())
        .count();
    classify_from_counts(matches.len(), active, confidence, critical_step)
}

/// Score conflict severity with the upstream thresholds.
pub fn score_severity(
    conflict_type: ConflictType,
    confidence: f64,
    critical_step: bool,
) -> Severity {
    match conflict_type {
        ConflictType::Ambiguous => {
            if critical_step {
                Severity::High
            } else {
                Severity::Medium
            }
        }
        ConflictType::Unknown => {
            if confidence >= 0.8 {
                Severity::Low
            } else if confidence >= 0.5 {
                Severity::Medium
            } else if critical_step {
                Severity::High
            } else {
                Severity::Medium
            }
        }
        ConflictType::UnresolvedCritical => Severity::High,
    }
}

/// Resolve a term against an explicit scope order.
///
/// This is the function-shaped equivalent of upstream `resolve_term`; it
/// returns the full serializable lookup result rather than only a sense list.
pub fn resolve_term(
    surface: &str,
    scopes: &[GlossaryScope],
    glossary: &Glossary,
) -> Result<LookupResult, GlossaryError> {
    glossary.resolve_in(surface, scopes)
}

fn classify_from_counts(
    total: usize,
    active: usize,
    confidence: f64,
    critical_step: bool,
) -> Option<ConflictType> {
    if total == 0 {
        if critical_step && confidence < 0.5 {
            Some(ConflictType::UnresolvedCritical)
        } else {
            Some(ConflictType::Unknown)
        }
    } else if active > 1 {
        Some(ConflictType::Ambiguous)
    } else {
        None
    }
}

impl Glossary {
    /// Resolve a CLI-style query, normalizing it first.
    pub fn resolve(&self, input: &str) -> Result<LookupResult, GlossaryError> {
        let surface = TermSurface::normalized(input)?;
        Ok(self.lookup(&surface))
    }

    /// Resolve a CLI-style query with explicit scope order and confidence.
    pub fn resolve_with(
        &self,
        input: &str,
        options: &LookupOptions,
    ) -> Result<LookupResult, GlossaryError> {
        let surface = TermSurface::normalized(input)?;
        self.lookup_with(&surface, options)
    }

    /// Resolve an already-validated normalized surface with default options.
    pub fn lookup(&self, surface: &TermSurface) -> LookupResult {
        self.build_lookup(surface, &LookupOptions::default())
    }

    /// Resolve a CLI-style query against an explicit scope order.
    pub fn resolve_in(
        &self,
        input: &str,
        scopes: &[GlossaryScope],
    ) -> Result<LookupResult, GlossaryError> {
        let surface = TermSurface::normalized(input)?;
        Ok(self.lookup_in(&surface, scopes))
    }

    /// Resolve an already-validated surface against an explicit scope order.
    pub fn lookup_in(&self, surface: &TermSurface, scopes: &[GlossaryScope]) -> LookupResult {
        let options = LookupOptions {
            scopes: scopes.to_vec(),
            ..LookupOptions::default()
        };
        self.build_lookup(surface, &options)
    }

    /// Resolve an already-validated normalized surface with explicit options.
    pub fn lookup_with(
        &self,
        surface: &TermSurface,
        options: &LookupOptions,
    ) -> Result<LookupResult, GlossaryError> {
        if !(0.0..=1.0).contains(&options.confidence) {
            return Err(GlossaryError::InvalidConfidence {
                value: options.confidence,
            });
        }
        Ok(self.build_lookup(surface, options))
    }

    fn build_lookup(&self, surface: &TermSurface, options: &LookupOptions) -> LookupResult {
        let mut matches = Vec::new();
        for &scope in &options.scopes {
            for sense in self.senses_in_scope(scope) {
                if sense.surface() == surface {
                    matches.push(sense);
                }
            }
        }

        let active = matches
            .iter()
            .filter(|sense| sense.status().is_active())
            .count();
        let status = if matches.is_empty() {
            LookupStatus::Unknown
        } else if active > 1 {
            LookupStatus::Ambiguous
        } else if active == 1 {
            LookupStatus::Resolved
        } else {
            LookupStatus::NoActiveSense
        };

        let view: Vec<SenseMatch> = matches
            .iter()
            .map(|sense| SenseMatch::from(*sense))
            .collect();
        let conflict_type = classify_from_counts(
            matches.len(),
            active,
            options.confidence,
            options.critical_step,
        );
        let conflict = conflict_type.map(|conflict_type| SemanticConflict {
            term: surface.as_str().to_owned(),
            conflict_type,
            severity: score_severity(conflict_type, options.confidence, options.critical_step),
            confidence: options.confidence,
            candidate_senses: if conflict_type == ConflictType::Ambiguous {
                view.clone()
            } else {
                Vec::new()
            },
            context: String::new(),
        });

        LookupResult {
            surface: surface.as_str().to_owned(),
            status,
            conflict,
            matches: view,
        }
    }
}

//! In-memory, read-only glossary store populated from seed files.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{GlossaryError, GlossaryScope, SCOPE_RESOLUTION_ORDER, TermSense, load_seed_file};

/// An in-memory glossary indexed by scope.
///
/// Scope precedence is the enum's declaration order (and therefore the
/// `BTreeMap` iteration order), matching `mission_local > team_domain >
/// audience_domain > spec_kitty_core`. Term order inside a scope is the seed
/// file order, which is the deterministic tie-break for duplicate surfaces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glossary {
    scopes: BTreeMap<GlossaryScope, Vec<TermSense>>,
}

impl Glossary {
    /// Create an empty glossary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a glossary from already-validated senses.
    ///
    /// Senses are grouped by scope. Insertion order is preserved.
    pub fn from_senses(senses: impl IntoIterator<Item = TermSense>) -> Self {
        let mut glossary = Self::new();
        for sense in senses {
            glossary.add_sense(sense);
        }
        glossary
    }

    /// Load all four canonical scopes from a repository root.
    pub fn load(repo_root: impl AsRef<Path>) -> Result<Self, GlossaryError> {
        Self::load_scopes(repo_root, &SCOPE_RESOLUTION_ORDER)
    }

    /// Load a selected set of scopes from a repository root.
    ///
    /// Missing seed files are skipped. Duplicate scopes are loaded once.
    pub fn load_scopes(
        repo_root: impl AsRef<Path>,
        scopes: &[GlossaryScope],
    ) -> Result<Self, GlossaryError> {
        let repo_root = repo_root.as_ref();
        let mut glossary = Self::new();
        for &scope in scopes {
            if glossary.scopes.contains_key(&scope) {
                continue;
            }
            let senses = load_seed_file(scope, repo_root)?;
            if !senses.is_empty() {
                glossary.scopes.insert(scope, senses);
            }
        }
        Ok(glossary)
    }

    /// Add one validated sense.
    pub fn add_sense(&mut self, sense: TermSense) {
        self.scopes.entry(sense.scope()).or_default().push(sense);
    }

    /// Iterate senses in scope precedence order, preserving seed order per scope.
    pub fn iter(&self) -> impl Iterator<Item = &TermSense> {
        self.scopes.values().flatten()
    }

    /// Every sense, sorted by scope precedence and then surface text.
    ///
    /// Equal surface/scope entries retain their insertion order because the sort
    /// is stable.
    pub fn all_terms_sorted(&self) -> Vec<&TermSense> {
        let mut terms: Vec<_> = self.iter().collect();
        terms.sort_by(|left, right| {
            left.scope()
                .cmp(&right.scope())
                .then_with(|| left.surface().cmp(right.surface()))
                .then_with(|| left.definition().cmp(right.definition()))
        });
        terms
    }

    /// Number of senses in the glossary.
    pub fn len(&self) -> usize {
        self.scopes.values().map(Vec::len).sum()
    }

    /// Whether the glossary has no senses.
    pub fn is_empty(&self) -> bool {
        self.scopes.values().all(Vec::is_empty)
    }

    pub(crate) fn senses_in_scope(&self, scope: GlossaryScope) -> &[TermSense] {
        match self.scopes.get(&scope) {
            Some(senses) => senses.as_slice(),
            None => &[],
        }
    }
}

/// Compatibility name for the read-only in-memory glossary store.
pub type GlossaryStore = Glossary;

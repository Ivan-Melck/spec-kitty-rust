//! Pure, I/O-free `ActionRouter`: request text -> `(profile_id, action)`.
//!
//! This is the native port of the reference's deterministic router
//! (`specify_cli/invocation/router.py`). It is a pure function over
//! **caller-supplied** profile data: it reads no YAML, no activation ledger, no
//! catalogue and no filesystem, and it never calls a model. A caller projects
//! the real profile model (id, primary role, `routing_priority`,
//! `specialization_context.domain_keywords`, `collaboration.canonical_verbs`)
//! into [`RoutableProfile`] and passes the profile slice in registry order
//! (`list_all()` is sorted by `profile_id`).
//!
//! Reproduced reference semantics (each one is oracle-verified, not "improved"):
//!
//! * failure precedence: an empty profile list is reported before a hint is
//!   resolved; `Some("")` is an explicit miss, never "absent";
//! * the explicit-hint branch derives the action from the request tokens
//!   (first canonical verb) and only then from the role's default capability
//!   verb -- so it is *not* the executor's role-default branch;
//! * the canonical-verb tier outranks the domain-keyword tier regardless of
//!   `routing_priority`; `routing_priority` only breaks ties *within* the
//!   selection pool (stable, higher wins);
//! * `alternatives` carries every non-winning candidate from the full candidate
//!   list, while an ambiguity error reports only the tied top candidates;
//! * verb candidates are ordered by first matching token, then profile order;
//!   keyword candidates follow in profile order and are de-duplicated by id;
//! * tokenization is the pinned interpreter's `re.split(r"[\s\W]+",
//!   text.lower())` minus the 45 stop words (using private pinned Unicode facts).
//!
//! Data fidelity: ids, roles, request text, keywords, match reasons, suggestions
//! and error text are exact Python code points ([`PythonString`]), so authored
//! NUL, non-printable text and lone surrogates are preserved instead of being
//! rejected, replaced or re-encoded. Actions are always from the closed
//! canonical tables, so they are `&'static str`.
//!
//! Scope: this module is the router only. It does not implement the catalogue
//! (activation + legacy overlay + language scope), the executor, the payload
//! writer, the CLI/StepContract wiring, model routing, glossary or any
//! governance text. It is not a security boundary and makes no performance
//! claim.

use std::fmt;

use sk_doctrine::{ROLE_CAPABILITIES, RoleCapabilities};

use crate::legacy_json::PythonString;
use crate::router_unicode;

/// Stop words removed during token normalization (reference `STOP_WORDS`).
///
/// Sorted for readability and lookup; membership is what matters.
pub const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "at", "be", "been", "being", "by", "can", "could", "did", "do",
    "does", "for", "had", "has", "have", "in", "is", "kindly", "may", "me", "might", "must", "my",
    "of", "on", "or", "our", "please", "should", "some", "that", "the", "these", "this", "those",
    "to", "us", "was", "were", "will", "with", "would",
];

/// One request token -> canonical action and role (reference
/// `CANONICAL_VERB_MAP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanonicalVerbMatch {
    /// The matched request token (lower-case ASCII).
    pub token: &'static str,
    /// The canonical action the token selects.
    pub action: &'static str,
    /// The canonical role the token selects.
    pub role: &'static str,
}

/// `(token, action, role)` in the reference's declaration order.
const CANONICAL_VERB_ENTRIES: &[(&str, &str, &str)] = &[
    // implementer
    ("implement", "implement", "implementer"),
    ("build", "implement", "implementer"),
    ("code", "implement", "implementer"),
    ("develop", "implement", "implementer"),
    ("create", "implement", "implementer"),
    ("write", "implement", "implementer"),
    ("generate", "implement", "implementer"),
    ("produce", "implement", "implementer"),
    ("output", "implement", "implementer"),
    ("refine", "implement", "implementer"),
    ("improve", "implement", "implementer"),
    ("fix", "implement", "implementer"),
    ("patch", "implement", "implementer"),
    ("repair", "implement", "implementer"),
    ("debug", "implement", "implementer"),
    // reviewer
    ("review", "review", "reviewer"),
    ("check", "review", "reviewer"),
    ("inspect", "review", "reviewer"),
    ("evaluate", "review", "reviewer"),
    ("audit", "review", "reviewer"),
    ("assess", "review", "reviewer"),
    // planner
    ("plan", "plan", "planner"),
    ("decompose", "plan", "planner"),
    ("break", "plan", "planner"),
    ("outline", "plan", "planner"),
    ("schedule", "plan", "planner"),
    ("prioritize", "plan", "planner"),
    ("triage", "plan", "planner"),
    ("rank", "plan", "planner"),
    ("order", "plan", "planner"),
    // architect
    ("specify", "specify", "architect"),
    ("spec", "specify", "architect"),
    ("define", "specify", "architect"),
    ("requirements", "specify", "architect"),
    ("scope", "specify", "architect"),
    // designer
    ("design", "design", "designer"),
    ("mockup", "design", "designer"),
    ("prototype", "design", "designer"),
    ("wireframe", "design", "designer"),
    // researcher
    ("analyze", "analyze", "researcher"),
    ("investigate", "analyze", "researcher"),
    ("research", "analyze", "researcher"),
    ("explore", "analyze", "researcher"),
    ("study", "analyze", "researcher"),
    ("summarize", "analyze", "researcher"),
    ("synthesize", "analyze", "researcher"),
    ("compile", "analyze", "researcher"),
    ("report", "analyze", "researcher"),
    // curator
    ("curate", "curate", "curator"),
    ("classify", "curate", "curator"),
    ("organize", "curate", "curator"),
    ("tag", "curate", "curator"),
    ("validate", "curate", "curator"),
    ("verify", "curate", "curator"),
    // manager
    ("coordinate", "coordinate", "manager"),
    ("manage", "coordinate", "manager"),
    ("delegate", "coordinate", "manager"),
    ("monitor", "coordinate", "manager"),
    ("track", "coordinate", "manager"),
];

/// The canonical verb table, `(token, action, role)` per entry.
pub fn canonical_verb_entries() -> &'static [(&'static str, &'static str, &'static str)] {
    CANONICAL_VERB_ENTRIES
}

/// Resolve one already-normalized request token against the canonical table.
pub fn canonical_verb(token: &PythonString) -> Option<CanonicalVerbMatch> {
    let units = token.codepoints();
    CANONICAL_VERB_ENTRIES
        .iter()
        .find(|(word, _, _)| code_units_eq_str(units, word))
        .map(|&(word, action, role)| CanonicalVerbMatch {
            token: word,
            action,
            role,
        })
}

/// Confidence carried by a routing decision.
///
/// [`RouterConfidence::GenericFallback`] is **never produced** by
/// [`ActionRouter::route`]: the reference pins `generic_fallback` in the
/// empty-charter seam, which pre-empts the router entirely. The variant exists
/// so such a decision can round-trip through this typed vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouterConfidence {
    /// Explicit `profile_hint`.
    Exact,
    /// Canonical-verb tier.
    CanonicalVerb,
    /// Domain-keyword tier.
    DomainKeyword,
    /// Empty-charter fallback (never produced here).
    GenericFallback,
}

impl RouterConfidence {
    /// The reference's literal spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::CanonicalVerb => "canonical_verb",
            Self::DomainKeyword => "domain_keyword",
            Self::GenericFallback => "generic_fallback",
        }
    }

    /// Parse the reference's literal spelling (round-trip helper).
    pub fn from_source_str(text: &str) -> Option<Self> {
        match text {
            "exact" => Some(Self::Exact),
            "canonical_verb" => Some(Self::CanonicalVerb),
            "domain_keyword" => Some(Self::DomainKeyword),
            "generic_fallback" => Some(Self::GenericFallback),
            _ => None,
        }
    }
}

impl fmt::Display for RouterConfidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Minimal, caller-supplied read view of one routable profile.
///
/// Every field is a projection of the real profile model; this module performs
/// no I/O and reads no YAML. `role` is the **primary** role (`roles[0]`), already
/// normalized the way the reference's `Role` normalizes it (lower-cased); custom
/// roles are allowed and simply have no canonical capability row. `routing_priority`
/// is the reference's `0..=100` integer (its default of 50 is applied by the
/// caller, exactly like the model does).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RoutableProfile {
    /// `profile-id`, verbatim.
    pub profile_id: PythonString,
    /// Primary role (`roles[0]`), normalized.
    pub role: PythonString,
    /// `routing-priority` (`0..=100`).
    pub routing_priority: i32,
    /// `specialization-context.domain-keywords`, verbatim and in authored order.
    pub domain_keywords: Vec<PythonString>,
    /// `collaboration.canonical-verbs`, verbatim and in authored order.
    pub canonical_verbs: Vec<PythonString>,
}

impl RoutableProfile {
    /// Build the caller-supplied view of one profile.
    pub fn new(
        profile_id: PythonString,
        role: PythonString,
        routing_priority: i32,
        domain_keywords: Vec<PythonString>,
        canonical_verbs: Vec<PythonString>,
    ) -> Self {
        Self {
            profile_id,
            role,
            routing_priority,
            domain_keywords,
            canonical_verbs,
        }
    }
}

/// One non-winning candidate the router considered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterAlternative {
    /// Candidate `profile-id`.
    pub profile_id: PythonString,
    /// Canonical action the candidate would run.
    pub action: &'static str,
    /// Tier the candidate matched in.
    pub confidence: RouterConfidence,
    /// Exact reference match-reason text.
    pub match_reason: PythonString,
}

/// A successful routing decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterDecision {
    /// Winning `profile-id`.
    pub profile_id: PythonString,
    /// Action to run (from the closed canonical action vocabulary).
    pub action: &'static str,
    /// Winning tier.
    pub confidence: RouterConfidence,
    /// Exact reference match-reason text.
    pub match_reason: PythonString,
    /// Every non-winning candidate, from the full candidate list.
    pub alternatives: Vec<RouterAlternative>,
}

/// The reference's routing error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouterErrorCode {
    /// `ROUTER_NO_MATCH`
    NoMatch,
    /// `PROFILE_NOT_FOUND`
    ProfileNotFound,
    /// `ROUTER_AMBIGUOUS`
    Ambiguous,
}

impl RouterErrorCode {
    /// The reference's literal code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoMatch => "ROUTER_NO_MATCH",
            Self::ProfileNotFound => "PROFILE_NOT_FOUND",
            Self::Ambiguous => "ROUTER_AMBIGUOUS",
        }
    }
}

impl fmt::Display for RouterErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A routing failure: the reference's `RouterAmbiguityError` surface.
///
/// Fields are authoritative (`request_text`, `candidates`, `suggestion`); the
/// reference's `str(error)` is `message()`. `Display` is an escaped diagnostic
/// rendering and is not a wire form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterError {
    code: RouterErrorCode,
    request_text: PythonString,
    candidates: Vec<RouterAlternative>,
    suggestion: PythonString,
}

impl RouterError {
    fn new(
        code: RouterErrorCode,
        request_text: PythonString,
        candidates: Vec<RouterAlternative>,
        suggestion: PythonString,
    ) -> Self {
        Self {
            code,
            request_text,
            candidates,
            suggestion,
        }
    }

    /// The reference's `error_code`.
    pub fn code(&self) -> RouterErrorCode {
        self.code
    }

    /// The reference's `request_text`, verbatim.
    pub fn request_text(&self) -> &PythonString {
        &self.request_text
    }

    /// The reference's `candidates` (empty for a no-match error; only the tied
    /// top candidates for an ambiguity).
    pub fn candidates(&self) -> &[RouterAlternative] {
        &self.candidates
    }

    /// The reference's `suggestion` text.
    pub fn suggestion(&self) -> &PythonString {
        &self.suggestion
    }

    /// `str(error)` -- `"<code>: <suggestion>"` as exact code points.
    pub fn message(&self) -> PythonString {
        let mut units: Vec<u32> = self.code.as_str().chars().map(u32::from).collect();
        units.extend(": ".chars().map(u32::from));
        units.extend_from_slice(self.suggestion.codepoints());
        PythonString::from_units(units)
    }
}

impl fmt::Display for RouterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Escaped diagnostic rendering; `message()` carries the exact text.
        write!(formatter, "{}", self.message())
    }
}

impl std::error::Error for RouterError {}

const EMPTY_CATALOGUE_SUGGESTION: &str = "No profiles available. Run 'spec-kitty charter \
activate agent-profile <id>' to activate one, or 'spec-kitty profiles list --all' to see the \
catalog.";
const NO_MATCH_SUGGESTION: &str = "No profile matched. Use 'spec-kitty dispatch \"<request>\" \
--profile <id>' or run 'spec-kitty profiles list'.";
const AMBIGUOUS_SUGGESTION: &str = "Multiple profiles matched. Use 'spec-kitty dispatch \
\"<request>\" --profile <id>' or run 'spec-kitty profiles list'.";
const SELECTED_BY_PRIORITY: &str = " (selected by routing_priority)";

/// The deterministic router.
///
/// A unit struct: it holds no registry, no cache and no state, exactly like a
/// pure function. The catalogue is the caller's responsibility.
#[derive(Clone, Copy, Debug, Default)]
pub struct ActionRouter;

impl ActionRouter {
    /// Route `request_text` to a profile and action.
    ///
    /// `profile_hint` is `None` when absent and `Some(_)` for any supplied
    /// value, including the empty string (which is an explicit miss, not
    /// "absent"). `profiles` is the caller's snapshot in registry order.
    pub fn route(
        request_text: &PythonString,
        profile_hint: Option<&PythonString>,
        profiles: &[RoutableProfile],
    ) -> Result<RouterDecision, RouterError> {
        // Empty catalogue wins over any hint (reference #4114 precedence).
        if profiles.is_empty() {
            return Err(RouterError::new(
                RouterErrorCode::NoMatch,
                request_text.clone(),
                Vec::new(),
                PythonString::from_scalars(EMPTY_CATALOGUE_SUGGESTION),
            ));
        }

        // Level 1: explicit hint -> exact confidence.
        if let Some(hint) = profile_hint {
            let Some(profile) = profiles.iter().find(|profile| profile.profile_id == *hint) else {
                return Err(profile_not_found(request_text, hint, profiles));
            };
            return Ok(RouterDecision {
                profile_id: profile.profile_id.clone(),
                action: derive_action_from_tokens(request_text, &profile.role),
                confidence: RouterConfidence::Exact,
                match_reason: reason_explicit_hint(hint),
                alternatives: Vec::new(),
            });
        }

        let tokens = normalize_tokens(request_text);

        // Level 2: canonical-verb matches, first occurrence per role wins.
        let mut verb_matches: Vec<(PythonString, CanonicalVerbMatch)> = Vec::new();
        for token in &tokens {
            let Some(matched) = canonical_verb(token) else {
                continue;
            };
            if !verb_matches
                .iter()
                .any(|(role, _)| code_units_eq_str(role.codepoints(), matched.role))
            {
                verb_matches.push((PythonString::from_scalars(matched.role), matched));
            }
        }

        // Level 3: domain-keyword matches (domain keywords plus the profile's
        // collaboration canonical verbs, folded in authored order).
        let mut keyword_matches: Vec<(usize, &'static str, PythonString)> = Vec::new();
        for (index, profile) in profiles.iter().enumerate() {
            let mut keywords: Vec<&PythonString> = profile.domain_keywords.iter().collect();
            for verb in &profile.canonical_verbs {
                if !keywords.iter().any(|existing| **existing == *verb) {
                    keywords.push(verb);
                }
            }
            for keyword in keywords {
                let lowered = router_unicode::lower_codepoints(keyword.codepoints());
                if tokens
                    .iter()
                    .any(|token| token.codepoints() == lowered.as_slice())
                {
                    keyword_matches.push((
                        index,
                        role_default_action(&profile.role),
                        keyword.clone(),
                    ));
                }
            }
        }

        // Aggregate: verb-tier candidates (token order, then profile order),
        // then keyword-tier candidates de-duplicated by profile id.
        let mut candidates: Vec<Candidate> = Vec::new();
        for (role, matched) in &verb_matches {
            for (index, profile) in profiles.iter().enumerate() {
                if profile.role == *role {
                    candidates.push(Candidate {
                        profile: index,
                        action: matched.action,
                        confidence: RouterConfidence::CanonicalVerb,
                        match_reason: reason_canonical_verb(matched, role),
                    });
                }
            }
        }
        for (index, action, keyword) in keyword_matches {
            if !candidates
                .iter()
                .any(|candidate| candidate.profile == index)
            {
                candidates.push(Candidate {
                    profile: index,
                    action,
                    confidence: RouterConfidence::DomainKeyword,
                    match_reason: reason_domain_keyword(&keyword),
                });
            }
        }

        if candidates.is_empty() {
            return Err(RouterError::new(
                RouterErrorCode::NoMatch,
                request_text.clone(),
                Vec::new(),
                PythonString::from_scalars(NO_MATCH_SUGGESTION),
            ));
        }

        // Two-tier partition: a canonical-verb candidate always outranks a
        // domain-keyword candidate. With an empty verb tier the pool is every
        // candidate (which is exactly the keyword tier).
        let verb_tier: Vec<usize> = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.confidence == RouterConfidence::CanonicalVerb)
            .map(|(index, _)| index)
            .collect();
        let pool: Vec<usize> = if verb_tier.is_empty() {
            (0..candidates.len()).collect()
        } else {
            verb_tier
        };

        if let [only] = pool.as_slice() {
            return Ok(decision(&candidates, *only, profiles, None));
        }

        // Tiebreaker: routing_priority (higher wins), stable within the pool.
        let mut sorted_pool = pool;
        sorted_pool.sort_by(|&left, &right| {
            priority(&candidates[right], profiles).cmp(&priority(&candidates[left], profiles))
        });
        let top_priority = priority(&candidates[sorted_pool[0]], profiles);
        let top_candidates: Vec<usize> = sorted_pool
            .iter()
            .copied()
            .filter(|&index| priority(&candidates[index], profiles) == top_priority)
            .collect();

        if let [only] = top_candidates.as_slice() {
            return Ok(decision(
                &candidates,
                *only,
                profiles,
                Some(SELECTED_BY_PRIORITY),
            ));
        }

        Err(RouterError::new(
            RouterErrorCode::Ambiguous,
            request_text.clone(),
            top_candidates
                .iter()
                .map(|&index| alternative(&candidates[index], profiles))
                .collect(),
            PythonString::from_scalars(AMBIGUOUS_SUGGESTION),
        ))
    }

    /// [`ActionRouter::route`] for already-decoded Rust text.
    ///
    /// A `&str` cannot carry lone surrogates; use [`ActionRouter::route`] with
    /// [`PythonString::from_codepoints`] for exact Python string input.
    pub fn route_scalars(
        request_text: &str,
        profile_hint: Option<&str>,
        profiles: &[RoutableProfile],
    ) -> Result<RouterDecision, RouterError> {
        let request_text = PythonString::from_scalars(request_text);
        let profile_hint = profile_hint.map(PythonString::from_scalars);
        Self::route(&request_text, profile_hint.as_ref(), profiles)
    }
}

/// One aggregated candidate during selection.
struct Candidate {
    /// Index into the caller's profile slice.
    profile: usize,
    action: &'static str,
    confidence: RouterConfidence,
    match_reason: PythonString,
}

fn priority(candidate: &Candidate, profiles: &[RoutableProfile]) -> i32 {
    profiles[candidate.profile].routing_priority
}

fn alternative(candidate: &Candidate, profiles: &[RoutableProfile]) -> RouterAlternative {
    RouterAlternative {
        profile_id: profiles[candidate.profile].profile_id.clone(),
        action: candidate.action,
        confidence: candidate.confidence,
        match_reason: candidate.match_reason.clone(),
    }
}

fn decision(
    candidates: &[Candidate],
    winner: usize,
    profiles: &[RoutableProfile],
    reason_suffix: Option<&str>,
) -> RouterDecision {
    let candidate = &candidates[winner];
    let mut match_reason = candidate.match_reason.codepoints().to_vec();
    if let Some(suffix) = reason_suffix {
        match_reason.extend(suffix.chars().map(u32::from));
    }
    RouterDecision {
        profile_id: profiles[candidate.profile].profile_id.clone(),
        action: candidate.action,
        confidence: candidate.confidence,
        match_reason: PythonString::from_units(match_reason),
        alternatives: candidates
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != winner)
            .map(|(_, candidate)| alternative(candidate, profiles))
            .collect(),
    }
}

/// The reference's `_normalize_tokens`: lower, split on `[\s\W]+`, drop stop words.
pub fn normalize_tokens(request_text: &PythonString) -> Vec<PythonString> {
    let lowered = router_unicode::lower_codepoints(request_text.codepoints());
    let mut tokens = Vec::new();
    for run in router_unicode::split_word_runs(&lowered) {
        if is_stop_word(run) {
            continue;
        }
        tokens.push(PythonString::from_units(run.to_vec()));
    }
    tokens
}

fn is_stop_word(word: &[u32]) -> bool {
    STOP_WORDS.iter().any(|stop| code_units_eq_str(word, stop))
}

/// The reference's `_derive_action_from_tokens`, including its role fallback.
fn derive_action_from_tokens(request_text: &PythonString, role: &PythonString) -> &'static str {
    for token in normalize_tokens(request_text) {
        if let Some(matched) = canonical_verb(&token) {
            return matched.action;
        }
    }
    role_default_action(role)
}

/// `DEFAULT_ROLE_CAPABILITIES[role].canonical_verbs[0]`, else `"review"`.
fn role_default_action(role: &PythonString) -> &'static str {
    capabilities(role)
        .and_then(|row| row.canonical_verbs.first().copied())
        .unwrap_or("review")
}

/// The canonical capability row for an already-normalized role value.
///
/// Lookup is an exact match against the one canonical table in `sk-doctrine`
/// (its own lookup authority); custom roles have no row.
fn capabilities(role: &PythonString) -> Option<&'static RoleCapabilities> {
    ROLE_CAPABILITIES
        .iter()
        .find(|row| code_units_eq_str(role.codepoints(), row.role))
}

fn profile_not_found(
    request_text: &PythonString,
    hint: &PythonString,
    profiles: &[RoutableProfile],
) -> RouterError {
    let mut available: Vec<&PythonString> =
        profiles.iter().map(|profile| &profile.profile_id).collect();
    available.sort();
    let mut units: Vec<u32> = "Profile '".chars().map(u32::from).collect();
    units.extend_from_slice(hint.codepoints());
    units.extend("' not found. Available: [".chars().map(u32::from));
    for (index, profile_id) in available.iter().enumerate() {
        if index > 0 {
            units.extend(", ".chars().map(u32::from));
        }
        // The reference interpolates the Python list itself, so each element is
        // rendered by the one canonical `repr` formatter.
        units.extend(profile_id.repr().chars().map(u32::from));
    }
    units.push(u32::from(b']'));
    RouterError::new(
        RouterErrorCode::ProfileNotFound,
        request_text.clone(),
        Vec::new(),
        PythonString::from_units(units),
    )
}

fn reason_explicit_hint(hint: &PythonString) -> PythonString {
    let mut units: Vec<u32> = "explicit profile_hint '".chars().map(u32::from).collect();
    units.extend_from_slice(hint.codepoints());
    units.push(u32::from(b'\''));
    PythonString::from_units(units)
}

fn reason_canonical_verb(matched: &CanonicalVerbMatch, role: &PythonString) -> PythonString {
    let mut units: Vec<u32> = "token '".chars().map(u32::from).collect();
    units.extend(matched.token.chars().map(u32::from));
    units.extend("' matched ".chars().map(u32::from));
    units.extend_from_slice(role.codepoints());
    units.extend(" canonical verb".chars().map(u32::from));
    PythonString::from_units(units)
}

fn reason_domain_keyword(keyword: &PythonString) -> PythonString {
    let mut units: Vec<u32> = "domain keyword '".chars().map(u32::from).collect();
    units.extend_from_slice(keyword.codepoints());
    units.extend("' matched".chars().map(u32::from));
    PythonString::from_units(units)
}

/// Code-point equality between Rust text and a Python string.
fn code_units_eq_str(units: &[u32], text: &str) -> bool {
    units.len() == text.chars().count() && units.iter().copied().eq(text.chars().map(u32::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> PythonString {
        PythonString::from_scalars(value)
    }

    fn profile(
        id: &str,
        role: &str,
        priority: i32,
        keywords: &[&str],
        verbs: &[&str],
    ) -> RoutableProfile {
        RoutableProfile::new(
            text(id),
            text(role),
            priority,
            keywords.iter().map(|item| text(item)).collect(),
            verbs.iter().map(|item| text(item)).collect(),
        )
    }

    fn tokens(request: &str) -> Vec<String> {
        normalize_tokens(&text(request))
            .iter()
            .map(|token| token.to_scalar_string().expect("scalar token"))
            .collect()
    }

    #[test]
    fn stop_words_are_the_reference_set() {
        assert_eq!(STOP_WORDS.len(), 45);
        assert!(STOP_WORDS.contains(&"the") && STOP_WORDS.contains(&"please"));
        assert!(
            !STOP_WORDS.contains(&"order"),
            "'order' is a canonical verb"
        );
        let mut sorted = STOP_WORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, STOP_WORDS, "the table stays sorted");
    }

    #[test]
    fn canonical_verb_table_is_the_reference_table() {
        assert_eq!(canonical_verb_entries().len(), 59);
        assert_eq!(
            canonical_verb(&text("audit")),
            Some(CanonicalVerbMatch {
                token: "audit",
                action: "review",
                role: "reviewer"
            })
        );
        assert_eq!(canonical_verb(&text("blueprint")), None);
        assert_eq!(
            canonical_verb(&text("IMPLEMENT")),
            None,
            "tokens arrive lowered"
        );
        // Every role in the map is a canonical role with a capability row.
        for (_, _, role) in canonical_verb_entries() {
            assert!(
                sk_doctrine::role_capabilities(role).is_some(),
                "canonical table covers {role}"
            );
        }
    }

    #[test]
    fn tokenization_matches_the_reference_vectors() {
        assert_eq!(
            tokens("Implement  the widget!"),
            vec!["implement", "widget"]
        );
        assert_eq!(tokens("review_the_change"), vec!["review_the_change"]);
        assert_eq!(tokens("fix-bug"), vec!["fix", "bug"]);
        assert_eq!(tokens("café naïve"), vec!["café", "naïve"]);
        assert_eq!(tokens("İstanbul"), vec!["i", "stanbul"]);
        assert_eq!(tokens("123 456"), vec!["123", "456"]);
        assert_eq!(tokens("   "), Vec::<String>::new());
        assert_eq!(tokens("ΑΣ"), vec!["ας"]);
    }

    #[test]
    fn confidence_round_trips_but_generic_fallback_is_never_produced() {
        for confidence in [
            RouterConfidence::Exact,
            RouterConfidence::CanonicalVerb,
            RouterConfidence::DomainKeyword,
            RouterConfidence::GenericFallback,
        ] {
            assert_eq!(
                RouterConfidence::from_source_str(confidence.as_str()),
                Some(confidence)
            );
        }
        assert_eq!(
            RouterConfidence::GenericFallback.as_str(),
            "generic_fallback"
        );
        let decision = ActionRouter::route_scalars(
            "implement it",
            None,
            &[profile("solo", "implementer", 50, &[], &[])],
        )
        .expect("routed");
        assert_eq!(decision.confidence, RouterConfidence::CanonicalVerb);
    }

    #[test]
    fn empty_catalogue_precedes_the_hint_and_the_message_is_exact() {
        let error = ActionRouter::route(&text("implement it"), Some(&text("x")), &[])
            .expect_err("empty catalogue");
        assert_eq!(error.code(), RouterErrorCode::NoMatch);
        assert!(error.candidates().is_empty());
        assert_eq!(
            error.suggestion().to_scalar_string().expect("scalar"),
            EMPTY_CATALOGUE_SUGGESTION
        );
        assert_eq!(
            error.message().to_scalar_string().expect("scalar"),
            format!("ROUTER_NO_MATCH: {EMPTY_CATALOGUE_SUGGESTION}")
        );
    }

    #[test]
    fn empty_hint_is_an_explicit_miss_with_the_available_repr() {
        let profiles = [
            profile("beta", "reviewer", 50, &[], &[]),
            profile("alpha", "manager", 50, &[], &[]),
        ];
        let error = ActionRouter::route(&text("implement it"), Some(&text("")), &profiles)
            .expect_err("empty hint misses");
        assert_eq!(error.code(), RouterErrorCode::ProfileNotFound);
        assert_eq!(
            error.suggestion().to_scalar_string().expect("scalar"),
            "Profile '' not found. Available: ['alpha', 'beta']"
        );
    }

    #[test]
    fn codepoint_ids_and_hints_survive_the_error_text() {
        let nul =
            PythonString::from_codepoints(&[0x6e, 0x75, 0x6c, 0x00, 0x69, 0x64]).expect("nul id");
        let surrogate =
            PythonString::from_codepoints(&[0x73, 0x75, 0x72, 0xd800, 0x67, 0x61, 0x74, 0x65])
                .expect("surrogate id");
        let profiles = [
            RoutableProfile::new(
                surrogate.clone(),
                text("manager"),
                60,
                Vec::new(),
                Vec::new(),
            ),
            RoutableProfile::new(nul.clone(), text("manager"), 70, Vec::new(), Vec::new()),
        ];
        let error = ActionRouter::route(&text("coordinate"), Some(&text("nope")), &profiles)
            .expect_err("missing hint");
        assert_eq!(
            error.suggestion().to_scalar_string().expect("scalar"),
            "Profile 'nope' not found. Available: ['nul\\x00id', 'sur\\ud800gate']"
        );
        let decision = ActionRouter::route(&text("coordinate"), None, &profiles).expect("routed");
        assert_eq!(decision.profile_id, nul);
        assert_eq!(decision.alternatives[0].profile_id, surrogate);
    }

    #[test]
    fn invalid_code_points_are_refused_by_the_public_constructor() {
        let error = PythonString::from_codepoints(&[0x41, 0x110000]).expect_err("out of range");
        assert_eq!(error.code(), 0x110000);
        assert_eq!(
            PythonString::from_codepoints(&[0xd800, 0xdc00, 0x00])
                .expect("surrogates and NUL are valid code units")
                .codepoints(),
            &[0xd800, 0xdc00, 0x00]
        );
        assert_eq!(
            PythonString::from_codepoints(&[0x41])
                .expect("scalar")
                .to_scalar_string()
                .expect("scalar"),
            "A"
        );
        assert!(
            PythonString::from_codepoints(&[0xd800])
                .expect("surrogate")
                .to_scalar_string()
                .is_err()
        );
    }

    #[test]
    fn capability_lookup_uses_the_one_canonical_table() {
        for role in ["implementer", "reviewer", "curator", "senior-tech-lead"] {
            let row = capabilities(&text(role));
            assert_eq!(row, sk_doctrine::role_capabilities(role));
        }
        assert_eq!(role_default_action(&text("reviewer")), "audit");
        assert_eq!(role_default_action(&text("implementer")), "generate");
        assert_eq!(role_default_action(&text("custom")), "review");
    }

    #[test]
    fn two_tier_and_priority_rules_hold() {
        let profiles = [
            profile("verb-manager", "manager", 10, &[], &["coordinate"]),
            profile("keyword-manager", "curator", 100, &["coordinate"], &[]),
        ];
        let decision =
            ActionRouter::route_scalars("coordinate the release", None, &profiles).expect("routed");
        assert_eq!(decision.profile_id, text("verb-manager"));
        assert_eq!(decision.action, "coordinate");
        assert_eq!(decision.confidence, RouterConfidence::CanonicalVerb);
        assert_eq!(decision.alternatives.len(), 1);
        assert_eq!(
            decision.alternatives[0].confidence,
            RouterConfidence::DomainKeyword
        );
        assert_eq!(decision.alternatives[0].action, "classify");
    }

    #[test]
    fn stable_ties_report_only_the_tied_top_candidates() {
        let profiles = [
            profile("mix-a", "implementer", 50, &[], &[]),
            profile("mix-b", "implementer", 50, &[], &[]),
            profile("mix-kw", "curator", 100, &["planet"], &[]),
        ];
        let error = ActionRouter::route_scalars("implement planet", None, &profiles)
            .expect_err("tied verb tier");
        assert_eq!(error.code(), RouterErrorCode::Ambiguous);
        assert_eq!(error.candidates().len(), 2);
        assert_eq!(error.candidates()[0].profile_id, text("mix-a"));
        assert_eq!(error.candidates()[1].profile_id, text("mix-b"));
        assert_eq!(
            error.suggestion().to_scalar_string().expect("scalar"),
            AMBIGUOUS_SUGGESTION
        );
    }
}

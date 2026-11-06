//! The requirement-ID grammar and the requirement-mapping readiness gate.
//!
//! This is a native reproduction of the pinned upstream authorities:
//!
//! * `specify_cli.requirement_mapping.grammar` (the single requirement-ID
//!   grammar), and
//! * `runtime.next.runtime_bridge_cores._evaluate_requirement_mapping` /
//!   `_evaluate_bare_prose_requirements` plus the parse helpers in
//!   `specify_cli.requirement_mapping`.
//!
//! The grammar is implemented with hand-written scanners instead of a regex
//! engine so the crate keeps its dependency set. The scanners preserve the
//! reference's token-boundary, compound-tail and qualifier semantics:
//!
//! ```text
//! id         := [qualifier "#"] kind "-" digits [suffix]
//! kind       := FR | NFR | SC | C          ; every mission type
//!             | DR | AR | QR               ; research only
//! digits     := DIGIT+                     ; verbatim, width significant
//! suffix     := ASCII letter               ; declared scan: lowercase only
//! qualifier  := slug                       ; foreign citation, never resolved
//! ```
//!
//! Nothing here writes files and nothing re-serializes user content. The
//! messages are the reference's own guard strings, so a caller can emit them
//! verbatim as `guard_failures`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// The reference's `malformed` rejection reason.
pub const MALFORMED: &str = "malformed";
/// The reference's `unknown_spec_id` rejection reason.
pub const UNKNOWN_SPEC_ID: &str = "unknown_spec_id";
/// The reference's `foreign_qualified` rejection reason (never failing).
pub const FOREIGN_QUALIFIED: &str = "foreign_qualified";

/// Rejection reasons that make a work package's mapping incomplete.
pub fn failing_reasons() -> [&'static str; 2] {
    [MALFORMED, UNKNOWN_SPEC_ID]
}

/// One requirement-ID kind.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RequirementKind {
    /// Functional requirement.
    Fr,
    /// Non-functional requirement.
    Nfr,
    /// Constraint.
    Constraint,
    /// Success criterion.
    SuccessCriterion,
    /// Research data-collection requirement.
    DataCollection,
    /// Research analysis requirement.
    Analysis,
    /// Research quality requirement.
    Quality,
}

impl RequirementKind {
    /// The canonical upper-case token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fr => "FR",
            Self::Nfr => "NFR",
            Self::Constraint => "C",
            Self::SuccessCriterion => "SC",
            Self::DataCollection => "DR",
            Self::Analysis => "AR",
            Self::Quality => "QR",
        }
    }

    /// The reference's coverage bucket name for this kind.
    pub const fn bucket(self) -> &'static str {
        match self {
            Self::Fr => "functional",
            Self::Nfr => "non_functional",
            Self::Constraint => "constraint",
            Self::SuccessCriterion => "success_criteria",
            Self::DataCollection => "data_collection",
            Self::Analysis => "analysis",
            Self::Quality => "quality",
        }
    }
}

/// The kind tokens admitted for one mission type.
fn kinds_for(mission_type: &str) -> &'static [RequirementKind] {
    const DEFAULT: [RequirementKind; 4] = [
        RequirementKind::Fr,
        RequirementKind::Nfr,
        RequirementKind::SuccessCriterion,
        RequirementKind::Constraint,
    ];
    const RESEARCH: [RequirementKind; 7] = [
        RequirementKind::Fr,
        RequirementKind::Nfr,
        RequirementKind::SuccessCriterion,
        RequirementKind::Constraint,
        RequirementKind::DataCollection,
        RequirementKind::Analysis,
        RequirementKind::Quality,
    ];
    if mission_type == "research" {
        &RESEARCH
    } else {
        &DEFAULT
    }
}

/// A parsed requirement ID (the reference's `RequirementId` value object).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequirementId {
    /// The requirement kind.
    pub kind: RequirementKind,
    /// Digits, verbatim and width-significant.
    pub digits: String,
    /// Lower-cased suffix letter, when authored.
    pub suffix: Option<char>,
    /// A foreign citation's qualifier slug, when authored.
    pub mission: Option<String>,
}

impl RequirementId {
    /// The unqualified canonical string (`FR-001a`).
    pub fn canonical(&self) -> String {
        let mut value = format!("{}-{}", self.kind.as_str(), self.digits);
        if let Some(suffix) = self.suffix {
            value.push(suffix);
        }
        value
    }

    /// The rendered form: qualified when foreign, canonical otherwise.
    pub fn rendered(&self) -> String {
        match &self.mission {
            Some(mission) => format!("{mission}#{}", self.canonical()),
            None => self.canonical(),
        }
    }

    /// Whether this is a foreign citation (a qualifier was authored).
    pub fn is_foreign(&self) -> bool {
        self.mission.is_some()
    }
}

fn ascii_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn boundary_before(text: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    !text[..index].chars().next_back().is_some_and(ascii_word)
}

fn char_at(text: &str, index: usize) -> Option<char> {
    text[index..].chars().next()
}

/// True when the token ending at `end` is a compound/prose fragment.
///
/// The reference drops `FR-008-mandated` and `FR-002.3` wholly instead of
/// truncating them to `FR-008` / `FR-002`.
pub fn is_compound_tail(text: &str, end: usize) -> bool {
    let Some(lead) = char_at(text, end) else {
        return false;
    };
    if lead != '-' && lead != '.' {
        return false;
    }
    char_at(text, end + lead.len_utf8()).is_some_and(|next| next.is_ascii_alphanumeric())
}

struct KindMatch {
    kind: RequirementKind,
    digits_start: usize,
    digits_end: usize,
    suffix: Option<char>,
    end: usize,
}

fn match_kind_digits(
    text: &str,
    start: usize,
    spec_scan: bool,
    mission_type: &str,
) -> Option<KindMatch> {
    for kind in kinds_for(mission_type) {
        let token = kind.as_str();
        let Some(candidate) = text.get(start..start + token.len()) else {
            continue;
        };
        if !candidate.eq_ignore_ascii_case(token) {
            continue;
        }
        let mut cursor = start + token.len();
        if char_at(text, cursor) != Some('-') {
            continue;
        }
        cursor += 1;
        let digits_start = cursor;
        while char_at(text, cursor).is_some_and(|c| c.is_ascii_digit()) {
            cursor += 1;
        }
        if cursor == digits_start {
            continue;
        }
        let digits_end = cursor;
        let mut suffix = None;
        if let Some(candidate) = char_at(text, cursor) {
            let allowed = if spec_scan {
                candidate.is_ascii_lowercase()
            } else {
                candidate.is_ascii_alphabetic()
            };
            if allowed {
                suffix = Some(candidate.to_ascii_lowercase());
                cursor += candidate.len_utf8();
            }
        }
        if char_at(text, cursor).is_some_and(ascii_word) {
            continue;
        }
        return Some(KindMatch {
            kind: *kind,
            digits_start,
            digits_end,
            suffix,
            end: cursor,
        });
    }
    None
}

/// Try a `slug#ID` qualifier starting at `index`.
fn match_qualified(
    text: &str,
    index: usize,
    spec_scan: bool,
    mission_type: &str,
) -> Option<(usize, RequirementId)> {
    let first = char_at(text, index)?;
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return None;
    }
    let mut cursor = index;
    while char_at(text, cursor)
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        cursor += 1;
    }
    // The slug grammar's optional mid8 tail: `-` followed by eight [0-9A-Z].
    let mut hash_at = (char_at(text, cursor) == Some('#')).then_some(cursor);
    if hash_at.is_none() {
        let mut probe = cursor;
        while probe > index {
            probe -= 1;
            if char_at(text, probe) == Some('-') {
                let tail_start = probe + 1;
                let tail_end = tail_start + 8;
                if let Some(candidate) = text.get(tail_start..tail_end)
                    && candidate
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                    && char_at(text, tail_end) == Some('#')
                {
                    hash_at = Some(tail_end);
                }
                break;
            }
        }
    }
    let hash_at = hash_at?;
    let matched = match_kind_digits(text, hash_at + 1, spec_scan, mission_type)?;
    Some((
        matched.end,
        RequirementId {
            kind: matched.kind,
            digits: text[matched.digits_start..matched.digits_end].to_string(),
            suffix: matched.suffix,
            mission: Some(text[index..hash_at].to_string()),
        },
    ))
}

struct TokenHit {
    start: usize,
    end: usize,
    id: RequirementId,
}

fn match_token(text: &str, index: usize, spec_scan: bool, mission_type: &str) -> Option<TokenHit> {
    if !boundary_before(text, index) {
        return None;
    }
    if let Some((end, id)) = match_qualified(text, index, spec_scan, mission_type) {
        return Some(TokenHit {
            start: index,
            end,
            id,
        });
    }
    let matched = match_kind_digits(text, index, spec_scan, mission_type)?;
    Some(TokenHit {
        start: index,
        end: matched.end,
        id: RequirementId {
            kind: matched.kind,
            digits: text[matched.digits_start..matched.digits_end].to_string(),
            suffix: matched.suffix,
            mission: None,
        },
    })
}

fn has_invalid_qualifier_prefix(text: &str, hit: &TokenHit) -> bool {
    if hit.start == 0 {
        return false;
    }
    let lead = text[..hit.start].chars().next_back();
    match lead {
        Some('#') => true,
        Some('.') => hit.id.mission.is_some(),
        _ => false,
    }
}

/// Every requirement ID token in `text`.
///
/// `spec_scan` restricts suffixes to lower case (the declared-shape scan);
/// ref matching is case-tolerant. Compound tails and leaked/truncated
/// qualifiers drop the whole token.
pub fn find_all(text: &str, spec_scan: bool, mission_type: &str) -> Vec<RequirementId> {
    let mut found = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(character) = char_at(text, cursor) else {
            break;
        };
        if character.is_ascii_alphabetic()
            && let Some(hit) = match_token(text, cursor, spec_scan, mission_type)
            && !is_compound_tail(text, hit.end)
            && !has_invalid_qualifier_prefix(text, &hit)
        {
            found.push(hit.id);
        }
        cursor += character.len_utf8();
    }
    found
}

/// Strict full-match parse (the reference's `grammar.parse`).
pub fn parse(token: &str, mission_type: &str) -> Option<RequirementId> {
    let (cursor, mission) = match qualified_slug_prefix(token) {
        Some((hash_at, true)) => (hash_at + 1, Some(token[..hash_at].to_string())),
        Some((_, false)) => return None,
        None => (0, None),
    };
    let matched = match_kind_digits(token, cursor, false, mission_type)?;
    if matched.end != token.len() {
        return None;
    }
    if mission_type != "research" && matched.kind.bucket() == "data_collection" {
        return None;
    }
    Some(RequirementId {
        kind: matched.kind,
        digits: token[matched.digits_start..matched.digits_end].to_string(),
        suffix: matched.suffix,
        mission,
    })
}

fn qualified_slug_prefix(token: &str) -> Option<(usize, bool)> {
    let hash_at = token.find('#')?;
    let slug = &token[..hash_at];
    if slug.is_empty() {
        return Some((hash_at, false));
    }
    let first = slug.chars().next()?;
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Some((hash_at, false));
    }
    let mut valid = slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        // Allow the optional eight-character mid8 tail after a dash.
        if let Some(dash) = slug.rfind('-') {
            let tail = &slug[dash + 1..];
            valid = tail.len() == 8
                && tail
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && slug[..dash]
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        }
    }
    Some((hash_at, valid))
}

/// The canonical string for `token` (qualified when the token was).
pub fn canonical(token: &str, mission_type: &str) -> Option<String> {
    parse(token, mission_type).map(|id| id.rendered())
}

/// Blank every HTML comment with spaces, preserving line/character positions.
pub fn blank_html_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if text[cursor..].starts_with("<!--") {
            let end = text[cursor + 4..]
                .find("-->")
                .map(|offset| cursor + 4 + offset + 3)
                .unwrap_or(text.len());
            for character in text[cursor..end].chars() {
                output.push(if character == '\n' { '\n' } else { ' ' });
            }
            cursor = end;
            continue;
        }
        let character = char_at(text, cursor).expect("cursor is a char boundary");
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\u{0c}')
}

fn skip_space(text: &str, mut cursor: usize) -> usize {
    while char_at(text, cursor).is_some_and(is_space) {
        cursor += 1;
    }
    cursor
}

fn skip_wrappers(text: &str, mut cursor: usize, allow_bold: bool, allow_strike: bool) -> usize {
    let mut count = 0;
    while count < 2 {
        if allow_bold && text[cursor..].starts_with("**") {
            cursor += 2;
            count += 1;
            continue;
        }
        if allow_strike && text[cursor..].starts_with("~~") {
            cursor += 2;
            count += 1;
            continue;
        }
        break;
    }
    cursor
}

fn match_declared_core(
    text: &str,
    start: usize,
    mission_type: &str,
) -> Option<(usize, RequirementId)> {
    let matched = match_kind_digits(text, start, true, mission_type)?;
    Some((
        matched.end,
        RequirementId {
            kind: matched.kind,
            digits: text[matched.digits_start..matched.digits_end].to_string(),
            suffix: matched.suffix,
            mission: None,
        },
    ))
}

fn boundary_after(text: &str, end: usize) -> bool {
    !char_at(text, end).is_some_and(ascii_word)
}

/// The first declared-shape ID on one line (the reference's four shapes).
///
/// Matching stops at the first pattern that produces a well-formed ID, which
/// is what excludes a citation inside a table row's description cell.
pub fn declared_id_on_line(line: &str, mission_type: &str) -> Option<RequirementId> {
    // Table row: `| **FR-001** | ... |`
    let mut cursor = skip_space(line, 0);
    if char_at(line, cursor) == Some('|') {
        cursor = skip_space(line, cursor + 1);
        let wrapped = skip_wrappers(line, cursor, true, true);
        if let Some((end, id)) = match_declared_core(line, wrapped, mission_type) {
            let after = skip_wrappers(line, end, true, true);
            let after = skip_space(line, after);
            if char_at(line, after) == Some('|') && !is_compound_tail(line, end) {
                return Some(id);
            }
        }
    }
    // Heading: `### FR-001`
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let after = skip_space(line, hashes);
        // `#FR-001` is not a heading: the reference requires whitespace after
        // the hashes, so `after > hashes` is the shape gate.
        if after > hashes
            && let Some((end, id)) = match_declared_core(line, after, mission_type)
            && boundary_after(line, end)
            && !is_compound_tail(line, end)
        {
            return Some(id);
        }
    }
    // Bulleted / numbered item: `- **FR-001**: ...`, `1. FR-001 ...`
    let mut cursor = skip_space(line, 0);
    let mut is_list = false;
    if matches!(char_at(line, cursor), Some('-' | '*')) {
        is_list = true;
        cursor += 1;
    } else {
        let digits = line[cursor..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        if digits > 0 && char_at(line, cursor + digits) == Some('.') {
            is_list = true;
            cursor += digits + 1;
        }
    }
    if is_list {
        let after = skip_space(line, cursor);
        let wrapped = skip_bold(line, after);
        if let Some((end, id)) = match_declared_core(line, wrapped, mission_type)
            && boundary_after(line, end)
            && !is_compound_tail(line, end)
        {
            return Some(id);
        }
    }
    // Bold-led paragraph: `**FR-001 — Title.**`
    let mut cursor = skip_space(line, 0);
    if line[cursor..].starts_with("**") {
        cursor += 2;
        if let Some((end, id)) = match_declared_core(line, cursor, mission_type)
            && boundary_after(line, end)
            && !is_compound_tail(line, end)
        {
            return Some(id);
        }
    }
    None
}

fn skip_bold(text: &str, mut cursor: usize) -> usize {
    let mut count = 0;
    while count < 2 && text[cursor..].starts_with('*') {
        cursor += 1;
        count += 1;
    }
    cursor
}

/// Every ID the spec genuinely *declares* (not merely cites).
pub fn declared_ids(spec_content: &str, mission_type: &str) -> BTreeSet<String> {
    let blanked = blank_html_comments(spec_content);
    let mut found = BTreeSet::new();
    for line in blanked.split('\n') {
        if let Some(id) = declared_id_on_line(line, mission_type) {
            found.insert(id.canonical());
        }
    }
    found
}

/// The declared requirement IDs, bucketed exactly as the reference does.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeclaredRequirementIds {
    /// Every declared canonical id, sorted.
    pub all: Vec<String>,
    /// Declared functional requirements, sorted.
    pub functional: Vec<String>,
    /// Declared non-functional requirements, sorted.
    pub non_functional: Vec<String>,
    /// Declared constraints, sorted.
    pub constraint: Vec<String>,
    /// Declared success criteria, sorted.
    pub success_criteria: Vec<String>,
    /// Research data-collection requirements, when any.
    pub data_collection: Vec<String>,
    /// Research analysis requirements, when any.
    pub analysis: Vec<String>,
    /// Research quality requirements, when any.
    pub quality: Vec<String>,
}

/// Parse the DECLARED requirement IDs from spec.md content.
pub fn parse_requirement_ids_from_spec_md(
    spec_content: &str,
    mission_type: &str,
) -> DeclaredRequirementIds {
    let declared = declared_ids(spec_content, mission_type);
    let mut buckets: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for id in &declared {
        if let Some(parsed) = parse(id, mission_type) {
            buckets
                .entry(parsed.kind.bucket())
                .or_default()
                .push(id.clone());
        }
    }
    let mut take = |name: &str| buckets.remove(name).unwrap_or_default();
    DeclaredRequirementIds {
        all: declared.into_iter().collect(),
        functional: take("functional"),
        non_functional: take("non_functional"),
        constraint: take("constraint"),
        success_criteria: take("success_criteria"),
        data_collection: take("data_collection"),
        analysis: take("analysis"),
        quality: take("quality"),
    }
}

/// Raw `requirement_refs` tokens from a frontmatter value, case preserved.
///
/// A list keeps its string items verbatim; a non-string item becomes
/// `<NON_STRING:...>`. A scalar string splits on commas/whitespace.
pub fn tokenize_refs(value: &Value) -> Vec<String> {
    let mut tokens = Vec::new();
    match value {
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(text) => split_scalar(text, &mut tokens),
                    other => tokens.push(format!("<NON_STRING:{}>", python_repr(other))),
                }
            }
        }
        Value::String(text) => split_scalar(text, &mut tokens),
        _ => {}
    }
    tokens
}

fn split_scalar(text: &str, tokens: &mut Vec<String>) {
    for token in text.split([',', ' ', '\t', '\n', '\r']) {
        if !token.trim().is_empty() {
            tokens.push(token.to_string());
        }
    }
}

/// Python's `str()` of a non-`str` frontmatter item, as the reference's
/// `<NON_STRING:{item}>` f-string renders it.
///
/// Strings go through the one canonical Kernel repr authority
/// (`sk_kernel::python_repr`: CPython quote choice and escapes), never a raw
/// `'…'` wrapper — the reference interleaves `str()`/`repr()` for containers,
/// so a nested string is repr'd and the container itself is `str()`-rendered.
fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => sk_kernel::python_repr(text),
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(container_repr).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| {
                    format!("{}: {}", sk_kernel::python_repr(key), container_repr(value))
                })
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

/// Python `repr()` of a value nested inside a container.
///
/// `repr` is `str` for every scalar except strings (which are quoted by the
/// canonical Kernel formatter) and containers (which recurse).
fn container_repr(value: &Value) -> String {
    match value {
        Value::String(text) => sk_kernel::python_repr(text),
        other => python_repr(other),
    }
}

/// One per-ref verdict of the shared FR-019 classification table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefVerdict {
    /// The ref parsed, is unqualified and is declared.
    Accepted(RequirementId),
    /// The ref was rejected with exactly one reason.
    Rejected {
        /// The authored token.
        raw: String,
        /// One of [`MALFORMED`], [`UNKNOWN_SPEC_ID`], [`FOREIGN_QUALIFIED`].
        reason: &'static str,
    },
}

/// Apply the FR-019 verdict table to `raw` against the declared set.
pub fn classify(raw: &str, declared: &BTreeSet<String>, mission_type: &str) -> RefVerdict {
    let Some(requirement_id) = parse(raw, mission_type) else {
        return RefVerdict::Rejected {
            raw: raw.to_string(),
            reason: MALFORMED,
        };
    };
    if requirement_id.is_foreign() {
        return RefVerdict::Rejected {
            raw: raw.to_string(),
            reason: FOREIGN_QUALIFIED,
        };
    }
    if !declared.contains(&requirement_id.canonical()) {
        return RefVerdict::Rejected {
            raw: raw.to_string(),
            reason: UNKNOWN_SPEC_ID,
        };
    }
    RefVerdict::Accepted(requirement_id)
}

/// The gathered facts the pure requirement-mapping decision needs.
#[derive(Clone, Debug, Default)]
pub struct RequirementMappingFacts {
    /// Every declared canonical requirement id.
    pub spec_requirement_ids: BTreeSet<String>,
    /// Every declared functional requirement id.
    pub functional_requirement_ids: BTreeSet<String>,
    /// Work-package ids, in the reference's sorted order.
    pub wp_ids: Vec<String>,
    /// Raw `requirement_refs` tokens per work package.
    pub wp_requirement_refs: BTreeMap<String, Vec<String>>,
    /// The mission directory's display name.
    pub feature_dir_name: String,
    /// The mission type (research admits its extra kinds).
    pub mission_type: String,
}

/// The pure decision tail of the requirement-mapping readiness check.
///
/// A work package is *missing* when it has no accepted ref; a foreign or
/// unknown ref is reported as rejected. Returns the reference's single,
/// actionable guard string, or nothing when the mapping is complete.
pub fn evaluate_requirement_mapping(facts: &RequirementMappingFacts) -> Vec<String> {
    let mut missing = Vec::new();
    let mut rejected: BTreeMap<String, Vec<(String, &'static str)>> = BTreeMap::new();
    let mut mapped = BTreeSet::new();

    for wp_id in &facts.wp_ids {
        let empty = Vec::new();
        let refs = facts.wp_requirement_refs.get(wp_id).unwrap_or(&empty);
        let mut accepted = Vec::new();
        let mut wp_rejected = Vec::new();
        for raw in refs {
            match classify(raw, &facts.spec_requirement_ids, &facts.mission_type) {
                RefVerdict::Accepted(id) => accepted.push(id.canonical()),
                RefVerdict::Rejected { raw, reason } if failing_reasons().contains(&reason) => {
                    wp_rejected.push((raw, reason));
                }
                RefVerdict::Rejected { .. } => {}
            }
        }
        if !wp_rejected.is_empty() {
            wp_rejected.sort();
            rejected.insert(wp_id.clone(), wp_rejected);
        }
        if accepted.is_empty() {
            missing.push(wp_id.clone());
        }
        mapped.extend(accepted);
    }

    let unmapped: Vec<String> = facts
        .functional_requirement_ids
        .iter()
        .filter(|id| !mapped.contains(*id))
        .cloned()
        .collect();

    if missing.is_empty() && rejected.is_empty() && unmapped.is_empty() {
        return Vec::new();
    }

    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing refs for WPs: {}", missing.join(", ")));
    }
    if !rejected.is_empty() {
        let parts: Vec<String> = rejected
            .iter()
            .map(|(wp_id, refs)| {
                let rendered: Vec<String> = refs
                    .iter()
                    .map(|(raw, reason)| format!("{raw} ({reason})"))
                    .collect();
                format!("{wp_id}: {}", rendered.join(", "))
            })
            .collect();
        details.push(format!("rejected refs: {}", parts.join("; ")));
    }
    if !unmapped.is_empty() {
        details.push(format!("unmapped FRs: {}", unmapped.join(", ")));
    }

    vec![format!(
        "Requirement mapping incomplete before finalize-tasks: {}. Run `spec-kitty agent tasks map-requirements --batch ... --mission {} --json` or update WP requirement_refs before finalizing.",
        details.join("; "),
        facts.feature_dir_name
    )]
}

/// One requirement-named section that carries undeclared bare-prose ids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BareProseCandidate {
    /// The section heading text (without `#`).
    pub section_heading: String,
    /// The undeclared canonical ids found in that section's prose.
    pub ids: Vec<String>,
}

fn is_requirement_heading(line: &str) -> bool {
    line.trim_start().starts_with('#') && line.to_ascii_lowercase().contains("requirement")
}

fn requirement_named_sections(spec_content: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = spec_content.split('\n').collect();
    let heading_indexes: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with('#'))
        .map(|(index, _)| index)
        .collect();
    let mut sections = Vec::new();
    for (position, index) in heading_indexes.iter().enumerate() {
        let heading_line = lines[*index];
        if !is_requirement_heading(heading_line) {
            continue;
        }
        let end = heading_indexes
            .get(position + 1)
            .copied()
            .unwrap_or(lines.len());
        let heading = heading_line.trim_start_matches('#').trim().to_string();
        sections.push((heading, lines[index + 1..end].join("\n")));
    }
    sections
}

fn unqualified_unsuffixed_ids(text: &str) -> Vec<RequirementId> {
    find_all(text, true, "software-dev")
        .into_iter()
        .filter(|id| {
            id.mission.is_none()
                && id.suffix.is_none()
                && matches!(
                    id.kind,
                    RequirementKind::Fr | RequirementKind::Nfr | RequirementKind::Constraint
                )
        })
        .collect()
}

/// Undeclared, bare-prose requirement tokens under a requirement-named section.
pub fn find_bare_prose_requirement_ids(spec_content: &str) -> Vec<BareProseCandidate> {
    let document_declared = declared_ids(spec_content, "software-dev");
    let mut result = Vec::new();
    for (heading, body) in requirement_named_sections(spec_content) {
        let mut section_ids: Vec<String> = Vec::new();
        for line in body.split('\n') {
            if declared_id_on_line(line, "software-dev").is_some() {
                continue;
            }
            for id in unqualified_unsuffixed_ids(line) {
                let token = id.canonical();
                if !document_declared.contains(&token) && !section_ids.contains(&token) {
                    section_ids.push(token);
                }
            }
        }
        if !section_ids.is_empty() {
            result.push(BareProseCandidate {
                section_heading: heading,
                ids: section_ids,
            });
        }
    }
    result
}

/// The blocking guard string for bare-prose requirement ids, or nothing.
pub fn evaluate_bare_prose_requirements(candidates: &[BareProseCandidate]) -> Vec<String> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let mut ordered: Vec<&BareProseCandidate> = candidates.iter().collect();
    ordered.sort_by(|left, right| left.section_heading.cmp(&right.section_heading));
    let details: Vec<String> = ordered
        .iter()
        .map(|candidate| {
            format!(
                "{}: {}",
                candidate.section_heading,
                candidate.ids.join(", ")
            )
        })
        .collect();
    vec![format!(
        "Bare-prose requirement id(s) found, uncounted by requirement mapping: {}. Rewrite them in a recognized declared shape (table row / id-naming heading / bulleted item / bold-led paragraph) before finalizing.",
        details.join("; ")
    )]
}

/// Extract the per-work-package sections of a `tasks.md` document.
fn wp_sections_from_tasks_md(tasks_content: &str) -> BTreeMap<String, String> {
    let mut matches: Vec<(String, usize, usize)> = Vec::new();
    let bytes = tasks_content.as_bytes();
    let mut search_at = 0;
    while search_at < bytes.len() {
        let Some(found) = tasks_content[search_at..].find("WP").map(|o| search_at + o) else {
            break;
        };
        let line_start = tasks_content[..found].rfind('\n').map_or(0, |i| i + 1);
        let line_end = tasks_content[found..]
            .find('\n')
            .map_or(tasks_content.len(), |i| found + i + 1);
        search_at = line_end;
        if !tasks_content[line_start..].starts_with("##") {
            continue;
        }
        let line = &tasks_content[line_start..line_end];
        if let Some((wp_id, matched)) = extract_wp_heading(line) {
            matches.push((wp_id, line_start + matched, line_start));
        }
    }
    let mut sections = BTreeMap::new();
    for (index, (wp_id, start, _)) in matches.iter().enumerate() {
        let end = matches
            .get(index + 1)
            .map(|(_, _, line_start)| *line_start)
            .unwrap_or(tasks_content.len());
        sections.insert(wp_id.clone(), tasks_content[*start..end].to_string());
    }
    sections
}

fn extract_wp_heading(line: &str) -> Option<(String, usize)> {
    let heading_level = line.chars().take_while(|c| *c == '#').count();
    if !(2..=4).contains(&heading_level) {
        return None;
    }
    let mut cursor = heading_level;
    if !char_at(line, cursor).is_some_and(is_space) {
        return None;
    }
    cursor = skip_space(line, cursor);
    let prefix = "Work Package";
    if line[cursor..].starts_with(prefix) {
        let prefix_end = cursor + prefix.len();
        if !char_at(line, prefix_end).is_some_and(is_space) {
            return None;
        }
        cursor = skip_space(line, prefix_end);
    }
    if !line[cursor..].starts_with("WP") {
        return None;
    }
    let digit_start = cursor + 2;
    let digit_end = digit_start + 2;
    let digits = line.get(digit_start..digit_end)?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let wp_id = line[cursor..digit_end].to_string();
    if digit_end == line.len() {
        return Some((wp_id, digit_end));
    }
    let trailing = char_at(line, digit_end).expect("checked");
    if trailing == ':' || !(trailing.is_ascii_alphanumeric() || trailing == '_') {
        return Some((wp_id, digit_end));
    }
    None
}

fn requirement_refs_for_section(section: &str, mission_type: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let mut in_requirement_ref_list = false;
    for line in section.split('\n') {
        let stripped = line.trim();
        if in_requirement_ref_list {
            if stripped.is_empty() {
                continue;
            }
            if stripped.starts_with('-') || stripped.starts_with('*') {
                for id in find_all(stripped, false, mission_type) {
                    push_unique(&mut refs, id.rendered());
                }
                continue;
            }
            in_requirement_ref_list = false;
        }
        if let Some(suffix) = requirement_inline_refs_suffix(line) {
            for id in find_all(suffix, false, mission_type) {
                push_unique(&mut refs, id.rendered());
            }
            continue;
        }
        if is_requirement_refs_heading(stripped) {
            in_requirement_ref_list = true;
        }
    }
    refs
}

fn push_unique(refs: &mut Vec<String>, value: String) {
    if !refs.contains(&value) {
        refs.push(value);
    }
}

fn requirement_inline_refs_suffix(line: &str) -> Option<&str> {
    if !line.to_ascii_lowercase().contains("requirement") {
        return None;
    }
    let (prefix, suffix) = line.split_once(':')?;
    prefix
        .to_ascii_lowercase()
        .contains("requirement")
        .then_some(suffix)
}

fn is_requirement_refs_heading(stripped: &str) -> bool {
    if !stripped.starts_with('#') {
        return false;
    }
    let body = stripped.trim_start_matches('#').trim();
    if body.is_empty() {
        return false;
    }
    matches!(
        body.replace('*', "").trim().to_ascii_lowercase().as_str(),
        "requirement" | "requirements" | "requirement refs" | "requirements refs"
    )
}

/// Parse the legacy `tasks.md` fallback requirement refs per work package.
pub fn parse_requirement_refs_from_tasks_md(
    tasks_content: &str,
    mission_type: &str,
) -> BTreeMap<String, Vec<String>> {
    wp_sections_from_tasks_md(tasks_content)
        .into_iter()
        .map(|(wp_id, section)| (wp_id, requirement_refs_for_section(&section, mission_type)))
        .collect()
}

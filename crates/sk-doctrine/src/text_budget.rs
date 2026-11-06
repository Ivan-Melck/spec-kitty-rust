//! Source character-budget and fetch-substitution engine (WP05/NFR-001).
//!
//! One descriptor/outcome model over the exact [`PythonString`] carrier plus the
//! two reference policies:
//!
//! * [`apply_token_budget`] — Source `apply_token_budget`: joins only nonempty
//!   header/body pieces, treats a nonpositive budget as a no-op, and repeatedly
//!   swaps the longest substitutable body for its canonical fetch stanza
//!   (ties on body length broken by `section_id` ascending) until the joined
//!   text fits the warning-reserved budget or no candidate remains.
//! * [`enforce_token_budget`] — Source `_enforce_token_budget`: returns the text
//!   unchanged when it already fits, otherwise collects one candidate per
//!   action-critical heading, per profile kind-block and per selected group,
//!   sorts longest-first, and replaces the FIRST occurrence of each candidate
//!   body anywhere in the whole text. A candidate whose replacement leaves the
//!   text identical is skipped. A nonpositive budget still substitutes here;
//!   only `apply` treats it as a no-op.
//!
//! Budgets count Python characters (`len(str)`), not UTF-8 bytes, tokens or
//! graphemes: one code point is one unit whether it is NUL, an ASCII letter, a
//! lone surrogate unit or an astral scalar. Authored code points are carried as
//! data and never normalized. Both engines may end above the budget — Source has
//! no hard total-size guarantee, no truncation and no valid-input refusal when
//! no candidate remains (for example a replacement longer than the body it
//! replaces). The trailing warning line reserves its own characters exactly; the
//! budget is a finite `i128` adapter over Source's arbitrary Python integer, so
//! the default 40000 is exact, negative budgets remain representable, and a
//! value outside `i128` is not silently clamped — the type cannot express it.

use sk_kernel::PythonString;
use sk_kernel::python_text::is_whitespace;

use crate::context_render::{
    DEFAULT_WHEN_CLAUSE, FetchScope, critical_section_selector_exact,
    critical_section_when_clause_exact, exact_strip, fetch_command, fetch_stanza_lines_exact,
};

/// Default character budget — Source `BUDGET_DEFAULT` (NFR-001 pin).
///
/// Raised 32_000 -> 40_000 on 2026-08-12 after the software-dev doctrine cascade
/// grew the essential Action Doctrine payload; the stale 32000 intro is history,
/// not policy.
pub const BUDGET_DEFAULT: i128 = 40_000;

/// Per-entry inline-body character ceiling — Source
/// `_PROFILE_INLINE_BODY_LIMIT_CHARS` (~2400 estimated characters).
pub const PROFILE_INLINE_BODY_LIMIT_CHARS: i128 = 2_400;

/// One rendered section eligible for budget substitution.
///
/// `header` is emitted verbatim and is never swapped. `body` is the substitutable
/// payload. A section is substitutable only when `substitutable` is true, its body
/// is nonempty, and it carries either a nonempty `selector` or an explicit
/// `replacement_body`; an empty `replacement_body` is distinct from `None` and
/// substitutes to an empty body. `indent` aligns the generated stanza; a
/// `replacement_body` is used verbatim (Source's global-selection groups).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedSection {
    /// Stable identifier used for tie-breaking and substitution notes.
    pub section_id: PythonString,
    /// Leading anchor line(s) emitted verbatim; never substituted.
    pub header: PythonString,
    /// The verbatim body that a substitution replaces.
    pub body: PythonString,
    /// Canonical `<kind>:<identifier>` fetch selector, or empty.
    pub selector: PythonString,
    /// Verb phrase completing `When you <clause>, ...`; empty uses the default.
    pub when_doing_clause: PythonString,
    /// Whether the body may be replaced with a fetch stanza.
    pub substitutable: bool,
    /// Indentation applied to generated stanza lines.
    pub indent: PythonString,
    /// Pre-rendered compact replacement; `Some("")` is an empty replacement.
    pub replacement_body: Option<PythonString>,
}

impl Default for RenderedSection {
    fn default() -> Self {
        Self {
            section_id: PythonString::default(),
            header: PythonString::default(),
            body: PythonString::default(),
            selector: PythonString::default(),
            when_doing_clause: PythonString::default(),
            substitutable: true,
            indent: PythonString::default(),
            replacement_body: None,
        }
    }
}

impl RenderedSection {
    /// A substitutable section with no selector (scalar convenience constructor).
    pub fn new(section_id: &str, header: &str, body: &str) -> Self {
        Self {
            section_id: PythonString::from_scalars(section_id),
            header: PythonString::from_scalars(header),
            body: PythonString::from_scalars(body),
            ..Self::default()
        }
    }

    /// Set the fetch selector.
    pub fn with_selector(mut self, selector: &str) -> Self {
        self.selector = PythonString::from_scalars(selector);
        self
    }

    /// Set the when-doing clause.
    pub fn with_when_clause(mut self, clause: &str) -> Self {
        self.when_doing_clause = PythonString::from_scalars(clause);
        self
    }

    /// Set the stanza indentation.
    pub fn with_indent(mut self, indent: &str) -> Self {
        self.indent = PythonString::from_scalars(indent);
        self
    }

    /// Set an explicit compact replacement (an empty string is meaningful).
    pub fn with_replacement_body(mut self, body: &str) -> Self {
        self.replacement_body = Some(PythonString::from_scalars(body));
        self
    }

    /// Mark the section as always inline.
    pub fn non_substitutable(mut self) -> Self {
        self.substitutable = false;
        self
    }
}

/// The result of [`apply_token_budget`]: joined text plus swap notes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BudgetOutcome {
    /// The joined (possibly substituted) text.
    pub text: PythonString,
    /// Human-readable substitution notes, in swap order.
    pub notes: Vec<PythonString>,
}

/// Source `warning_line`: the trailing budget line for *count* swaps.
pub fn warning_line(count: usize, budget: i128) -> PythonString {
    PythonString::from_scalars(&format!(
        "# Governance payload: {count} sections substituted with fetch commands (budget={budget})."
    ))
}

/// The exact remaining budget is below this explicit i128 projection. This
/// is a Native adapter boundary, never a Source ValueError or saturated value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Native finite character-budget adapter cannot represent budget minus warning reservation")]
pub struct BudgetRemainingError {
    /// The original, still-exact input budget.
    pub budget: i128,
    /// Number of actual substitutions.
    pub count: usize,
    /// Warning separator and line length in code points.
    pub reserved_chars: usize,
}

/// Checked finite view of Source budget_without_warning_line.
///
/// # Errors
/// The exact subtraction falls below i128::MIN; no clamping or panic occurs.
pub fn budget_without_warning_line(
    count: usize,
    budget: i128,
) -> Result<i128, BudgetRemainingError> {
    if count == 0 {
        return Ok(budget);
    }
    let reserved_chars = 2 + chars(&warning_line(count, budget));
    budget
        .checked_sub(i128::try_from(reserved_chars).expect("bounded warning line"))
        .ok_or(BudgetRemainingError {
            budget,
            count,
            reserved_chars,
        })
}

/// Compare without projecting an unrepresentable negative threshold: every
/// nonnegative text length exceeds a mathematical result below i128::MIN.
fn exceeds_warning_budget(text: &PythonString, count: usize, budget: i128) -> bool {
    match budget_without_warning_line(count, budget) {
        Ok(remaining) => chars_as_i128(text) > remaining,
        Err(BudgetRemainingError { .. }) => true,
    }
}

/// Source `_budget_estimate`: `sum(len(line) + 1)`, including the final line.
pub fn budget_estimate(lines: &[PythonString]) -> i128 {
    lines.iter().fold(0, |total, line| {
        total + i128::try_from(chars(line)).expect("bounded line") + 1
    })
}

/// Combine sections into one text block (Source `_join_sections`).
///
/// Each section contributes its nonempty header then its nonempty body joined by
/// a newline, with a blank line between sections; a section with neither
/// contributes nothing.
pub fn join_sections(sections: &[RenderedSection]) -> PythonString {
    let mut blocks: Vec<PythonString> = Vec::new();
    for section in sections {
        let mut pieces: Vec<&PythonString> = Vec::new();
        if !section.header.is_empty() {
            pieces.push(&section.header);
        }
        if !section.body.is_empty() {
            pieces.push(&section.body);
        }
        if pieces.is_empty() {
            continue;
        }
        blocks.push(join(&pieces, "\n"));
    }
    join_refs(&blocks, "\n\n")
}

/// Source `apply_token_budget`: join *sections* under *budget* characters.
///
/// A nonpositive budget returns the plain join with no substitutions and no
/// warning line. Otherwise, while the text is over the warning-reserved budget,
/// the longest substitutable section is replaced (ties by `section_id`
/// ascending) and the same joined text is re-evaluated; the loop stops when the
/// budget holds or no candidate remains. Swapped sections are never re-swapped,
/// and every swap adds one note. A warning line is appended only when at least
/// one swap happened.
pub fn apply_token_budget(
    sections: &[RenderedSection],
    budget: i128,
    scope: Option<&FetchScope<'_>>,
) -> BudgetOutcome {
    if budget <= 0 {
        return BudgetOutcome {
            text: join_sections(sections),
            notes: Vec::new(),
        };
    }
    let mut current = sections.to_vec();
    let mut notes: Vec<PythonString> = Vec::new();
    let mut text = join_sections(&current);
    if chars_as_i128(&text) <= budget {
        return BudgetOutcome { text, notes };
    }
    while exceeds_warning_budget(&text, notes.len(), budget) {
        let Some(target_index) = (0..current.len())
            .filter(|&index| {
                let section = &current[index];
                section.substitutable
                    && (!section.selector.is_empty() || section.replacement_body.is_some())
                    && !section.body.is_empty()
            })
            .min_by(|&left, &right| {
                chars(&current[right].body)
                    .cmp(&chars(&current[left].body))
                    .then(current[left].section_id.cmp(&current[right].section_id))
            })
        else {
            // Nothing left to swap: Source breaks out and lets the caller see
            // the over-budget text (with its warning for any notes so far).
            break;
        };
        let target = current[target_index].clone();
        let replaced = substituted_section(&target, scope);
        let body_chars = chars(&target.body);
        current[target_index] = replaced;
        let mut note = target.section_id.clone();
        note.push_scalars(&format!(" (body {body_chars} chars -> fetch stanza)"));
        notes.push(note);
        text = join_sections(&current);
    }
    if !notes.is_empty() {
        text.push_scalars("\n\n");
        text.append(&warning_line(notes.len(), budget));
    }
    BudgetOutcome { text, notes }
}

/// Source `_enforce_token_budget`: budget an already-rendered bootstrap text.
///
/// `section_block`, `profile_block` and `selection_block` are the exact blocks
/// the render owner already emitted into `text`; they are read once here, never
/// re-rendered. `action` reaches only the single-block fallback selector.
pub fn enforce_token_budget(
    text: &PythonString,
    action: &PythonString,
    profile_block: &PythonString,
    section_block: &PythonString,
    selection_block: &PythonString,
    budget: i128,
    scope: Option<&FetchScope<'_>>,
) -> PythonString {
    if chars_as_i128(text) <= budget {
        return text.clone();
    }
    let mut candidates = collect_section_block_candidates(section_block, action);
    candidates.extend(collect_profile_block_candidates(profile_block));
    candidates.extend(collect_selection_block_candidates(selection_block, scope));
    if candidates.is_empty() {
        // Nothing safe to substitute: Source returns the original text rather
        // than dropping content or refusing the valid input.
        return text.clone();
    }
    candidates.sort_by(|left, right| {
        chars(&right.body)
            .cmp(&chars(&left.body))
            .then(left.section_id.cmp(&right.section_id))
    });
    let mut current = text.clone();
    let mut swapped: Vec<PythonString> = Vec::new();
    for section in &candidates {
        if !exceeds_warning_budget(&current, swapped.len(), budget) {
            break;
        }
        let stanza = match &section.replacement_body {
            Some(replacement) => replacement.clone(),
            None => stanza_for(section, scope),
        };
        let Some(next) = replace_first(&current, &section.body, &stanza) else {
            // Defensive: the block is no longer present (renderer drift).
            continue;
        };
        if next == current {
            continue;
        }
        current = next;
        swapped.push(section.section_id.clone());
    }
    if !swapped.is_empty() {
        current.push_scalars("\n\n");
        current.append(&warning_line(swapped.len(), budget));
    }
    current
}

/// The unchanged-or-substituted copy of one section (Source
/// `_substituted_section`). Already-substituted sections are never re-swapped.
fn substituted_section(
    section: &RenderedSection,
    scope: Option<&FetchScope<'_>>,
) -> RenderedSection {
    let clause = if section.when_doing_clause.is_empty() {
        PythonString::from_scalars(DEFAULT_WHEN_CLAUSE)
    } else {
        section.when_doing_clause.clone()
    };
    let replacement = match &section.replacement_body {
        Some(replacement) => replacement.clone(),
        None if !section.selector.is_empty() => stanza_for(section, scope),
        // Guarded by the candidate filter; keep the original rather than
        // silently dropping content with no recovery rail.
        None => return section.clone(),
    };
    RenderedSection {
        section_id: section.section_id.clone(),
        header: section.header.clone(),
        body: replacement,
        selector: section.selector.clone(),
        when_doing_clause: clause,
        substitutable: false,
        indent: section.indent.clone(),
        replacement_body: None,
    }
}

/// The stanza for one candidate (Source `fetch_stanza_lines` joined by LF).
fn stanza_for(section: &RenderedSection, scope: Option<&FetchScope<'_>>) -> PythonString {
    let mut text = PythonString::default();
    for (position, line) in fetch_stanza_lines_exact(
        &section.selector,
        &section.when_doing_clause,
        &section.indent,
        scope,
    )
    .iter()
    .enumerate()
    {
        if position > 0 {
            text.push_scalars("\n");
        }
        text.append(line);
    }
    text
}

/// Literal partition at LF, shared by callers with distinct no-LF policies.
fn partition_lf(text: &PythonString) -> Option<(PythonString, PythonString)> {
    let units = text.codepoints();
    let newline = units.iter().position(|&code| code == u32::from(b'\n'))?;
    Some((
        PythonString::from_units(units[..newline].to_vec()).expect("a code-point slice"),
        PythonString::from_units(units[newline + 1..].to_vec()).expect("a code-point slice"),
    ))
}

/// Source _split_leading_header deliberately treats no LF as a bare BODY.
fn split_leading_header(text: &PythonString) -> (PythonString, PythonString) {
    partition_lf(text).unwrap_or_else(|| (PythonString::default(), text.clone()))
}

/// Source `_split_profile_blocks`: split on ASCII LF LF, dropping empty chunks.
fn split_profile_blocks(text: &PythonString) -> Vec<(PythonString, PythonString)> {
    if text.is_empty() {
        return Vec::new();
    }
    split_on(text, &[u32::from(b'\n'), u32::from(b'\n')])
        .into_iter()
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| split_leading_header(&chunk))
        .collect()
}

/// Python `text.split(separator)` for a nonempty literal separator.
fn split_on(text: &PythonString, separator: &[u32]) -> Vec<PythonString> {
    let units = text.codepoints();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index + separator.len() <= units.len() {
        if units[index..index + separator.len()] == separator[..] {
            parts.push(PythonString::from_units(units[start..index].to_vec()).expect("slice"));
            index += separator.len();
            start = index;
        } else {
            index += 1;
        }
    }
    parts.push(PythonString::from_units(units[start..].to_vec()).expect("slice"));
    parts
}

/// Source `_heading_text`: `###`, Python whitespace, then `str.strip()`.
fn heading_text(line: &PythonString) -> Option<PythonString> {
    let units = line.codepoints();
    if !units.starts_with(&[u32::from(b'#'), u32::from(b'#'), u32::from(b'#')]) {
        return None;
    }
    let fourth = *units.get(3)?;
    if !is_whitespace(fourth) {
        return None;
    }
    let heading = exact_strip(&PythonString::from_units(units[3..].to_vec()).expect("slice"));
    if heading.is_empty() {
        None
    } else {
        Some(heading)
    }
}

/// Source `_split_critical_sections`: split on LF LF before a literal `### `.
fn split_critical_sections(text: &PythonString) -> Vec<(PythonString, PythonString, PythonString)> {
    if text.is_empty() {
        return Vec::new();
    }
    let units = text.codepoints();
    let separator = [
        u32::from(b'\n'),
        u32::from(b'\n'),
        u32::from(b'#'),
        u32::from(b'#'),
        u32::from(b'#'),
        u32::from(b' '),
    ];
    let mut chunks: Vec<&[u32]> = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index + separator.len() <= units.len() {
        if units[index..index + separator.len()] == separator[..] {
            chunks.push(&units[start..index]);
            index += 2;
            start = index;
        } else {
            index += 1;
        }
    }
    chunks.push(&units[start..]);
    let mut result = Vec::new();
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        let chunk = PythonString::from_units(chunk.to_vec()).expect("a code-point slice");
        // Source uses str.partition here: no LF leaves the WHOLE first line
        // and an empty remainder, unlike profile's bare-body policy above.
        let (first_line, remainder) =
            partition_lf(&chunk).unwrap_or_else(|| (chunk.clone(), PythonString::default()));
        let Some(heading) = heading_text(&first_line) else {
            // The block's own outer header precedes the first `### ` heading.
            continue;
        };
        result.push((first_line, remainder, heading));
    }
    result
}

/// One swap candidate per action-critical `### <heading>` sub-block.
fn collect_section_block_candidates(
    section_block: &PythonString,
    action: &PythonString,
) -> Vec<RenderedSection> {
    if section_block.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let headed = split_critical_sections(section_block);
    if !headed.is_empty() {
        for (header, body, heading) in headed {
            if body.is_empty() {
                continue;
            }
            let mut section_id = PythonString::from_scalars("action-critical-sections:");
            section_id.append(&heading);
            candidates.push(RenderedSection {
                section_id,
                header,
                body,
                selector: critical_section_selector_exact(&heading),
                when_doing_clause: critical_section_when_clause_exact(&heading),
                substitutable: true,
                indent: PythonString::from_scalars("  "),
                replacement_body: None,
            });
        }
        return candidates;
    }
    let (header, body) = split_leading_header(section_block);
    if !body.is_empty() {
        let mut selector = PythonString::from_scalars("section:critical-");
        selector.append(action);
        candidates.push(RenderedSection {
            section_id: PythonString::from_scalars("action-critical-sections"),
            header,
            body,
            selector,
            when_doing_clause: PythonString::from_scalars(
                "need to consult the action-critical charter sections",
            ),
            substitutable: true,
            indent: PythonString::from_scalars("  "),
            replacement_body: None,
        });
    }
    candidates
}

/// One swap candidate per profile kind-block.
fn collect_profile_block_candidates(profile_block: &PythonString) -> Vec<RenderedSection> {
    if profile_block.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for (index, (header, body)) in split_profile_blocks(profile_block).into_iter().enumerate() {
        if body.is_empty() {
            continue;
        }
        candidates.push(RenderedSection {
            section_id: PythonString::from_scalars(&format!("profile-cited-sections-{index}")),
            header,
            body,
            selector: PythonString::from_scalars("section:profile-citations"),
            when_doing_clause: PythonString::from_scalars(
                "need to consult the profile-cited directives and tactics",
            ),
            substitutable: true,
            indent: PythonString::from_scalars("  "),
            replacement_body: None,
        });
    }
    candidates
}

/// The eight selected-group headers and their `(kind, when-clause)` facts.
///
/// This table is the ONE authored roster; the renderer never re-derives a group.
const SELECTION_GROUPS: [(&str, &str, &str); 8] = [
    (
        "Selected paradigms:",
        "paradigm",
        "are about to choose a reasoning frame",
    ),
    (
        "Selected directives:",
        "directive",
        "are about to apply a code change",
    ),
    (
        "Selected tactics:",
        "tactic",
        "are about to apply a code change",
    ),
    (
        "Selected styleguides:",
        "styleguide",
        "are about to write styled output",
    ),
    (
        "Selected toolguides:",
        "toolguide",
        "are about to invoke a project tool",
    ),
    (
        "Selected procedures:",
        "procedure",
        "are about to follow a workflow",
    ),
    (
        "Selected agent profiles:",
        "agent_profile",
        "are about to apply a code change",
    ),
    (
        "Selected mission step contracts:",
        "mission_step_contract",
        "are about to step a mission action",
    ),
];

/// Compactable per-kind candidates for global doctrine selections.
///
/// Under pressure every selected ID line survives and only the bodies collapse
/// to one parameterized, executable-shape fetch instruction per kind.
fn collect_selection_block_candidates(
    selection_block: &PythonString,
    scope: Option<&FetchScope<'_>>,
) -> Vec<RenderedSection> {
    let mut candidates = Vec::new();
    for (header, body) in split_profile_blocks(selection_block) {
        let Some((_, kind, clause)) = SELECTION_GROUPS
            .iter()
            .find(|(group_header, _, _)| header == PythonString::from_scalars(group_header))
        else {
            continue;
        };
        if body.is_empty() {
            continue;
        }
        let id_lines: Vec<&[u32]> =
            sk_kernel::python_text::splitlines_codepoints(body.codepoints())
                .into_iter()
                .filter(|line| {
                    line.starts_with(&[
                        u32::from(b' '),
                        u32::from(b' '),
                        u32::from(b'-'),
                        u32::from(b' '),
                    ])
                })
                .collect();
        if id_lines.is_empty() {
            continue;
        }
        let mut compact = PythonString::default();
        for line in &id_lines {
            compact.append(&PythonString::from_units(line.to_vec()).expect("slice"));
            compact.push_scalars("\n");
        }
        compact.push_scalars("  Run: ");
        compact.append(&fetch_command(
            &PythonString::from_scalars(&format!("{kind}:SELECTED_ID")),
            scope,
        ));
        compact.push_scalars(&format!(
            "\n  When you {clause}, replace SELECTED_ID with an ID above and apply the returned rule."
        ));
        let mut section_id = PythonString::from_scalars("global-selection:");
        section_id.push_scalars(kind);
        candidates.push(RenderedSection {
            section_id,
            header,
            body,
            selector: PythonString::default(),
            when_doing_clause: PythonString::default(),
            substitutable: true,
            indent: PythonString::default(),
            replacement_body: Some(compact),
        });
    }
    candidates
}

/// Python `text.replace(old, new, 1)` over exact code points.
fn replace_first(
    text: &PythonString,
    old: &PythonString,
    new: &PythonString,
) -> Option<PythonString> {
    if old.is_empty() {
        return None;
    }
    let haystack = text.codepoints();
    let needle = old.codepoints();
    if needle.len() > haystack.len() {
        return None;
    }
    let position = (0..=haystack.len() - needle.len())
        .find(|&start| haystack[start..start + needle.len()] == needle[..])?;
    let mut replaced = PythonString::default();
    replaced.append(&PythonString::from_units(haystack[..position].to_vec()).expect("slice"));
    replaced.append(new);
    replaced.append(
        &PythonString::from_units(haystack[position + needle.len()..].to_vec()).expect("slice"),
    );
    Some(replaced)
}

/// Join exact strings with a literal separator (Source `separator.join`).
fn join(values: &[&PythonString], separator: &str) -> PythonString {
    let mut out = PythonString::default();
    for (position, value) in values.iter().enumerate() {
        if position > 0 {
            out.push_scalars(separator);
        }
        out.append(value);
    }
    out
}

/// `join` over owned exact strings.
fn join_refs(values: &[PythonString], separator: &str) -> PythonString {
    let refs: Vec<&PythonString> = values.iter().collect();
    join(&refs, separator)
}

/// Python `len(text)`.
fn chars(text: &PythonString) -> usize {
    text.codepoints().len()
}

/// Python `len(text)` as the budget adapter's integer.
fn chars_as_i128(text: &PythonString) -> i128 {
    i128::try_from(chars(text)).expect("a Python string length fits i128")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_render::{
        DEFAULT_WHEN_CLAUSE, fetch_stanza_lines, fetch_stanza_lines_exact,
    };

    fn text(value: &str) -> PythonString {
        PythonString::from_scalars(value)
    }

    fn section(id: &str, header: &str, body: &str) -> RenderedSection {
        RenderedSection::new(id, header, body)
    }

    #[test]
    fn default_budget_and_profile_limit_are_the_source_pins() {
        assert_eq!(BUDGET_DEFAULT, 40_000);
        assert_eq!(PROFILE_INLINE_BODY_LIMIT_CHARS, 2_400);
        assert_eq!(budget_estimate(&[text("abc"), text("de"), text("")]), 8);
        assert_eq!(
            warning_line(2, BUDGET_DEFAULT).to_scalar_string().unwrap(),
            "# Governance payload: 2 sections substituted with fetch commands (budget=40000)."
        );
        assert_eq!(
            budget_without_warning_line(2, BUDGET_DEFAULT).expect("representable remainder"),
            BUDGET_DEFAULT - 2 - i128::try_from(chars(&warning_line(2, BUDGET_DEFAULT))).unwrap()
        );
        assert_eq!(budget_without_warning_line(0, -7), Ok(-7));
    }

    #[test]
    fn apply_joins_only_nonempty_pieces_and_nonpositive_is_a_noop() {
        let sections = [
            section("s1", "Header One", "xxxx"),
            section("s2", "", "yyyy"),
            RenderedSection {
                section_id: text("s3"),
                header: text("H3"),
                body: PythonString::default(),
                substitutable: true,
                selector: text("tactic:T"),
                ..RenderedSection::default()
            },
        ];
        let outcome = apply_token_budget(&sections, 1_000_000, None);
        assert_eq!(
            outcome.text.to_scalar_string().unwrap(),
            "Header One\nxxxx\n\nyyyy\n\nH3"
        );
        assert!(outcome.notes.is_empty());
        let huge = [section("a", "H", &"z".repeat(50))];
        let noop = apply_token_budget(&huge, 0, None);
        assert_eq!(
            noop.text.to_scalar_string().unwrap(),
            format!("H\n{}", "z".repeat(50))
        );
        assert!(noop.notes.is_empty());
        assert!(
            !noop
                .text
                .to_scalar_string()
                .unwrap()
                .contains("Governance payload")
        );
    }

    #[test]
    fn apply_breaks_ties_by_section_id_and_swaps_longest_first() {
        let sections = [
            section("b", "B:", &"1".repeat(300)).with_selector("tactic:B"),
            section("a", "A:", &"2".repeat(300)).with_selector("tactic:A"),
        ];
        let base = 2 + 1 + 300 + 2 + 2 + 1 + 300;
        let outcome = apply_token_budget(&sections, i128::try_from(base).unwrap() - 10, None);
        let notes: Vec<String> = outcome
            .notes
            .iter()
            .map(|note| note.to_scalar_string().unwrap())
            .collect();
        assert_eq!(notes, vec!["a (body 300 chars -> fetch stanza)".to_owned()]);
        let notes_order = apply_token_budget(
            &[
                section("c", "C:", &"3".repeat(10)).with_selector("tactic:C"),
                section("a", "A:", &"1".repeat(10)).with_selector("tactic:A"),
                section("b", "B:", &"2".repeat(10)).with_selector("tactic:B"),
            ],
            1,
            None,
        )
        .notes;
        let ids: Vec<String> = notes_order
            .iter()
            .map(|note| note.to_scalar_string().unwrap())
            .collect();
        assert_eq!(ids[0], "a (body 10 chars -> fetch stanza)");
        assert_eq!(ids[1], "b (body 10 chars -> fetch stanza)");
        assert_eq!(ids[2], "c (body 10 chars -> fetch stanza)");
    }

    #[test]
    fn apply_distinguishes_empty_replacement_from_missing_and_survives_longer_ones() {
        let empty_replacement = [section("g", "Grp:", "long body text").with_replacement_body("")];
        let outcome = apply_token_budget(&empty_replacement, 5, None);
        assert_eq!(
            outcome.text.to_scalar_string().unwrap(),
            "Grp:\n\n# Governance payload: 1 sections substituted with fetch commands (budget=5)."
        );
        let no_rail = [section("n", "N:", &"q".repeat(100))];
        let outcome = apply_token_budget(&no_rail, 5, None);
        assert_eq!(
            outcome.text.to_scalar_string().unwrap(),
            format!("N:\n{}", "q".repeat(100))
        );
        assert!(outcome.notes.is_empty());
        let longer = [section("L", "L:", "tiny").with_selector("directive:LONG-SELECTOR-NAME")];
        let outcome = apply_token_budget(&longer, 6, None);
        assert!(outcome.text.to_scalar_string().unwrap().contains(
            "Run: spec-kitty charter context --include directive:LONG-SELECTOR-NAME\nWhen you \
             are about to apply a code change, run this command and apply the returned rule."
        ));
        assert_eq!(
            outcome.notes[0].to_scalar_string().unwrap(),
            "L (body 4 chars -> fetch stanza)"
        );
        let non_substitutable = [
            section("crit", "Crit:", &"C".repeat(60)).non_substitutable(),
            section("big", "Big:", &"D".repeat(40)).with_selector("tactic:T"),
        ];
        let outcome = apply_token_budget(&non_substitutable, 1, None);
        let rendered = outcome.text.to_scalar_string().unwrap();
        assert!(rendered.contains(&"C".repeat(60)));
        assert!(rendered.contains("--include tactic:T"));
        assert_eq!(
            outcome.notes[0].to_scalar_string().unwrap(),
            "big (body 40 chars -> fetch stanza)"
        );
    }

    #[test]
    fn apply_exhausts_candidates_and_still_reports_over_budget() {
        let sections = [
            section("x1", "X1:", &"f".repeat(60)).with_selector("tactic:X1"),
            section("x2", "X2:", &"g".repeat(61)).with_selector("tactic:X2"),
        ];
        let outcome = apply_token_budget(&sections, 1, None);
        let notes: Vec<String> = outcome
            .notes
            .iter()
            .map(|note| note.to_scalar_string().unwrap())
            .collect();
        assert_eq!(
            notes,
            vec![
                "x2 (body 61 chars -> fetch stanza)".to_owned(),
                "x1 (body 60 chars -> fetch stanza)".to_owned()
            ]
        );
        assert!(
            outcome
                .text
                .to_scalar_string()
                .unwrap()
                .contains("Governance payload: 2")
        );
    }

    #[test]
    fn enforce_identity_no_candidate_and_nonpositive_policy() {
        let critical = text(
            "Action-Critical Charter Sections (implement):\n### Terminology Canon\ncanon body \
             one\n\n### Code Review Checklist\nreview body two\n### Regression Vigilance\nregression \
             body three",
        );
        let full = text("Bootstrap header\n\nTail");
        let joined = text(&format!(
            "{}\n\n{}",
            full.to_scalar_string().unwrap(),
            critical.to_scalar_string().unwrap()
        ));
        assert_eq!(
            enforce_token_budget(
                &joined,
                &text("implement"),
                &PythonString::default(),
                &critical,
                &PythonString::default(),
                1_000_000,
                None,
            ),
            joined
        );
        assert_eq!(
            enforce_token_budget(
                &text("plain text with no candidates at all"),
                &text("implement"),
                &PythonString::default(),
                &PythonString::default(),
                &PythonString::default(),
                5,
                None,
            )
            .to_scalar_string()
            .unwrap(),
            "plain text with no candidates at all"
        );
        let rendered = enforce_token_budget(
            &joined,
            &text("implement"),
            &PythonString::default(),
            &critical,
            &PythonString::default(),
            0,
            None,
        )
        .to_scalar_string()
        .unwrap();
        assert!(rendered.contains("--include section:code-review-checklist"));
        assert!(!rendered.contains("Governance payload: 1 sections substituted (budget=0)."));
        assert!(rendered.contains(
            "Governance payload: 1 sections substituted with fetch commands (budget=0)."
        ));
        assert!(rendered.contains("canon body one"));
    }

    #[test]
    fn enforce_replaces_only_the_first_occurrence_anywhere() {
        let body = text("SHARED-BODY");
        let mut profile = text("Profile-Cited Directives (p):\n");
        profile.append(&body);
        let rendered = enforce_token_budget(
            &text("Head\n\nSHARED-BODY\n\nmid\n\nSHARED-BODY\n\nTail"),
            &text("implement"),
            &profile,
            &PythonString::default(),
            &PythonString::default(),
            1,
            None,
        )
        .to_scalar_string()
        .unwrap();
        assert!(rendered.contains("--include section:profile-citations"));
        assert!(rendered.ends_with("mid\n\nSHARED-BODY\n\nTail\n\n# Governance payload: 1 sections substituted with fetch commands (budget=1)."));
    }

    #[test]
    fn splits_match_the_source_separators() {
        let critical = text(
            "Action-Critical Charter Sections (implement):\n\n### Terminology Canon\nbody with\n\nblank \
             line\n\n### Regression Vigilance\ntail",
        );
        let chunks = split_critical_sections(&critical);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].2, text("Terminology Canon"));
        assert_eq!(chunks[0].1, text("body with\n\nblank line"));
        let profile = text("A:\n- d1\n\nB:\n- t1");
        let chunks = split_profile_blocks(&profile);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].0, text("B:"));
        assert_eq!(chunks[1].1, text("- t1"));
        // Several blank lines still leave one empty chunk that Source drops;
        // the surviving chunk then has no leading header line.
        let triple = split_profile_blocks(&text("A:\n- d1\n\n\nB:\n- t1"));
        assert_eq!(triple.len(), 2);
        assert_eq!(triple[1].0, PythonString::default());
        assert_eq!(triple[1].1, text("B:\n- t1"));
        // CRLF is not an LF LF separator in either splitter.
        let crlf = text(
            "### Terminology Canon\r\nbody one\r\n\r\n### Regression Vigilance\r\nbody two\r\n",
        );
        assert_eq!(split_critical_sections(&crlf).len(), 1);
        let crlf_profile = text("A:\r\n- d1\r\n\r\nB:\r\n- t1");
        assert_eq!(split_profile_blocks(&crlf_profile).len(), 1);
        // NBSP right after `###` is Python whitespace, so it starts a heading.
        assert_eq!(
            heading_text(&text("###\u{a0}Terminology Canon")),
            Some(text("Terminology Canon"))
        );
        assert_eq!(heading_text(&text("###\t")), None);
        assert_eq!(heading_text(&text("###x")), None);
    }

    #[test]
    fn selection_groups_retain_every_id_line_and_one_fetch_instruction() {
        let selection = text(
            "Selected directives:\n  - DIRECTIVE_001\n  - DIRECTIVE_002\n\nSelected tactics:\n  - \
             tactic-one",
        );
        let rendered = enforce_token_budget(
            &text(&format!(
                "Head\n\n{}\n\nTail",
                selection.to_scalar_string().unwrap()
            )),
            &text("implement"),
            &PythonString::default(),
            &PythonString::default(),
            &selection,
            5,
            None,
        )
        .to_scalar_string()
        .unwrap();
        assert!(rendered.contains("  - DIRECTIVE_001\n  - DIRECTIVE_002\n  Run: spec-kitty charter context --include directive:SELECTED_ID\n  When you are about to apply a code change, replace SELECTED_ID with an ID above and apply the returned rule."));
        assert!(rendered.contains(
            "  - tactic-one\n  Run: spec-kitty charter context --include tactic:SELECTED_ID"
        ));
    }

    #[test]
    fn character_counts_are_python_codepoints_not_bytes_or_graphemes() {
        let astral = "A\u{1f600}B";
        assert_eq!(chars(&text(astral)), 3);
        let lone = PythonString::from_codepoints(&[0x41, 0xd800, 0x42]).unwrap();
        assert_eq!(chars(&lone), 3);
        let outcome = apply_token_budget(
            &[RenderedSection {
                section_id: text("a"),
                header: PythonString::default(),
                body: lone.clone(),
                substitutable: true,
                selector: PythonString::default(),
                when_doing_clause: PythonString::default(),
                indent: PythonString::default(),
                replacement_body: Some(PythonString::default()),
            }],
            2,
            None,
        );
        assert_eq!(
            outcome.notes[0].to_scalar_string().unwrap(),
            "a (body 3 chars -> fetch stanza)"
        );
    }

    #[test]
    fn fetch_composition_quotes_orders_and_scopes_exactly() {
        let lines = fetch_stanza_lines("section:terminology-canon", "", "  ");
        assert_eq!(
            lines,
            vec![
                "  Run: spec-kitty charter context --include section:terminology-canon".to_owned(),
                format!(
                    "  When you {DEFAULT_WHEN_CLAUSE}, run this command and apply the returned rule."
                ),
            ]
        );
        let spaced = text("section:My Section");
        assert_eq!(
            fetch_command(&spaced, None).to_scalar_string().unwrap(),
            "spec-kitty charter context --include 'section:My Section'"
        );
        let owned = text("/tmp/x y/owned");
        let handle = text("mission-1");
        let scope = FetchScope {
            owned_root: &owned,
            mission_handle: &handle,
        };
        assert_eq!(
            fetch_command(&text("directive:DIRECTIVE_001"), Some(&scope))
                .to_scalar_string()
                .unwrap(),
            "spec-kitty charter context --include directive:DIRECTIVE_001 --owned-checkout '/tmp/x \
             y/owned' --mission mission-1"
        );
        // The closed lead-in set decides the exact wording.
        let lead_in = fetch_stanza_lines_exact(
            &text("directive:X"),
            &text("Are About To ship"),
            &PythonString::default(),
            None,
        );
        assert!(
            lead_in[1]
                .to_scalar_string()
                .unwrap()
                .contains("When you Are About To ship, run")
        );
        let not_a_lead_in = fetch_stanza_lines_exact(
            &text("directive:X"),
            &text("introduces a term"),
            &PythonString::default(),
            None,
        );
        assert!(
            not_a_lead_in[1]
                .to_scalar_string()
                .unwrap()
                .contains("When you need to introduces a term, run")
        );
    }
}

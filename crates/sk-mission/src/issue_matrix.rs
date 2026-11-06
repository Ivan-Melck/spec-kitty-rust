//! Gating GitHub-issue reference discovery and the structured
//! `issue-matrix.json` validators.
//!
//! Source requires an issue matrix only when the mission **declares a gating
//! issue reference**. The classifier (`tasks/issue_reference_discovery.py`) is
//! the single authority for that predicate, and the review gate consumes it
//! instead of treating a missing matrix as "not applicable" regardless of the
//! references the mission actually cites. This module ports the reference
//! scanner, the occurrence classifier and the structured-document validator so
//! the review gate can judge a missing/structured matrix for real.
//!
//! # Recognised references
//!
//! * `#NNNN` — 2–6 digits with Source's leading and trailing boundary rules.
//!   `#1234:45`, `#1234:L5`, `#1234?x`, `#1234_x`, `#12-fix`, `#1` and
//!   `owner/repo#1234` are not references; `#12/#13`, `**#12**` and
//!   `Issue #12: title` are.
//! * `https://github.com/<owner>/<repo>/issues/<n>` — only the canonical
//!   same-repo slug; a cross-repo/prior-art URL is never a reference.
//!
//! # Classification (FR-011 default-gating fail-safe)
//!
//! A line carrying a PR/commit signal is `pr_or_commit_ref`; a line carrying an
//! explicit context marker is `context_only`; every other line is
//! `implementation_target`. Aggregation: any implementation-target occurrence
//! wins; else all-PR/commit is `pr_or_commit_ref`; else `context_only`. Only
//! `implementation_target` is gating.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

/// The canonical same-repo slug Source treats as a local issue URL.
pub const OWN_REPO_SLUG: &str = "spec-kitty/spec-kitty";

/// The allowed structured-issue-matrix verdicts.
pub const ISSUE_MATRIX_VERDICTS: &[&str] = &[
    "fixed",
    "verified-already-fixed",
    "deferred-with-followup",
    "in-mission",
    "not-applicable",
];

/// The scaffold verdict placeholder (never a valid verdict).
pub const ISSUE_MATRIX_PLACEHOLDER_VERDICT: &str = "unknown";

/// One occurrence of an issue number on one line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Occurrence {
    /// Basename of the file the line came from.
    pub source_file: String,
    /// The stripped line.
    pub line_text: String,
}

/// How one reference must be treated by the gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatingClass {
    /// An implementation target: requires an issue-matrix row.
    ImplementationTarget,
    /// Background context: never requires a row.
    ContextOnly,
    /// A PR/commit reference: never requires a row.
    PrOrCommitRef,
}

impl GatingClass {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImplementationTarget => "implementation_target",
            Self::ContextOnly => "context_only",
            Self::PrOrCommitRef => "pr_or_commit_ref",
        }
    }

    /// Whether this classification requires an issue-matrix row.
    pub fn is_gating(self) -> bool {
        self == Self::ImplementationTarget
    }
}

/// One discovered issue reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueReference {
    /// The issue number, digits only.
    pub number: u32,
    /// Every occurrence, in scan order.
    pub occurrences: Vec<Occurrence>,
    /// The aggregated classification.
    pub classification: GatingClass,
}

impl IssueReference {
    /// The canonical `#NNNN` spelling (the number as scanned, no padding).
    pub fn canonical(&self) -> String {
        format!("#{}", self.number)
    }
}

/// Classify one occurrence line, PR/commit signal first (Source order).
pub fn classify_occurrence(occurrence: &Occurrence) -> GatingClass {
    let lowered = occurrence.line_text.to_ascii_lowercase();
    if has_pr_or_commit_signal(&occurrence.line_text, &lowered) {
        return GatingClass::PrOrCommitRef;
    }
    if has_context_marker(&occurrence.line_text, &lowered) {
        return GatingClass::ContextOnly;
    }
    GatingClass::ImplementationTarget
}

fn has_pr_or_commit_signal(_line: &str, lowered: &str) -> bool {
    if lowered.contains("/pull/") || lowered.contains("pull") {
        return true;
    }
    // A `PR` / `PR#` token at a word boundary.
    let bytes = lowered.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if &bytes[index..index + 2] != b"pr" {
            index += 1;
            continue;
        }
        let boundary_before = index == 0
            || !lowered[..index]
                .chars()
                .next_back()
                .is_some_and(is_word_char);
        if !boundary_before {
            index += 2;
            continue;
        }
        let mut tail = &lowered[index + 2..];
        let before = tail.len();
        tail = tail.trim_start();
        if tail.starts_with('#') || tail.len() != before {
            return true;
        }
        index += 2;
    }
    false
}

fn has_context_marker(line: &str, lowered: &str) -> bool {
    if lowered.contains("follow-up:") || lowered.contains("baseline-red") {
        return true;
    }
    if lowered.trim_start().starts_with("see #") {
        return true;
    }
    // `parent`/`epic` used as a citation label: a `#` within the next 12
    // non-`#` characters after the keyword.
    for keyword in ["parent", "epic"] {
        for (index, _) in lowered.match_indices(keyword) {
            let after = &lowered[index + keyword.len()..];
            for (offset, character) in after.chars().enumerate() {
                if offset > 12 {
                    break;
                }
                if character == '#' {
                    return true;
                }
            }
        }
    }
    let _ = line;
    false
}

fn is_word_char(value: char) -> bool {
    value.is_alphanumeric() || value == '_'
}

/// Aggregate classifications over all occurrences of one number.
pub fn classify_occurrences(occurrences: &[Occurrence]) -> GatingClass {
    if occurrences.is_empty() {
        return GatingClass::ImplementationTarget;
    }
    let per_occurrence: Vec<GatingClass> = occurrences.iter().map(classify_occurrence).collect();
    if per_occurrence.contains(&GatingClass::ImplementationTarget) {
        return GatingClass::ImplementationTarget;
    }
    if per_occurrence
        .iter()
        .all(|class| *class == GatingClass::PrOrCommitRef)
    {
        return GatingClass::PrOrCommitRef;
    }
    GatingClass::ContextOnly
}

/// Scan one text for `#NNNN` and same-repo issue-URL references.
pub fn detect_issue_references_text(text: &str, source_file: &str) -> Vec<IssueReference> {
    let mut first_seen: BTreeMap<u32, Occurrence> = BTreeMap::new();
    let mut occurrences: BTreeMap<u32, Vec<Occurrence>> = BTreeMap::new();
    let mut seen_pairs: BTreeSet<(u32, String)> = BTreeSet::new();
    for line in text.lines() {
        let stripped = line.trim().to_owned();
        for number in issue_numbers_on_line(line) {
            first_seen.entry(number).or_insert_with(|| Occurrence {
                source_file: source_file.to_owned(),
                line_text: stripped.clone(),
            });
            if seen_pairs.insert((number, stripped.clone())) {
                occurrences.entry(number).or_default().push(Occurrence {
                    source_file: source_file.to_owned(),
                    line_text: stripped.clone(),
                });
            }
        }
    }
    let mut references: Vec<IssueReference> = first_seen
        .into_keys()
        .map(|number| {
            let rows = occurrences.remove(&number).unwrap_or_default();
            IssueReference {
                number,
                classification: classify_occurrences(&rows),
                occurrences: rows,
            }
        })
        .collect();
    references.sort_by_key(|reference| reference.number);
    references
}

fn issue_numbers_on_line(line: &str) -> Vec<u32> {
    let mut found = BTreeSet::new();
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        if !leading_boundary(line, index) {
            continue;
        }
        let mut end = index + 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        let digits = &line[index + 1..end];
        if !(2..=6).contains(&digits.len()) {
            continue;
        }
        if !trailing_boundary(line, end) {
            continue;
        }
        if let Ok(number) = digits.parse::<u32>() {
            found.insert(number);
        }
    }
    for number in issue_url_numbers(line) {
        found.insert(number);
    }
    found.into_iter().collect()
}

fn leading_boundary(line: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let before = line[..index].chars().next_back().unwrap_or(' ');
    if before.is_whitespace() || matches!(before, '(' | '[' | '`' | '*') {
        return true;
    }
    // `#12/#13`: the `/` after a digit is a list separator.
    before == '/'
        && line[..index - 1]
            .chars()
            .next_back()
            .is_some_and(|value| value.is_ascii_digit())
}

fn trailing_boundary(line: &str, end: usize) -> bool {
    let Some(after) = line[end..].chars().next() else {
        return true;
    };
    if after.is_whitespace()
        || matches!(
            after,
            ')' | ']' | ',' | ';' | '.' | '`' | '*' | '"' | '\'' | '|' | '—' | '–'
        )
    {
        return true;
    }
    if matches!(after, ':' | '!' | '?' | '_') {
        // Not a terminator when a word character follows (`#1234:45`).
        let next = line[end..].chars().nth(1);
        return !next.is_some_and(is_word_char);
    }
    if after == '/' {
        return line[end..].chars().nth(1) == Some('#');
    }
    false
}

fn issue_url_numbers(line: &str) -> Vec<u32> {
    let mut numbers = Vec::new();
    for prefix in ["https://github.com/", "http://github.com/"] {
        let mut cursor = 0usize;
        while let Some(relative) = line[cursor..].find(prefix) {
            let start = cursor + relative + prefix.len();
            let rest = &line[start..];
            if let Some((owner, after_owner)) = rest.split_once('/')
                && let Some((repo, after_repo)) = after_owner.split_once('/')
                && let Some(after_issues) = after_repo.strip_prefix("issues/")
                && format!("{owner}/{repo}") == OWN_REPO_SLUG
            {
                let digits: String = after_issues
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if (2..=6).contains(&digits.len())
                    && !after_issues[digits.len()..]
                        .chars()
                        .next()
                        .is_some_and(is_word_char)
                    && let Ok(number) = digits.parse::<u32>()
                {
                    numbers.push(number);
                }
            }
            cursor = start;
        }
    }
    numbers
}

/// The files Source scans, in its deterministic order.
fn scanned_files(feature_dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for name in ["spec.md", "plan.md", "research.md", "analysis-report.md"] {
        let path = feature_dir.join(name);
        if path.is_file() {
            files.push(path);
        }
    }
    for directory in ["tasks", "contracts"] {
        let dir = feature_dir.join(directory);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|value| value == "md"))
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

/// Discover every GH issue reference across a mission's planning artifacts.
pub fn discover_issue_references(feature_dir: &Path) -> Vec<IssueReference> {
    let mut merged: BTreeMap<u32, IssueReference> = BTreeMap::new();
    let mut order: Vec<u32> = Vec::new();
    for path in scanned_files(feature_dir) {
        let Ok(bytes) = sk_kernel::read_bounded(&path, 4 * 1024 * 1024) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let source_file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for reference in detect_issue_references_text(&text, &source_file) {
            let entry = merged.entry(reference.number).or_insert_with(|| {
                order.push(reference.number);
                IssueReference {
                    number: reference.number,
                    occurrences: Vec::new(),
                    classification: GatingClass::ContextOnly,
                }
            });
            entry.occurrences.extend(reference.occurrences);
        }
    }
    order
        .into_iter()
        .filter_map(|number| merged.remove(&number))
        .map(|mut reference| {
            reference.classification = classify_occurrences(&reference.occurrences);
            reference
        })
        .collect()
}

/// The canonical set of gating `#NNNN` references for a mission.
pub fn gating_issue_numbers(feature_dir: &Path) -> BTreeSet<String> {
    discover_issue_references(feature_dir)
        .into_iter()
        .filter(|reference| reference.classification.is_gating())
        .map(|reference| reference.canonical())
        .collect()
}

/// Validate a structured `issue-matrix.json` text.
///
/// Returns the canonical row keys on success. Diagnostics are `(code, message)`
/// pairs; a blank/malformed document is a diagnostic, never an empty pass.
pub fn validate_issue_matrix_json_text(
    text: &str,
) -> Result<BTreeSet<String>, Vec<(String, String)>> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        vec![(
            "ISSUE_MATRIX_INVALID".to_owned(),
            format!("issue-matrix.json is not valid JSON: {error}"),
        )]
    })?;
    let Value::Object(document) = value else {
        return Err(vec![(
            "ISSUE_MATRIX_SCHEMA_DRIFT".to_owned(),
            "issue-matrix.json is not a JSON object".to_owned(),
        )]);
    };
    let Some(Value::Object(rows)) = document.get("rows") else {
        return Err(vec![(
            "ISSUE_MATRIX_SCHEMA_DRIFT".to_owned(),
            "issue-matrix.json has no `rows` object".to_owned(),
        )]);
    };
    let mut keys = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for (raw_key, row) in rows {
        let key = canonicalize_issue_ref(raw_key);
        keys.insert(key.clone());
        let Value::Object(row) = row else {
            diagnostics.push((
                "ISSUE_MATRIX_SCHEMA_DRIFT".to_owned(),
                format!("row {key} is not a JSON object"),
            ));
            continue;
        };
        let verdict = row.get("verdict").and_then(Value::as_str).unwrap_or("");
        if verdict == ISSUE_MATRIX_PLACEHOLDER_VERDICT {
            diagnostics.push((
                "ISSUE_MATRIX_PLACEHOLDER".to_owned(),
                format!("row {key} still carries the scaffold verdict {verdict:?}"),
            ));
            continue;
        }
        if !ISSUE_MATRIX_VERDICTS.contains(&verdict) {
            diagnostics.push((
                "ISSUE_MATRIX_VERDICT_INVALID".to_owned(),
                format!("row {key} has verdict {verdict:?}, not one of {ISSUE_MATRIX_VERDICTS:?}"),
            ));
            continue;
        }
        let evidence = row
            .get("evidence_ref")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if evidence.is_empty() {
            diagnostics.push((
                "ISSUE_MATRIX_EVIDENCE_MISSING".to_owned(),
                format!("row {key} has no evidence_ref"),
            ));
        }
        if verdict == "deferred-with-followup" {
            let handle = row
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if handle.is_empty() && evidence.is_empty() {
                diagnostics.push((
                    "ISSUE_MATRIX_FOLLOWUP_MISSING".to_owned(),
                    format!("row {key} is deferred but names no follow-up handle"),
                ));
            }
        }
    }
    if diagnostics.is_empty() {
        Ok(keys)
    } else {
        Err(diagnostics)
    }
}

fn canonicalize_issue_ref(raw: &str) -> String {
    let trimmed = raw.trim();
    let digits: String = trimmed
        .chars()
        .rev()
        .take_while(|value| value.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if digits.is_empty() {
        trimmed.to_owned()
    } else {
        format!("#{digits}")
    }
}

/// Validate a structured `issue-matrix.json` file.
pub fn validate_issue_matrix_json(path: &Path) -> Result<BTreeSet<String>, Vec<(String, String)>> {
    let bytes = sk_kernel::read_bounded(path, 4 * 1024 * 1024).map_err(|error| {
        vec![(
            "ISSUE_MATRIX_INVALID".to_owned(),
            format!("could not read {}: {error}", path.display()),
        )]
    })?;
    let text = String::from_utf8(bytes).map_err(|error| {
        vec![(
            "ISSUE_MATRIX_INVALID".to_owned(),
            format!("{} is not valid UTF-8: {error}", path.display()),
        )]
    })?;
    validate_issue_matrix_json_text(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_rules_match_the_source_examples() {
        let numbers = issue_numbers_on_line("Issue #1234: title");
        assert_eq!(numbers, vec![1234]);
        assert_eq!(issue_numbers_on_line("**#1234**"), vec![1234]);
        assert_eq!(issue_numbers_on_line("_Fixes #1234_"), vec![1234]);
        assert_eq!(issue_numbers_on_line("#12/#13"), vec![12, 13]);
        assert!(issue_numbers_on_line("#1234:45").is_empty());
        assert!(issue_numbers_on_line("#1234:L5").is_empty());
        assert!(issue_numbers_on_line("#1234?x").is_empty());
        assert!(issue_numbers_on_line("#1234_x").is_empty());
        assert!(issue_numbers_on_line("#12-fix").is_empty());
        assert!(issue_numbers_on_line("#1").is_empty());
        assert!(issue_numbers_on_line("owner/repo#1234").is_empty());
        assert!(issue_numbers_on_line("#1234567").is_empty());
        assert!(issue_numbers_on_line("#section-name").is_empty());
        assert_eq!(
            issue_numbers_on_line("see https://github.com/spec-kitty/spec-kitty/issues/42"),
            vec![42]
        );
        assert!(
            issue_numbers_on_line("https://github.com/other/repo/issues/42").is_empty(),
            "a cross-repo URL is never a reference"
        );
        assert!(
            issue_numbers_on_line("https://github.com/spec-kitty/spec-kitty/pull/42").is_empty()
        );
    }

    #[test]
    fn classification_prefers_pr_then_context_then_target() {
        let line = |text: &str| Occurrence {
            source_file: "spec.md".to_owned(),
            line_text: text.to_owned(),
        };
        assert_eq!(
            classify_occurrence(&line("PR #300 (see #300)")),
            GatingClass::PrOrCommitRef
        );
        assert_eq!(
            classify_occurrence(&line("follow-up: #200")),
            GatingClass::ContextOnly
        );
        assert_eq!(
            classify_occurrence(&line("see #200 for context")),
            GatingClass::ContextOnly
        );
        assert_eq!(
            classify_occurrence(&line("Tracked by the epic #200.")),
            GatingClass::ContextOnly
        );
        assert_eq!(
            classify_occurrence(&line("The parent widget must resolve #4521")),
            GatingClass::ImplementationTarget
        );
        assert_eq!(
            classify_occurrence(&line("Fix #1234 in this mission")),
            GatingClass::ImplementationTarget
        );
    }

    #[test]
    fn aggregation_matches_the_source_precedence() {
        let make = |class: GatingClass| {
            let text = match class {
                GatingClass::ImplementationTarget => "Fix #12",
                GatingClass::ContextOnly => "see #12",
                GatingClass::PrOrCommitRef => "PR #12",
            };
            Occurrence {
                source_file: "spec.md".to_owned(),
                line_text: text.to_owned(),
            }
        };
        assert_eq!(
            classify_occurrences(&[
                make(GatingClass::ContextOnly),
                make(GatingClass::ImplementationTarget)
            ]),
            GatingClass::ImplementationTarget
        );
        assert_eq!(
            classify_occurrences(&[
                make(GatingClass::PrOrCommitRef),
                make(GatingClass::PrOrCommitRef)
            ]),
            GatingClass::PrOrCommitRef
        );
        assert_eq!(
            classify_occurrences(&[
                make(GatingClass::PrOrCommitRef),
                make(GatingClass::ContextOnly)
            ]),
            GatingClass::ContextOnly
        );
    }

    #[test]
    fn structured_rows_are_validated_against_the_closed_verdict_set() {
        let ok =
            r##"{"schema_version":1,"rows":{"#12":{"verdict":"fixed","evidence_ref":"abc123"}}}"##;
        assert!(validate_issue_matrix_json_text(ok).is_ok());
        let placeholder =
            r##"{"schema_version":1,"rows":{"#12":{"verdict":"unknown","evidence_ref":""}}}"##;
        assert!(validate_issue_matrix_json_text(placeholder).is_err());
        let invalid =
            r##"{"schema_version":1,"rows":{"#12":{"verdict":"maybe","evidence_ref":"x"}}}"##;
        assert!(validate_issue_matrix_json_text(invalid).is_err());
        let no_evidence =
            r##"{"schema_version":1,"rows":{"#12":{"verdict":"fixed","evidence_ref":""}}}"##;
        assert!(validate_issue_matrix_json_text(no_evidence).is_err());
    }
}

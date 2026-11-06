//! Pure analysis-report freshness for the software-dev `analyze` gate.
//!
//! Upstream `specify_cli.analysis_report.check_analysis_report_current` decides
//! whether the recorded `analysis-report.md` still describes the current
//! `spec.md` / `plan.md` / `tasks.md` / charter inputs. The gate then re-issues
//! the `analyze` step with a typed `error_code` instead of advancing the run.
//!
//! This module owns the deterministic half: normalizing `tasks.md` status
//! churn, reading the report's recorded input hashes, and comparing them with
//! the caller's freshly computed hashes. Reading files and hashing bytes stays
//! in the CLI adapter, so nothing here touches the filesystem.
//!
//! # Fail-closed, never silently current
//!
//! A report that a caller cannot judge (an unsupported carrier, an unreadable
//! frontmatter block, a missing input map) is [`AnalysisVerdict::stale`] or
//! [`AnalysisVerdict::unavailable`], never [`AnalysisVerdict::Current`]. That
//! mirrors the upstream contract "every code path returns a typed freshness".

use std::collections::BTreeMap;

use serde_json::Value;

/// The recorded analysis report file name.
pub const ANALYSIS_REPORT_FILENAME: &str = "analysis-report.md";
/// The structured carrier schema that is *not* a valid freshness carrier.
pub const FINDINGS_SCHEMA_V1: &str = "analysis-findings/v1";
/// The `artifact_type` a freshness carrier must declare.
pub const ANALYSIS_REPORT_ARTIFACT_TYPE: &str = "analysis-report";

/// Analyse-report guard messages, byte-identical to the upstream constants.
pub const ANALYSIS_MISSING_MESSAGE: &str =
    "Analysis report missing: analysis-report.md has not been recorded";
/// Prefix of one stale-input failure; the input name is appended.
pub const ANALYSIS_STALE_PREFIX: &str = "Analysis report is stale: ";
/// The fail-closed message when no currency check could judge the report.
pub const ANALYSIS_UNAVAILABLE_MESSAGE: &str =
    "Analysis currency could not be evaluated: no analysis-currency check is available";

/// The `error_code` values the `analyze` guard can carry.
pub const ANALYSIS_REPORT_MISSING: &str = "ANALYSIS_REPORT_MISSING";
/// Stale report code.
pub const ANALYSIS_REPORT_STALE: &str = "ANALYSIS_REPORT_STALE";
/// Unavailable check code.
pub const ANALYSIS_CURRENCY_UNAVAILABLE: &str = "ANALYSIS_CURRENCY_UNAVAILABLE";

/// The freshness verdict for one recorded report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisVerdict {
    /// The recorded hashes match the live inputs.
    Current,
    /// No report has been recorded.
    Missing,
    /// The report no longer describes the live inputs (one entry per stale
    /// input), or its carrier is invalid.
    Stale {
        /// Input keys whose recorded hash differs from the live hash.
        stale_inputs: Vec<String>,
    },
    /// The report exists but a feature the native checker does not implement is
    /// required to judge it. The payload names that feature.
    Unavailable {
        /// Why the report cannot be judged.
        reason: String,
    },
}

impl AnalysisVerdict {
    /// Whether the report is current.
    pub fn is_current(&self) -> bool {
        matches!(self, Self::Current)
    }

    /// The guard failures this verdict produces (empty when current).
    pub fn guard_failures(&self) -> Vec<String> {
        match self {
            Self::Current => Vec::new(),
            Self::Missing => vec![ANALYSIS_MISSING_MESSAGE.to_string()],
            Self::Stale { stale_inputs } => {
                if stale_inputs.is_empty() {
                    vec![format!("{ANALYSIS_STALE_PREFIX}{ANALYSIS_REPORT_FILENAME}")]
                } else {
                    stale_inputs
                        .iter()
                        .map(|name| format!("{ANALYSIS_STALE_PREFIX}{name}"))
                        .collect()
                }
            }
            Self::Unavailable { .. } => vec![ANALYSIS_UNAVAILABLE_MESSAGE.to_string()],
        }
    }

    /// The typed `error_code` this verdict carries.
    pub fn error_code(&self) -> Option<&'static str> {
        match self {
            Self::Current => None,
            Self::Missing => Some(ANALYSIS_REPORT_MISSING),
            Self::Stale { .. } => Some(ANALYSIS_REPORT_STALE),
            Self::Unavailable { .. } => Some(ANALYSIS_CURRENCY_UNAVAILABLE),
        }
    }
}

/// Normalize `tasks.md` status churn so a recorded analysis survives
/// `move-task` / `mark-status` transitions.
///
/// Two upstream rules, applied exactly:
///
/// * a leading list marker followed by `[ ]`, `[x]` or `[X]` becomes `[ ]`;
/// * a pipe-table cell whose entire trimmed content is `[ ]`, `[x]`, `[X]`,
///   `[D]` or `[P]` becomes `[ ]` (the surrounding whitespace is preserved).
pub fn normalize_tasks_md(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let _ = index;
        output.push_str(&normalize_pipe_status_cells(&normalize_checkbox(line)));
    }
    if !text.is_empty() && !text.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }
    output
}

fn normalize_checkbox(line: &str) -> String {
    // ^(\s*[-*]\s*)\[[ xX]\]
    let bytes = line.as_bytes();
    let mut position = 0;
    while position < bytes.len()
        && bytes[position].is_ascii_whitespace()
        && bytes[position] != b'\n'
    {
        position += 1;
    }
    if position >= bytes.len() || (bytes[position] != b'-' && bytes[position] != b'*') {
        return line.to_string();
    }
    position += 1;
    while position < bytes.len()
        && bytes[position].is_ascii_whitespace()
        && bytes[position] != b'\n'
    {
        position += 1;
    }
    if position + 2 >= bytes.len()
        || bytes[position] != b'['
        || !matches!(bytes[position + 1], b' ' | b'x' | b'X')
        || bytes[position + 2] != b']'
    {
        return line.to_string();
    }
    let mut normalized = String::with_capacity(line.len());
    normalized.push_str(&line[..position + 1]);
    normalized.push_str(" ]");
    normalized.push_str(&line[position + 3..]);
    normalized
}

fn normalize_pipe_status_cells(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let Some(pipe) = rest.find('|') else {
            output.push_str(rest);
            return output;
        };
        output.push_str(&rest[..=pipe]);
        let after = &rest[pipe + 1..];
        // |(\s*)\[[ xXDP]\](\s*)(?=\|)
        let trimmed_start = after.len() - after.trim_start().len();
        let leading = &after[..trimmed_start];
        let candidate = &after[trimmed_start..];
        let bytes = candidate.as_bytes();
        let is_marker = bytes.len() >= 3
            && bytes[0] == b'['
            && matches!(bytes[1], b' ' | b'x' | b'X' | b'D' | b'P')
            && bytes[2] == b']';
        if !is_marker {
            output.push_str(leading);
            rest = candidate;
            continue;
        }
        let after_marker = &candidate[3..];
        let trailing_start = after_marker.len() - after_marker.trim_start().len();
        let trailing = &after_marker[..trailing_start];
        if !after_marker[trailing_start..].starts_with('|') {
            output.push_str(leading);
            rest = candidate;
            continue;
        }
        output.push_str(leading);
        output.push_str("[ ]");
        output.push_str(trailing);
        rest = &after_marker[trailing_start..];
    }
}

/// Judge a recorded report against live input hashes.
///
/// `report` is the whole file text (frontmatter and body), or `None` when the
/// file is absent. `live` maps an input key (`spec.md`, `plan.md`, `tasks.md`,
/// `charter`) to its current SHA-256, or `None` when the input does not exist.
pub fn check_analysis_report(
    report: Option<&str>,
    live: &BTreeMap<String, Option<String>>,
) -> AnalysisVerdict {
    let Some(report) = report else {
        return AnalysisVerdict::Missing;
    };
    let frontmatter = match split_report_frontmatter(report) {
        Ok(Some(value)) => value,
        Ok(None) | Err(_) => {
            return AnalysisVerdict::Stale {
                stale_inputs: Vec::new(),
            };
        }
    };
    let object = match frontmatter {
        Value::Object(object) => object,
        _ => {
            return AnalysisVerdict::Stale {
                stale_inputs: Vec::new(),
            };
        }
    };
    if object.get("schema").and_then(Value::as_str) == Some(FINDINGS_SCHEMA_V1) {
        // A findings carrier is not a freshness carrier.
        return AnalysisVerdict::Stale {
            stale_inputs: Vec::new(),
        };
    }
    if object.get("artifact_type").and_then(Value::as_str) != Some(ANALYSIS_REPORT_ARTIFACT_TYPE) {
        return AnalysisVerdict::Stale {
            stale_inputs: Vec::new(),
        };
    }
    let Some(saved) = object.get("input_artifacts").and_then(Value::as_object) else {
        return AnalysisVerdict::Stale {
            stale_inputs: Vec::new(),
        };
    };
    // Features that change what the check must read are named, not guessed at.
    if object.contains_key("report_transaction") {
        return AnalysisVerdict::Unavailable {
            reason: "the recorded report is qualified by a report transaction, which the native checker does not implement".to_string(),
        };
    }
    if object.contains_key("material_manifest_version") {
        return AnalysisVerdict::Unavailable {
            reason: "the recorded report carries a material-input manifest, which the native checker does not implement".to_string(),
        };
    }

    let mut stale = Vec::new();
    for key in live.keys() {
        let saved_hash = saved
            .get(key)
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("sha256"))
            .and_then(Value::as_str);
        let live_hash = live.get(key).and_then(|value| value.as_deref());
        if saved_hash.is_none() || saved_hash != live_hash {
            stale.push(key.clone());
        }
    }
    for key in saved.keys() {
        if !live.contains_key(key) {
            stale.push(key.clone());
        }
    }
    if stale.is_empty() {
        return AnalysisVerdict::Current;
    }
    stale.sort();
    stale.dedup();
    AnalysisVerdict::Stale {
        stale_inputs: stale,
    }
}

/// Split a report's leading YAML frontmatter and decode it.
///
/// Returns `Ok(None)` when the file has no frontmatter block at all (the
/// upstream "missing_input_artifacts" path).
fn split_report_frontmatter(report: &str) -> Result<Option<Value>, String> {
    let source = report.strip_prefix('\u{feff}').unwrap_or(report);
    if !source.starts_with("---") {
        return Ok(None);
    }
    let mut lines = source.split_inclusive('\n');
    let Some(opening) = lines.next() else {
        return Ok(None);
    };
    if opening.trim_end() != "---" {
        return Ok(None);
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    if !closed {
        return Err("unterminated frontmatter".to_string());
    }
    let value: Value = serde_yaml_ng::from_str(&yaml).map_err(|error| error.to_string())?;
    Ok(Some(value))
}

/// The live input keys the software-dev analysis check hashes.
pub const ANALYSIS_INPUT_KEYS: [&str; 4] = ["spec.md", "plan.md", "tasks.md", "charter"];

#[cfg(test)]
mod tests {
    use super::*;

    fn live(map: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
        map.iter()
            .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
            .collect()
    }

    const CURRENT_REPORT: &str = "---\nartifact_type: analysis-report\ninput_artifacts:\n  spec.md:\n    path: kitty-specs/x/spec.md\n    sha256: aaa\n  plan.md:\n    path: kitty-specs/x/plan.md\n    sha256: bbb\n  tasks.md:\n    path: kitty-specs/x/tasks.md\n    sha256: ccc\n  charter:\n    path: .kittify/charter/charter.md\n    sha256: ddd\n---\n\n# Analysis\n";

    #[test]
    fn a_matching_report_is_current() {
        let verdict = check_analysis_report(
            Some(CURRENT_REPORT),
            &live(&[
                ("spec.md", Some("aaa")),
                ("plan.md", Some("bbb")),
                ("tasks.md", Some("ccc")),
                ("charter", Some("ddd")),
            ]),
        );
        assert_eq!(verdict, AnalysisVerdict::Current);
        assert!(verdict.guard_failures().is_empty());
    }

    #[test]
    fn an_absent_report_is_missing() {
        let verdict = check_analysis_report(None, &live(&[("spec.md", Some("aaa"))]));
        assert_eq!(verdict, AnalysisVerdict::Missing);
        assert_eq!(
            verdict.guard_failures(),
            vec![ANALYSIS_MISSING_MESSAGE.to_string()]
        );
        assert_eq!(verdict.error_code(), Some(ANALYSIS_REPORT_MISSING));
    }

    #[test]
    fn a_moved_input_names_every_stale_entry() {
        let verdict = check_analysis_report(
            Some(CURRENT_REPORT),
            &live(&[
                ("spec.md", Some("zzz")),
                ("plan.md", Some("bbb")),
                ("tasks.md", Some("ccc")),
                ("charter", Some("ddd")),
            ]),
        );
        assert_eq!(
            verdict,
            AnalysisVerdict::Stale {
                stale_inputs: vec!["spec.md".to_string()]
            }
        );
        assert_eq!(
            verdict.guard_failures(),
            vec!["Analysis report is stale: spec.md".to_string()]
        );
        assert_eq!(verdict.error_code(), Some(ANALYSIS_REPORT_STALE));
    }

    #[test]
    fn a_findings_carrier_or_a_wrong_artifact_type_is_stale() {
        let carrier = CURRENT_REPORT.replace(
            "artifact_type: analysis-report",
            "schema: analysis-findings/v1",
        );
        let verdict = check_analysis_report(Some(&carrier), &live(&[("spec.md", Some("aaa"))]));
        assert!(matches!(verdict, AnalysisVerdict::Stale { .. }));

        let wrong =
            CURRENT_REPORT.replace("artifact_type: analysis-report", "artifact_type: notes");
        let verdict = check_analysis_report(Some(&wrong), &live(&[("spec.md", Some("aaa"))]));
        assert!(matches!(verdict, AnalysisVerdict::Stale { .. }));
    }

    #[test]
    fn an_unsupported_feature_fails_closed_as_unavailable() {
        let transactional = CURRENT_REPORT.replace(
            "artifact_type: analysis-report",
            "artifact_type: analysis-report\nreport_transaction: abc",
        );
        let verdict =
            check_analysis_report(Some(&transactional), &live(&[("spec.md", Some("aaa"))]));
        assert!(matches!(verdict, AnalysisVerdict::Unavailable { .. }));
        assert_eq!(verdict.error_code(), Some(ANALYSIS_CURRENCY_UNAVAILABLE));
        assert_eq!(
            verdict.guard_failures(),
            vec![ANALYSIS_UNAVAILABLE_MESSAGE.to_string()]
        );
    }

    #[test]
    fn tasks_checkbox_churn_normalizes_but_prose_is_preserved() {
        let before = "- [ ] T001 alpha\n* [x] T002 beta\n  [ ] not a list\n";
        let after = "- [x] T001 alpha\n* [ ] T002 beta\n  [ ] not a list\n";
        assert_eq!(normalize_tasks_md(before), normalize_tasks_md(after));
        assert_eq!(
            normalize_tasks_md("- [X] T001 alpha\n"),
            "- [ ] T001 alpha\n"
        );
        assert_eq!(normalize_tasks_md("prose [x] stays"), "prose [x] stays");
        assert_eq!(normalize_tasks_md("no newline [x]"), "no newline [x]");
    }

    #[test]
    fn pipe_table_status_cells_normalize_only_whole_cells() {
        let before = "| WP01 | Tasks | [ ] |\n| WP02 | Tasks | [P] |\n";
        let after = "| WP01 | Tasks | [x] |\n| WP02 | Tasks | [D] |\n";
        assert_eq!(normalize_tasks_md(before), normalize_tasks_md(after));
        assert_eq!(normalize_tasks_md("| WP01 | [x] |\n"), "| WP01 | [ ] |\n");
        // A bracketed letter inside prose is not a whole table cell.
        assert_eq!(
            normalize_tasks_md("a | [x] note | b\n"),
            "a | [x] note | b\n"
        );
    }
}

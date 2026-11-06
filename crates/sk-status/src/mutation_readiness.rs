//! Dependency-readiness verdicts for the emit shells, and the declared-
//! dependency reader they need.
//!
//! Native counterpart of `specify_cli.status.dependency_verdict` +
//! `status.emit._declared_dependencies` / `_resolve_dependency_readiness` at
//! baseline `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
//!
//! Two separable decisions live here:
//!
//! 1. **What the WP file declares.** Only the raw top-level `dependencies`
//!    frontmatter value is read (the baseline deliberately never runs whole-model
//!    `WPMetadata` validation on the emit path), then normalised through the same
//!    legacy coercion `WPMetadata` applies: `"[]"` → empty, `"WP01, WP02"` →
//!    `["WP01", "WP02"]`, a bare `"WP01"` → `["WP01"]`, a list → itself. Zero
//!    matching `tasks/*.md` files declares nothing; more than one is unresolvable
//!    (a rename leftover), which yields an *unsatisfied* verdict rather than an
//!    error so only the guarded entry edges are refused and `--force` still wins.
//! 2. **Whether those dependencies are ready**, given the reduced write surface.
//!    The acceptable-ending authority is `status_lanes.is_acceptable_ending`:
//!    `approved` and `done` always satisfy; `canceled` satisfies only with
//!    operator-authored provenance (`reason_source == "operator"`); everything
//!    else — including a missing/absent lane — does not.
//!
//! The verdict is computed inside the writer's single in-lock reduction, so it
//! reflects post-lock state and adds no log read (`NFR-004`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::lane::Lane;
use crate::snapshot::{Snapshot, WorkPackageState};

/// The operator provenance marker the reduced snapshot carries on a canceled WP.
pub const OPERATOR_REASON_SOURCE: &str = "operator";

/// The marker used when the declared dependencies cannot be resolved.
pub const UNRESOLVABLE_MARKER: &str = "<unresolvable>";

/// One WP's dependency verdict (`core.dependency_graph.DependencyReadiness`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DependencyVerdict {
    pub wp_id: String,
    pub dependencies: Vec<String>,
    /// Dependencies that have not reached an acceptable ending. One entry may be
    /// the self-describing `<unresolvable> <reason>` marker, never a WP id.
    pub unsatisfied: Vec<String>,
}

impl DependencyVerdict {
    /// True when every declared dependency is acceptable.
    pub fn satisfied(&self) -> bool {
        self.unsatisfied.is_empty()
    }

    /// An *unsatisfied* verdict for a WP whose declarations cannot be read.
    pub fn unresolvable(wp_id: &str, reason: impl Into<String>) -> Self {
        DependencyVerdict {
            wp_id: wp_id.to_owned(),
            dependencies: Vec::new(),
            unsatisfied: vec![format!("{UNRESOLVABLE_MARKER} {}", reason.into())],
        }
    }
}

/// `status_lanes.is_acceptable_ending`: the single acceptable-ending authority.
pub fn is_acceptable_ending(lane: Lane, has_provenance: bool) -> bool {
    match lane {
        Lane::Approved | Lane::Done => true,
        Lane::Canceled => has_provenance,
        _ => false,
    }
}

/// `status_lanes.has_operator_provenance`: `reason_source == "operator"`.
pub fn has_operator_provenance(state: Option<&WorkPackageState>) -> bool {
    state
        .and_then(|state| state.get("reason_source"))
        .and_then(serde_json::Value::as_str)
        == Some(OPERATOR_REASON_SOURCE)
}

/// `dependency_verdict.wp_lanes_from_snapshot`: current lane per WP, `genesis`
/// when the state has no lane slot.
pub fn wp_lanes_from_snapshot(snapshot: &Snapshot) -> BTreeMap<String, String> {
    snapshot
        .work_packages
        .iter()
        .map(|(wp_id, state)| {
            (
                wp_id.clone(),
                state
                    .lane_string()
                    .unwrap_or(Lane::Genesis.as_str())
                    .to_owned(),
            )
        })
        .collect()
}

/// `dependency_readiness_for_wp` over a reduced snapshot (pure).
pub fn readiness_from_snapshot(
    snapshot: &Snapshot,
    wp_id: &str,
    dependencies: &[String],
) -> DependencyVerdict {
    let mut unsatisfied = Vec::new();
    for dependency in dependencies {
        let state = snapshot.work_packages.get(dependency);
        let lane = state
            .and_then(WorkPackageState::lane)
            .unwrap_or(Lane::Planned);
        if !is_acceptable_ending(lane, has_operator_provenance(state)) {
            unsatisfied.push(dependency.clone());
        }
    }
    DependencyVerdict {
        wp_id: wp_id.to_owned(),
        dependencies: dependencies.to_vec(),
        unsatisfied,
    }
}

/// Resolve the declared-dependencies verdict for one WP.
///
/// An unreadable/ambiguous declaration resolves to
/// [`DependencyVerdict::unresolvable`], never an error: the guard then refuses
/// only `planned -> claimed` / `claimed -> in_progress`, `--force` + reason still
/// overrides, and no other edge is affected.
pub fn resolve_dependency_verdict(
    snapshot: &Snapshot,
    feature_dir: &Path,
    wp_id: &str,
) -> DependencyVerdict {
    match declared_dependencies(feature_dir, wp_id) {
        Ok(dependencies) => readiness_from_snapshot(snapshot, wp_id, &dependencies),
        Err(reason) => DependencyVerdict::unresolvable(wp_id, reason),
    }
}

/// The raw declared `dependencies` of `wp_id`, or an explanation of why they
/// cannot be resolved.
pub fn declared_dependencies(feature_dir: &Path, wp_id: &str) -> Result<Vec<String>, String> {
    let matches = matching_wp_files(feature_dir, wp_id);
    match matches.len() {
        0 => Ok(Vec::new()),
        1 => {
            let path = &matches[0];
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("{} is unreadable ({error})", path.display()))?;
            frontmatter_dependencies(&text).map_err(|reason| format!("{} {reason}", path.display()))
        }
        count => {
            let names = matches
                .iter()
                .filter_map(|path| path.file_name().and_then(|value| value.to_str()))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "ambiguous match ({count} files in {}: {names})",
                feature_dir.join("tasks").display()
            ))
        }
    }
}

/// The `tasks/*.md` files the baseline's `_match_wp_files` regex accepts.
fn matching_wp_files(feature_dir: &Path, wp_id: &str) -> Vec<PathBuf> {
    let tasks_dir = feature_dir.join("tasks");
    let mut matches = Vec::new();
    let Ok(entries) = std::fs::read_dir(&tasks_dir) else {
        return matches;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.eq_ignore_ascii_case("readme.md") {
            continue;
        }
        // `^{wp_id}(?:[-_.]|\.md$)`
        let lower = name.to_ascii_lowercase();
        let prefix = wp_id.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() || rest.starts_with(['-', '_', '.']) {
            matches.push(path);
        }
    }
    matches.sort();
    matches
}

/// Read the top-level `dependencies` value from a Markdown frontmatter block.
///
/// Supported authored forms: a block list (`dependencies:` + `- WP01`), an
/// inline list (`dependencies: []`, `dependencies: [WP01, WP02]`), and the
/// legacy scalar string forms `"[]"`, `"WP01, WP02"`, `"WP01"`. A missing key is
/// an empty declaration. Any other shape is a refusal (fail-closed), matching
/// the baseline's `_coerce_declared_dependencies`.
fn frontmatter_dependencies(text: &str) -> Result<Vec<String>, String> {
    let Some(block) = frontmatter_block(text) else {
        return Err("has no YAML frontmatter block".to_owned());
    };
    let mut index = 0;
    while index < block.len() {
        let line = block[index];
        if let Some(inline) = line.strip_prefix("dependencies:") {
            let inline = inline.trim();
            if inline.is_empty() {
                let mut entries = Vec::new();
                index += 1;
                while index < block.len() {
                    let trimmed = block[index].trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        index += 1;
                        continue;
                    }
                    let Some(item) = trimmed.strip_prefix("- ") else {
                        break;
                    };
                    let item = strip_yaml_comment(item);
                    if !item.is_empty() {
                        entries.push(unquote(&item));
                    }
                    index += 1;
                }
                return Ok(normalize(entries));
            }
            let inline = strip_yaml_comment(inline);
            if inline.starts_with('[') {
                if !inline.ends_with(']') {
                    return Err("declares a malformed inline `dependencies` list".to_owned());
                }
                let inner = &inline[1..inline.len() - 1];
                if inner.trim().is_empty() {
                    return Ok(Vec::new());
                }
                return Ok(normalize(
                    split_flow_items(inner)
                        .into_iter()
                        .map(|item| unquote(&strip_yaml_comment(&item)))
                        .collect(),
                ));
            }
            let scalar = unquote(&inline);
            if scalar.trim() == "[]" {
                return Ok(Vec::new());
            }
            return Ok(normalize(
                scalar
                    .split(',')
                    .map(|item| item.trim().to_owned())
                    .collect(),
            ));
        }
        index += 1;
    }
    Ok(Vec::new())
}

fn frontmatter_block(text: &str) -> Option<Vec<&str>> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    let mut block = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            return Some(block);
        }
        block.push(line);
    }
    None
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

/// Drop a YAML end-of-line comment from one plain scalar / flow item.
///
/// `#` opens a comment only at a token boundary (start, or after whitespace)
/// and only outside quotes, so `WP#123` and `"a # b"` stay data. This is the
/// one place an inline `# first dependency` is removed before the id is
/// compared, which is what the baseline gets from its real YAML parse.
fn strip_yaml_comment(value: &str) -> String {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut previous_whitespace = true;
    for (index, character) in value.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' && active == '"' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => quote = Some(character),
            '#' if previous_whitespace => return value[..index].trim_end().to_owned(),
            _ => {}
        }
        previous_whitespace = character.is_whitespace();
    }
    value.trim_end().to_owned()
}

/// Split a YAML flow sequence body on commas outside quotes.
fn split_flow_items(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for character in value.chars() {
        if let Some(active) = quote {
            current.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' && active == '"' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' => {
                quote = Some(character);
                current.push(character);
            }
            ',' => {
                items.push(std::mem::take(&mut current));
            }
            _ => current.push(character),
        }
    }
    items.push(current);
    items
}

fn normalize(entries: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !seen.iter().any(|existing| existing == trimmed) {
            seen.push(trimmed.to_owned());
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acceptable_endings_match_the_shared_authority() {
        assert!(is_acceptable_ending(Lane::Approved, false));
        assert!(is_acceptable_ending(Lane::Done, false));
        assert!(!is_acceptable_ending(Lane::Canceled, false));
        assert!(is_acceptable_ending(Lane::Canceled, true));
        assert!(!is_acceptable_ending(Lane::InReview, true));
        assert!(!is_acceptable_ending(Lane::Planned, true));
    }

    #[test]
    fn legacy_dependency_strings_are_coerced() {
        assert_eq!(
            frontmatter_dependencies("---\ndependencies: []\n---\n").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            frontmatter_dependencies("---\ndependencies: \"WP01, WP02\"\n---\n").unwrap(),
            vec!["WP01".to_owned(), "WP02".to_owned()]
        );
        assert_eq!(
            frontmatter_dependencies("---\ndependencies: WP03\n---\n").unwrap(),
            vec!["WP03".to_owned()]
        );
        assert_eq!(
            frontmatter_dependencies("---\ndependencies:\n- WP01  # first\n- WP02\n---\n").unwrap(),
            vec!["WP01".to_owned(), "WP02".to_owned()]
        );
        // A `#` inside quotes (or glued to the id) is data, not a comment.
        assert_eq!(
            frontmatter_dependencies("---\ndependencies: ['WP#1', \"WP02 # two\"] # tail\n---\n")
                .unwrap(),
            vec!["WP#1".to_owned(), "WP02 # two".to_owned()]
        );
        assert_eq!(
            frontmatter_dependencies("---\ndependencies: WP03  # third\n---\n").unwrap(),
            vec!["WP03".to_owned()]
        );
        assert_eq!(
            frontmatter_dependencies("---\ntitle: x\n---\n").unwrap(),
            Vec::<String>::new()
        );
        assert!(frontmatter_dependencies("no frontmatter").is_err());
        assert!(frontmatter_dependencies("---\ndependencies: [unterminated\n---\n").is_err());
    }

    #[test]
    fn readiness_reads_lane_and_canceled_provenance() {
        let mut snapshot = Snapshot {
            mission_slug: "001-demo".to_owned(),
            materialized_at: String::new(),
            event_count: 0,
            last_event_id: None,
            work_packages: BTreeMap::new(),
            summary: BTreeMap::new(),
            mission_number: None,
            mission_type: None,
            retrospective: None,
            schema_version: None,
        };
        let mut approved = WorkPackageState::new();
        approved.insert("lane", json!("approved"));
        let mut canceled = WorkPackageState::new();
        canceled.insert("lane", json!("canceled"));
        canceled.insert("reason_source", json!("operator"));
        let mut synthetic = WorkPackageState::new();
        synthetic.insert("lane", json!("canceled"));
        synthetic.insert("reason_source", json!("synthetic"));
        snapshot.work_packages.insert("WP01".to_owned(), approved);
        snapshot.work_packages.insert("WP02".to_owned(), canceled);
        snapshot.work_packages.insert("WP03".to_owned(), synthetic);

        let verdict = readiness_from_snapshot(
            &snapshot,
            "WP09",
            &[
                "WP01".to_owned(),
                "WP02".to_owned(),
                "WP03".to_owned(),
                "WP04".to_owned(),
            ],
        );
        assert_eq!(
            verdict.unsatisfied,
            vec!["WP03".to_owned(), "WP04".to_owned()],
            "synthetic cancels and absent WPs are not ready; operator cancels are"
        );
        assert!(!verdict.satisfied());

        assert!(readiness_from_snapshot(&snapshot, "WP09", &[]).satisfied());
    }

    #[test]
    fn unresolvable_verdict_is_unsatisfied_with_a_marker() {
        let verdict = DependencyVerdict::unresolvable("WP01", "ambiguous match");
        assert!(!verdict.satisfied());
        assert_eq!(verdict.unsatisfied.len(), 1);
        assert!(verdict.unsatisfied[0].starts_with(UNRESOLVABLE_MARKER));
    }
}

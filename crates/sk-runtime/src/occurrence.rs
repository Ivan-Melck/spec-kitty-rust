//! The bulk-edit occurrence-map gate.
//!
//! Native reproduction of `specify_cli.bulk_edit.occurrence_map`'s
//! structural validation and admissibility checks, plus the self-conditioning
//! `ensure_occurrence_classification_ready` gate the `tasks` guard runs.
//!
//! Non-bulk-edit missions pass at zero cost, exactly as the reference does.
//! A `change_mode: bulk_edit` mission must carry a structurally valid,
//! admissible `occurrence_map.yaml`; every error string here is the
//! reference's own text so it can be emitted verbatim as a guard failure.

use serde_json::Value;

/// Accepted per-category actions (the pinned `action` enum).
pub const VALID_ACTIONS: [&str; 4] = [
    "rename",
    "manual_review",
    "do_not_change",
    "rename_if_user_visible",
];

/// Accepted `target.operation` values (the pinned `operation` enum).
pub const VALID_OPERATIONS: [&str; 3] = ["rename", "remove", "deprecate"];

/// The eight standard occurrence categories every admissible map classifies.
pub const STANDARD_CATEGORIES: [&str; 8] = [
    "code_symbols",
    "import_paths",
    "filesystem_paths",
    "serialized_keys",
    "cli_commands",
    "user_facing_strings",
    "tests_fixtures",
    "logs_telemetry",
];

/// Well-known placeholder target terms.
pub const PLACEHOLDER_TERMS: [&str; 5] = ["TODO", "TBD", "FIXME", "XXX", "PLACEHOLDER"];

/// An admissible map must classify at least this many categories.
pub const MIN_ADMISSIBLE_CATEGORIES: usize = 3;

const KNOWN_TOP_LEVEL_KEYS: [&str; 6] = [
    "target",
    "categories",
    "exceptions",
    "moves",
    "structural_targets",
    "status",
];

/// The outcome of a structural validation or admissibility check.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OccurrenceValidation {
    /// Validation errors, in the reference's order.
    pub errors: Vec<String>,
    /// Non-blocking warnings.
    pub warnings: Vec<String>,
}

impl OccurrenceValidation {
    /// Whether no errors were produced.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

fn as_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

/// Validate a parsed occurrence map for structural correctness.
pub fn validate_occurrence_map(raw: &Value) -> OccurrenceValidation {
    let mut result = OccurrenceValidation::default();
    let object = as_object(raw);
    let target = object.and_then(|object| object.get("target"));
    let categories = object.and_then(|object| object.get("categories"));
    let exceptions = object.and_then(|object| object.get("exceptions"));
    let moves = object.and_then(|object| object.get("moves"));
    let structural = object.and_then(|object| object.get("structural_targets"));

    result.errors.extend(validate_target(target));
    result.errors.extend(validate_categories(categories));
    result.errors.extend(validate_exceptions(exceptions));
    result.errors.extend(validate_moves(moves));
    result
        .errors
        .extend(validate_structural_targets(structural));

    if let Some(object) = object {
        for key in object.keys() {
            if !KNOWN_TOP_LEVEL_KEYS.contains(&key.as_str()) {
                result
                    .warnings
                    .push(format!("Unknown top-level key '{key}'"));
            }
        }
    }
    result
}

fn validate_target(target: Option<&Value>) -> Vec<String> {
    let Some(target) = target else {
        return vec!["Missing required 'target' section".to_string()];
    };
    let Some(target) = as_object(target) else {
        return vec!["'target' must be a mapping".to_string()];
    };
    let mut errors = Vec::new();
    match target.get("term") {
        None | Some(Value::Null) => errors.push("Missing required 'target.term'".to_string()),
        Some(Value::String(term)) if term.trim().is_empty() => {
            errors.push("'target.term' must be a non-empty string".to_string());
        }
        Some(Value::String(_)) => {}
        Some(_) => errors.push("'target.term' must be a non-empty string".to_string()),
    }
    if let Some(operation) = target.get("operation")
        && !matches!(operation, Value::Null)
    {
        let valid = operation
            .as_str()
            .is_some_and(|operation| VALID_OPERATIONS.contains(&operation));
        if !valid {
            let rendered = match operation {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let mut sorted = VALID_OPERATIONS;
            sorted.sort_unstable();
            errors.push(format!(
                "Invalid target.operation '{rendered}'; must be one of {}",
                python_string_list(&sorted)
            ));
        }
    }
    errors
}

fn validate_categories(categories: Option<&Value>) -> Vec<String> {
    let Some(categories) = categories else {
        return vec!["Missing required 'categories' section".to_string()];
    };
    let Some(categories) = as_object(categories) else {
        return vec!["'categories' must be a non-empty mapping".to_string()];
    };
    if categories.is_empty() {
        return vec!["'categories' must be a non-empty mapping".to_string()];
    }
    let mut errors = Vec::new();
    for (name, value) in categories {
        let Some(entry) = as_object(value) else {
            errors.push(format!("Category '{name}' must be a mapping"));
            continue;
        };
        match entry.get("action") {
            None | Some(Value::Null) => {
                errors.push(format!("Category '{name}' missing required 'action' key"));
            }
            Some(Value::String(action)) if VALID_ACTIONS.contains(&action.as_str()) => {}
            Some(action) => {
                let rendered = match action {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                let mut sorted = VALID_ACTIONS;
                sorted.sort_unstable();
                errors.push(format!(
                    "Category '{name}' has invalid action '{rendered}'; must be one of {}",
                    python_string_list(&sorted)
                ));
            }
        }
    }
    errors
}

fn validate_exceptions(exceptions: Option<&Value>) -> Vec<String> {
    let Some(exceptions) = exceptions else {
        return Vec::new();
    };
    if matches!(exceptions, Value::Null) {
        return Vec::new();
    }
    let Some(entries) = exceptions.as_array() else {
        return vec!["'exceptions' must be a list when present".to_string()];
    };
    let mut errors = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        errors.extend(validate_exception_entry(index, entry));
    }
    errors
}

fn validate_exception_entry(index: usize, entry: &Value) -> Vec<String> {
    let label = format!("exceptions[{index}]");
    let Some(entry) = as_object(entry) else {
        return vec![format!(
            "{label} must be a mapping with 'path' and 'action'"
        )];
    };
    let mut errors = Vec::new();
    match entry.get("path") {
        None | Some(Value::Null) => errors.push(format!("{label} missing required 'path' key")),
        Some(Value::String(path)) if path.trim().is_empty() => {
            errors.push(format!("{label}.path must be a non-empty string"));
        }
        Some(Value::String(_)) => {}
        Some(_) => errors.push(format!("{label}.path must be a non-empty string")),
    }
    match entry.get("action") {
        None | Some(Value::Null) => {
            errors.push(format!("{label} missing required 'action' key"));
        }
        Some(Value::String(action)) if VALID_ACTIONS.contains(&action.as_str()) => {}
        Some(action) => {
            let rendered = match action {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let mut sorted = VALID_ACTIONS;
            sorted.sort_unstable();
            errors.push(format!(
                "{label} has invalid action '{rendered}'; must be one of {}",
                python_string_list(&sorted)
            ));
        }
    }
    if let Some(field_path) = entry.get("field_path")
        && !matches!(field_path, Value::Null)
    {
        let valid = field_path
            .as_str()
            .is_some_and(|path| !path.trim().is_empty());
        if !valid {
            errors.push(format!(
                "{label}.field_path must be a non-empty string when present"
            ));
        }
    }
    errors
}

fn validate_moves(moves: Option<&Value>) -> Vec<String> {
    let Some(moves) = moves else {
        return Vec::new();
    };
    if matches!(moves, Value::Null) {
        return Vec::new();
    }
    let Some(entries) = moves.as_array() else {
        return vec!["'moves' must be a list when present".to_string()];
    };
    let mut errors = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        errors.extend(validate_move_entry(index, entry));
    }
    errors
}

fn validate_move_entry(index: usize, entry: &Value) -> Vec<String> {
    let label = format!("moves[{index}]");
    let Some(entry) = as_object(entry) else {
        return vec![format!("{label} must be a mapping with 'from' and 'to'")];
    };
    let mut errors = Vec::new();
    match entry.get("from") {
        None | Some(Value::Null) => errors.push(format!("{label} missing required 'from' key")),
        Some(sources) => {
            let valid = sources
                .as_array()
                .is_some_and(|sources| !sources.is_empty())
                && sources.as_array().is_some_and(|sources| {
                    sources.iter().all(|source| {
                        source
                            .as_str()
                            .is_some_and(|source| !source.trim().is_empty())
                    })
                });
            if !valid {
                errors.push(format!("{label}.from must be a non-empty list of paths"));
            }
        }
    }
    match entry.get("to") {
        None | Some(Value::Null) => errors.push(format!("{label} missing required 'to' key")),
        Some(Value::String(destination)) if !destination.trim().is_empty() => {}
        Some(_) => errors.push(format!("{label}.to must be a non-empty string")),
    }
    errors
}

fn validate_structural_targets(raw: Option<&Value>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    if matches!(raw, Value::Null) {
        return Vec::new();
    }
    let Some(entries) = raw.as_array() else {
        return vec!["'structural_targets' must be a list when present".to_string()];
    };
    let mut errors = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        errors.extend(validate_structural_target_entry(index, entry));
    }
    errors
}

fn validate_structural_target_entry(index: usize, entry: &Value) -> Vec<String> {
    let label = format!("structural_targets[{index}]");
    let Some(entry) = as_object(entry) else {
        return vec![format!(
            "{label} must be a mapping with 'path' and 'reason'"
        )];
    };
    let mut errors = Vec::new();
    match entry.get("path") {
        None | Some(Value::Null) => {
            errors.push(format!("{label} missing required 'path' key"));
        }
        Some(Value::String(path)) if path.trim().is_empty() => {
            errors.push(format!("{label}.path must be a non-empty string"));
        }
        Some(Value::String(path)) if !is_narrow_structural_path(path) => {
            errors.push(format!(
                "{label}.path '{path}' is too broad for a structural-target exemption: it must be a concrete file path with an extension (e.g. 'src/foo/bar.py') or an extension-bounded glob (e.g. 'src/foo/*.py', '**/bar_*.py'). Bare directories, directory-prefixes, and unbounded globs ('*', '**', '**/*') are rejected — a structural-target exemption must name specific file(s), never a whole directory tree."
            ));
        }
        Some(Value::String(_)) => {}
        Some(_) => errors.push(format!("{label}.path must be a non-empty string")),
    }
    match entry.get("reason") {
        None | Some(Value::Null) => {
            errors.push(format!("{label} missing required 'reason' key"));
        }
        Some(Value::String(reason)) if !reason.trim().is_empty() => {}
        Some(_) => errors.push(format!("{label}.reason must be a non-empty string")),
    }
    errors
}

fn is_narrow_structural_path(path: &str) -> bool {
    let normalized = path.trim();
    if normalized.is_empty() || normalized.ends_with('/') {
        return false;
    }
    if normalized == "*" || normalized == "**" || normalized.contains("**") {
        return false;
    }
    let basename = normalized.rsplit('/').next().unwrap_or(normalized);
    if basename == "*" || basename == "**" {
        return false;
    }
    let Some(dot) = basename.rfind('.') else {
        return false;
    };
    if dot == 0 {
        return false;
    }
    let extension = &basename[dot + 1..];
    !extension.is_empty() && !extension.contains('*') && !extension.contains('?')
}

/// Check whether a structurally valid map is admissible for execution.
pub fn check_admissibility(raw: &Value) -> OccurrenceValidation {
    let mut result = OccurrenceValidation::default();
    let Some(object) = as_object(raw) else {
        return result;
    };
    let target = object.get("target").and_then(as_object);
    let term = target
        .and_then(|target| target.get("term"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    let placeholder = term.is_empty()
        || PLACEHOLDER_TERMS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&term));
    if placeholder {
        let raw_term = target
            .and_then(|target| target.get("term"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        result.errors.push(format!(
            "target.term '{raw_term}' is a placeholder; provide a real term before execution"
        ));
    }

    let categories: Vec<&String> = object
        .get("categories")
        .and_then(as_object)
        .map(|categories| categories.keys().collect())
        .unwrap_or_default();
    if categories.len() < MIN_ADMISSIBLE_CATEGORIES {
        result.errors.push(format!(
            "Need at least {MIN_ADMISSIBLE_CATEGORIES} categories, got {}",
            categories.len()
        ));
    }

    let mut missing: Vec<&str> = STANDARD_CATEGORIES
        .iter()
        .copied()
        .filter(|category| {
            !categories
                .iter()
                .any(|present| present.as_str() == *category)
        })
        .collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        result.errors.push(format!(
            "Occurrence map is missing required standard categories: {}. Every category must be present — use action 'do_not_change' for risk surfaces that must not be modified.",
            python_string_list(&missing)
        ));
    }
    result
}

/// The gate's errors for a mission with the given `change_mode` and map.
///
/// `map` is `None` when the file is absent. `map_display` is the path the
/// operator should create (the reference embeds `feature_dir/occurrence_map.yaml`).
pub fn evaluate_occurrence_gate(
    change_mode: Option<&str>,
    map: Option<&Value>,
    map_display: &str,
) -> Vec<String> {
    if change_mode != Some("bulk_edit") {
        return Vec::new();
    }
    let Some(map) = map else {
        return vec![format!(
            "Occurrence map required for bulk_edit missions. Create {map_display} with target, categories, and actions."
        )];
    };
    let validation = validate_occurrence_map(map);
    if !validation.is_valid() {
        return validation.errors;
    }
    let admissibility = check_admissibility(map);
    if !admissibility.is_valid() {
        return admissibility.errors;
    }
    Vec::new()
}

fn python_string_list(items: &[impl AsRef<str>]) -> String {
    let rendered: Vec<String> = items
        .iter()
        .map(|item| format!("'{}'", item.as_ref()))
        .collect();
    format!("[{}]", rendered.join(", "))
}

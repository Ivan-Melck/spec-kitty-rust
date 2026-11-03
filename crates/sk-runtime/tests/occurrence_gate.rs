//! Oracle differential tests for the bulk-edit occurrence-map gate.
//!
//! `tests/golden/occurrence_oracle.json` is generated from
//! `specify_cli.bulk_edit.occurrence_map` by
//! `tests/oracle/generate_occurrence_goldens.py`; every expected error string
//! is the reference's own.

use std::path::PathBuf;

use serde_json::Value;
use sk_runtime::{
    check_occurrence_admissibility, evaluate_occurrence_gate, validate_occurrence_map,
};

fn golden() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/occurrence_oracle.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("occurrence golden is JSON")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string").to_string())
        .collect()
}

#[test]
fn structural_validation_matches_the_oracle() {
    let golden = golden();
    let cases = golden["cases"].as_object().expect("cases");
    assert!(!cases.is_empty());
    for (name, case) in cases {
        let result = validate_occurrence_map(&case["map"]);
        assert_eq!(
            result.errors,
            strings(&case["validation"]["errors"]),
            "{name}"
        );
        assert_eq!(
            result.warnings,
            strings(&case["validation"]["warnings"]),
            "{name}"
        );
        let admissibility = check_occurrence_admissibility(&case["map"]);
        assert_eq!(
            admissibility.errors,
            strings(&case["admissibility"]["errors"]),
            "{name} admissibility"
        );
    }
}

#[test]
fn the_gate_self_conditions_on_change_mode() {
    let golden = golden();
    let valid = &golden["cases"]["valid"]["map"];
    assert!(evaluate_occurrence_gate(None, Some(valid), "map").is_empty());
    assert!(evaluate_occurrence_gate(Some("normal"), Some(valid), "map").is_empty());
    assert!(evaluate_occurrence_gate(Some("bulk_edit"), Some(valid), "map").is_empty());
}

#[test]
fn a_bulk_edit_mission_without_a_map_refuses_with_the_reference_remedy() {
    let errors = evaluate_occurrence_gate(
        Some("bulk_edit"),
        None,
        "kitty-specs/demo-01M6GC7E/occurrence_map.yaml",
    );
    assert_eq!(
        errors,
        vec![
            "Occurrence map required for bulk_edit missions. Create \
             kitty-specs/demo-01M6GC7E/occurrence_map.yaml with target, categories, and actions."
                .to_string()
        ]
    );
}

#[test]
fn a_structurally_broken_map_reports_validation_errors_before_admissibility() {
    let golden = golden();
    let broken = &golden["cases"]["category_bad_action"]["map"];
    let errors = evaluate_occurrence_gate(Some("bulk_edit"), Some(broken), "map");
    assert_eq!(
        errors,
        vec![
            "Category 'code_symbols' has invalid action 'delete'; must be one of \
             ['do_not_change', 'manual_review', 'rename', 'rename_if_user_visible']"
                .to_string()
        ]
    );
}

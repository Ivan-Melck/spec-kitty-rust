//! Oracle differential tests for the requirement-ID grammar and gates.
//!
//! `tests/golden/requirements_oracle.json` is produced by
//! `tests/oracle/generate_requirement_goldens.py` from the pinned reference
//! implementation (`specify_cli.requirement_mapping` and
//! `runtime.next.runtime_bridge_cores`). Every expectation below is the
//! reference's own output, so this is a differential contract rather than a
//! structural expectation.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use sk_runtime::{
    BareProseCandidate, RefVerdict, RequirementMappingFacts, classify_requirement_ref,
    evaluate_bare_prose_requirements, evaluate_requirement_mapping,
    find_bare_prose_requirement_ids, find_requirement_ids, parse_requirement_id,
    parse_requirement_ids_from_spec_md, parse_requirement_refs_from_tasks_md,
};

fn golden() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/requirements_oracle.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).expect("requirement golden is JSON")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string").to_string())
        .collect()
}

fn id_json(id: &sk_runtime::RequirementId) -> Value {
    serde_json::json!({
        "kind": id.kind.as_str(),
        "digits": id.digits,
        "suffix": id.suffix.map(|suffix| suffix.to_string()),
        "mission": id.mission,
        "canonical": id.canonical(),
        "rendered": id.rendered(),
    })
}

#[test]
fn declared_requirement_parsing_matches_the_oracle() {
    let golden = golden();
    let specs = golden["specs"].as_object().expect("specs");
    assert!(!specs.is_empty());
    for (name, case) in specs {
        let content = case["content"].as_str().expect("content");
        let parsed = parse_requirement_ids_from_spec_md(content, "software-dev");
        let expected = &case["parsed"];
        assert_eq!(parsed.all, strings(&expected["all"]), "{name}: all ids");
        assert_eq!(
            parsed.functional,
            strings(&expected["functional"]),
            "{name}"
        );
        assert_eq!(
            parsed.non_functional,
            strings(&expected["non_functional"]),
            "{name}"
        );
        assert_eq!(
            parsed.constraint,
            strings(&expected["constraint"]),
            "{name}"
        );
        assert_eq!(
            parsed.success_criteria,
            strings(&expected["success_criteria"]),
            "{name}"
        );
    }
}

#[test]
fn token_scans_match_the_oracle_in_both_modes() {
    let golden = golden();
    for mode in ["prose", "prose_spec_scan"] {
        let spec_scan = mode == "prose_spec_scan";
        let cases = golden[mode].as_object().expect("prose object");
        assert!(!cases.is_empty());
        for (text, expected) in cases {
            let actual: Vec<Value> = find_requirement_ids(text, spec_scan, "software-dev")
                .iter()
                .map(id_json)
                .collect();
            assert_eq!(Value::Array(actual), *expected, "{mode}: {text}");
        }
    }
}

#[test]
fn ref_classification_matches_the_oracle() {
    let golden = golden();
    let cases = golden["classify"].as_array().expect("classify");
    assert!(!cases.is_empty());
    for case in cases {
        let raw = case["raw"].as_str().expect("raw");
        let declared: BTreeSet<String> = strings(&case["declared"]).into_iter().collect();
        let actual = match classify_requirement_ref(raw, &declared, "software-dev") {
            RefVerdict::Accepted(id) => serde_json::json!({"accepted": id.canonical()}),
            RefVerdict::Rejected { raw, reason } => {
                serde_json::json!({"rejected": {"raw": raw, "reason": reason}})
            }
        };
        assert_eq!(actual, case["verdict"], "{raw}");
    }
}

#[test]
fn parsed_ids_render_their_authored_qualifier() {
    let id = parse_requirement_id("other-01M6GC7E#fr-001a", "software-dev").expect("parse");
    assert_eq!(id.canonical(), "FR-001a");
    assert_eq!(id.rendered(), "other-01M6GC7E#FR-001a");
    assert!(id.is_foreign());
    assert_eq!(parse_requirement_id("dr-001", "software-dev"), None);
    assert!(parse_requirement_id("dr-001", "research").is_some());
}

#[test]
fn requirement_mapping_failures_match_the_oracle() {
    let golden = golden();
    let cases = golden["mapping"].as_array().expect("mapping");
    assert!(!cases.is_empty());
    for case in cases {
        let facts = &case["facts"];
        let name = facts["name"].as_str().expect("name");
        let mut refs = std::collections::BTreeMap::new();
        for (wp_id, values) in facts["refs"].as_object().expect("refs") {
            refs.insert(wp_id.clone(), strings(values));
        }
        let facts = RequirementMappingFacts {
            spec_requirement_ids: strings(&facts["spec"]).into_iter().collect(),
            functional_requirement_ids: strings(&facts["functional"]).into_iter().collect(),
            wp_ids: strings(&facts["wp_ids"]),
            wp_requirement_refs: refs,
            feature_dir_name: facts["feature_dir_name"]
                .as_str()
                .expect("feature dir")
                .to_string(),
            mission_type: facts
                .get("mission_type")
                .and_then(Value::as_str)
                .unwrap_or("software-dev")
                .to_string(),
        };
        assert_eq!(
            evaluate_requirement_mapping(&facts),
            strings(&case["failures"]),
            "{name}"
        );
    }
}

#[test]
fn bare_prose_detection_matches_the_oracle() {
    let golden = golden();
    let cases = golden["bare_prose"].as_object().expect("bare prose");
    assert!(!cases.is_empty());
    for (name, case) in cases {
        let content = case["content"].as_str().expect("content");
        let expected = &case["candidates"];
        let candidates = find_bare_prose_requirement_ids(content);
        let actual: Vec<Value> = candidates
            .iter()
            .map(|candidate: &BareProseCandidate| {
                serde_json::json!({
                    "section_heading": candidate.section_heading,
                    "ids": candidate.ids,
                })
            })
            .collect();
        assert_eq!(Value::Array(actual), *expected, "{name}");
        assert_eq!(
            Value::Array(
                evaluate_bare_prose_requirements(&candidates)
                    .into_iter()
                    .map(Value::String)
                    .collect()
            ),
            golden["bare_prose_failures"][name],
            "{name} failures"
        );
    }
}

#[test]
fn tasks_md_fallback_refs_match_the_oracle() {
    let golden = golden();
    let tasks_md = "# Tasks\n\n## WP01 - first\n\nRequirement refs:\n- FR-001\n- `NFR-001`\n\n## WP02 - second\n\nRequirement refs: FR-002, bogus\n\n## Work Package WP03 - third\n\n### Requirements\n\n- C-001\n";
    let actual = parse_requirement_refs_from_tasks_md(tasks_md, "software-dev");
    let expected = golden["tasks_md_refs"]["software-dev"]
        .as_object()
        .expect("tasks refs");
    assert_eq!(actual.len(), expected.len());
    for (wp_id, values) in expected {
        assert_eq!(
            actual.get(wp_id).cloned().unwrap_or_default(),
            strings(values),
            "{wp_id}"
        );
    }
}

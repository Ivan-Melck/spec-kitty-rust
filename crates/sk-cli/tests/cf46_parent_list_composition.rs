//! Canonical list availability is not the identity index's winning-stem view.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use sk_cli::compat::charter;

const STEM: &str = "003-decision-documentation-requirement";

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture(missing: bool) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    write(
        temporary.path(),
        ".kittify/config.yaml",
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: present\n        local_path: present\n{}",
            if missing {
                "      - name: missing\n        local_path: not-fetched\n"
            } else {
                ""
            }
        ),
    );
    write(
        temporary.path(),
        "present/org-charter.yaml",
        "schema_version: 2\norg_name: present\n",
    );
    write(
        temporary.path(),
        ".kittify/charter/activation.yaml",
        "activated_directives: []\n",
    );
    temporary
}

fn row<'a>(payload: &'a Value, kind: &str) -> &'a Value {
    payload["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == kind)
        .unwrap()
}

fn duplicated_layers(root: &Path) {
    for (directory, id) in [
        ("present/directives", "DIRECTIVE_PARENT_ORG"),
        (
            ".kittify/charter-packs/directive",
            "DIRECTIVE_PARENT_PROJECT",
        ),
    ] {
        write(
            root,
            &format!("{directory}/{STEM}.directive.yaml"),
            &format!(
                "schema_version: \"1.0\"\nid: {id}\ntitle: Parent fixture\nintent: Parent fixture\nenforcement: required\n"
            ),
        );
    }
}

#[test]
fn availability_preserves_each_layer_and_its_own_declared_id() {
    let temporary = fixture(false);
    duplicated_layers(temporary.path());
    let payload = charter::list_payload(temporary.path(), false, true).unwrap();
    let entries: Vec<Value> = row(&payload, "directive")["available"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["artifact_id"] == STEM)
        .cloned()
        .collect();
    assert_eq!(
        entries,
        vec![
            json!({"artifact_id": STEM, "layer": "built-in", "declared_id": "DIRECTIVE_003"}),
            json!({"artifact_id": STEM, "layer": "org", "declared_id": "DIRECTIVE_PARENT_ORG"}),
            json!({"artifact_id": STEM, "layer": "project", "declared_id": "DIRECTIVE_PARENT_PROJECT"}),
        ]
    );
}

#[test]
fn activating_the_project_declared_id_does_not_hide_other_layers() {
    let temporary = fixture(false);
    duplicated_layers(temporary.path());
    write(
        temporary.path(),
        ".kittify/charter/activation.yaml",
        "activated_directives:\n- DIRECTIVE_PARENT_PROJECT\n",
    );
    let payload = charter::list_payload(temporary.path(), false, true).unwrap();
    let layers: Vec<&str> = row(&payload, "directive")["available"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["artifact_id"] == STEM)
        .map(|entry| entry["layer"].as_str().unwrap())
        .collect();
    assert_eq!(layers, ["built-in", "org"]);
}

#[test]
fn an_unfetched_sibling_does_not_erase_readable_lenient_floors() {
    let temporary = fixture(true);
    write(
        temporary.path(),
        "present/org-charter.yaml",
        &format!("schema_version: 2\nrequired_directives:\n- {STEM}\n- missing-parent-directive\n"),
    );
    let payload = charter::list_payload(temporary.path(), false, false).unwrap();
    assert_eq!(
        payload["required_not_in_force"]["directives"],
        json!([
            {"id": STEM, "packs": ["present"]},
            {"id": "missing-parent-directive", "packs": ["present"]},
        ])
    );
    assert!(
        payload["required_floor_unreadable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|kind| kind == "tactics")
    );
}

#[test]
fn step_contract_overlay_ids_are_not_source_availability() {
    let temporary = fixture(false);
    for directory in [
        "present/mission_step_contracts",
        ".kittify/charter-packs/mission_step_contracts",
    ] {
        write(
            temporary.path(),
            &format!("{directory}/parent-overlay.step-contract.yaml"),
            "id: PARENT_OVERLAY_STEP\n",
        );
    }
    let payload = charter::list_payload(temporary.path(), false, true).unwrap();
    assert!(
        !row(&payload, "mission-step-contract")["available"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["artifact_id"] == "parent-overlay")
    );
}

#[test]
fn mission_type_activation_display_is_a_sorted_set_without_rewriting_the_ledger() {
    let temporary = fixture(false);
    let ledger = "mission_type_activations:\n- zeta\n- alpha\n- zeta\n";
    write(temporary.path(), ".kittify/charter/activation.yaml", ledger);
    let payload = charter::list_payload(temporary.path(), false, false).unwrap();
    assert_eq!(
        row(&payload, "mission-type")["activated"],
        json!(["alpha", "zeta"])
    );
    assert_eq!(
        fs::read_to_string(temporary.path().join(".kittify/charter/activation.yaml")).unwrap(),
        ledger
    );
}

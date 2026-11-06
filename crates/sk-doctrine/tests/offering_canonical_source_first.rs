//! E6 — Source-first conformance for the canonical offering/YAML/activation slice.
//!
//! Every test below runs the **actual reference first**, asserts the intended
//! cause it observed, and only then compares the actual Native answer. Both sides
//! are isolated re-executed children (`support/isolated.rs`): the Source child
//! asserts CPython 3.11.15, its CWD, HOME and all seven XDG/TMP roots plus the
//! absolute `PYTHONPATH`/pack root before resolving; the Native child asserts the
//! same nine roots and the same two absolute paths before answering. Ids travel
//! as code-point arrays, so nothing is projected lossily.
//!
//! Covered defects (E5 independent review findings):
//!
//! * **F1** — the activation gate (`_gated`): deny with the allow key absent, an
//!   explicitly empty allow set, a present allow set in stem and declared-id
//!   spelling, and the service-only step-contract case where the scan half cannot
//!   mask a wrong service mapping.
//! * **F2** — whole-model nested admission: a document the reference's Pydantic
//!   models reject must be skipped, including nested `extra="forbid"` keys,
//!   literals, patterns, `min_length` and unique step ids.
//! * **F6** — the mission-type roster model, per file (the reference's
//!   `scan_mission_types_dir` is the shared loud-fail primitive).
//! * **F7** — the raw authored declared id is the repository key; only the
//!   identity/availability phases strip.
//! * **F4** — the reference's uncaught constructor exceptions abort the pass (and
//!   the fallback re-read propagates them); malformed YAML stays a per-file skip.
//! * **F3** — the constructed Python `str()` domain: bytes, dates/datetimes,
//!   sets, `!!omap`, `!!pairs`, arbitrary integers and container nesting.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::Path;

fn write(fixture: &Path, relative: &str, body: &str) {
    let path = fixture.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
    std::fs::write(&path, body).expect("fixture file");
}

fn tactic(id: &str) -> String {
    format!(
        "schema_version: \"1.0\"\nid: {id}\nname: Tactic {id}\nsteps:\n  - title: Step one\n    description: d\n"
    )
}

fn contract(id: &str) -> String {
    format!(
        "schema_version: \"1.0\"\nid: {id}\naction: audit\nmission: documentation\nsteps:\n  - id: one\n    description: first\n  - id: two\n    description: second\n"
    )
}

const TACTIC_ATTRIBUTES: [&str; 1] = ["tactics"];
const CONTRACT_ATTRIBUTES: [&str; 1] = ["mission_step_contracts"];

fn ids(receipt: &isolated::FullReceipt, attribute: &str) -> Vec<String> {
    receipt.service[attribute]
        .iter()
        .map(|points| {
            points
                .iter()
                .map(|point| char::from_u32(*point).expect("scalar id"))
                .collect()
        })
        .collect()
}

fn contains(set: &isolated::SetView, text: &str) -> bool {
    isolated::has_id(set, text)
}

// ---------------------------------------------------------------------------
// F1 — the activation gate
// ---------------------------------------------------------------------------

/// Deny wins over allow, an absent allow key is still deny-netted, and a
/// service-only step contract is excluded from the effective set too.
#[test]
fn activation_gate_matches_the_reference_three_state_contract() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        let keys = isolated::requested_keys();
        let attributes = isolated::requested_attributes();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let attributes: Vec<&str> = attributes.iter().map(String::as_str).collect();
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_full_json(&root, &keys, &attributes)
        );
        return;
    }

    let fixture = isolated::fixture("gate-three-state");
    write(&fixture, ".kittify/config.yaml", "");
    write(
        &fixture,
        ".kittify/charter/activation.yaml",
        "deactivated_tactics: [project-denied-lens, moscow-scoping-lens]\n\
         activated_tactics: [project-denied-lens, project-allowed-lens, 901-allowed]\n\
         deactivated_mission_step_contracts: [project-service-only]\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/tactic/900-denied.tactic.yaml",
        &tactic("project-denied-lens"),
    );
    write(
        &fixture,
        ".kittify/charter-packs/tactic/901-allowed.tactic.yaml",
        &tactic("project-allowed-lens"),
    );
    write(
        &fixture,
        ".kittify/charter-packs/mission_step_contracts/902-service-only.step-contract.yaml",
        &contract("project-service-only"),
    );

    let keys = ["activated_tactics", "activated_mission_step_contracts"];
    let attributes = ["tactics", "mission_step_contracts"];
    let source = isolated::run_source_full(&fixture, &keys, &attributes);
    assert!(
        source.error.is_none(),
        "the reference resolves: {:?}",
        source.error
    );
    let source_tactics = ids(&source, "tactics");
    // The intended cause, asserted before the Native run: a denied id is gone
    // even when the allow set names it; the allowed id survives; a built-in the
    // deny set names is gone; the service-only contract is gone.
    assert!(
        !source_tactics.contains(&"project-denied-lens".to_owned()),
        "deny wins over allow: {source_tactics:?}"
    );
    assert!(
        source_tactics.contains(&"project-allowed-lens".to_owned()),
        "the allow set resolves the stem token 901-allowed: {source_tactics:?}"
    );
    assert!(
        !source_tactics.contains(&"moscow-scoping-lens".to_owned()),
        "a denied built-in is gone"
    );
    assert!(!ids(&source, "mission_step_contracts").contains(&"project-service-only".to_owned()));
    let tactic_set = source.entry("activated_tactics");
    assert!(
        contains(tactic_set, "900-denied"),
        "the scan half keeps the denied stem: {tactic_set:?}"
    );
    let contract_set = source.entry("activated_mission_step_contracts");
    assert!(contract_set.resolved);
    assert!(
        !contains(contract_set, "902-service-only")
            && !contains(contract_set, "project-service-only"),
        "a denied service-only contract is in neither half: {contract_set:?}"
    );

    let native = isolated::run_native_full(
        &fixture,
        &keys,
        &attributes,
        "activation_gate_matches_the_reference_three_state_contract",
    );
    assert_eq!(native.error, source.error);
    assert_eq!(
        ids(&native, "tactics"),
        source_tactics,
        "gated tactic mapping"
    );
    assert_eq!(
        ids(&native, "mission_step_contracts"),
        ids(&source, "mission_step_contracts"),
        "gated step-contract mapping"
    );
    isolated::assert_same(tactic_set, native.entry("activated_tactics"), "tactic set");
    isolated::assert_same(
        contract_set,
        native.entry("activated_mission_step_contracts"),
        "step-contract set",
    );
}

/// The allow key is *absent*: the deny list still filters the service mapping.
#[test]
fn activation_deny_applies_while_the_allow_key_is_absent() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        let keys = isolated::requested_keys();
        let attributes = isolated::requested_attributes();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let attributes: Vec<&str> = attributes.iter().map(String::as_str).collect();
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_full_json(&root, &keys, &attributes)
        );
        return;
    }

    let fixture = isolated::fixture("gate-deny-noallow");
    write(&fixture, ".kittify/config.yaml", "");
    write(
        &fixture,
        ".kittify/charter/activation.yaml",
        "deactivated_tactics: [project-denied-lens]\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/tactic/900-denied.tactic.yaml",
        &tactic("project-denied-lens"),
    );
    let keys = ["activated_tactics"];
    let attributes = TACTIC_ATTRIBUTES;
    let source = isolated::run_source_full(&fixture, &keys, &attributes);
    assert!(source.error.is_none(), "{:?}", source.error);
    let source_tactics = ids(&source, "tactics");
    assert!(
        !source_tactics.contains(&"project-denied-lens".to_owned()),
        "the deny list applies with no allow key: {source_tactics:?}"
    );
    assert!(
        source_tactics.contains(&"moscow-scoping-lens".to_owned()),
        "every other tactic still loads"
    );
    let native = isolated::run_native_full(
        &fixture,
        &keys,
        &attributes,
        "activation_deny_applies_while_the_allow_key_is_absent",
    );
    assert_eq!(ids(&native, "tactics"), source_tactics);
}

// ---------------------------------------------------------------------------
// F2 — nested admission
// ---------------------------------------------------------------------------

/// One fixture per kind, several nested-invalid documents each. The reference
/// skips every one of them (the built-ins stay); so must the Native loader.
#[test]
fn nested_invalid_documents_are_skipped_for_every_kind() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        let keys = isolated::requested_keys();
        let attributes = isolated::requested_attributes();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let attributes: Vec<&str> = attributes.iter().map(String::as_str).collect();
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_full_json(&root, &keys, &attributes)
        );
        return;
    }

    // (case name, attribute, relative path, body, rejected declared ids)
    struct Case {
        name: &'static str,
        attribute: &'static str,
        yaml_key: &'static str,
        directory: &'static str,
        files: Vec<(&'static str, String)>,
        rejected: Vec<&'static str>,
    }
    let cases = vec![
        Case {
            name: "nested-tactic",
            attribute: "tactics",
            yaml_key: "activated_tactics",
            directory: ".kittify/charter-packs/tactic",
            files: vec![
                (
                    "900-bad-ref.tactic.yaml",
                    format!(
                        "{}references:\n  - name: r\n    type: tactic\n    id: 'Bad_ID!'\n    when: w\n",
                        tactic("project-bad-ref")
                    ),
                ),
                (
                    "901-bad-ref-type.tactic.yaml",
                    format!(
                        "{}references:\n  - name: r\n    type: bogus_kind\n    id: Good_ID\n    when: w\n",
                        tactic("project-bad-ref-type")
                    ),
                ),
                (
                    "902-ref-extra.tactic.yaml",
                    format!(
                        "{}references:\n  - name: r\n    type: tactic\n    id: Good_ID\n    when: w\n    extra_ref_key: 1\n",
                        tactic("project-ref-extra")
                    ),
                ),
                (
                    "903-failure-modes.tactic.yaml",
                    format!("{}failure_modes: [1]\n", tactic("project-failure-modes")),
                ),
                (
                    "904-step-extra.tactic.yaml",
                    "schema_version: \"1.0\"\nid: project-step-extra\nname: N\nsteps:\n  - title: s\n    unknown_step_field: 1\n".to_owned(),
                ),
                (
                    "906-set-steps.tactic.yaml",
                    "schema_version: \"1.0\"\nid: project-set-steps\nname: N\nsteps: !!set {title: null}\n".to_owned(),
                ),
                (
                    "905-empty-steps.tactic.yaml",
                    "schema_version: \"1.0\"\nid: project-empty-steps\nname: N\nsteps: []\n".to_owned(),
                ),
            ],
            rejected: vec![
                "project-bad-ref",
                "project-bad-ref-type",
                "project-ref-extra",
                "project-failure-modes",
                "project-step-extra",
                "project-empty-steps",
                "project-set-steps",
            ],
        },
        Case {
            name: "nested-styleguide",
            attribute: "styleguides",
            yaml_key: "activated_styleguides",
            directory: ".kittify/charter-packs/styleguide",
            files: vec![
                (
                    "900-patterns-empty.styleguide.yaml",
                    "schema_version: \"1.0\"\nid: project-patterns-empty\ntitle: T\nscope: code\nprinciples:\n  - p\npatterns: []\n".to_owned(),
                ),
                (
                    "901-pattern-nested.styleguide.yaml",
                    "schema_version: \"1.0\"\nid: project-pattern-bad\ntitle: T\nscope: code\nprinciples:\n  - p\npatterns:\n  - name: p\n    description: d\n    unknown_pattern_key: 1\n".to_owned(),
                ),
                (
                    "902-scope.styleguide.yaml",
                    "schema_version: \"1.0\"\nid: project-scope-invalid\ntitle: T\nscope: nonsense\nprinciples:\n  - p\n".to_owned(),
                ),
            ],
            rejected: vec![
                "project-patterns-empty",
                "project-pattern-bad",
                "project-scope-invalid",
            ],
        },
        Case {
            name: "nested-procedure",
            attribute: "procedures",
            yaml_key: "activated_procedures",
            directory: ".kittify/charter-packs/procedure",
            files: vec![
                (
                    "900-actor.procedure.yaml",
                    "schema_version: \"1.0\"\nid: project-bad-actor\nname: N\npurpose: p\nentry_condition: e\nexit_condition: x\nsteps:\n  - title: s\n    actor: robot\n".to_owned(),
                ),
                (
                    "901-anti.procedure.yaml",
                    "schema_version: \"1.0\"\nid: project-bad-anti\nname: N\npurpose: p\nentry_condition: e\nexit_condition: x\nsteps:\n  - title: s\nanti_patterns:\n  - name: ''\n    description: d\n".to_owned(),
                ),
            ],
            rejected: vec!["project-bad-actor", "project-bad-anti"],
        },
        Case {
            name: "nested-glossary",
            attribute: "glossary_packs",
            yaml_key: "activated_glossary_packs",
            directory: ".kittify/charter-packs/glossary_packs",
            files: vec![
                (
                    "900-confidence.glossary-pack.yaml",
                    "id: project-bad-confidence\nprovenance: acme\nterms:\n  - surface: a\n    definition: d\n    confidence: high\n    status: active\n".to_owned(),
                ),
                (
                    "901-duplicate.glossary-pack.yaml",
                    "id: project-dup-surface\nprovenance: acme\nterms:\n  - surface: a\n    definition: d\n    confidence: 0.5\n    status: active\n  - surface: a\n    definition: e\n    confidence: 0.5\n    status: active\n".to_owned(),
                ),
                (
                    "902-extra.glossary-pack.yaml",
                    "id: project-term-extra\nprovenance: acme\nterms:\n  - surface: a\n    definition: d\n    confidence: 0.5\n    status: active\n    unknown_term_key: 1\n".to_owned(),
                ),
            ],
            rejected: vec![
                "project-bad-confidence",
                "project-dup-surface",
                "project-term-extra",
            ],
        },
        Case {
            name: "nested-step-contract",
            attribute: "mission_step_contracts",
            yaml_key: "activated_mission_step_contracts",
            directory: ".kittify/charter-packs/mission_step_contracts",
            files: vec![
                (
                    "900-dup.step-contract.yaml",
                    "schema_version: \"1.0\"\nid: project-dup-steps\naction: audit\nmission: documentation\nsteps:\n  - id: one\n    description: a\n  - id: one\n    description: b\n".to_owned(),
                ),
                (
                    "901-bad-gate.step-contract.yaml",
                    format!(
                        "{}gates:\n  - on_transition: a->b\n    handler: h\n    handler_kind: not_a_literal\n    schema_version: '1.0'\n",
                        contract("project-bad-gate")
                    ),
                ),
                (
                    "902-gate-extra.step-contract.yaml",
                    format!(
                        "{}gates:\n  - on_transition: a->b\n    handler: h\n    schema_version: '1.0'\n    unknown_gate_key: 1\n",
                        contract("project-gate-extra")
                    ),
                ),
                (
                    "903-gate-no-schema.step-contract.yaml",
                    format!(
                        "{}gates:\n  - on_transition: a->b\n    handler: h\n",
                        contract("project-gate-noschema")
                    ),
                ),
                (
                    "904-step-extra.step-contract.yaml",
                    "schema_version: \"1.0\"\nid: project-step-extra\naction: audit\nmission: documentation\nsteps:\n  - id: one\n    description: a\n    unknown_step_key: 1\n".to_owned(),
                ),
                (
                    "905-delegates-empty.step-contract.yaml",
                    "schema_version: \"1.0\"\nid: project-delegates-empty\naction: audit\nmission: documentation\nsteps:\n  - id: one\n    description: a\n    delegates_to:\n      kind: tactic\n      candidates: []\n".to_owned(),
                ),
                (
                    "906-inputs-empty.step-contract.yaml",
                    "schema_version: \"1.0\"\nid: project-inputs-empty\naction: audit\nmission: documentation\nsteps:\n  - id: one\n    description: a\n    inputs:\n      - flag: ''\n        source: s\n".to_owned(),
                ),
            ],
            rejected: vec![
                "project-dup-steps",
                "project-bad-gate",
                "project-gate-extra",
                "project-gate-noschema",
                "project-step-extra",
                "project-delegates-empty",
                "project-inputs-empty",
            ],
        },
    ];

    for case in cases {
        let fixture = isolated::fixture(case.name);
        write(&fixture, ".kittify/config.yaml", "");
        for (name, body) in &case.files {
            write(&fixture, &format!("{}/{name}", case.directory), body);
        }
        let keys = [case.yaml_key];
        let attributes = [case.attribute];
        let source = isolated::run_source_full(&fixture, &keys, &attributes);
        assert!(source.error.is_none(), "{}: {:?}", case.name, source.error);
        let source_ids = ids(&source, case.attribute);
        assert!(!source_ids.is_empty(), "{}: built-ins load", case.name);
        for rejected in &case.rejected {
            assert!(
                !source_ids.contains(&(*rejected).to_owned()),
                "{}: the reference skips {rejected}: {}",
                case.name,
                source_ids.len()
            );
        }
        let native = isolated::run_native_full(
            &fixture,
            &keys,
            &attributes,
            "nested_invalid_documents_are_skipped_for_every_kind",
        );
        assert_eq!(
            ids(&native, case.attribute),
            source_ids,
            "{}: the Native mapping must be the reference's",
            case.name
        );
        isolated::assert_same(
            source.entry(case.yaml_key),
            native.entry(case.yaml_key),
            case.name,
        );
    }
}

// ---------------------------------------------------------------------------
// F6 — mission-type roster model, per file
// ---------------------------------------------------------------------------

const MISSION_TYPE_PROBE: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
from charter.offering.missions.mission_type_repository import scan_mission_types_dir
out = {}
cases = root / "mission-types"
for case in sorted(cases.iterdir()):
    try:
        found = scan_mission_types_dir(case)
    except BaseException as exc:  # noqa: BLE001 - the reference's refusal is the answer
        out[case.name] = {"raised": type(exc).__name__}
    else:
        out[case.name] = {"ids": [[ord(char) for char in item.id] for item in found]}
print("MISSION_TYPE_RECEIPT " + json.dumps(out))
"#;

fn native_mission_type_json(root: &Path) -> String {
    let cases = root.join("mission-types");
    let mut out = serde_json::Map::new();
    let mut entries: Vec<_> = std::fs::read_dir(&cases)
        .expect("mission-type cases")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for directory in entries {
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .expect("case name")
            .to_owned();
        let value = match sk_doctrine::scan_mission_types_dir(&directory) {
            Ok(ids) => serde_json::json!({
                "ids": ids.iter().map(|id| id.codepoints().to_vec()).collect::<Vec<_>>(),
            }),
            Err(_) => serde_json::json!({"raised": "ValueError"}),
        };
        out.insert(name, value);
    }
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "cases": out,
    })
    .to_string()
}

/// Every mission-type model rule the reference enforces, one file per case.
#[test]
fn mission_type_roster_matches_the_reference_per_file() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!("NATIVE_MISSION_TYPE {}", native_mission_type_json(&root));
        return;
    }

    let fixture = isolated::fixture("mission-type-model");
    write(&fixture, ".kittify/config.yaml", "");
    let cases: Vec<(&str, &str)> = vec![
        (
            "schema-int",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\n",
        ),
        (
            "schema-string",
            "schema_version: '1'\nid: acme-x\ndisplay_name: S\n",
        ),
        (
            "schema-float-integral",
            "schema_version: 1.0\nid: acme-x\ndisplay_name: S\n",
        ),
        (
            "schema-bool",
            "schema_version: true\nid: acme-x\ndisplay_name: S\n",
        ),
        (
            "schema-float",
            "schema_version: 1.5\nid: acme-x\ndisplay_name: S\n",
        ),
        (
            "schema-bad",
            "schema_version: 'x'\nid: acme-x\ndisplay_name: S\n",
        ),
        (
            "path-bogus",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions:\n  bogus_key: wherever\n",
        ),
        (
            "path-valid",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions:\n  workspace: src/\n",
        ),
        (
            "path-int-value",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions:\n  workspace: 5\n",
        ),
        (
            "path-empty",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions: {}\n",
        ),
        (
            "path-int-key",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\npath_conventions:\n  1: x\n",
        ),
        (
            "seq-set",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\naction_sequence: !!set {a: null}\n",
        ),
        (
            "display-bytes",
            "schema_version: 1\nid: acme-x\ndisplay_name: !!binary Uw==\n",
        ),
        (
            "extends-int",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\nextends: 123\n",
        ),
        (
            "extends-str",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\nextends: plan\n",
        ),
        (
            "bool-yes",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\nrequires_substantive_spec: 'yes'\n",
        ),
        (
            "bool-int",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\ncode_producing: 1\n",
        ),
        (
            "bool-str",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\ncode_producing: 'true'\n",
        ),
        (
            "seq-dup",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\naction_sequence: [a, a]\n",
        ),
        (
            "seq-empty",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\naction_sequence: []\n",
        ),
        (
            "seq-int",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\naction_sequence: [1]\n",
        ),
        (
            "extra-key",
            "schema_version: 1\nid: acme-x\ndisplay_name: S\ntemplate_set: {}\n",
        ),
        (
            "id-upper",
            "schema_version: 1\nid: AcmeX\ndisplay_name: S\n",
        ),
        (
            "id-space",
            "schema_version: 1\nid: acme x\ndisplay_name: S\n",
        ),
        ("display-missing", "schema_version: 1\nid: acme-x\n"),
        (
            "id-mismatch",
            "schema_version: 1\nid: acme-y\ndisplay_name: S\n",
        ),
        ("non-mapping", "- a\n- b\n"),
        ("malformed", "id: [broken\n"),
        (
            "dup-key",
            "schema_version: 1\nid: acme-x\nid: acme-x\ndisplay_name: S\n",
        ),
        ("empty", ""),
    ];
    for (name, body) in &cases {
        // Every file is `acme-x.yaml`; the `id-mismatch` case's body declares
        // `acme-y`, which is the constraint under test.
        write(&fixture, &format!("mission-types/{name}/acme-x.yaml"), body);
    }

    let output = isolated::source_script(&fixture, MISSION_TYPE_PROBE, &[]);
    let source = isolated::receipt_json(&output, "MISSION_TYPE_RECEIPT ");
    let source_cases = source.as_object().expect("cases");
    // The intended cause, asserted per case before the Native run.
    for accepted in [
        "schema-int",
        "schema-string",
        "schema-float-integral",
        "schema-bool",
        "path-valid",
        "path-empty",
        "seq-set",
        "display-bytes",
        "extends-str",
        "bool-yes",
        "bool-int",
        "bool-str",
    ] {
        assert_eq!(
            source_cases[accepted]["ids"].as_array().map(Vec::len),
            Some(1),
            "the reference admits {accepted}: {:?}",
            source_cases[accepted]
        );
    }
    for rejected in [
        "schema-float",
        "schema-bad",
        "path-bogus",
        "path-int-value",
        "path-int-key",
        "extends-int",
        "seq-dup",
        "seq-empty",
        "seq-int",
        "extra-key",
        "id-upper",
        "id-space",
        "display-missing",
        "id-mismatch",
        "non-mapping",
        "malformed",
        "dup-key",
        "empty",
    ] {
        assert!(
            source_cases[rejected]["ids"].is_null(),
            "the reference rejects {rejected}: {:?}",
            source_cases[rejected]
        );
    }

    let stdout = isolated::native_reexec(
        &fixture,
        "mission_type_roster_matches_the_reference_per_file",
        &[],
    );
    let native = isolated::receipt_json(&stdout, "NATIVE_MISSION_TYPE ");
    let native_cases = native["cases"].as_object().expect("cases");
    for (name, _) in &cases {
        let source_entry = &source_cases[*name];
        let native_entry = &native_cases[*name];
        let source_ids = source_entry["ids"].as_array().cloned().unwrap_or_default();
        let native_ids = native_entry["ids"].as_array().cloned().unwrap_or_default();
        let source_raised = source_entry["raised"].is_string();
        let native_raised = native_entry["raised"].is_string();
        assert_eq!(native_raised, source_raised, "{name}: refusal parity");
        assert_eq!(native_ids, source_ids, "{name}: admitted ids");
    }
}

// ---------------------------------------------------------------------------
// F7 — raw declared ids keep their repository key
// ---------------------------------------------------------------------------

/// `id: "  padded  "` on a kind without an id pattern: the repository key is the
/// authored string, while the availability/identity phases still strip.
#[test]
fn padded_declared_ids_keep_the_authored_repository_key() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        let keys = isolated::requested_keys();
        let attributes = isolated::requested_attributes();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let attributes: Vec<&str> = attributes.iter().map(String::as_str).collect();
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_full_json(&root, &keys, &attributes)
        );
        return;
    }

    let fixture = isolated::fixture("padded-declared-id");
    write(&fixture, ".kittify/config.yaml", "");
    write(
        &fixture,
        ".kittify/charter-packs/mission_step_contracts/904-padded.step-contract.yaml",
        &contract("'  project-padded  '"),
    );
    // Pydantic's lax `str` accepts constructed `bytes`, so a `!!binary` id is
    // admitted under its decoded text — the model id, not the file stem.
    write(
        &fixture,
        ".kittify/charter-packs/mission_step_contracts/905-bytes.step-contract.yaml",
        &contract("!!binary cHJvamVjdC1ieXRlcw=="),
    );
    let keys = ["activated_mission_step_contracts"];
    let attributes = CONTRACT_ATTRIBUTES;
    let source = isolated::run_source_full(&fixture, &keys, &attributes);
    assert!(source.error.is_none(), "{:?}", source.error);
    let source_contracts = ids(&source, "mission_step_contracts");
    assert!(
        source_contracts.contains(&"  project-padded  ".to_owned()),
        "the repository key is the authored string: {source_contracts:?}"
    );
    assert!(
        source_contracts.contains(&"project-bytes".to_owned()),
        "a `!!binary` id is admitted under its decoded text: {source_contracts:?}"
    );
    let set = source.entry("activated_mission_step_contracts");
    // The identity index strips, so the padded id is *unknown* there and the
    // service key is kept verbatim rather than mapped to the file stem.
    assert!(
        contains(set, "  project-padded  ") && !contains(set, "904-padded"),
        "the effective set keeps the unknown raw id: {set:?}"
    );

    let native = isolated::run_native_full(
        &fixture,
        &keys,
        &attributes,
        "padded_declared_ids_keep_the_authored_repository_key",
    );
    assert_eq!(ids(&native, "mission_step_contracts"), source_contracts);
    isolated::assert_same(
        set,
        native.entry("activated_mission_step_contracts"),
        "padded declared id",
    );
}

// ---------------------------------------------------------------------------
// F4 — constructor aborts vs skips
// ---------------------------------------------------------------------------

/// `!!bool notabool` is a `KeyError` the reference does not catch: the pass
/// raises. Malformed YAML stays a per-file skip (and the pass still resolves).
#[test]
fn constructor_aborts_abort_the_pass_while_malformed_yaml_is_skipped() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        let keys = isolated::requested_keys();
        let attributes = isolated::requested_attributes();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let attributes: Vec<&str> = attributes.iter().map(String::as_str).collect();
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_full_json(&root, &keys, &attributes)
        );
        return;
    }

    // A constructor abort: the pass raises in the Reference.
    let abort = isolated::fixture("constructor-abort");
    write(&abort, ".kittify/config.yaml", "");
    write(
        &abort,
        ".kittify/charter-packs/tactic/905-badbool.tactic.yaml",
        &format!("{}notes: !!bool notabool\n", tactic("project-badbool")),
    );
    let keys = ["activated_tactics"];
    let attributes = TACTIC_ATTRIBUTES;
    let source = isolated::run_source_full(&abort, &keys, &attributes);
    assert_eq!(
        source.error.as_deref(),
        Some("KeyError: 'notabool'"),
        "the reference aborts the pass with the bare KeyError"
    );
    assert_eq!(
        source.service_errors.get("tactics").map(String::as_str),
        Some("KeyError: 'notabool'"),
        "the repository property raises the same KeyError"
    );
    let native = isolated::run_native_full(
        &abort,
        &keys,
        &attributes,
        "constructor_aborts_abort_the_pass_while_malformed_yaml_is_skipped",
    );
    assert_eq!(
        native.error, source.error,
        "the Native pass must abort with the same Python exception"
    );

    // An unhashable overlay id is a `TypeError` in the reference's id lookup.
    let unhashable = isolated::fixture("constructor-unhashable");
    write(&unhashable, ".kittify/config.yaml", "");
    write(
        &unhashable,
        ".kittify/charter-packs/tactic/906-unhashable.tactic.yaml",
        &tactic("[a, b]"),
    );
    let source = isolated::run_source_full(&unhashable, &keys, &attributes);
    assert_eq!(
        source.error.as_deref(),
        Some("TypeError: unhashable type: 'list'"),
        "an unhashable overlay id aborts the load"
    );
    assert_eq!(
        source.service_errors.get("tactics").map(String::as_str),
        Some("TypeError: unhashable type: 'list'")
    );
    let native = isolated::run_native_full(
        &unhashable,
        &keys,
        &attributes,
        "constructor_aborts_abort_the_pass_while_malformed_yaml_is_skipped",
    );
    assert_eq!(native.error, source.error);

    // Malformed YAML is a per-file skip, not an abort.
    let malformed = isolated::fixture("constructor-skip");
    write(&malformed, ".kittify/config.yaml", "");
    write(
        &malformed,
        ".kittify/charter-packs/tactic/907-malformed.tactic.yaml",
        "id: [broken\n",
    );
    let source = isolated::run_source_full(&malformed, &keys, &attributes);
    assert!(
        source.error.is_none(),
        "malformed YAML is skipped: {:?}",
        source.error
    );
    let set = source.entry("activated_tactics");
    assert!(set.resolved, "the pass still resolves: {set:?}");
    // The availability scan reads the file's `id:` and skips an unparsable one, so
    // a malformed file contributes no stem at all (E5's F5 identity shape).
    assert!(
        !contains(set, "907-malformed"),
        "an unparsable file contributes no availability stem: {set:?}"
    );
    let native = isolated::run_native_full(
        &malformed,
        &keys,
        &attributes,
        "constructor_aborts_abort_the_pass_while_malformed_yaml_is_skipped",
    );
    assert_eq!(native.error, None);
    isolated::assert_same(
        set,
        native.entry("activated_tactics"),
        "malformed YAML skip",
    );
}

// ---------------------------------------------------------------------------
// F3 — the constructed Python `str()` domain
// ---------------------------------------------------------------------------

const IDENTITY_PROBE: &str = r#"
import base64, json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert os.environ.get("PYTHONHASHSEED") == "0", "the hash seed is pinned"
from charter.activation.identity_index import IdentityIndex
from charter.offering.artifact_kinds import ArtifactKind
index = IdentityIndex(org_roots=[], project_root=root / ".kittify/charter-packs",
                      offering_root=repo / "packs/built-in")
kind = ArtifactKind.DIRECTIVE
out = {}
for path in sorted((root / ".kittify/charter-packs/directive").glob("*.directive.yaml")):
    stem = path.name.split(".")[0]
    value = index.declared_id(kind, stem)
    out[stem] = None if value is None else [ord(char) for char in str(value)]
print("IDENTITY_RECEIPT " + json.dumps(out))
"#;

fn native_identity_json(root: &Path) -> String {
    let repository = crate::isolated::repository_root();
    let index = sk_doctrine::IdentityIndex::from_roots(
        &repository.join("packs/built-in"),
        &[],
        Some(&root.join(".kittify/charter-packs")),
    );
    let directive = root.join(".kittify/charter-packs/directive");
    let mut entries: Vec<_> = std::fs::read_dir(&directive)
        .expect("directives")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    let mut out = serde_json::Map::new();
    for path in entries {
        let stem = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name")
            .split('.')
            .next()
            .expect("stem")
            .to_owned();
        let value = index
            .declared_id(sk_charter::ArtifactKind::Directive, &stem)
            .map(|text| {
                text.chars()
                    .map(|character| character as u32)
                    .collect::<Vec<_>>()
            });
        out.insert(stem, serde_json::json!(value));
    }
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "values": out,
    })
    .to_string()
}

/// Every constructed scalar/container kind, as the reference spells its `str()`.
#[test]
fn python_str_construction_domain_matches_source() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        assert_eq!(
            std::env::var("PYTHONHASHSEED").as_deref(),
            Ok("0"),
            "the hash seed is pinned"
        );
        println!("NATIVE_IDENTITY {}", native_identity_json(&root));
        return;
    }

    let fixture = isolated::fixture("identity-str-domain");
    write(&fixture, ".kittify/config.yaml", "");
    let bytes = b"Hello \"'\\\n\t\x00\xff";
    let bodies: Vec<(&str, String)> = vec![
        (
            "900-binary",
            format!("id: !!binary {}\n", base64_encode(bytes)),
        ),
        ("901-timestamp", "id: 2020-01-01T01:02:03\n".to_owned()),
        ("902-date", "id: 2020-01-01\n".to_owned()),
        ("903-tz", "id: 2020-01-01T01:02:03Z\n".to_owned()),
        ("904-micro", "id: 2020-01-01T01:02:03.500000\n".to_owned()),
        ("905-list", "id: [1, 2]\n".to_owned()),
        ("906-map", "id: {a: 1}\n".to_owned()),
        (
            "907-set",
            "id: !!set {alpha: null, beta: null}\n".to_owned(),
        ),
        ("908-pairs", "id: !!pairs [{a: b}, {c: d}]\n".to_owned()),
        ("909-omap", "id: !!omap [{a: b}, {c: d}]\n".to_owned()),
        ("910-nested", "id: [[1, 2], {a: [b, 3]}]\n".to_owned()),
        (
            "911-bigint",
            "id: 123456789012345678901234567890\n".to_owned(),
        ),
        ("912-float", "id: 1e20\n".to_owned()),
        ("913-negzero", "id: -0.0\n".to_owned()),
        ("914-nan", "id: .nan\n".to_owned()),
        ("915-inf", "id: .inf\n".to_owned()),
        ("916-bool", "id: true\n".to_owned()),
        ("917-null", "id: null\n".to_owned()),
        ("918-str", "id: \"it's\"\n".to_owned()),
        ("919-flow-implicit", "id: {x, y}\n".to_owned()),
        ("920-flow-colon", "id: {x: , y: }\n".to_owned()),
        ("921-set-implicit", "id: !!set {alpha, beta}\n".to_owned()),
        (
            "922-nested-implicit",
            "id: {a: {x, y}, b: [{z}]}\n".to_owned(),
        ),
        ("923-explicit-null", "id: {x: ~, y: null}\n".to_owned()),
        ("924-empty-strings", "id: {x: \"\", y: ''}\n".to_owned()),
        ("925-block-implicit", "id:\n  a:\n  b:\n".to_owned()),
        ("926-top-implicit", "id:\n".to_owned()),
    ];
    for (stem, body) in &bodies {
        write(
            &fixture,
            &format!(".kittify/charter-packs/directive/{stem}.directive.yaml"),
            body,
        );
    }

    // A `!!set`'s iteration order is CPython's hash order, so both children must
    // run with the same pinned seed (`PYTHONHASHSEED=0`) for the reference to
    // have one deterministic answer at all.
    let output = isolated::source_script(&fixture, IDENTITY_PROBE, &[("PYTHONHASHSEED", "0")]);
    let source = isolated::receipt_json(&output, "IDENTITY_RECEIPT ");
    let expected = source.as_object().expect("identity values");
    // The intended cause, asserted before the Native answer.
    let rendered = |stem: &str| -> String {
        expected[stem]
            .as_array()
            .unwrap_or_else(|| panic!("{stem}: {:?}", expected[stem]))
            .iter()
            .map(|point| {
                char::from_u32(u32::try_from(point.as_u64().expect("point")).expect("u32"))
                    .expect("char")
            })
            .collect()
    };
    assert!(
        rendered("900-binary").starts_with("b'Hello"),
        "{}",
        rendered("900-binary")
    );
    assert_eq!(rendered("901-timestamp"), "2020-01-01 01:02:03");
    assert_eq!(rendered("902-date"), "2020-01-01");
    assert_eq!(rendered("903-tz"), "2020-01-01 01:02:03+00:00");
    assert_eq!(rendered("904-micro"), "2020-01-01 01:02:03.500000");
    assert_eq!(rendered("905-list"), "[1, 2]");
    assert_eq!(rendered("906-map"), "{'a': 1}");
    assert!(
        rendered("907-set") == "{'alpha', 'beta'}" || rendered("907-set") == "{'beta', 'alpha'}",
        "a set renders with braces: {}",
        rendered("907-set")
    );
    assert_eq!(rendered("908-pairs"), "[('a', 'b'), ('c', 'd')]");
    assert_eq!(
        rendered("909-omap"),
        "ordereddict([('a', 'b'), ('c', 'd')])"
    );
    assert_eq!(rendered("910-nested"), "[[1, 2], {'a': ['b', 3]}]");
    assert_eq!(rendered("911-bigint"), "123456789012345678901234567890");
    assert_eq!(rendered("912-float"), "1e+20");
    assert_eq!(rendered("913-negzero"), "-0.0");
    assert_eq!(rendered("914-nan"), "nan");
    assert_eq!(rendered("915-inf"), "inf");
    assert_eq!(rendered("916-bool"), "True");
    assert_eq!(rendered("917-null"), "None");
    assert_eq!(rendered("918-str"), "it's");
    assert_eq!(rendered("919-flow-implicit"), "{'x': None, 'y': None}");
    assert_eq!(rendered("920-flow-colon"), "{'x': None, 'y': None}");
    assert!(
        rendered("921-set-implicit") == "{'alpha', 'beta'}"
            || rendered("921-set-implicit") == "{'beta', 'alpha'}"
    );
    assert_eq!(
        rendered("922-nested-implicit"),
        "{'a': {'x': None, 'y': None}, 'b': [{'z': None}]}"
    );
    assert_eq!(rendered("923-explicit-null"), "{'x': None, 'y': None}");
    assert_eq!(rendered("924-empty-strings"), "{'x': '', 'y': ''}");
    assert_eq!(rendered("925-block-implicit"), "{'a': None, 'b': None}");
    assert_eq!(rendered("926-top-implicit"), "None");

    let stdout = isolated::native_reexec(
        &fixture,
        "python_str_construction_domain_matches_source",
        &[("PYTHONHASHSEED", "0")],
    );
    let native = isolated::receipt_json(&stdout, "NATIVE_IDENTITY ");
    let values = native["values"].as_object().expect("identity values");
    for (stem, _) in &bodies {
        assert_eq!(values[*stem], expected[*stem], "{stem}: constructed str()");
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(first >> 2) as usize] as char);
        out.push(ALPHABET[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(third & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

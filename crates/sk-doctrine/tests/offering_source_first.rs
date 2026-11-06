//! Source-first conformance for the canonical offering service itself.
//!
//! `effective_set_source_first.rs` compares the *effective sets*. This suite
//! compares the service's own per-kind mappings to the reference's
//! `build_active_charter_service(repo_root)` repositories, because a mapping the
//! scan half happens to mask in the union (a language-excluded artifact still
//! contributes its file stem) must still be right for the promotion callers that
//! consume the service directly.
//!
//! Both sides run as isolated re-executed children (`support/isolated.rs`); the
//! Source child also asserts its interpreter version, CWD and every root before
//! building the service. Mappings travel as exact code-point arrays.
//!
//! Two tests at the end are explicitly labelled API-shape tests: they assert the
//! DTO/roster contract rather than comparing against the reference.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::Path;

/// The eight kinds whose service mapping the reference consults.
const MAPPINGS: [&str; 8] = [
    "tactics",
    "styleguides",
    "toolguides",
    "paradigms",
    "procedures",
    "agent_profiles",
    "mission_step_contracts",
    "glossary_packs",
];

fn built_in(relative: &str) -> String {
    let path = isolated::repository_root()
        .join("packs/built-in")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn write(fixture: &Path, relative: &str, body: &str) {
    let path = fixture.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
    std::fs::write(&path, body).expect("fixture file");
}

fn declare_org_pack(fixture: &Path, extra_config: &str) {
    write(
        fixture,
        ".kittify/config.yaml",
        &format!(
            "{extra_config}charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme-org\n"
        ),
    );
}

const SERVICE_PROBE: &str = r#"
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
from charter.activation.active_charter_service_builder import build_active_charter_service
service = build_active_charter_service(root)
mappings = {}
for attribute in json.loads(os.environ["SK_E4_MAPPINGS"]):
    mapping = getattr(service, attribute)
    mappings[attribute] = [[ord(char) for char in key] for key in sorted(mapping.keys())]
print("SERVICE_RECEIPT " + json.dumps({
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "mappings": mappings,
}))
"#;

type Mapping = Vec<Vec<u32>>;

fn parse_mappings(stdout: &str) -> std::collections::BTreeMap<String, Mapping> {
    let line = stdout
        .lines()
        .find(|line| line.starts_with("SERVICE_RECEIPT "))
        .unwrap_or_else(|| panic!("no service receipt in child stdout: {stdout:?}"));
    let value: serde_json::Value =
        serde_json::from_str(line["SERVICE_RECEIPT ".len()..].trim()).expect("receipt JSON");
    assert_eq!(
        value["runner"].as_str(),
        Some(concat!("source-python-", "3.11.15")),
        "the Source child is the pinned interpreter"
    );
    value["mappings"]
        .as_object()
        .expect("mappings object")
        .iter()
        .map(|(attribute, keys)| {
            let keys = keys
                .as_array()
                .expect("mapping keys")
                .iter()
                .map(|key| {
                    key.as_array()
                        .expect("code points")
                        .iter()
                        .map(|point| {
                            u32::try_from(point.as_u64().expect("code point")).expect("u32")
                        })
                        .collect()
                })
                .collect();
            (attribute.clone(), keys)
        })
        .collect()
}

/// Run the reference's service builder in the isolated Source child.
fn run_source_service(root: &Path) -> std::collections::BTreeMap<String, Mapping> {
    let mappings = serde_json::to_string(&MAPPINGS).expect("mappings JSON");
    let output = isolated::source_script(root, SERVICE_PROBE, &[("SK_E4_MAPPINGS", &mappings)]);
    parse_mappings(&output)
}

/// The Native service's mappings for the same fixture, in the same shape.
fn native_mappings(root: &Path) -> std::collections::BTreeMap<String, Mapping> {
    let context = sk_doctrine::OfferingContext::resolve(root).expect("offering context");
    let service = sk_doctrine::CanonicalOfferingService::new(context);
    MAPPINGS
        .iter()
        .map(|attribute| {
            let yaml_key = format!("activated_{attribute}");
            let keys = service
                .ids_for(&yaml_key)
                .unwrap_or_else(|error| panic!("{attribute} keys: {error}"))
                .iter()
                .map(|id| id.codepoints().to_vec())
                .collect();
            ((*attribute).to_owned(), keys)
        })
        .collect()
}

/// A fixture with an org tactic, a python-scoped org styleguide and a project
/// step contract, under `languages: [rust]` when `rust` is set.
fn layered_fixture(case: &str, rust: bool) -> std::path::PathBuf {
    let fixture = isolated::fixture(case);
    declare_org_pack(&fixture, if rust { "languages:\n  - rust\n" } else { "" });
    let tactic = built_in("tactics/moscow-scoping-lens.tactic.yaml")
        .replace("id: moscow-scoping-lens", "id: acme-scoping-lens");
    write(
        &fixture,
        "packs/acme-org/tactics/900-acme-scoping.tactic.yaml",
        &tactic,
    );
    let styleguide = built_in("styleguides/python-conventions.styleguide.yaml")
        .replace("id: python-conventions", "id: acme-python-conventions");
    write(
        &fixture,
        "packs/acme-org/styleguides/900-acme-python.styleguide.yaml",
        &styleguide,
    );
    let contract =
        built_in("missions/built_in_step_contracts/documentation-audit.step-contract.yaml")
            .replace("id: documentation-audit", "id: project-documentation-audit");
    write(
        &fixture,
        ".kittify/charter-packs/mission_step_contracts/901-project-audit.step-contract.yaml",
        &contract,
    );
    fixture
}

/// The service mappings are the reference's loaded repository keys, kind by kind.
#[test]
fn service_mappings_match_the_reference_repositories() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!("NATIVE_SERVICE_RECEIPT {}", native_mappings_json(&root));
        return;
    }
    let fixture = layered_fixture("service-mappings", false);
    let source = run_source_service(&fixture);
    let native = native_mappings(&fixture);
    for attribute in MAPPINGS {
        let expected = source
            .get(attribute)
            .unwrap_or_else(|| panic!("reference reports {attribute}"));
        assert!(!expected.is_empty(), "reference loads some {attribute}");
        assert_eq!(
            native.get(attribute).expect("native mapping"),
            expected,
            "{attribute} mapping"
        );
    }
}

/// Language inference narrows the **service** mapping and never the effective
/// set: a python-scoped styleguide disappears from `service.styleguides` under
/// `languages: [rust]`, while both sides still list its file stem in the set.
#[test]
fn language_gating_narrows_the_service_not_the_scan() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        let keys = ["activated_styleguides"];
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &keys)
        );
        println!("NATIVE_SERVICE_RECEIPT {}", native_mappings_json(&root));
        return;
    }
    let open = layered_fixture("service-languages-open", false);
    let rust = layered_fixture("service-languages-rust", true);

    let reference_open = run_source_service(&open);
    let reference_rust = run_source_service(&rust);
    let native_open = native_mappings(&open);
    let native_rust = native_mappings(&rust);
    for attribute in MAPPINGS {
        assert_eq!(
            native_open.get(attribute),
            reference_open.get(attribute),
            "open {attribute}"
        );
        assert_eq!(
            native_rust.get(attribute),
            reference_rust.get(attribute),
            "rust {attribute}"
        );
    }

    // The intended cause: the scoped styleguide is admitted with no language
    // signal and excluded once the project declares rust.
    let scoped = "acme-python-conventions"
        .chars()
        .map(|character| character as u32)
        .collect::<Vec<u32>>();
    assert!(
        reference_open["styleguides"].contains(&scoped),
        "no signal admits the scoped styleguide: {:?}",
        reference_open["styleguides"]
    );
    assert!(
        !reference_rust["styleguides"].contains(&scoped),
        "an explicit rust project excludes the python-scoped styleguide: {:?}",
        reference_rust["styleguides"]
    );
    assert_eq!(native_rust["styleguides"], reference_rust["styleguides"]);

    // And the effective set still lists the excluded file's stem in both.
    let keys = ["activated_styleguides"];
    let (source_set, native_set) = isolated::run_pair(
        &rust,
        &keys,
        "language_gating_narrows_the_service_not_the_scan",
    );
    let reference = isolated::entry(&source_set, "activated_styleguides");
    assert!(
        isolated::has_id(reference, "900-acme-python"),
        "the scan half keeps the stem: {reference:?}"
    );
    isolated::assert_same(
        reference,
        isolated::entry(&native_set, "activated_styleguides"),
        "language-gated effective set",
    );
}

fn native_mappings_json(root: &Path) -> String {
    let mappings: serde_json::Map<String, serde_json::Value> = native_mappings(root)
        .into_iter()
        .map(|(attribute, keys)| (attribute, serde_json::json!(keys)))
        .collect();
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "mappings": mappings,
    })
    .to_string()
}

/// The roster API is flat, schema-validated and per-root-map (API-shape test).
#[test]
fn mission_type_roster_api_drops_only_the_broken_map() {
    let fixture = isolated::fixture("roster-api");
    declare_org_pack(&fixture, "");
    write(
        &fixture,
        "packs/acme-org/mission_types/acme-flow.yaml",
        "schema_version: 1\nid: acme-flow\ndisplay_name: Acme Flow\n",
    );
    write(
        &fixture,
        "packs/acme-org/mission_types/broken.yaml",
        "id: broken\ndisplay_name: Broken\nunknown_key: 1\n",
    );
    let context = sk_doctrine::OfferingContext::resolve(&fixture).expect("offering context");
    let ids = sk_doctrine::mission_type_fallback(&context.roots);
    let scalars: Vec<String> = ids
        .iter()
        .map(|id| id.to_scalar_string().expect("roster ids are scalar"))
        .collect();
    assert!(scalars.contains(&"software-dev".to_owned()), "{scalars:?}");
    assert!(
        !scalars.contains(&"acme-flow".to_owned()),
        "the malformed file poisons its whole map: {scalars:?}"
    );
    assert!(!scalars.contains(&"broken".to_owned()), "{scalars:?}");
}

/// Exact ids are the authority; the scalar projection is explicit (API-shape test).
#[test]
fn exact_ids_are_carried_and_the_scalar_view_is_explicit() {
    let lone_surrogate =
        sk_kernel::PythonString::from_codepoints(&[0x61, 0xD800]).expect("code points");
    let scalar = sk_kernel::PythonString::from_scalars("plain");
    let set = sk_doctrine::EffectiveSet {
        kind: "tactic".to_owned(),
        yaml_key: "activated_tactics".to_owned(),
        ids: vec![lone_surrogate.clone(), scalar.clone()],
        fallback_ids: vec![lone_surrogate.clone()],
        resolved: true,
        reason: None,
    };
    assert_eq!(set.ids[0].codepoints(), &[0x61, 0xD800]);
    assert_eq!(
        set.scalar_ids(),
        None,
        "a set carrying a non-scalar id has no silent scalar projection"
    );
    assert_eq!(set.scalar_fallback_ids(), None);
    let scalar_only = sk_doctrine::EffectiveSet {
        ids: vec![scalar],
        fallback_ids: Vec::new(),
        ..set
    };
    assert_eq!(scalar_only.scalar_ids(), Some(vec!["plain".to_owned()]));
}

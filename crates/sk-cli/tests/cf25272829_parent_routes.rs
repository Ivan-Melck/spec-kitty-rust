//! Bounded real-binary registry routes and capability facts; not paired Source proof.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    write(
        &temp.path().join(".kittify/config.yaml"),
        "agents:\n  available: [codex]\n",
    );
    write(
        &temp.path().join("pyproject.toml"),
        "[project]\nname = 'fixture'\nversion = '3.2.0'\n",
    );
    fs::create_dir_all(temp.path().join("home")).unwrap();
    temp
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(root)
        .args(args)
        .env("HOME", root.join("home"))
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env("SPECIFY_KITTY_NO_NAG", "1")
        .env("SPECIFY_KITTY_NO_UPGRADE_CHECK", "1")
        .output()
        .expect("real binary")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("one JSON document: {error}: {output:?}"))
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[test]
fn both_empty_registries_are_reached_and_read_only() {
    let temp = fixture();
    let root = temp.path();
    write(
        &root.join("docs/migrations/shim-registry.yaml"),
        "shims: []\n",
    );
    write(
        &root.join("docs/contracts/contract-registry.yaml"),
        "contracts: []\n",
    );
    let before = files(root);
    let shim = run(root, &["doctor", "shim-registry", "--json"]);
    assert_eq!(shim.status.code(), Some(0), "{shim:?}");
    assert_eq!(json(&shim)["entries"], json!([]));
    assert_eq!(json(&shim)["project_version"], json!("3.2.0"));
    assert_eq!(json(&shim)["has_overdue"], json!(false));
    let contracts = run(root, &["doctor", "contracts", "--json"]);
    assert_eq!(contracts.status.code(), Some(0), "{contracts:?}");
    assert_eq!(json(&contracts)["record_count"], json!(0));
    assert_eq!(json(&contracts)["records"], json!([]));
    assert_eq!(files(root), before, "diagnostics write nothing");
}

#[test]
fn a_real_existing_overdue_shim_is_a_finding_not_unsupported() {
    let temp = fixture();
    let root = temp.path();
    write(
        &root.join("docs/migrations/shim-registry.yaml"),
        concat!(
            "shims:\n  - legacy_path: fixture.legacy\n",
            "    canonical_import: fixture.current\n",
            "    introduced_in_release: '3.1.0'\n",
            "    removal_target_release: '3.2.0'\n",
            "    tracker_issue: '#123'\n    grandfathered: false\n"
        ),
    );
    write(
        &root.join("src/fixture/legacy.py"),
        "# legacy shim fixture\n",
    );
    let before = files(root);
    let output = run(root, &["doctor", "shim-registry", "--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let payload = json(&output);
    assert_eq!(payload["has_overdue"], json!(true));
    assert_eq!(payload["entries"][0]["status"], json!("overdue"));
    assert_eq!(payload["entries"][0]["shim_exists"], json!(true));
    assert_eq!(payload["exit_code"], json!(1));
    assert_eq!(files(root), before);
}

#[test]
fn missing_registry_is_an_actual_config_refusal_not_unsupported() {
    let temp = fixture();
    let output = run(temp.path(), &["doctor", "contracts", "--json"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    // This is the current typed Native adapter contract; Source's registry
    // failure console channel remains separate wire debt, not a parity claim.
    let payload = json(&output);
    assert_eq!(payload["error"]["code"], json!("registry_not_found"));
    assert!(
        payload["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Contract registry not found at")
    );
}

#[test]
fn capabilities_stay_partial_and_name_actual_registry_reachability() {
    let temp = fixture();
    let output = run(temp.path(), &["capabilities", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let payload = json(&output);
    let commands = payload["commands"].as_array().unwrap();
    let doctor = commands.iter().find(|row| row["name"] == "doctor").unwrap();
    assert_eq!(doctor["state"], json!("partial"));
    assert_eq!(doctor["implemented_leaves"].as_array().unwrap().len(), 13);
    assert_eq!(doctor["refused_leaves"].as_array().unwrap().len(), 6);
    for name in ["shim-registry", "contracts"] {
        assert!(
            doctor["implemented_leaves"]
                .as_array()
                .unwrap()
                .contains(&json!(name))
        );
        assert!(
            !doctor["refused_leaves"]
                .as_array()
                .unwrap()
                .contains(&json!(name))
        );
    }
    let migrate = commands
        .iter()
        .find(|row| row["name"] == "migrate backfill-runtime-state")
        .unwrap();
    assert_eq!(migrate["state"], json!("partial"));
    assert_eq!(migrate["mutates"], json!(true));
}

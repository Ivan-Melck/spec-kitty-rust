//! CF27 — CLI adapter smoke for `doctor shim-registry` and `doctor contracts`.
//!
//! These exercise the real runners in
//! `sk_cli::compat::doctor::registries` against fresh `tempfile` fixtures, with
//! the project root passed explicitly (the workspace forbids `unsafe`, so tests
//! cannot mutate the process-global CWD).
//!
//! **Not whole-command parity.** Root dispatch for the two leaves is wired by
//! the parent (this lane supplies the adapters and the exact patch), so the
//! `doctor` subcommand is not invoked here. The exit/finding contract, the
//! configuration/schema refusals and the JSON path are covered directly.
//!
//! Deferred deep proof is listed in the lane report: Rich-rendered human-output
//! parity, `--json` byte comparison against the reference's `json.dumps` output
//! ordering, and the interactive root-discovery path.

use std::fs;
use std::path::Path;

use sk_cli::compat::doctor::registries::{
    CONFIGURATION_ERROR_PREFIX, CONTRACT_SCHEMA_ERROR_PREFIX, IMPLEMENTED_LEAVES_ADDED,
    SHIM_SCHEMA_ERROR_PREFIX, run_contracts_in, run_shim_registry_in,
};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn shim_project(root: &Path, version: &str, registry: &str) {
    write(
        &root.join("pyproject.toml"),
        &format!("[project]\nname = \"spec-kitty-cli\"\nversion = \"{version}\"\n"),
    );
    write(&root.join("docs/migrations/shim-registry.yaml"), registry);
}

fn shim_file(root: &Path, legacy_path: &str) {
    let mut base = root.join("src");
    for part in legacy_path.split('.') {
        base.push(part);
    }
    write(&base.with_extension("py"), "__deprecated__ = True\n");
}

const OVERDUE_REGISTRY: &str = "shims:\n  - legacy_path: specify_cli.example\n    \
     canonical_import: \"example\"\n    introduced_in_release: \"3.1.0\"\n    \
     removal_target_release: \"3.2.0\"\n    tracker_issue: \"#615\"\n    grandfathered: false\n";

const PENDING_REGISTRY: &str = "shims:\n  - legacy_path: specify_cli.example\n    \
     canonical_import: \"example\"\n    introduced_in_release: \"3.2.0\"\n    \
     removal_target_release: \"3.3.0\"\n    tracker_issue: \"#615\"\n    grandfathered: false\n";

const VALID_CONTRACT: &str = "contracts:\n  - id: test.synthetic-literal\n    \
     kind: retired_literal\n    anchor:\n      literals:\n        - value: synthetic_token\n    \
     status: retired\n    owner: \"#2441\"\n    replaced_by: a.b.c\n    retirement:\n      \
     introduced_in: \"3.0.0\"\n      removal_target: \"3.4.0\"\n      tracker_issue: \"#2077\"\n    \
     consumers:\n      scan_roots: [\"src\"]\n      exemptions: []\n    verification:\n      \
     enforcement: advisory\n";

#[test]
fn an_overdue_shim_is_the_only_finding_exit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", OVERDUE_REGISTRY);
    shim_file(root, "specify_cli.example");
    assert_eq!(run_shim_registry_in(root, false).expect("ran"), 1);

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", PENDING_REGISTRY);
    shim_file(root, "specify_cli.example");
    assert_eq!(run_shim_registry_in(root, false).expect("ran"), 0);

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", OVERDUE_REGISTRY);
    // No shim module on disk: `removed`, not overdue.
    assert_eq!(run_shim_registry_in(root, false).expect("ran"), 0);
}

#[test]
fn the_json_path_emits_and_still_applies_the_finding_exit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", OVERDUE_REGISTRY);
    shim_file(root, "specify_cli.example");
    // cargo captures stdout, so the document is emitted without noise; the
    // assertion here is that serialization succeeded and the exit is Source's.
    assert_eq!(run_shim_registry_in(root, true).expect("ran"), 1);

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        &root.join("docs/contracts/contract-registry.yaml"),
        VALID_CONTRACT,
    );
    assert_eq!(run_contracts_in(root, true).expect("ran"), 0);
}

#[test]
fn an_empty_registry_is_clean_not_an_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "shims: []\n");
    assert_eq!(run_shim_registry_in(root, false).expect("ran"), 0);

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        &root.join("docs/contracts/contract-registry.yaml"),
        "contracts: []\n",
    );
    assert_eq!(run_contracts_in(root, false).expect("ran"), 0);
}

#[test]
fn a_missing_registry_is_a_configuration_error_with_exit_two() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    // A project without either artifact: the shim command reads `pyproject.toml`
    // first, so with no project metadata that is the reported failure.
    let error = run_shim_registry_in(root, false).expect_err("refused");
    assert_eq!(error.exit(), 2);
    assert!(error.message().starts_with(CONFIGURATION_ERROR_PREFIX));
    assert!(error.message().contains("pyproject.toml not found at "));

    // A project with its metadata but no registry names the canonical home.
    write(
        &root.join("pyproject.toml"),
        "[project]\nname = \"spec-kitty-cli\"\nversion = \"3.2.0\"\n",
    );
    let error = run_shim_registry_in(root, false).expect_err("refused");
    assert_eq!(error.exit(), 2);
    assert!(error.message().starts_with(CONFIGURATION_ERROR_PREFIX));
    assert!(error.message().contains("Shim registry not found at "));
    assert_eq!(
        error.to_json()["error"]["code"],
        serde_json::json!("registry_not_found")
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let error = run_contracts_in(root, false).expect_err("refused");
    assert_eq!(error.exit(), 2);
    assert!(error.message().starts_with(CONFIGURATION_ERROR_PREFIX));
    assert!(error.message().contains("Contract registry not found at "));
}

#[test]
fn a_schema_violation_is_exit_two_and_never_a_clean_bill_of_health() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "shims:\n  - legacy_path: a.b\n");
    let error = run_shim_registry_in(root, false).expect_err("refused");
    assert_eq!(error.exit(), 2);
    assert!(error.message().starts_with(SHIM_SCHEMA_ERROR_PREFIX));
    assert!(error.message().contains("required field is missing"));
    assert_eq!(
        error.to_json()["error"]["code"],
        serde_json::json!("shim_registry_schema")
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        &root.join("docs/contracts/contract-registry.yaml"),
        &VALID_CONTRACT.replace(
            "      literals:\n        - value: synthetic_token",
            "      file: src/x.py\n      line: 7",
        ),
    );
    let error = run_contracts_in(root, false).expect_err("refused");
    assert_eq!(error.exit(), 2);
    assert!(error.message().starts_with(CONTRACT_SCHEMA_ERROR_PREFIX));
    assert!(error.message().contains("file:line"));
    assert_eq!(
        error.to_json()["error"]["code"],
        serde_json::json!("contract_registry_schema")
    );
}

#[test]
fn a_valid_registry_exits_zero_with_no_finding() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(
        &root.join("docs/contracts/contract-registry.yaml"),
        VALID_CONTRACT,
    );
    assert_eq!(run_contracts_in(root, false).expect("ran"), 0);
}

#[test]
fn the_adapter_names_the_two_reference_leaves() {
    assert_eq!(IMPLEMENTED_LEAVES_ADDED, ["shim-registry", "contracts"]);
}

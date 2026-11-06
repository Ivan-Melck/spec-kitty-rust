//! CF27 — the two NEW registry diagnostics: shim registry + contract registry.
//!
//! These are functional smoke tests for the real read-only diagnostics in
//! `sk_install::registry_diagnostics`:
//!
//! * the **seeded** artifacts in this checkout (`docs/migrations/shim-registry.yaml`,
//!   `docs/contracts/contract-registry.yaml`, `pyproject.toml`) load, validate
//!   and project into typed records;
//! * Source's four shim states and its two finding exits (0 / 1) plus the exit-2
//!   configuration/schema path;
//! * the Contract Registry's structural gate, including the DIR-041 positional
//!   `file:line` rejections and the fragment-reconstruction case;
//! * read-only behaviour, proved by a byte/mode/mtime/inode footprint.
//!
//! Deliberately **not** here (deferred deep proof, listed in the lane report):
//! byte-for-byte Source differential over a generated corpus, Rich human-output
//! parity, lone-surrogate/undecodable-YAML matrices, Windows/macOS path and
//! filesystem behaviour, and large-registry performance.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sk_install::Version;
use sk_install::registry_diagnostics::contract::{
    ALLOWED_ENFORCEMENT, ALLOWED_STATUSES, check_contract_registry, load_contract_registry,
    resolve_contract_registry_path, validate_contract_registry,
};
use sk_install::registry_diagnostics::shim::{
    CanonicalImport, SHIM_REGISTRY_REL, ShimStatus, check_shim_registry, classify,
    load_shim_registry, resolve_shim_registry_path, shim_exists,
};
use sk_install::registry_diagnostics::{
    RegistryDiagnosticsError, is_file_line_anchor, read_project_version,
};
use sk_kernel::PythonString;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root")
        .to_path_buf()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn project(version: &str) -> String {
    format!("[project]\nname = \"spec-kitty-cli\"\nversion = \"{version}\"\n")
}

/// Write `pyproject.toml` + the shim registry for a fixture project.
fn shim_project(root: &Path, version: &str, registry: &str) {
    write(&root.join("pyproject.toml"), &project(version));
    write(&root.join(SHIM_REGISTRY_REL), registry);
}

fn shim_entry(
    legacy_path: &str,
    canonical: &str,
    introduced: &str,
    removal: &str,
    issue: &str,
    grandfathered: bool,
) -> String {
    format!(
        "  - legacy_path: {legacy_path}\n    canonical_import: \"{canonical}\"\n    \
         introduced_in_release: \"{introduced}\"\n    removal_target_release: \"{removal}\"\n    \
         tracker_issue: \"{issue}\"\n    grandfathered: {grandfathered}\n"
    )
}

fn create_shim_file(root: &Path, legacy_path: &str) {
    let mut base = root.join("src");
    for part in legacy_path.split('.') {
        base.push(part);
    }
    write(&base.with_extension("py"), "__deprecated__ = True\n");
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

const WELL_FORMED_CONTRACT: &str = "\
contracts:
  - id: test.synthetic-literal
    kind: retired_literal
    anchor:
      literals:
        - value: synthetic_retired_token
    status: retired
    owner: \"#2441\"
    replaced_by: specify_cli.something.canonical
    retirement:
      introduced_in: \"3.0.0\"
      removal_target: \"3.4.0\"
      tracker_issue: \"#2077\"
    consumers:
      scan_roots: [\"src\"]
      exemptions: []
    verification:
      enforcement: advisory
";

fn contract_project(root: &Path, body: &str) {
    write(&root.join("docs/contracts/contract-registry.yaml"), body);
}

fn contract_errors(root: &Path) -> Vec<String> {
    check_contract_registry(root)
        .expect_err("must be refused")
        .schema_errors()
        .expect("schema failure")
        .to_vec()
}

fn shim_errors(root: &Path) -> Vec<String> {
    check_shim_registry(root)
        .expect_err("must be refused")
        .schema_errors()
        .expect("schema failure")
        .to_vec()
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

// ---------------------------------------------------------------------------
// The seeded artifacts in this checkout
// ---------------------------------------------------------------------------

#[test]
fn the_seeded_contract_registry_projects_both_records() {
    let root = repo_root();
    let report = check_contract_registry(&root).expect("seeded registry validates");
    assert_eq!(report.record_count(), 2);
    assert_eq!(report.advisory_count(), 2);
    assert_eq!(report.enforcing_count(), 0);
    assert_eq!(report.registry_path, resolve_contract_registry_path(&root));

    let ids: Vec<String> = report
        .records
        .iter()
        .map(|record| record.id.to_scalar_string().expect("id"))
        .collect();
    assert_eq!(
        ids,
        [
            "terminology.legacy-status-commit-terms",
            "paths.legacy-home-literals-cli-tree"
        ]
    );
    for record in &report.records {
        assert_eq!(record.kind.to_scalar_string().unwrap(), "retired_literal");
        assert_eq!(record.status.to_scalar_string().unwrap(), "retired");
        assert_eq!(
            record.verification.enforcement.to_scalar_string().unwrap(),
            "advisory"
        );
        assert!(record.anchor.symbol.is_none());
    }
}

#[test]
fn the_seeded_terminology_record_reconstructs_fragment_literals() {
    let root = repo_root();
    let records = load_contract_registry(&root).expect("seeded registry loads");
    let terminology = records
        .iter()
        .find(|record| {
            record.id.to_scalar_string().unwrap() == "terminology.legacy-status-commit-terms"
        })
        .expect("terminology record");
    let literals: Vec<String> = terminology
        .anchor
        .literals
        .iter()
        .map(|literal| literal.to_scalar_string().expect("literal"))
        .collect();
    // The manifest stores the two retired terms as `fragments:` so it does not
    // trip the sweep it models; the loader joins them back.
    assert_eq!(
        literals,
        [
            String::from("cere") + "mony",
            String::from("status") + "-writing"
        ]
    );
    let scan_roots: Vec<String> = terminology
        .consumers
        .scan_roots
        .iter()
        .map(|root| root.to_scalar_string().unwrap())
        .collect();
    assert_eq!(scan_roots, ["src", "tests", "docs"]);
    assert!(
        terminology
            .consumers
            .exemptions
            .iter()
            .any(|exemption| exemption.to_scalar_string().unwrap() == "docs/adr/")
    );
}

#[test]
fn the_seeded_paths_record_scopes_to_the_cli_tree() {
    let root = repo_root();
    let records = load_contract_registry(&root).expect("seeded registry loads");
    let paths = records
        .iter()
        .find(|record| {
            record.id.to_scalar_string().unwrap() == "paths.legacy-home-literals-cli-tree"
        })
        .expect("paths record");
    let literals: Vec<String> = paths
        .anchor
        .literals
        .iter()
        .map(|literal| literal.to_scalar_string().unwrap())
        .collect();
    assert_eq!(literals, ["~/.kittify", "~/.spec-kitty"]);
    let scan_roots: Vec<String> = paths
        .consumers
        .scan_roots
        .iter()
        .map(|root| root.to_scalar_string().unwrap())
        .collect();
    assert_eq!(scan_roots, ["src/specify_cli/cli"]);
    assert!(paths.consumers.exemptions.is_empty());
}

#[test]
fn the_seeded_shim_registry_is_empty_and_uses_the_project_version() {
    let root = repo_root();
    let report = check_shim_registry(&root).expect("seeded registry validates");
    assert!(report.entries.is_empty());
    assert!(!report.has_overdue());
    assert_eq!(report.recommended_exit_code(), 0);
    assert_eq!(report.registry_path, resolve_shim_registry_path(&root));
    // The reported version is the authored one, not an invented constant.
    let authored = read_project_version(&root).expect("pyproject version");
    assert_eq!(report.project_version, authored);
    assert!(Version::parse(&authored).is_ok(), "{authored} is PEP 440");
}

// ---------------------------------------------------------------------------
// Status classification (Source's four states and their precedence)
// ---------------------------------------------------------------------------

#[test]
fn a_pending_shim_exits_zero() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    create_shim_file(root, "specify_cli.example");
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.2.0",
                "3.3.0",
                "#615",
                false
            )
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    assert_eq!(report.entries[0].status, ShimStatus::Pending);
    assert!(report.entries[0].shim_exists);
    assert_eq!(report.recommended_exit_code(), 0);
}

#[test]
fn an_overdue_shim_exits_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    create_shim_file(root, "specify_cli.example");
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.1.0",
                "3.2.0",
                "#615",
                false
            )
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    assert_eq!(report.entries[0].status, ShimStatus::Overdue);
    assert!(report.has_overdue());
    assert_eq!(report.recommended_exit_code(), 1);
}

#[test]
fn a_grandfathered_shim_wins_over_an_existing_file_past_its_release() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    create_shim_file(root, "specify_cli.example");
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.0.0",
                "3.1.0",
                "#615",
                true
            )
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    assert_eq!(report.entries[0].status, ShimStatus::Grandfathered);
    assert_eq!(report.recommended_exit_code(), 0);
}

#[test]
fn a_missing_shim_module_is_removed_even_past_its_release() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.0.0",
                "3.1.0",
                "#615",
                false
            )
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    assert_eq!(report.entries[0].status, ShimStatus::Removed);
    assert!(!report.entries[0].shim_exists);
    assert_eq!(report.recommended_exit_code(), 0);
}

#[test]
fn grandfathering_is_checked_before_the_file_probe() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.0.0",
                "3.1.0",
                "#615",
                true
            )
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    assert_eq!(report.entries[0].status, ShimStatus::Grandfathered);
    assert!(!report.entries[0].shim_exists);
}

#[test]
fn the_classifier_is_pure_and_covers_each_arm() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.2.0",
                "3.3.0",
                "#615",
                false
            )
        ),
    );
    let entry = load_shim_registry(root).expect("valid registry").remove(0);
    let current = Version::parse("3.2.0").expect("version");
    assert_eq!(classify(&entry, &current, true), ShimStatus::Pending);
    assert_eq!(classify(&entry, &current, false), ShimStatus::Removed);
    let later = Version::parse("3.3.0").expect("version");
    assert_eq!(classify(&entry, &later, true), ShimStatus::Overdue);
}

#[test]
fn shim_exists_probes_both_package_forms() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    assert!(!shim_exists(root, &text("specify_cli.example")));
    write(&root.join("src/specify_cli/example.py"), "x = 1\n");
    assert!(shim_exists(root, &text("specify_cli.example")));
    assert!(!shim_exists(root, &text("specify_cli.example.inner")));
    write(&root.join("src/specify_cli/example/inner/__init__.py"), "");
    assert!(shim_exists(root, &text("specify_cli.example.inner")));
}

#[test]
fn a_canonical_import_list_is_preserved_in_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: specify_cli.old\n    canonical_import: [\"mod.a\", \"mod.b\"]\n    \
         introduced_in_release: \"3.2.0\"\n    removal_target_release: \"3.3.0\"\n    \
         tracker_issue: \"https://github.com/o/r/issues/1\"\n    grandfathered: false\n",
    );
    let entries = load_shim_registry(root).expect("valid registry");
    match &entries[0].canonical_import {
        CanonicalImport::Many(imports) => {
            let imports: Vec<String> = imports
                .iter()
                .map(|import| import.to_scalar_string().unwrap())
                .collect();
            assert_eq!(imports, ["mod.a", "mod.b"]);
        }
        other => panic!("expected a list, got {other:?}"),
    }
    assert_eq!(entries[0].canonical_import.joined(), "mod.a, mod.b");
}

// ---------------------------------------------------------------------------
// Shim registry configuration and schema failures (exit 2 in the adapter)
// ---------------------------------------------------------------------------

fn shim_failure(root: &Path) -> RegistryDiagnosticsError {
    check_shim_registry(root).expect_err("must be refused")
}

#[test]
fn a_missing_registry_names_the_canonical_home() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(&root.join("pyproject.toml"), &project("3.2.0"));
    let error = shim_failure(root);
    assert!(matches!(
        &error,
        RegistryDiagnosticsError::NotFound {
            label: "Shim registry",
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "Shim registry not found at {}",
            root.join(SHIM_REGISTRY_REL).display()
        )
    );
    assert_eq!(error.code(), "registry_not_found");
}

#[test]
fn a_missing_pyproject_is_read_before_the_registry() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let error = shim_failure(root);
    assert!(matches!(
        &error,
        RegistryDiagnosticsError::PyprojectNotFound { .. }
    ));
    assert!(
        error
            .to_string()
            .starts_with("pyproject.toml not found at ")
    );
}

#[test]
fn a_missing_project_key_keeps_the_key_error_shape() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(&root.join("pyproject.toml"), "[tool.other]\nx = 1\n");
    let error = shim_failure(root);
    assert_eq!(
        error,
        RegistryDiagnosticsError::ProjectKeyMissing { key: "project" }
    );
    assert_eq!(error.to_string(), "missing key 'project' in pyproject.toml");

    write(&root.join("pyproject.toml"), "[project]\nname = \"x\"\n");
    let error = shim_failure(root);
    assert_eq!(
        error,
        RegistryDiagnosticsError::ProjectKeyMissing { key: "version" }
    );
    assert_eq!(error.to_string(), "missing key 'version' in pyproject.toml");
}

#[test]
fn schema_violations_are_collected_not_hidden() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: 123\n    canonical_import: null\n    \
         introduced_in_release: true\n    removal_target_release: []\n    \
         tracker_issue: 0\n    grandfathered: maybe\n",
    );
    let errors = shim_errors(root);
    assert!(errors.len() >= 5, "{errors:?}");
    for expected in [
        "entry[0].legacy_path: must be a dotted identifier string",
        "entry[0].canonical_import: must be a string or list of strings",
        "entry[0].introduced_in_release: must be a semver string like '1.2.3' or '1.2.3a1'",
        "entry[0].tracker_issue: must be '#123' or a URL",
        "entry[0].grandfathered: must be a boolean (true/false), got str",
    ] {
        assert!(
            errors.iter().any(|message| message == expected),
            "{expected}"
        );
    }
}

#[test]
fn unknown_and_missing_keys_use_source_wording() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: a.b\n    canonical_import: c\n    \
         introduced_in_release: \"1.0.0\"\n    removal_target_release: \"2.0.0\"\n    \
         tracker_issue: \"#1\"\n    grandfathered: false\n    future_field: keep\n",
    );
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0].future_field: unknown field"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: a.b\n    canonical_import: c\n    \
         introduced_in_release: \"1.0.0\"\n    removal_target_release: \"2.0.0\"\n    \
         tracker_issue: \"#1\"\n",
    );
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0].grandfathered: required field is missing"])
    );
}

#[test]
fn duplicate_legacy_paths_and_bad_version_orders_are_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let entry = shim_entry("a.b", "c", "1.0.0", "2.0.0", "#1", false);
    shim_project(root, "3.2.0", &format!("shims:\n{entry}{entry}"));
    assert_eq!(
        shim_errors(root),
        owned(&["entry[1].legacy_path: duplicate value 'a.b'"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let entry = shim_entry("a.b", "c", "3.3.0", "3.2.0", "#1", false);
    shim_project(root, "3.2.0", &format!("shims:\n{entry}"));
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0].removal_target_release: must be >= introduced_in_release"])
    );
}

#[test]
fn a_semver_shaped_but_invalid_pep440_release_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let entry = shim_entry("a.b", "c", "1.2.3z1", "1.2.3z2", "#1", false);
    shim_project(root, "3.2.0", &format!("shims:\n{entry}"));
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0]: version strings are not valid semver"])
    );
}

#[test]
fn canonical_import_list_items_and_blank_optionals_are_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: a.b\n    canonical_import: [\"valid.module\", \"123-bad\"]\n    \
         introduced_in_release: \"1.0.0\"\n    removal_target_release: \"2.0.0\"\n    \
         tracker_issue: \"#1\"\n    grandfathered: false\n",
    );
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0].canonical_import[1]: must be a dotted identifier string"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: a.b\n    canonical_import: c\n    \
         introduced_in_release: \"1.0.0\"\n    removal_target_release: \"2.0.0\"\n    \
         tracker_issue: \"#1\"\n    grandfathered: false\n    extension_rationale: \"   \"\n",
    );
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0].extension_rationale: if present, must be a non-empty string"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(
        root,
        "3.2.0",
        "shims:\n  - legacy_path: a.b\n    canonical_import: c\n    \
         introduced_in_release: \"1.0.0\"\n    removal_target_release: \"2.0.0\"\n    \
         tracker_issue: \"#1\"\n    grandfathered: false\n    notes: 42\n",
    );
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0].notes: if present, must be a string"])
    );
}

#[test]
fn container_shape_failures_use_source_wording() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "");
    assert_eq!(
        shim_errors(root),
        owned(&["top-level: must be a mapping with a 'shims' key"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "other_key: []\n");
    assert_eq!(
        shim_errors(root),
        owned(&["top-level: must be a mapping with a 'shims' key"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "shims: not-a-list\n");
    assert_eq!(
        shim_errors(root),
        owned(&["top-level.shims: must be a list"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "shims:\n  - 42\n  - [\"a\"]\n");
    assert_eq!(
        shim_errors(root),
        owned(&["entry[0]: must be a mapping", "entry[1]: must be a mapping"])
    );
}

#[test]
fn a_yaml_parse_error_is_a_schema_failure_with_the_source_prefix() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    shim_project(root, "3.2.0", "shims: [\n  {legacy_path: foo\n");
    let errors = shim_errors(root);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].starts_with("YAML parse error: "), "{}", errors[0]);
}

// ---------------------------------------------------------------------------
// Contract registry schema (structural gate + DIR-041)
// ---------------------------------------------------------------------------

#[test]
fn a_well_formed_contract_record_validates_and_projects() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(root, WELL_FORMED_CONTRACT);
    let report = check_contract_registry(root).expect("valid registry");
    assert_eq!(report.record_count(), 1);
    assert_eq!(report.advisory_count(), 1);
    assert_eq!(report.enforcing_count(), 0);
    let record = &report.records[0];
    assert_eq!(
        record.id.to_scalar_string().unwrap(),
        "test.synthetic-literal"
    );
    assert_eq!(
        record.anchor.literals[0].to_scalar_string().unwrap(),
        "synthetic_retired_token"
    );
    assert_eq!(
        record.retirement.introduced_in.to_scalar_string().unwrap(),
        "3.0.0"
    );
    assert_eq!(record.consumers.scan_roots.len(), 1);
    assert!(record.consumers.exemptions.is_empty());
    assert!(record.consumers.test_shards.is_empty());
}

#[test]
fn a_positional_file_line_field_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace(
            "      literals:\n        - value: synthetic_retired_token",
            "      file: src/specify_cli/foo.py\n      line: 42",
        ),
    );
    let errors = contract_errors(root);
    let joined = errors.join("\n");
    assert!(joined.contains("file:line"), "{joined}");
    assert!(joined.contains("NFR-003/DIR-041"), "{joined}");
    assert!(joined.contains("contracts[0].anchor.file"), "{joined}");
    assert!(joined.contains("contracts[0].anchor.line"), "{joined}");
}

#[test]
fn a_string_shaped_like_file_line_is_rejected() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace("synthetic_retired_token", "src/specify_cli/foo.py:42"),
    );
    let joined = contract_errors(root).join("\n");
    assert!(
        joined.contains("is a positional 'file:line' anchor"),
        "{joined}"
    );
    assert!(
        joined.contains("contracts[0].anchor.literals[0].value"),
        "{joined}"
    );
}

#[test]
fn fragments_may_not_reconstruct_a_positional_anchor() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace(
            "        - value: synthetic_retired_token",
            "        - fragments: [\"src/foo.py\", \":42\"]",
        ),
    );
    let joined = contract_errors(root).join("\n");
    assert!(
        joined.contains("fragments must not reconstruct a line number"),
        "{joined}"
    );
}

#[test]
fn a_missing_required_field_is_named() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let without_retirement: Vec<&str> = WELL_FORMED_CONTRACT
        .lines()
        .filter(|line| {
            !(line.starts_with("    retirement:")
                || line.starts_with("      introduced_in:")
                || line.starts_with("      removal_target:")
                || line.contains("tracker_issue: \"#2077\""))
        })
        .collect();
    contract_project(root, &format!("{}\n", without_retirement.join("\n")));
    let errors = contract_errors(root);
    assert!(
        errors
            .iter()
            .any(|message| message == "contracts[0].retirement: required field is missing"),
        "{errors:?}"
    );
}

#[test]
fn an_out_of_range_kind_names_every_allowed_value() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace("kind: retired_literal", "kind: not_a_real_kind"),
    );
    assert_eq!(
        contract_errors(root),
        owned(&["contracts[0].kind: must be one of ['fallback_name', 'retired_literal']"])
    );
}

#[test]
fn a_retired_literal_without_literals_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace(
            "      literals:\n        - value: synthetic_retired_token",
            "      literals: []",
        ),
    );
    assert_eq!(
        contract_errors(root),
        owned(&[
            "contracts[0].anchor.literals: must be a non-empty list for kind 'retired_literal'"
        ])
    );
}

#[test]
fn each_anchor_kind_uses_only_its_own_field() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT
            .replace("kind: retired_literal", "kind: fallback_name")
            .replace(
                "      literals:\n        - value: synthetic_retired_token",
                "      symbol: specify_cli.legacy.helper\n      literals:\n        - value: x",
            ),
    );
    assert_eq!(
        contract_errors(root),
        owned(&["contracts[0].anchor: kind 'fallback_name' uses 'symbol', not 'literals'"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace(
            "      literals:\n        - value: synthetic_retired_token",
            "      literals:\n        - value: x\n      symbol: a.b",
        ),
    );
    assert_eq!(
        contract_errors(root),
        owned(&["contracts[0].anchor: kind 'retired_literal' uses 'literals', not 'symbol'"])
    );
}

#[test]
fn a_fallback_name_symbol_record_is_valid() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT
            .replace("kind: retired_literal", "kind: fallback_name")
            .replace(
                "      literals:\n        - value: synthetic_retired_token",
                "      symbol: specify_cli.legacy.helper",
            )
            .replace("status: retired", "status: active")
            .replace("enforcement: advisory", "enforcement: enforcing"),
    );
    let report = check_contract_registry(root).expect("valid registry");
    assert_eq!(report.enforcing_count(), 1);
    assert_eq!(report.advisory_count(), 0);
    assert_eq!(
        report.records[0]
            .anchor
            .symbol
            .as_ref()
            .unwrap()
            .to_scalar_string()
            .unwrap(),
        "specify_cli.legacy.helper"
    );
    assert!(report.records[0].anchor.literals.is_empty());
}

#[test]
fn duplicate_ids_and_consumer_shape_failures_are_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let first = WELL_FORMED_CONTRACT.trim_end().to_owned();
    let second = &first[first.find("  - id:").expect("record start")..];
    contract_project(root, &format!("{first}\n{second}\n"));
    let errors = contract_errors(root);
    assert!(
        errors
            .iter()
            .any(|message| message == "contracts[1].id: duplicate value 'test.synthetic-literal'"),
        "{errors:?}"
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace(
            "      scan_roots: [\"src\"]\n      exemptions: []",
            "      exemptions: []\n      extra_field: 1",
        ),
    );
    let errors = contract_errors(root);
    assert!(
        errors
            .iter()
            .any(|message| message == "contracts[0].consumers.extra_field: unknown field"),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(
            |message| message == "contracts[0].consumers.scan_roots: required field is missing"
        ),
        "{errors:?}"
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT.replace("scan_roots: [\"src\"]", "scan_roots: []"),
    );
    assert_eq!(
        contract_errors(root),
        owned(&["contracts[0].consumers.scan_roots: must not be empty"])
    );
}

#[test]
fn enforcement_and_status_are_range_checked() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(
        root,
        &WELL_FORMED_CONTRACT
            .replace("enforcement: advisory", "enforcement: blocking")
            .replace("status: retired", "status: obsolete"),
    );
    let errors = contract_errors(root);
    assert!(
        errors.iter().any(|message| message
            == "contracts[0].verification.enforcement: must be one of ['advisory', 'enforcing']"),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|message| message
            == "contracts[0].status: must be one of ['active', 'deprecated', 'retired']"),
        "{errors:?}"
    );
    assert!(ALLOWED_ENFORCEMENT.contains(&"advisory"));
    assert!(ALLOWED_STATUSES.contains(&"deprecated"));

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(root, &format!("{WELL_FORMED_CONTRACT}    notes: 42\n"));
    assert_eq!(
        contract_errors(root),
        owned(&["contracts[0].notes: if present, must be a string"])
    );
}

#[test]
fn contract_container_shape_failures_use_source_wording() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(root, "");
    assert_eq!(
        contract_errors(root),
        owned(&["top-level: must be a mapping with a 'contracts' key"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(root, "contracts: 3\n");
    assert_eq!(
        contract_errors(root),
        owned(&["top-level.contracts: must be a list"])
    );

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(root, "contracts:\n  - \"not a record\"\n");
    assert_eq!(
        contract_errors(root),
        owned(&["contracts[0]: must be a mapping"])
    );
}

#[test]
fn a_missing_contract_registry_is_a_configuration_error() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    let error = check_contract_registry(root).expect_err("must be refused");
    assert!(matches!(
        &error,
        RegistryDiagnosticsError::NotFound {
            label: "Contract registry",
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "Contract registry not found at {}",
            root.join("docs/contracts/contract-registry.yaml").display()
        )
    );
}

#[test]
fn a_document_that_is_not_a_mapping_is_refused_by_the_typed_entry_point() {
    let document = sk_kernel::python_yaml::decode_graph(
        "- just\n- a list\n",
        sk_kernel::python_yaml::Dialect::RuamelSafe12,
    )
    .expect("decodes");
    let document = sk_kernel::python_yaml::GraphDocument::new(document);
    let error = validate_contract_registry(&document).expect_err("refused");
    assert_eq!(
        error.schema_errors().expect("schema"),
        ["top-level: must be a mapping with a 'contracts' key"]
    );
}

// ---------------------------------------------------------------------------
// The exit-code contract of the two diagnostics
// ---------------------------------------------------------------------------

#[test]
fn only_overdue_shims_recommend_a_non_zero_exit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    create_shim_file(root, "a.one");
    create_shim_file(root, "a.two");
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}{}",
            shim_entry("a.one", "b.one", "3.2.0", "3.3.0", "#1", false),
            shim_entry("a.two", "b.two", "3.0.0", "3.1.0", "#2", true),
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    assert_eq!(report.recommended_exit_code(), 0);
    assert_eq!(report.entries[0].status, ShimStatus::Pending);
    assert_eq!(report.entries[1].status, ShimStatus::Grandfathered);
}

#[test]
fn the_json_documents_carry_source_keys_and_the_observed_values() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    create_shim_file(root, "specify_cli.example");
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.1.0",
                "3.2.0",
                "#615",
                false
            )
        ),
    );
    let report = check_shim_registry(root).expect("valid registry");
    let document = report.to_json().expect("json");
    assert_eq!(document["project_version"].as_str(), Some("3.2.0"));
    assert_eq!(
        document["registry_path"].as_str(),
        root.join(SHIM_REGISTRY_REL).to_str()
    );
    assert_eq!(document["has_overdue"].as_bool(), Some(true));
    assert_eq!(document["exit_code"].as_u64(), Some(1));
    let entry = &document["entries"][0];
    assert_eq!(entry["legacy_path"].as_str(), Some("specify_cli.example"));
    assert_eq!(entry["canonical_import"].as_str(), Some("example"));
    assert_eq!(entry["removal_target_release"].as_str(), Some("3.2.0"));
    assert_eq!(entry["grandfathered"].as_bool(), Some(false));
    assert_eq!(entry["tracker_issue"].as_str(), Some("#615"));
    assert_eq!(entry["status"].as_str(), Some("overdue"));
    assert_eq!(entry["shim_exists"].as_bool(), Some(true));

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    contract_project(root, WELL_FORMED_CONTRACT);
    let report = check_contract_registry(root).expect("valid registry");
    let document = report.to_json().expect("json");
    assert_eq!(document["record_count"].as_u64(), Some(1));
    assert_eq!(document["advisory_count"].as_u64(), Some(1));
    assert_eq!(document["enforcing_count"].as_u64(), Some(0));
    assert_eq!(document["exit_code"].as_u64(), Some(0));
    let record = &document["records"][0];
    assert_eq!(record["id"].as_str(), Some("test.synthetic-literal"));
    assert_eq!(record["kind"].as_str(), Some("retired_literal"));
    assert_eq!(record["status"].as_str(), Some("retired"));
    assert_eq!(record["owner"].as_str(), Some("#2441"));
    assert_eq!(
        record["replaced_by"].as_str(),
        Some("specify_cli.something.canonical")
    );
    assert_eq!(record["enforcement"].as_str(), Some("advisory"));
    assert_eq!(record["scan_roots"][0].as_str(), Some("src"));
}

#[test]
fn the_positional_anchor_detector_is_shared_and_correct() {
    for anchor in [
        "src/specify_cli/foo.py:42",
        "tests/architectural/_ratchet_keys.py:229",
        "docs/status-model.md:7",
    ] {
        assert!(is_file_line_anchor(&text(anchor)), "{anchor}");
    }
    for content in [
        "~/.kittify",
        "~/.spec-kitty",
        "specify_cli.status.emit.emit_status_transition",
        "3.4.0",
        "#2077",
        "status commit",
    ] {
        assert!(!is_file_line_anchor(&text(content)), "{content}");
    }
}

// ---------------------------------------------------------------------------
// Read-only proof
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn footprint(root: &Path) -> Vec<(PathBuf, u64, u32, i64, i64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let mut rows = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).expect("read fixture dir");
        for entry in entries {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            rows.push((
                path.clone(),
                metadata.len(),
                metadata.mode(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ino(),
            ));
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    rows.sort();
    rows
}

#[cfg(unix)]
#[test]
fn the_diagnostics_leave_the_project_byte_and_metadata_identical() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    create_shim_file(root, "specify_cli.example");
    shim_project(
        root,
        "3.2.0",
        &format!(
            "shims:\n{}",
            shim_entry(
                "specify_cli.example",
                "example",
                "3.1.0",
                "3.2.0",
                "#615",
                false
            )
        ),
    );
    contract_project(root, WELL_FORMED_CONTRACT);

    let before = footprint(root);
    let shim_report = check_shim_registry(root).expect("valid registry");
    let contract_report = check_contract_registry(root).expect("valid registry");
    let _ = shim_report.to_json().expect("json");
    let _ = contract_report.to_json().expect("json");
    let after = footprint(root);
    assert_eq!(before, after, "diagnostics must not touch the project tree");

    // A refused schema is equally read-only.
    let broken = tempfile::tempdir().expect("tempdir");
    let root = broken.path();
    shim_project(root, "3.2.0", "shims: not-a-list\n");
    contract_project(root, "contracts: 3\n");
    let before = footprint(root);
    assert!(check_shim_registry(root).is_err());
    assert!(check_contract_registry(root).is_err());
    assert_eq!(before, footprint(root));
}

#[cfg(unix)]
#[test]
fn writes_are_visible_in_the_footprint_so_the_proof_is_not_vacuous() {
    // Positive control for the read-only assertion above: a real write must
    // change the footprint, otherwise that assertion cannot fail.
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    write(&root.join("fixture.txt"), "one\n");
    let before = footprint(root);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(root.join("fixture.txt"))
        .expect("open");
    file.write_all(b"two\n").expect("append");
    drop(file);
    assert_ne!(before, footprint(root));

    let created = tempfile::tempdir().expect("created");
    let root = created.path();
    write(&root.join("fixture.txt"), "one\n");
    let before = footprint(root);
    write(&root.join("new.txt"), "x\n");
    assert_ne!(before, footprint(root));
}

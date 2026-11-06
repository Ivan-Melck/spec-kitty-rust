//! CF72 — project-governance resolution and compact-summary preparation.
//!
//! In-process Native controls over fresh fixtures. The paired Source/Native
//! child comparison lives in `crates/sk-cli/tests/cf72_compact_governance_cli.rs`;
//! this file pins the policy branches with injected catalogue facts where a
//! domain cannot be absent from the shipped packs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_doctrine::{
    Diagnostic, DiagnosticCode, GovernanceCatalogue, ProjectGovernanceError,
    ProjectGovernanceOptions, compact_governance_summary, resolve_project_governance,
    resolve_project_governance_with,
};

fn fixture(case: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/cf72-project-governance");
    std::fs::create_dir_all(&base).expect("fixture base");
    let path = tempfile::Builder::new()
        .prefix(&format!("{case}-"))
        .tempdir_in(&base)
        .expect("fresh fixture")
        .keep();
    std::fs::create_dir_all(path.join(".kittify/charter")).expect("kittify dirs");
    std::fs::write(
        path.join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    )
    .expect("config");
    let status = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&path)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed");
    path
}

fn write_charter(root: &Path, governance: &str, directives: &str) {
    let mut body = String::from("schema_version: \"1.1.0\"\n");
    if !governance.is_empty() {
        body.push_str("governance:\n  charter:\n");
        body.push_str(governance);
    }
    if !directives.is_empty() {
        body.push_str("directives:\n  directives:\n");
        body.push_str(directives);
    }
    std::fs::write(root.join(".kittify/charter/charter.yaml"), body).expect("charter");
}

fn write_activation(root: &Path, body: &str) {
    std::fs::write(root.join(".kittify/charter/activation.yaml"), body).expect("activation");
}

fn set_of(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn assert_code(diagnostic: &Diagnostic, code: DiagnosticCode) {
    assert_eq!(diagnostic.code, code, "diagnostic code: {:?}", diagnostic);
}

#[test]
fn sparse_project_uses_the_catalog_defaults_and_fallbacks() {
    let root = fixture("sparse");
    let resolution = resolve_project_governance(&root).expect("sparse resolution");

    assert_eq!(resolution.template_set, "software-dev-default");
    assert_eq!(resolution.tools, vec!["git", "spec-kitty"]);
    assert_eq!(
        resolution.metadata_value("tools_source"),
        Some("registry_only")
    );
    assert_eq!(
        resolution.metadata_value("directives_source"),
        Some("catalog_fallback")
    );
    assert_eq!(
        resolution.metadata_value("template_set_source"),
        Some("fallback")
    );
    let ordered: Vec<&str> = resolution
        .metadata
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(
        ordered,
        vec!["tools_source", "directives_source", "template_set_source"]
    );
    assert!(
        resolution
            .paradigms
            .contains(&"domain-driven-design".to_owned())
    );
    assert!(resolution.directives.contains(&"DIRECTIVE_010".to_owned()));
    assert!(resolution.tactics.is_empty());
    assert!(resolution.assets.is_empty());
    assert_eq!(resolution.profile_id, None);
    assert_eq!(resolution.role, None);

    let codes: Vec<DiagnosticCode> = resolution
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec![
            DiagnosticCode::CatalogDefaultApplied,
            DiagnosticCode::FallbackApplied,
            DiagnosticCode::CatalogDefaultApplied,
            DiagnosticCode::FallbackApplied,
        ]
    );
}

#[test]
fn explicit_empty_activation_is_not_the_absent_catalog_default() {
    let root = fixture("empty");
    write_charter(&root, "    {}\n", "");
    write_activation(&root, "activated_paradigms: []\nactivated_directives: []\n");
    let resolution = resolve_project_governance(&root).expect("empty activation");

    assert!(resolution.paradigms.is_empty());
    assert!(resolution.directives.is_empty());
    assert_eq!(
        resolution.metadata_value("directives_source"),
        Some("activation")
    );
    // No CATALOG_DEFAULT_APPLIED for either kind: an explicit empty list is an
    // opt-out, not an absent key.
    assert!(
        resolution
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != DiagnosticCode::CatalogDefaultApplied)
    );
    assert_eq!(
        resolution
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>(),
        vec![
            DiagnosticCode::FallbackApplied,
            DiagnosticCode::FallbackApplied
        ]
    );
}

#[test]
fn charter_selections_union_first_occurrence_and_drop_unknowns() {
    let root = fixture("selected");
    write_charter(
        &root,
        "    selected_paradigms: [domain-driven-design, deep-module-design, no-such-paradigm]\n\
         \x20   selected_directives: [DIRECTIVE_010, 010-specification-fidelity-requirement, no-such-directive]\n\
         \x20   available_tools: [pytest, mypy, ruff]\n\
         \x20   template_set: software-dev-default\n",
        "    - id: PROJ_001\n      title: Project local rule\n",
    );
    write_activation(&root, "activated_directives: [DIRECTIVE_024]\n");

    let resolution = resolve_project_governance(&root).expect("selected resolution");
    assert_eq!(resolution.template_set, "software-dev-default");
    assert_eq!(
        resolution.tools,
        vec!["git", "mypy", "pytest", "ruff", "spec-kitty"]
    );
    assert_eq!(
        resolution.metadata_value("tools_source"),
        Some("charter+registry")
    );
    assert_eq!(
        resolution.metadata_value("directives_source"),
        Some("activation+charter+project_local")
    );
    assert_eq!(
        resolution.metadata_value("template_set_source"),
        Some("charter")
    );
    assert_eq!(
        resolution.directives,
        vec!["DIRECTIVE_024", "DIRECTIVE_010", "PROJ_001"]
    );

    let codes: Vec<DiagnosticCode> = resolution
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec![
            DiagnosticCode::UnknownSelectionDropped,
            DiagnosticCode::CatalogDefaultApplied,
            DiagnosticCode::SelectionMerged,
            DiagnosticCode::SelectionMerged,
            DiagnosticCode::UnknownSelectionDropped,
            DiagnosticCode::SelectionMerged,
            DiagnosticCode::SelectionMerged,
        ]
    );
    let unknown_paradigm = &resolution.diagnostics[0];
    assert_code(unknown_paradigm, DiagnosticCode::UnknownSelectionDropped);
    assert!(unknown_paradigm.message.contains("no-such-paradigm"));
    let unknown_directive = &resolution.diagnostics[4];
    assert_code(unknown_directive, DiagnosticCode::UnknownSelectionDropped);
    assert!(unknown_directive.message.contains("no-such-directive"));
}

#[test]
fn tools_distinguish_absent_default_empty_registry_and_custom_registry() {
    let root = fixture("tools");
    write_charter(&root, "    available_tools: [pytest, mypy, ruff]\n", "");

    let absent = resolve_project_governance(&root).expect("absent registry");
    assert_eq!(
        absent.tools,
        vec!["git", "mypy", "pytest", "ruff", "spec-kitty"]
    );

    let empty = resolve_project_governance_with(
        &root,
        &ProjectGovernanceOptions {
            tool_registry: Some(BTreeSet::new()),
            ..ProjectGovernanceOptions::default()
        },
    )
    .expect("empty registry falls back to default");
    assert_eq!(
        empty.tools,
        vec!["git", "mypy", "pytest", "ruff", "spec-kitty"]
    );

    let custom = resolve_project_governance_with(
        &root,
        &ProjectGovernanceOptions {
            tool_registry: Some(set_of(&["make"])),
            ..ProjectGovernanceOptions::default()
        },
    )
    .expect("custom registry");
    assert_eq!(custom.tools, vec!["make", "mypy", "pytest", "ruff"]);
    assert_eq!(
        custom.metadata_value("tools_source"),
        Some("charter+registry")
    );
}

#[test]
fn catalogue_domain_absence_is_not_an_explicitly_empty_domain() {
    let root = fixture("domains");
    write_charter(
        &root,
        "    selected_paradigms: [domain-driven-design]\n\
         \x20   selected_directives: [DIRECTIVE_010]\n\
         \x20   template_set: foo-default\n",
        "",
    );

    let empty_built_in = root.join("empty-built-in");
    std::fs::create_dir_all(&empty_built_in).expect("empty built-in");
    let empty_identity = sk_doctrine::IdentityIndex::from_roots(&empty_built_in, &[], None);
    let empty_domain = GovernanceCatalogue::new(
        BTreeSet::new(),
        BTreeSet::new(),
        set_of(&["foo-default"]),
        set_of(&["paradigms", "directives", "template_sets"]),
    );
    let resolution = resolve_project_governance_with(
        &root,
        &ProjectGovernanceOptions {
            catalogue: Some(empty_domain),
            identity: Some(empty_identity.clone()),
            ..ProjectGovernanceOptions::default()
        },
    )
    .expect("present-but-empty domain drops the selection");
    assert!(resolution.paradigms.is_empty());
    assert!(resolution.directives.is_empty());
    assert_eq!(resolution.template_set, "foo-default");
    assert_eq!(
        resolution.metadata_value("template_set_source"),
        Some("charter")
    );
    let codes: Vec<DiagnosticCode> = resolution
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec![
            DiagnosticCode::UnknownSelectionDropped,
            DiagnosticCode::CatalogDefaultApplied,
            DiagnosticCode::FallbackApplied,
            DiagnosticCode::CatalogDefaultApplied,
            DiagnosticCode::UnknownSelectionDropped,
        ]
    );

    let missing_domain = GovernanceCatalogue::new(
        BTreeSet::new(),
        BTreeSet::new(),
        set_of(&["foo-default"]),
        set_of(&["template_sets"]),
    );
    let resolution = resolve_project_governance_with(
        &root,
        &ProjectGovernanceOptions {
            catalogue: Some(missing_domain),
            identity: Some(empty_identity),
            ..ProjectGovernanceOptions::default()
        },
    )
    .expect("absent domain permits the unvalidated paradigm fallback");
    assert_eq!(resolution.paradigms, vec!["domain-driven-design"]);
    // Directives still validate: the reference's domain presence bypass is
    // paradigm-only.
    assert!(resolution.directives.is_empty());
}

#[test]
fn template_selection_validates_only_a_present_domain_and_reports_ordered_issues() {
    let root = fixture("template");
    write_charter(&root, "    template_set: no-such-set\n", "");
    let catalogue = GovernanceCatalogue::new(
        BTreeSet::new(),
        BTreeSet::new(),
        set_of(&["software-dev-default"]),
        set_of(&["paradigms", "directives", "template_sets"]),
    );
    let error = resolve_project_governance_with(
        &root,
        &ProjectGovernanceOptions {
            catalogue: Some(catalogue),
            ..ProjectGovernanceOptions::default()
        },
    )
    .expect_err("unavailable template set refuses");
    let ProjectGovernanceError::Resolution(resolution) = &error else {
        panic!("expected GovernanceResolutionError, got {error:?}");
    };
    assert_eq!(
        resolution.issues(),
        &[
            "Charter selected unavailable template_set: 'no-such-set'".to_owned(),
            "Available template sets: software-dev-default".to_owned(),
            "Update charter template_set to one of the available template sets, or provide it through a project override (see 'spec-kitty charter list --show-available').".to_owned(),
        ]
    );
    assert!(
        error
            .to_string()
            .starts_with("Governance resolution failed:\n- ")
    );

    let summary = compact_governance_summary(&root).expect("catalogue read from real packs");
    assert_eq!(summary.template_set, None);
    assert!(summary.diagnostics.len() == 1);
    assert!(
        summary.diagnostics[0].contains("governance unresolved (Governance resolution failed:")
    );
}

#[test]
fn deny_applies_after_every_union_and_keeps_the_producer_order() {
    let root = fixture("deny");
    write_charter(
        &root,
        "    selected_directives: [DIRECTIVE_010]\n\
         \x20   selected_paradigms: [domain-driven-design]\n",
        "    - id: PROJ_001\n      title: Project local rule\n",
    );
    write_activation(
        &root,
        "activated_directives: [DIRECTIVE_010, DIRECTIVE_024]\n\
         deactivated_directives: [DIRECTIVE_010]\n\
         activated_paradigms: [domain-driven-design]\n\
         deactivated_paradigms: [domain-driven-design]\n",
    );
    let resolution = resolve_project_governance(&root).expect("deny resolution");
    assert!(resolution.paradigms.is_empty());
    assert_eq!(resolution.directives, vec!["DIRECTIVE_024", "PROJ_001"]);
    assert_eq!(
        resolution.metadata_value("directives_source"),
        Some("activation+charter+project_local")
    );
    let codes: Vec<DiagnosticCode> = resolution
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert_eq!(
        codes,
        vec![
            DiagnosticCode::SelectionMerged,
            DiagnosticCode::FallbackApplied,
            DiagnosticCode::SelectionMerged,
            DiagnosticCode::SelectionMerged,
            DiagnosticCode::FallbackApplied,
        ]
    );
    // The activation base is deny-netted before the selection comparison, so
    // the selected-but-denied paradigm is "beyond the activation base".
    assert!(
        resolution.diagnostics[0]
            .message
            .starts_with("Charter selected 1 paradigm(s) beyond the activation base")
    );
}

#[test]
fn retired_fields_escape_the_summary_while_ordinary_errors_degrade() {
    let root = fixture("retired");
    std::fs::write(
        root.join(".kittify/charter/charter.yaml"),
        "schema_version: \"1.1.0\"\n\
         governance:\n  activations:\n    - activation_context: {}\n      doctrine_pack_id: built-in\n      artifact_id: x\n      artifact_kind: directives\n",
    )
    .expect("retired charter");
    let error = resolve_project_governance(&root).expect_err("retired activation field");
    let ProjectGovernanceError::RetiredPackField { field, .. } = &error else {
        panic!("expected RetiredPackField, got {error:?}");
    };
    assert_eq!(field, "doctrine_pack_id");
    let summary_error = compact_governance_summary(&root).expect_err("summary re-raises");
    assert!(matches!(
        summary_error,
        ProjectGovernanceError::RetiredPackField { .. }
    ));

    let doctrine = fixture("retired-doctrine");
    std::fs::write(
        doctrine.join(".kittify/charter/charter.yaml"),
        "schema_version: \"1.1.0\"\ngovernance:\n  doctrine:\n    charter:\n      selected_directives: [DIRECTIVE_010]\n",
    )
    .expect("retired doctrine");
    let error = resolve_project_governance(&doctrine).expect_err("retired selection key");
    assert!(matches!(
        error,
        ProjectGovernanceError::RetiredGovernanceSelection { .. }
    ));
    let summary = compact_governance_summary(&doctrine).expect("unavailable, not re-raised");
    assert_eq!(summary.template_set, None);
    assert!(summary.diagnostics[0].starts_with("governance unavailable ("));
    assert!(summary.diagnostics[0].contains("retired selection key"));
}

#[test]
fn explicit_read_root_is_distinct_from_the_activation_root() {
    let read_root = fixture("scope-read");
    write_charter(&read_root, "    selected_directives: [DIRECTIVE_010]\n", "");
    let activation_root = fixture("scope-activation");
    write_charter(
        &activation_root,
        "    selected_directives: [DIRECTIVE_003]\n",
        "",
    );
    write_activation(&activation_root, "activated_directives: [DIRECTIVE_024]\n");

    let resolution = resolve_project_governance_with(
        &activation_root,
        &ProjectGovernanceOptions {
            read_root: Some(read_root),
            ..ProjectGovernanceOptions::default()
        },
    )
    .expect("split roots");
    // The charter selection comes from the explicit read root; the activation
    // base comes from the activation repository root.
    assert_eq!(
        resolution.directives,
        vec!["DIRECTIVE_024", "DIRECTIVE_010"]
    );
    assert_eq!(
        resolution.metadata_value("directives_source"),
        Some("activation+charter")
    );
    assert!(resolution.directives.iter().all(|id| id != "DIRECTIVE_003"));
}

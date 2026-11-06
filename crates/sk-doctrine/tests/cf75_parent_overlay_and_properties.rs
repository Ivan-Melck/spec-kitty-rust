use sk_doctrine::{
    CanonicalOfferingService, GovernanceCharterSelection, OfferingContext, OfferingRoots,
    SelectionBlockInputs, governance_charter_selection, render_selection_block,
};
use sk_kernel::PythonString;
use std::{fs, process::Command};
fn text(s: &str) -> PythonString {
    PythonString::from_scalars(s)
}
fn overlay_root(doc: &str, required: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
    fs::create_dir_all(root.join(".kittify/charter")).unwrap();
    let org = root.join("org");
    fs::create_dir(&org).unwrap();
    fs::write(root.join(".kittify/config.yaml"),format!("charter_packs:\n  org:\n    packs:\n      - name: fixture-org\n        local_path: {}\n",org.display())).unwrap();
    fs::write(
        org.join("org-charter.yaml"),
        format!("required_directives: {required}\n"),
    )
    .unwrap();
    fs::write(root.join(".kittify/charter/charter.yaml"), doc).unwrap();
    t
}
#[test]
fn syntax_failure_defaults_then_still_applies_required_overlay() {
    let root = overlay_root("governance: [", "[DIRECTIVE_010]");
    assert_eq!(
        governance_charter_selection(root.path(), None)
            .unwrap()
            .selected_directives,
        vec![text("DIRECTIVE_010")]
    );
}
#[test]
fn schema_failure_defaults_then_still_applies_required_overlay() {
    let root = overlay_root("governance: invalid", "[DIRECTIVE_010]");
    assert_eq!(
        governance_charter_selection(root.path(), None)
            .unwrap()
            .selected_directives,
        vec![text("DIRECTIVE_010")]
    );
}
#[test]
fn required_overlay_uses_source_coercing_reader_not_strict_token_view() {
    let root = overlay_root("governance: {}", "[true, 123, ' DIRECTIVE_010 ']");
    assert_eq!(
        governance_charter_selection(root.path(), None)
            .unwrap()
            .selected_directives,
        vec![text("True"), text("123"), text("DIRECTIVE_010")]
    );
}
#[test]
fn denied_kind_validates_required_tokens_strictly() {
    let root = overlay_root("governance: {}", "[true]");
    fs::write(
        root.path().join(".kittify/charter/activation.yaml"),
        "deactivated_directives: [DIRECTIVE_010]\n",
    )
    .unwrap();
    assert!(
        governance_charter_selection(root.path(), None).is_err(),
        "Source strict deny floor rejects the mixed/nontext required field"
    );
}
#[test]
fn selected_styleguide_reads_properties_not_unrequested_any_dump() {
    let t = tempfile::tempdir().unwrap();
    let pack = t.path().join("built-in");
    fs::create_dir_all(pack.join("styleguides")).unwrap();
    fs::write(pack.join("styleguides/docs-style.styleguide.yaml"),"id: docs-style\nschema_version: '1.0'\ntitle: Docs\nscope: docs\nprinciples: [p]\nstructural_lint_config:\n  binary: !!binary /w==\n").unwrap();
    let catalog = CanonicalOfferingService::new(OfferingContext::new(
        t.path().to_path_buf(),
        OfferingRoots {
            built_in: Some(pack),
            org_roots: vec![],
            project: None,
        },
        None,
    ));
    let cfg = GovernanceCharterSelection {
        selected_styleguides: vec![text("docs-style")],
        ..GovernanceCharterSelection::default()
    };
    let rendered = render_selection_block(&SelectionBlockInputs {
        charter: Some(&cfg),
        catalog: &catalog,
        identity: None,
        repo_root: None,
        scope: None,
        warning_sink: None,
    })
    .expect("Source never dumps unrequested structural_lint_config");
    let rendered = rendered.to_scalar_string().unwrap();
    assert!(rendered.contains("    Principles:\n      - p"));
    assert!(!rendered.contains("catalog entry not found"));
}

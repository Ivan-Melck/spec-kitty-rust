use sk_doctrine::{
    CanonicalOfferingService, OfferingContext, OfferingRoots, ProfileRepository,
    ProfileSectionsInputs, format_profile_directive_code, render_profile_sections,
};
use sk_doctrine::{CatalogMissCause, DrgGraph, classify_catalog_miss};
use sk_kernel::PythonString;
use std::fs;
fn text(s: &str) -> PythonString {
    PythonString::from_scalars(s)
}
#[test]
fn empty_directive_code_is_not_a_fabricated_directive() {
    assert_eq!(format_profile_directive_code(&text("")), text(""));
}
#[test]
fn directive_regex_dot_does_not_match_lf_inside_suffix() {
    assert_eq!(
        format_profile_directive_code(&text("12-x\ny")),
        text("12-x\ny")
    );
    assert_eq!(
        format_profile_directive_code(&text("12-x\ry")),
        text("DIRECTIVE_012")
    );
}
#[test]
fn close_match_direction_is_candidate_then_missing_word() {
    let diagnosis = classify_catalog_miss(&text("xxxxtide"), &[text("xxxxdiet")]);
    assert_eq!(diagnosis.cause, CatalogMissCause::TypoSuspected);
    assert_eq!(diagnosis.suggestion, Some(text("xxxxdiet")));
}
#[test]
fn pointer_only_styleguide_does_not_dump_unrequested_any() {
    let temp = tempfile::tempdir().unwrap();
    let pack = temp.path().join("built-in");
    fs::create_dir_all(pack.join("styleguides")).unwrap();
    fs::create_dir_all(pack.join("agent_profiles")).unwrap();
    fs::write(pack.join("styleguides/docs-style.styleguide.yaml"),"schema_version: '1.0'\nid: docs-style\ntitle: Docs\nscope: docs\nprinciples: [p]\nstructural_lint_config:\n  binary: !!binary /w==\n").unwrap();
    fs::write(pack.join("agent_profiles/p.agent.yaml"),"profile-id: p\nname: P\nschema-version: '1.0'\nroles: [implementer]\npurpose: Probing\nspecialization:\n  primary-focus: Probe\nstyleguide-references:\n  - id: docs-style\n    rationale: why\n").unwrap();
    let graph = DrgGraph::new("1.0", "2026-11-06T00:00:00Z", "parent", vec![], vec![]).unwrap();
    let profiles = ProfileRepository::load(&pack, &[], None).unwrap();
    let profile = profiles.resolve_str("p", &graph).unwrap();
    let catalog = CanonicalOfferingService::new(OfferingContext::new(
        temp.path().to_path_buf(),
        OfferingRoots {
            built_in: Some(pack),
            org_roots: vec![],
            project: None,
        },
        None,
    ));
    let rendered = render_profile_sections(&ProfileSectionsInputs {
        profile: &profile,
        graph: &graph,
        catalog: &catalog,
        scope: None,
        warning_sink: None,
    })
    .expect("Source pointer-only path never serializes the admitted body");
    assert!(
        rendered
            .to_scalar_string()
            .unwrap()
            .contains("--include styleguide:docs-style")
    );
}

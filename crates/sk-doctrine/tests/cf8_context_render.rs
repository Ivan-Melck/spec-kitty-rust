//! CF8 focused smoke: the canonical `charter context` text renderer.
//!
//! These prove the formatter consumes the real `CharterContext` payload and
//! emits the reference's block structure. They are not a whole-prompt parity
//! campaign (token budget, org provenance suffixes and bundle-schema variants
//! remain pending).

use std::path::{Path, PathBuf};

use sk_doctrine::context_render::{
    BOOTSTRAP_HEADER, CompactGovernance, NO_POLICY_SUMMARY_MESSAGE, REFERENCE_DOCS_HEADER,
    ReferenceDoc, RenderInputs, extract_policy_summary, render, render_compact,
    render_critical_section_bodies, render_section_include, render_with, section_anchors,
};
use sk_doctrine::{
    ArtifactEntry, CharterContext, ContextMode, Delivery, DirectiveEntry, EdgeReference, NodeKind,
    OrgCharterBlock, ProjectCharterBlock, ReferenceEntry,
};
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonValue;

fn entry(
    id: &str,
    kind: NodeKind,
    delivery: Delivery,
    title: Option<&str>,
    summary: Option<&str>,
    when: Option<&str>,
    body: Option<PythonJsonValue>,
) -> ArtifactEntry {
    ArtifactEntry {
        id: id.to_owned(),
        urn: format!("{}:{id}", kind.as_str()),
        kind,
        source: "built-in".to_owned(),
        title: title.map(PythonString::from_scalars),
        summary: summary.map(PythonString::from_scalars),
        references: vec![EdgeReference {
            id: "edge-target".to_owned(),
            relation: "requires".to_owned(),
            when: None,
            reason: None,
        }],
        delivery,
        body,
        when: when.map(str::to_owned),
    }
}

fn payload() -> CharterContext {
    CharterContext {
        context_schema_version: "1.4.0".to_owned(),
        action: "implement".to_owned(),
        mode: ContextMode::Bootstrap,
        directives: vec![entry(
            "024-locality-of-change",
            NodeKind::Directive,
            Delivery::Inline,
            Some("Locality of Change"),
            Some("Keep edits close to the problem"),
            None,
            Some(PythonJsonValue::from_serde(&serde_json::json!({
                "intent": "keep the change local",
                "scope": "code",
                "procedures": ["read the diff"],
            }))),
        )],
        tactics: vec![entry(
            "adversarial-qa-handoff",
            NodeKind::Tactic,
            Delivery::Link,
            Some("Adversarial QA handoff"),
            None,
            Some("review a work package"),
            None,
        )],
        styleguides: Vec::new(),
        toolguides: Vec::new(),
        procedures: Vec::new(),
        all_directives: vec![DirectiveEntry {
            id: "024-locality-of-change".to_owned(),
            source: "built-in".to_owned(),
            title: Some(PythonString::from_scalars("Locality of Change")),
            summary: None,
        }],
        directives_source: Some("built-in".to_owned()),
        diagnostics: Vec::new(),
        references: Vec::new(),
        project_charter: ProjectCharterBlock {
            present: true,
            path: ".kittify/charter/charter.yaml".to_owned(),
            charter_md_present: true,
            charter_md_path: ".kittify/charter/charter.md".to_owned(),
            bytes: Some(1024),
            hash: None,
            source_path: None,
            bundle_schema_version: None,
            schema_version: None,
        },
        org_charter: OrgCharterBlock::default(),
        governance_references: Vec::new(),
        profile_links: Vec::new(),
        tension_arbiters: Vec::new(),
        unarbitrated_tensions: Vec::new(),
    }
}

const CHARTER_MD: &str = "# Spec Kitty Charter\n\n\
- First bullet\n\
- Second bullet\n\n\
## Policy Summary\n\n\
- Keep one canonical authority.\n\
- Verify against live code.\n\n\
## Terminology Canon\n\n\
Use Mission, never Feature.\n\n\
## Code Review Checklist\n\n\
- tests added\n\n\
## Regression Vigilance (2026-04-06)\n\n\
grep for the old term.\n\n\
## Other\n\nignored\n";

#[test]
fn bootstrap_render_carries_every_reference_block() {
    let root = tempfile::TempDir::new().expect("temp");
    std::fs::create_dir_all(root.path().join("docs/context")).expect("docs/context");
    let declared = vec![
        "docs/context/".to_owned(),
        "docs/does-not-exist/".to_owned(),
    ];
    let references = vec![ReferenceDoc {
        entry: ReferenceEntry {
            id: PythonString::from_scalars("DIRECTIVE_024"),
            title: PythonString::from_scalars("Locality of Change"),
            local_path: PythonString::from_scalars(
                "directives/024-locality-of-change.directive.yaml",
            ),
            kind: PythonString::from_scalars("directive"),
            summary: PythonString::default(),
        },
        source: PathBuf::from("packs/built-in/directives/024-locality-of-change.directive.yaml"),
        source_text: PythonString::from_scalars(
            "packs/built-in/directives/024-locality-of-change.directive.yaml",
        ),
    }];
    let inputs = RenderInputs {
        charter_markdown: Some(CHARTER_MD),
        project_root: Some(root.path()),
        declared_authority_paths: &declared,
        reference_docs: &references,
        compact_governance: None,
        languages: None,
        lacks_specialist_guidance: false,
        profile_sections: None,
        selection_block: None,
    };
    let text = render_with(&payload(), &inputs).expect("render");
    assert!(text.starts_with(BOOTSTRAP_HEADER));
    assert!(text.contains("  - Keep one canonical authority."));
    assert!(!text.contains(NO_POLICY_SUMMARY_MESSAGE));
    assert!(text.contains("Project authority paths:"));
    assert!(text.contains("  - docs/context/: canonical terminology"));
    assert!(!text.contains("docs/does-not-exist/"));
    assert!(text.contains("Action-Critical Charter Sections (implement):"));
    assert!(text.contains("### Terminology Canon\nUse Mission, never Feature."));
    assert!(text.contains("Run: spec-kitty charter context --include section:terminology-canon"));
    assert!(text.contains("### Regression Vigilance"));
    assert!(text.contains("Action Doctrine (implement):"));
    assert!(text.contains("  Directives:"));
    assert!(text.contains(
        "    - 024-locality-of-change: Locality of Change — Keep edits close to the problem"
    ));
    // The reference bootstrap render emits the header line only: the artifact
    // body is delivered through the `--json` payload, never inline in this text.
    assert!(!text.contains("Intent: keep the change local"));
    assert!(!text.contains("      - read the diff"));
    assert!(text.contains("    - adversarial-qa-handoff: Adversarial QA handoff"));
    assert!(text.contains("When you review a work package, run this command"));
    assert!(text.contains(REFERENCE_DOCS_HEADER));
    assert!(text.contains("  - DIRECTIVE_024: Locality of Change"));
}

#[test]
fn bootstrap_render_uses_the_reference_fallbacks() {
    let text = render(&payload()).expect("render");
    assert!(text.contains(NO_POLICY_SUMMARY_MESSAGE));
    assert!(text.contains("  - No references manifest found."));
    assert!(text.contains("Action-Critical Charter Sections (implement):"));
    assert!(
        text.contains("Run: spec-kitty charter context --include section:code-review-checklist")
    );
}

#[test]
fn compact_render_keeps_ids_and_anchors() {
    let mut payload = payload();
    payload.mode = ContextMode::Compact;
    let governance = CompactGovernance {
        template_set: Some("software-dev-default".to_owned()),
        paradigms: vec!["structured-prompt-driven-development".to_owned()],
        tools: vec!["git".to_owned(), "pytest".to_owned()],
        diagnostics: vec!["resolver note".to_owned()],
        resolver_directive_ids: vec!["DIRECTIVE_003".to_owned()],
    };
    let languages = vec!["python".to_owned(), "java".to_owned()];
    let inputs = RenderInputs {
        charter_markdown: Some(CHARTER_MD),
        project_root: None,
        declared_authority_paths: &[],
        reference_docs: &[],
        compact_governance: Some(&governance),
        languages: Some(&languages),
        lacks_specialist_guidance: true,
        profile_sections: None,
        selection_block: None,
    };
    let text = render_compact(&payload, &inputs).expect("compact");
    assert!(text.starts_with("Governance:"));
    assert!(text.contains("  - Template set: software-dev-default"));
    assert!(text.contains("  - Paradigms: structured-prompt-driven-development"));
    assert!(text.contains("  - Tools: git, pytest"));
    assert!(text.contains("Directive IDs:\n  - 024-locality-of-change\n  - DIRECTIVE_003"));
    assert!(text.contains("Tactic IDs:\n  - adversarial-qa-handoff"));
    assert!(!text.contains("Styleguide IDs:"));
    assert!(text.contains("Section Anchors:"));
    assert!(text.contains("  - Regression Vigilance (2026-04-06)"));
    assert!(text.contains("  - Diagnostics: resolver note"));
    assert!(text.contains("  - Languages: java, python"));
    assert!(text.contains("  - Advisory: No specialist guidance exists"));
}

#[test]
fn section_include_renders_body_or_honest_placeholder() {
    let body = render_section_include(CHARTER_MD, "terminology-canon").expect("known section");
    assert!(body.starts_with("### Terminology Canon"));
    assert!(body.contains("Use Mission, never Feature."));
    let missing = render_section_include("# Empty\n", "regression-vigilance").expect("known slug");
    assert!(missing.contains("has not yet authored a **Regression Vigilance** section"));
    assert!(render_section_include("# Empty\n", "no-such-section").is_none());
}

#[test]
fn policy_summary_and_anchors_follow_the_reference_rules() {
    assert_eq!(
        extract_policy_summary(CHARTER_MD),
        vec![
            "Keep one canonical authority.".to_owned(),
            "Verify against live code.".to_owned()
        ]
    );
    assert_eq!(
        extract_policy_summary("- alpha\n- beta\n"),
        vec!["alpha".to_owned(), "beta".to_owned()]
    );
    let anchors = section_anchors(Some(CHARTER_MD));
    assert_eq!(
        anchors,
        vec![
            "Spec Kitty Charter".to_owned(),
            "Policy Summary".to_owned(),
            "Terminology Canon".to_owned(),
            "Code Review Checklist".to_owned(),
            "Regression Vigilance (2026-04-06)".to_owned(),
            "Other".to_owned(),
        ]
    );
    assert!(render_critical_section_bodies("# nothing\n", "specify").is_empty());
}

#[test]
fn missing_critical_section_keeps_the_fetch_rail() {
    let block = render_critical_section_bodies("# Charter\n", "review");
    assert!(block.contains("Action-Critical Charter Sections (review):"));
    assert!(
        block.contains("Run: spec-kitty charter context --include section:regression-vigilance")
    );
    assert!(block.contains("When you need to perform a terminology cutover"));
    let _ = Path::new(".");
}

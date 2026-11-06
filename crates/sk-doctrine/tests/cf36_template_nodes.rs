//! CF36 focused smoke: the Doctrine template→DRG node adapter over the canonical CF30
//! template catalog.
//!
//! These exercise real temporary mission trees through the actual catalog
//! ([`sk_charter::template_catalog::discover_templates`]) and the actual graph
//! authority ([`sk_doctrine::DrgNode`]). They are not a DRG merge/aggregation campaign:
//! this lane mints nodes as data and does not merge, register or persist them.

use std::fs;
use std::path::Path;

use sk_charter::template_catalog::{TemplateTier, TierRoot, discover_templates};
use sk_doctrine::template_nodes::{template_node, template_nodes};
use sk_doctrine::{DoctrineError, NodeKind};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn catalog(
    root: &Path,
    tiers: &[(TemplateTier, &str)],
) -> Vec<sk_charter::template_catalog::TemplateRef> {
    let tier_roots: Vec<TierRoot> = tiers
        .iter()
        .map(|(tier, relative)| TierRoot::new(*tier, root.join(*relative)))
        .collect();
    discover_templates(&tier_roots).expect("catalog")
}

#[test]
fn discovered_templates_mint_typed_mission_qualified_nodes() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join("package/missions/software-dev/templates/spec-template.md"),
        "package spec\n",
    );
    write(
        &root.join("package/missions/software-dev/command-templates/implement.md"),
        "package implement\n",
    );
    write(
        &root.join("package/missions/software-dev/templates/README.md"),
        "not a template\n",
    );
    let references = catalog(root, &[(TemplateTier::PackageDefault, "package/missions")]);
    assert_eq!(references.len(), 2, "{references:#?}");

    let nodes = template_nodes(&references).expect("nodes");
    assert_eq!(nodes.len(), references.len());
    for (node, reference) in nodes.iter().zip(references.iter()) {
        assert_eq!(
            node.urn(),
            format!("template:{}", reference.template_id),
            "URN is the canonical template:<mission>/<name> spelling"
        );
        assert_eq!(node.kind(), NodeKind::Template);
        assert_eq!(node.label(), Some(reference.template_id.as_str()));
        assert_eq!(node.kind().as_str(), "template");
        assert_eq!(
            node.kind().artifact_kind(),
            Some(sk_charter::ArtifactKind::Template)
        );
    }
    assert!(
        nodes
            .iter()
            .any(|node| node.urn() == "template:software-dev/spec-template.md")
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.urn() == "template:software-dev/implement.md")
    );
}

#[test]
fn the_same_basename_under_two_missions_mints_two_distinct_nodes() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join("package/missions/alpha/templates/spec-template.md"),
        "alpha\n",
    );
    write(
        &root.join("package/missions/beta/templates/spec-template.md"),
        "beta\n",
    );
    let references = catalog(root, &[(TemplateTier::PackageDefault, "package/missions")]);
    let nodes = template_nodes(&references).expect("nodes");
    assert_eq!(nodes.len(), 2);
    let urns: Vec<&str> = nodes.iter().map(|node| node.urn()).collect();
    assert_eq!(
        urns,
        vec![
            "template:alpha/spec-template.md",
            "template:beta/spec-template.md"
        ]
    );
    assert_ne!(nodes[0], nodes[1]);
}

#[test]
fn one_mission_qualified_id_across_two_tiers_mints_one_node() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join("override/missions/software-dev/templates/spec-template.md"),
        "override\n",
    );
    write(
        &root.join("package/missions/software-dev/templates/spec-template.md"),
        "package\n",
    );
    let references = catalog(
        root,
        &[
            (TemplateTier::Override, "override/missions"),
            (TemplateTier::PackageDefault, "package/missions"),
        ],
    );
    assert_eq!(references.len(), 1, "{references:#?}");
    assert_eq!(references[0].tier, TemplateTier::Override);
    let nodes = template_nodes(&references).expect("nodes");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].urn(), "template:software-dev/spec-template.md");
    assert_eq!(nodes[0].id(), "software-dev/spec-template.md");
}

#[test]
fn a_name_outside_the_urn_grammar_is_a_typed_refusal_not_a_lossy_urn() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join("package/missions/alpha/templates/with space.md"),
        "space\n",
    );
    let references = catalog(root, &[(TemplateTier::PackageDefault, "package/missions")]);
    assert_eq!(references.len(), 1);
    let error = template_node(&references[0]).expect_err("the reference URN grammar refuses this");
    assert!(
        matches!(error, DoctrineError::InvalidUrn { .. }),
        "{error:?}"
    );
    // The catalog ref itself is untouched: the discovery identity is not rewritten.
    assert_eq!(references[0].template_id, "alpha/with space.md");
    assert_eq!(references[0].name, "with space.md");
}

#[test]
fn an_empty_catalog_mints_no_nodes() {
    let temp = tempfile::TempDir::new().expect("temp");
    let references = catalog(
        temp.path(),
        &[(TemplateTier::PackageDefault, "missing/missions")],
    );
    assert!(references.is_empty());
    assert!(template_nodes(&references).expect("nodes").is_empty());
}

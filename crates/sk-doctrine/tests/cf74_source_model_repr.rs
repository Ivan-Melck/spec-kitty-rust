//! CF74 — the prepared profile block's named-step fallback is Source `str(step)`.
//!
//! SOURCE FIRST: the expected lines were observed from the pinned oracle child
//! (`/tmp/sk-cf74-evidence.20261106T134451Z/source-probe-step-model.0W596f.log`,
//! Python 3.11.15 / pydantic 2.13.3) rendering a real `Tactic` model (packed from
//! the real offering models) through the real consumer
//! `charter.activation.context_renderers.profile_sections.format_inline_named_body`:
//!
//! ```text
//!     Steps:
//!       - title='' description=None examples=[] references=[]
//!       - described
//!       - <whitespace title>
//! ```
//!
//! This file has two layers: a Native-only focused check of the existing public
//! profile API plus the exact-payload/include helpers, and one tiny **paired**
//! control where the parent runs the pinned Source child and re-executes this
//! binary as an isolated Native child over two independent business fixtures —
//! both asserting their temporary CWD/HOME/all-XDG/TMP roots, absolute Source and
//! packs roots and hash seed before any business read. Neither layer is
//! whole-text parity or an independent review.

use std::fs;
use std::path::{Path, PathBuf};

use sk_doctrine::context_render::include_body_lines_exact;
use sk_doctrine::{
    CanonicalOfferingService, DrgEdge, DrgGraph, DrgNode, NodeKind, OfferingContext, OfferingRoots,
    ProfileRepository, ProfileSectionsInputs, Relation, exact_payload_value,
    render_profile_sections,
};
use sk_kernel::PythonString;
use sk_kernel::python_json::PythonJsonRenderOptions;

fn scalar(value: &PythonString) -> String {
    value.to_scalar_string().expect("scalar fixture text")
}

/// `json.dumps(model_dump(mode="json", exclude_none=True), indent=2, sort_keys=True)`.
fn source_model_json(kind: sk_charter::ArtifactKind, value: &serde_yaml_ng::Value) -> String {
    exact_payload_value(kind, value)
        .expect("exact payload")
        .render_python_with(PythonJsonRenderOptions::new(Some(2)).sort_keys(true))
        .to_scalar_string()
        .expect("ascii model JSON")
}

fn write_fixture(root: &Path) -> PathBuf {
    let fixture = root.join("fixture");
    let pack = fixture.join("packs/built-in");
    for directory in ["tactics", "procedures", "agent_profiles"] {
        fs::create_dir_all(pack.join(directory)).expect("pack directory");
    }
    fs::create_dir_all(fixture.join(".kittify/charter")).expect("charter dir");
    fs::write(
        fixture.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n",
    )
    .expect("charter");
    fs::write(
        pack.join("tactics/cf74-echo.tactic.yaml"),
        r#"schema_version: "1.0"
id: cf74-echo
name: Cf74 Echo
purpose: Echo the steps.
steps:
  - title: ""
    description:
  - title: ""
    description: described
  - title: Only title
"#,
    )
    .expect("echo tactic");
    fs::write(
        pack.join("procedures/cf74-run.procedure.yaml"),
        r#"schema_version: "1.0"
id: cf74-run
name: Cf74 Run
purpose: Run the steps.
entry_condition: Entry.
exit_condition: Exit.
steps:
  - title: ""
    description: ""
  - title: Procedure step
"#,
    )
    .expect("run procedure");
    fs::write(
        pack.join("agent_profiles/cf74-profile.agent.yaml"),
        r#"profile-id: cf74-profile
name: Cf74 Profile
schema-version: "1.0"
roles:
  - implementer
purpose: Cf74 purpose.
specialization:
  primary-focus: Echoing.
tactic-references:
  - id: cf74-echo
    rationale: echo rationale
"#,
    )
    .expect("cf74 profile");
    fixture
}

fn graph() -> DrgGraph {
    let node = |urn: &str, kind: NodeKind| DrgNode::new(urn, kind, None, Vec::new()).expect("node");
    DrgGraph::new(
        "1.0",
        "2026-11-06T00:00:00Z",
        "cf74-fixture",
        vec![
            node("agent_profile:cf74-profile", NodeKind::AgentProfile),
            node("tactic:cf74-echo", NodeKind::Tactic),
            node("procedure:cf74-run", NodeKind::Procedure),
        ],
        vec![
            DrgEdge::new(
                "agent_profile:cf74-profile",
                "tactic:cf74-echo",
                Relation::Requires,
            )
            .expect("edge"),
            DrgEdge::new(
                "agent_profile:cf74-profile",
                "procedure:cf74-run",
                Relation::Requires,
            )
            .expect("edge"),
        ],
    )
    .expect("graph")
}

#[test]
fn cf74_profile_named_body_uses_the_model_str_when_title_and_description_are_falsy() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture = write_fixture(workspace.path());
    let pack = fixture.join("packs/built-in");
    let graph = graph();
    let profiles = ProfileRepository::load(&pack, &[], None).expect("profiles");
    let catalog = CanonicalOfferingService::new(OfferingContext::new(
        fixture.clone(),
        OfferingRoots {
            built_in: Some(pack.clone()),
            org_roots: Vec::new(),
            project: None,
        },
        Some(vec!["ruby".to_owned()]),
    ));
    let profile = profiles
        .resolve_str("cf74-profile", &graph)
        .expect("profile");
    let inputs = ProfileSectionsInputs {
        profile: &profile,
        graph: &graph,
        catalog: &catalog,
        scope: None,
        warning_sink: None,
    };
    let block = scalar(&render_profile_sections(&inputs).expect("profile block"));
    // Source `header = title or description or str(step)`; the model `str` is the
    // space-separated `field=repr` form, never the JSON payload.
    assert!(
        block.contains("      - title='' description=None examples=[] references=[]\n"),
        "the falsy-title step must render Source str(step):\n{block}"
    );
    assert!(!block.contains("{\"title\""), "{block}");
    // A falsy title with a truthy description renders the description as the
    // header and never duplicates it as a sub-line.
    assert!(block.contains("      - described\n"), "{block}");
    assert!(!block.contains("described\n        described"), "{block}");
    // A truthy title is the header; its description is not authored here, and the
    // resolved procedure renders its own defaults through the same view.
    assert!(block.contains("      - Only title"), "{block}");
    assert!(
        block.contains(
            "      - title='' description='' actor=<ActorRole.AGENT: 'agent'> on_success=None on_failure=None"
        ),
        "{block}"
    );
}

#[test]
fn cf74_exact_json_is_unchanged_by_the_model_view() {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        "id: probe-tactic\nschema_version: '1.0'\nname: Probe Tactic\nsteps:\n  - title: ''\n  - title: ''\n    description: ''\n  - title: T\n    examples: [one, two]\n    references:\n      - name: Architectural Integrity\n        type: directive\n        id: DIRECTIVE_001\n        when: always\n",
    )
    .expect("tactic fixture");
    // Byte-for-byte the pinned Source model JSON: no class names, no Python repr,
    // no `None`, every model default materialised, declaration order preserved.
    assert_eq!(
        source_model_json(sk_charter::ArtifactKind::Tactic, &value),
        "{\n  \"applies_to_languages\": [],\n  \"failure_modes\": [],\n  \"id\": \"probe-tactic\",\n  \"name\": \"Probe Tactic\",\n  \"references\": [],\n  \"schema_version\": \"1.0\",\n  \"steps\": [\n    {\n      \"examples\": [],\n      \"references\": [],\n      \"title\": \"\"\n    },\n    {\n      \"description\": \"\",\n      \"examples\": [],\n      \"references\": [],\n      \"title\": \"\"\n    },\n    {\n      \"examples\": [\n        \"one\",\n        \"two\"\n      ],\n      \"references\": [\n        {\n          \"id\": \"DIRECTIVE_001\",\n          \"name\": \"Architectural Integrity\",\n          \"type\": \"directive\",\n          \"when\": \"always\"\n        }\n      ],\n      \"title\": \"T\"\n    }\n  ]\n}"
    );
    // The enum member's serialized JSON is its value, never its repr.
    assert!(!source_model_json(sk_charter::ArtifactKind::Tactic, &value).contains("ArtifactKind"));
}

#[test]
fn cf74_include_step_lines_keep_the_title_phase() {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        "id: probe-tactic\nschema_version: '1.0'\nname: Probe Tactic\nsteps:\n  - title: ''\n    description: described\n  - title: T\n  - title: '   '\n    description: '   '\n",
    )
    .expect("tactic fixture");
    let payload = exact_payload_value(sk_charter::ArtifactKind::Tactic, &value).expect("payload");
    // Source `_append_step_lines`: the `title` attribute verbatim (an empty title
    // stays an empty header — a description never replaces it, unlike the profile
    // formatter) plus a non-blank description sub-line.
    assert_eq!(
        include_body_lines_exact("tactic", &payload)
            .iter()
            .map(scalar)
            .collect::<Vec<_>>(),
        vec![
            "    Name: Probe Tactic".to_owned(),
            "    Steps:".to_owned(),
            "      - ".to_owned(),
            "        described".to_owned(),
            "      - T".to_owned(),
            "      -    ".to_owned(),
        ]
    );
}

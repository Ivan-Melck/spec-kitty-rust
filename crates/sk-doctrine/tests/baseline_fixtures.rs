//! Baseline conformance: the checked-in upstream fixtures and the exact
//! traversal/validation shapes the Python reference pins.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sk_doctrine::{
    ActivationInputs, ActivationLedger, CONTEXT_CONTRACT_TOP_LEVEL_KEYS, CascadeScope,
    ContextInputs, ContextRequest, DeactivationInputs, DenyListChoice, DoctrineError, DrgEdge,
    DrgGraph, DrgNode, IdentityIndex, NodeKind, ProfileRepository, Relation, build_action_context,
    cascade_activation_targets, deactivation_plan, load_graph_or_dir, load_org_pack,
    merge_three_layers, plan_activation, plan_deactivation, resolve_context, validate_graph,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn fixture(path: &str) -> PathBuf {
    repo_root().join(path)
}

fn node(urn: &str, kind: NodeKind) -> DrgNode {
    DrgNode::new(urn, kind, None, Vec::new()).unwrap()
}

fn edge(source: &str, target: &str, relation: Relation) -> DrgEdge {
    DrgEdge::new(source, target, relation).unwrap()
}

#[test]
fn example_org_fixture_merges_with_provenance_and_reason() {
    let pack_root = fixture("tests/architectural/_fixtures/org_packs/example_org");
    let fragment = load_org_pack("example-org", &pack_root, 1).expect("fragment");
    let built_in = DrgGraph::new(
        "1.0",
        "2026-01-01T00:00:00Z",
        "built-in",
        vec![node("styleguide:plain-language", NodeKind::Styleguide)],
        Vec::new(),
    )
    .unwrap();
    let report = merge_three_layers(&built_in, &[fragment], None).expect("merge");
    let source = report
        .graph
        .node("directive:sox-controls")
        .expect("org node");
    assert_eq!(source.provenance(), Some("org:example-org"));
    let edge = report
        .graph
        .edges()
        .iter()
        .find(|edge| edge.source() == "directive:sox-controls")
        .expect("refines edge");
    assert_eq!(edge.target(), "styleguide:plain-language");
    assert_eq!(edge.relation(), Relation::Refines);
    assert_eq!(edge.provenance(), Some("org:example-org"));
}

#[test]
fn second_org_pack_contributes_its_distinctive_marker() {
    let pack_root = fixture("tests/charter_offering/fixtures/minimal_org_pack_2");
    let fragment = load_org_pack("minimal-org-2", &pack_root, 2).expect("fragment");
    let built_in = DrgGraph::new(
        "1.0",
        "2026-01-01T00:00:00Z",
        "built-in",
        vec![node(
            "procedure:red-main-release-discipline",
            NodeKind::Procedure,
        )],
        Vec::new(),
    )
    .unwrap();
    let report = merge_three_layers(&built_in, &[fragment], None).expect("merge");
    assert_eq!(
        report
            .graph
            .node("directive:MINIMAL_ORG_2_MARKER")
            .expect("marker")
            .provenance(),
        Some("org:minimal-org-2")
    );
    let edge = report
        .graph
        .edges()
        .iter()
        .find(|edge| edge.source() == "directive:MINIMAL_ORG_2_MARKER")
        .expect("refines edge");
    assert_eq!(
        edge.reason(),
        Some("Marker refines a shipped built-in procedure so pack #2 folds clean")
    );
}

#[test]
fn template_asset_fixture_keeps_template_asset_nodes_and_edges() {
    let pack_root = fixture("tests/charter_offering/fixtures/org_pack_template_asset/valid_pack");
    let fragment = load_org_pack("regnology", &pack_root, 1).expect("fragment");
    let built_in = DrgGraph::new("1.0", "x", "empty", Vec::new(), Vec::new()).unwrap();
    let report = merge_three_layers(&built_in, &[fragment], None).expect("merge");
    assert!(
        report
            .graph
            .node("template:meeting-minutes")
            .is_some_and(|node| node.kind() == NodeKind::Template)
    );
    assert!(
        report
            .graph
            .node("asset:company-logo")
            .is_some_and(|node| node.kind() == NodeKind::Asset)
    );
    assert!(report.graph.edges().iter().any(|edge| {
        edge.source() == "styleguide:regnology-house-style"
            && edge.target() == "template:meeting-minutes"
            && edge.reason()
                == Some("House style mandates the meeting-minutes template for internal reviews")
    }));
}

#[test]
fn duplicate_asset_urns_across_org_packs_are_refused() {
    let first = load_org_pack(
        "pack-a",
        &fixture("tests/charter_offering/fixtures/org_pack_template_asset/duplicate_asset_pack_a"),
        1,
    )
    .expect("pack a");
    let second = load_org_pack(
        "pack-b",
        &fixture("tests/charter_offering/fixtures/org_pack_template_asset/duplicate_asset_pack_b"),
        2,
    )
    .expect("pack b");
    let built_in = DrgGraph::new("1.0", "x", "empty", Vec::new(), Vec::new()).unwrap();
    let error = merge_three_layers(&built_in, &[first, second], None).unwrap_err();
    assert!(matches!(error, DoctrineError::DuplicateUrn { .. }));
}

#[test]
fn loader_merges_fragments_alphabetically_and_keeps_the_later_label() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("aaa.graph.yaml"),
        "schema_version: \"1.0\"\ngenerated_at: x\ngenerated_by: aaa\nnodes:\n  - urn: directive:DIRECTIVE_001\n    kind: directive\n    label: early\nedges: []\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("zzz.graph.yaml"),
        "schema_version: \"1.0\"\ngenerated_at: x\ngenerated_by: zzz\nnodes:\n  - urn: directive:DIRECTIVE_001\n    kind: directive\n    label: late\n  - urn: tactic:t\n    kind: tactic\nedges:\n  - source: tactic:t\n    target: directive:DIRECTIVE_001\n    relation: requires\n",
    )
    .unwrap();
    let graph = load_graph_or_dir(directory.path()).expect("graph");
    assert_eq!(
        graph.node("directive:DIRECTIVE_001").unwrap().label(),
        Some("late")
    );
    assert!(graph.node("tactic:t").is_some());
    assert_eq!(graph.edges().len(), 1);
}

#[test]
fn loader_refuses_unknown_top_level_keys_and_node_fields() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("graph.yaml"),
        "schema_version: \"1.0\"\ngenerated_at: x\ngenerated_by: y\nsurprise: true\nnodes: []\nedges: []\n",
    )
    .unwrap();
    let error = load_graph_or_dir(directory.path()).unwrap_err();
    assert!(matches!(error, DoctrineError::UnknownTopLevelKey { .. }));

    fs::write(
        directory.path().join("graph.yaml"),
        "schema_version: \"1.0\"\ngenerated_at: x\ngenerated_by: y\nnodes:\n  - urn: directive:DIRECTIVE_001\n    kind: directive\n    surprise: true\nedges: []\n",
    )
    .unwrap();
    let error = load_graph_or_dir(directory.path()).unwrap_err();
    assert!(matches!(error, DoctrineError::UnknownField { .. }));
}

#[test]
fn invalid_urns_and_ambiguous_endpoints_are_refused() {
    assert!(DrgNode::new("directive:bad id", NodeKind::Directive, None, Vec::new()).is_err());
    assert!(DrgNode::new("tactic:001", NodeKind::Directive, None, Vec::new()).is_err());
    assert!(DrgEdge::new("directive:001", "not-an-urn", Relation::Requires).is_err());

    let built_in = DrgGraph::new(
        "1.0",
        "x",
        "built-in",
        vec![
            node("directive:plain", NodeKind::Directive),
            node("tactic:plain", NodeKind::Tactic),
        ],
        Vec::new(),
    )
    .unwrap();
    let pack = tempfile::tempdir().unwrap();
    fs::write(
        pack.path().join("pack.yaml"),
        "name: acme\npack_level: org\n",
    )
    .unwrap();
    fs::create_dir_all(pack.path().join("drg")).unwrap();
    fs::write(
        pack.path().join("drg/fragment.yaml"),
        "nodes:\n  - id: x\n    kind: directives\nedges:\n  - source: x\n    target: plain\n    relation: requires\n",
    )
    .unwrap();
    let fragment = load_org_pack("acme", pack.path(), 1).unwrap();
    let error = merge_three_layers(&built_in, &[fragment], None).unwrap_err();
    assert!(matches!(error, DoctrineError::AmbiguousEndpoint { .. }));
}

#[test]
fn tension_annotation_matches_the_reference_corpus_shape() {
    let action = "action:software-dev/implement";
    let graph = DrgGraph::new(
        "1.0",
        "2026-06-01T00:00:00Z",
        "test",
        vec![
            node(action, NodeKind::Action),
            node("directive:DIRECTIVE_024", NodeKind::Directive),
            node("directive:DIRECTIVE_025", NodeKind::Directive),
            node(
                "directive:RECONCILE_CHANGE_SCOPE_TENSIONS",
                NodeKind::Directive,
            ),
            node("directive:DIRECTIVE_TENSION_A", NodeKind::Directive),
            node("directive:DIRECTIVE_TENSION_B", NodeKind::Directive),
        ],
        vec![
            edge(action, "directive:DIRECTIVE_024", Relation::Scope),
            edge(action, "directive:DIRECTIVE_025", Relation::Scope),
            edge(action, "directive:DIRECTIVE_TENSION_A", Relation::Scope),
            edge(action, "directive:DIRECTIVE_TENSION_B", Relation::Scope),
            edge(
                "directive:DIRECTIVE_024",
                "directive:DIRECTIVE_025",
                Relation::InTensionWith,
            ),
            edge(
                "directive:DIRECTIVE_TENSION_A",
                "directive:DIRECTIVE_TENSION_B",
                Relation::InTensionWith,
            ),
            edge(
                "directive:RECONCILE_CHANGE_SCOPE_TENSIONS",
                "directive:DIRECTIVE_024",
                Relation::ReconcilesTension,
            ),
            edge(
                "directive:RECONCILE_CHANGE_SCOPE_TENSIONS",
                "directive:DIRECTIVE_025",
                Relation::ReconcilesTension,
            ),
        ],
    )
    .unwrap();
    let context = resolve_context(&graph, action, 2);
    assert_eq!(
        context.tension_arbiters,
        vec![(
            "directive:RECONCILE_CHANGE_SCOPE_TENSIONS".to_owned(),
            vec![
                "directive:DIRECTIVE_024".to_owned(),
                "directive:DIRECTIVE_025".to_owned()
            ]
        )]
    );
    assert_eq!(
        context.unarbitrated_tensions,
        vec![(
            "directive:DIRECTIVE_TENSION_A".to_owned(),
            "directive:DIRECTIVE_TENSION_B".to_owned()
        )]
    );
    assert!(
        !context
            .artifact_urns
            .contains("directive:RECONCILE_CHANGE_SCOPE_TENSIONS")
    );
}

#[test]
fn graph_validation_reports_every_reference_failure() {
    let graph = DrgGraph::new(
        "1.0",
        "x",
        "test",
        vec![
            node("directive:a", NodeKind::Directive),
            node("directive:b", NodeKind::Directive),
            node("agent_profile:p", NodeKind::AgentProfile),
            node("tactic:t", NodeKind::Tactic),
            node("anti_pattern:orphan", NodeKind::AntiPattern),
        ],
        vec![
            edge("directive:a", "directive:missing", Relation::Requires),
            edge("directive:a", "directive:missing", Relation::Requires),
            edge("directive:a", "directive:b", Relation::Requires),
            edge("directive:b", "directive:a", Relation::Requires),
            edge("agent_profile:p", "tactic:t", Relation::SpecializesFrom),
            edge("directive:a", "directive:b", Relation::Rejects),
        ],
    )
    .unwrap();
    let errors = validate_graph(&graph);
    let joined = errors.join("\n");
    assert!(joined.contains("Dangling target"));
    assert!(joined.contains("Duplicate edge"));
    assert!(joined.contains("Cycle in requires"));
    assert!(joined.contains("must be an agent_profile"));
    assert!(joined.contains("Rejects-edge target must be an anti_pattern"));
    assert!(joined.contains("Orphaned anti_pattern"));
}

#[test]
fn cascade_and_shared_reference_deactivation_match_the_contract() {
    let graph = DrgGraph::new(
        "1.0",
        "x",
        "test",
        vec![
            node("agent_profile:root", NodeKind::AgentProfile),
            node("agent_profile:other", NodeKind::AgentProfile),
            node("directive:shared", NodeKind::Directive),
            node("tactic:exclusive", NodeKind::Tactic),
            node("template:t", NodeKind::Template),
        ],
        vec![
            edge("agent_profile:root", "directive:shared", Relation::Requires),
            edge("agent_profile:root", "tactic:exclusive", Relation::Requires),
            edge("agent_profile:root", "template:t", Relation::Instantiates),
            edge(
                "agent_profile:other",
                "directive:shared",
                Relation::Requires,
            ),
        ],
    )
    .unwrap();
    let activation = cascade_activation_targets(
        &graph,
        "agent_profile:root",
        &CascadeScope::all(),
        &BTreeSet::new(),
        false,
    );
    assert_eq!(
        activation.not_cascaded_kind_filtered["template"],
        vec!["t".to_owned()]
    );
    let plan = deactivation_plan(
        &graph,
        "agent_profile:root",
        &CascadeScope::all(),
        &BTreeSet::from([
            "agent_profile:root".to_owned(),
            "agent_profile:other".to_owned(),
        ]),
    );
    assert_eq!(plan.deactivate, vec!["tactic:exclusive".to_owned()]);
    assert_eq!(
        plan.skipped_shared[0].referencing_active_urn,
        "agent_profile:other"
    );
}

#[test]
fn activation_plan_and_commit_preserve_user_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join(".kittify/charter")).unwrap();
    let ledger = root.join(".kittify/charter/activation.yaml");
    fs::write(
        &ledger,
        "# operator note\nactivated_directives:\n  - 001-arch  # keep this rationale\n",
    )
    .unwrap();
    let document = sk_doctrine::YamlDocument::open(&ledger).unwrap();
    let available = vec!["001-arch".to_owned(), "002-style".to_owned()];
    let plan = plan_activation(
        "directive",
        "002-style",
        &ActivationInputs {
            yaml_key: "activated_directives",
            available_ids: &available,
            config_data: &document,
            effective_ids: &available,
            effective_when_absent: sk_doctrine::EffectiveWhenAbsent::All,
            effective_ids_in_force: true,
            narrow_base: None,
            previous_ids: &[],
            deny_key: Some("deactivated_directives"),
            current_deny: &[],
            deny_change: DenyListChoice::Activate,
            effective_deny: &sk_doctrine::DenySet::empty(),
        },
    )
    .unwrap();
    let transaction = sk_doctrine::ActivationTransaction::begin(root).unwrap();
    transaction.commit(&plan).unwrap();
    let text = fs::read_to_string(&ledger).unwrap();
    assert!(text.contains("# operator note"));
    assert!(text.contains("# keep this rationale"));
    assert!(text.contains("002-style"));
}

#[test]
fn deactivation_is_refused_by_the_required_floor_without_a_write() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join(".kittify/charter")).unwrap();
    let ledger = root.join(".kittify/charter/activation.yaml");
    fs::write(&ledger, "activated_directives: [001-arch]\n").unwrap();
    let before = fs::read_to_string(&ledger).unwrap();
    let document = sk_doctrine::YamlDocument::open(&ledger).unwrap();
    let required = BTreeMap::from([(
        "001-arch".to_owned(),
        vec![sk_doctrine::RequiringPack {
            pack: "acme".to_owned(),
            via: None,
        }],
    )]);
    let available = vec!["001-arch".to_owned()];
    let result = plan_deactivation(
        "directive",
        "001-arch",
        &DeactivationInputs {
            yaml_key: "activated_directives",
            config_data: &document,
            available_ids: &available,
            required_by: &required,
            deny_key: Some("deactivated_directives"),
            current_deny: &[],
            record_exclusion: true,
            also_matches: &[],
        },
    );
    assert!(matches!(
        result,
        Err(DoctrineError::RequiredArtifact { .. })
    ));
    assert_eq!(fs::read_to_string(&ledger).unwrap(), before);
}

#[test]
fn context_payload_has_exactly_the_contracted_top_level_keys() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let built_in = root.join("packs/built-in");
    fs::create_dir_all(built_in.join("directives")).unwrap();
    fs::write(
        built_in.join("directives/001-arch.directive.yaml"),
        "id: DIRECTIVE_001\nschema_version: '1.0'\ntitle: Architecture\nintent: Keep seams\nenforcement: required\n",
    )
    .unwrap();
    fs::create_dir_all(built_in.join("tactics")).unwrap();
    fs::write(
        built_in.join("tactics/hard.tactic.yaml"),
        "id: hard\nschema_version: '1.0'\nname: Hard\nsteps:\n  - title: one\n",
    )
    .unwrap();
    let identity = IdentityIndex::from_roots(&built_in, &[], None);
    let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
    let ledger = ActivationLedger::default();
    let floor = BTreeMap::new();
    let graph = DrgGraph::new(
        "1.0",
        "x",
        "built-in",
        vec![
            node("action:software-dev/implement", NodeKind::Action),
            node("directive:DIRECTIVE_001", NodeKind::Directive),
            node("tactic:hard", NodeKind::Tactic),
        ],
        vec![
            edge(
                "action:software-dev/implement",
                "directive:DIRECTIVE_001",
                Relation::Scope,
            ),
            edge("directive:DIRECTIVE_001", "tactic:hard", Relation::Requires),
        ],
    )
    .unwrap();
    let inputs = ContextInputs {
        graph: &graph,
        ledger: &ledger,
        identity: &identity,
        profiles: &profiles,
        project_root: root,
        required_floor: &floor,
        project_warnings: &[],
    };
    let context = build_action_context(
        &inputs,
        &ContextRequest::new("action:software-dev/implement"),
    )
    .unwrap();
    let payload = context.to_json();
    let keys: BTreeSet<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = CONTEXT_CONTRACT_TOP_LEVEL_KEYS.into_iter().collect();
    assert_eq!(keys, expected);
    assert_eq!(context.directives[0].id, "DIRECTIVE_001");
    assert_eq!(
        context.directives[0].delivery,
        sk_doctrine::Delivery::Inline
    );
    assert_eq!(
        context.directives[0]
            .body
            .as_ref()
            .and_then(|body| body.field("id"))
            .and_then(|value| match value {
                sk_kernel::python_json::PythonJsonValue::String(text) => {
                    text.to_scalar_string().ok()
                }
                _ => None,
            }),
        Some("DIRECTIVE_001".to_owned())
    );
}

#[test]
fn baseline_valid_profile_fixture_loads_and_composes() {
    let directory = tempfile::tempdir().unwrap();
    let built_in = directory.path().join("built-in");
    fs::create_dir_all(built_in.join("agent_profiles")).unwrap();
    fs::copy(
        fixture("tests/charter_offering/fixtures/valid-profile.agent.yaml"),
        built_in.join("agent_profiles/valid-profile.agent.yaml"),
    )
    .unwrap();
    let repository = ProfileRepository::load(&built_in, &[], None).unwrap();
    let graph = DrgGraph::new(
        "1.0",
        "x",
        "test",
        vec![node(
            "agent_profile:architect-alphonso",
            NodeKind::AgentProfile,
        )],
        Vec::new(),
    )
    .unwrap();
    let profile = repository
        .resolve_str("architect-alphonso", &graph)
        .expect("profile");
    assert_eq!(
        profile.name().and_then(|name| name.to_scalar_string().ok()),
        Some("Architect Alphonso".to_owned())
    );
    assert_eq!(
        profile
            .directive_references()
            .iter()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect::<Vec<_>>(),
        vec!["ARCH-001".to_owned(), "DESIGN-002".to_owned()]
    );
    assert_eq!(
        profile
            .lineage
            .iter()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect::<Vec<_>>(),
        vec!["architect-alphonso".to_owned()]
    );
}

#[test]
fn urn_grammar_matches_the_reference_exactly() {
    for (value, valid) in [
        ("directive:001", true),
        ("directive:", false),
        ("directive:a b", false),
        ("Directive:1", false),
        ("directive:a:b", false),
        ("_x:a", true),
        ("directive:a.b/c-d_e", true),
        ("a:1", true),
    ] {
        assert_eq!(sk_doctrine::is_valid_urn(value), valid, "URN {value:?}");
    }
}

#[test]
fn ledger_vocabulary_matches_the_reference_exactly() {
    assert_eq!(
        sk_doctrine::ledger_keys(),
        vec![
            "activated_kinds",
            "mission_type_activations",
            "activated_directives",
            "activated_tactics",
            "activated_styleguides",
            "activated_toolguides",
            "activated_paradigms",
            "activated_procedures",
            "activated_agent_profiles",
            "activated_mission_step_contracts",
            "activated_glossary_packs",
            "activated_anti_patterns",
            "activated_skills",
            "deactivated_directives",
            "deactivated_tactics",
            "deactivated_styleguides",
            "deactivated_toolguides",
            "deactivated_paradigms",
            "deactivated_procedures",
            "deactivated_agent_profiles",
            "deactivated_mission_step_contracts",
            "deactivated_glossary_packs",
            "deactivated_anti_patterns",
            "deactivated_skills",
        ]
    );
}

#[test]
fn validator_messages_match_the_reference_exactly() {
    let graph = DrgGraph::new(
        "1.0",
        "x",
        "test",
        vec![
            node("directive:a", NodeKind::Directive),
            node("directive:b", NodeKind::Directive),
            node("agent_profile:p", NodeKind::AgentProfile),
            node("tactic:t", NodeKind::Tactic),
            node("anti_pattern:orphan", NodeKind::AntiPattern),
        ],
        vec![
            edge("directive:a", "directive:missing", Relation::Requires),
            edge("directive:a", "directive:missing", Relation::Requires),
            edge("directive:a", "directive:b", Relation::Requires),
            edge("directive:b", "directive:a", Relation::Requires),
            edge("agent_profile:p", "tactic:t", Relation::SpecializesFrom),
            edge("directive:a", "directive:b", Relation::Rejects),
        ],
    )
    .unwrap();
    assert_eq!(
        validate_graph(&graph),
        vec![
            "Dangling target: edge (directive:a --requires--> directive:missing) references non-existent node 'directive:missing'".to_owned(),
            "Dangling target: edge (directive:a --requires--> directive:missing) references non-existent node 'directive:missing'".to_owned(),
            "Duplicate edge: (directive:a --requires--> directive:missing)".to_owned(),
            "Cycle in requires: directive:a -> directive:b -> directive:a".to_owned(),
            "Profile-edge target must be an agent_profile: edge (agent_profile:p --specializes_from--> tactic:t) has target 'tactic:t' of kind 'tactic'".to_owned(),
            "Rejects-edge target must be an anti_pattern node: edge (directive:a --rejects--> directive:b) has target 'directive:b' of kind 'directive'".to_owned(),
            "Orphaned anti_pattern node: 'anti_pattern:orphan' is marked anti_pattern but has no inbound rejects edge".to_owned(),
        ]
    );
}

#[test]
fn activation_warnings_match_the_reference_exactly() {
    let config = sk_doctrine::YamlDocument::parse("{}\n", Path::new("<memory>")).unwrap();
    let available = vec!["a".to_owned(), "b".to_owned()];
    let effective = vec!["a".to_owned()];
    let plan = plan_activation(
        "directive",
        "b",
        &ActivationInputs {
            yaml_key: "activated_directives",
            available_ids: &available,
            config_data: &config,
            effective_ids: &effective,
            effective_when_absent: sk_doctrine::EffectiveWhenAbsent::All,
            effective_ids_in_force: true,
            narrow_base: None,
            previous_ids: &[],
            deny_key: Some("deactivated_directives"),
            current_deny: &[],
            deny_change: DenyListChoice::Activate,
            effective_deny: &sk_doctrine::DenySet::empty(),
        },
    )
    .unwrap();
    assert_eq!(
        plan.warnings,
        vec![
            "Kind 'directive' had no explicit activation set. Initialized from the 1 artifact(s) already effective, so nothing in force was deactivated. This list is now explicit: artifacts shipped later are not activated automatically; use 'spec-kitty charter activate <kind> <id> --only' to keep activation narrow.".to_owned()
        ]
    );

    let implicit = plan_activation(
        "directive",
        "a",
        &ActivationInputs {
            yaml_key: "activated_directives",
            available_ids: &available,
            config_data: &config,
            effective_ids: &effective,
            effective_when_absent: sk_doctrine::EffectiveWhenAbsent::All,
            effective_ids_in_force: true,
            narrow_base: None,
            previous_ids: &[],
            deny_key: Some("deactivated_directives"),
            current_deny: &[],
            deny_change: DenyListChoice::Activate,
            effective_deny: &sk_doctrine::DenySet::empty(),
        },
    )
    .unwrap();
    assert_eq!(implicit.new_list, None);
    assert_eq!(
        implicit.warnings,
        vec![
            "directive a is already in force; nothing written. A list would freeze directive. Use `spec-kitty charter activate directive a --only` to write one.".to_owned()
        ]
    );
}

//! Acceptance tests for the real `StepContractExecutor`.
//!
//! The expected request text and delegation outcomes are the pinned reference's
//! own output for the shipped `software-dev/implement` contract
//! (CPython 3.11.15, Source `9aed3a80`):
//!
//! ```text
//! bootstrap:
//!   Execute mission step contract implement (software-dev/implement).
//!   Step bootstrap: Load charter context for this action
//!   Declared command: spec-kitty charter context --action implement --json
//!   Command status: declared only; the host/operator owns execution.
//! execute step resolved:
//!   tactic:tdd-red-green-refactor, tactic:acceptance-test-first,
//!   tactic:change-apply-smallest-viable-diff
//! quality_gate resolved: directive:DIRECTIVE_030
//! ```
//!
//! A declared command is guidance: the executor never runs it (`invoke` only
//! records the composition trail), which the request text states explicitly.

use std::fs;

use serde_json::json;
use sk_invocation::PythonString;
use sk_invocation::executor::{
    CatalogProfile, GlossaryObservation, GlossaryScanPort, GovernanceContext, GovernancePort,
    InvocationCatalog, ProfileInvocationExecutor, RegistryCatalog, StaticCatalog,
};
use sk_invocation::step_contract::{
    ActionDoctrine, ContractStep, DelegatesTo, MissionStepContract, StepContractCatalog,
    StepContractExecutionContext, StepContractExecutionError, StepContractExecutor, StepInput,
    action_profile_default, build_request_text, default_profile_role, resolve_step_delegations,
    select_role_fallback,
};
use tempfile::TempDir;

fn py(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

fn catalog_profile(profile_id: &str, role: &str, priority: i32) -> CatalogProfile {
    CatalogProfile {
        profile_id: py(profile_id),
        name: Some(py(profile_id)),
        role: py(role),
        routing_priority: priority,
        preferred_model: None,
        effort: None,
        domain_keywords: Vec::new(),
        canonical_verbs: vec![py("implement")],
    }
}

struct NoGovernance;

impl GovernancePort for NoGovernance {
    fn build_context(
        &self,
        _profile_id: &sk_kernel::PythonString,
        _action: &str,
    ) -> GovernanceContext {
        GovernanceContext::missing()
    }
}

struct NoGlossary;

impl GlossaryScanPort for NoGlossary {
    fn scan(&self, _request: &str, _id: &str, _actor: &str) -> GlossaryObservation {
        GlossaryObservation::empty()
    }
}

/// The shipped `software-dev/implement` contract, step for step.
fn implement_contract() -> MissionStepContract {
    MissionStepContract {
        schema_version: "1.0".to_string(),
        id: "implement".to_string(),
        action: "implement".to_string(),
        mission: "software-dev".to_string(),
        steps: vec![
            ContractStep {
                id: "bootstrap".to_string(),
                description: "Load charter context for this action".to_string(),
                command: Some("spec-kitty charter context --action implement --json".to_string()),
                inputs: Vec::new(),
                delegates_to: None,
                guidance: None,
            },
            ContractStep {
                id: "workspace".to_string(),
                description: "Create or enter the work package workspace".to_string(),
                command: None,
                inputs: Vec::new(),
                delegates_to: Some(DelegatesTo {
                    kind: "paradigm".to_string(),
                    candidates: vec![
                        "execution-lanes".to_string(),
                        "shared-branch-ci".to_string(),
                        "git-flow".to_string(),
                        "trunk-based".to_string(),
                    ],
                }),
                guidance: Some(
                    "Execution lanes are the default and only supported git worktree paradigm.\nIf no branching paradigm is selected in the charter, use execution-lane worktrees.\n"
                        .to_string(),
                ),
            },
            ContractStep {
                id: "execute".to_string(),
                description:
                    "Implement the work package according to the prompt file".to_string(),
                command: None,
                inputs: Vec::new(),
                delegates_to: Some(DelegatesTo {
                    kind: "tactic".to_string(),
                    candidates: vec![
                        "tdd-red-green-refactor".to_string(),
                        "acceptance-test-first".to_string(),
                        "change-apply-smallest-viable-diff".to_string(),
                    ],
                }),
                guidance: None,
            },
            ContractStep {
                id: "quality_gate".to_string(),
                description: "Run tests and type checks".to_string(),
                command: None,
                inputs: Vec::new(),
                delegates_to: Some(DelegatesTo {
                    kind: "directive".to_string(),
                    candidates: vec!["DIRECTIVE_030".to_string()],
                }),
                guidance: None,
            },
        ],
        gates: Vec::new(),
    }
}

/// Every per-invocation Op record under `root` (not the index or spine).
fn op_record_paths(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(root.join("kitty-ops")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".jsonl") && name != "ops-index.jsonl")
        })
        .collect()
}

fn two_step_contract() -> MissionStepContract {
    MissionStepContract {
        schema_version: "1.0".to_string(),
        id: "custom:demo:step".to_string(),
        action: "step".to_string(),
        mission: "demo".to_string(),
        steps: vec![
            ContractStep {
                id: "one".to_string(),
                description: "first".to_string(),
                command: None,
                inputs: Vec::new(),
                delegates_to: None,
                guidance: None,
            },
            ContractStep {
                id: "two".to_string(),
                description: "second".to_string(),
                command: None,
                inputs: vec![StepInput {
                    flag: "--mission".to_string(),
                    source: "mission_slug".to_string(),
                    optional: true,
                }],
                delegates_to: None,
                guidance: None,
            },
        ],
        gates: Vec::new(),
    }
}

#[test]
fn request_text_matches_the_reference_line_for_line() {
    let contract = implement_contract();
    let temp = TempDir::new().expect("temp");
    let context =
        StepContractExecutionContext::new(temp.path(), "software-dev", "implement", "claude");
    let bootstrap = &contract.steps[0];
    assert_eq!(
        build_request_text(&contract, &context, bootstrap, &[], &[]),
        "Execute mission step contract implement (software-dev/implement).\n\
         Step bootstrap: Load charter context for this action\n\
         Declared command: spec-kitty charter context --action implement --json\n\
         Command status: declared only; the host/operator owns execution."
    );
    let workspace = &contract.steps[1];
    // The YAML folded guidance keeps its trailing newline; the builder appends
    // it verbatim (the reference does the same).
    assert_eq!(
        build_request_text(&contract, &context, workspace, &[], &[]),
        "Execute mission step contract implement (software-dev/implement).\n\
         Step workspace: Create or enter the work package workspace\n\
         Step guidance: Execution lanes are the default and only supported git worktree paradigm.\n\
         If no branching paradigm is selected in the charter, use execution-lane worktrees.\n"
    );
    // Declared inputs render with the optional brackets.
    let optional = StepInput {
        flag: "--opt".to_string(),
        source: "o".to_string(),
        optional: true,
    };
    assert_eq!(optional.render(), "[--opt {o}]");
}

#[test]
fn declarative_commands_are_never_executed() {
    // The only effect of a declared command on the executor is the request text:
    // a command that would be destructive if executed is recorded as guidance and
    // the Op chain still completes normally.
    let temp = TempDir::new().expect("temp");
    let mut contract = two_step_contract();
    contract.steps[0].command = Some("rm -rf /tmp/does-not-matter".to_string());
    let catalog = StaticCatalog::new(vec![catalog_profile("demo-impl", "implementer", 50)]);
    let mut invocation = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&NoGovernance)
        .with_glossary(&NoGlossary);
    let contracts = StepContractCatalog::from_contracts(vec![contract]);
    let mut executor = StepContractExecutor::new(temp.path(), &contracts, &mut invocation);
    let context = StepContractExecutionContext {
        repo_root: temp.path(),
        mission: "demo",
        action: "step",
        actor: "claude",
        profile_hint: Some("demo-impl"),
        request_text: None,
        mode_of_work: None,
        resolution_depth: 2,
    };
    let result = executor.execute(&context, None).expect("execute");
    assert_eq!(result.steps.len(), 2);
    assert!(result.steps[0].command_declared);
    assert_eq!(
        op_record_paths(temp.path()).len(),
        2,
        "one Op per contract step"
    );
}

#[test]
fn delegations_resolve_against_the_real_doctrine_graph() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let graph = sk_doctrine::load_graph_or_dir(&repo_root.join("packs/built-in"))
        .expect("built-in graph loads");
    let doctrine = ActionDoctrine::from_graph(&graph, "action:software-dev/implement", 2);
    assert!(
        doctrine.delivered.contains("tactic:acceptance-test-first"),
        "the built-in graph delivers the ATDD tactic"
    );
    let contract = implement_contract();
    let (resolved, unresolved) = resolve_step_delegations(&doctrine, &contract.steps[2]);
    let urns: Vec<&str> = resolved.iter().map(|entry| entry.urn.as_str()).collect();
    assert_eq!(
        urns,
        vec![
            "tactic:tdd-red-green-refactor",
            "tactic:acceptance-test-first",
            "tactic:change-apply-smallest-viable-diff"
        ]
    );
    assert!(unresolved.is_empty());
    // Labels come from the delivered nodes.
    assert_eq!(
        resolved[1].label.as_deref(),
        Some("Acceptance Test Driven Development (ATDD)")
    );
    // A candidate that is not in the action context is unresolved, not fatal.
    let step = ContractStep {
        id: "x".to_string(),
        description: "x".to_string(),
        command: None,
        inputs: Vec::new(),
        delegates_to: Some(DelegatesTo {
            kind: "tactic".to_string(),
            candidates: vec![
                "acceptance-test-first".to_string(),
                "no-such-tactic".to_string(),
            ],
        }),
        guidance: None,
    };
    let (resolved, unresolved) = resolve_step_delegations(&doctrine, &step);
    assert_eq!(resolved.len(), 1);
    assert_eq!(unresolved, vec!["no-such-tactic".to_string()]);
    // The directive candidate resolves through its zero-padded URN.
    let (resolved, unresolved) = resolve_step_delegations(&doctrine, &contract.steps[3]);
    assert_eq!(resolved[0].urn, "directive:DIRECTIVE_030");
    assert!(unresolved.is_empty());
}

#[test]
fn execute_finds_the_contract_by_action_and_closes_each_step_once() {
    let temp = TempDir::new().expect("temp");
    let catalog = StaticCatalog::new(vec![catalog_profile("demo-impl", "implementer", 50)]);
    let mut invocation = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&NoGovernance)
        .with_glossary(&NoGlossary);
    let contracts = StepContractCatalog::from_contracts(vec![two_step_contract()]);
    let mut executor = StepContractExecutor::new(temp.path(), &contracts, &mut invocation);
    let context = StepContractExecutionContext {
        repo_root: temp.path(),
        mission: "demo",
        action: "step",
        actor: "claude",
        profile_hint: Some("demo-impl"),
        request_text: Some("do the thing"),
        mode_of_work: None,
        resolution_depth: 2,
    };
    let result = executor.execute(&context, None).expect("execute");
    assert_eq!(result.contract_id, "custom:demo:step");
    assert_eq!(result.resolution_source, "merged_drg");
    assert_eq!(result.profile_hint, "demo-impl");
    let ids = result.invocation_ids();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    for id in ids {
        let path = temp.path().join("kitty-ops").join(format!("{id}.jsonl"));
        let lines: Vec<serde_json::Value> = fs::read_to_string(path)
            .expect("record")
            .lines()
            .map(|line| serde_json::from_str(line).expect("line"))
            .collect();
        assert_eq!(lines.len(), 2, "one started + one completed for {id}");
        assert_eq!(lines[0]["event"], json!("started"));
        assert_eq!(lines[1]["event"], json!("completed"));
        assert_eq!(lines[1]["outcome"], json!("done"));
    }
    // A declared input is carried as guidance, not executed.
    assert_eq!(result.steps[1].inputs[0].flag, "--mission");
}

#[test]
fn a_missing_contract_is_a_structured_error() {
    let temp = TempDir::new().expect("temp");
    let catalog = StaticCatalog::new(vec![catalog_profile("demo-impl", "implementer", 50)]);
    let mut invocation =
        ProfileInvocationExecutor::new(temp.path(), temp.path()).with_catalog(&catalog);
    let contracts = StepContractCatalog::new();
    let mut executor = StepContractExecutor::new(temp.path(), &contracts, &mut invocation);
    let context = StepContractExecutionContext::new(temp.path(), "demo", "missing", "claude");
    let error = executor.execute(&context, None).expect_err("missing");
    assert!(matches!(error, StepContractExecutionError::NotFound { .. }));
    assert!(!temp.path().join("kitty-ops").exists());
}

#[test]
fn built_in_default_profiles_and_the_role_fallback_match_the_reference() {
    assert_eq!(
        action_profile_default("software-dev", "implement"),
        Some("implementer-ivan")
    );
    assert_eq!(
        action_profile_default("software-dev", "review"),
        Some("reviewer-renata")
    );
    assert_eq!(action_profile_default("custom", "step"), None);
    assert_eq!(
        default_profile_role("implementer-ivan"),
        Some("implementer")
    );
    // Highest routing priority first, then profile id ascending.
    let catalog = vec![
        catalog_profile("implementer-low", "implementer", 10),
        catalog_profile("implementer-b", "implementer", 60),
        catalog_profile("implementer-a", "implementer", 60),
        catalog_profile("reviewer", "reviewer", 99),
    ];
    let chosen = select_role_fallback(&catalog, "implementer").expect("fallback");
    assert_eq!(chosen.scalar_id().as_deref(), Some("implementer-a"));
}

#[test]
fn a_deactivated_default_falls_back_to_a_same_role_profile() {
    let temp = TempDir::new().expect("temp");
    // `implementer-ivan` is absent, so the availability fallback picks the
    // highest-priority same-role profile.
    let catalog = StaticCatalog::new(vec![
        catalog_profile("implementer-zoe", "implementer", 70),
        catalog_profile("implementer-amy", "implementer", 70),
    ]);
    let mut invocation = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&NoGovernance)
        .with_glossary(&NoGlossary);
    let contracts = StepContractCatalog::from_contracts(vec![implement_contract()]);
    let mut executor = StepContractExecutor::new(temp.path(), &contracts, &mut invocation);
    let context =
        StepContractExecutionContext::new(temp.path(), "software-dev", "implement", "claude");
    let result = executor.execute(&context, None).expect("fallback execute");
    assert_eq!(result.profile_hint, "implementer-amy");
    let started: Vec<serde_json::Value> = op_record_paths(temp.path())
        .into_iter()
        .map(|path| {
            let first = fs::read_to_string(path).expect("record");
            serde_json::from_str(first.lines().next().expect("line")).expect("json")
        })
        .collect();
    assert_eq!(started.len(), 4, "four contract steps");
    for line in &started {
        assert_eq!(line["profile_id"], json!("implementer-amy"));
    }
}

#[test]
fn no_available_same_role_profile_is_a_typed_refusal() {
    let temp = TempDir::new().expect("temp");
    // The catalogue has profiles, but none carries the default's role.
    let catalog = StaticCatalog::new(vec![catalog_profile("curator-one", "curator", 50)]);
    let mut invocation = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&NoGovernance)
        .with_glossary(&NoGlossary);
    let contracts = StepContractCatalog::from_contracts(vec![implement_contract()]);
    let mut executor = StepContractExecutor::new(temp.path(), &contracts, &mut invocation);
    let context =
        StepContractExecutionContext::new(temp.path(), "software-dev", "implement", "claude");
    let error = executor.execute(&context, None).expect_err("refused");
    match error {
        StepContractExecutionError::NoAvailableProfile {
            default, role_note, ..
        } => {
            assert_eq!(default, "implementer-ivan");
            assert_eq!(role_note, " (role 'implementer')");
        }
        other => panic!("unexpected error: {other}"),
    }
    assert!(
        !temp.path().join("kitty-ops").exists(),
        "nothing was written"
    );
}

#[test]
fn the_repository_catalogue_routes_over_the_exact_registry_snapshot() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let registry = sk_invocation::catalogue::ProfileRegistry::load(&repo_root)
        .expect("canonical catalogue loads");
    let catalog = RegistryCatalog::new(&registry);
    let routing = catalog.routing_profiles();
    assert!(!routing.is_empty());
    let ivan = catalog
        .profile(&py("implementer-ivan"))
        .expect("implementer-ivan resolves");
    assert_eq!(ivan.scalar_id().as_deref(), Some("implementer-ivan"));
    assert_eq!(
        ivan.role.to_scalar_string().ok().as_deref(),
        Some("implementer")
    );
    assert!(
        ivan.canonical_verbs
            .iter()
            .any(|verb| verb.to_scalar_string().ok().as_deref() == Some("implement"))
    );
    // The routing snapshot is the registry's own exact projection.
    let snapshot = routing
        .iter()
        .find(|profile| profile.profile_id == py("implementer-ivan"))
        .expect("routing entry");
    assert_eq!(snapshot.role, ivan.role);
}

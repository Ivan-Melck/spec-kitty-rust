//! Acceptance tests for the real `ProfileInvocationExecutor` vertical.
//!
//! Every expected payload/piece of text below is the pinned reference's own
//! output (CPython 3.11.15, Source `9aed3a80`), captured before these assertions
//! were written. The Op-record expectations are the existing writer contract
//! (already oracle-bound by `writer_filesystem.rs` / `oracle_vectors.rs`); these
//! tests add the *executor* order on top of it.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sk_invocation::executor::{
    CatalogProfile, CompletionRequest, GlossaryObservation, GlossaryScanPort, GovernanceContext,
    GovernancePort, InvocationRequest, ProfileInvocationExecutor, StaticCatalog,
    build_close_contract, model_routing,
};
use sk_invocation::{ClosedBy, InvocationId, ModeOfWork, OpOutcome, RecordedMode};
use tempfile::TempDir;

/// A one-profile catalogue matching the shipped `implementer-ivan` facts.
fn catalog() -> StaticCatalog {
    StaticCatalog::new(vec![catalog_profile("implementer-ivan")])
}

fn catalog_profile(profile_id: &str) -> CatalogProfile {
    CatalogProfile {
        profile_id: sk_invocation::PythonString::from_scalars(profile_id),
        name: Some(sk_invocation::PythonString::from_scalars(
            "Implementer Ivan",
        )),
        role: sk_invocation::PythonString::from_scalars("implementer"),
        routing_priority: 50,
        preferred_model: None,
        effort: None,
        domain_keywords: vec![sk_invocation::PythonString::from_scalars("implementation")],
        canonical_verbs: ["implement", "fix", "refactor"]
            .iter()
            .map(|verb| sk_invocation::PythonString::from_scalars(verb))
            .collect(),
    }
}

struct Governance {
    text: Option<&'static str>,
}

impl GovernancePort for Governance {
    fn build_context(
        &self,
        _profile_id: &sk_kernel::PythonString,
        _action: &str,
    ) -> GovernanceContext {
        match self.text {
            Some(text) => GovernanceContext {
                text: Some(text.to_string()),
                mode: "bootstrap".to_string(),
            },
            None => GovernanceContext::missing(),
        }
    }
}

struct NoGlossary;

impl GlossaryScanPort for NoGlossary {
    fn scan(&self, _request: &str, _id: &str, _actor: &str) -> GlossaryObservation {
        GlossaryObservation::empty()
    }
}

fn record_root() -> TempDir {
    let temp = TempDir::new().expect("temp");
    // Not a git work tree: writes are admitted and the best-effort commit reports
    // the reference's own "not a git worktree" outcome.
    temp
}

fn started_lines(root: &Path, id: &str) -> Vec<serde_json::Value> {
    let path = root.join("kitty-ops").join(format!("{id}.jsonl"));
    fs::read_to_string(path)
        .expect("record file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("record line JSON"))
        .collect()
}

fn invoke(
    executor: &mut ProfileInvocationExecutor<'_>,
    request_text: &str,
) -> sk_invocation::executor::InvocationPayload {
    let profile_hint = sk_kernel::PythonString::from_scalars("implementer-ivan");
    executor
        .invoke(&InvocationRequest {
            request_text,
            profile_hint: Some(&profile_hint),
            actor: "claude",
            mode_of_work: Some(ModeOfWork::TaskExecution),
            action_hint: Some("implement"),
            mission_id: Some("01M6GC7EV7RY46WMHMXYWS6XZM"),
            wp_id: Some("WP01"),
        })
        .expect("invoke succeeds")
}

#[test]
fn payload_key_set_and_close_contract_match_the_reference() {
    let temp = record_root();
    let catalog = catalog();
    let governance = Governance { text: Some("ctx") };
    let mut executor = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&governance)
        .with_glossary(&NoGlossary);
    let payload = invoke(&mut executor, "implement the work package");

    // Key sets (natively emitted sorted; the reference's order is the struct's).
    let mut open_keys: Vec<String> = payload
        .to_dict()
        .expect("scalar payload")
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect();
    open_keys.sort();
    assert_eq!(
        open_keys,
        vec![
            "action",
            "alternatives",
            "close_contract",
            "empty_charter_fallback",
            "glossary_observations",
            "governance_context_available",
            "governance_context_hash",
            "governance_context_text",
            "invocation_id",
            "profile_friendly_name",
            "profile_id",
            "recommendation",
            "router_confidence",
            "status",
        ]
    );
    assert_eq!(
        payload.to_dict().expect("scalar payload")["status"],
        json!("open")
    );
    assert_eq!(
        payload.to_dict().expect("scalar payload")["profile_friendly_name"],
        json!("Implementer Ivan")
    );
    assert_eq!(
        payload.to_dict().expect("scalar payload")["action"],
        json!("implement")
    );
    assert_eq!(
        payload.to_dict().expect("scalar payload")["router_confidence"],
        json!(null)
    );
    // The hash is the first 16 hex characters of the SHA-256 of the text.
    assert_eq!(
        payload.to_dict().expect("scalar payload")["governance_context_hash"],
        json!(&sk_kernel::content_digest::sha256_hex(b"ctx")[..16])
    );

    // The close contract: evidence is offered only for evidence-eligible modes.
    let contract = build_close_contract("01M6GC7EV7RY46WMHMXYWS6XZM", Some("task_execution"));
    assert_eq!(contract.evidence_flag.as_deref(), Some("--evidence"));
    for ineligible in ["advisory", "query"] {
        let contract = build_close_contract("01M6GC7EV7RY46WMHMXYWS6XZM", Some(ineligible));
        assert_eq!(contract.evidence_flag, None, "{ineligible}");
        let value = serde_json::to_value(&contract).expect("contract JSON");
        assert!(value.get("evidence_flag").is_none(), "{ineligible}");
    }
    assert_eq!(
        build_close_contract("X", Some("mission_step")).command,
        "spec-kitty profile-invocation complete --invocation-id X --outcome <done|failed|abandoned>"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let temp = record_root();
    let catalog = catalog();
    let governance = Governance { text: None };
    let mut executor = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&governance)
        .with_glossary(&NoGlossary);
    let profile_hint = sk_kernel::PythonString::from_scalars("implementer-ivan");
    let payload = executor
        .dry_run(&InvocationRequest {
            request_text: "implement it",
            profile_hint: Some(&profile_hint),
            actor: "claude",
            mode_of_work: None,
            action_hint: Some("implement"),
            mission_id: None,
            wp_id: None,
        })
        .expect("dry run");
    assert_eq!(payload.invocation_id, "");
    let value = payload.to_dry_run_dict().expect("scalar payload");
    assert_eq!(value["status"], json!("dry_run"));
    assert!(value.get("invocation_id").is_none());
    assert!(value.get("close_contract").is_none());
    // `router_confidence` is pinned to "exact" on the dry-run explicit-hint path.
    assert_eq!(value["router_confidence"], json!("exact"));
    assert!(!temp.path().join("kitty-ops").exists());
}

#[test]
fn invoke_then_complete_records_once_with_evidence_and_links() {
    let temp = record_root();
    let catalog = catalog();
    let governance = Governance { text: Some("ctx") };
    let mut executor = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&catalog)
        .with_governance(&governance)
        .with_glossary(&NoGlossary);
    let payload = invoke(&mut executor, "implement the work package");
    let id = payload.invocation_id.clone();
    let started = started_lines(temp.path(), &id);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["event"], json!("started"));
    assert_eq!(started[0]["profile_id"], json!("implementer-ivan"));
    assert_eq!(started[0]["action"], json!("implement"));
    assert_eq!(started[0]["mode_of_work"], json!("task_execution"));
    assert_eq!(
        started[0]["mission_id"],
        json!("01M6GC7EV7RY46WMHMXYWS6XZM")
    );
    assert_eq!(started[0]["wp_id"], json!("WP01"));

    let evidence_path = temp.path().join("note.md");
    fs::write(&evidence_path, "the real evidence\n").expect("evidence file");
    let artifacts = vec!["kitty-specs/mission/spec.md".to_string()];
    let completion = executor
        .complete_invocation(&CompletionRequest {
            invocation_id: &id,
            outcome: OpOutcome::Done,
            evidence_ref: Some(evidence_path.to_str().expect("utf8")),
            artifact_refs: &artifacts,
            commit_sha: Some("0123456789012345678901234567890123456789"),
            closed_by: ClosedBy::Agent,
        })
        .expect("close");
    assert_eq!(completion.completed.outcome(), OpOutcome::Done);
    let lines = started_lines(temp.path(), &id);
    assert_eq!(
        lines.len(),
        4,
        "started, completed, artifact_link, commit_link"
    );
    assert_eq!(lines[1]["event"], json!("completed"));
    assert_eq!(lines[1]["closed_by"], json!("agent"));
    assert_eq!(lines[2]["event"], json!("artifact_link"));
    assert_eq!(lines[3]["event"], json!("commit_link"));
    // The evidence artifact is the reference layout, with a record snapshot.
    let directory = temp.path().join(".kittify/evidence").join(&id);
    assert_eq!(
        fs::read_to_string(directory.join("evidence.md")).unwrap(),
        "the real evidence\n"
    );
    let snapshot: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(directory.join("record.json")).unwrap()).unwrap();
    assert_eq!(snapshot["invocation_id"], json!(id));
    assert_eq!(snapshot["outcome"], json!("done"));
    // A non-work-tree record root is the reference's "not a git worktree" commit
    // outcome: the record stays on disk and the outcome is reported, not hidden.
    let commit = executor.last_op_commit().expect("commit outcome recorded");
    assert!(!commit.committed());
    assert_eq!(commit.refusal(), Some("not a git worktree"));
}

#[test]
fn second_close_is_refused() {
    let temp = record_root();
    let catalog = catalog();
    let mut executor =
        ProfileInvocationExecutor::new(temp.path(), temp.path()).with_catalog(&catalog);
    let payload = invoke(&mut executor, "implement");
    let id = payload.invocation_id.clone();
    executor
        .complete_invocation(&CompletionRequest::agent(&id, OpOutcome::Done))
        .expect("first close");
    let error = executor
        .complete_invocation(&CompletionRequest::agent(&id, OpOutcome::Done))
        .expect_err("second close refuses");
    assert!(
        error.to_string().contains("already closed"),
        "unexpected error: {error}"
    );
}

#[test]
fn evidence_is_refused_for_an_ineligible_mode() {
    let temp = record_root();
    let catalog = catalog();
    let mut executor =
        ProfileInvocationExecutor::new(temp.path(), temp.path()).with_catalog(&catalog);
    let profile_hint = sk_kernel::PythonString::from_scalars("implementer-ivan");
    let payload = executor
        .invoke(&InvocationRequest {
            request_text: "an advisory request",
            profile_hint: Some(&profile_hint),
            actor: "claude",
            mode_of_work: Some(ModeOfWork::Advisory),
            action_hint: Some("implement"),
            mission_id: None,
            wp_id: None,
        })
        .expect("invoke");
    let id = payload.invocation_id.clone();
    let error = executor
        .complete_invocation(&CompletionRequest {
            invocation_id: &id,
            outcome: OpOutcome::Done,
            evidence_ref: Some("inline evidence"),
            artifact_refs: &[],
            commit_sha: None,
            closed_by: ClosedBy::Agent,
        })
        .expect_err("advisory must refuse evidence");
    assert!(matches!(
        error,
        sk_invocation::executor::ExecutorError::InvalidModeForEvidence { .. }
    ));
    // Nothing was written for the refused close.
    assert_eq!(started_lines(temp.path(), &id).len(), 1);
    // The Op is still closable without evidence.
    executor
        .complete_invocation(&CompletionRequest::agent(&id, OpOutcome::Failed))
        .expect("closable");
}

#[test]
fn protected_main_refuses_before_any_write() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    fs::write(root.join("README.md"), "x\n").expect("readme");
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    // A resolvable origin/HEAD is what makes `main` the *primary* branch; without
    // it the reference (and this port) treats the repository as one that keeps no
    // Op records and admits the write.
    let remote = temp.path().join("remote.git");
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(["init", "-q", "--bare", remote.to_str().expect("utf8")])
        .output()
        .expect("git init bare");
    assert!(output.status.success());
    git(&["remote", "add", "origin", remote.to_str().expect("utf8")]);
    git(&["push", "-qu", "origin", "main"]);
    git(&["remote", "set-head", "origin", "main"]);
    // The protected-branch refusal only applies where the primary branch *tracks*
    // Op records (#6410 skip order): a repository that keeps none is admitted and
    // simply never commits the record.
    fs::create_dir_all(root.join("kitty-ops")).expect("kitty-ops");
    fs::write(
        root.join("kitty-ops/01M6GC7EV7RY46WMHMXYWS6XZM.jsonl"),
        "{\"event\":\"started\"}\n",
    )
    .expect("tracked op record");
    git(&["add", "-A"]);
    git(&["commit", "-qm", "track op records"]);
    git(&["push", "-q", "origin", "main"]);

    let catalog = catalog();
    let mut executor = ProfileInvocationExecutor::new(root, root).with_catalog(&catalog);
    let profile_hint = sk_kernel::PythonString::from_scalars("implementer-ivan");
    let error = executor
        .invoke(&InvocationRequest {
            request_text: "implement on main",
            profile_hint: Some(&profile_hint),
            actor: "claude",
            mode_of_work: Some(ModeOfWork::TaskExecution),
            action_hint: Some("implement"),
            mission_id: None,
            wp_id: None,
        })
        .expect_err("protected main refuses the Op write");
    assert!(matches!(
        error,
        sk_invocation::executor::ExecutorError::OpWriteRefused(_)
    ));
    let records: Vec<String> = fs::read_dir(root.join("kitty-ops"))
        .expect("kitty-ops")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        records,
        vec!["01M6GC7EV7RY46WMHMXYWS6XZM.jsonl".to_string()],
        "admission refuses before any new kitty-ops write"
    );
}

#[test]
fn hint_action_hint_wins_and_empty_hint_falls_back_to_the_role_default() {
    let temp = record_root();
    let catalog = catalog();
    let mut executor =
        ProfileInvocationExecutor::new(temp.path(), temp.path()).with_catalog(&catalog);
    let explicit = invoke(&mut executor, "implement");
    assert_eq!(explicit.action, "implement");
    // An empty action hint is treated as absent (EDGE-005) and derives the role
    // default (`implementer` -> "generate"), exactly like the reference.
    let profile_hint = sk_kernel::PythonString::from_scalars("implementer-ivan");
    let derived = executor
        .invoke(&InvocationRequest {
            request_text: "anything",
            profile_hint: Some(&profile_hint),
            actor: "claude",
            mode_of_work: None,
            action_hint: Some(""),
            mission_id: None,
            wp_id: None,
        })
        .expect("invoke");
    assert_eq!(derived.action, "generate");
}

#[test]
fn advisory_recommendation_is_real_for_a_configured_catalog() {
    // A minimal catalog whose `code-implementation` row scores one model.
    let catalog: model_routing::ModelRoutingCatalog = serde_json::from_value(json!({
        "schema_version": "1.0",
        "routing_policy": {
            "objective": "quality_first",
            "weights": {"quality": 0.4, "cost": 0.2, "risk": 0.2, "latency": 0.2},
            "override_policy": {"mode": "advisory"}
        },
        "models": [
            {
                "id": "strong-model",
                "cost": {"tier": "high"},
                "latency_tier": "medium",
                "task_fit": [
                    {"task_type": "code-implementation", "score": 0.9, "confidence": "high", "rationale": "best fit"}
                ]
            }
        ]
    }))
    .expect("catalog");

    let temp = record_root();
    let profiles = catalog_for("implementer-ivan");
    let mut executor = ProfileInvocationExecutor::new(temp.path(), temp.path())
        .with_catalog(&profiles)
        .with_model_catalog(&catalog);
    let payload = invoke(&mut executor, "implement the thing");
    let recommendation = payload.recommendation.expect("recommendation");
    assert_eq!(recommendation.task_type, "code-implementation");
    assert_eq!(recommendation.objective, "quality_first");
    let candidate = recommendation.catalog_candidate().expect("catalog pick");
    assert_eq!(candidate.model_id, "strong-model");
    assert_eq!(candidate.source, "catalog");
    // The durable started record carries the catalog candidate's model id.
    let started = started_lines(temp.path(), &payload.invocation_id);
    assert_eq!(started[0]["model_id"], json!("strong-model"));
}

fn catalog_for(profile_id: &str) -> StaticCatalog {
    StaticCatalog::new(vec![catalog_profile(profile_id)])
}

#[test]
fn recorded_mode_classification_keeps_three_states() {
    assert!(RecordedMode::Absent.permits_evidence());
    assert!(!RecordedMode::Known(ModeOfWork::Query).permits_evidence());
    assert!(!RecordedMode::Undetermined(json!("nonsense")).permits_evidence());
}

#[test]
fn invocation_ids_are_valid_ulids_on_disk() {
    let temp = record_root();
    let catalog = catalog();
    let mut executor =
        ProfileInvocationExecutor::new(temp.path(), temp.path()).with_catalog(&catalog);
    let payload = invoke(&mut executor, "implement");
    let parsed = InvocationId::parse(&payload.invocation_id).expect("valid ULID");
    assert_eq!(parsed.as_str(), payload.invocation_id);
    let path: PathBuf = temp
        .path()
        .join("kitty-ops")
        .join(format!("{}.jsonl", payload.invocation_id));
    assert!(path.is_file());
}

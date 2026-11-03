//! CLI mutation-route tests: the canonical `agent status emit`,
//! `agent tasks move-task`, `agent tasks mark-status` and
//! `agent status lifecycle` bodies, driven through the library entry points the
//! dispatcher calls.
//!
//! The route functions own every decision, payload and refusal; the dispatcher
//! only selects them. Testing them directly therefore exercises the real route
//! without depending on the (separately owned) `compat/mod.rs` registration
//! patch, which is supplied verbatim in `CONTINUATION-REPORT.md`.
//!
//! Every case uses a throwaway project root: no test reads or writes this
//! checkout, the user's `HOME`, or a real mission.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sk_cli::compat::status::{self, EmitArgs};
use sk_cli::compat::support::StatusHeadCommittedProbe;
use sk_cli::compat::tasks::{self, MarkStatusArgs, MoveTaskArgs};
use sk_status::HeadCommittedProbe;

const SLUG: &str = "034-cli-mutation-fixture";

struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
    feature: PathBuf,
    slug: String,
}

impl Project {
    fn new() -> Self {
        Self::named(SLUG)
    }

    fn named(slug: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let feature = root.join("kitty-specs").join(slug);
        fs::create_dir_all(feature.join("tasks")).unwrap();
        fs::create_dir_all(root.join(".kittify")).unwrap();
        fs::write(feature.join("tasks.md"), "# Tasks\n").unwrap();
        fs::write(
            feature.join("meta.json"),
            serde_json::to_string_pretty(&json!({
                "mission_slug": slug,
                "mission_id": "01K9M0R3W00000000000000000",
                "mission_number": null,
                "mission_type": "software-dev",
                "target_branch": "main",
            }))
            .unwrap(),
        )
        .unwrap();
        write_wp(&feature, "WP01", &["T001", "T002"], &[]);
        write_wp(&feature, "WP02", &["T003"], &["WP01"]);
        Self {
            _temp: temp,
            root,
            feature,
            slug: slug.to_owned(),
        }
    }

    fn rows(&self) -> Vec<Value> {
        let path = self.feature.join("status.events.jsonl");
        fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("json row"))
            .collect()
    }

    fn lane(&self, wp: &str) -> String {
        let rows = self.rows();
        let mut lane = "genesis".to_owned();
        for row in rows {
            if row["wp_id"] == json!(wp)
                && let Some(target) = row["to_lane"].as_str()
            {
                lane = target.to_owned();
            }
        }
        lane
    }

    fn seed(&self, wp: &str, lane: &str) {
        let mut request = sk_status::mutation::TransitionRequest::new(
            &self.feature,
            &self.slug,
            wp,
            lane,
            sk_status::mutation::ActorValue::identity("claude"),
        );
        request.repo_root = Some(self.root.clone());
        request.workspace_context = Some("worktree:/tmp".to_owned());
        request.subtasks_complete = Some(true);
        request.implementation_evidence_present = Some(true);
        request.evidence = matches!(lane, "approved" | "done").then(
            || json!({"review": {"reviewer": "alice", "verdict": "approved", "reference": "PR#1"}}),
        );
        sk_status::mutation::emit_status_transition(&request)
            .unwrap_or_else(|error| panic!("seed {wp} -> {lane}: {error}"));
    }

    fn walk(&self, wp: &str, lanes: &[&str]) {
        for lane in lanes {
            self.seed(wp, lane);
        }
    }

    /// Initialise a Git checkout with one commit (the gates are Git-derived).
    fn git_init(&self) -> String {
        let run = |arguments: &[&str]| -> String {
            let output = std::process::Command::new("git")
                .current_dir(&self.root)
                .args(arguments)
                .env("GIT_AUTHOR_NAME", "QA")
                .env("GIT_AUTHOR_EMAIL", "qa@example.invalid")
                .env("GIT_COMMITTER_NAME", "QA")
                .env("GIT_COMMITTER_EMAIL", "qa@example.invalid")
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {arguments:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        run(&["init", "-q", "-b", "main", "."]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
        run(&["rev-parse", "HEAD"])
    }

    fn git(&self, arguments: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(&self.root)
            .args(arguments)
            .env("GIT_AUTHOR_NAME", "QA")
            .env("GIT_AUTHOR_EMAIL", "qa@example.invalid")
            .env("GIT_COMMITTER_NAME", "QA")
            .env("GIT_COMMITTER_EMAIL", "qa@example.invalid")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn write_lanes(&self, lane_id: &str, planning_commit: Option<&str>) {
        self.write_lanes_for(&self.slug, lane_id, planning_commit);
    }

    /// Write a **canonical** manifest: every field the upstream `to_dict` emits,
    /// including the `write_scope`/`predicted_surfaces` the lane model requires.
    fn write_lanes_for(&self, slug: &str, lane_id: &str, planning_commit: Option<&str>) {
        let value = json!({
            "version": 1,
            "mission_slug": slug,
            "mission_id": "01K9M0R3W00000000000000000",
            "mission_branch": format!("kitty/mission-{slug}"),
            "target_branch": "main",
            "lanes": [{
                "lane_id": lane_id,
                "wp_ids": ["WP01"],
                "write_scope": ["src/**"],
                "predicted_surfaces": [],
                "depends_on_lanes": [],
                "parallel_group": 0,
            }],
            "computed_at": "2026-11-03T00:00:00Z",
            "computed_from": "test",
            "planning_artifact_wps": [],
            "planning_commit_sha": planning_commit,
        });
        fs::write(
            self.feature.join("lanes.json"),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    /// Write `.kittify/config.yaml` (a real YAML document the CLI parses).
    fn write_config(&self, text: &str) {
        fs::create_dir_all(self.root.join(".kittify")).unwrap();
        fs::write(self.root.join(".kittify/config.yaml"), text).unwrap();
    }

    /// Set one `meta.json` field.
    fn set_meta(&self, key: &str, value: Value) {
        let path = self.feature.join("meta.json");
        let mut meta: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        meta.as_object_mut()
            .expect("meta object")
            .insert(key.to_owned(), value);
        fs::write(&path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    }

    /// Install `.git/hooks/<name>` with an executable body.
    fn install_hook(&self, name: &str, body: &str) {
        let hooks = self.root.join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        let path = hooks.join(name);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn status_porcelain(&self) -> String {
        self.git(&["status", "--porcelain", "-uall"])
    }

    /// An isolated `HOME` for real-binary runs.
    fn home(&self) -> PathBuf {
        let home = self._temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        home
    }

    /// Drop one `meta.json` field (the legacy meta shape).
    fn remove_meta(&self, key: &str) {
        let path = self.feature.join("meta.json");
        let mut meta: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        meta.as_object_mut().expect("meta object").remove(key);
        fs::write(&path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    }
}

/// Run the real `spec-kitty` binary from `root` with an isolated home.
fn run_cli(root: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let packs = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("built-in pack tree");
    std::process::Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(root)
        .env("HOME", home)
        .env("SPEC_KITTY_PACKS_ROOT", packs)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("run spec-kitty")
}

/// Parse the single JSON document a `--json` route writes to stdout.
fn single_json(output: &std::process::Output) -> Value {
    let text = String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8");
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "stdout was not one JSON document ({error}):\n{text}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A WP whose frontmatter declares `owned_files` (for the evidence window).
fn write_wp_owned(feature: &Path, wp: &str, owned: &[&str]) {
    let owned = owned
        .iter()
        .map(|glob| format!("\"{glob}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        feature.join("tasks").join(format!("{wp}.md")),
        format!(
            "---\nwork_package_id: {wp}\ntitle: {wp} title\ndependencies: []\nowned_files: [{owned}]\nsubtasks: []\n---\n\n# {wp}\n"
        ),
    )
    .unwrap();
}

fn write_wp(feature: &Path, wp: &str, subtasks: &[&str], dependencies: &[&str]) {
    let subtasks = if subtasks.is_empty() {
        "[]".to_owned()
    } else {
        format!(
            "\n{}",
            subtasks
                .iter()
                .map(|id| format!("- {id}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let dependencies = if dependencies.is_empty() {
        "[]".to_owned()
    } else {
        format!(
            "[{}]",
            dependencies
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    fs::write(
        feature.join("tasks").join(format!("{wp}.md")),
        format!(
            "---\nwork_package_id: {wp}\ntitle: {wp} title\ndependencies: {dependencies}\nsubtasks: {subtasks}\n---\n\n# {wp}\n"
        ),
    )
    .unwrap();
}

fn emit_args(to: &str) -> EmitArgs {
    EmitArgs {
        wp_id: Some("WP01".to_owned()),
        wp: None,
        to: to.to_owned(),
        actor: "claude".to_owned(),
        mission: Some(SLUG.to_owned()),
        force: false,
        reason: None,
        evidence_json: None,
        review_ref: None,
        review_result_json: None,
        workspace_context: None,
        subtasks_complete: None,
        implementation_evidence_present: None,
        execution_mode: "worktree".to_owned(),
        json: true,
    }
}

fn move_args(to: &str) -> MoveTaskArgs {
    MoveTaskArgs {
        task_id: "WP01".to_owned(),
        to: to.to_owned(),
        mission: Some(SLUG.to_owned()),
        agent: Some("claude".to_owned()),
        model: None,
        profile: None,
        invocation_id: None,
        assignee: None,
        shell_pid: None,
        note: None,
        review_feedback_file: None,
        approval_ref: None,
        reviewer: None,
        self_review_fallback: false,
        intended_reviewer: None,
        reviewer_failure_reason: None,
        done_override_reason: None,
        force: false,
        tracker_ref: Vec::new(),
        skip_review_artifact_check: false,
        auto_commit: None,
        no_auto_commit: Some(1),
        skip_pre_review_gate: false,
        owned_checkout: None,
        json: true,
    }
}

#[test]
fn status_emit_moves_a_wp_and_returns_the_upstream_payload() {
    let project = Project::new();
    project.seed("WP01", "planned");

    let payload = status::emit(&project.root, &emit_args("claimed")).expect("emit");
    assert_eq!(
        payload,
        json!({
            "event_id": payload["event_id"],
            "wp_id": "WP01",
            "work_package_id": "WP01",
            "from_lane": "planned",
            "to_lane": "claimed",
            "status_events_path": project.feature.join("status.events.jsonl").display().to_string(),
            "actor": "claude",
        })
    );

    let rows = project.rows();
    assert_eq!(rows.len(), 2);
    let event = &rows[1];
    assert_eq!(event["to_lane"], json!("claimed"));
    // `--actor claude` records the structured 4-key actor, role from the hop.
    assert_eq!(
        event["actor"],
        json!({"role": "implementer", "profile": null, "tool": "claude", "model": null})
    );
    assert_eq!(project.lane("WP01"), "claimed");
}

#[test]
fn status_emit_refuses_a_seed_shaped_actor_before_any_write() {
    let project = Project::new();
    let mut args = emit_args("planned");
    args.actor = "migration:seed-writer".to_owned();
    let error = status::emit(&project.root, &args).expect_err("seed refused");
    assert!(
        error.message().contains("reconstruction seeds"),
        "got {}",
        error.message()
    );
    assert!(project.rows().is_empty(), "nothing was written");
}

#[test]
fn status_emit_refuses_a_coordination_routed_mission_instead_of_writing_primary() {
    let project = Project::new();
    let meta = project.feature.join("meta.json");
    let mut value: Value = serde_json::from_str(&fs::read_to_string(&meta).unwrap()).unwrap();
    value["coordination_branch"] = json!("kitty/mission-034-cli-mutation-fixture-01K9M0R3");
    fs::write(&meta, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    let error =
        status::emit(&project.root, &emit_args("planned")).expect_err("coordination refused");
    assert_eq!(
        error.to_json()["error_code"],
        json!("COORDINATION_SURFACE_UNAVAILABLE")
    );
    assert!(!project.feature.join("status.events.jsonl").exists());
}

#[test]
fn status_emit_validates_the_positional_and_wp_selectors_and_review_json() {
    let project = Project::new();
    let mut args = emit_args("planned");
    args.wp = Some("WP02".to_owned());
    assert!(status::emit(&project.root, &args).is_err());

    let mut args = emit_args("planned");
    args.actor = "claude".to_owned();
    args.review_result_json = Some("{\"reviewer\": \"alice\"}".to_owned());
    let error = status::emit(&project.root, &args).expect_err("incomplete review result");
    assert!(error.message().contains("reviewer, verdict, and reference"));

    args.review_result_json = Some(
        "{\"reviewer\": \"alice\", \"verdict\": \"maybe\", \"reference\": \"PR#1\"}".to_owned(),
    );
    let error = status::emit(&project.root, &args).expect_err("bad verdict");
    assert!(
        error
            .message()
            .contains("must be 'approved' or 'changes_requested'")
    );
    assert!(project.rows().is_empty());
}

#[test]
fn move_task_records_a_rejection_with_feedback_and_a_changes_requested_verdict() {
    let project = Project::new();
    project.walk(
        "WP01",
        &[
            "planned",
            "claimed",
            "in_progress",
            "for_review",
            "in_review",
        ],
    );
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "T001 needs a retry policy.\n").unwrap();

    let mut args = move_args("planned");
    args.review_feedback_file = Some(feedback.clone());
    let payload = tasks::move_task(&project.root, &args).expect("rollback");
    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["old_lane"], json!("in_review"));
    assert_eq!(payload["new_lane"], json!("planned"));
    let pointer = "review-cycle://034-cli-mutation-fixture/WP01/review-cycle-1.md";
    assert_eq!(payload["review_feedback"], json!(pointer));
    assert_eq!(payload["verdict_durably_persisted"], json!(false));
    assert_eq!(payload["durability_classification"], json!("local_only"));
    assert_eq!(payload["durability_reason"], json!("no_auto_commit"));

    // The canonical artifact is written locally (no commit without auto-commit)
    // and carries the reviewer's feedback verbatim.
    let artifact = project.feature.join("tasks/WP01/review-cycle-1.md");
    let artifact_bytes = fs::read_to_string(&artifact).expect("review-cycle artifact");
    assert!(artifact_bytes.starts_with("---\naffected_files: []\ncycle_number: 1\n"));
    assert!(artifact_bytes.contains("reviewer_agent: claude\n"));
    assert!(artifact_bytes.ends_with("---\n\nT001 needs a retry policy.\n"));
    // The user's feedback file is preserved, never deleted.
    assert_eq!(
        fs::read_to_string(&feedback).unwrap(),
        "T001 needs a retry policy.\n"
    );

    let rows = project.rows();
    let event = rows
        .iter()
        .find(|row| row["to_lane"] == json!("planned") && row["from_lane"] == json!("in_review"))
        .expect("rejection row");
    assert_eq!(
        event["review_result"]["verdict"],
        json!("changes_requested")
    );
    assert_eq!(event["review_result"]["reference"], json!(pointer));
    assert_eq!(event["review_ref"], json!(pointer));
    assert_eq!(project.lane("WP01"), "planned");
}

#[test]
fn move_task_refuses_a_rejection_without_a_rationale() {
    let project = Project::new();
    project.walk("WP01", &["planned", "claimed", "in_progress"]);
    // Canonical `_guard_planned_rollback`: the whole review-rejection family
    // needs a substantive `--review-feedback-file`; a missing rationale is the
    // feedback refusal, and `--force` does not lift it.
    let before = project.rows().len();
    let error = tasks::move_task(&project.root, &move_args("planned"))
        .expect_err("review feedback required");
    assert!(
        error
            .message()
            .contains("Moving WP01 to 'planned' requires review feedback."),
        "got {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("This requirement cannot be bypassed with --force.")
    );
    assert_eq!(project.lane("WP01"), "in_progress");
    assert_eq!(project.rows().len(), before, "the refusal writes no row");
}

#[test]
fn move_task_records_approval_evidence_and_the_review_verdict() {
    let project = Project::new();
    project.walk(
        "WP01",
        &[
            "planned",
            "claimed",
            "in_progress",
            "for_review",
            "in_review",
        ],
    );

    let mut args = move_args("approved");
    // The implementer of record is the seeded `claude`, so the approver must be
    // a provably different identity (#6715).
    args.agent = Some("bob".to_owned());
    args.reviewer = Some("alice".to_owned());
    args.approval_ref = Some("PR#42".to_owned());
    args.profile = Some("reviewer-renata".to_owned());
    args.model = Some("opus".to_owned());
    let payload = tasks::move_task(&project.root, &args).expect("approve");
    assert_eq!(payload["to_lane"], json!("approved"));

    let rows = project.rows();
    let event = rows
        .iter()
        .find(|row| row["to_lane"] == json!("approved"))
        .expect("approval row");
    let pointer = "review-cycle://034-cli-mutation-fixture/WP01/review-cycle-1.md";
    assert_eq!(payload["review_feedback"], json!(pointer));
    assert_eq!(payload["verdict_durably_persisted"], json!(false));
    assert_eq!(payload["durability_classification"], json!("local_only"));
    assert_eq!(event["review_result"]["verdict"], json!("approved"));
    assert_eq!(event["review_result"]["reviewer"], json!("alice"));
    assert_eq!(event["evidence"]["review"]["reference"], json!(pointer));
    assert_eq!(event["review_ref"], json!(pointer));
    // The committed-when-enabled artifact records the approval reference.
    let artifact = project.feature.join("tasks/WP01/review-cycle-1.md");
    let artifact_bytes = fs::read_to_string(&artifact).expect("approval artifact");
    assert!(artifact_bytes.contains("reproduction_command: spec-kitty agent tasks move-task WP01 --to approved --mission 034-cli-mutation-fixture --agent alice\n"));
    assert!(artifact_bytes.ends_with("---\n\nApproved by alice: PR#42\n"));
    // FR-006: the gate-side decision sidecar is never null on an approval.
    assert_eq!(
        event["policy_metadata"],
        json!({"tool": "alice", "profile": "reviewer-renata", "model": "opus"})
    );
    assert_eq!(project.lane("WP01"), "approved");
}

#[test]
fn move_task_requires_the_documented_done_override() {
    let project = Project::new();
    project.walk("WP01", &["planned", "claimed", "in_progress", "approved"]);
    let error = tasks::move_task(&project.root, &move_args("done")).expect_err("done refused");
    assert_eq!(
        error.to_json()["error_code"],
        json!("DONE_ANCESTRY_UNVERIFIED")
    );

    let mut args = move_args("done");
    args.agent = Some("bob".to_owned());
    args.done_override_reason = Some("branch merged out of band".to_owned());
    args.approval_ref = Some("PR#7".to_owned());
    let payload = tasks::move_task(&project.root, &args).expect("documented override");
    assert_eq!(payload["to_lane"], json!("done"));
    assert_eq!(project.lane("WP01"), "done");
}

#[test]
fn move_task_records_the_claim_sidecar_and_off_axis_deltas() {
    let project = Project::new();
    project.seed("WP01", "planned");
    let mut args = move_args("claimed");
    args.shell_pid = Some("4242".to_owned());
    args.assignee = Some("reed".to_owned());
    args.note = Some("claiming for implementation".to_owned());
    args.tracker_ref = vec!["#1298".to_owned()];
    tasks::move_task(&project.root, &args).expect("claim");

    let rows = project.rows();
    let claim = rows
        .iter()
        .find(|row| row["to_lane"] == json!("claimed"))
        .expect("claim row");
    assert_eq!(claim["policy_metadata"]["shell_pid"], json!(4242));
    assert_eq!(claim["policy_metadata"]["agent"], json!("claude"));
    assert_eq!(claim["policy_metadata"]["shell_pid_created_at"], json!(""));

    let annotation = rows
        .iter()
        .find(|row| row["kind"] == json!("annotation"))
        .expect("annotation row");
    assert_eq!(annotation["delta"]["assignee"], json!("reed"));
    assert_eq!(
        annotation["delta"]["note"],
        json!("claiming for implementation")
    );
    assert_eq!(annotation["delta"]["tracker_refs"], json!(["#1298"]));
}

#[test]
fn move_task_refuses_an_owned_checkout_instead_of_pretending_to_support_it() {
    let project = Project::new();
    let mut args = move_args("planned");
    args.owned_checkout = Some(PathBuf::from("/tmp/owned"));
    let error = tasks::move_task(&project.root, &args).expect_err("owned refused");
    assert_eq!(
        error.to_json()["error_code"],
        json!("OWNED_CHECKOUT_UNAVAILABLE")
    );
}

#[test]
fn mark_status_groups_by_owning_wp_and_is_idempotent() {
    let project = Project::new();
    let args = MarkStatusArgs {
        task_ids: vec!["T001".to_owned(), "T002".to_owned()],
        status: "done".to_owned(),
        mission: Some(SLUG.to_owned()),
        wp: Some("WP01".to_owned()),
        agent: Some("claude".to_owned()),
        auto_commit: None,
        no_auto_commit: Some(1),
        owned_checkout: None,
        json: true,
    };
    let payload = tasks::mark_status(&project.root, &args).expect("mark done");
    assert_eq!(
        payload["summary"],
        json!({"updated": 2, "already_satisfied": 0, "not_found": 0})
    );
    let annotations: Vec<Value> = project
        .rows()
        .into_iter()
        .filter(|row| row["kind"] == json!("annotation"))
        .collect();
    assert_eq!(
        annotations.len(),
        1,
        "one delta per WP, never one per task id"
    );
    assert_eq!(
        annotations[0]["delta"],
        json!({"subtasks": {"T001": "done", "T002": "done"}})
    );

    let again = tasks::mark_status(&project.root, &args).expect("idempotent");
    assert_eq!(
        again["summary"],
        json!({"updated": 0, "already_satisfied": 2, "not_found": 0})
    );
    assert_eq!(
        project
            .rows()
            .iter()
            .filter(|row| row["kind"] == json!("annotation"))
            .count(),
        1,
        "the already-satisfied pass writes nothing"
    );
}

#[test]
fn mark_status_records_the_not_applicable_alias_as_skipped_and_reports_unknown_ids() {
    let project = Project::new();
    let args = MarkStatusArgs {
        task_ids: vec!["T003".to_owned(), "T999".to_owned()],
        status: "not_applicable".to_owned(),
        mission: Some(SLUG.to_owned()),
        wp: None,
        agent: None,
        auto_commit: None,
        no_auto_commit: Some(1),
        owned_checkout: None,
        json: true,
    };
    let payload = tasks::mark_status(&project.root, &args).expect("mark");
    assert_eq!(
        payload["summary"],
        json!({"updated": 1, "already_satisfied": 0, "not_found": 1})
    );
    let annotation = project
        .rows()
        .into_iter()
        .find(|row| row["kind"] == json!("annotation"))
        .expect("annotation");
    assert_eq!(annotation["wp_id"], json!("WP02"));
    assert_eq!(
        annotation["delta"],
        json!({"subtasks": {"T003": "skipped"}})
    );
}

#[test]
fn mark_status_refuses_an_ambiguous_roster_without_wp() {
    let project = Project::new();
    write_wp(&project.feature, "WP03", &["T003"], &[]);
    let args = MarkStatusArgs {
        task_ids: vec!["T003".to_owned()],
        status: "done".to_owned(),
        mission: Some(SLUG.to_owned()),
        wp: None,
        agent: None,
        auto_commit: None,
        no_auto_commit: Some(1),
        owned_checkout: None,
        json: true,
    };
    let error = tasks::mark_status(&project.root, &args).expect_err("ambiguous");
    assert!(
        error.message().contains("pass --wp to select one"),
        "got {}",
        error.message()
    );
    assert!(project.rows().is_empty());
}

#[test]
fn status_lifecycle_reports_the_derived_state_and_counts() {
    let project = Project::new();
    let active = status::lifecycle(&project.root, Some(SLUG), true).expect("lifecycle");
    assert_eq!(active["state"], json!("recoverable"));
    assert_eq!(active["total_wps"], json!(0));
    assert_eq!(active["has_event_log"], json!(false));

    project.walk("WP01", &["planned", "claimed", "in_progress"]);
    let active = status::lifecycle(&project.root, Some(SLUG), true).expect("lifecycle");
    assert_eq!(active["state"], json!("active"));
    assert_eq!(active["surface_state"], json!("active"));
    assert_eq!(active["total_wps"], json!(1));
    assert_eq!(active["active_wp_count"], json!(1));
    assert_eq!(active["terminal_wp_count"], json!(0));
    assert!(active["completion_pct"].as_f64().unwrap() > 0.0);
    assert_eq!(active["stale_after_days"], json!(14));
}

#[test]
fn status_lifecycle_marks_a_reopened_mission_actionable() {
    let project = Project::new();
    project.walk(
        "WP01",
        &["planned", "claimed", "in_progress", "approved", "done"],
    );
    // Freshly terminal: within the three-day completion window.
    let archived = status::lifecycle(&project.root, Some(SLUG), true).expect("lifecycle");
    assert_eq!(archived["state"], json!("recently_completed"));

    let mut request = sk_status::mutation::LifecycleRequest::new(
        sk_status::mutation::mission_event_log_path(&project.feature),
        "MissionReopened",
        json!({
            "mission_id": "01K9M0R3W00000000000000000",
            "mission_slug": SLUG,
            "reason": "operator reopened",
            "reopened_by": "alice",
            "reopened_at": "2026-11-02T12:00:00Z",
            "cleared_merge": null,
        }),
        "01K9M0R3W00000000000000000",
        "Mission",
    );
    request.mission_slug = Some(SLUG.to_owned());
    request.repo_root = Some(project.root.clone());
    sk_status::mutation::persist_lifecycle_event_local(&request).expect("reopen");

    let reopened = status::lifecycle(&project.root, Some(SLUG), true).expect("lifecycle");
    assert_eq!(reopened["state"], json!("reopened"));
    assert_eq!(reopened["post_mission_events"].as_array().unwrap().len(), 1);
    assert_eq!(
        reopened["reason"],
        json!("Mission was re-opened after merge and is actionable again.")
    );
}

#[test]
fn status_lifecycle_matches_the_oracle_derivation_over_the_recorded_post_state() {
    // The oracle fixture records the exact status rows + post-mission envelopes
    // it derived from, so replaying them natively compares the whole derived
    // payload (state, counts, timestamps, post-mission history) field for field.
    let fixture: Value = serde_json::from_str(
        &fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../sk-status/tests/fixtures/mutation/contract_vectors.json"),
        )
        .expect("oracle fixture"),
    )
    .expect("fixture json");

    let fixture_slug = fixture["lifecycle_derivation"]["mission_slug"]
        .as_str()
        .expect("fixture mission slug");
    let project = Project::named(fixture_slug);
    let rows = fixture["lifecycle_derivation_inputs"]["status_rows"]
        .as_array()
        .expect("status rows");
    sk_status::mutation::append_rows_atomic(
        &sk_status::mutation::events_path(&project.feature),
        rows,
        &sk_status::mutation::AppendOptions::default(),
    )
    .expect("replay status rows");

    for envelope in fixture["lifecycle_derivation_inputs"]["lifecycle_envelopes"]
        .as_array()
        .expect("envelopes")
    {
        let mut request = sk_status::mutation::LifecycleRequest::new(
            sk_status::mutation::mission_event_log_path(&project.feature),
            envelope["event_type"].as_str().expect("event_type"),
            envelope["payload"].clone(),
            envelope["aggregate_id"].as_str().expect("aggregate_id"),
            envelope["aggregate_type"].as_str().expect("aggregate_type"),
        );
        request.event_id = Some(envelope["event_id"].as_str().expect("event_id").to_owned());
        request.timestamp = Some(
            envelope["timestamp"]
                .as_str()
                .expect("timestamp")
                .to_owned(),
        );
        request.mission_slug = Some(fixture_slug.to_owned());
        request.repo_root = Some(project.root.clone());
        sk_status::mutation::persist_lifecycle_event_local(&request).expect("replay envelope");
    }

    let now = sk_kernel::clock::parse_iso(
        fixture["lifecycle_derivation_now"]
            .as_str()
            .expect("derivation now"),
    )
    .expect("parse now");
    let derived =
        status::lifecycle_at(&project.root, Some(fixture_slug), true, now).expect("derive");
    assert_eq!(
        derived, fixture["lifecycle_derivation"],
        "the native derivation must match the pinned oracle's"
    );
}

// ===========================================================================
// Remediation regressions: reviewer independence, self-review fallback,
// review-ward gates, feedback edges, canonical reason/sidecar, mark-status
// envelope, dependency comments and legacy identity.
// ===========================================================================

fn approval_args(agent: Option<&str>) -> MoveTaskArgs {
    let mut args = move_args("approved");
    args.agent = agent.map(str::to_owned);
    args
}

fn walk_to_in_review(project: &Project) {
    project.walk(
        "WP01",
        &[
            "planned",
            "claimed",
            "in_progress",
            "for_review",
            "in_review",
        ],
    );
}

#[test]
fn move_task_refuses_an_approval_by_the_implementer_of_record() {
    let project = Project::new();
    walk_to_in_review(&project);

    let error = tasks::move_task(&project.root, &approval_args(Some("claude")))
        .expect_err("self approval refused");
    assert_eq!(
        error.to_json()["error_code"],
        json!("REVIEW_NOT_INDEPENDENT")
    );
    assert!(
        error
            .message()
            .contains("cannot be approved by its implementer (claude::)"),
        "got {}",
        error.message()
    );
    assert!(error.message().contains("(claude::) is not provably"));
    assert_eq!(project.lane("WP01"), "in_review", "nothing approved");
    assert!(
        project
            .rows()
            .iter()
            .all(|row| row["to_lane"] != json!("approved"))
    );
}

#[test]
fn move_task_refuses_a_placeholder_approver() {
    let project = Project::new();
    walk_to_in_review(&project);

    let error =
        tasks::move_task(&project.root, &approval_args(None)).expect_err("placeholder refused");
    assert_eq!(
        error.to_json()["error_code"],
        json!("REVIEW_NOT_INDEPENDENT")
    );
    assert!(
        error.message().contains("placeholder 'user'"),
        "got {}",
        error.message()
    );
    assert_eq!(project.lane("WP01"), "in_review");
}

#[test]
fn move_task_accepts_a_provably_independent_approver() {
    let project = Project::new();
    walk_to_in_review(&project);

    let mut args = approval_args(Some("bob"));
    args.reviewer = Some("alice".to_owned());
    args.approval_ref = Some("PR#9".to_owned());
    let payload = tasks::move_task(&project.root, &args).expect("independent approval");
    assert_eq!(payload["to_lane"], json!("approved"));
    let row = project
        .rows()
        .into_iter()
        .find(|row| row["to_lane"] == json!("approved"))
        .expect("approval row");
    assert_eq!(row["force"], json!(false));
    assert_eq!(
        row["reason"],
        json!("move-task: in_review -> approved"),
        "the canonical default reason is always present"
    );
    assert_eq!(row["policy_metadata"]["tool"], json!("alice"));
}

#[test]
fn move_task_validates_the_self_review_fallback_option_set_before_writing() {
    let project = Project::new();
    walk_to_in_review(&project);

    let mut args = approval_args(Some("bob"));
    args.intended_reviewer = Some("carol".to_owned());
    let error = tasks::move_task(&project.root, &args).expect_err("fallback flag required");
    assert!(
        error.message().contains(
            "--intended-reviewer/--reviewer-failure-reason require --self-review-fallback."
        )
    );

    let mut args = approval_args(Some("bob"));
    args.self_review_fallback = true;
    let error = tasks::move_task(&project.root, &args).expect_err("force required");
    assert!(
        error
            .message()
            .contains("--self-review-fallback requires --force")
    );

    let mut args = approval_args(Some("bob"));
    args.self_review_fallback = true;
    args.force = true;
    let error = tasks::move_task(&project.root, &args).expect_err("intended reviewer required");
    assert!(
        error
            .message()
            .contains("--self-review-fallback requires --intended-reviewer <agent>.")
    );

    args.intended_reviewer = Some("carol".to_owned());
    let error = tasks::move_task(&project.root, &args).expect_err("failure reason required");
    assert!(
        error
            .message()
            .contains("--self-review-fallback requires --reviewer-failure-reason <reason>.")
    );

    let mut args = move_args("in_progress");
    args.self_review_fallback = true;
    args.force = true;
    args.intended_reviewer = Some("carol".to_owned());
    args.reviewer_failure_reason = Some("timeout".to_owned());
    let error = tasks::move_task(&project.root, &args).expect_err("approval only");
    assert!(
        error
            .message()
            .contains("--self-review-fallback is only valid when approving or marking done.")
    );

    assert_eq!(project.lane("WP01"), "in_review", "nothing was written");
}

#[test]
fn move_task_records_the_reviewer_self_approval_lifecycle_event() {
    let project = Project::new();
    walk_to_in_review(&project);

    let mut args = approval_args(Some("bob"));
    args.self_review_fallback = true;
    args.force = true;
    args.intended_reviewer = Some("carol".to_owned());
    args.reviewer_failure_reason = Some("reviewer timed out".to_owned());
    args.note = Some("self review fallback".to_owned());
    tasks::move_task(&project.root, &args).expect("documented fallback");

    let rows = project.rows();
    // Canonical ordering: the lane transition is written first, then the
    // `ReviewerSelfApproval` lifecycle fact. A lifecycle payload has no lane, so
    // the approved hop is the last *transition* row, not the last row.
    let approved = rows
        .iter()
        .rfind(|row| row["to_lane"] == json!("approved"))
        .expect("approved transition row");
    assert_eq!(approved["from_lane"], json!("in_review"));
    assert_eq!(approved["force"], json!(true));
    assert_eq!(
        approved["reason"],
        json!("self review fallback"),
        "the fallback keeps the operator note as the transition reason"
    );
    let event = rows
        .iter()
        .find(|row| row["event_type"] == json!("ReviewerSelfApproval"))
        .expect("ReviewerSelfApproval row");
    assert_eq!(
        rows.last().unwrap()["event_id"],
        event["event_id"],
        "the lifecycle fact is appended after the transition"
    );
    assert_eq!(event["aggregate_id"], json!("WP01"));
    assert_eq!(event["aggregate_type"], json!("WorkPackage"));
    // The canonical `ReviewerSelfApproval` payload has exactly these seven keys;
    // it never carries a lane field.
    let mut keys: Vec<&str> = event["payload"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "failure_reason",
            "fallback_approved",
            "implementing_actor",
            "intended_reviewer",
            "mission_slug",
            "timestamp",
            "wp_id",
        ]
    );
    assert_eq!(event["payload"]["fallback_approved"], json!(true));
    assert_eq!(event["payload"]["intended_reviewer"], json!("carol"));
    assert_eq!(
        event["payload"]["failure_reason"],
        json!("reviewer timed out")
    );
    assert_eq!(event["payload"]["implementing_actor"], json!("bob"));
    assert_eq!(event["payload"]["mission_slug"], json!(SLUG));
    assert_eq!(event["payload"]["wp_id"], json!("WP01"));

    // A second identical fallback is not recorded twice.
    let before = project.rows().len();
    let mut again = approval_args(Some("bob"));
    again.self_review_fallback = true;
    again.force = true;
    again.intended_reviewer = Some("carol".to_owned());
    again.reviewer_failure_reason = Some("reviewer timed out".to_owned());
    again.note = Some("self review fallback".to_owned());
    let _ = tasks::move_task(&project.root, &again);
    assert_eq!(
        project
            .rows()
            .iter()
            .filter(|row| row["event_type"] == json!("ReviewerSelfApproval"))
            .count(),
        1
    );
    assert!(project.rows().len() >= before);
}

#[test]
fn move_task_rejection_requires_a_substantive_feedback_file_not_a_note() {
    let project = Project::new();
    project.walk("WP01", &["planned", "claimed", "in_progress"]);

    let mut args = move_args("planned");
    args.note = Some("reset".to_owned());
    let error = tasks::move_task(&project.root, &args).expect_err("feedback required");
    assert!(
        error.message().contains("requires review feedback"),
        "got {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("This requirement cannot be bypassed with --force.")
    );
    assert_eq!(project.lane("WP01"), "in_progress");

    // An empty feedback file is refused too.
    let feedback = project.root.join("empty.md");
    fs::write(&feedback, "   \n").unwrap();
    let mut args = move_args("planned");
    args.review_feedback_file = Some(feedback.clone());
    let error = tasks::move_task(&project.root, &args).expect_err("empty feedback");
    assert!(
        error.message().contains(&format!(
            "Review feedback file is empty: {}",
            feedback.display()
        )),
        "got {}",
        error.message()
    );
    assert_eq!(project.lane("WP01"), "in_progress");
}

#[test]
fn move_task_refuses_a_feedback_file_on_an_edge_that_writes_no_review_cycle() {
    let project = Project::new();
    project.seed("WP01", "planned");
    project.seed("WP01", "claimed");
    project.seed("WP01", "in_progress");
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "T001 needs a retry policy.\n").unwrap();

    let mut args = move_args("claimed");
    args.review_feedback_file = Some(feedback);
    let error = tasks::move_task(&project.root, &args).expect_err("edge refused");
    assert!(
        error.message().contains("this edge writes no review cycle"),
        "got {}",
        error.message()
    );
    assert!(
        project
            .rows()
            .iter()
            .all(|row| row["review_ref"] != json!("feedback.md"))
    );
    assert_eq!(project.lane("WP01"), "in_progress");
}

#[test]
fn move_task_rejections_keep_the_feedback_pointer_as_the_review_ref() {
    let project = Project::new();
    walk_to_in_review(&project);
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "Add a retry policy.\n").unwrap();

    let mut args = move_args("planned");
    args.review_feedback_file = Some(feedback.clone());
    tasks::move_task(&project.root, &args).expect("rejection");
    let row = project
        .rows()
        .into_iter()
        .find(|row| row["to_lane"] == json!("planned") && row["from_lane"] == json!("in_review"))
        .expect("rejection row");
    assert_eq!(
        row["review_ref"],
        json!("review-cycle://034-cli-mutation-fixture/WP01/review-cycle-1.md")
    );
    assert_eq!(row["review_result"]["verdict"], json!("changes_requested"));
    // The raw draft path never becomes on-wire review authority, but the
    // reviewer's own file is left untouched.
    assert_eq!(
        fs::read_to_string(&feedback).unwrap(),
        "Add a retry policy.\n"
    );
}

#[test]
fn move_task_writes_the_canonical_default_reason() {
    let project = Project::new();
    project.seed("WP01", "planned");
    project.seed("WP01", "claimed");
    tasks::move_task(&project.root, &move_args("in_progress")).expect("advance");
    let row = project
        .rows()
        .into_iter()
        .find(|row| row["to_lane"] == json!("in_progress"))
        .expect("row");
    assert_eq!(row["reason"], json!("move-task: claimed -> in_progress"));

    // A forced move with no note gets the forced default instead.
    project.seed("WP01", "for_review");
    let mut args = move_args("blocked");
    args.force = true;
    tasks::move_task(&project.root, &args).expect("forced blocked");
    let row = project
        .rows()
        .into_iter()
        .find(|row| row["to_lane"] == json!("blocked"))
        .expect("blocked row");
    assert_eq!(row["reason"], json!("Force move to blocked"));
}

#[test]
fn status_emit_enforces_the_for_review_commit_gate() {
    let project = Project::new();
    project.git_init();
    project.write_lanes("lane-a", None);
    project.walk("WP01", &["planned", "claimed", "in_progress"]);

    let mut args = emit_args("for_review");
    args.subtasks_complete = Some(true);
    args.implementation_evidence_present = Some(true);
    let error = status::emit(&project.root, &args).expect_err("no lane commit");
    assert!(
        error
            .message()
            .contains("cannot move to for_review: no implementation commit on lane lane-a"),
        "got {}",
        error.message()
    );
    assert_eq!(error.to_json()["wp_id"], json!("WP01"));
    assert_eq!(error.to_json()["lane_id"], json!("lane-a"));
    assert_eq!(project.lane("WP01"), "in_progress");

    // `--force` is the documented bypass, but a force still needs an auditable
    // actor + reason (canonical `check_force`), so the gate bypass must carry one.
    let mut forced = emit_args("for_review");
    forced.force = true;
    forced.reason = Some("gate bypass for the regression".to_owned());
    forced.subtasks_complete = Some(true);
    forced.implementation_evidence_present = Some(true);
    status::emit(&project.root, &forced).expect("forced for_review");
    assert_eq!(project.lane("WP01"), "for_review");
}

#[test]
fn status_emit_accepts_a_planning_lane_with_a_qualifying_commit_since_claim() {
    let project = Project::new();
    let base = project.git_init();
    project.write_lanes("lane-planning", None);
    project.walk("WP01", &["planned", "claimed", "in_progress"]);
    project.git(&[
        "update-ref",
        &format!("refs/spec-kitty/wp-base/{SLUG}/WP01"),
        &base,
    ]);
    fs::create_dir_all(project.root.join("src")).unwrap();
    fs::write(project.root.join("src/lib.rs"), "pub fn shipped() {}\n").unwrap();
    project.git(&["add", "src/lib.rs"]);
    project.git(&["commit", "-qm", "feat: implementation"]);

    let mut args = emit_args("for_review");
    args.subtasks_complete = Some(true);
    args.implementation_evidence_present = Some(true);
    status::emit(&project.root, &args).expect("qualifying commit");
    assert_eq!(project.lane("WP01"), "for_review");
}

#[test]
fn move_task_refuses_a_review_ward_move_without_an_evidence_window() {
    let project = Project::new();
    project.write_lanes("lane-a", None);
    project.walk("WP01", &["planned", "claimed", "in_progress"]);

    let error =
        tasks::move_task(&project.root, &move_args("for_review")).expect_err("no planning commit");
    assert_eq!(
        error.to_json()["error_code"],
        json!("EVIDENCE_WINDOW_UNCOMPUTABLE")
    );
    // The canonical code also leads the message (the pinned non-owned `--json`
    // envelope carries the message, so the code must be readable there).
    assert!(
        error
            .message()
            .starts_with("EVIDENCE_WINDOW_UNCOMPUTABLE: WP01 has no evidence window (lane-a: lanes.json records no planning commit), so its implementation cannot be verified."),
        "got {}",
        error.message()
    );
    assert_eq!(project.lane("WP01"), "in_progress");
}

#[test]
fn mark_status_resolves_owners_case_insensitively_and_reports_the_format() {
    let project = Project::new();
    let payload = tasks::mark_status(
        &project.root,
        &MarkStatusArgs {
            task_ids: vec!["t002".to_owned()],
            status: "done".to_owned(),
            mission: Some(SLUG.to_owned()),
            wp: None,
            agent: None,
            auto_commit: None,
            no_auto_commit: Some(1),
            owned_checkout: None,
            json: true,
        },
    )
    .expect("case-insensitive owner");
    assert_eq!(
        payload["results"],
        json!([{
            "id": "t002",
            "outcome": "updated",
            "format": "authored_roster",
            "message": "Recorded done for t002 in WP01.",
        }])
    );
    let annotation = project
        .rows()
        .into_iter()
        .find(|row| row["kind"] == json!("annotation"))
        .expect("annotation");
    assert_eq!(annotation["delta"], json!({"subtasks": {"t002": "done"}}));

    // Mission-qualified ids normalize to the upper-cased bare id.
    let payload = tasks::mark_status(
        &project.root,
        &MarkStatusArgs {
            task_ids: vec![format!("{SLUG}/T003")],
            status: "done".to_owned(),
            mission: Some(SLUG.to_owned()),
            wp: None,
            agent: None,
            auto_commit: None,
            no_auto_commit: Some(1),
            owned_checkout: None,
            json: true,
        },
    )
    .expect("qualified id");
    assert_eq!(payload["results"][0]["id"], json!("T003"));
}

#[test]
fn mark_status_fails_with_exit_class_1_when_nothing_resolves() {
    let project = Project::new();
    let error = tasks::mark_status(
        &project.root,
        &MarkStatusArgs {
            task_ids: vec!["T999".to_owned()],
            status: "done".to_owned(),
            mission: Some(SLUG.to_owned()),
            wp: None,
            agent: None,
            auto_commit: None,
            no_auto_commit: Some(1),
            owned_checkout: None,
            json: true,
        },
    )
    .expect_err("nothing resolved");
    assert_eq!(error.exit(), 1);
    assert_eq!(
        error.to_json(),
        json!({
            "results": [{
                "id": "T999",
                "outcome": "not_found",
                "format": Value::Null,
                "message": "T999 was not found in any supported task format.",
            }],
            "summary": {"updated": 0, "already_satisfied": 0, "not_found": 1},
        })
    );
    assert!(project.rows().is_empty());

    // A missing `tasks.md` refuses before resolution.
    fs::remove_file(project.feature.join("tasks.md")).unwrap();
    let error = tasks::mark_status(
        &project.root,
        &MarkStatusArgs {
            task_ids: vec!["T001".to_owned()],
            status: "done".to_owned(),
            mission: Some(SLUG.to_owned()),
            wp: None,
            agent: None,
            auto_commit: None,
            no_auto_commit: Some(1),
            owned_checkout: None,
            json: true,
        },
    )
    .expect_err("tasks.md required");
    assert!(
        error.message().contains(&format!(
            "tasks.md not found: {}",
            project.feature.join("tasks.md").display()
        )),
        "got {}",
        error.message()
    );
}

#[test]
fn status_emit_parses_inline_dependency_comments() {
    let project = Project::new();
    fs::write(
        project.feature.join("tasks").join("WP02.md"),
        "---\nwork_package_id: WP02\ntitle: Two\ndependencies:\n- WP01  # first dependency\n---\n# WP02\n",
    )
    .unwrap();
    // The unforced FSM admits only `genesis -> planned|canceled`; seed the
    // dependency through the legal lifecycle rather than bypassing the edge.
    project.walk(
        "WP01",
        &["planned", "claimed", "in_progress", "approved", "done"],
    );
    project.seed("WP02", "planned");

    status::emit(&project.root, &{
        let mut args = emit_args("claimed");
        args.wp_id = Some("WP02".to_owned());
        args
    })
    .expect("a satisfied dependency with an inline comment is satisfied");
    assert_eq!(project.lane("WP02"), "claimed");
}

#[test]
fn legacy_missions_without_a_mission_id_keep_a_null_identity() {
    let project = Project::new();
    let meta_path = project.feature.join("meta.json");
    let mut meta: Value = serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta.as_object_mut().unwrap().remove("mission_id");
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

    let lifecycle = status::lifecycle(&project.root, Some(SLUG), true).expect("lifecycle");
    assert_eq!(lifecycle["mission_id"], Value::Null, "never a fake ULID");
    assert_eq!(lifecycle["mission_slug"], json!(SLUG));

    status::emit(&project.root, &emit_args("planned")).expect("legacy emit");
    let rows = project.rows();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].get("mission_id").is_none(),
        "a legacy row omits mission_id"
    );

    let payload = tasks::mark_status(
        &project.root,
        &MarkStatusArgs {
            task_ids: vec!["T001".to_owned()],
            status: "done".to_owned(),
            mission: Some(SLUG.to_owned()),
            wp: None,
            agent: None,
            auto_commit: None,
            no_auto_commit: Some(1),
            owned_checkout: None,
            json: true,
        },
    )
    .expect("legacy mark-status");
    assert_eq!(payload["summary"]["updated"], json!(1));

    let listed = tasks::list_tasks(&project.root, Some(SLUG), None, true).expect("list-tasks");
    assert_eq!(listed["count"], json!(2));
}

#[test]
fn the_status_rollback_head_probe_reads_the_committed_log() {
    let project = Project::new();
    project.git_init();
    let events = project.feature.join("status.events.jsonl");
    project.seed("WP01", "planned");
    project.git(&["add", "kitty-specs"]);
    project.git(&["commit", "-qm", "status"]);

    let probe = sk_cli::compat::support::StatusHeadCommittedProbe::open(&project.root).unwrap();
    let committed = sk_status::HeadCommittedProbe::committed_event_ids(&probe, &events).unwrap();
    let row = project.rows().into_iter().next().unwrap();
    assert!(
        committed.contains(row["event_id"].as_str().unwrap()),
        "the committed row is visible to the rollback probe"
    );

    // An untracked log is not committed: the probe answers empty, never an error.
    fs::remove_file(&events).unwrap();
    project.seed("WP01", "planned");
    let committed = sk_status::HeadCommittedProbe::committed_event_ids(&probe, &events).unwrap();
    let row = project.rows().into_iter().next().unwrap();
    assert!(!committed.contains(row["event_id"].as_str().unwrap()));
}

// ===========================================================================
// Recovery regressions: the six stale-expectation classes plus the review
// findings that had no end-to-end route coverage (lane-head stamp for numbered
// slugs, phase-1 mirror gating, legacy mission move-task, HEAD-probe errors).
// Every expectation below is derived from the pinned Python baseline, not from
// the native implementation's current output.
// ===========================================================================

#[test]
fn genesis_is_a_seed_source_not_a_direct_done_target() {
    // Canonical FSM (`wp_state.GenesisState.allowed_targets`): an unforced
    // `genesis -> done` is illegal, and `--force` alone (no actor/reason) is not
    // the escape either. This is why a fixture that needs a `done` dependency
    // must walk the real lifecycle instead of seeding the terminal lane.
    let project = Project::new();
    let mut request = sk_status::mutation::TransitionRequest::new(
        &project.feature,
        SLUG,
        "WP01",
        "done",
        sk_status::mutation::ActorValue::identity("claude"),
    );
    request.repo_root = Some(project.root.clone());
    let error = sk_status::mutation::emit_status_transition(&request)
        .expect_err("genesis -> done is not an unforced edge");
    assert!(
        error
            .to_string()
            .contains("Illegal transition: genesis -> done"),
        "got {error}"
    );
    assert!(project.rows().is_empty(), "a refused seed writes nothing");
}

#[test]
fn status_emit_stamps_the_lane_head_for_a_numbered_mission_slug() {
    // N6: a pure legacy `057-` mission keeps its numeric prefix in the code-lane
    // branch (`code_lane_branch_name`), so the lane-head probe reads the branch
    // Git actually created instead of silently dropping the stamp.
    let project = Project::named("057-lanehead");
    let head = project.git_init();
    project.write_lanes_for("057-lanehead", "lane-a", None);
    project.git(&["branch", "kitty/mission-057-lanehead-lane-a", &head]);
    project.seed("WP01", "planned");

    let mut args = emit_args("claimed");
    args.mission = Some("057-lanehead".to_owned());
    status::emit(&project.root, &args).expect("claim");

    let row = project
        .rows()
        .into_iter()
        .find(|row| row["from_lane"] == json!("planned") && row["to_lane"] == json!("claimed"))
        .expect("claim row");
    assert_eq!(
        row["policy_metadata"],
        json!({"lane_head": head}),
        "the numbered-slug lane branch carries the stamp"
    );
}

#[test]
fn status_emit_mirrors_a_whole_model_frontmatter_and_skips_an_off_model_one() {
    // N12: the phase-1 mirror is gated on a whole-model frontmatter read. A
    // valid block loses nothing but the `lane` scalar; an off-model block is
    // logged and skipped while the canonical transition still lands.
    let project = Project::new();
    let meta_path = project.feature.join("meta.json");
    let mut meta: Value = serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta["status_phase"] = json!(1);
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

    let wp_path = project.feature.join("tasks").join("WP01.md");
    let authored = "---\nwork_package_id: WP01\ntitle: WP01 title\nlane: planned  # keep me\ndependencies: []\nsubtasks:\n  - T001\n  - T002\n---\n\n# WP01\n";
    fs::write(&wp_path, authored).unwrap();
    project.seed("WP01", "planned");
    status::emit(&project.root, &emit_args("claimed")).expect("claim");
    let mirrored = fs::read_to_string(&wp_path).unwrap();
    assert!(
        mirrored.contains("lane: claimed  # keep me"),
        "the lane scalar is updated and the comment preserved: {mirrored}"
    );
    assert!(mirrored.contains("subtasks:\n  - T001\n  - T002"));
    assert!(mirrored.contains("\n# WP01\n"));

    let off_model = Project::new();
    let meta_path = off_model.feature.join("meta.json");
    let mut meta: Value = serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta["status_phase"] = json!(1);
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
    let wp_path = off_model.feature.join("tasks").join("WP01.md");
    let authored =
        "---\nwork_package_id: WP01\ntitle: WP01 title\nlane: planned\nextra: 1\n---\n\n# WP01\n";
    fs::write(&wp_path, authored).unwrap();
    off_model.seed("WP01", "planned");
    status::emit(&off_model.root, &emit_args("claimed")).expect("claim still lands");
    assert_eq!(
        fs::read_to_string(&wp_path).unwrap(),
        authored,
        "an off-model frontmatter is never rewritten"
    );
    assert_eq!(off_model.lane("WP01"), "claimed");
}

#[test]
fn legacy_missions_without_a_mission_id_move_tasks_without_inventing_one() {
    // N10: the baseline supports a legacy `meta.json` with no `mission_id` and
    // simply omits the field from the row. The move-task door must not inherit an
    // identity refusal from the inventory reader.
    let project = Project::new();
    let meta_path = project.feature.join("meta.json");
    let mut meta: Value = serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta.as_object_mut().unwrap().remove("mission_id");
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

    project.seed("WP01", "planned");
    let payload = tasks::move_task(&project.root, &move_args("claimed")).expect("legacy move");
    assert_eq!(payload["to_lane"], json!("claimed"));
    let row = project
        .rows()
        .into_iter()
        .find(|row| row["to_lane"] == json!("claimed"))
        .expect("claim row");
    assert!(
        row.get("mission_id").is_none(),
        "a legacy row omits mission_id instead of inventing one"
    );
}

#[test]
fn the_status_rollback_head_probe_retains_committed_parse_errors() {
    // The verified rollback must be able to tell "HEAD holds a log I cannot
    // trust" from "HEAD holds no rows": a malformed committed row is an error,
    // which becomes HEAD_UNREADABLE (fail closed), never an empty committed set.
    let project = Project::new();
    project.git_init();
    let events = project.feature.join("status.events.jsonl");
    fs::write(&events, b"{\"event_id\": \"not-a-whole-row\n").unwrap();
    project.git(&["add", "kitty-specs"]);
    project.git(&["commit", "-qm", "malformed status"]);

    let probe = sk_cli::compat::support::StatusHeadCommittedProbe::open(&project.root).unwrap();
    let error = sk_status::HeadCommittedProbe::committed_event_ids(&probe, &events)
        .expect_err("a malformed committed log is not an empty set");
    assert!(error.contains("not JSON"), "got {error}");
}

#[test]
fn the_status_rollback_head_probe_treats_corrupt_git_as_an_error_not_an_absence() {
    // A corrupt symbolic HEAD must never become `BlobLookup::Absent`/`NoRevision`
    // (an empty committed set would let a rollback cut rows that Git cannot
    // prove are uncommitted). `GitRepository::open` still discovers the work
    // tree for `ref: refs/heads/<empty>`; resolving HEAD is then a hard error.
    let project = Project::new();
    project.git_init();
    let events = project.feature.join("status.events.jsonl");
    project.seed("WP01", "planned");
    let head_path = project.root.join(".git").join("HEAD");
    fs::write(&head_path, b"ref: refs/heads/\n").unwrap();

    let probe = sk_cli::compat::support::StatusHeadCommittedProbe::open(&project.root)
        .expect("the work tree still discovers; only HEAD resolution is broken");
    let error = sk_status::HeadCommittedProbe::committed_event_ids(&probe, &events)
        .expect_err("a corrupt HEAD is an error, not an absent committed log");
    assert!(error.contains("HEAD could not be read"), "got {error}");

    // A HEAD that makes Git classify the checkout as not-a-repository is an
    // error at open time, never a successful "there is no committed log".
    fs::write(&head_path, b"").unwrap();
    assert!(
        sk_cli::compat::support::StatusHeadCommittedProbe::open(&project.root).is_err(),
        "an unreadable repository is not a non-Git empty result"
    );
}

// ===========================================================================
// D2/R3: real primary/non-owned commit + review-cycle durability, protected
// destination refusal, verified rollback, and the committed-log probe bound.
// ===========================================================================

#[test]
fn move_task_refuses_a_protected_destination_before_writing() {
    let project = Project::new();
    // The default protection set protects `main`; auto-commit is on by config.
    project.write_config("agents:\n  auto_commit: true\n");
    project.git_init();
    project.seed("WP01", "planned");
    let before = project.rows().len();
    let head = project.head();

    let mut args = move_args("claimed");
    args.auto_commit = Some(1);
    args.no_auto_commit = None;
    let error = tasks::move_task(&project.root, &args).expect_err("protected destination");
    assert_eq!(
        error.to_json()["error_code"],
        json!("PROTECTED_BRANCH_COMMIT_REFUSED")
    );
    assert!(
        error.message().contains("protected branch 'main'"),
        "got {}",
        error.message()
    );
    assert_eq!(project.rows().len(), before, "no status row is written");
    assert_eq!(project.head(), head, "no commit is made");
}

#[test]
fn move_task_honours_the_project_auto_commit_default_and_writes_local_evidence() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: false\nprotection:\n  protected_branches: []\n");
    project.git_init();
    walk_to_in_review(&project);
    let head = project.head();
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "Add a retry policy.\n").unwrap();

    // Neither flag is present: the project default (`false`) decides.
    let mut args = move_args("planned");
    args.auto_commit = None;
    args.no_auto_commit = None;
    args.review_feedback_file = Some(feedback);
    let payload = tasks::move_task(&project.root, &args).expect("local-only rejection");
    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["durability_classification"], json!("local_only"));
    assert_eq!(payload["durability_reason"], json!("no_auto_commit"));
    assert_eq!(payload["verdict_durably_persisted"], json!(false));
    assert_eq!(project.head(), head, "no commit is made");
    assert!(
        project
            .feature
            .join("tasks/WP01/review-cycle-1.md")
            .is_file(),
        "the evidence is still written locally"
    );
    assert!(project.status_porcelain().contains("review-cycle-1.md"));
}

#[test]
fn move_task_auto_commits_status_and_verdict_evidence_durably() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    // No authored subtasks: the `for_review` completeness gate is satisfied by
    // the empty roster rather than by a mark-status side write.
    write_wp(&project.feature, "WP01", &[], &[]);
    project.git_init();
    project.walk("WP01", &["planned", "claimed", "in_progress"]);

    // A durable status commit carries the transition row on the target branch.
    let mut args = move_args("for_review");
    args.auto_commit = Some(1);
    args.no_auto_commit = None;
    let payload = tasks::move_task(&project.root, &args).expect("for_review commit");
    let event_id = payload["event_id"].as_str().expect("event id").to_owned();
    let committed = project.git(&[
        "show",
        "main:kitty-specs/034-cli-mutation-fixture/status.events.jsonl",
    ]);
    assert!(
        committed.contains(&event_id),
        "the committed status log must carry the transition row"
    );
    assert_eq!(project.status_porcelain(), "", "the status tree is clean");

    // The approval hop commits the status artifacts AND the review-cycle
    // evidence, and is durable only after the exact bytes read back.
    project.seed("WP01", "in_review");
    let mut args = approval_args(Some("bob"));
    args.auto_commit = Some(1);
    args.no_auto_commit = None;
    args.reviewer = Some("alice".to_owned());
    args.approval_ref = Some("PR#42".to_owned());
    let payload = tasks::move_task(&project.root, &args).expect("approval commit");
    assert_eq!(payload["verdict_durably_persisted"], json!(true));
    assert_eq!(payload["durability_classification"], json!("durable"));
    assert_eq!(payload["destination_ref"], json!("main"));
    let evidence_ref = payload["evidence_ref"]
        .as_str()
        .expect("evidence ref")
        .to_owned();
    assert_eq!(
        evidence_ref,
        "kitty-specs/034-cli-mutation-fixture/tasks/WP01/review-cycle-1.md"
    );
    let local = fs::read_to_string(project.root.join(&evidence_ref)).expect("local evidence");
    let committed = project.git(&["show", &format!("main:{evidence_ref}")]);
    assert_eq!(
        committed,
        local.trim_end_matches('\n'),
        "the committed evidence bytes must equal the local artifact"
    );
    assert_eq!(project.status_porcelain(), "", "the status tree is clean");
    assert!(local.contains("reviewer_agent: alice\n"));
    assert!(local.ends_with("---\n\nApproved by alice: PR#42\n"));
}

#[test]
fn move_task_rolls_back_the_status_tail_when_the_commit_did_not_land() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    walk_to_in_review(&project);
    let rows_before = project.rows().len();
    let lane_before = project.lane("WP01");
    // A rejecting hook makes safe commit provably NOT land.
    project.install_hook("pre-commit", "#!/bin/sh\nexit 1\n");
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "Add a retry policy.\n").unwrap();

    let mut args = move_args("planned");
    args.auto_commit = Some(1);
    args.no_auto_commit = None;
    args.review_feedback_file = Some(feedback);
    let error = tasks::move_task(&project.root, &args).expect_err("the commit is refused");
    assert_eq!(
        error.to_json()["error_code"],
        json!("VERDICT_PERSISTENCE_FAILED")
    );
    assert!(
        error.message().contains("did not land"),
        "got {}",
        error.message()
    );
    assert_eq!(
        project.rows().len(),
        rows_before,
        "only this operation's own uncommitted tail is cut"
    );
    assert_eq!(project.lane("WP01"), lane_before);
    assert!(
        !project
            .feature
            .join("tasks/WP01/review-cycle-1.md")
            .exists(),
        "the uncommitted evidence this call wrote is removed"
    );
}

#[test]
fn the_status_rollback_head_probe_reads_a_committed_log_beyond_the_old_16_mib_cap() {
    // The Git owner landed the canonical 64 MiB explicit bound; the probe must
    // request it rather than the previous 16 MiB seam cap.
    assert_eq!(sk_git::MAX_BLOB_BYTES, 64 * 1024 * 1024);
    let project = Project::new();
    let padding = "x".repeat(1000);
    let mut rows = String::new();
    let mut expected = 0u64;
    while rows.len() <= 17 * 1024 * 1024 {
        rows.push_str(&format!(
            "{{\"event_id\": \"E{expected:08}\", \"padding\": \"{padding}\"}}\n"
        ));
        expected += 1;
    }
    let log = project.feature.join("status.events.jsonl");
    fs::write(&log, rows).unwrap();
    project.git_init();

    let probe = StatusHeadCommittedProbe::open(&project.root).expect("open probe");
    let ids = probe.committed_event_ids(&log).expect("read committed log");
    assert_eq!(
        ids.len() as u64,
        expected,
        "the whole >16 MiB committed log is read, never truncated or refused"
    );
    assert!(ids.contains("E00000000"));
}

#[test]
fn move_task_accepts_a_real_evidence_window_for_owned_files() {
    let slug = "057-evidence-window";
    let project = Project::named(slug);
    project.set_meta("topology", json!("single_branch"));
    write_wp_owned(&project.feature, "WP01", &["src/**"]);
    let planning = project.git_init();
    // A canonical single-branch manifest: WP01 is in no lane, so the lane
    // commit gate is a no-op while the single-branch evidence window applies.
    fs::write(
        project.feature.join("lanes.json"),
        serde_json::to_string_pretty(&json!({
            "version": 1,
            "mission_slug": slug,
            "mission_id": "01K9M0R3W00000000000000000",
            "mission_branch": "main",
            "target_branch": "main",
            "lanes": [{
                "lane_id": "lane-a",
                "wp_ids": ["WP02"],
                "write_scope": ["src/**"],
                "predicted_surfaces": [],
                "depends_on_lanes": [],
                "parallel_group": 0,
            }],
            "computed_at": "2026-11-03T00:00:00Z",
            "computed_from": "test",
            "planning_artifact_wps": [],
            "planning_commit_sha": planning,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(project.root.join("src")).unwrap();
    fs::write(project.root.join("src/retry.rs"), "pub fn retry() {}\n").unwrap();
    project.git(&["add", "-A"]);
    project.git(&["commit", "-qm", "implement WP01"]);

    project.walk("WP01", &["planned", "claimed", "in_progress"]);
    let mut args = move_args("for_review");
    args.mission = Some(slug.to_owned());
    args.auto_commit = None;
    args.no_auto_commit = Some(1);
    let payload = tasks::move_task(&project.root, &args).expect("the owned change is evidence");
    assert_eq!(payload["to_lane"], json!("for_review"));
    assert_eq!(project.lane("WP01"), "for_review");
}

#[test]
fn mark_status_auto_commits_annotation_rows_for_a_transactional_mission() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    let args = MarkStatusArgs {
        task_ids: vec!["T001".to_owned()],
        status: "done".to_owned(),
        mission: Some(SLUG.to_owned()),
        wp: Some("WP01".to_owned()),
        agent: Some("claude".to_owned()),
        auto_commit: Some(1),
        no_auto_commit: None,
        owned_checkout: None,
        json: true,
    };
    let payload = tasks::mark_status(&project.root, &args).expect("committed annotation");
    assert_eq!(payload["summary"]["updated"], json!(1));
    let committed = project.git(&[
        "show",
        "main:kitty-specs/034-cli-mutation-fixture/status.events.jsonl",
    ]);
    assert!(committed.contains("\"T001\""));
    assert_eq!(project.status_porcelain(), "");
}

#[test]
fn mark_status_rolls_back_when_the_annotation_commit_did_not_land() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    project.install_hook("pre-commit", "#!/bin/sh\nexit 1\n");
    let before = project.rows().len();
    let args = MarkStatusArgs {
        task_ids: vec!["T001".to_owned()],
        status: "done".to_owned(),
        mission: Some(SLUG.to_owned()),
        wp: Some("WP01".to_owned()),
        agent: Some("claude".to_owned()),
        auto_commit: Some(1),
        no_auto_commit: None,
        owned_checkout: None,
        json: true,
    };
    let error = tasks::mark_status(&project.root, &args).expect_err("commit refused");
    assert_eq!(error.to_json()["error_code"], json!("COMMIT_NOT_LANDED"));
    assert_eq!(project.rows().len(), before);
}

#[test]
fn mark_status_commits_nothing_when_every_requested_subtask_is_satisfied() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    let args = MarkStatusArgs {
        task_ids: vec!["T001".to_owned()],
        status: "done".to_owned(),
        mission: Some(SLUG.to_owned()),
        wp: Some("WP01".to_owned()),
        agent: Some("claude".to_owned()),
        auto_commit: Some(1),
        no_auto_commit: None,
        owned_checkout: None,
        json: true,
    };
    tasks::mark_status(&project.root, &args).expect("first mark commits");
    let head = project.head();
    let payload = tasks::mark_status(&project.root, &args).expect("idempotent mark");
    assert_eq!(payload["summary"]["updated"], json!(0));
    assert_eq!(payload["summary"]["already_satisfied"], json!(1));
    assert_eq!(project.head(), head, "an idempotent mark creates no commit");
    assert_eq!(project.status_porcelain(), "");
}

// ===========================================================================
// Second remediation — independent review findings F1-F6.
//
// Every case below drives the real `spec-kitty` binary or the real route with
// a real temporary Git checkout, so the regression pins observable behavior
// rather than an internal helper's shape.
// ===========================================================================

/// A single-branch fixture with auto-commit on, an unprotected target, a real
/// Git checkout and `WP01` seeded to `planned`.
fn auto_commit_fixture() -> Project {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    write_wp(&project.feature, "WP01", &[], &[]);
    project.git_init();
    project.walk("WP01", &["planned"]);
    project
}

fn prior_review_cycle(project: &Project) -> PathBuf {
    let directory = sk_status::mutation_commit::review_cycle_dir(&project.feature, "WP01");
    sk_status::mutation_commit::ReviewCycleArtifact {
        cycle_number: 1,
        wp_id: "WP01".to_owned(),
        mission_slug: project.slug.clone(),
        reviewer_agent: "alice".to_owned(),
        reviewed_at: "2026-11-03T00:00:00Z".to_owned(),
        affected_files: Vec::new(),
        reproduction_command: None,
        body: "The prior review finding.\n".to_owned(),
        override_actor: None,
        override_reason: None,
    }
    .write_new(&directory)
    .expect("write the prior review-cycle artifact")
}

// --- F1 -------------------------------------------------------------------

#[test]
fn f1_a_refused_rollback_retains_the_transition_row_and_its_review_cycle_evidence() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    walk_to_in_review(&project);
    let rows_before = project.rows().len();
    // The hook breaks HEAD and rejects the commit: the commit provably did not
    // land, but the rollback then cannot PROVE the rows are uncommitted.
    project.install_hook(
        "pre-commit",
        "#!/bin/sh\nprintf 'not-a-ref\\n' > .git/HEAD\nexit 1\n",
    );
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "Add a retry policy.\n").unwrap();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "planned",
            "--agent",
            "claude",
            "--review-feedback-file",
            feedback.to_str().unwrap(),
            "--auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        !output.status.success(),
        "the commit is refused: {output:?}"
    );
    let payload = single_json(&output);
    assert_eq!(payload["error_code"], json!("VERDICT_PERSISTENCE_FAILED"));
    let message = payload["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("STATUS_ROLLBACK_REFUSED"),
        "the rollback refusal is reported: {message}"
    );

    // The retained row is the applied rejection: the rollback refused, so the
    // lane stays at the transition it recorded.
    assert_eq!(
        project.rows().len(),
        rows_before + 1,
        "the transition row is retained, not rolled back"
    );
    assert_eq!(project.lane("WP01"), "planned");
    let artifact = project.feature.join("tasks/WP01/review-cycle-1.md");
    assert!(
        artifact.is_file(),
        "evidence for a retained row is never deleted"
    );
    let rendered = fs::read_to_string(&artifact).unwrap();
    assert!(
        rendered.starts_with(
            "---\naffected_files: []\ncycle_number: 1\nmission_slug: 034-cli-mutation-fixture\nreproduction_command:\nreviewed_at: '"
        ),
        "the canonical artifact layout survives: {rendered}"
    );
    assert!(
        rendered.contains("\nreviewer_agent: claude\nwp_id: WP01\n---\n\nAdd a retry policy.\n")
    );
    let last = project.rows().last().cloned().expect("retained row");
    let pointer = last["review_ref"]
        .as_str()
        .expect("the retained row keeps its review pointer");
    assert!(pointer.ends_with("/review-cycle-1.md"), "got {pointer}");
    assert!(
        artifact.is_file(),
        "the retained pointer must not dangle: {pointer}"
    );
}

#[test]
fn f1_a_hook_side_commit_keeps_the_committed_evidence() {
    let project = Project::new();
    project.write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    walk_to_in_review(&project);
    let rows_before = project.rows().len();
    // The hook commits rows + artifact itself, then rejects the outer commit.
    project.install_hook(
        "pre-commit",
        "#!/bin/sh\ngit add -A\ngit -c user.name=Hook -c user.email=hook@example.invalid commit --no-verify -qm 'hook side commit'\nexit 1\n",
    );
    let feedback = project.root.join("feedback.md");
    fs::write(&feedback, "Add a retry policy.\n").unwrap();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "planned",
            "--agent",
            "claude",
            "--review-feedback-file",
            feedback.to_str().unwrap(),
            "--auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        !output.status.success(),
        "the commit is refused: {output:?}"
    );
    let payload = single_json(&output);
    assert_eq!(payload["error_code"], json!("VERDICT_PERSISTENCE_FAILED"));
    let message = payload["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("already committed at HEAD"),
        "the rollback refuses the side-committed rows: {message}"
    );

    let evidence_ref = "kitty-specs/034-cli-mutation-fixture/tasks/WP01/review-cycle-1.md";
    let artifact = project.root.join(evidence_ref);
    // The side commit is the checkout's HEAD and carries the evidence.
    let committed = project.git(&["show", &format!("HEAD:{evidence_ref}")]);
    assert_eq!(
        committed,
        fs::read_to_string(&artifact)
            .expect("the committed evidence remains in the worktree")
            .trim_end_matches('\n'),
        "the committed evidence is not deleted from the checkout"
    );
    assert_eq!(
        project.rows().len(),
        rows_before + 1,
        "the side-committed rows are retained"
    );
    let last = project.rows().last().cloned().expect("side-committed row");
    let pointer = last["review_ref"]
        .as_str()
        .expect("the retained row keeps its review pointer");
    assert!(pointer.ends_with("/review-cycle-1.md"), "got {pointer}");
    assert!(
        artifact.is_file(),
        "the retained pointer must not dangle: {pointer}"
    );
    // The artifact is present and byte-identical to the committed blob; this
    // call never stages or performs its deletion. (The hook's own commit used
    // the safe-commit transaction's temporary index, so the operator index can
    // lag it; repairing that index is the Git owner's contract, not this
    // deletion guard.)
    assert!(
        !committed.is_empty(),
        "the side commit still carries the evidence blob"
    );
}

// --- F2 -------------------------------------------------------------------

#[test]
fn f2_mark_status_degrades_to_an_uncommitted_annotation_on_a_protected_branch() {
    let project = Project::new();
    // No config at all: the default project (auto-commit on, `main` protected).
    project.set_meta("topology", json!("single_branch"));
    project.git_init();
    let head = project.head();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "mark-status",
            "T001",
            "--status",
            "done",
            "--wp",
            "WP01",
            "--agent",
            "claude",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "the ordinary implementer command must not hard-refuse: {output:?}"
    );
    let payload = single_json(&output);
    assert_eq!(payload["summary"]["updated"], json!(1));
    assert_eq!(
        payload["durability_reason"],
        json!("protected_target_branch")
    );
    assert_eq!(payload["destination_ref"], json!("main"));
    assert_eq!(project.head(), head, "the annotation is not committed");
    let rows = project.rows();
    let annotation = rows
        .iter()
        .find(|row| row["kind"] == json!("annotation"))
        .expect("the annotation is recorded locally");
    assert_eq!(annotation["delta"], json!({"subtasks": {"T001": "done"}}));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "was written but not committed: the mission records directly on a protected branch"
        ),
        "the canonical degrade warning is printed: {stderr}"
    );
    assert!(
        project.status_porcelain().contains("status.events.jsonl"),
        "the local status write stays uncommitted"
    );
}

// --- F3 -------------------------------------------------------------------

#[test]
fn f3_mark_status_leaves_lanes_annotations_uncommitted() {
    // `LANES` and flat/legacy missions: neither is `single_branch`, so the
    // reference records the annotation locally with no commit.
    for topology in [Some("lanes"), None] {
        let project = Project::new();
        project
            .write_config("agents:\n  auto_commit: true\nprotection:\n  protected_branches: []\n");
        match topology {
            Some(topology) => project.set_meta("topology", json!(topology)),
            None => project.remove_meta("topology"),
        }
        project.git_init();
        let head = project.head();

        let output = run_cli(
            &project.root,
            &project.home(),
            &[
                "agent",
                "tasks",
                "mark-status",
                "T001",
                "--status",
                "done",
                "--wp",
                "WP01",
                "--agent",
                "claude",
                "--mission",
                SLUG,
                "--json",
            ],
        );
        assert!(output.status.success(), "{topology:?}: {output:?}");
        assert_eq!(single_json(&output)["summary"]["updated"], json!(1));
        assert!(
            project
                .rows()
                .iter()
                .any(|row| row["kind"] == json!("annotation")),
            "{topology:?}: the annotation is recorded"
        );
        assert_eq!(
            project.head(),
            head,
            "{topology:?}: a non-single_branch annotation is written locally, exactly like the reference"
        );
    }
}

// --- F4 -------------------------------------------------------------------

#[test]
fn f4_move_task_commits_on_the_respected_current_branch_not_the_meta_target() {
    let project = auto_commit_fixture();
    project.git(&["checkout", "-q", "-b", "topic-branch"]);
    project.walk("WP01", &["claimed", "in_progress"]);
    let head = project.head();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "for_review",
            "--agent",
            "claude",
            "--auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "the ordinary topic-branch workflow must not be refused: {output:?}"
    );
    let payload = single_json(&output);
    assert_eq!(payload["destination_ref"], json!("topic-branch"));
    assert_ne!(
        project.head(),
        head,
        "the commit landed on the current branch"
    );
    let committed = project.git(&[
        "show",
        "topic-branch:kitty-specs/034-cli-mutation-fixture/status.events.jsonl",
    ]);
    assert!(
        committed.contains(payload["event_id"].as_str().unwrap()),
        "the transition is committed on the respected current branch"
    );
    // The stored destination is unchanged; only the traced destination differs.
    assert_eq!(
        project.git(&[
            "show",
            "HEAD:kitty-specs/034-cli-mutation-fixture/meta.json"
        ]),
        fs::read_to_string(project.feature.join("meta.json"))
            .unwrap()
            .trim_end_matches('\n')
    );
}

#[test]
fn f4_move_task_commits_when_meta_has_no_stored_target_branch() {
    let project = auto_commit_fixture();
    project.remove_meta("target_branch");
    project.walk("WP01", &["claimed", "in_progress"]);
    let head = project.head();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "for_review",
            "--agent",
            "claude",
            "--auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "a legacy meta without `target_branch` is not a refusal: {output:?}"
    );
    assert_eq!(single_json(&output)["destination_ref"], json!("main"));
    assert_ne!(project.head(), head);
}

// --- F5 -------------------------------------------------------------------

#[test]
fn f5_auto_commit_flags_reject_valued_spellings_and_resolve_polarity_last_wins() {
    // Upstream declares ONE Typer boolean pair: a value is never accepted, so
    // `--no-auto-commit=true` can never enable a commit and cannot even parse.
    for spelling in [
        "--no-auto-commit=true",
        "--no-auto-commit=false",
        "--auto-commit=false",
        "--auto-commit=yes",
    ] {
        let project = auto_commit_fixture();
        let rows = project.rows().len();
        let head = project.head();
        let output = run_cli(
            &project.root,
            &project.home(),
            &[
                "agent",
                "tasks",
                "move-task",
                "WP01",
                "--to",
                "claimed",
                "--agent",
                "claude",
                spelling,
                "--mission",
                SLUG,
                "--json",
            ],
        );
        assert!(
            !output.status.success(),
            "{spelling} must be rejected like the reference: {output:?}"
        );
        assert_eq!(project.rows().len(), rows, "{spelling} writes nothing");
        assert_eq!(project.head(), head, "{spelling} commits nothing");
    }

    // Bare flags: repeats and the pair are last-wins, and the negative option
    // never resolves to an enabling `true`.
    let cases: &[(&[&str], bool)] = &[
        (&["--auto-commit"], true),
        (&["--no-auto-commit"], false),
        (&["--auto-commit", "--no-auto-commit"], false),
        (&["--no-auto-commit", "--auto-commit"], true),
        (&["--auto-commit", "--auto-commit"], true),
        (&["--no-auto-commit", "--no-auto-commit"], false),
    ];
    for (flags, expect_commit) in cases {
        let project = auto_commit_fixture();
        let head = project.head();
        let mut argv = vec![
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "claimed",
            "--agent",
            "claude",
            "--mission",
            SLUG,
            "--json",
        ];
        argv.extend_from_slice(flags);
        let output = run_cli(&project.root, &project.home(), &argv);
        assert!(output.status.success(), "{flags:?}: {output:?}");
        assert_eq!(single_json(&output)["new_lane"], json!("claimed"));
        assert_eq!(
            project.head() != head,
            *expect_commit,
            "flags {flags:?} must be last-wins and never inverted"
        );
    }
}

// --- F6 -------------------------------------------------------------------

#[test]
fn f6_move_task_refuses_a_prior_review_cycle_artifact_as_review_feedback() {
    let project = Project::new();
    walk_to_in_review(&project);
    let prior = prior_review_cycle(&project);
    let rows = project.rows().len();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "planned",
            "--agent",
            "claude",
            "--review-feedback-file",
            prior.to_str().unwrap(),
            "--no-auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        !output.status.success(),
        "a prior artifact is not fresh feedback: {output:?}"
    );
    let payload = single_json(&output);
    let message = payload["error"].as_str().expect("error message");
    assert!(
        message
            .contains("feedback_source is this WP's own review-cycle artifact (review-cycle-1.md)")
            && message.contains("pass the underlying reviewer feedback"),
        "got {message}"
    );
    assert_eq!(project.rows().len(), rows, "no row is written");
    assert_eq!(project.lane("WP01"), "in_review");
    assert!(
        !project
            .feature
            .join("tasks/WP01/review-cycle-2.md")
            .exists(),
        "no new cycle wraps the prior artifact"
    );

    // Content identity: a copy at a non-artifact path is refused too.
    let copy = project.root.join("feedback.md");
    fs::copy(&prior, &copy).unwrap();
    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "planned",
            "--agent",
            "claude",
            "--review-feedback-file",
            copy.to_str().unwrap(),
            "--no-auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(!output.status.success(), "{output:?}");
    let message = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        message.contains("feedback_source content parses as a review-cycle artifact")
            && message.contains("pass distinct reviewer feedback"),
        "got {message}"
    );
    assert!(
        !project
            .feature
            .join("tasks/WP01/review-cycle-2.md")
            .exists(),
        "no new cycle wraps a content copy"
    );

    // Positive control (FR-004): the same reviewer prose WITHOUT frontmatter is
    // admissible, even byte-identical to the prior artifact's body.
    let prose = project.root.join("prose-feedback.md");
    fs::write(&prose, "The prior review finding.\n").unwrap();
    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "planned",
            "--agent",
            "claude",
            "--review-feedback-file",
            prose.to_str().unwrap(),
            "--no-auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "plain reviewer prose is admissible: {output:?}"
    );
    let new_artifact =
        fs::read_to_string(project.feature.join("tasks/WP01/review-cycle-2.md")).unwrap();
    assert!(new_artifact.ends_with("---\n\nThe prior review finding.\n"));
}

#[test]
fn f6_move_task_refuses_a_prior_review_cycle_artifact_on_an_approval_edge() {
    let project = Project::new();
    walk_to_in_review(&project);
    let prior = prior_review_cycle(&project);
    let rows = project.rows().len();

    let output = run_cli(
        &project.root,
        &project.home(),
        &[
            "agent",
            "tasks",
            "move-task",
            "WP01",
            "--to",
            "approved",
            "--agent",
            "bob",
            "--reviewer",
            "alice",
            "--review-feedback-file",
            prior.to_str().unwrap(),
            "--no-auto-commit",
            "--mission",
            SLUG,
            "--json",
        ],
    );
    assert!(
        !output.status.success(),
        "the approval feedback_source is guarded too: {output:?}"
    );
    let message = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        message.contains("feedback_source is this WP's own review-cycle artifact")
            && message.contains("pass the underlying reviewer feedback"),
        "got {message}"
    );
    assert_eq!(project.rows().len(), rows, "no row is written");
    assert_eq!(project.lane("WP01"), "in_review");
    assert!(
        !project
            .feature
            .join("tasks/WP01/review-cycle-2.md")
            .exists(),
        "no approval cycle wraps the prior artifact"
    );
}

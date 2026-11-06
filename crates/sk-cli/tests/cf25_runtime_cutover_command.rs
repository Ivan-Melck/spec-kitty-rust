//! CF25 — the real `migrate backfill-runtime-state` command end to end.
//!
//! These drive `sk_cli::compat::migrate::run` — the exact entry the compatibility
//! dispatcher calls — over fresh `mktemp` repositories, so the exercised path is
//! the composed command: group-flag forwarding / refusal → `--mission` handle
//! resolution → the ONE runtime-cutover engine (`cutover_selected_mission` /
//! `cutover_repo_best_effort`, both Source walkers over
//! `sk_mission::migration_rescue::cutover_mission`) → the Source payload/summary
//! and exit contract. The owned lock-root composition is exercised through the
//! canonical ownership fact at the engine entry the CLI's owned caller would use.
//!
//! Native functional smokes only: no Source child process, no oracle, no
//! isolation pairing, no equivalence/readiness claim.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::migrate::{MigrateArgs, MigrateCommand, run};
use sk_git::BranchName;
use sk_install::migrations::completion_modern::rescue_spine::cutover_owned_checkout;
use sk_mission::{Mission, list_missions};
use sk_workspace::OwnedCheckout;

const MISSION_BRANCH: &str = "kitty/mission-my-mission-01KNXQS9";

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Spec Kitty Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Spec Kitty Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}:\nstdout: {}\nstderr: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, contents).expect("write file");
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root).expect("root");
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Spec Kitty Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    write(&root.join("README.md"), "# fixture\n");
    // Every real project has `.kittify/`; Source's `locate_project_root` requires it.
    write(
        &root.join(".kittify/config.yaml"),
        "agents:\n  available:\n  - claude\n",
    );
    git(root, &["add", "README.md", ".kittify/config.yaml"]);
    git(root, &["commit", "-m", "initial"]);
}

/// One legacy mission whose WP frontmatter still carries evictable runtime state.
fn write_mission(root: &Path, slug: &str, mission_id: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    let meta = json!({
        "mission_id": mission_id,
        "mission_slug": slug,
        "topology": "single_branch",
        "target_branch": "main",
        "mission_type": "software-dev",
    });
    write(
        &dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).expect("meta")),
    );
    write(&dir.join("spec.md"), "# spec\n");
    write(
        &dir.join("tasks/WP01-task.md"),
        "---\nwork_package_id: WP01\nlane: in_progress\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\nassignee: ana\ntitle: Task\n---\n\nbody\n",
    );
    dir
}

/// A fresh repository with one committed legacy mission per `(slug, mission_id)`.
fn fixture(missions: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("repo");
    init_repo(&root);
    for (slug, mission_id) in missions {
        write_mission(&root, slug, mission_id);
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "missions"]);
    (temp, root)
}

fn args(dry_run: bool, mission: Option<&str>, json: bool) -> MigrateArgs {
    MigrateArgs {
        dry_run,
        verbose: false,
        force: false,
        command: Some(MigrateCommand::BackfillRuntimeState {
            dry_run,
            mission: mission.map(str::to_string),
            json,
        }),
    }
}

fn status_phase(mission_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(mission_dir.join("meta.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("status_phase")
        .and_then(|phase| phase.as_str())
        .map(str::to_string)
}

fn events_path(root: &Path, slug: &str) -> PathBuf {
    root.join("kitty-specs")
        .join(slug)
        .join("status.events.jsonl")
}

#[test]
fn the_group_dry_run_is_forwarded_and_writes_nothing() {
    let (_temp, root) = fixture(&[("alpha", "01KNXQS9ATWWFXS3K5ZJ9E5003")]);
    // `--dry-run` typed BEFORE the subcommand: Source forwards it because the
    // subcommand declares the same flag. The subcommand's own flag stays false.
    let mut group = args(false, None, false);
    group.dry_run = true;
    assert_eq!(run(&root, &group).expect("dry run"), 0);

    assert!(
        !events_path(&root, "alpha").exists(),
        "a dry run seeds no events"
    );
    assert!(
        status_phase(&root.join("kitty-specs").join("alpha")).is_none(),
        "a dry run flips nothing"
    );
}

#[test]
fn a_group_flag_the_subcommand_does_not_declare_is_refused() {
    let (_temp, root) = fixture(&[("alpha", "01KNXQS9ATWWFXS3K5ZJ9E5003")]);
    let mut group = args(false, None, false);
    group.force = true;
    let error = run(&root, &group).expect_err("--force is not declared");
    assert_eq!(error.exit(), 2);
    assert!(error.message().contains("--force"), "{}", error.message());
    assert!(
        error.message().contains("migrate backfill-runtime-state"),
        "{}",
        error.message()
    );
}

#[test]
fn a_live_apply_seeds_verifies_and_flips_then_retry_is_byte_stable() {
    let (_temp, root) = fixture(&[
        ("alpha", "01KNXQS9ATWWFXS3K5ZJ9E5001"),
        ("beta", "01KNXQS9ATWWFXS3K5ZJ9E5002"),
    ]);
    assert_eq!(run(&root, &args(false, None, false)).expect("apply"), 0);

    for slug in ["alpha", "beta"] {
        let dir = root.join("kitty-specs").join(slug);
        assert_eq!(status_phase(&dir).as_deref(), Some("1"), "{slug}");
        let events = fs::read_to_string(events_path(&root, slug)).expect("seeded events");
        // The claim carrier plus the annotation seeds the legacy frontmatter holds.
        assert!(
            events.lines().count() >= 2,
            "{slug} seeded too few rows: {events}"
        );
        assert!(
            events.contains("migration:backfill_runtime_state"),
            "{slug}"
        );
        assert!(events.contains("\"to_lane\": \"claimed\""), "{slug}");
    }

    let before: Vec<String> = ["alpha", "beta"]
        .iter()
        .map(|slug| fs::read_to_string(events_path(&root, slug)).expect("events"))
        .collect();
    assert_eq!(
        run(&root, &args(false, None, false)).expect("retry"),
        0,
        "a converged corpus still exits 0"
    );
    for (index, slug) in ["alpha", "beta"].into_iter().enumerate() {
        let after = fs::read_to_string(events_path(&root, slug)).expect("events");
        assert_eq!(after, before[index], "{slug} re-seeded on a retry");
        assert_eq!(
            status_phase(&root.join("kitty-specs").join(slug)).as_deref(),
            Some("1")
        );
    }
}

#[test]
fn a_failed_verify_leaves_that_mission_unflipped_while_others_still_flip() {
    let broken_id = "01KNXQS9ATWWFXS3K5ZJ9E5001";
    let (_temp, root) = fixture(&[
        ("broken", broken_id),
        ("healthy", "01KNXQS9ATWWFXS3K5ZJ9E5003"),
    ]);
    // Seed the healthy mission for real and harvest one authentic seed row. Its
    // deterministic id is recomputed for the BROKEN mission's own seed namespace
    // (the public `deterministic_ulid` authority, not a private clone), so the
    // broken mission has a claim row at the exact id verify expects — with a
    // payload that diverges from its own legacy state.
    assert_eq!(
        run(&root, &args(false, Some("healthy"), false)).expect("healthy"),
        0
    );
    let harvested = fs::read_to_string(events_path(&root, "healthy"))
        .expect("healthy events")
        .lines()
        .next()
        .expect("claim carrier")
        .to_string();
    let mut row: Value = serde_json::from_str(&harvested).expect("claim row");
    row["event_id"] = json!(sk_status::migration_backfill::deterministic_ulid(&format!(
        "{broken_id}|WP01|claim"
    )));
    write(
        &events_path(&root, "broken"),
        &format!("{}\n", serde_json::to_string(&row).expect("row")),
    );

    let code = run(&root, &args(false, Some("broken"), true)).expect("broken run");
    assert_eq!(code, 1, "a failed verify exits 1");
    assert!(
        status_phase(&root.join("kitty-specs").join("broken")).is_none(),
        "the flip is unreachable after a failed verify"
    );
    assert_eq!(
        status_phase(&root.join("kitty-specs").join("healthy")).as_deref(),
        Some("1")
    );

    // Per-mission best-effort: the corpus walk reports the broken mission and
    // still exits non-zero for the run, without stranding another mission.
    let (_temp2, root2) = fixture(&[
        ("broken", broken_id),
        ("healthy", "01KNXQS9ATWWFXS3K5ZJ9E5003"),
    ]);
    assert_eq!(
        run(&root2, &args(false, Some("healthy"), false)).expect("healthy"),
        0
    );
    let harvested = fs::read_to_string(events_path(&root2, "healthy"))
        .expect("healthy events")
        .lines()
        .next()
        .expect("claim carrier")
        .to_string();
    let mut row: Value = serde_json::from_str(&harvested).expect("claim row");
    row["event_id"] = json!(sk_status::migration_backfill::deterministic_ulid(&format!(
        "{broken_id}|WP01|claim"
    )));
    write(
        &events_path(&root2, "broken"),
        &format!("{}\n", serde_json::to_string(&row).expect("row")),
    );
    assert_eq!(
        run(&root2, &args(false, None, false)).expect("corpus"),
        1,
        "one failed mission fails the run"
    );
    assert_eq!(
        status_phase(&root2.join("kitty-specs").join("healthy")).as_deref(),
        Some("1"),
        "the healthy mission is still flipped by the same walk"
    );
}

#[test]
fn an_unknown_handle_refuses_with_the_source_envelope_and_exit() {
    let (_temp, root) = fixture(&[("alpha", "01KNXQS9ATWWFXS3K5ZJ9E5003")]);
    let error = run(&root, &args(false, Some("no-such-mission"), false))
        .expect_err("unknown handle is refused");
    assert_eq!(error.exit(), 2, "human mode exits 2");
    assert!(
        error
            .message()
            .contains("No mission found for handle \"no-such-mission\"."),
        "{}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("the mission exists in kitty-specs/"),
        "{}",
        error.message()
    );

    let json_error = run(&root, &args(false, Some("no-such-mission"), true))
        .expect_err("unknown handle is refused in json mode");
    assert_eq!(json_error.exit(), 1, "json mode exits 1");
    let payload = json_error.to_json();
    assert_eq!(payload["error_code"], json!("MISSION_NOT_FOUND"));
    assert_eq!(payload["handle"], json!("no-such-mission"));
}

#[test]
fn a_missing_project_root_exits_one_without_writing() {
    let temp = tempfile::tempdir().expect("temp");
    assert_eq!(
        run(temp.path(), &args(false, None, false)).expect("no root"),
        1
    );
}

#[test]
fn the_owned_source_valid_path_flips_the_fact_mission_with_the_owned_lock_root() {
    let temp = tempfile::tempdir().expect("temp");
    let repository_root = temp.path().join("repo");
    init_repo(&repository_root);
    let dir = write_mission(&repository_root, "my-mission", "01KNXQS9ATWWFXS3K5ZJ9E5001");
    git(&repository_root, &["add", "-A"]);
    git(&repository_root, &["commit", "-m", "mission"]);
    git(&repository_root, &["branch", MISSION_BRANCH]);
    let owned_root = temp.path().join("owned-checkout");
    git(
        &repository_root,
        &[
            "worktree",
            "add",
            owned_root.to_str().expect("utf8"),
            MISSION_BRANCH,
        ],
    );

    let mission: Mission = list_missions(&owned_root)
        .expect("mission inventory")
        .into_iter()
        .find(|mission| mission.slug().as_str() == "my-mission")
        .expect("mission present in the owned checkout");
    let fact = OwnedCheckout::mint(
        &repository_root,
        &owned_root,
        &mission,
        &BranchName::parse(MISSION_BRANCH).expect("branch name"),
    )
    .expect("owned fact");

    let preview = cutover_owned_checkout(&repository_root, &fact, true).expect("preview");
    assert!(preview.abort.is_none(), "{preview:?}");
    assert!(status_phase(fact.mission_dir()).is_none(), "preview wrote");

    let report = cutover_owned_checkout(&repository_root, &fact, false).expect("apply");
    assert!(report.abort.is_none(), "{report:?}");
    assert_eq!(report.outcomes.len(), 1, "{report:?}");
    let outcome = &report.outcomes[0];
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert!(outcome.flipped, "{outcome:?}");
    // The seed and the flip both landed on the FACT's mission directory: the
    // seed's status lock took the fact's repository root, and the flip's did too.
    assert_eq!(
        status_phase(fact.mission_dir()).as_deref(),
        Some("1"),
        "{outcome:?}"
    );
    assert!(
        fact.mission_dir().join("status.events.jsonl").is_file(),
        "{outcome:?}"
    );
    // The repository root checkout's copy of the same mission is untouched.
    assert!(status_phase(&dir).is_none(), "the unowned copy was written");
}

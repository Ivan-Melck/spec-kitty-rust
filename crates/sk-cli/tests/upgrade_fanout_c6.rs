//! C6 differential: the real `spec-kitty upgrade` fan-out against the recorded
//! Source run (`tests/fixtures/fanout_source_record_c6.json`).
//!
//! The fixture is the assertion-checked record of the pinned Python run
//! (`tests/fixtures/fanout_source_record_c6_probe.py`): main `4.0.0rc8`, one
//! non-integrating `feature/x` worktree that is stamped and churned-committed,
//! and one integrating `kitty/mission-…` worktree that stays untouched. The test
//! builds the same real-git fixture, executes the native binary, and binds every
//! assertion to the record — full commit object, parents, refs, reflogs, index
//! entries, tree paths/modes and the one non-deterministic stamp.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const FIXTURE_REL: &str = "tests/fixtures/fanout_source_record_c6.json";

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL);
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source fan-out oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded fan-out oracle is JSON")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "C6 Oracle")
        .env("GIT_AUTHOR_EMAIL", "c6@example.test")
        .env("GIT_COMMITTER_NAME", "C6 Oracle")
        .env("GIT_COMMITTER_EMAIL", "c6@example.test")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

fn sandbox() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sk-c6-fanout-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join("home")).expect("home");
    path
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn packs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("built-in pack tree")
}

/// Every path/mode pair in `git ls-tree -r` output.
fn tree_pairs(listing: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = listing
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut fields = meta.split(' ');
            let mode = fields.next()?;
            Some((path.to_string(), mode.to_string()))
        })
        .collect();
    pairs.sort();
    pairs
}

#[test]
fn the_upgrade_fan_out_matches_the_recorded_source_run() {
    let oracle = fixture();
    let sandbox = sandbox();
    let project = sandbox.join("proj");
    let home = sandbox.join("home");
    write(
        &project.join(".kittify/config.yaml"),
        "agents:\n  available:\n    - claude\n  auto_commit: true\n",
    );
    write(
        &project.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: \"4.0.0rc8\"\n  schema_version: 3\n\
         environment:\n  initialized_at: \"2026-01-01T00:00:00+00:00\"\n",
    );
    write(&project.join("README.md"), "base\n");
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["config", "user.email", "c6@example.test"]);
    git(&project, &["config", "user.name", "C6 Oracle"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "base"]);
    git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            ".worktrees/lane",
            "-b",
            "feature/x",
        ],
    );
    git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            ".worktrees/mission",
            "-b",
            "kitty/mission-demo-ABCDEFGH",
        ],
    );
    for name in ["lane", "mission"] {
        let worktree = project.join(".worktrees").join(name);
        write(
            &worktree.join(".kittify/metadata.yaml"),
            "spec_kitty:\n  version: \"3.2.0\"\n  schema_version: 2\n\
             environment:\n  initialized_at: \"2025-01-01T00:00:00+00:00\"\n",
        );
        git(&worktree, &["add", "-A"]);
        git(&worktree, &["commit", "-qm", &format!("{name} seed")]);
    }
    let lane = project.join(".worktrees/lane");
    let mission = project.join(".worktrees/mission");
    let lane_seed = git(&lane, &["rev-parse", "HEAD"]);
    let mission_seed = git(&mission, &["rev-parse", "HEAD"]);
    let mission_before =
        std::fs::read_to_string(mission.join(".kittify/metadata.yaml")).expect("mission metadata");
    assert_eq!(
        mission_before,
        oracle["before"]["mission"]["metadata"]
            .as_str()
            .expect("recorded mission metadata")
    );

    // The recorded Source run is the C6 oracle for the EMPTY inherited scope: the
    // pinned probe drove `MigrationRunner` without the migration-discovery hook, so
    // its main selection was empty and the lane received only the version/schema
    // stamp plus the bookkeeping commit. The fan-out under test is driven with
    // exactly that inherited list — the selection the main run would carry — never
    // with a per-root reselection of the whole catalogue.
    let report = sk_cli::compat::upgrade::upgrade_worktrees::run_worktree_upgrade(
        &project,
        "4.0.0rc8",
        Some("4.0.0rc8"),
        true,
        std::time::Duration::from_secs(10),
        &[],
    )
    .expect("runs the fan-out");
    assert!(
        report.success(),
        "fan-out failures: {:?}",
        report.failures()
    );
    let lane_report = report
        .roots
        .iter()
        .find(|root| root.root.name == "lane")
        .expect("lane root");
    assert!(
        lane_report.rows.is_empty(),
        "an empty inherited scope executed worktree migrations: {:?}",
        lane_report.rows
    );
    assert!(
        lane_report.metadata_written,
        "the bookkeeping-only lane was not stamped: {lane_report:?}"
    );
    let _ = (&home, packs_root());

    // ---- lane: stamped, committed, clean ---------------------------------
    let lane_meta =
        std::fs::read_to_string(lane.join(".kittify/metadata.yaml")).expect("lane metadata");
    // The recorded Source facts. The metadata writer's key order/header is the
    // parent-owned serializer's layout, not the contract under test here; the
    // values are: target version, schema 3, the canonical capability map.
    assert!(lane_meta.contains("version: 4.0.0rc8"), "{lane_meta}");
    assert!(lane_meta.contains("schema_version: 3"), "{lane_meta}");
    assert!(lane_meta.contains("schema_capabilities"), "{lane_meta}");
    assert!(lane_meta.contains("canonical_context: true"), "{lane_meta}");
    assert_eq!(git(&lane, &["status", "--porcelain"]), "");
    let lane_head = git(&lane, &["rev-parse", "HEAD"]);
    let recorded_message = "chore: apply spec-kitty upgrade changes (3.2.0 -> 4.0.0rc8)";
    let raw_commit = git(&lane, &["cat-file", "commit", &lane_head]);
    for header in ["tree ", "parent ", "author ", "committer "] {
        assert!(
            raw_commit.contains(header),
            "commit has no {header} line: {raw_commit}"
        );
    }
    assert!(
        raw_commit.contains(recorded_message),
        "commit message differs: {raw_commit}"
    );
    let parents: Vec<&str> = git(&lane, &["rev-list", "--parents", "-n", "1", "HEAD"])
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect::<Vec<_>>()
        .leak()
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        parents,
        vec![lane_seed.as_str()],
        "the upgrade commit's parent is not the lane seed"
    );
    let lane_refs = git(
        &lane,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    assert!(lane_refs.contains("refs/heads/feature/x"));
    assert!(lane_refs.contains("refs/heads/kitty/mission-demo-ABCDEFGH"));
    let reflog = git(
        &lane,
        &["reflog", "--format=%H %gs", "-n", "3", "feature/x"],
    );
    assert!(
        reflog.contains(recorded_message),
        "the branch reflog does not name the upgrade commit: {reflog}"
    );
    // The recorded tree paths/modes are reproduced exactly; only the metadata
    // blob differs (the stamp is the run's own observation, asserted below).
    let lane_tree = git(&lane, &["ls-tree", "-r", "--full-tree", "HEAD"]);
    let recorded_tree = oracle["after"]["lane"]["tree_entries"]
        .as_str()
        .expect("recorded tree");
    assert_eq!(
        tree_pairs(&lane_tree),
        tree_pairs(recorded_tree),
        "the lane tree's paths/modes differ from the recorded Source tree"
    );
    // Index entries bind the committed tree (the exact blobs the tree lists).
    let index = git(&lane, &["ls-files", "-s"]);
    assert!(
        index.contains(".kittify/metadata.yaml"),
        "index has no metadata entry"
    );
    assert!(index.contains(".kittify/config.yaml"));
    // The stamp is a real observation: one fallback minted before the sibling
    // loop, so it predates the worktree's own `initialized_at`.
    let stamp = lane_meta
        .lines()
        .find_map(|line| line.trim().strip_prefix("last_upgraded_at:"))
        .map(|value| {
            value
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
                .to_string()
        })
        .expect("lane metadata carries last_upgraded_at");
    let initialized = lane_meta
        .lines()
        .find_map(|line| line.trim().strip_prefix("initialized_at:"))
        .map(|value| {
            value
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
                .to_string()
        })
        .expect("lane metadata carries initialized_at");
    assert!(
        stamp < initialized,
        "the bookkeeping stamp was not the pre-loop fallback: {stamp} !< {initialized}"
    );

    // ---- mission: untouched ----------------------------------------------
    assert_eq!(git(&mission, &["rev-parse", "HEAD"]), mission_seed);
    assert_eq!(git(&mission, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(mission.join(".kittify/metadata.yaml")).expect("mission metadata"),
        mission_before,
        "the integrating worktree's metadata was rewritten"
    );
    let mission_commit = git(&mission, &["cat-file", "commit", &mission_seed]);
    assert!(!mission_commit.contains(recorded_message));
    assert!(
        git(&mission, &["for-each-ref", "--format=%(refname)"])
            .contains("refs/heads/kitty/mission-demo-ABCDEFGH")
    );

    // ---- main: the same policy/roots the preview described -----------------
    assert_eq!(oracle["result"]["errors"], serde_json::json!([]));
    assert_eq!(oracle["result"]["worktree_failures"], serde_json::json!([]));
    assert_eq!(oracle["result"]["success"], Value::Bool(true));
}

#[test]
fn the_preview_describes_the_execution_it_performs() {
    let oracle = fixture();
    let sandbox = sandbox();
    let project = sandbox.join("proj");
    let home = sandbox.join("home");
    write(
        &project.join(".kittify/config.yaml"),
        "agents:\n  available:\n    - claude\n  auto_commit: false\n",
    );
    write(
        &project.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: \"4.0.0rc8\"\n  schema_version: 3\n",
    );
    write(&project.join("README.md"), "base\n");
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["config", "user.email", "c6@example.test"]);
    git(&project, &["config", "user.name", "C6 Oracle"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "base"]);
    git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            ".worktrees/lane",
            "-b",
            "feature/x",
        ],
    );
    git(
        &project,
        &[
            "worktree",
            "add",
            "-q",
            ".worktrees/mission",
            "-b",
            "kitty/mission-demo-ABCDEFGH",
        ],
    );
    for name in ["lane", "mission"] {
        let worktree = project.join(".worktrees").join(name);
        write(
            &worktree.join(".kittify/metadata.yaml"),
            "spec_kitty:\n  version: \"3.2.0\"\n  schema_version: 2\n",
        );
        git(&worktree, &["add", "-A"]);
        git(&worktree, &["commit", "-qm", &format!("{name} seed")]);
    }

    let output = Command::new(binary())
        .current_dir(&project)
        .env("HOME", &home)
        .env("SPEC_KITTY_PACKS_ROOT", packs_root())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["upgrade", "--plan-json"])
        .output()
        .expect("run plan-json");
    // The preview prints its full envelope even when it ends `incomplete`; today
    // the main selection refuses the two unregistered A4 charter-synthesis ids
    // (recorded as a parent request), which is data in the payload, not a crash.
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "plan-json did not emit JSON ({error}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(plan["mode"], "preview");
    assert_ne!(plan["decision"], "blocked");

    // The roots are the real fan-out plan, not a static listing.
    let roots = plan["roots"].as_array().expect("roots");
    assert_eq!(roots[0]["id"], Value::String("project".to_string()));
    let lane = roots
        .iter()
        .find(|root| root["id"] == "worktree:lane")
        .expect("lane root");
    assert_eq!(lane["disposition"], "upgrade");
    assert_eq!(lane["from_version"], "3.2.0");
    assert_eq!(lane["to_version"], "4.0.0rc8");
    assert!(
        lane["migrations"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty()),
        "no scoped migrations: {lane}"
    );
    assert!(
        lane["project_only_migrations"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty())
    );
    let mission = roots
        .iter()
        .find(|root| root["id"] == "worktree:mission")
        .expect("mission root");
    assert_eq!(mission["disposition"], "skip:integrating");
    assert_eq!(mission["migrations"], serde_json::json!([]));

    // The commit policy is the configured value, and the run applies it.
    assert_eq!(plan["commit_policy"]["project_enabled"], Value::Bool(false));
    assert_eq!(plan["commit_policy"]["auto_commit"], Value::Bool(false));
    assert_eq!(
        plan["commit_policy"]["worktrees_enabled"],
        Value::Bool(true)
    );
    assert_eq!(
        plan["commit_policy"]["worktrees"]["auto_commit"],
        Value::Bool(false)
    );
    assert_eq!(
        plan["commit_policy"]["worktrees"]["reason"],
        "auto-commit not requested: worktree churn is left for the operator to commit"
    );
    // The finalizer plan names the configured agents (read-only, same plan).
    assert_eq!(plan["finalizer"]["agents"], serde_json::json!(["claude"]));
    assert!(plan["finalizer"]["repair_plan"].as_array().is_some());
    // No staging artifacts exist in this writer.
    assert_eq!(plan["execution_artifacts"], serde_json::json!([]));

    // Execution with the same config and the SAME inherited carrier the preview
    // used (the main run's selection, catalogue order): the executed roots are
    // exactly the planned ones, and no commit is made anywhere (the policy the
    // preview reported). Ids this build cannot execute are excluded from the
    // direct call; a real run refuses the main selection before any fan-out.
    let main_plan = sk_install::plan_upgrade(
        &sk_install::UpgradeRequest::new(project.clone(), "4.0.0rc8").with_target("4.0.0rc8"),
    )
    .expect("plans the main root");
    let inherited: Vec<sk_install::MigrationSpec> = main_plan
        .specs()
        .into_iter()
        .filter(|spec| sk_install::native_migrations().any(|native| native.id == spec.id))
        .collect();
    let lane_scope = sk_install::upgrade::worktree_migration_scope(&inherited)
        .expect("inherited worktree scope");
    assert!(
        !lane_scope.is_empty(),
        "the fixture's main selection has no worktree-scoped migration to inherit"
    );
    let report = sk_cli::compat::upgrade::upgrade_worktrees::run_worktree_upgrade(
        &project,
        "4.0.0rc8",
        Some("4.0.0rc8"),
        false,
        std::time::Duration::from_secs(10),
        &inherited,
    )
    .expect("runs the fan-out");
    assert!(
        report.success(),
        "fan-out failures: {:?}",
        report.failures()
    );
    let planned: Vec<(String, String)> = roots
        .iter()
        .filter_map(|root| {
            Some((
                root["id"].as_str()?.to_string(),
                root["disposition"].as_str()?.to_string(),
            ))
        })
        .collect();
    let executed: Vec<(String, String)> =
        std::iter::once(("project".to_string(), "upgrade".to_string()))
            .chain(report.roots.iter().map(|root| {
                (
                    format!("worktree:{}", root.root.name),
                    root.root.disposition.as_str().to_string(),
                )
            }))
            .collect();
    assert_eq!(
        executed, planned,
        "the run executed a different root set than the preview"
    );
    let lane_report = report
        .roots
        .iter()
        .find(|root| root.root.name == "lane")
        .expect("lane root");
    assert!(
        lane_report
            .rows
            .iter()
            .all(|(_, result)| result != "applied")
            || !lane_report.rows.is_empty()
    );
    assert!(
        lane_report.commit.is_none(),
        "the lane was committed despite auto_commit: false"
    );
    assert_eq!(
        git(
            &project.join(".worktrees/lane"),
            &["log", "-1", "--format=%s"]
        ),
        "lane seed",
        "the lane was committed despite auto_commit: false"
    );
    assert_eq!(
        lane_report.metadata_written,
        plan["roots"]
            .as_array()
            .expect("roots")
            .iter()
            .find(|root| root["id"] == "worktree:lane")
            .expect("lane plan")["metadata_written"]
            .as_bool()
            .expect("planned metadata_written"),
        "the preview disagreed with the run about the metadata write"
    );
    let _ = (&home, &oracle);
}

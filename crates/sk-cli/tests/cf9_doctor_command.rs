//! CF9 focused smoke: the native `doctor` diagnosis against real fixtures.
//!
//! These exercise the actual canonical authorities — the bounded `meta.json`
//! reader, the `sk-git` index/worktree seam, the `sk-workspace` lane store, the
//! `sk-doctrine` activation ledger and the `sk-mission` definition resolver — on
//! fresh `mktemp` fixtures and a real `git init` repository.
//!
//! They are **not** a whole-command parity campaign. Not covered here (deferred
//! deep proof): the reference's `--fix` destructive arms, a corrupt git index,
//! non-UTF-8 mission directory names, platform-specific path spellings, race and
//! concurrency behaviour, and any byte-level differential of the human output.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sk_cli::compat::doctor::{self, facts};
use sk_workspace::doctor::{
    self as diagnosis, IdentityState, MissionTypeState, StateRootEnvironment, TrackedPosture,
    audit_identity, audit_mission_types, audit_state_roots, audit_topology, parse_fail_on,
    scan_workspace_husks,
};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn mission(root: &Path, slug: &str, meta: &str) -> PathBuf {
    let dir = root.join("kitty-specs").join(slug);
    fs::create_dir_all(&dir).expect("mission dir");
    fs::write(dir.join("meta.json"), meta).expect("meta write");
    dir
}

fn row<'a>(audit: &'a diagnosis::IdentityAudit, slug: &str) -> &'a diagnosis::IdentityRow {
    audit
        .rows
        .iter()
        .find(|row| row.slug == slug)
        .expect("identity row")
}

/// Run `git` in `root` with an isolated configuration and no system config.
fn git(root: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "CF9 Fixture")
        .env("GIT_AUTHOR_EMAIL", "cf9@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF9 Fixture")
        .env("GIT_COMMITTER_EMAIL", "cf9@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_repository(root: &Path) {
    git(root, &["init", "--quiet", "--initial-branch=main"]);
    // A commit so the index exists and `ls-files` answers.
    write(&root.join("README.md"), "fixture\n");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "--quiet", "-m", "fixture"]);
}

/// A byte/mode/mtime/inode snapshot of every entry below `root`.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut entries = BTreeMap::new();
    fn walk(base: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, String>) {
        let metadata = fs::symlink_metadata(path).expect("metadata");
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = 0u32;
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        };
        #[cfg(not(unix))]
        let inode = 0u64;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let contents = if metadata.is_file() {
            fs::read(path)
                .map(|bytes| format!("{}:{}", bytes.len(), fnv1a(&bytes)))
                .unwrap_or_else(|_| "unreadable".to_owned())
        } else {
            String::new()
        };
        entries.insert(
            path.strip_prefix(base).unwrap_or(path).to_path_buf(),
            format!("mode={mode} inode={inode} mtime={modified} {contents}"),
        );
        if metadata.is_dir() {
            let mut children: Vec<PathBuf> = fs::read_dir(path)
                .expect("read dir")
                .flatten()
                .map(|entry| entry.path())
                .collect();
            children.sort();
            for child in children {
                walk(base, &child, entries);
            }
        }
    }
    walk(root, root, &mut entries);
    entries
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn environment(root: &Path) -> StateRootEnvironment {
    StateRootEnvironment {
        home: root.join("home"),
        kittify_home: root.join("home/.kittify"),
        runtime_base: root.join("home/.spec-kitty"),
    }
}

// ---------------------------------------------------------------------------
// identity
// ---------------------------------------------------------------------------

#[test]
fn identity_healthy_fixture_is_all_assigned_and_reports_no_collisions() {
    let temp = tempfile::tempdir().unwrap();
    mission(
        temp.path(),
        "001-first",
        "{\"mission_id\":\"01AAAAAAAAAAAAAAAAAAAAAAAA\",\"mission_number\":1}",
    );
    mission(
        temp.path(),
        "002-second",
        "{\"mission_id\":\"01BBBBBBBBBBBBBBBBBBBBBBBB\",\"mission_number\":2}",
    );

    let audit = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit");
    assert_eq!(audit.rows.len(), 2);
    assert_eq!(row(&audit, "001-first").state, IdentityState::Assigned);
    assert_eq!(audit.counts()["assigned"], 2);
    assert_eq!(audit.counts()["residue"], 0);
    assert!(audit.duplicate_prefixes.is_empty());
    assert!(audit.ambiguous_selectors.is_empty());

    let payload = audit.to_json(false);
    assert_eq!(payload["summary"]["assigned"], 2);
    assert_eq!(
        payload["missions"][0]["path"],
        Value::String(
            temp.path()
                .join("kitty-specs/001-first")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(payload["missions"][0]["mission_number"], 1);
    assert_eq!(payload["fail_on_triggered"], false);
    assert!(payload.get("residue_remedies").is_none());
}

#[test]
fn identity_legacy_orphan_and_residue_are_findings_with_remedies() {
    let temp = tempfile::tempdir().unwrap();
    mission(temp.path(), "010-legacy", "{\"mission_number\":5}");
    mission(temp.path(), "011-orphan", "{}");
    fs::create_dir_all(temp.path().join("kitty-specs/012-residue")).unwrap();

    let audit = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit");
    assert_eq!(row(&audit, "010-legacy").state, IdentityState::Legacy);
    assert_eq!(row(&audit, "010-legacy").mission_number, Some(5));
    assert_eq!(row(&audit, "011-orphan").state, IdentityState::Orphan);
    assert_eq!(row(&audit, "012-residue").state, IdentityState::Residue);

    let triggered = audit.any_in(&[IdentityState::Legacy].into_iter().collect());
    assert!(triggered);
    let payload = audit.to_json(triggered);
    assert_eq!(payload["fail_on_triggered"], true);
    assert!(
        payload["residue_remedies"]["no_meta"]
            .as_str()
            .unwrap()
            .contains("no mission_id")
            || payload["residue_remedies"]["no_meta"]
                .as_str()
                .unwrap()
                .contains("meta.json")
    );
}

#[test]
fn identity_duplicate_prefix_and_ambiguous_selector_are_reported() {
    let temp = tempfile::tempdir().unwrap();
    mission(
        temp.path(),
        "020-alpha",
        "{\"mission_id\":\"01CCCCCCCCCCCCCCCCCCCCCCCC\",\"mission_number\":1}",
    );
    mission(
        temp.path(),
        "020-beta",
        "{\"mission_id\":\"01DDDDDDDDDDDDDDDDDDDDDDDD\",\"mission_number\":2}",
    );
    mission(
        temp.path(),
        "021-alpha",
        "{\"mission_id\":\"01EEEEEEEEEEEEEEEEEEEEEEEE\",\"mission_number\":3}",
    );

    let audit = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit");
    assert_eq!(audit.duplicate_prefixes["020"].len(), 2);
    assert_eq!(audit.ambiguous_selectors["alpha"].len(), 2);
    let payload = audit.to_json(false);
    assert_eq!(
        payload["duplicate_prefixes"]["020"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        payload["ambiguous_selectors"]["alpha"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn identity_scoping_reports_the_child_or_refuses_an_unknown_handle() {
    let temp = tempfile::tempdir().unwrap();
    mission(
        temp.path(),
        "030-scoped",
        "{\"mission_id\":\"01FFFFFFFFFFFFFFFFFFFFFFFF\",\"mission_number\":1}",
    );
    let scoped = audit_identity(temp.path(), Some("030-scoped"), TrackedPosture::Strict)
        .unwrap()
        .expect("known");
    assert_eq!(scoped.rows.len(), 1);
    assert!(
        audit_identity(temp.path(), Some("031-absent"), TrackedPosture::Strict)
            .unwrap()
            .is_none()
    );
}

#[test]
fn identity_uses_the_git_index_to_tell_residue_from_an_idless_mission() {
    let temp = tempfile::tempdir().unwrap();
    git_repository(temp.path());
    let dir = mission(temp.path(), "040-idless", "{\"slug\":\"idless\"}");

    let untracked = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit");
    assert_eq!(row(&untracked, "040-idless").state, IdentityState::Residue);
    assert_eq!(
        row(&untracked, "040-idless").residue_reason,
        Some(diagnosis::ResidueReason::UntrackedIdlessMeta)
    );

    git(
        temp.path(),
        &["add", dir.join("meta.json").to_str().unwrap()],
    );
    let tracked = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit");
    assert_eq!(
        row(&tracked, "040-idless").state,
        IdentityState::Orphan,
        "a git-tracked id-less meta.json is a Mission row awaiting backfill"
    );
}

// ---------------------------------------------------------------------------
// topology
// ---------------------------------------------------------------------------

#[test]
fn topology_reports_stored_values_and_both_structural_findings() {
    let temp = tempfile::tempdir().unwrap();
    mission(temp.path(), "050-unset", "{}");
    let code_lane = mission(
        temp.path(),
        "051-code-lane",
        "{\"topology\":\"single_branch\",\"flattened\":false}",
    );
    write(
        &code_lane.join("lanes.json"),
        r#"{"version":1,"mission_slug":"051-code-lane","mission_branch":"kitty/mission-051","target_branch":"main","lanes":[{"lane_id":"lane-a","wp_ids":["WP01"]}],"computed_at":"2026-01-01T00:00:00+00:00","computed_from":"fixture"}"#,
    );
    let corrupt = mission(
        temp.path(),
        "052-corrupt-lanes",
        "{\"topology\":\"single_branch\"}",
    );
    write(&corrupt.join("lanes.json"), "{not json");
    mission(temp.path(), "053-corrupt-meta", "{[oops");

    let audit = audit_topology(temp.path(), None).unwrap().expect("audit");
    let by_slug = |slug: &str| {
        audit
            .rows
            .iter()
            .find(|row| row.slug == slug)
            .expect("topology row")
    };
    assert_eq!(by_slug("050-unset").topology, None);
    assert_eq!(by_slug("050-unset").error, None);
    assert_eq!(by_slug("050-unset").finding, None);
    assert_eq!(
        by_slug("051-code-lane").finding,
        Some(diagnosis::TopologyFinding::SingleBranchCodeLanesUnmigrated)
    );
    assert_eq!(
        by_slug("052-corrupt-lanes").finding,
        Some(diagnosis::TopologyFinding::LanesManifestUnreadable)
    );
    assert!(by_slug("053-corrupt-meta").error.is_some());

    let payload = audit.to_json();
    let rows = payload["missions"].as_array().unwrap();
    assert_eq!(rows[1]["topology"], "single_branch");
    assert_eq!(rows[1]["finding"], "SINGLE_BRANCH_CODE_LANES_UNMIGRATED");
    assert!(
        rows[1]["remedy"]
            .as_str()
            .unwrap()
            .contains("--restamp-single-branch")
    );
    assert_eq!(rows[2]["finding"], "LANES_MANIFEST_UNREADABLE");
}

// ---------------------------------------------------------------------------
// mission-type
// ---------------------------------------------------------------------------

#[test]
fn mission_type_six_states_resolve_through_the_activation_ledger_and_rosters() {
    let temp = tempfile::tempdir().unwrap();
    write(
        &temp.path().join(".kittify/charter/activation.yaml"),
        "mission_type_activations:\n- software-dev\n- activated-ghost\n",
    );
    // The sparse registration makes `software-dev` resolvable without a full
    // mission.yaml tree; `activated-ghost` is activated but has none anywhere.
    write(
        &temp
            .path()
            .join(".kittify/missions/mission_types/software-dev.yaml"),
        "id: software-dev\n",
    );
    mission(
        temp.path(),
        "060-resolved",
        "{\"mission_id\":\"01GGGGGGGGGGGGGGGGGGGGGGGG\",\"mission_type\":\"  software-dev  \"}",
    );
    mission(
        temp.path(),
        "061-unresolvable",
        "{\"mission_id\":\"01HHHHHHHHHHHHHHHHHHHHHHHH\",\"mission_type\":\"activated-ghost\"}",
    );
    mission(
        temp.path(),
        "062-unknown",
        "{\"mission_id\":\"01IIIIIIIIIIIIIIIIIIIIIIII\",\"mission_type\":\"cf9-never-activated-xyz\"}",
    );
    mission(
        temp.path(),
        "063-typeless",
        "{\"mission_id\":\"01JJJJJJJJJJJJJJJJJJJJJJJJ\"}",
    );
    mission(
        temp.path(),
        "064-legacy",
        "{\"mission_id\":\"01KKKKKKKKKKKKKKKKKKKKKKKK\",\"mission\":\"software-dev\"}",
    );
    mission(temp.path(), "065-error", "{[oops");

    let health = facts::CharterMissionTypeHealth::resolve(temp.path()).expect("facts");
    let audit = audit_mission_types(temp.path(), &health).unwrap();
    let state = |slug: &str| {
        audit
            .rows
            .iter()
            .find(|row| row.slug == slug)
            .expect("mission-type row")
            .state
    };
    assert_eq!(state("060-resolved"), MissionTypeState::Resolved);
    assert_eq!(
        state("061-unresolvable"),
        MissionTypeState::ActivatedUnresolvable
    );
    assert_eq!(state("062-unknown"), MissionTypeState::Unknown);
    assert_eq!(state("063-typeless"), MissionTypeState::Typeless);
    assert_eq!(state("064-legacy"), MissionTypeState::LegacyKeyOnly);
    assert_eq!(state("065-error"), MissionTypeState::Error);

    let fail_on = parse_fail_on(Some("unknown,activated-unresolvable")).unwrap();
    assert!(audit.any_in(&fail_on));
    let payload = audit.to_json(true);
    assert_eq!(payload["summary"]["resolved"], 1);
    assert_eq!(payload["fail_on_triggered"], true);
}

// ---------------------------------------------------------------------------
// state-roots
// ---------------------------------------------------------------------------

#[test]
fn state_roots_reports_every_surface_and_warns_on_uncovered_runtime_state() {
    let temp = tempfile::tempdir().unwrap();
    git_repository(temp.path());
    write(
        &temp.path().join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    );
    write(
        &temp.path().join(".kittify/workspaces/060-abcd-WP01.json"),
        "{\"lane\":\"lane-a\"}\n",
    );

    let audit = audit_state_roots(temp.path(), &environment(temp.path())).unwrap();
    assert_eq!(audit.surfaces.len(), 62);
    assert!(!audit.healthy());
    assert!(
        audit
            .warnings
            .iter()
            .any(|warning| warning.contains("workspace_context")),
        "warnings: {:?}",
        audit.warnings
    );
    let payload = audit.to_json();
    assert_eq!(payload["healthy"], false);
    assert_eq!(payload["surfaces"].as_array().unwrap().len(), 62);
    assert_eq!(payload["roots"].as_array().unwrap().len(), 3);

    write(&temp.path().join(".gitignore"), ".kittify/workspaces/\n");
    let covered = audit_state_roots(temp.path(), &environment(temp.path())).unwrap();
    assert!(
        !covered
            .warnings
            .iter()
            .any(|warning| warning.contains("workspace_context")),
        "warnings: {:?}",
        covered.warnings
    );
}

// ---------------------------------------------------------------------------
// workspaces
// ---------------------------------------------------------------------------

#[test]
fn workspaces_reports_husks_and_the_adapter_refuses_the_fix() {
    let temp = tempfile::tempdir().unwrap();
    git_repository(temp.path());
    fs::create_dir_all(temp.path().join(".worktrees/070-abcd-lane-a")).unwrap();
    write(
        &temp.path().join(".worktrees/070-abcd-lane-a/leftover.txt"),
        "x\n",
    );

    let report = scan_workspace_husks(temp.path()).unwrap();
    assert!(!report.healthy());
    assert_eq!(report.husks.len(), 1);
    assert_eq!(report.husks[0].registered, Some(false));
    let payload = report.to_json();
    assert_eq!(payload["husks"][0]["path"], ".worktrees/070-abcd-lane-a");

    let args = doctor::parse(&["doctor", "workspaces", "--fix"]).unwrap();
    let error = doctor::run(&args).unwrap_err();
    assert_eq!(error.exit(), 2);
    assert_eq!(
        error.to_json()["error_code"],
        Value::String("UNSUPPORTED_DOCTOR_WORKSPACES_FIX".to_owned())
    );
}

// ---------------------------------------------------------------------------
// read-only invariants
// ---------------------------------------------------------------------------

#[test]
fn every_implemented_diagnosis_leaves_the_repository_bytes_untouched() {
    let temp = tempfile::tempdir().unwrap();
    git_repository(temp.path());
    write(
        &temp.path().join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    );
    write(
        &temp.path().join(".kittify/charter/activation.yaml"),
        "mission_type_activations:\n- software-dev\n",
    );
    mission(
        temp.path(),
        "080-assigned",
        "{\"mission_id\":\"01LLLLLLLLLLLLLLLLLLLLLLLL\",\"mission_number\":9,\"topology\":\"single_branch\"}",
    );
    mission(temp.path(), "081-corrupt", "{[oops");
    write(
        &temp.path().join(".kittify/runtime/runs/run.json"),
        "{\"state\":\"running\"}\n",
    );
    fs::create_dir_all(temp.path().join(".worktrees/082-abcd-lane-a")).unwrap();

    let before = snapshot(temp.path());
    let hook_count = fs::read_dir(temp.path().join(".git/hooks"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);

    let identity = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("identity");
    let _ = identity.to_json(true);
    let topology = audit_topology(temp.path(), None)
        .unwrap()
        .expect("topology");
    let _ = topology.to_json();
    let health = facts::CharterMissionTypeHealth::resolve(temp.path()).expect("facts");
    let mission_types = audit_mission_types(temp.path(), &health).unwrap();
    let _ = mission_types.to_json(true);
    let roots = audit_state_roots(temp.path(), &environment(temp.path())).unwrap();
    let _ = roots.to_json();
    let husks = scan_workspace_husks(temp.path()).unwrap();
    let _ = husks.to_json();

    let after = snapshot(temp.path());
    assert_eq!(
        before, after,
        "a read-only diagnosis must not touch the tree"
    );
    assert_eq!(
        hook_count,
        fs::read_dir(temp.path().join(".git/hooks"))
            .map(|entries| entries.flatten().count())
            .unwrap_or(0),
        "no hook may be installed"
    );
}

// ---------------------------------------------------------------------------
// adapter surface
// ---------------------------------------------------------------------------

#[test]
fn the_adapter_parses_the_upstream_options_and_refuses_mutating_leaves() {
    for argv in [
        vec![
            "doctor",
            "identity",
            "--json",
            "--mission",
            "083-foo",
            "--fail-on",
            "legacy",
        ],
        vec!["doctor", "topology", "--mission", "083-foo"],
        vec!["doctor", "mission-type", "--json", "--fail-on", "unknown"],
        vec!["doctor", "state-roots", "--json"],
        vec!["doctor", "workspaces", "--json"],
    ] {
        assert!(doctor::parse(&argv).is_ok(), "{argv:?}");
    }

    // A misspelled `--fail-on` state is a usage refusal, not a vacuous green.
    let args = doctor::parse(&["doctor", "mission-type", "--fail-on", "unkown"]).unwrap();
    let error = doctor::run(&args).unwrap_err();
    assert_eq!(error.exit(), 2);
    assert!(error.message().contains("valid states are"));

    // Refused leaves name their code and their remaining coverage.
    for argv in [
        vec!["doctor", "cutover"],
        vec!["doctor", "sparse-checkout"],
        vec!["doctor", "mission-state", "--fix"],
        vec!["doctor", "decisions", "--mission", "083-foo", "--repair"],
    ] {
        let args = doctor::parse(&argv).unwrap();
        let error = doctor::run(&args).unwrap_err();
        assert_eq!(error.exit(), 2, "{argv:?}");
        let payload = error.to_json();
        assert!(payload["error_code"].is_string(), "{argv:?}");
        assert!(
            payload["coverage"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{argv:?} must name its remaining coverage"
        );
    }
}

/// The exact root-CLI shape the parent dispatch patch uses: a `Command::Doctor`
/// variant carrying `DoctorArgs`. If this parses, the requested root enum edit
/// is additive and shape-correct.
#[test]
fn the_root_dispatch_shape_for_the_doctor_group_parses() {
    #[derive(clap::Subcommand)]
    enum RootCommand {
        Doctor(doctor::DoctorArgs),
    }
    #[derive(clap::Parser)]
    struct Root {
        #[command(subcommand)]
        command: RootCommand,
    }

    use clap::Parser as _;
    let cli = Root::try_parse_from(["spec-kitty", "doctor", "identity", "--json"]).expect("parse");
    let RootCommand::Doctor(args) = cli.command;
    match args.command.expect("leaf") {
        doctor::DoctorCommand::Identity { json, mission, .. } => {
            assert!(json);
            assert!(mission.is_none());
        }
        other => panic!("unexpected leaf: {other:?}"),
    }
}

#[test]
fn identity_json_records_are_stable_across_two_runs() {
    let temp = tempfile::tempdir().unwrap();
    mission(
        temp.path(),
        "090-stable",
        "{\"mission_id\":\"01MMMMMMMMMMMMMMMMMMMMMMMM\",\"mission_number\":4}",
    );
    let first = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit")
        .to_json(false);
    let second = audit_identity(temp.path(), None, TrackedPosture::Strict)
        .unwrap()
        .expect("audit")
        .to_json(false);
    assert_eq!(first, second);
}

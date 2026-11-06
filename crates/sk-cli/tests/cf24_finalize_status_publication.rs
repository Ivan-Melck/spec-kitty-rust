//! CF24: canonical finalize **status publication** and the branch-ref read
//! surface.
//!
//! These drive `sk_cli::compat::mission::finalize_tasks` — the exact entry the
//! compatibility dispatcher calls — over fresh, independent Git fixtures:
//!
//! * on a coordination-routed Mission every `genesis -> planned` seed is
//!   **published**: one path-scoped commit of that seed's status artifacts on the
//!   coordination branch, with Git's own identity, parent, tree and message, and
//!   no status file left untracked;
//! * a re-run publishes nothing and seeds nothing (no duplicate rows, no
//!   duplicate commits);
//! * a PRIMARY-partition Mission keeps the reference's uncommitted arm (files on
//!   disk, HEAD untouched);
//! * a refused seed commit cuts back only that seed's own row and reports the
//!   real failure phase, leaving this run's completed prompt/lanes writes alone;
//! * `--validate-only` reads the real initialized ids out of an **unmaterialized**
//!   coordination surface's branch ref without recreating the worktree, taking a
//!   lock, moving a ref or writing a byte.
//!
//! Native functional contracts only. These are NOT Source isolation/pairing
//! proof: no oracle runs here, the fixtures are ordinary `mktemp` directories
//! with their own `git init`, and nothing asserts full parity.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::mission::finalize_tasks;

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const MID8: &str = "01KN2371";
const SLUG: &str = "demo";

const WP01: &str = "---\nwork_package_id: WP01\ntitle: Alpha\nexecution_mode: code_change\nowned_files:\n  - src/alpha.rs\ndependencies: []\n---\n\n# WP01 Alpha\n\nAlpha body.\n";

const WP02: &str = "---\nwork_package_id: WP02\ntitle: Beta\nexecution_mode: code_change\nowned_files:\n  - src/beta.rs\ndependencies: []\n---\n\n# WP02 Beta\n\nBeta body.\n";

const TASKS_MD: &str = "# Tasks\n\n## WP01 - Alpha\n\n- Dependencies: none\n\n## WP02 - Beta\n\n- Dependencies: WP01\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("read fixture")
}

fn git(dir: &Path, home: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A fresh, independent Mission repository.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(topology: &str) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).expect("repo");
        fs::create_dir_all(&home).expect("home");
        git(&root, &home, &["init", "-q", "-b", "main"]);
        // The publisher's Git child inherits this process environment, so the
        // fixture pins the identity in repository-local config: the commit then
        // carries real, assertable author/committer facts.
        git(&root, &home, &["config", "user.name", "Test"]);
        git(&root, &home, &["config", "user.email", "test@example.com"]);
        write(&root.join("README.md"), "# Demo\n");
        write(&root.join("src/alpha.rs"), "// alpha\n");
        write(&root.join("src/beta.rs"), "// beta\n");
        git(&root, &home, &["add", "-A"]);
        git(&root, &home, &["commit", "-qm", "init"]);

        let mut meta = json!({
            "created_at": "2026-11-02T10:00:00Z",
            "flattened": false,
            "friendly_name": "Demo",
            "mission_id": MISSION_ID,
            "mission_number": 7,
            "mission_slug": SLUG,
            "mission_type": "software-dev",
            "slug": SLUG,
            "target_branch": "main",
            "topology": topology,
        });
        if sk_mission::MissionTopology::Coord.as_str() == topology
            || sk_mission::MissionTopology::LanesWithCoord.as_str() == topology
        {
            meta["coordination_branch"] = json!(sk_workspace::coord_reconstruct_branch(SLUG, MID8));
        }
        let mission = root.join("kitty-specs").join(SLUG);
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&meta).expect("meta json"),
        );
        write(&mission.join("spec.md"), "# Demo spec\n");
        write(&mission.join("plan.md"), "# Demo plan\n");
        write(&mission.join("tasks.md"), TASKS_MD);
        write(&mission.join("tasks/WP01-alpha.md"), WP01);
        write(&mission.join("tasks/WP02-beta.md"), WP02);
        git(&root, &home, &["add", "-A"]);
        git(&root, &home, &["commit", "-qm", "mission scaffold"]);
        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn mission(&self) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.mission().join(relative)
    }

    fn coord_branch(&self) -> String {
        sk_workspace::coord_reconstruct_branch(SLUG, MID8)
    }

    fn coord_worktree(&self) -> PathBuf {
        self.root
            .join(".worktrees")
            .join(sk_workspace::coord_dir_name(SLUG, MID8))
    }

    fn coord_mission(&self) -> PathBuf {
        self.coord_worktree()
            .join("kitty-specs")
            .join(sk_workspace::coord_mission_dir_name(SLUG, MID8))
    }

    fn coord_path(&self, relative: &str) -> PathBuf {
        self.coord_mission().join(relative)
    }

    fn workspace(&self) -> sk_workspace::MissionWorkspace {
        sk_workspace::ProjectWorkspace::open(&self.root)
            .expect("open workspace")
            .mission(SLUG)
            .expect("load mission workspace")
    }

    /// Mint + establish the coordination surface through the canonical owner,
    /// then make it MATERIALIZED (its Mission directory exists).
    fn coord_materialized(self) -> Self {
        let policy = sk_workspace::ProtectionPolicy::resolve(&self.root).expect("policy");
        sk_workspace::bootstrap_coordination(&self.workspace(), &policy)
            .expect("coordination bootstrap");
        // An empty directory is enough for the probe's `Materialized` state and
        // leaves `git status` clean (Git tracks no empty directories), so a
        // cleanliness assertion stays about the status artifacts.
        fs::create_dir_all(self.coord_mission()).expect("coord mission dir");
        self
    }

    /// Remove the worktree, keeping the branch (and its committed log).
    fn without_coordination_worktree(self) -> Self {
        if self.coord_worktree().exists() {
            git(
                &self.root,
                &self.home,
                &[
                    "worktree",
                    "remove",
                    "--force",
                    self.coord_worktree().to_str().expect("utf-8 path"),
                ],
            );
        }
        assert!(!self.coord_worktree().exists(), "worktree removed");
        self
    }

    fn finalize(&self, validate_only: bool) -> Result<Value, sk_cli::compat::support::CompatError> {
        finalize_tasks(&self.root, Some(SLUG), validate_only, true)
    }

    fn branch_tip(&self, branch: &str) -> String {
        git(
            &self.root,
            &self.home,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
        .trim()
        .to_owned()
    }

    fn commit_count(&self, branch: &str) -> usize {
        git(
            &self.root,
            &self.home,
            &["rev-list", "--count", &format!("refs/heads/{branch}")],
        )
        .trim()
        .parse()
        .expect("commit count")
    }

    /// The exact `parent subject|author|email|committer|email` rows for a range.
    fn commit_rows(&self, range: &str) -> Vec<String> {
        let output = git(
            &self.root,
            &self.home,
            &[
                "log",
                "--reverse",
                "--format=%P|%s|%an|%ae|%cn|%ce",
                range,
                "--",
            ],
        );
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The committed bytes of a path at a branch ref.
    fn committed_blob(&self, branch: &str, relative: &str) -> Option<String> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["show", &format!("refs/heads/{branch}:{relative}")])
            .output()
            .expect("run git show");
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The working-tree status of the coordination checkout.
    fn coord_status(&self) -> String {
        git(
            &self.coord_worktree(),
            &self.home,
            &["status", "--porcelain"],
        )
    }

    /// Every file under the repository root except Git's own directory, plus the
    /// ref list and HEAD — the whole footprint a dry run must not touch.
    fn footprint(&self) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![self.root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let relative = path
                        .strip_prefix(&self.root)
                        .expect("under root")
                        .to_string_lossy()
                        .into_owned();
                    files.insert(relative, fs::read(&path).expect("read footprint file"));
                }
            }
        }
        let refs = git(
            &self.root,
            &self.home,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        files.insert("#refs".to_owned(), refs.into_bytes());
        let head = git(&self.root, &self.home, &["rev-parse", "--verify", "HEAD"]);
        files.insert("#head".to_owned(), head.into_bytes());
        let worktrees = git(&self.root, &self.home, &["worktree", "list", "--porcelain"]);
        files.insert("#worktrees".to_owned(), worktrees.into_bytes());
        files
    }

    /// The parsed status rows committed at the coordination branch tip.
    fn committed_status_rows(&self) -> Vec<Value> {
        let relative = format!(
            "kitty-specs/{}/status.events.jsonl",
            sk_workspace::coord_mission_dir_name(SLUG, MID8)
        );
        let Some(text) = self.committed_blob(&self.coord_branch(), &relative) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("canonical event row"))
            .collect()
    }
}

#[test]
fn a_coordination_seed_publishes_one_commit_per_work_package() {
    let fixture = Fixture::new(sk_mission::MissionTopology::Coord.as_str()).coord_materialized();
    let branch = fixture.coord_branch();
    let before_tip = fixture.branch_tip(&branch);
    let before_count = fixture.commit_count(&branch);

    let payload = fixture.finalize(false).expect("coord finalize succeeds");
    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    assert_eq!(payload["bootstrap"]["already_initialized"], json!(0));

    // One commit per seeded work package, on the coordination branch.
    assert_eq!(
        fixture.commit_count(&branch),
        before_count + 2,
        "each genesis -> planned seed must be published as its own commit"
    );

    let coord_dir_name = sk_workspace::coord_mission_dir_name(SLUG, MID8);
    let expected_message = format!("chore(spec-kitty): status transition {coord_dir_name}");
    let rows = fixture.commit_rows(&format!("{before_tip}..{branch}"));
    assert_eq!(rows.len(), 2, "two seed commits: {rows:?}");
    for row in &rows {
        let fields: Vec<&str> = row.split('|').collect();
        assert_eq!(fields.len(), 6, "unexpected log row: {row}");
        assert_eq!(fields[1], expected_message, "canonical commit message");
        assert_eq!(fields[2], "Test", "real Git author name");
        assert_eq!(fields[3], "test@example.com", "real Git author email");
        assert_eq!(fields[4], "Test", "real Git committer name");
        assert_eq!(fields[5], "test@example.com", "real Git committer email");
    }
    // A linear chain: the second seed's single parent is the first seed.
    let parents: Vec<Vec<&str>> = rows
        .iter()
        .map(|row| {
            row.split('|')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect()
        })
        .collect();
    assert_eq!(parents[0].len(), 1, "the first seed has one parent");
    assert_eq!(
        parents[0][0], before_tip,
        "the first seed's parent is the old tip"
    );
    assert_eq!(parents[1], vec![fixture.branch_tip(&format!("{branch}~1"))]);

    // The committed log is the real one: both rows, Source's actor and reason.
    let committed = fixture.committed_status_rows();
    assert_eq!(committed.len(), 2, "both seed rows are committed");
    for row in &committed {
        assert_eq!(row["from_lane"], json!("genesis"));
        assert_eq!(row["to_lane"], json!("planned"));
        assert_eq!(row["reason"], json!("canonical bootstrap"));
        assert_eq!(row["actor"]["tool"], json!("finalize-tasks"));
    }
    // The snapshot is committed beside it, not left behind.
    assert!(
        fixture
            .committed_blob(
                &branch,
                &format!(
                    "kitty-specs/{}/status.json",
                    sk_workspace::coord_mission_dir_name(SLUG, MID8)
                )
            )
            .is_some(),
        "the materialized snapshot must be published with the row"
    );
    // Nothing is left untracked in the coordination checkout.
    assert_eq!(
        fixture.coord_status().trim(),
        "",
        "the coordination checkout must be clean after publication"
    );
}

#[test]
fn a_retry_does_not_duplicate_seeds_or_commits() {
    let fixture =
        Fixture::new(sk_mission::MissionTopology::LanesWithCoord.as_str()).coord_materialized();
    fixture.finalize(false).expect("first finalize succeeds");
    let branch = fixture.coord_branch();
    let tip_after_first = fixture.branch_tip(&branch);
    let rows_after_first = fixture.committed_status_rows();

    let payload = fixture.finalize(false).expect("retry succeeds");
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(0));
    assert_eq!(payload["bootstrap"]["already_initialized"], json!(2));
    assert_eq!(
        fixture.branch_tip(&branch),
        tip_after_first,
        "a retry must publish no further commit"
    );
    assert_eq!(
        fixture.committed_status_rows(),
        rows_after_first,
        "a retry must not append duplicate seed rows"
    );
    assert_eq!(fixture.coord_status().trim(), "");
}

#[test]
fn a_primary_partition_seed_commits_nothing() {
    let fixture = Fixture::new(sk_mission::MissionTopology::SingleBranch.as_str());
    let head_before = fixture.branch_tip("main");
    let payload = fixture
        .finalize(false)
        .expect("single_branch finalize succeeds");

    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    let log = fixture.path("status.events.jsonl");
    assert!(log.is_file(), "the primary arm still writes the log");
    assert!(fixture.path("status.json").is_file());
    assert_eq!(
        fixture.branch_tip("main"),
        head_before,
        "the reference's primary arm commits nothing"
    );
    let status = git(&fixture.root, &fixture.home, &["status", "--porcelain"]);
    assert!(
        status.contains("kitty-specs/demo/status.events.jsonl"),
        "the primary status log stays an uncommitted write: {status}"
    );
}

#[test]
fn a_refused_seed_commit_rolls_back_only_that_row_and_reports_the_failure() {
    let fixture = Fixture::new(sk_mission::MissionTopology::Coord.as_str()).coord_materialized();
    // A pre-commit hook that always refuses: the seed row is appended, then the
    // commit cannot land. Source's `_emit_on_coord_then_commit` cuts exactly that
    // row back and propagates the failure.
    let hook = fixture.root.join(".git/hooks/pre-commit");
    write(&hook, "#!/bin/sh\nexit 1\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("hook mode");
    }
    let branch = fixture.coord_branch();
    let tip_before = fixture.branch_tip(&branch);

    let error = fixture
        .finalize(false)
        .expect_err("a refused seed commit must fail the run");
    let document = error.to_json();
    assert_eq!(document["phase"], json!("apply"), "{document}");
    let message = document["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("did not land") || message.contains("was refused before it could land"),
        "the refusal must describe the real commit outcome: {message}"
    );
    assert!(
        message.contains("rolled back"),
        "the rollback outcome rides the error message, as the reference appends it: {message}"
    );
    // No extra envelope keys beyond the route's own contract.
    let keys: Vec<&str> = document
        .as_object()
        .expect("error document object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["error", "phase"], "unexpected keys: {keys:?}");

    // The coordination branch did not move and carries no seed row.
    assert_eq!(fixture.branch_tip(&branch), tip_before);
    assert!(
        fixture.committed_status_rows().is_empty(),
        "no seed row may be left committed"
    );
    // The failed row was cut back out of the working tree as well.
    let log = fixture.coord_path("status.events.jsonl");
    let remaining = if log.is_file() {
        read(&log)
    } else {
        String::new()
    };
    assert!(
        remaining
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count()
            == 0,
        "the refused seed's own row is rolled back: {remaining}"
    );
    // Source's partial-write policy: this run's completed writes stand.
    assert!(fixture.path("lanes.json").is_file(), "lanes.json stays");
    let wp02 = read(&fixture.path("tasks/WP02-beta.md"));
    assert!(
        wp02.contains("WP01"),
        "the dependency write already completed stays in place: {wp02}"
    );
}

#[test]
fn the_dry_run_reads_a_branch_ref_only_coordination_log() {
    let fixture = Fixture::new(sk_mission::MissionTopology::Coord.as_str()).coord_materialized();
    fixture.finalize(false).expect("coord finalize succeeds");
    let branch = fixture.coord_branch();
    assert_eq!(fixture.committed_status_rows().len(), 2);

    // Only the worktree goes away; the branch still holds the committed log.
    let fixture = fixture.without_coordination_worktree();
    let tip_before = fixture.branch_tip(&branch);
    let before = fixture.footprint();

    let payload = fixture
        .finalize(true)
        .expect("the dry run reads the branch-ref log");
    assert_eq!(payload["result"], json!("validation_passed"));
    assert_eq!(
        payload["bootstrap"]["already_initialized"],
        json!(2),
        "the dry run must read the real ids out of the coordination branch: {payload}"
    );
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(0));
    let details = payload["bootstrap"]["wp_details"]
        .as_object()
        .expect("wp_details object");
    assert_eq!(details.get("WP01"), Some(&json!("already_exists")));
    assert_eq!(details.get("WP02"), Some(&json!("already_exists")));
    // Non-vacuity: the PRIMARY Mission directory holds no status log, so a
    // reader that only looked there would necessarily report both WPs as
    // unseeded. The `already_initialized` result can only come from the
    // branch-ref arm.
    assert!(
        !fixture.path("status.events.jsonl").exists(),
        "the fixture must leave the primary checkout with no status log"
    );
    // B1: the work-package scan still reads PRIMARY.
    assert_eq!(payload["bootstrap"]["total_wps"], json!(2));

    // No materialization, no lock, no ref move, no byte written.
    assert!(
        !fixture.coord_worktree().exists(),
        "the dry run must not recreate the coordination worktree"
    );
    let after = fixture.footprint();
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "the dry run must not add or remove a file"
    );
    for (path, bytes) in &before {
        assert_eq!(after.get(path), Some(bytes), "the dry run changed {path}");
    }
    assert_eq!(
        fixture.branch_tip(&branch),
        tip_before,
        "the dry run must not move the coordination branch"
    );
}

#[test]
fn the_cf24_occurrence_map_is_structurally_valid_and_admissible() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/rust/occurrence-maps/cf24-finalize-status-publication.yaml");
    let text = fs::read_to_string(&path).expect("read the CF24 occurrence map");
    let map: Value = serde_yaml_ng::from_str(&text).expect("the map is valid YAML");
    let structural = sk_runtime::validate_occurrence_map(&map);
    assert!(
        structural.errors.is_empty(),
        "structural errors: {:?}",
        structural.errors
    );
    let admissible = sk_runtime::check_occurrence_admissibility(&map);
    assert!(
        admissible.errors.is_empty(),
        "admissibility errors: {:?}",
        admissible.errors
    );
}

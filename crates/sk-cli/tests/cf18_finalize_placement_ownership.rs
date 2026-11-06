//! CF18: Source-correct `finalize-tasks` write placement and ownership
//! validation.
//!
//! These drive `sk_cli::compat::mission::finalize_tasks` — the exact entry the
//! compatibility dispatcher calls — over fresh, independent Git fixtures:
//!
//! * a coordination-routed Mission's status bootstrap writes into the
//!   **coordination** Mission directory (established first), while the WP scan,
//!   the dependency plan and the identity payload keep reading **PRIMARY**;
//! * `--validate-only` establishes nothing: no worktree, no seed, no lock, no
//!   ref, no file;
//! * a literal `owned_files` path that matches no file is refused before
//!   `lanes.json` is written, with Source's `ownership_literal_path_errors`
//!   payload, and `create_intent` suppresses that refusal;
//! * a failed status bootstrap keeps the writes this run already completed
//!   (Source has no implicit rollback on this route) and a retry completes.
//!
//! Native functional contracts only. These are NOT Source isolation/pairing
//! proof: no oracle is executed here, the fixtures are ordinary `mktemp`
//! directories with their own `git init`, and nothing asserts full parity.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::mission::finalize_tasks;
use sk_cli::compat::support::CompatError;

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const MID8: &str = "01KN2371";
const SLUG: &str = "demo";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("read fixture")
}

fn git(dir: &Path, home: &Path, args: &[&str]) {
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
}

fn git_stdout(dir: &Path, home: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const WP01: &str = "---\nwork_package_id: WP01\ntitle: Alpha\nexecution_mode: code_change\nowned_files:\n  - src/alpha.rs\ndependencies: []\n---\n\n# WP01 Alpha\n\nAlpha body.\n";

const WP02: &str = "---\nwork_package_id: WP02\ntitle: Beta\nexecution_mode: code_change\nowned_files:\n  - src/beta.rs\ndependencies: []\n---\n\n# WP02 Beta\n\nBeta body.\n";

const TASKS_MD: &str = "# Tasks\n\n## WP01 - Alpha\n\n- Dependencies: none\n\n## WP02 - Beta\n\n- Dependencies: WP01\n";

/// Source's `lanes.compute_and_persist.LaneGlobValidationError` message.
const LANE_GLOB_ABORT: &str = "Lane computation aborted: literal-path owned_files entries match zero files. Fix the paths before lanes.json is written.";

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

    /// Mint + establish the coordination surface through the canonical owner.
    fn bootstrap_coordination(&self) -> sk_workspace::CoordBootstrapReceipt {
        let policy = sk_workspace::ProtectionPolicy::resolve(&self.root).expect("policy");
        let receipt = sk_workspace::bootstrap_coordination(&self.workspace(), &policy)
            .expect("coordination bootstrap");
        assert!(
            !receipt.is_deferred(),
            "the fixture target branch must resolve: {receipt:?}"
        );
        assert!(
            receipt.branch_created,
            "the fixture expects a freshly minted coordination branch"
        );
        receipt
    }

    /// A coordination-routed Mission whose coordination surface is MATERIALIZED.
    fn coord_materialized(self) -> Self {
        self.bootstrap_coordination();
        // MATERIALIZED means the worktree is registered *and* its Mission
        // directory exists; the seed may carry nothing on a scaffold-only
        // Mission, so create the directory the way any coord-kind write would.
        write(&self.coord_path("notes.md"), "coord notes\n");
        self
    }

    /// A coordination-routed Mission whose worktree is registered with no
    /// Mission directory (the `EMPTY` seed ticket).
    fn coord_empty(self) -> Self {
        self.bootstrap_coordination();
        if self.coord_mission().exists() {
            fs::remove_dir_all(self.coord_mission()).expect("remove coord mission dir");
        }
        assert!(
            !self.coord_mission().exists(),
            "the EMPTY fixture must have no coordination Mission directory"
        );
        self
    }

    /// A coordination-routed Mission whose branch exists but has no worktree.
    fn coord_unmaterialized(self) -> Self {
        self.bootstrap_coordination();
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

    fn finalize(&self, validate_only: bool) -> Result<Value, CompatError> {
        finalize_tasks(&self.root, Some(SLUG), validate_only, true)
    }

    /// Every file under the repository root except Git's own directory, plus
    /// the ref list — the whole footprint a run must not touch when it may not
    /// write.
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
        let refs = git_stdout(
            &self.root,
            &self.home,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        );
        files.insert("#refs".to_owned(), refs.into_bytes());
        let status = git_stdout(&self.root, &self.home, &["rev-parse", "--verify", "HEAD"]);
        files.insert("#head".to_owned(), status.into_bytes());
        files
    }
}

#[test]
fn a_single_branch_mission_writes_status_to_the_primary_directory() {
    let fixture = Fixture::new("single_branch");
    let payload = fixture.finalize(false).expect("ordinary finalize succeeds");

    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    let rows = read(&fixture.path("status.events.jsonl"));
    assert_eq!(
        rows.lines().filter(|line| !line.trim().is_empty()).count(),
        2
    );
    assert!(fixture.path("lanes.json").is_file());
    // No coordination surface is created for a coord-less topology.
    assert!(
        !fixture.root.join(".worktrees").exists(),
        "a single_branch mission must never mint a coordination worktree"
    );
    // Source's payload has no `committed` key at all.
    assert!(
        payload.get("committed").is_none(),
        "the payload must match Source's key set: {payload}"
    );
}

#[test]
fn a_materialized_coordination_mission_writes_status_into_the_coordination_surface() {
    let fixture = Fixture::new("coord").coord_materialized();
    let payload = fixture.finalize(false).expect("coord finalize succeeds");

    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    assert_eq!(payload["updated_wp_count"], json!(1));

    // PRIMARY scan + identity stayed primary: the payload names the Mission.
    assert_eq!(payload["mission_slug"], json!(SLUG));
    assert_eq!(payload["updated_wp_count"], json!(1));

    // The prompts and `lanes.json` are PRIMARY-partition artifacts.
    assert!(fixture.path("lanes.json").is_file());
    let updated = read(&fixture.path("tasks/WP02-beta.md"));
    let wp02 = sk_mission::WorkPackage::parse(&fixture.path("tasks/WP02-beta.md"), &updated)
        .expect("the rewritten prompt is still a canonical work package");
    assert_eq!(
        wp02.dependencies()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01"],
        "the prompt was rewritten in the primary partition: {updated}"
    );

    // The status log and snapshot landed in the COORDINATION Mission directory.
    let coord_log = fixture.coord_path("status.events.jsonl");
    assert!(
        coord_log.is_file(),
        "the status bootstrap must write into {}",
        coord_log.display()
    );
    let rows: Vec<Value> = read(&coord_log)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("canonical event row"))
        .collect();
    assert_eq!(rows.len(), 2, "one genesis -> planned event per WP");
    for row in &rows {
        assert_eq!(row["from_lane"], json!("genesis"));
        assert_eq!(row["to_lane"], json!("planned"));
        assert_eq!(row["reason"], json!("canonical bootstrap"));
        assert_eq!(row["actor"]["tool"], json!("finalize-tasks"));
    }
    assert!(
        fixture.coord_path("status.json").is_file(),
        "the snapshot is materialized beside the coordination log"
    );

    // Source B1: the PRIMARY Mission directory never receives the status log.
    assert!(
        !fixture.path("status.events.jsonl").exists(),
        "a coordination-routed mission must not bootstrap into the primary checkout"
    );
    assert!(!fixture.path("status.json").exists());
    // The primary scan still saw both work packages (B1 regression).
    assert_eq!(payload["bootstrap"]["total_wps"], json!(2));
}

#[test]
fn a_never_seeded_coordination_surface_is_established_before_the_bootstrap() {
    let fixture = Fixture::new("lanes_with_coord").coord_empty();
    let payload = fixture
        .finalize(false)
        .expect("finalize establishes the surface");

    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    let coord_log = fixture.coord_path("status.events.jsonl");
    assert!(
        coord_log.is_file(),
        "an EMPTY surface must be established (materialized + seeded) before the write"
    );
    assert!(
        !fixture.path("status.events.jsonl").exists(),
        "the establish leg must never fall back to the primary checkout"
    );
}

#[test]
fn an_unmaterialized_coordination_branch_is_materialized_for_the_status_write() {
    let fixture = Fixture::new("coord").coord_unmaterialized();
    let payload = fixture
        .finalize(false)
        .expect("finalize materializes the worktree");

    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    assert!(
        fixture.coord_worktree().is_dir(),
        "the coordination worktree is materialized at {}",
        fixture.coord_worktree().display()
    );
    assert!(fixture.coord_path("status.events.jsonl").is_file());
    assert!(!fixture.path("status.events.jsonl").exists());
}

#[test]
fn validate_only_never_materializes_seeds_or_locks_anything() {
    let fixture = Fixture::new("coord").coord_unmaterialized();
    let before = fixture.footprint();

    let payload = fixture.finalize(true).expect("the dry run succeeds");

    assert_eq!(payload["result"], json!("validation_passed"));
    assert_eq!(payload["validate_only"], json!(true));
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    assert_eq!(
        fixture.footprint(),
        before,
        "Source NFR-002: --validate-only must not materialize, seed, lock, create a ref, or write a byte"
    );
}

#[test]
fn a_literal_owned_files_path_matching_nothing_is_refused_before_lanes_are_written() {
    let fixture = Fixture::new("single_branch");
    write(
        &fixture.path("tasks/WP02-beta.md"),
        "---\nwork_package_id: WP02\ntitle: Beta\nexecution_mode: code_change\nowned_files:\n  - src/missing_module.rs\ndependencies: []\n---\n\n# WP02 Beta\n\nBeta body.\n",
    );

    let error = fixture
        .finalize(false)
        .expect_err("Source refuses a zero-match literal path");
    let payload = error.to_json();
    assert_eq!(error.exit(), 1);
    assert_eq!(payload["error"], json!(LANE_GLOB_ABORT));
    assert_eq!(payload["phase"], json!("apply"));
    let errors = payload["ownership_literal_path_errors"]
        .as_array()
        .expect("Source's ownership_literal_path_errors payload");
    assert_eq!(errors.len(), 1, "one refusal row: {errors:?}");
    let row = errors[0].as_str().expect("the row is a string");
    assert!(
        row.starts_with("WP02: owned_files path 'src/missing_module.rs'"),
        "{row}"
    );
    assert!(
        row.contains("is a literal file path that matches zero files"),
        "{row}"
    );
    assert!(
        row.contains("create_intent"),
        "the remedy is Source's: {row}"
    );

    // Source writes the prompts first, then refuses before lanes/status.
    let updated = read(&fixture.path("tasks/WP02-beta.md"));
    assert!(
        updated.contains("WP01"),
        "the completed prompt write stands (Source has no implicit rollback): {updated}"
    );
    assert!(
        !fixture.path("lanes.json").exists(),
        "no lanes.json is written"
    );
    assert!(
        !fixture.path("status.events.jsonl").exists(),
        "no events are seeded"
    );
    assert!(
        !fixture.path("status.json").exists(),
        "no snapshot is materialized"
    );
}

#[test]
fn a_create_intent_entry_suppresses_the_zero_match_refusal() {
    let fixture = Fixture::new("single_branch");
    write(
        &fixture.path("tasks/WP01-alpha.md"),
        "---\nwork_package_id: WP01\ntitle: Alpha\nexecution_mode: code_change\nowned_files:\n  - src/alpha.rs\n  - src/brand_new.rs\ndependencies: []\ncreate_intent:\n  - src/brand_new.rs\n---\n\n# WP01 Alpha\n\nAlpha body.\n",
    );

    let payload = fixture
        .finalize(false)
        .expect("a planned-new-file literal path is not a refusal");
    assert_eq!(payload["result"], json!("success"));
    assert!(fixture.path("lanes.json").is_file());
    assert!(fixture.path("status.events.jsonl").is_file());
}

#[test]
fn a_failed_status_bootstrap_keeps_completed_writes_and_a_retry_completes() {
    let fixture = Fixture::new("coord").coord_materialized();
    // A malformed coordination log makes the status leg fail AFTER the prompt
    // write and the lanes write have already published.
    write(
        &fixture.coord_path("status.events.jsonl"),
        "{not a canonical row}\n",
    );

    let error = fixture
        .finalize(false)
        .expect_err("the corrupted log refuses the run");
    let payload = error.to_json();
    assert_eq!(payload["phase"], json!("apply"));
    assert_eq!(error.exit(), 1);
    assert!(
        payload.get("rolled_back_writes").is_none(),
        "Source has no implicit rollback on this route: {payload}"
    );

    // Source _do_finalize_tasks reports the failure without undoing writes.
    let updated = read(&fixture.path("tasks/WP02-beta.md"));
    let wp = sk_mission::WorkPackage::parse(&fixture.path("tasks/WP02-beta.md"), &updated)
        .expect("the completed prompt write is still valid");
    assert_eq!(
        wp.dependencies()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01"]
    );
    assert!(
        fixture.path("lanes.json").is_file(),
        "the lanes write stands"
    );
    assert_eq!(
        read(&fixture.coord_path("status.events.jsonl")),
        "{not a canonical row}\n",
        "the corrupted log is left exactly as found, never rewritten"
    );

    // Fixing the surface and re-running completes the bootstrap.
    fs::remove_file(fixture.coord_path("status.events.jsonl")).expect("remove corrupt log");
    let retry = fixture.finalize(false).expect("retry succeeds");
    assert_eq!(retry["result"], json!("success"));
    assert_eq!(retry["bootstrap"]["newly_seeded"], json!(2));
    assert_eq!(
        read(&fixture.coord_path("status.events.jsonl"))
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        2
    );
}

#[test]
fn a_rerun_after_establishment_is_idempotent() {
    let fixture = Fixture::new("coord").coord_materialized();
    let first = fixture.finalize(false).expect("first finalize");
    assert_eq!(first["bootstrap"]["newly_seeded"], json!(2));
    let coord_log = read(&fixture.coord_path("status.events.jsonl"));
    let lanes = read(&fixture.path("lanes.json"));

    let second = fixture.finalize(false).expect("second finalize");
    assert_eq!(second["bootstrap"]["newly_seeded"], json!(0));
    assert_eq!(second["bootstrap"]["already_initialized"], json!(2));
    assert_eq!(read(&fixture.coord_path("status.events.jsonl")), coord_log);
    assert_eq!(read(&fixture.path("lanes.json")), lanes);
}

/// The pre-edit occurrence classification this lane was developed under must be
/// structurally valid and admissible, or the guardrail is decorative.
#[test]
fn the_cf18_occurrence_map_is_structurally_valid_and_admissible() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/rust/occurrence-maps/cf18-finalize-placement.yaml");
    let text = read(&path);
    let map: Value = serde_yaml_ng::from_str(&text).expect("the map parses as YAML");
    let structural = sk_runtime::validate_occurrence_map(&map);
    assert!(
        structural.is_valid(),
        "structural errors: {:?}",
        structural.errors
    );
    let admissible = sk_runtime::check_occurrence_admissibility(&map);
    assert!(
        admissible.is_valid(),
        "admissibility errors: {:?}",
        admissible.errors
    );
}

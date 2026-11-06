//! CF15: the real `finalize-tasks` orchestration over the delivered owners.
//!
//! These drive `sk_cli::compat::mission::finalize_tasks` — the exact entry the
//! compatibility dispatcher calls — so the exercised path is the composed
//! pipeline (resolve → occurrence gate → dependency plan → gated prompt writes →
//! `lanes.json` planning write → canonical status bootstrap → payload), not a
//! plan unit test.
//!
//! Native functional contracts only. These are NOT Source isolation/pairing
//! proof: no oracle is executed here, the fixtures are ordinary `mktemp`
//! directories with their own `git init`, and nothing asserts full parity.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::mission::finalize_tasks;
use sk_cli::compat::support::CompatError;

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
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

const WP01: &str = "---\nwork_package_id: WP01\ntitle: Alpha\nexecution_mode: code_change\nowned_files:\n  - src/alpha.rs\ndependencies: []\n---\n\n# WP01 Alpha\n\nAlpha body.\n";

const WP02: &str = "---\nwork_package_id: WP02\ntitle: Beta\nexecution_mode: code_change\nowned_files:\n  - src/beta.rs\ndependencies: []\n---\n\n# WP02 Beta\n\nBeta body.\n";

const TASKS_MD: &str = "# Tasks\n\n## WP01 - Alpha\n\n- Dependencies: none\n\n## WP02 - Beta\n\n- Dependencies: WP01\n";

/// A fresh, independent Mission repository.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(change_mode: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).expect("repo");
        fs::create_dir_all(&home).expect("home");
        git(&root, &home, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        // CF18: the work packages own `src/alpha.rs` and `src/beta.rs`. Source
        // re-validates `owned_files` against the repository before `lanes.json`
        // is written, so a fixture whose owned paths do not exist is not a
        // Source-valid mission and would (correctly) be refused.
        write(&root.join("src/alpha.rs"), "// alpha\n");
        write(&root.join("src/beta.rs"), "// beta\n");

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
            "topology": "single_branch",
        });
        if let Some(change_mode) = change_mode {
            meta["change_mode"] = json!(change_mode);
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
        Self { _temp: temp, root }
    }

    fn mission(&self) -> PathBuf {
        self.root.join("kitty-specs").join(SLUG)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.mission().join(relative)
    }

    fn read(&self, relative: &str) -> String {
        read(&self.path(relative))
    }

    fn finalize(&self, validate_only: bool) -> Result<Value, CompatError> {
        finalize_tasks(&self.root, Some(SLUG), validate_only, true)
    }
}

#[test]
fn the_ordinary_route_writes_dependencies_planning_lanes_and_canonical_status() {
    let fixture = Fixture::new(None);
    let payload = fixture.finalize(false).expect("ordinary finalize succeeds");

    assert_eq!(payload["result"], json!("success"));
    assert_eq!(payload["updated_wp_count"], json!(1), "only WP02 changes");
    assert_eq!(payload["modified_wps"], json!(["WP02"]));
    assert_eq!(payload["unchanged_wps"], json!(["WP01"]));
    assert_eq!(payload["preserved_wps"], json!([]));
    assert_eq!(payload["dependencies"]["WP02"], json!(["WP01"]));
    assert_eq!(payload["dependencies"]["WP01"], json!([]));
    assert_eq!(payload["mission_slug"], json!(SLUG));
    assert_eq!(payload["mission_number"], json!(7));
    assert_eq!(payload["mission_type"], json!("software-dev"));
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    assert_eq!(payload["bootstrap"]["total_wps"], json!(2));
    assert_eq!(payload["bootstrap"]["skipped"], json!(0));
    assert_eq!(
        payload["bootstrap"]["wp_details"]["WP01"],
        json!("initialized")
    );

    // Authored prompt content is preserved; only the dependency list changed.
    let wp02_text = fixture.read("tasks/WP02-beta.md");
    let wp02 = sk_mission::WorkPackage::parse(&fixture.path("tasks/WP02-beta.md"), &wp02_text)
        .expect("the rewritten prompt is still a canonical work package");
    assert_eq!(
        wp02.dependencies()
            .iter()
            .map(|dep| dep.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01"]
    );
    assert_eq!(wp02.title(), Some("Beta"), "title preserved: {wp02_text}");
    assert_eq!(
        wp02.frontmatter_field("owned_files"),
        Some(&json!(["src/beta.rs"])),
        "owned files preserved: {wp02_text}"
    );
    assert_eq!(
        wp02.frontmatter_field("execution_mode"),
        Some(&json!("code_change"))
    );
    assert!(
        wp02_text.contains("# WP02 Beta") && wp02_text.contains("Beta body."),
        "body preserved: {wp02_text}"
    );
    assert_eq!(
        fixture.read("tasks/WP01-alpha.md"),
        WP01,
        "an unchanged prompt is byte-identical"
    );

    // The planning lanes were really computed and written.
    let lanes: Value =
        serde_json::from_str(&fixture.read("lanes.json")).expect("lanes.json is canonical JSON");
    assert_eq!(lanes["version"], json!(1));
    assert_eq!(lanes["mission_slug"], json!(SLUG));
    assert_eq!(lanes["mission_id"], json!(MISSION_ID));
    assert_eq!(lanes["target_branch"], json!("main"));
    assert!(
        lanes["planning_commit_sha"].is_null() || lanes["planning_commit_sha"].is_string(),
        "the planning sha is captured, never invented: {}",
        lanes["planning_commit_sha"]
    );

    // The canonical status log was bootstrapped through the real writer.
    let events = fixture.read("status.events.jsonl");
    let rows: Vec<Value> = events
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("canonical event row"))
        .collect();
    assert_eq!(rows.len(), 2, "one genesis -> planned event per WP");
    for row in &rows {
        assert_eq!(row["from_lane"], json!("genesis"));
        assert_eq!(row["to_lane"], json!("planned"));
        assert_eq!(row["reason"], json!("canonical bootstrap"));
        assert_eq!(row["actor"]["role"], json!("system"));
        assert_eq!(row["actor"]["tool"], json!("finalize-tasks"));
        assert_eq!(row["execution_mode"], json!("worktree"));
    }
    assert!(
        fixture.path("status.json").is_file(),
        "the canonical snapshot is materialized"
    );
}

#[test]
fn an_invalid_occurrence_map_blocks_before_any_write() {
    let fixture = Fixture::new(Some("bulk_edit"));
    write(
        &fixture.path("occurrence_map.yaml"),
        "target:\n  term: Old\ncategories: {}\n",
    );
    let error = fixture
        .finalize(false)
        .expect_err("the bulk-edit gate refuses");
    let payload = error.to_json();
    assert_eq!(error.exit(), 1);
    assert_eq!(
        payload["error"],
        json!("Bulk edit occurrence-map gate blocked finalize-tasks.")
    );
    assert_eq!(payload["phase"], json!("validate"));
    let gate_errors = payload["gate_errors"].as_array().expect("gate errors");
    assert!(!gate_errors.is_empty(), "the refusal names what is wrong");

    // Refused BEFORE the first byte: nothing was written anywhere.
    assert_eq!(fixture.read("tasks/WP02-beta.md"), WP02);
    assert!(!fixture.path("lanes.json").exists(), "no lanes written");
    assert!(
        !fixture.path("status.events.jsonl").exists(),
        "no status events seeded"
    );
}

#[test]
fn a_missing_occurrence_map_is_reported_with_the_reference_message() {
    let fixture = Fixture::new(Some("bulk_edit"));
    let error = fixture
        .finalize(false)
        .expect_err("a bulk_edit mission needs the map");
    let payload = error.to_json();
    assert_eq!(
        payload["error"],
        json!("Bulk edit occurrence-map gate blocked finalize-tasks.")
    );
    assert!(
        payload["gate_errors"]
            .as_array()
            .expect("gate errors")
            .iter()
            .any(|message| message
                .as_str()
                .unwrap_or_default()
                .contains("Occurrence map required for bulk_edit missions")),
        "names the missing artifact: {}",
        payload["gate_errors"]
    );
    assert_eq!(fixture.read("tasks/WP02-beta.md"), WP02);
}

#[test]
fn validate_only_reads_without_writing_anything() {
    let fixture = Fixture::new(None);
    let payload = fixture
        .finalize(true)
        .expect("the dry run reports what it would do");

    assert_eq!(payload["result"], json!("validation_passed"));
    assert_eq!(payload["validate_only"], json!(true));
    assert_eq!(payload["updated_wp_count"], json!(1));
    assert_eq!(payload["would_modify"][0]["wp_id"], json!("WP02"));
    assert_eq!(
        payload["would_modify"][0]["changes"]["dependencies"],
        json!(["WP01"])
    );
    assert_eq!(payload["would_preserve"], json!([]));
    assert_eq!(payload["unchanged"], json!(["WP01"]));
    assert_eq!(payload["bootstrap"]["newly_seeded"], json!(2));
    assert_eq!(
        payload["bootstrap"]["wp_details"]["WP02"],
        json!("would_seed")
    );

    // The dry run mutated nothing at all.
    assert_eq!(fixture.read("tasks/WP02-beta.md"), WP02);
    assert!(!fixture.path("lanes.json").exists());
    assert!(!fixture.path("status.events.jsonl").exists());
    assert!(!fixture.path("status.json").exists());
}

#[test]
fn a_failed_status_bootstrap_keeps_source_completed_writes_and_foreign_bytes() {
    let fixture = Fixture::new(None);
    // A malformed canonical log makes the status leg fail AFTER the prompt write
    // and the lanes write have already published.
    write(
        &fixture.path("status.events.jsonl"),
        "{not a canonical row}\n",
    );
    // A foreign file this run never touches.
    write(&fixture.path("notes.md"), "operator notes\n");

    let error = fixture
        .finalize(false)
        .expect_err("the corrupted canonical log refuses the run");
    let payload = error.to_json();
    assert_eq!(payload["phase"], json!("apply"));
    assert_eq!(error.exit(), 1);
    assert!(
        payload.get("rolled_back_writes").is_none(),
        "no invented rollback: {payload}"
    );
    assert!(payload.get("restore_error").is_none());

    // Source _do_finalize_tasks reports the failure without undoing writes.
    let updated = fixture.read("tasks/WP02-beta.md");
    let wp = sk_mission::WorkPackage::parse(&fixture.path("tasks/WP02-beta.md"), &updated)
        .expect("the completed prompt write is still valid");
    assert_eq!(
        wp.dependencies()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01"]
    );
    assert!(updated.contains("Beta body."), "authored body preserved");
    let lanes: Value =
        serde_json::from_str(&fixture.read("lanes.json")).expect("completed lanes write");
    assert_eq!(lanes["mission_id"], json!(MISSION_ID));
    assert!(
        !fixture.path("status.json").exists(),
        "failed status was not fabricated"
    );
    // The pre-existing foreign artifacts are untouched.
    assert_eq!(
        fixture.read("status.events.jsonl"),
        "{not a canonical row}\n",
        "the corrupted log is left exactly as found, never rewritten"
    );
    assert_eq!(fixture.read("notes.md"), "operator notes\n");
}

#[test]
fn a_rerun_is_idempotent() {
    let fixture = Fixture::new(None);
    let first = fixture.finalize(false).expect("first finalize");
    assert_eq!(first["updated_wp_count"], json!(1));
    let lanes_after_first = fixture.read("lanes.json");
    let events_after_first = fixture.read("status.events.jsonl");

    let second = fixture.finalize(false).expect("second finalize");
    assert_eq!(second["result"], json!("success"));
    assert_eq!(second["updated_wp_count"], json!(0));
    assert_eq!(second["modified_wps"], json!([]));
    assert_eq!(second["unchanged_wps"], json!(["WP01", "WP02"]));
    assert_eq!(second["bootstrap"]["newly_seeded"], json!(0));
    assert_eq!(second["bootstrap"]["already_initialized"], json!(2));

    assert_eq!(
        fixture.read("lanes.json"),
        lanes_after_first,
        "existing lanes are never rewritten"
    );
    assert_eq!(
        fixture.read("status.events.jsonl"),
        events_after_first,
        "no WP is seeded twice"
    );
    assert!(!fixture.read("tasks/WP02-beta.md").is_empty());
}

#[test]
fn a_cyclic_dependency_is_refused_before_any_write() {
    let fixture = Fixture::new(None);
    write(
        &fixture.path("tasks.md"),
        "# Tasks\n\n## WP01 - Alpha\n\n- Dependencies: WP02\n\n## WP02 - Beta\n\n- Dependencies: WP01\n",
    );
    let error = fixture.finalize(false).expect_err("a cycle is refused");
    let payload = error.to_json();
    assert_eq!(payload["phase"], json!("validate"));
    assert!(
        payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cycle"),
        "names the cycle: {payload}"
    );
    assert_eq!(fixture.read("tasks/WP02-beta.md"), WP02);
    assert_eq!(fixture.read("tasks/WP01-alpha.md"), WP01);
    assert!(!fixture.path("lanes.json").exists());
}

/// The pre-edit occurrence classification this lane was developed under must be
/// structurally valid and admissible, or the guardrail is decorative.
#[test]
fn the_cf15_occurrence_map_is_structurally_valid_and_admissible() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/rust/occurrence-maps/cf15-finalize-tasks.yaml");
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

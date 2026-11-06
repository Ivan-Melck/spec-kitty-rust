//! Native merge-driver migration family (seven declared ids).
//!
//! Each case builds a throwaway Git project, applies only that migration, and
//! checks the exact tracked `.gitattributes` line plus the clone-local
//! `merge.<key>.name`/`driver` values. No repository fixture is reused.

use std::path::Path;
use std::process::Command;

use sk_install::migrations::merge_drivers::{self, DriverSpec};

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn repo() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp");
    git(directory.path(), &["init", "-q", "-b", "main"]);
    git(directory.path(), &["config", "user.email", "t@example.com"]);
    git(directory.path(), &["config", "user.name", "Test"]);
    directory
}

fn expected(
    id: &str,
) -> (
    &'static [&'static str],
    &'static str,
    &'static str,
    &'static str,
) {
    match id {
        "3.1.1_event_log_merge_driver" => (
            &["kitty-specs/**/status.events.jsonl merge=spec-kitty-event-log"],
            "spec-kitty-event-log",
            "Spec Kitty event log union merge",
            "spec-kitty merge-driver-event-log %O %A %B",
        ),
        "3.2.6_decisions_event_log_merge_driver" => (
            &["kitty-specs/**/decisions.events.jsonl merge=spec-kitty-event-log"],
            "spec-kitty-event-log",
            "Spec Kitty event log union merge",
            "spec-kitty merge-driver-event-log %O %A %B",
        ),
        "3.2.6_gate_artifact_merge_drivers" => (
            &[
                "kitty-specs/**/acceptance-matrix.json merge=spec-kitty-acceptance-matrix",
                "kitty-specs/**/issue-matrix.md merge=spec-kitty-issue-matrix",
            ],
            "spec-kitty-issue-matrix",
            "Spec Kitty issue matrix filled-side merge",
            "spec-kitty merge-driver-issue-matrix %O %A %B",
        ),
        "3.2.6_issue_matrix_driver_repoint" => (
            &["kitty-specs/**/issue-matrix.json merge=spec-kitty-issue-matrix"],
            "spec-kitty-issue-matrix",
            "Spec Kitty issue matrix row-aware merge",
            "spec-kitty merge-driver-issue-matrix %O %A %B",
        ),
        "3.2.6_meta_traces_merge_drivers" => (
            &[
                "kitty-specs/**/meta.json merge=spec-kitty-meta",
                "kitty-specs/**/traces/*.md merge=spec-kitty-traces",
            ],
            "spec-kitty-meta",
            "Spec Kitty mission meta field merge",
            "spec-kitty merge-driver-meta %O %A %B",
        ),
        "3.2.7_review_cycle_merge_driver" => (
            &["kitty-specs/**/tasks/*/review-cycle-*.md merge=spec-kitty-review-cycle"],
            "spec-kitty-review-cycle",
            "Spec Kitty review-cycle verdict collision refusal",
            "spec-kitty merge-driver-review-cycle %O %A %B",
        ),
        "4.0.0rc5_decision_index_merge_driver" => (
            &["kitty-specs/**/decisions/index.json merge=spec-kitty-decision-index"],
            "spec-kitty-decision-index",
            "Spec Kitty decision-index entry union merge",
            "spec-kitty merge-driver-decision-index %O %A %B",
        ),
        other => panic!("unknown driver id {other}"),
    }
}

const IDS: [&str; 7] = [
    "3.1.1_event_log_merge_driver",
    "3.2.6_decisions_event_log_merge_driver",
    "3.2.6_gate_artifact_merge_drivers",
    "3.2.6_issue_matrix_driver_repoint",
    "3.2.6_meta_traces_merge_drivers",
    "3.2.7_review_cycle_merge_driver",
    "4.0.0rc5_decision_index_merge_driver",
];

#[test]
fn every_declared_driver_migration_installs_both_halves_and_is_idempotent() {
    for id in IDS {
        let directory = repo();
        let root = directory.path();
        std::fs::write(root.join(".gitattributes"), "*.png binary\n").unwrap();
        let (entries, key, name, command) = expected(id);

        assert!(
            merge_drivers::detect_for(id, root).expect("detect"),
            "{id} should be pending"
        );
        let outcome = merge_drivers::apply_for(id, root).expect("apply");
        assert!(outcome.changed, "{id} should change something");
        assert!(!outcome.changes.is_empty(), "{id}");

        let attributes = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
        assert!(
            attributes.starts_with("*.png binary\n"),
            "{id} preserves content"
        );
        for entry in entries {
            assert!(attributes.contains(entry), "{id}: {entry}");
        }
        assert_eq!(
            git(
                root,
                &["config", "--local", "--get", &format!("merge.{key}.name")]
            ),
            name,
            "{id}"
        );
        assert_eq!(
            git(
                root,
                &["config", "--local", "--get", &format!("merge.{key}.driver")]
            ),
            command,
            "{id}"
        );
        assert!(
            !merge_drivers::detect_for(id, root).expect("detect after"),
            "{id} should be settled"
        );
        let again = merge_drivers::apply_for(id, root).expect("second apply");
        assert!(again.changes.is_empty(), "{id} should be idempotent");
    }
}

#[test]
fn a_non_git_project_gets_the_tracked_half_and_the_exact_warning() {
    let directory = tempfile::tempdir().expect("temp");
    let outcome =
        merge_drivers::apply_for("3.1.1_event_log_merge_driver", directory.path()).expect("apply");
    assert!(outcome.changed);
    assert!(directory.path().join(".gitattributes").is_file());
    assert_eq!(
        outcome.warnings,
        vec![
            "Skipped local git merge-driver config because this project is not a git repository"
                .to_string()
        ]
    );
    assert!(!directory.path().join(".git").exists());
}

#[test]
fn the_frozen_specs_match_the_declared_driver_identity() {
    let specs: &[DriverSpec] =
        merge_drivers::drivers_for("3.2.6_gate_artifact_merge_drivers").expect("gate specs");
    assert_eq!(specs.len(), 2);
    assert_eq!(
        specs[1].attributes_entry(),
        "kitty-specs/**/issue-matrix.md merge=spec-kitty-issue-matrix"
    );
    assert_eq!(
        merge_drivers::dry_run_summary("3.2.6_issue_matrix_driver_repoint"),
        Some("Would repoint the issue-matrix merge driver to issue-matrix.json")
    );
    assert!(merge_drivers::drivers_for("2.0.0_charter_directory").is_none());
    assert!(merge_drivers::apply_for("2.0.0_charter_directory", Path::new(".")).is_err());
}

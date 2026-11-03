//! Live protected-branch resolution against real repositories.
//!
//! The differential expectations encoded here were checked against the pinned
//! Python oracle (`ProtectionPolicy.resolve(...).is_protected(...)`) on the same
//! temporary repositories; see `COORDINATION-REPORT.md`.

mod support;

use sk_workspace::{ProtectionPolicy, WorkspaceError};

use support::{git, init_repo, mission_id, write, write_mission_with};

fn write_config(root: &std::path::Path, body: &str) {
    write(&root.join(".kittify").join("config.yaml"), body);
}

#[test]
fn absent_config_protects_the_name_defaults() {
    let fixture = init_repo();
    let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap();
    assert!(policy.is_protected("main"));
    assert!(policy.is_protected("master"));
    assert!(!policy.is_protected("release"));
    assert!(!policy.operator_hatch_active());
}

#[test]
fn the_remote_default_branch_augments_but_never_replaces_the_defaults() {
    let fixture = init_repo();
    write(
        &fixture.root().join(".git/refs/remotes/origin/HEAD"),
        "ref: refs/remotes/origin/trunk\n",
    );
    let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap();
    assert!(policy.is_protected("trunk"));
    assert!(policy.is_protected("main"));
}

#[test]
fn an_explicit_list_replaces_the_defaults_exactly() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "protection:\n  protected_branches:\n    - release\n",
    );
    let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap();
    assert!(policy.is_protected("release"));
    assert!(
        !policy.is_protected("main"),
        "an explicit list replaces the defaults"
    );
    assert!(!policy.is_protected("master"));
}

#[test]
fn an_empty_list_unprotects_everything() {
    let fixture = init_repo();
    write_config(fixture.root(), "protection:\n  protected_branches: []\n");
    let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap();
    assert!(policy.protected_branches().is_empty());
    assert!(!policy.is_protected("main"));
    let set = policy.branch_set().unwrap();
    assert!(set.is_empty());
}

#[test]
fn a_malformed_list_is_a_typed_refusal_not_a_default() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: release\n",
    );
    let error = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap_err();
    assert!(
        matches!(error, WorkspaceError::ProtectionConfig { .. }),
        "{error}"
    );
}

#[test]
fn a_non_mapping_document_is_a_typed_refusal() {
    let fixture = init_repo();
    write_config(fixture.root(), "- main\n");
    let error = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap_err();
    assert!(
        matches!(error, WorkspaceError::ProtectionConfig { .. }),
        "{error}"
    );
}

#[test]
fn the_operator_hatch_disables_every_protection() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: [release]\n",
    );
    for hatch in ["1", "true", "YES"] {
        let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), Some(hatch)).unwrap();
        assert!(policy.operator_hatch_active(), "{hatch}");
        assert!(!policy.is_protected("release"));
        assert!(policy.branch_set().unwrap().is_empty());
    }
    let off = ProtectionPolicy::resolve_with_hatch(fixture.root(), Some("off")).unwrap();
    assert!(off.is_protected("release"));
}

#[test]
fn commit_to_target_unprotects_only_the_missions_own_target() {
    let fixture = init_repo();
    let id = mission_id(1);
    write_mission_with(
        fixture.root(),
        "demo",
        &id,
        "single_branch",
        "main",
        serde_json::json!({"commit_to_target": true, "mission_branch": "kitty/mission-demo"}),
    );
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: [main, release]\n",
    );
    let policy = ProtectionPolicy::resolve_for_mission(fixture.root(), "demo").unwrap();
    assert!(
        !policy.is_protected("main"),
        "the mission's own target is opted out"
    );
    assert!(
        policy.is_protected("release"),
        "other branches stay protected"
    );
    assert_eq!(policy.mission_bypass_branch(), Some("main"));
    let set = policy.branch_set().unwrap();
    assert!(!set.contains(&sk_git::BranchName::parse("main").unwrap()));
    assert!(set.contains(&sk_git::BranchName::parse("release").unwrap()));
}

#[test]
fn commit_to_target_on_a_non_single_branch_topology_grants_no_bypass() {
    let fixture = init_repo();
    let id = mission_id(2);
    write_mission_with(
        fixture.root(),
        "demo",
        &id,
        "lanes",
        "main",
        serde_json::json!({"commit_to_target": true}),
    );
    let policy = ProtectionPolicy::resolve_for_mission(fixture.root(), "demo").unwrap();
    assert!(policy.is_protected("main"));
    assert_eq!(policy.mission_bypass_branch(), None);
}

#[test]
fn a_non_boolean_commit_to_target_grants_no_bypass() {
    let fixture = init_repo();
    let id = mission_id(3);
    write_mission_with(
        fixture.root(),
        "demo",
        &id,
        "single_branch",
        "main",
        serde_json::json!({"commit_to_target": "yes"}),
    );
    let policy = ProtectionPolicy::resolve_for_mission(fixture.root(), "demo").unwrap();
    assert!(policy.is_protected("main"));
}

#[test]
fn an_unknown_mission_keeps_the_plain_policy() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: [main]\n",
    );
    let policy = ProtectionPolicy::resolve_for_mission(fixture.root(), "nope").unwrap();
    assert!(policy.is_protected("main"));
}

#[test]
fn is_protected_target_counts_the_primary_branch_even_under_an_explicit_list() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: [release]\n",
    );
    let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap();
    assert!(policy.is_protected_target("trunk", "trunk"));
    assert!(policy.is_protected_target("release", "trunk"));
    assert!(!policy.is_protected_target("main", "trunk"));
}

#[test]
fn config_changes_are_read_live() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: [release]\n",
    );
    assert!(
        ProtectionPolicy::resolve_with_hatch(fixture.root(), None)
            .unwrap()
            .is_protected("release")
    );
    write_config(fixture.root(), "protection:\n  protected_branches: []\n");
    assert!(
        !ProtectionPolicy::resolve_with_hatch(fixture.root(), None)
            .unwrap()
            .is_protected("release")
    );
}

#[test]
fn git_style_config_comments_do_not_change_the_answer() {
    let fixture = init_repo();
    write_config(
        fixture.root(),
        "# protection policy\nprotection:\n  # the released line\n  protected_branches:\n    - release  # shipping\n",
    );
    let policy = ProtectionPolicy::resolve_with_hatch(fixture.root(), None).unwrap();
    assert!(policy.is_protected("release"));
}

#[test]
fn a_hatch_value_is_matched_case_insensitively_but_not_trimmed() {
    let fixture = init_repo();
    git(fixture.root(), &["status"]);
    write_config(
        fixture.root(),
        "protection:\n  protected_branches: [release]\n",
    );
    assert!(
        !ProtectionPolicy::resolve_with_hatch(fixture.root(), Some(" true "))
            .unwrap()
            .operator_hatch_active()
    );
}

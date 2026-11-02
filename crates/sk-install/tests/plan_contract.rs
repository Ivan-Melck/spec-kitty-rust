//! Plan → validate → commit contract: refusals before writes, preconditions,
//! and partial-manifest recovery.

mod support;

use std::time::Duration;

use sk_install::RenderFamily;
use sk_install::plan::{InstallPlan, Observed, PlanOptions, PlannedAction};
use sk_install::render::RenderedAsset;

fn asset(path: &str, bytes: &[u8], agent: &'static str) -> RenderedAsset {
    RenderedAsset {
        path: path.to_string(),
        bytes: bytes.to_vec(),
        family: RenderFamily::Markdown,
        command: "plan".to_string(),
        agent,
        source_hash: None,
        source_template: None,
    }
}

fn options() -> PlanOptions {
    PlanOptions {
        now: Some("2026-11-02T10:00:00.000000+00:00".to_string()),
        ..PlanOptions::new()
    }
}

#[test]
fn a_fresh_plan_creates_and_records_ownership() {
    let directory = tempfile::tempdir().expect("temp dir");
    let plan = InstallPlan::plan(
        directory.path(),
        "4.0.0rc7",
        &[asset(
            ".agents/skills/spec-kitty-plan/SKILL.md",
            b"body\n",
            "codex",
        )],
        &options(),
    )
    .expect("plan");

    assert_eq!(plan.summary().created, 1);
    assert!(plan.writes_manifest());
    assert!(!plan.is_noop());
    assert_eq!(plan.assets()[0].action, PlannedAction::Create);
    plan.validate().expect("valid");

    let report = plan.commit(Duration::from_secs(1)).expect("commit");
    assert_eq!(report.written.len(), 1);
    assert!(report.manifest_written);
    assert_eq!(
        support::read(
            &directory
                .path()
                .join(".agents/skills/spec-kitty-plan/SKILL.md")
        ),
        "body\n"
    );
    let manifest = sk_install::AssetManifest::load(directory.path())
        .expect("manifest")
        .expect("exists");
    let entry = manifest
        .find(".agents/skills/spec-kitty-plan/SKILL.md")
        .expect("entry");
    assert_eq!(entry.agents, vec!["codex"]);
    assert_eq!(entry.installed_at, "2026-11-02T10:00:00.000000+00:00");
    assert_eq!(entry.content_hash, sk_install::content_hash(b"body\n"));

    // Re-planning the same batch is a no-op.
    let again = InstallPlan::plan(
        directory.path(),
        "4.0.0rc7",
        &[asset(
            ".agents/skills/spec-kitty-plan/SKILL.md",
            b"body\n",
            "codex",
        )],
        &options(),
    )
    .expect("plan");
    assert_eq!(again.summary().kept, 1);
    assert!(!again.writes_manifest());
    assert!(again.is_noop());
}

#[test]
fn plan_refuses_unsafe_and_duplicate_destinations() {
    let directory = tempfile::tempdir().expect("temp dir");
    for path in ["../escape.md", "/etc/passwd", "a/../../b.md", "-flag"] {
        let error = InstallPlan::plan(
            directory.path(),
            "v",
            &[asset(path, b"x", "codex")],
            &options(),
        )
        .expect_err("unsafe path refused");
        assert_eq!(error.code(), "unsafe_path", "path {path}");
    }

    // Same destination twice from one agent-with-different-bytes is a conflict.
    let error = InstallPlan::plan(
        directory.path(),
        "v",
        &[
            asset(".agents/skills/spec-kitty-plan/SKILL.md", b"a", "codex"),
            asset(".agents/skills/spec-kitty-plan/SKILL.md", b"b", "codex"),
        ],
        &options(),
    )
    .expect_err("shared content conflict");
    assert_eq!(error.code(), "shared_content_conflict");

    // Identical bytes from several agents is a shared ownership group, not a duplicate.
    let plan = InstallPlan::plan(
        directory.path(),
        "v",
        &[
            asset(".agents/skills/spec-kitty-plan/SKILL.md", b"a", "codex"),
            asset(".agents/skills/spec-kitty-plan/SKILL.md", b"a", "vibe"),
        ],
        &options(),
    )
    .expect("shared ownership");
    assert_eq!(plan.summary().created, 1);
    assert_eq!(plan.assets()[0].owners, vec!["codex", "vibe"]);
}

#[test]
fn a_symlinked_parent_escaping_the_project_is_refused() {
    let directory = tempfile::tempdir().expect("temp dir");
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), directory.path().join(".agents")).expect("symlink");
    let error = InstallPlan::plan(
        directory.path(),
        "v",
        &[asset(
            ".agents/skills/spec-kitty-plan/SKILL.md",
            b"x",
            "codex",
        )],
        &options(),
    )
    .expect_err("escape refused");
    assert_eq!(error.code(), "unsafe_path");
    assert!(!outside.path().join("skills").exists());
}

#[test]
fn a_changed_precondition_aborts_before_writing() {
    let directory = tempfile::tempdir().expect("temp dir");
    support::write(
        &directory
            .path()
            .join(".agents/skills/spec-kitty-plan/SKILL.md"),
        "original\n",
    );
    let plan = InstallPlan::plan(
        directory.path(),
        "v",
        &[asset(
            ".agents/skills/spec-kitty-plan/SKILL.md",
            b"new\n",
            "codex",
        )],
        &options(),
    )
    .expect("plan");
    assert_eq!(plan.assets()[0].action, PlannedAction::Collision);

    // A symlink swap after planning is a precondition change, not a write.
    let target = directory
        .path()
        .join(".agents/skills/spec-kitty-plan/SKILL.md");
    std::fs::remove_file(&target).expect("remove");
    std::os::unix::fs::symlink("/etc/passwd", &target).expect("symlink");
    let error = plan
        .commit(Duration::from_secs(1))
        .expect_err("precondition");
    assert_eq!(error.code(), "precondition_changed");
}

#[test]
fn a_failure_mid_batch_persists_a_partial_manifest() {
    let directory = tempfile::tempdir().expect("temp dir");
    // Two files; the second's directory becomes unwritable after planning, so its
    // atomic write fails while the first has already landed.
    let plan = InstallPlan::plan(
        directory.path(),
        "4.0.0rc7",
        &[
            asset(
                ".agents/skills/spec-kitty-plan/SKILL.md",
                b"plan\n",
                "codex",
            ),
            asset(
                ".agents/skills/spec-kitty-review/SKILL.md",
                b"review\n",
                "codex",
            ),
        ],
        &options(),
    )
    .expect("plan");
    assert_eq!(plan.summary().created, 2);

    let review_dir = directory.path().join(".agents/skills/spec-kitty-review");
    std::fs::create_dir_all(&review_dir).expect("dir");
    let mut permissions = std::fs::metadata(&review_dir)
        .expect("metadata")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o500);
    std::fs::set_permissions(&review_dir, permissions).expect("chmod");

    let result = plan.commit(Duration::from_secs(1));
    // Restore permissions so the temporary directory can be cleaned up.
    let mut permissions = std::fs::metadata(&review_dir)
        .expect("metadata")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o700);
    std::fs::set_permissions(&review_dir, permissions).expect("chmod back");

    let error = result.expect_err("commit fails at the second write");
    assert_eq!(error.code(), "io_failed");

    // The first write landed, and the manifest records exactly that.
    assert!(
        directory
            .path()
            .join(".agents/skills/spec-kitty-plan/SKILL.md")
            .is_file()
    );
    let manifest = sk_install::AssetManifest::load(directory.path())
        .expect("manifest")
        .expect("partial manifest exists");
    assert!(
        manifest
            .find(".agents/skills/spec-kitty-plan/SKILL.md")
            .is_some(),
        "completed write is recorded"
    );
    assert!(
        manifest
            .find(".agents/skills/spec-kitty-review/SKILL.md")
            .is_none(),
        "failed write is not recorded"
    );
    // A retry after the obstruction is removed completes the batch.
    std::fs::remove_dir(&review_dir).expect("remove");
    let retry = InstallPlan::plan(
        directory.path(),
        "4.0.0rc7",
        &[
            asset(
                ".agents/skills/spec-kitty-plan/SKILL.md",
                b"plan\n",
                "codex",
            ),
            asset(
                ".agents/skills/spec-kitty-review/SKILL.md",
                b"review\n",
                "codex",
            ),
        ],
        &options(),
    )
    .expect("re-plan");
    retry.commit(Duration::from_secs(1)).expect("retry commit");
    assert!(
        directory
            .path()
            .join(".agents/skills/spec-kitty-review/SKILL.md")
            .is_file()
    );
}

#[test]
fn observed_kinds_are_classified_without_following_links() {
    let directory = tempfile::tempdir().expect("temp dir");
    let file = directory.path().join("file");
    support::write(&file, "x");
    assert!(matches!(
        sk_install::observe(&file).expect("observe"),
        Observed::File { .. }
    ));
    assert!(matches!(
        sk_install::observe(&directory.path().join("absent")).expect("observe"),
        Observed::Absent
    ));
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(&file, &link).expect("symlink");
    assert!(matches!(
        sk_install::observe(&link).expect("observe"),
        Observed::Symlink { .. }
    ));
}

#[test]
fn a_modified_managed_file_is_updated_only_when_unmodified() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = ".claude/commands/spec-kitty.plan.md";
    let first = InstallPlan::plan(
        directory.path(),
        "4.0.0rc7",
        &[asset(path, b"canonical v1\n", "claude")],
        &options(),
    )
    .expect("plan");
    first.commit(Duration::from_secs(1)).expect("commit");

    // The canonical render moved on: an unmodified managed file is updated.
    let second = InstallPlan::plan(
        directory.path(),
        "4.0.0rc8",
        &[asset(path, b"canonical v2\n", "claude")],
        &options(),
    )
    .expect("plan");
    assert_eq!(second.assets()[0].action, PlannedAction::Update);
    second.commit(Duration::from_secs(1)).expect("commit");
    assert_eq!(
        support::read(&directory.path().join(path)),
        "canonical v2\n"
    );

    // A user edit is preserved, not clobbered.
    support::write(&directory.path().join(path), "my edit\n");
    let third = InstallPlan::plan(
        directory.path(),
        "4.0.0rc9",
        &[asset(path, b"canonical v3\n", "claude")],
        &options(),
    )
    .expect("plan");
    assert_eq!(third.assets()[0].action, PlannedAction::Preserve);
    third.commit(Duration::from_secs(1)).expect("commit");
    assert_eq!(support::read(&directory.path().join(path)), "my edit\n");
}

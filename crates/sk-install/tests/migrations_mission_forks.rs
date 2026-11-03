//! `4.0.0rc8_retire_stale_mission_template_forks` acceptance tests.
//!
//! Stale bytes are the frozen research fork fixture; replacements come from the
//! verified built-in pack in this checkout. Every project is a temporary
//! directory.

use std::path::{Path, PathBuf};

use sk_install::migrations::mission_template_forks as forks;

const TASK_PROMPT: &str = "task-prompt-template.md";
const PLAN_TEMPLATE: &str = "plan-template.md";
const FORK_TASK: &[u8] =
    include_bytes!("fixtures/migrations/forks/research/task-prompt-template.md");
const FORK_PLAN: &[u8] = include_bytes!("fixtures/migrations/forks/research/plan-template.md");

fn pack_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("packs/built-in/missions")
}

fn project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp");
    std::fs::create_dir_all(directory.path().join(".kittify")).unwrap();
    directory
}

fn plant(root: &Path, tree: &str, name: &str, bytes: &[u8]) -> PathBuf {
    let path = root
        .join(".kittify")
        .join(tree)
        .join("research/templates")
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn an_override_stale_copy_is_removed_and_its_directory_kept() {
    let directory = project();
    let root = directory.path();
    let stale = plant(root, "overrides/missions", TASK_PROMPT, FORK_TASK);
    assert!(forks::detect_with_pack_root(root, Some(&pack_root())).unwrap());
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(outcome.changed);
    assert!(!stale.exists());
    assert!(stale.parent().unwrap().is_dir());
    assert!(!forks::detect_with_pack_root(root, Some(&pack_root())).unwrap());
}

#[test]
fn a_legacy_stale_copy_is_replaced_with_the_pack_copy() {
    let directory = project();
    let root = directory.path();
    let legacy = plant(root, "missions", TASK_PROMPT, FORK_TASK);
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(outcome.changed);
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        std::fs::read(pack_root().join("research/templates").join(TASK_PROMPT)).unwrap()
    );
    assert!(outcome.changes[0].starts_with("Replaced stale .kittify/missions/"));
}

#[test]
fn fork_only_names_are_removed_in_both_trees() {
    let directory = project();
    let root = directory.path();
    let legacy = plant(root, "missions", PLAN_TEMPLATE, FORK_PLAN);
    let override_path = plant(root, "overrides/missions", PLAN_TEMPLATE, FORK_PLAN);
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(outcome.changed);
    assert!(!legacy.exists());
    assert!(!override_path.exists());
}

#[test]
fn a_hand_edited_copy_is_kept_and_named() {
    let directory = project();
    let root = directory.path();
    let mut edited = FORK_TASK.to_vec();
    edited.extend_from_slice(b"\nOur team's extra step.\n");
    let path = plant(root, "overrides/missions", TASK_PROMPT, &edited);
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(!outcome.changed);
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    assert_eq!(
        outcome.warnings,
        vec![
            "Kept .kittify/overrides/missions/research/templates/task-prompt-template.md: it differs from every version Spec Kitty shipped, so it is treated as your edit. Delete it to use the template Spec Kitty ships now."
                .to_string()
        ]
    );
}

#[test]
fn unrelated_files_and_pack_identical_copies_are_left_alone() {
    let directory = project();
    let root = directory.path();
    let own = plant(root, "overrides/missions", "my-own-template.md", b"mine\n");
    let current = plant(
        root,
        "overrides",
        TASK_PROMPT,
        &std::fs::read(pack_root().join("research/templates").join(TASK_PROMPT)).unwrap(),
    );
    assert!(!forks::detect_with_pack_root(root, Some(&pack_root())).unwrap());
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(!outcome.changed);
    assert!(outcome.changes.is_empty());
    assert!(outcome.warnings.is_empty());
    assert_eq!(std::fs::read(&own).unwrap(), b"mine\n");
    assert!(current.exists());
}

#[test]
fn crlf_folding_recognises_a_windows_checkout() {
    let directory = project();
    let root = directory.path();
    let crlf = String::from_utf8(FORK_TASK.to_vec())
        .unwrap()
        .replace('\n', "\r\n");
    let path = plant(root, "overrides/missions", TASK_PROMPT, crlf.as_bytes());
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(outcome.changed);
    assert!(!path.exists());
}

#[test]
fn dry_run_reports_without_changing_anything() {
    let directory = project();
    let root = directory.path();
    let legacy = plant(root, "missions", TASK_PROMPT, FORK_TASK);
    let override_path = plant(root, "overrides/missions", TASK_PROMPT, FORK_TASK);
    let outcome = forks::apply_with_pack_root(root, &pack_root(), true).unwrap();
    assert!(
        outcome
            .changes
            .iter()
            .any(|change| change.starts_with("Would replace stale"))
    );
    assert!(
        outcome
            .changes
            .iter()
            .any(|change| change.starts_with("Would remove package-owned"))
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), FORK_TASK);
    assert!(override_path.exists());
}

#[test]
fn a_second_run_is_a_no_op() {
    let directory = project();
    let root = directory.path();
    plant(root, "missions", TASK_PROMPT, FORK_TASK);
    plant(root, "overrides/missions", TASK_PROMPT, FORK_TASK);
    forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(!forks::detect_with_pack_root(root, Some(&pack_root())).unwrap());
    let again = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(!again.changed);
    assert!(again.changes.is_empty() && again.warnings.is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_tree_is_refused_and_the_outside_file_survives() {
    let directory = project();
    let root = directory.path();
    let outside = tempfile::tempdir().expect("outside");
    let target = outside
        .path()
        .join("missions/research/templates")
        .join(TASK_PROMPT);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, FORK_TASK).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join(".kittify/overrides")).unwrap();
    assert!(forks::detect_with_pack_root(root, Some(&pack_root())).unwrap());
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(
        outcome
            .warnings
            .contains(&forks::skipped_tree_warning(".kittify/overrides"))
    );
    assert!(target.exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_template_file_is_kept() {
    let directory = project();
    let root = directory.path();
    let real = root.join("real.md");
    std::fs::write(&real, FORK_TASK).unwrap();
    let link = root
        .join(".kittify/overrides/missions/research/templates")
        .join(TASK_PROMPT);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(link.is_symlink() && real.exists());
    assert_eq!(
        outcome.warnings,
        vec![forks::kept_warning(
            ".kittify/overrides/missions/research/templates/task-prompt-template.md"
        )]
    );
}

#[test]
fn absent_trees_detect_false_and_create_nothing() {
    let directory = project();
    let root = directory.path();
    assert!(!forks::detect_with_pack_root(root, Some(&pack_root())).unwrap());
    let outcome = forks::apply_with_pack_root(root, &pack_root(), false).unwrap();
    assert!(!outcome.changed);
    assert_eq!(
        std::fs::read_dir(root.join(".kittify"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>()
            .len(),
        0
    );
}

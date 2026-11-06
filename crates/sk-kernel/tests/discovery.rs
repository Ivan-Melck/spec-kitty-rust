use std::fs;
use std::path::Path;

use sk_kernel::{KernelError, discover_project_root};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn canonical(path: &Path) -> std::path::PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

fn write_git_head(repo: &Path) {
    fs::create_dir_all(repo.join(".git")).expect("mkdir .git");
    fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
}

#[test]
fn finds_kittify_marker_from_deep_descendant() {
    let temp = temp();
    let root = temp.path().join("project");
    let deep = root.join("a/b/c");
    fs::create_dir_all(root.join(".kittify")).expect("mkdir marker");
    fs::create_dir_all(&deep).expect("mkdir deep");

    assert_eq!(discover_project_root(&deep).unwrap(), canonical(&root));
}

#[test]
fn finds_git_checkout_with_head_but_no_kittify() {
    let temp = temp();
    let root = temp.path().join("checkout");
    let deep = root.join("src/nested");
    fs::create_dir_all(&deep).expect("mkdir deep");
    write_git_head(&root);

    assert_eq!(discover_project_root(&deep).unwrap(), canonical(&root));
}

#[test]
fn stops_at_an_empty_git_directory_instead_of_climbing() {
    let temp = temp();
    let outer = temp.path().join("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(outer.join(".kittify")).expect("outer marker");
    write_git_head(&outer);
    fs::create_dir_all(inner.join(".git")).expect("empty inner .git");

    let error = discover_project_root(&inner).unwrap_err();
    assert!(matches!(error, KernelError::ProjectRootNotFound { .. }));
}

#[test]
fn nested_checkout_wins_over_an_outer_project() {
    let temp = temp();
    let outer = temp.path().join("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(outer.join(".kittify")).expect("outer marker");
    fs::create_dir_all(inner.join("nested")).expect("inner nested");
    write_git_head(&inner);

    assert_eq!(
        discover_project_root(&inner.join("nested")).unwrap(),
        canonical(&inner)
    );
}

#[test]
fn git_directory_takes_precedence_over_kittify_marker_at_same_level() {
    let temp = temp();
    let root = temp.path().join("both");
    fs::create_dir_all(root.join(".kittify")).expect("marker");
    write_git_head(&root);

    assert_eq!(discover_project_root(&root).unwrap(), canonical(&root));
}

#[test]
fn non_worktree_git_file_does_not_stop_the_walk() {
    let temp = temp();
    let outer = temp.path().join("outer");
    let submodule = outer.join("vendor/submodule");
    fs::create_dir_all(outer.join(".kittify")).expect("outer marker");
    fs::create_dir_all(&submodule).expect("submodule dir");
    fs::write(
        submodule.join(".git"),
        "gitdir: ../../.git/modules/submodule\n",
    )
    .expect("write submodule pointer");

    assert_eq!(
        discover_project_root(&submodule).unwrap(),
        canonical(&outer)
    );
}

#[test]
fn worktree_pointer_resolves_to_the_primary_checkout() {
    let temp = temp();
    let primary = temp.path().join("primary");
    let worktree = temp.path().join("worktrees/feature");
    fs::create_dir_all(primary.join(".kittify")).expect("primary marker");
    fs::create_dir_all(primary.join(".git/worktrees/feature")).expect("worktree gitdir");
    fs::create_dir_all(&worktree).expect("worktree dir");
    fs::write(
        worktree.join(".git"),
        format!(
            "gitdir: {}\n",
            primary.join(".git/worktrees/feature").display()
        ),
    )
    .expect("write worktree pointer");

    assert_eq!(
        discover_project_root(&worktree).unwrap(),
        canonical(&primary)
    );
}

#[test]
fn worktree_pointer_requires_the_primary_marker() {
    let temp = temp();
    let primary = temp.path().join("primary");
    let worktree = temp.path().join("worktrees/feature");
    fs::create_dir_all(primary.join(".git/worktrees/feature")).expect("worktree gitdir");
    fs::create_dir_all(&worktree).expect("worktree dir");
    fs::write(
        worktree.join(".git"),
        format!(
            "gitdir: {}\n",
            primary.join(".git/worktrees/feature").display()
        ),
    )
    .expect("write worktree pointer");

    assert!(discover_project_root(&worktree).is_err());
}

#[test]
fn malformed_worktree_pointer_is_ignored() {
    let temp = temp();
    let root = temp.path().join("checkout");
    let start = root.join("nested");
    fs::create_dir_all(root.join(".kittify")).expect("marker");
    fs::create_dir_all(&start).expect("start");
    fs::write(start.join(".git"), "this is not a gitdir pointer\n").expect("malformed pointer");

    assert_eq!(discover_project_root(&start).unwrap(), canonical(&root));
}

#[test]
fn searches_from_a_nonexistent_descendant() {
    let temp = temp();
    let root = temp.path().join("project");
    fs::create_dir_all(root.join(".kittify")).expect("marker");

    assert_eq!(
        discover_project_root(&root.join("does/not/exist/yet")).unwrap(),
        canonical(&root)
    );
}

#[test]
fn broken_kittify_symlink_is_not_a_marker() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("project");
        fs::create_dir_all(&root).expect("root");
        symlink(root.join("missing-target"), root.join(".kittify")).expect("broken marker symlink");

        assert!(discover_project_root(&root).is_err());
    }
}

#[test]
fn reports_start_when_no_root_exists() {
    let temp = temp();
    let start = temp.path().join("nowhere");
    fs::create_dir_all(&start).expect("start");

    let error = discover_project_root(&start).unwrap_err();
    match error {
        KernelError::ProjectRootNotFound { start: reported } => assert_eq!(reported, start),
        other => panic!("unexpected error: {other}"),
    }
}

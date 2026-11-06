use std::fs;
use std::path::{Path, PathBuf};

use sk_kernel::{KernelError, resolve_within};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("canonical path")
}

fn assert_escape(result: Result<PathBuf, KernelError>) {
    match result {
        Err(KernelError::PathEscape { .. }) => {}
        other => panic!("expected PathEscape, got {other:?}"),
    }
}

#[test]
fn resolves_a_regular_relative_path() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("a/b")).expect("tree");

    let resolved = resolve_within(&root, Path::new("a/b/file.txt")).expect("resolve");
    assert_eq!(resolved, canonical(&root).join("a/b/file.txt"));
    assert!(!resolved.exists(), "resolution must accept a missing leaf");
}

#[test]
fn empty_relative_path_resolves_to_the_root() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("root");

    assert_eq!(
        resolve_within(&root, Path::new("")).unwrap(),
        canonical(&root)
    );
}

#[test]
fn dot_components_are_normalized() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("a/b")).expect("tree");

    assert_eq!(
        resolve_within(&root, Path::new("./a/./b/file")).unwrap(),
        canonical(&root).join("a/b/file")
    );
}

#[test]
fn rejects_absolute_paths() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("root");

    let error = resolve_within(&root, Path::new("/etc/passwd")).unwrap_err();
    assert!(matches!(error, KernelError::AbsoluteRelativePath { .. }));
}

#[test]
fn rejects_windows_absolute_path_forms_even_on_posix() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("root");

    for candidate in [
        r"C:\Windows\System32",
        "C:/Windows/System32",
        r"\\server\share\file",
    ] {
        let error = resolve_within(&root, Path::new(candidate)).unwrap_err();
        assert!(
            matches!(error, KernelError::AbsoluteRelativePath { .. }),
            "{candidate}: {error}"
        );
    }
}

#[test]
fn rejects_every_parent_component_even_when_it_stays_inside() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("a/b")).expect("tree");

    for candidate in ["../outside", "a/../../outside", "a/../b", "a/b/.."] {
        let error = resolve_within(&root, Path::new(candidate)).unwrap_err();
        assert!(
            matches!(error, KernelError::ParentTraversal { .. }),
            "{candidate}: {error}"
        );
    }
}

#[test]
fn rejects_missing_or_file_roots() {
    let temp = temp();
    let missing = temp.path().join("missing");
    let error = resolve_within(&missing, Path::new("file")).unwrap_err();
    assert!(matches!(error, KernelError::RootNotDirectory { .. }));

    let file = temp.path().join("file");
    fs::write(&file, b"root").expect("file");
    let error = resolve_within(&file, Path::new("child")).unwrap_err();
    assert!(matches!(error, KernelError::RootNotDirectory { .. }));
}

#[test]
fn root_symlink_is_resolved_before_containment() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let real = temp.path().join("real");
        let linked = temp.path().join("linked");
        fs::create_dir_all(real.join("child")).expect("real tree");
        symlink(&real, &linked).expect("root symlink");

        assert_eq!(
            resolve_within(&linked, Path::new("child/file")).unwrap(),
            canonical(&real).join("child/file")
        );
    }
}

#[test]
fn internal_symlinks_are_allowed_and_canonicalized() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("real")).expect("real dir");
        symlink(root.join("real"), root.join("link")).expect("dir symlink");
        fs::write(root.join("real/file"), b"inside").expect("file");

        let resolved = resolve_within(&root, Path::new("link/file")).unwrap();
        assert_eq!(resolved, canonical(&root.join("real/file")));
    }
}

#[test]
fn symlinked_leaf_is_allowed_when_its_target_is_inside() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).expect("root");
        fs::write(root.join("target"), b"inside").expect("target");
        symlink(root.join("target"), root.join("link")).expect("file symlink");

        assert_eq!(
            resolve_within(&root, Path::new("link")).unwrap(),
            canonical(&root.join("target"))
        );
    }
}

#[test]
fn rejects_directory_symlink_escape() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(outside.join("secret")).expect("outside");
        fs::write(outside.join("secret/file"), b"secret").expect("secret");
        symlink(&outside, root.join("link")).expect("escaping dir symlink");

        assert_escape(resolve_within(&root, Path::new("link/secret/file")));
        assert_escape(resolve_within(&root, Path::new("link/missing/file")));
    }
}

#[test]
fn rejects_file_symlink_escape() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret"), b"secret").expect("secret");
        symlink(outside.join("secret"), root.join("link")).expect("escaping file symlink");

        assert_escape(resolve_within(&root, Path::new("link")));
    }
}

#[test]
fn rejects_broken_symlink_that_points_outside() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).expect("root");
        fs::create_dir_all(&outside).expect("outside");
        symlink(outside.join("not-created-yet"), root.join("link")).expect("broken escaping link");

        assert_escape(resolve_within(&root, Path::new("link")));
        assert_escape(resolve_within(&root, Path::new("link/child")));
    }
}

#[test]
fn rejects_relative_symlink_traversal() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("sub")).expect("root sub");
        fs::create_dir_all(&outside).expect("outside");
        symlink("../../outside", root.join("sub/link")).expect("relative escaping link");

        assert_escape(resolve_within(&root, Path::new("sub/link/file")));
    }
}

#[test]
fn reports_symlink_loops_as_typed_errors() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let temp = temp();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).expect("root");
        symlink(root.join("b"), root.join("a")).expect("a -> b");
        symlink(root.join("a"), root.join("b")).expect("b -> a");

        let error = resolve_within(&root, Path::new("a/file")).unwrap_err();
        assert!(
            matches!(error, KernelError::SymlinkLoop { .. }),
            "expected SymlinkLoop, got {error}"
        );
    }
}

#[test]
fn keeps_a_missing_leaf_lexical_under_the_resolved_root() {
    let temp = temp();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("root");

    let resolved = resolve_within(&root, Path::new("one/two/three")).unwrap();
    assert_eq!(resolved, canonical(&root).join("one/two/three"));
    assert!(resolved.starts_with(canonical(&root)));
}

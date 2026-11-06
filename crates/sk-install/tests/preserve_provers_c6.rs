//! C6/A5 differential: the three cutover ownership provers.
//!
//! Oracle: `tests/fixtures/preserve_provers_c6.json`, recorded by the pinned
//! Python CLI (`tests/fixtures/preserve_provers_c6_probe.py`) over the same
//! shapes. Each case asserts the exact proof (or its absence) and, for the
//! chosen cases, the real guarded-removal verdict, dry run and retry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_install::preserve::{Prover, RemovalVerdict, guard_destructive_removal};

const FIXTURE_REL: &str = "tests/fixtures/preserve_provers_c6.json";

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL);
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source prover oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded prover oracle is JSON")
}

fn sandbox() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sk-c6-provers-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("sandbox");
    path
}

fn write(path: &Path, text: &str) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
    path.to_path_buf()
}

fn proof_of(prover: &Prover, path: &Path, project: &Path) -> (Option<String>, Option<String>) {
    prover
        .prove(path, project)
        .map(|proof| (Some(proof.kind.to_string()), Some(proof.detail)))
        .unwrap_or((None, None))
}

fn expected<'a>(oracle: &'a Value, case: &str) -> &'a Value {
    &oracle["cases"][case]
}

fn assert_proof(oracle: &Value, case: &str, actual: (Option<String>, Option<String>)) {
    let recorded = expected(oracle, case);
    match recorded["outcome"].as_str() {
        Some("proof") => {
            assert_eq!(
                actual.0.as_deref(),
                recorded["kind"].as_str(),
                "{case}: proof kind differs"
            );
            assert_eq!(
                actual.1.as_deref(),
                recorded["reference"].as_str(),
                "{case}: proof reference differs"
            );
        }
        Some("none") => {
            assert_eq!(
                actual.0, None,
                "{case}: native proved a shape Source does not"
            );
            assert_eq!(
                actual.1, None,
                "{case}: native proved a shape Source does not"
            );
        }
        other => panic!("{case}: unexpected recorded outcome {other:?}"),
    }
}

#[test]
fn all_prover_shapes_match_the_recorded_source_projection() {
    let oracle = fixture();
    let root = sandbox();
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project");

    // ---- moved copy -------------------------------------------------------
    let moved = write(&project.join("legacy/old.md"), "same bytes\n");
    let copy = write(&project.join("pack/new.md"), "same bytes\n");
    assert_proof(
        &oracle,
        "moved_copy_identical",
        proof_of(&Prover::MovedCopy { copy: copy.clone() }, &moved, &project),
    );

    let link_source = project.join("legacy/link-old");
    let link_copy = project.join("pack/link-new");
    std::fs::create_dir_all(link_source.parent().expect("parent")).expect("legacy");
    std::fs::create_dir_all(link_copy.parent().expect("parent")).expect("pack");
    #[cfg(unix)]
    std::os::unix::fs::symlink("target-a.md", &link_source).expect("source link");
    #[cfg(unix)]
    std::os::unix::fs::symlink("target-a.md", &link_copy).expect("copy link");
    assert_proof(
        &oracle,
        "moved_copy_symlink_identical",
        proof_of(
            &Prover::MovedCopy {
                copy: link_copy.clone(),
            },
            &link_source,
            &project,
        ),
    );
    let link_other = project.join("pack/link-other");
    #[cfg(unix)]
    std::os::unix::fs::symlink("target-b.md", &link_other).expect("other link");
    assert_proof(
        &oracle,
        "moved_copy_symlink_different_target",
        proof_of(
            &Prover::MovedCopy { copy: link_other },
            &link_source,
            &project,
        ),
    );
    assert_proof(
        &oracle,
        "moved_copy_source_symlink_copy_file",
        proof_of(
            &Prover::MovedCopy {
                copy: moved.clone(),
            },
            &link_source,
            &project,
        ),
    );
    let different = write(&project.join("pack/different.md"), "other bytes\n");
    assert_proof(
        &oracle,
        "moved_copy_different_bytes",
        proof_of(&Prover::MovedCopy { copy: different }, &moved, &project),
    );
    assert_proof(
        &oracle,
        "moved_copy_missing_copy",
        proof_of(
            &Prover::MovedCopy {
                copy: project.join("pack/missing.md"),
            },
            &moved,
            &project,
        ),
    );
    let directory_copy = project.join("pack/dir-copy");
    std::fs::create_dir_all(&directory_copy).expect("dir copy");
    assert_proof(
        &oracle,
        "moved_copy_copy_is_directory",
        proof_of(
            &Prover::MovedCopy {
                copy: directory_copy,
            },
            &moved,
            &project,
        ),
    );

    // ---- empty tree -------------------------------------------------------
    let empty = project.join("legacy/empty");
    std::fs::create_dir_all(&empty).expect("empty");
    assert_proof(
        &oracle,
        "empty_tree_empty",
        proof_of(&Prover::EmptyTree, &empty, &project),
    );
    let nested = project.join("legacy/nested");
    std::fs::create_dir_all(nested.join("a/b/c")).expect("nested");
    assert_proof(
        &oracle,
        "empty_tree_nested_empty",
        proof_of(&Prover::EmptyTree, &nested, &project),
    );
    let with_file = project.join("legacy/with-file");
    write(&with_file.join("sub/keep.txt"), "x\n");
    assert_proof(
        &oracle,
        "empty_tree_with_file",
        proof_of(&Prover::EmptyTree, &with_file, &project),
    );
    let with_link = project.join("legacy/with-link");
    std::fs::create_dir_all(&with_link).expect("with link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&nested, with_link.join("sub")).expect("symlink");
    assert_proof(
        &oracle,
        "empty_tree_with_symlinked_dir",
        proof_of(&Prover::EmptyTree, &with_link, &project),
    );
    let plain = write(&project.join("legacy/plain.md"), "x\n");
    assert_proof(
        &oracle,
        "empty_tree_file_path",
        proof_of(&Prover::EmptyTree, &plain, &project),
    );
    let link_root = project.join("legacy/link-root");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&empty, &link_root).expect("root link");
    assert_proof(
        &oracle,
        "empty_tree_symlink_root",
        proof_of(&Prover::EmptyTree, &link_root, &project),
    );

    // ---- frozen tree hashes ----------------------------------------------
    let tree = project.join("legacy/skill");
    write(&tree.join("SKILL.md"), "skill body\n");
    write(&tree.join("references/notes.md"), "notes\n");
    let mut hashes = BTreeMap::new();
    for (member, text) in [
        ("SKILL.md", "skill body\n"),
        ("references/notes.md", "notes\n"),
    ] {
        hashes.insert(
            member.to_string(),
            sk_install::manifest::content_hash(text.as_bytes()),
        );
    }
    assert_proof(
        &oracle,
        "tree_hashes_exact",
        proof_of(&Prover::TreeHashes(hashes.clone()), &tree, &project),
    );
    let extra = project.join("legacy/skill-extra");
    write(&extra.join("SKILL.md"), "skill body\n");
    write(&extra.join("references/notes.md"), "notes\n");
    write(&extra.join("extra.md"), "extra\n");
    assert_proof(
        &oracle,
        "tree_hashes_extra_file",
        proof_of(&Prover::TreeHashes(hashes.clone()), &extra, &project),
    );
    let missing = project.join("legacy/skill-missing");
    write(&missing.join("SKILL.md"), "skill body\n");
    assert_proof(
        &oracle,
        "tree_hashes_missing_file",
        proof_of(&Prover::TreeHashes(hashes.clone()), &missing, &project),
    );
    let changed = project.join("legacy/skill-changed");
    write(&changed.join("SKILL.md"), "changed body\n");
    write(&changed.join("references/notes.md"), "notes\n");
    assert_proof(
        &oracle,
        "tree_hashes_changed_bytes",
        proof_of(&Prover::TreeHashes(hashes.clone()), &changed, &project),
    );
    let linked = project.join("legacy/skill-link");
    write(&linked.join("SKILL.md"), "skill body\n");
    write(&linked.join("references/notes.md"), "notes\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink("SKILL.md", linked.join("alias.md")).expect("member link");
    assert_proof(
        &oracle,
        "tree_hashes_symlink_member",
        proof_of(&Prover::TreeHashes(hashes.clone()), &linked, &project),
    );
    assert_proof(
        &oracle,
        "tree_hashes_empty_map",
        proof_of(&Prover::TreeHashes(BTreeMap::new()), &tree, &project),
    );
    let root_link = project.join("legacy/skill-root-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&tree, &root_link).expect("root link");
    assert_proof(
        &oracle,
        "tree_hashes_symlink_root",
        proof_of(&Prover::TreeHashes(hashes.clone()), &root_link, &project),
    );
    assert_proof(
        &oracle,
        "tree_hashes_file_path",
        proof_of(&Prover::TreeHashes(hashes), &plain, &project),
    );
}

#[test]
fn guarded_removal_dry_run_remove_and_retry_match_the_recorded_source() {
    let oracle = fixture();
    let root = sandbox();
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project");

    // moved copy: dry run removes nothing, the real call removes, the retry is absent
    let candidate = write(&project.join("legacy/guard-me.md"), "guard bytes\n");
    let copy = write(&project.join("pack/guard-copy.md"), "guard bytes\n");
    let prover = Prover::MovedCopy { copy };
    let dry = guard_destructive_removal(&candidate, &project, &prover, false, None, true)
        .expect("dry run");
    match &dry {
        RemovalVerdict::Owned { proof } => {
            assert_eq!(proof.kind, "canonical_content");
            assert_eq!(proof.detail, "moved-copy:legacy/guard-me.md");
        }
        other => panic!("unexpected dry-run verdict: {other:?}"),
    }
    assert!(candidate.is_file(), "a dry run removed the candidate");
    let removed = guard_destructive_removal(&candidate, &project, &prover, false, None, false)
        .expect("remove");
    assert!(matches!(removed, RemovalVerdict::Owned { .. }));
    assert!(!candidate.exists(), "the proven candidate survived");
    let retry = guard_destructive_removal(&candidate, &project, &prover, false, None, false)
        .expect("retry");
    assert_eq!(retry, RemovalVerdict::Absent);
    // an unproven candidate is preserved in place (name is not ownership)
    let kept = write(&project.join("legacy/keep-me.md"), "mine\n");
    let unproven = guard_destructive_removal(
        &kept,
        &project,
        &Prover::MovedCopy {
            copy: project.join("pack/guard-copy.md"),
        },
        false,
        None,
        true,
    )
    .expect("preserve");
    assert!(matches!(unproven, RemovalVerdict::Preserved { .. }));
    assert!(kept.is_file());

    // empty tree: same dry-run/remove/retry behaviour
    let empty = project.join("legacy/guard-empty/inner");
    std::fs::create_dir_all(&empty).expect("empty");
    let guard_root = project.join("legacy/guard-empty");
    let dry =
        guard_destructive_removal(&guard_root, &project, &Prover::EmptyTree, true, None, true)
            .expect("dry run");
    match &dry {
        RemovalVerdict::Owned { proof } => {
            assert_eq!(proof.kind, "managed_path");
            assert_eq!(proof.detail, "emptied-legacy-root:legacy/guard-empty");
        }
        other => panic!("unexpected empty-tree dry run: {other:?}"),
    }
    assert!(guard_root.is_dir());
    assert!(matches!(
        guard_destructive_removal(&guard_root, &project, &Prover::EmptyTree, true, None, false)
            .expect("remove"),
        RemovalVerdict::Owned { .. }
    ));
    assert!(!guard_root.exists());
    assert_eq!(
        guard_destructive_removal(&guard_root, &project, &Prover::EmptyTree, true, None, false)
            .expect("retry"),
        RemovalVerdict::Absent
    );

    // frozen tree hashes: a shipped-skill copy proves and is removed
    let skill = project.join("legacy/guard-skill");
    write(&skill.join("SKILL.md"), "skill body\n");
    write(&skill.join("references/notes.md"), "notes\n");
    let mut hashes = BTreeMap::new();
    hashes.insert(
        "SKILL.md".to_string(),
        sk_install::manifest::content_hash(b"skill body\n"),
    );
    hashes.insert(
        "references/notes.md".to_string(),
        sk_install::manifest::content_hash(b"notes\n"),
    );
    let prover = Prover::TreeHashes(hashes);
    let dry =
        guard_destructive_removal(&skill, &project, &prover, true, None, true).expect("dry run");
    match &dry {
        RemovalVerdict::Owned { proof } => {
            assert_eq!(proof.kind, "canonical_content");
            assert_eq!(proof.detail, "shipped-skill-hash:legacy/guard-skill");
        }
        other => panic!("unexpected tree-hash dry run: {other:?}"),
    }
    assert!(skill.is_dir());
    assert!(matches!(
        guard_destructive_removal(&skill, &project, &prover, true, None, false).expect("remove"),
        RemovalVerdict::Owned { .. }
    ));
    assert!(!skill.exists());
    assert_eq!(
        guard_destructive_removal(&skill, &project, &prover, true, None, false).expect("retry"),
        RemovalVerdict::Absent
    );
    assert_eq!(oracle["assertions"], "all passed");
}

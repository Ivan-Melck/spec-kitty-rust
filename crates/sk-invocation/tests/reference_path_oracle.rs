//! Live, lock-exact oracle vectors for reference-path resolution.
//!
//! `normalise_ref` is the reference's `writer.normalise_ref`, which calls
//! `kernel.resolution.resolve_rejecting_loops` (Python's non-strict
//! `Path.resolve()` with a loop probe). These cases are applied to the real
//! pinned module and to this workspace, so the native resolution authority is
//! compared on the shapes where a strict, budgeted resolver diverges:
//! 41/42+-link acyclic chains, real loops, unreadable intermediates,
//! dangling/missing links, `..` applied after expansion, finite repeated
//! links, absolute/relative/URL refs and an embedded NUL.
//!
//! The oracle is required, never skipped: a missing interpreter or drifted
//! pin fails the test instead of producing an empty comparison.

mod support;

use std::fs;
use std::path::Path;

use serde_json::json;
use sk_invocation::normalise_ref;

use support::run_python;

const NORMALISE_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.invocation.writer import normalise_ref

payload = json.load(sys.stdin)
root = Path(payload["root"])
json.dump([normalise_ref(ref, root) for ref in payload["refs"]], sys.stdout)
"#;

fn oracle_normalise(root: &Path, refs: &[String]) -> Vec<String> {
    let request = json!({"root": root.to_string_lossy(), "refs": refs});
    serde_json::from_str(&run_python(NORMALISE_SCRIPT, Some(&request.to_string())))
        .expect("the oracle emitted one result per ref")
}

fn native_normalise(root: &Path, refs: &[String]) -> Vec<String> {
    refs.iter()
        .map(|reference| normalise_ref(reference, root))
        .collect()
}

fn assert_parity(root: &Path, refs: &[String]) {
    let oracle = oracle_normalise(root, refs);
    let native = native_normalise(root, refs);
    assert_eq!(
        native.len(),
        refs.len(),
        "one native result per case is required"
    );
    for (index, reference) in refs.iter().enumerate() {
        assert_eq!(
            native.get(index).map(String::as_str),
            oracle.get(index).map(String::as_str),
            "ref {reference:?} diverged from the reference"
        );
    }
}

const RAW_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from kernel.resolution import resolve_rejecting_loops

payload = json.load(sys.stdin)
out = []
for raw in payload["paths"]:
    try:
        out.append("ok:" + str(resolve_rejecting_loops(Path(raw))))
    except OSError:
        out.append("err:OSError")
    except ValueError:
        out.append("err:ValueError")
json.dump(out, sys.stdout)
"#;

/// Compare the kernel reference-path authority itself, not just its use in
/// `normalise_ref`: Python `resolve_rejecting_loops` from the pinned module.
fn assert_raw_parity(repo: &Path, refs: &[String]) {
    let paths: Vec<String> = refs
        .iter()
        .map(|reference| {
            let path = Path::new(reference);
            let candidate = if path.is_absolute() {
                path.to_path_buf()
            } else {
                repo.join(path)
            };
            candidate.to_string_lossy().into_owned()
        })
        .collect();
    let request = json!({"paths": paths});
    let oracle: Vec<String> =
        serde_json::from_str(&run_python(RAW_SCRIPT, Some(&request.to_string())))
            .expect("the oracle emitted one outcome per path");
    for (index, raw) in paths.iter().enumerate() {
        let native = match sk_kernel::resolve_reference_path(Path::new(raw)) {
            Ok(resolved) => format!("ok:{}", resolved.display()),
            Err(sk_kernel::ReferencePathError::InteriorNul { .. }) => "err:ValueError".to_owned(),
            Err(_) => "err:OSError".to_owned(),
        };
        assert_eq!(
            native,
            oracle.get(index).cloned().unwrap_or_default(),
            "path {raw:?} diverged from resolve_rejecting_loops"
        );
    }
}

#[test]
fn url_and_missing_refs_match_the_oracle_without_symlinks() {
    let temp = tempfile::tempdir().expect("isolated project");
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("docs")).unwrap();
    fs::write(repo.join("docs/present.md"), "x").unwrap();

    let refs: Vec<String> = [
        "https://example.com/x".to_owned(),
        "file:///tmp/x".to_owned(),
        "s3://bucket/key".to_owned(),
        "C:/x".to_owned(),
        "docs/present.md".to_owned(),
        "docs/missing.md".to_owned(),
        "no/such/tree".to_owned(),
        ".".to_owned(),
        repo.join("docs/present.md").to_string_lossy().into_owned(),
        temp.path()
            .join("outside.log")
            .to_string_lossy()
            .into_owned(),
        "embedded\u{0}nul".to_owned(),
    ]
    .into_iter()
    .collect();
    assert_parity(&repo, &refs);
}

#[cfg(unix)]
#[test]
fn symlinked_reference_paths_match_the_lock_exact_oracle() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().expect("isolated project");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("target.txt"), "x").unwrap();
    fs::create_dir_all(repo.join("target-dir")).unwrap();
    fs::write(repo.join("target-dir/leaf.md"), "x").unwrap();

    // 41- and 42-link (and a longer 200-link) acyclic chain: the reference
    // has no total-link cap, only loop detection, so every one must resolve.
    // The containment resolver's 40-link budget must not leak into this
    // reference-path profile.
    for (count, name) in [(41_usize, "chain41"), (42, "chain42"), (200, "chain200")] {
        let link = |index: usize| repo.join(format!("{name}-{index:05}"));
        symlink("target.txt", link(count - 1)).unwrap();
        for index in (0..count - 1).rev() {
            let next = format!("{name}-{:05}", index + 1);
            symlink(&next, link(index)).unwrap();
        }
    }

    // Real loops: a two-link cycle and a self-link must be refused verbatim.
    symlink("loop-b", repo.join("loop-a")).unwrap();
    symlink("loop-a", repo.join("loop-b")).unwrap();
    symlink("self", repo.join("self")).unwrap();

    // `..` is applied after the link is expanded (the parent of the target).
    symlink("target-dir", repo.join("dir-link")).unwrap();

    // A dangling link resolves to its (missing) target.
    symlink("dangling-target.md", repo.join("dangling-link")).unwrap();

    // Finite repeated link reuse: the same link twice, not a cycle.
    symlink("target-dir", repo.join("reused")).unwrap();

    // An unreadable intermediate directory: non-strict resolution tolerates
    // EACCES and keeps the tail lexically. A strict resolver would fail here.
    fs::create_dir_all(repo.join("locked")).unwrap();
    fs::write(repo.join("locked/hidden.md"), "x").unwrap();
    let mut locked = fs::metadata(repo.join("locked")).unwrap().permissions();
    locked.set_mode(0o000);
    fs::set_permissions(repo.join("locked"), locked).unwrap();
    let permission_bypass = fs::read_dir(repo.join("locked")).is_ok();

    let mut refs: Vec<String> = [
        "chain41-00000".to_owned(),
        "chain42-00000".to_owned(),
        "chain200-00000".to_owned(),
        "loop-a".to_owned(),
        "loop-a/tail".to_owned(),
        "self".to_owned(),
        "dir-link/leaf.md".to_owned(),
        "dir-link/../target.txt".to_owned(),
        "dangling-link".to_owned(),
        "reused/../reused/leaf.md".to_owned(),
        "locked/hidden.md".to_owned(),
    ]
    .into_iter()
    .collect();
    if permission_bypass {
        // Running with permission-bypass privileges: the EACCES shape cannot
        // be produced here, so compare the rest and say so explicitly.
        eprintln!("running with permission-bypass privileges; EACCES case not discriminating");
        refs.retain(|reference| reference != "locked/hidden.md");
    }
    assert_parity(&repo, &refs);
    assert_raw_parity(&repo, &refs);

    if !permission_bypass {
        // The native side must have tolerated the unreadable intermediate
        // (not returned the ref verbatim through a strict error).
        assert_eq!(
            normalise_ref("locked/hidden.md", &repo),
            "locked/hidden.md",
            "EACCES must be tolerated lexically, not refused"
        );
    }

    let mut restore = fs::metadata(repo.join("locked")).unwrap().permissions();
    restore.set_mode(0o700);
    fs::set_permissions(repo.join("locked"), restore).unwrap();
}
